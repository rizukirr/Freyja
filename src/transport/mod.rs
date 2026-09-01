//! Shared HTTP execution and request serialization.

use crate::endpoint::{Auth, EndpointConfig};
use crate::error::Error;
use crate::model::GenerateRequest;
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// How many same-origin redirects one request may follow.
///
/// Redirects between two paths on one endpoint are ordinary. A chain of them
/// is a loop, and `reqwest`'s own default stops at ten for the same reason.
const MAX_REDIRECTS: usize = 10;

/// Whether two URLs are the same origin, by `reqwest`'s own definition.
///
/// Deliberately the same rule `reqwest::redirect::remove_sensitive_headers`
/// uses to decide whether to strip `Authorization`, so the hop Freyja refuses
/// and the hop `reqwest` strips for are the same hop. Two rules here would
/// mean a gap between them, and the gap is where a credential goes.
fn same_origin(a: &reqwest::Url, b: &reqwest::Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

/// A redirect policy that will not carry the credential to another origin.
///
/// `reqwest` strips `Authorization`, `Cookie`, `Proxy-Authorization` and
/// `WWW-Authenticate` when a redirect crosses an origin, and it cannot strip
/// what it cannot recognize. [`Auth::Header`] puts the key in `x-api-key` or
/// `x-goog-api-key`, which are ordinary headers as far as `reqwest` can tell,
/// so `reqwest` forwards them across an origin and the credential travels
/// with the redirect.
///
/// Refusing the hop rather than stripping the header is the choice here. A
/// stripped credential produces a 401 from a host the caller never named,
/// which is a worse thing to debug than a redirect that says it was refused.
///
/// A caller who supplies their own client through
/// [`crate::Client::with_http_client`] gets `reqwest`'s default policy back,
/// which is documented beside that constructor.
fn same_origin_redirects() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        let Some(previous) = attempt.previous().last() else {
            return attempt.follow();
        };
        if !same_origin(previous, attempt.url()) {
            return attempt.stop();
        }
        if attempt.previous().len() >= MAX_REDIRECTS {
            return attempt.error("too many redirects");
        }
        attempt.follow()
    })
}

