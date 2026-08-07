//! Wire types for the Gemini Interactions API and their conversions to and from
//! the neutral model.

use crate::provider::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const PROVIDER: &str = "Gemini";
const DEFAULT_MODEL: &str = "gemini-3.5-flash";

#[derive(Serialize)]
pub struct Request {
    model: String,
    input: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_interaction_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    labels: Option<Value>,
}

impl TryFrom<&GenerateRequest> for Request {
    type Error = ProviderError;

    fn try_from(value: &GenerateRequest) -> Result<Self, Self::Error> {
        if value.reasoning_effort.is_some() {
            return Err(ProviderError::UnsupportedCapability {
                provider: PROVIDER,
                capability: "portable reasoning effort levels",
            });
        }
        if value.tool_choice.is_some() {
            return Err(ProviderError::UnsupportedCapability {
                provider: PROVIDER,
                capability: "portable tool choice",
            });
        }

        let mut system = Vec::new();
        let mut turns: Vec<Value> = Vec::new();

        for message in &value.messages {
            if matches!(message.role, Role::System | Role::Developer) {
                for part in &message.content {
                    match part {
                        InputContent::Text(text) => system.push(text.clone()),
                        _ => {
                            return Err(ProviderError::UnsupportedCapability {
                                provider: PROVIDER,
                                capability: "non-text content in system/developer messages",
                            });
                        }
                    }
                }
                continue;
            }

            let role = match message.role {
                Role::Assistant => "model",
                // Tool results are reported on a user turn, the same way the
                // model's own output comes back on a model turn.
                _ => "user",
            };

            let mut content = Vec::with_capacity(message.content.len());
            for part in &message.content {
                content.push(match part {
                    InputContent::Text(text) => {
                        if message.role == Role::Tool {
                            return Err(ProviderError::InvalidRequest {
                                provider: PROVIDER,
                                message: "tool messages may only contain tool results".into(),
                            });
                        }
                        serde_json::json!({"type": "text", "text": text})
                    }
                    InputContent::ImageUrl(url) => {
                        if message.role != Role::User {
                            return Err(ProviderError::UnsupportedCapability {
                                provider: PROVIDER,
                                capability: "images outside user messages",
                            });
                        }
                        serde_json::json!({"type": "image", "uri": url})
                    }
                    InputContent::ToolCall {
                        id,
                        name,
                        arguments,
                    } => serde_json::json!({
                        "type": "function_call",
                        "id": id,
                        "name": name,
                        "arguments": parse_json_or_string(arguments),
                    }),
                    InputContent::ToolResult { call_id, output } => serde_json::json!({
                        "type": "function_result",
                        "id": call_id,
                        "result": parse_json_or_string(output),
                    }),
                });
            }

            turns.push(serde_json::json!({"role": role, "content": content}));
        }

        // A lone plain-text user turn may be sent as a bare string.
        let input = if turns.len() == 1
            && turns[0]["role"] == "user"
            && turns[0]["content"]
                .as_array()
                .is_some_and(|content| content.len() == 1 && content[0]["type"] == "text")
        {
            turns[0]["content"][0]["text"].clone()
        } else {
            Value::Array(turns)
        };

        let response_format = value.response_format.as_ref().map(|format| match format {
            ResponseFormat::Text => serde_json::json!({"type": "text"}),
            ResponseFormat::JsonObject => {
                serde_json::json!({"type": "json_schema", "json_schema": {"type": "object"}})
            }
            ResponseFormat::JsonSchema {
                name,
                schema,
                strict,
            } => serde_json::json!({
                "type": "json_schema",
                "name": name,
                "json_schema": schema,
                "strict": strict,
            }),
        });

        Ok(Self {
            model: value
                .model
                .clone()
                .unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            input,
            system_instruction: (!system.is_empty()).then(|| system.join("\n\n")),
            max_output_tokens: value.max_tokens,
            temperature: value.temperature,
            top_p: value.top_p,
            response_format,
            tools: value
                .tools
                .iter()
                .map(|tool| {
                    serde_json::json!({
                        "type": "function",
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
                    })
                })
                .collect(),
            previous_interaction_id: value.previous_response_id.clone(),
            labels: value.metadata.clone(),
        })
    }
}

/// Tool arguments and results travel as strings in the neutral model but as
/// structured values on the wire. Anything that is not valid JSON is sent as a
/// JSON string rather than being rejected.
fn parse_json_or_string(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
}

