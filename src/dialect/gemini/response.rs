//! Inbound wire structures for the Gemini Interactions API.

use crate::endpoint::EndpointConfig;
use crate::error::Error;
use crate::model::{GenerateResponse, OutputContent, ResponseStatus, Usage};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
pub struct Response {
    id: String,
    #[serde(default)]
    model: String,
    status: String,
    /// Steps stay as raw values so unrecognized ones, thought signatures above
    /// all, can be replayed verbatim on the next request.
    #[serde(default)]
    steps: Vec<Value>,
    usage: Option<UsageWire>,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

#[derive(Deserialize)]
struct UsageWire {
    #[serde(default)]
    total_input_tokens: u64,
    #[serde(default)]
    total_output_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

impl From<Response> for GenerateResponse {
    fn from(value: Response) -> Self {
        let content = value.steps.into_iter().flat_map(convert_step).collect();

        Self {
            id: value.id,
            model: value.model,
            status: match value.status.as_str() {
                "completed" => ResponseStatus::Completed,
                "incomplete" | "budget_exceeded" => ResponseStatus::Incomplete,
                "requires_action" => ResponseStatus::RequiresAction,
                "failed" | "cancelled" => ResponseStatus::Failed,
                _ => ResponseStatus::Other(value.status),
            },
            content,
            usage: value.usage.map(|u| Usage {
                input_tokens: u.total_input_tokens,
                output_tokens: u.total_output_tokens,
                total_tokens: u.total_tokens,
            }),
            provider_metadata: Some(Value::Object(value.extra)),
        }
    }
}

/// Converts one response step into neutral output parts.
///
/// Anything Freyja does not model becomes [`OutputContent::Reasoning`] rather
/// than being dropped. Gemini rejects a follow-up request whose thought steps
/// are missing or rebuilt, so preserving them verbatim is what makes multi-turn
/// tool calling work at all.
fn convert_step(step: Value) -> Vec<OutputContent> {
    match step.get("type").and_then(Value::as_str) {
        Some("model_output") => step
            .get("content")
            .and_then(Value::as_array)
            .map(|content| {
                content
                    .iter()
                    .filter_map(|part| match part.get("type").and_then(Value::as_str) {
                        Some("text") => part
                            .get("text")
                            .and_then(Value::as_str)
                            .map(|text| OutputContent::Text(text.to_string())),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        Some("function_call") => {
            let name = step.get("name").and_then(Value::as_str).unwrap_or_default();
            vec![OutputContent::ToolCall {
                id: step
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                name: name.to_string(),
                arguments: step
                    .get("arguments")
                    .map(Value::to_string)
                    .unwrap_or_else(|| "{}".to_string()),
            }]
        }
        _ => vec![OutputContent::Reasoning { data: step }],
    }
}

/// Parses a successful response body, attributing failures to the endpoint.
pub(crate) fn parse(body: &str, config: &EndpointConfig) -> Result<GenerateResponse, Error> {
    let wire: Response = serde_json::from_str(body).map_err(|error| Error::InvalidResponse {
        endpoint: config.name.clone(),
        message: format!("{error}; body: {body}"),
    })?;
    Ok(wire.into())
}