pub(crate) fn default_http() -> reqwest::Client {
    reqwest::Client::builder()
        .read_timeout(DEFAULT_TIMEOUT)
        .redirect(same_origin_redirects())
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

/// A header value marked sensitive, so it stays out of anything that prints a
/// `HeaderMap` and out of HTTP/2's compression table.
///
/// `reqwest::bearer_auth` marks only its own header sensitive. The other two
/// ways a credential reaches a header do not, and two of the three shipped
/// presets take their key through [`Auth::Header`], so this applies the flag
/// uniformly.
///
/// What it buys is two things. Middleware over a caller-supplied
/// `reqwest::Client` prints `Sensitive` rather than the key, and that is the
/// documented way to add tracing or a proxy recorder. And an HPACK encoder
/// sends the value literal rather than indexing it into the dynamic table,
/// where it would sit for the life of the connection.
///
/// A value `HeaderValue` refuses is handed back untouched, so it keeps failing
/// at `send` the way it always has rather than being swallowed here.
fn sensitive(value: &str) -> Option<reqwest::header::HeaderValue> {
    let mut header = reqwest::header::HeaderValue::from_str(value).ok()?;
    header.set_sensitive(true);
    Some(header)
}

/// Applies the endpoint's headers and its credential to a request builder.
///
/// Separate from `post` so a test can read the request that comes out. The
/// only property worth reading is one `post` cannot expose: whether the
/// credential went out marked sensitive.
///
/// Classification is [`EndpointConfig::is_secret_header`], the same predicate
/// that type's `Debug` and the error redaction use, so a value cannot be
/// withheld in what Freyja prints and exposed in what it sends.
fn apply_headers(
    mut post: reqwest::RequestBuilder,
    config: &EndpointConfig,
    api_key: Option<&str>,
) -> reqwest::RequestBuilder {
    for (name, value) in resolved_headers(config, api_key) {
        post = match config
            .is_secret_header(name)
            .then(|| sensitive(value))
            .flatten()
        {
            Some(header) => post.header(name, header),
            None => post.header(name, value),
        };
    }
    if let Some(key) = api_key {
        post = match config.auth {
            Auth::Bearer => post.bearer_auth(key),
            Auth::Header(name) => match sensitive(key) {
                Some(header) => post.header(name, header),
                None => post.header(name, key),
            },
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
        .map_err(|error| Error::transport(config.name.clone(), &error, Some(config)))
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
        .map_err(|error| Error::transport(endpoint.clone(), &error, None))?
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

/// Reads `Retry-After`, clamped to [`crate::error::MAX_RETRY_AFTER`].
///
/// Only the `delay-seconds` form of RFC 9110 §10.2.3 is read. The `HTTP-date`
/// form yields `None`, which callers already handle as "use your own backoff",
/// so an endpoint sending a date costs a hint rather than causing a failure.
///
/// The clamp is the part that matters. This is a number an endpoint hands the
/// caller expecting them to sleep for it, and the pattern in
/// `docs/reference/errors.md` does exactly that, so an unclamped
/// `Retry-After: 99999999999` parks a task for three thousand years. Freyja
/// already refuses a body and a stream that grow without bound, on the stated
/// grounds that it is pointed at gateways it has never met. This is the same
/// argument applied to the one untrusted quantity that is not bytes.
pub(crate) fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let seconds = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;

    Some(Duration::from_secs(seconds).min(crate::error::MAX_RETRY_AFTER))
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
    fn the_key_is_sensitive_under_every_auth_style_that_uses_a_header() {
        // Looped rather than written once for `Bearer`, which is how the gap
        // survived: `bearer_auth` set the flag, `header` did not, and two of
        // the three shipped presets take their key the second way.
        for (auth, header) in [
            (Auth::Bearer, "authorization"),
            (Auth::Header("x-api-key"), "x-api-key"),
            (Auth::Header("x-goog-api-key"), "x-goog-api-key"),
        ] {
            let config =
                EndpointConfig::new(Dialect::OpenAiChat, "test", "https://x.test").auth(auth);
            let request = apply_headers(
                default_http().post("https://x.test"),
                &config,
                Some("sk-test"),
            )
            .build()
            .expect("the request builds");

            let value = request
                .headers()
                .get(header)
                .unwrap_or_else(|| panic!("a key was supplied, so auth set {header}"));
            assert!(
                value.is_sensitive(),
                "the credential must stay out of anything that prints headers: {header}"
            );
        }
    }

    #[test]
    fn a_classified_extra_header_is_sensitive_too() {
        // A second credential goes out through `header`, which never marked
        // anything, so classifying it changed what Freyja printed and not what
        // it sent.
        let config = EndpointConfig::new(Dialect::OpenAiChat, "test", "https://x.test")
            .secret_header("x-acme-passport", "pp-live");
        let request = apply_headers(default_http().post("https://x.test"), &config, None)
            .build()
            .expect("the request builds");

        let value = request
            .headers()
            .get("x-acme-passport")
            .expect("the extra header was applied");
        assert!(value.is_sensitive(), "{value:?}");
        // And it is still the value the endpoint needs, not a redacted one.
        assert_eq!(
            request
                .headers()
                .get("x-acme-passport")
                .map(|value| value.as_bytes()),
            Some(&b"pp-live"[..])
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

    /// The policy is a predicate over two URLs, so it is tested as one. The
    /// hop that mattered is a port change, which is what the measured leak
    /// used and what `reqwest` treats as cross-origin.
    #[test]
    fn a_redirect_may_not_change_origin() {
        let parse = |url: &str| reqwest::Url::parse(url).expect("a valid url");
        let base = parse("https://api.acme.test/v1/messages");

        for allowed in [
            "https://api.acme.test/v2/messages",
            "https://api.acme.test/v1/messages/",
        ] {
            assert!(same_origin(&base, &parse(allowed)), "{allowed}");
        }

        for refused in [
            "https://attacker.test/v1/messages",
            "https://api.acme.test:8443/v1/messages",
            "http://api.acme.test/v1/messages",
        ] {
            assert!(!same_origin(&base, &parse(refused)), "{refused}");
        }
    }

    #[test]
    fn the_default_client_carries_the_policy() {
        // Cheap, and it is the wiring that actually protects anything: the
        // predicate above is correct whether or not `default_http` uses it.
        let rendered = format!("{:?}", default_http());
        assert!(rendered.contains("redirect"), "{rendered}");
    }

    #[test]
    fn a_retry_after_is_clamped() {
        let header = |value: &str| {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(
                reqwest::header::RETRY_AFTER,
                reqwest::header::HeaderValue::from_str(value).expect("a valid value"),
            );
            parse_retry_after(&headers)
        };

        // The ordinary case is untouched.
        assert_eq!(header("30"), Some(Duration::from_secs(30)));
        assert_eq!(header(" 30 "), Some(Duration::from_secs(30)));
        assert_eq!(header("0"), Some(Duration::ZERO));

        // An endpoint cannot choose how long the caller sleeps.
        assert_eq!(header("99999999999"), Some(crate::error::MAX_RETRY_AFTER));
        assert_eq!(
            header("18446744073709551615"),
            Some(crate::error::MAX_RETRY_AFTER)
        );

        // The forms RFC 9110 allows that this does not read, and the ones it
        // does not allow at all, are all `None` rather than a wrong duration.
        for unread in ["Wed, 21 Oct 2015 07:28:00 GMT", "1.5", "-1", "soon", ""] {
            assert_eq!(header(unread), None, "{unread}");
        }
        assert_eq!(parse_retry_after(&reqwest::header::HeaderMap::new()), None);
    }
}
