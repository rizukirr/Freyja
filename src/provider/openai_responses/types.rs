//! Wire types for the OpenAI Responses API and their conversions to and from
//! the neutral model.

use crate::provider::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize)]
pub struct Request {
    model: String,
    input: Vec<InputItemWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ReasoningWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<TextWire>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ToolWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_response_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<Value>,
}

/// Responses API input items are a flat list: messages, tool calls, and tool
/// results all sit at the top level rather than nesting inside a message.
#[derive(Serialize)]
#[serde(untagged)]
enum InputItemWire {
    /// An item Freyja models.
    Item(TypedItemWire),
    /// An item preserved from a previous response and replayed verbatim, such
    /// as a reasoning item that the model requires back unchanged.
    Raw(Value),
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum TypedItemWire {
    #[serde(rename = "message")]
    Message { role: Role, content: Vec<InputWire> },
    #[serde(rename = "function_call")]
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    #[serde(rename = "function_call_output")]
    FunctionCallOutput { call_id: String, output: String },
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum InputWire {
    #[serde(rename = "input_text")]
    Text { text: String },
    /// Assistant turns replayed as input use `output_text`, not `input_text`.
    #[serde(rename = "output_text")]
    OutputText { text: String },
    #[serde(rename = "input_image")]
    Image { image_url: String },
}

#[derive(Serialize)]
struct ReasoningWire {
    effort: ReasoningEffort,
}

#[derive(Serialize)]
struct TextWire {
    format: Value,
}

#[derive(Serialize)]
struct ToolWire {
    #[serde(rename = "type")]
    kind: &'static str,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    parameters: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    strict: Option<bool>,
}

impl Request {
    /// Converts a neutral request into this dialect's wire format.
    pub(crate) fn build(
        value: &GenerateRequest,
        config: &ProviderConfig,
    ) -> Result<Self, ProviderError> {
        let mut instructions = Vec::new();
        let mut input = Vec::new();

        for message in &value.messages {
            if matches!(message.role, Role::System | Role::Developer) {
                for content in &message.content {
                    match content {
                        InputContent::Text(text) => instructions.push(text.clone()),
                        _ => {
                            return Err(ProviderError::UnsupportedCapability {
                                provider: config.name.clone(),
                                capability: "non-text content in system/developer messages",
                            });
                        }
                    }
                }
                continue;
            }

            // Text and images accumulate into one message item; tool calls and
            // tool results are separate top-level items, so the pending message
            // is flushed before each of them to keep transcript order intact.
            let mut pending: Vec<InputWire> = Vec::new();
            for content in &message.content {
                match content {
                    InputContent::Text(text) => pending.push(if message.role == Role::Assistant {
                        InputWire::OutputText { text: text.clone() }
                    } else {
                        InputWire::Text { text: text.clone() }
                    }),
                    InputContent::ImageUrl(image_url) => {
                        if message.role != Role::User {
                            return Err(ProviderError::UnsupportedCapability {
                                provider: config.name.clone(),
                                capability: "images outside user messages",
                            });
                        }
                        pending.push(InputWire::Image {
                            image_url: image_url.clone(),
                        });
                    }
                    InputContent::ToolCall {
                        id,
                        name,
                        arguments,
                    } => {
                        flush(&mut input, message.role, &mut pending);
                        input.push(InputItemWire::Item(TypedItemWire::FunctionCall {
                            call_id: id.clone(),
                            name: name.clone(),
                            arguments: arguments.clone(),
                        }));
                    }
                    InputContent::ToolResult { call_id, output } => {
                        flush(&mut input, message.role, &mut pending);
                        input.push(InputItemWire::Item(TypedItemWire::FunctionCallOutput {
                            call_id: call_id.clone(),
                            output: output.clone(),
                        }));
                    }
                    InputContent::Reasoning { data } => {
                        flush(&mut input, message.role, &mut pending);
                        input.push(InputItemWire::Raw(data.clone()));
                    }
                }
            }

            if message.role == Role::Tool && !pending.is_empty() {
                return Err(ProviderError::InvalidRequest {
                    provider: config.name.clone(),
                    message: "tool messages may only contain tool results".into(),
                });
            }
            flush(&mut input, message.role, &mut pending);
        }

        let text = value.response_format.as_ref().map(|format| TextWire {
            format: match format {
                ResponseFormat::Text => serde_json::json!({"type": "text"}),
                ResponseFormat::JsonObject => serde_json::json!({"type": "json_object"}),
                ResponseFormat::JsonSchema {
                    name,
                    schema,
                    strict,
                } => serde_json::json!({
                    "type": "json_schema",
                    "name": name,
                    "schema": schema,
                    "strict": strict,
                }),
            },
        });

        let tool_choice = value.tool_choice.as_ref().map(|choice| match choice {
            ToolChoice::Auto => Value::String("auto".into()),
            ToolChoice::None => Value::String("none".into()),
            ToolChoice::Required => Value::String("required".into()),
            ToolChoice::Named(name) => serde_json::json!({"type": "function", "name": name}),
        });

        Ok(Self {
            model: config.model_for(value)?,
            input,
            instructions: (!instructions.is_empty()).then(|| instructions.join("\n\n")),
            max_output_tokens: value.max_tokens,
            temperature: value.temperature,
            top_p: value.top_p,
            reasoning: value
                .reasoning_effort
                .map(|effort| ReasoningWire { effort }),
            text,
            tools: value
                .tools
                .iter()
                .map(|tool| ToolWire {
                    kind: "function",
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    parameters: tool.parameters.clone(),
                    strict: tool.strict,
                })
                .collect(),
            tool_choice,
            previous_response_id: value.previous_response_id.clone(),
            metadata: value.metadata.clone(),
        })
    }
}

/// Emits the accumulated text/image parts as a message item, if any.
fn flush(input: &mut Vec<InputItemWire>, role: Role, pending: &mut Vec<InputWire>) {
    if pending.is_empty() {
        return;
    }
    input.push(InputItemWire::Item(TypedItemWire::Message {
        role,
        content: std::mem::take(pending),
    }));
}

#[derive(Deserialize)]
pub struct Response {
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

fn parse_status(status: String) -> ResponseStatus {
    match status.as_str() {
        "completed" => ResponseStatus::Completed,
        "incomplete" => ResponseStatus::Incomplete,
        "requires_action" => ResponseStatus::RequiresAction,
        "failed" => ResponseStatus::Failed,
        _ => ResponseStatus::Other(status),
    }
}

/// Parses a successful response body, attributing failures to the endpoint.
pub(crate) fn parse(
    body: &str,
    config: &ProviderConfig,
) -> Result<GenerateResponse, ProviderError> {
    let wire: Response =
        serde_json::from_str(body).map_err(|error| ProviderError::InvalidResponse {
            provider: config.name.clone(),
            message: format!("{error}; body: {body}"),
        })?;
    Ok(wire.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderType;

    /// The shipped endpoint for this dialect, so tests cover the real defaults.
    fn config() -> ProviderConfig {
        ProviderType::OpenAi.config()
    }

    #[test]
    fn maps_neutral_request_to_openai_wire_format() {
        let request = GenerateRequest::new()
            .message(Message::text(Role::System, "Be concise"))
            .message(Message::text(Role::User, "Hello"))
            .max_tokens(42);

        let wire = Request::build(&request, &config()).unwrap();
        let json = serde_json::to_value(wire).unwrap();

        assert_eq!(json["model"], "gpt-5.6-sol");
        assert_eq!(json["instructions"], "Be concise");
        assert_eq!(json["input"][0]["type"], "message");
        assert_eq!(json["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(json["max_output_tokens"], 42);
    }

    #[test]
    fn omits_capabilities_the_caller_did_not_ask_for() {
        let request = GenerateRequest::new().message(Message::text(Role::User, "Hello"));

        let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();

        assert!(json.get("reasoning").is_none());
        assert!(json.get("tool_choice").is_none());
        assert!(json.get("tools").is_none());
    }

    #[test]
    fn maps_a_full_tool_round_trip() {
        let request = GenerateRequest::new()
            .message(Message::text(Role::User, "What is 20 + 22?"))
            .message(Message::new(
                Role::Assistant,
                vec![
                    InputContent::Text("Let me add those.".into()),
                    InputContent::ToolCall {
                        id: "call_1".into(),
                        name: "add".into(),
                        arguments: "{\"a\":20,\"b\":22}".into(),
                    },
                ],
            ))
            .message(Message::tool_result("call_1", "42"));

        let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();
        let input = json["input"].as_array().unwrap();

        assert_eq!(input.len(), 4);
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[0]["role"], "user");

        // The assistant's text is flushed before its tool call, preserving order.
        assert_eq!(input[1]["type"], "message");
        assert_eq!(input[1]["role"], "assistant");
        assert_eq!(input[1]["content"][0]["type"], "output_text");

        assert_eq!(input[2]["type"], "function_call");
        assert_eq!(input[2]["call_id"], "call_1");
        assert_eq!(input[2]["name"], "add");
        assert_eq!(input[2]["arguments"], "{\"a\":20,\"b\":22}");

        assert_eq!(input[3]["type"], "function_call_output");
        assert_eq!(input[3]["call_id"], "call_1");
        assert_eq!(input[3]["output"], "42");
    }

    #[test]
    fn rejects_text_in_a_tool_message() {
        let request = GenerateRequest::new().message(Message::text(Role::Tool, "42"));

        assert!(matches!(
            Request::build(&request, &config()),
            Err(ProviderError::InvalidRequest { .. })
        ));
    }

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

        // The response feeds straight back in as the next turn.
        let follow_up = GenerateRequest::new()
            .message(Message::text(Role::User, "What is 20 + 22?"))
            .message(response.to_message())
            .message(Message::tool_result("call_1", "42"));

        let json = serde_json::to_value(Request::build(&follow_up, &config()).unwrap()).unwrap();
        let input = json["input"].as_array().unwrap();

        assert_eq!(input.len(), 3);
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[2]["type"], "function_call_output");
        assert_eq!(input[2]["output"], "42");
    }
}
