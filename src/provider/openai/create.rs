use crate::provider::openai::model::{DEFAULT_MODEL, Response, ResponseRequest};

const RESPONSE_URL: &str = "https://api.openai.com/v1/responses";

pub async fn create(api_key: &str, request: &mut ResponseRequest) -> Result<Response, String> {
    if request.model.is_empty() {
        request.model = DEFAULT_MODEL.to_string();
    }

    let response = reqwest::Client::new()
        .post(RESPONSE_URL)
        .bearer_auth(api_key)
        .json(request)
        .send()
        .await
        .map_err(|error| error.to_string())?;

    let status = response.status();
    let body = response.text().await.map_err(|error| error.to_string())?;

    if !status.is_success() {
        return Err(format!("OpenAI returned {status}: {body}"));
    }

    serde_json::from_str(&body)
        .map_err(|error| format!("Invalid response JSON: {error}; body: {body}"))
}