#[derive(Deserialize)]
pub struct Response {
    id: String,
    #[serde(default)]
    model: String,
    status: String,
    #[serde(default)]
    steps: Vec<Step>,
    usage: Option<UsageWire>,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum Step {
    #[serde(rename = "model_output")]
    ModelOutput { content: Vec<ContentWire> },
    #[serde(rename = "function_call")]
    FunctionCall {
        #[serde(default)]
        id: String,
        name: String,
        arguments: Value,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ContentWire {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(other)]
    Unknown,
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
        let content = value
            .steps
            .into_iter()
            .flat_map(|step| match step {
                Step::ModelOutput { content } => content
                    .into_iter()
                    .filter_map(|item| match item {
                        ContentWire::Text { text } => Some(OutputContent::Text(text)),
                        ContentWire::Unknown => None,
                    })
                    .collect(),
                Step::FunctionCall {
                    id,
                    name,
                    arguments,
                } => vec![OutputContent::ToolCall {
                    id,
                    name,
                    arguments: arguments.to_string(),
                }],
                Step::Unknown => vec![],
            })
            .collect();

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_neutral_request_to_gemini_wire_format() {
        let request = GenerateRequest::new()
            .message(Message::text(Role::System, "Be concise"))
            .message(Message::text(Role::User, "Hello"))
            .max_tokens(42);

        let wire = Request::try_from(&request).unwrap();
        let json = serde_json::to_value(wire).unwrap();

        assert_eq!(json["model"], DEFAULT_MODEL);
        assert_eq!(json["system_instruction"], "Be concise");
        assert_eq!(json["input"], "Hello");
        assert_eq!(json["max_output_tokens"], 42);
    }

    #[test]
    fn maps_a_full_tool_round_trip() {
        let request = GenerateRequest::new()
            .message(Message::text(Role::User, "What is 20 + 22?"))
            .message(Message::new(
                Role::Assistant,
                vec![InputContent::ToolCall {
                    id: "call_1".into(),
                    name: "add".into(),
                    arguments: "{\"a\":20,\"b\":22}".into(),
                }],
            ))
            .message(Message::tool_result("call_1", "42"));

        let json = serde_json::to_value(Request::try_from(&request).unwrap()).unwrap();
        let turns = json["input"].as_array().unwrap();

        assert_eq!(turns.len(), 3);
        assert_eq!(turns[0]["role"], "user");

        assert_eq!(turns[1]["role"], "model");
        assert_eq!(turns[1]["content"][0]["type"], "function_call");
        assert_eq!(turns[1]["content"][0]["id"], "call_1");
        assert_eq!(turns[1]["content"][0]["arguments"]["a"], 20);

        assert_eq!(turns[2]["role"], "user");
        assert_eq!(turns[2]["content"][0]["type"], "function_result");
        assert_eq!(turns[2]["content"][0]["id"], "call_1");
        assert_eq!(turns[2]["content"][0]["result"], 42);
    }

    #[test]
    fn sends_non_json_tool_output_as_a_string() {
        let request = GenerateRequest::new()
            .message(Message::text(Role::User, "Hi"))
            .message(Message::tool_result("call_1", "not json"));

        let json = serde_json::to_value(Request::try_from(&request).unwrap()).unwrap();

        assert_eq!(json["input"][1]["content"][0]["result"], "not json");
    }

    #[test]
    fn normalizes_gemini_response() {
        let wire: Response = serde_json::from_value(serde_json::json!({
            "id": "int_1", "model": "gemini-test", "status": "completed",
            "steps": [{"type":"model_output", "content":[{"type":"text", "text":"hello"}]}],
            "usage": {"total_input_tokens": 2, "total_output_tokens": 1, "total_tokens": 3}
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
            "id": "int_1", "model": "gemini-test", "status": "requires_action",
            "steps": [{
                "type": "function_call",
                "id": "call_1",
                "name": "add",
                "arguments": {"a": 20, "b": 22}
            }]
        }))
        .unwrap();

        let response = GenerateResponse::from(wire);
        assert!(response.has_tool_calls());

        let follow_up = GenerateRequest::new()
            .message(Message::text(Role::User, "What is 20 + 22?"))
            .message(response.to_message())
            .message(Message::tool_result("call_1", "42"));

        let json = serde_json::to_value(Request::try_from(&follow_up).unwrap()).unwrap();
        let turns = json["input"].as_array().unwrap();

        assert_eq!(turns.len(), 3);
        assert_eq!(turns[1]["content"][0]["name"], "add");
        assert_eq!(turns[2]["content"][0]["result"], 42);
    }

    #[test]
    fn rejects_capabilities_that_cannot_be_translated() {
        let request = GenerateRequest::new().reasoning_effort(ReasoningEffort::High);

        assert!(matches!(
            Request::try_from(&request),
            Err(ProviderError::UnsupportedCapability { .. })
        ));
    }

    #[test]
    fn rejects_text_in_a_tool_message() {
        let request = GenerateRequest::new().message(Message::text(Role::Tool, "42"));

        assert!(matches!(
            Request::try_from(&request),
            Err(ProviderError::InvalidRequest { .. })
        ));
    }
}
