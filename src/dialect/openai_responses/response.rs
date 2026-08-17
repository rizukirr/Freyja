//! Inbound wire format for the OpenAI Responses API.

use crate::endpoint::EndpointConfig;
use crate::error::Error;
use crate::model::{GenerateResponse, OutputContent, ResponseStatus, Usage};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
pub(crate) struct Response {
    id: String,
    model: String,
    status: String,
    /// Output items stay as raw values so unrecognized ones, reasoning items
    /// above all, can be replayed verbatim on the next request.
    #[serde(default)]
    output: Vec<Value>,
    usage: Option<UsageWire>,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

#[derive(Deserialize)]
struct UsageWire {
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
}

impl From<Response> for GenerateResponse {
    fn from(value: Response) -> Self {
        let content = value.output.into_iter().flat_map(convert_item).collect();

        Self {
            id: value.id,
            model: value.model,
            status: parse_status(value.status),
            content,
            usage: value.usage.map(|u| Usage {
                input_tokens: u.input_tokens,
                output_tokens: u.output_tokens,
                total_tokens: u.total_tokens,
            }),
            provider_metadata: Some(Value::Object(value.extra)),
        }
    }
}

/// Converts one output item into neutral output parts.
///
/// Anything Freyja does not model becomes [`OutputContent::Reasoning`] rather
/// than being dropped, so reasoning items survive into the next request. Models
/// that emit them reject a follow-up transcript that leaves them out.
fn convert_item(item: Value) -> Vec<OutputContent> {
    match item.get("type").and_then(Value::as_str) {
        Some("message") => item
            .get("content")
            .and_then(Value::as_array)
            .map(|content| {
                content
                    .iter()
                    .filter_map(|part| match part.get("type").and_then(Value::as_str) {
                        Some("output_text") => part
                            .get("text")
                            .and_then(Value::as_str)
                            .map(|text| OutputContent::Text(text.to_string())),
                        Some("refusal") => part
                            .get("refusal")
                            .and_then(Value::as_str)
                            .map(|text| OutputContent::Refusal(text.to_string())),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        Some("function_call") => vec![OutputContent::ToolCall {
            id: item
                .get("call_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            name: item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            arguments: item
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}")
                .to_string(),
        }],
        _ => vec![OutputContent::Reasoning { data: item }],
    }
}

pub(crate) fn parse_status(status: String) -> ResponseStatus {
    match status.as_str() {
        "completed" => ResponseStatus::Completed,
        "incomplete" => ResponseStatus::Incomplete,
        "requires_action" => ResponseStatus::RequiresAction,
        "failed" => ResponseStatus::Failed,
        _ => ResponseStatus::Other(status),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::openai_responses::request::Request;
    use crate::{EndpointPreset, GenerateRequest, Message, Role};

    #[test]
    fn normalizes_openai_response() {
        let wire: Response = serde_json::from_value(serde_json::json!({
            "id": "resp_1", "model": "gpt-test", "status": "completed",
            "output": [{"type":"message", "content":[{"type":"output_text", "text":"hello"}]}],
            "usage": {"input_tokens": 2, "output_tokens": 1, "total_tokens": 3}
        }))
        .unwrap();

        let response = GenerateResponse::from(wire);
        assert_eq!(response.output_text(), "hello");
        assert_eq!(response.status, ResponseStatus::Completed);
        assert_eq!(response.usage.unwrap().total_tokens, 3);
    }

    #[test]
    fn round_trips_a_tool_call_back_into_a_request() {
        let wire: Response = serde_json::from_value(serde_json::json!({
            "id": "resp_1", "model": "gpt-test", "status": "requires_action",
            "output": [{
                "type": "function_call",
                "call_id": "call_1",
                "name": "add",
                "arguments": "{\"a\":20,\"b\":22}"
            }]
        }))
        .unwrap();

        let response = GenerateResponse::from(wire);
        let calls: Vec<_> = response.tool_calls().collect();
        assert_eq!(calls, vec![("call_1", "add", "{\"a\":20,\"b\":22}")]);

        let follow_up = GenerateRequest::new()
            .message(Message::text(Role::User, "What is 20 + 22?"))
            .message(response.to_message())
            .message(Message::tool_result("call_1", "42"));
        let config = EndpointPreset::OpenAi.config();
        let json = serde_json::to_value(Request::build(&follow_up, &config).unwrap()).unwrap();
        let input = json["input"].as_array().unwrap();

        assert_eq!(input.len(), 3);
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[2]["type"], "function_call_output");
        assert_eq!(input[2]["output"], "42");
    }
}
