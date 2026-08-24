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

pub(crate) async fn post<T: Serialize>(
    http: &reqwest::Client,
    config: &EndpointConfig,
    api_key: Option<&str>,
    url: String,
    wire: &T,
) -> Result<reqwest::Response, Error> {
    let mut post = http.post(url);
    for (name, value) in config.dialect.required_headers() {
        post = post.header(*name, *value);
    }
    for (name, value) in &config.extra_headers {
        post = post.header(name, value);
    }
    if let Some(key) = api_key {
        post = match config.auth {
            Auth::Bearer => post.bearer_auth(key),
            Auth::Header(name) => post.header(name, key),
            Auth::None => post,
        };
    }

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
