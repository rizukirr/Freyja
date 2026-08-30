//! Shared HTTP execution and request serialization.

use crate::endpoint::{Auth, EndpointConfig};
use crate::error::Error;
use crate::model::GenerateRequest;
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

pub(crate) fn default_http() -> reqwest::Client {
    reqwest::Client::builder()
        .read_timeout(DEFAULT_TIMEOUT)
        .build()
        .expect("the TLS backend could not be initialized")
}

/// The header name the endpoint's auth would set, if it sets one.
fn auth_header_name(auth: &Auth) -> Option<&'static str> {
    match auth {
        Auth::Bearer => Some("authorization"),
        Auth::Header(name) => Some(*name),
        Auth::Query(_) | Auth::None => None,
    }
}

/// The dialect's required headers and the endpoint's extra ones, resolved.
///
/// `reqwest`'s `header` appends rather than replaces, so a name written by two
/// layers goes out twice and a server rejecting the request says nothing about
/// which copy it read. Three layers can collide: a required header, an extra
/// header, and auth.
///
/// Later wins, so an endpoint pinned to a different `anthropic-version` can
/// say so and a second `header` call with the same name supersedes the first.
/// Auth outranks both and is applied by the caller rather than folded in here,
/// so `bearer_auth` keeps marking the credential sensitive.
fn resolved_headers<'a>(
    config: &'a EndpointConfig,
    api_key: Option<&str>,
) -> Vec<(&'a str, &'a str)> {
    let reserved = api_key.and(auth_header_name(&config.auth));

    let mut all: Vec<(&str, &str)> = config
        .dialect
        .required_headers()
        .iter()
        .map(|(name, value)| (*name, *value))
        .chain(
            config
                .extra_headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str())),
        )
        .filter(|(name, _)| !reserved.is_some_and(|taken| name.eq_ignore_ascii_case(taken)))
        .collect();

    let mut kept: Vec<(&str, &str)> = Vec::with_capacity(all.len());
    while let Some((name, value)) = all.pop() {
        if !kept.iter().any(|(seen, _)| seen.eq_ignore_ascii_case(name)) {
            kept.push((name, value));
        }
    }
    kept.reverse();
    kept
}

/// Applies the endpoint's headers and its credential to a request builder.
///
/// Separate from `post` so a test can read the request that comes out. The
/// only property worth reading is one `post` cannot expose: `bearer_auth`
/// marks the credential sensitive, and a hand-built `Authorization` header
/// would silently not.
fn apply_headers(
    mut post: reqwest::RequestBuilder,
    config: &EndpointConfig,
    api_key: Option<&str>,
) -> reqwest::RequestBuilder {
    for (name, value) in resolved_headers(config, api_key) {
        post = post.header(name, value);
    }
    if let Some(key) = api_key {
        post = match config.auth {
            Auth::Bearer => post.bearer_auth(key),
            Auth::Header(name) => post.header(name, key),
            Auth::Query(_) | Auth::None => post,
        };
    }
    post
}

/// Adds the credential to the URL when the endpoint carries it there.
///
/// Applied at send time rather than in `EndpointConfig::build_url`, because
/// `url()` is public and is what a caller reaches for to print where requests
/// go. A public method returning a live key is the shape two earlier fixes
/// undid: safe where people rarely look, exposed where they always do.
///
/// An existing parameter of the same name is dropped first, so auth wins a
/// collision exactly as `resolved_headers` makes it win one between headers.
///
/// A URL that will not parse is returned untouched, which leaves it to fail at
/// send time the way `EndpointConfig::build_url`'s own fallback does.
fn apply_query_auth(url: String, config: &EndpointConfig, api_key: Option<&str>) -> String {
    let (Auth::Query(name), Some(key)) = (&config.auth, api_key) else {
        return url;
    };
    let Ok(mut parsed) = reqwest::Url::parse(&url) else {
        return url;
    };

    let kept: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(existing, _)| existing.as_ref() != *name)
        .map(|(existing, value)| (existing.into_owned(), value.into_owned()))
        .collect();
    parsed
        .query_pairs_mut()
        .clear()
        .extend_pairs(kept)
        .append_pair(name, key);

    parsed.into()
}

