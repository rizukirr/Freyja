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
