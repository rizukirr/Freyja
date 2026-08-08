//! Anthropic backend, speaking the Messages API.

mod types;

use crate::provider::{GenerateRequest, GenerateResponse, Provider, ProviderError};

const MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";
const PROVIDER: &str = "Anthropic";

pub(crate) struct AnthropicProvider;

impl Provider for AnthropicProvider {
    async fn generate(
        &self,
        http: &reqwest::Client,
        api_key: &str,
        request: &GenerateRequest,
    ) -> Result<GenerateResponse, ProviderError> {
        let wire_request = types::Request::try_from(request)?;

        let response = http
            .post(MESSAGES_URL)
            .header("x-api-key", api_key)
            .header("anthropic-version", API_VERSION)
            .json(&wire_request)
            .send()
            .await
            .map_err(|error| ProviderError::Http(error.to_string()))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| ProviderError::Http(error.to_string()))?;

        if !status.is_success() {
            return Err(ProviderError::Api {
                provider: PROVIDER,
                status: status.as_u16(),
                body,
            });
        }

        let wire: types::Response =
            serde_json::from_str(&body).map_err(|error| ProviderError::InvalidResponse {
                provider: PROVIDER,
                message: format!("{error}; body: {body}"),
            })?;

        Ok(wire.into())
    }
}