pub(crate) async fn post<T: Serialize>(
    http: &reqwest::Client,
    config: &EndpointConfig,
    api_key: Option<&str>,
    url: String,
    wire: &T,
) -> Result<reqwest::Response, Error> {
    let url = apply_query_auth(url, config, api_key);
    let post = apply_headers(http.post(url), config, api_key);

    post.json(wire)
        .send()
        .await
        .map_err(|error| Error::transport(config.name.clone(), &error))
}

/// The most one response body may occupy before the read is abandoned.
///
/// Generously above any real generation, and there to bound a body Freyja has
/// no other bound on: `read_timeout` limits silence, not volume, so an endpoint
/// that keeps sending forever is never late. The same reasoning as
/// `stream::sse::MAX_FRAME_BYTES`, one layer up.
pub(crate) const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

/// Reads a whole response body, refusing to grow past [`MAX_BODY_BYTES`].
///
/// Stands in for `Response::text`, which has no ceiling. JSON is UTF-8 by
/// specification, so the charset negotiation `text` does buys nothing here.
pub(crate) async fn read_body(
    response: reqwest::Response,
    endpoint: &std::sync::Arc<str>,
) -> Result<String, Error> {
    let mut response = response;
    let mut body: Vec<u8> = Vec::new();

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| Error::transport(endpoint.clone(), &error))?
    {
        if body.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(Error::InvalidResponse {
                endpoint: endpoint.clone(),
                message: format!("response body grew past {MAX_BODY_BYTES} bytes"),
            });
        }
        body.extend_from_slice(&chunk);
    }

    Ok(String::from_utf8_lossy(&body).into_owned())
}

pub(crate) fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

pub(crate) fn to_value<T: Serialize>(
    wire: &T,
    request: &GenerateRequest,
    config: &EndpointConfig,
) -> Result<Value, Error> {
    let invalid = |error: serde_json::Error| Error::InvalidRequest {
        endpoint: config.name.clone(),
        message: error.to_string(),
    };

    let mut body = match serde_json::to_value(wire).map_err(invalid)? {
        Value::Object(body) => body,
        other => {
            return Err(Error::InvalidRequest {
                endpoint: config.name.clone(),
                message: format!("request body must be a JSON object, built {other}"),
            });
        }
    };

    crate::model::merge_into(&mut body, &config.extra_body);
    for (dialect, fields) in &request.extra {
        if *dialect == config.dialect {
            crate::model::merge_into(&mut body, fields);
        }
    }

    Ok(Value::Object(body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::Dialect;

    #[test]
    fn the_key_sets_a_sensitive_authorization_header() {
        let config = EndpointConfig::new(Dialect::OpenAiChat, "test", "https://x.test");
        let request = apply_headers(
            default_http().post("https://x.test"),
            &config,
            Some("sk-test"),
        )
        .build()
        .expect("the request builds");

        let value = request
            .headers()
            .get(reqwest::header::AUTHORIZATION)
            .expect("a key was supplied, so auth set a header");
        assert!(
            value.is_sensitive(),
            "the credential must stay out of anything that prints headers"
        );
    }

    #[test]
    fn a_caller_header_is_not_marked_sensitive() {
        // The negative case, so the assertion above cannot pass by every
        // header being sensitive.
        let config = EndpointConfig::new(Dialect::OpenAiChat, "test", "https://x.test")
            .header("X-Route", "eu");
        let request = apply_headers(default_http().post("https://x.test"), &config, None)
            .build()
            .expect("the request builds");

        let value = request
            .headers()
            .get("x-route")
            .expect("the extra header was applied");
        assert!(!value.is_sensitive(), "{value:?}");
    }
}
