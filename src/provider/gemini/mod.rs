//! Gemini backend, speaking the Interactions API.

mod types;

use crate::provider::{GenerateRequest, GenerateResponse, Provider, ProviderError};

const INTERACTIONS_URL: &str = "https://generativelanguage.googleapis.com/v1beta/interactions";
const API_REVISION: &str = "2026-05-20";
const PROVIDER: &str = "Gemini";

pub(crate) struct GeminiProvider;

impl Provider for GeminiProvider {
    async fn generate(
        &self,
        http: &reqwest::Client,
        api_key: &str,
        request: &GenerateRequest,
    ) -> Result<GenerateResponse, ProviderError> {
        let wire_request = types::Request::try_from(request)?;

        let response = http
            .post(INTERACTIONS_URL)
            .header("x-goog-api-key", api_key)
            .header("Api-Revision", API_REVISION)
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
