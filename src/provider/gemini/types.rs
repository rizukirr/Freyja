//! Wire types for the Gemini Interactions API and their conversions to and from
//! the neutral model.

use crate::provider::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

impl Request {
    /// Converts a neutral request into this dialect's wire format.
    pub(crate) fn build(
        value: &GenerateRequest,
        config: &ProviderConfig,
    ) -> Result<Self, ProviderError> {
        if value.reasoning_effort.is_some() {
            return Err(ProviderError::UnsupportedCapability {
                provider: config.name.clone(),
                capability: "portable reasoning effort levels",
            });
        }
        if value.tool_choice.is_some() {
            return Err(ProviderError::UnsupportedCapability {
                provider: config.name.clone(),
                capability: "portable tool choice",
            });
        }

        let mut system = Vec::new();
        let mut steps: Vec<Value> = Vec::new();

        // A function_result must carry the name of the tool it answers, but the
        // neutral model only records the call id, so resolve the name from the
        // matching tool call earlier in the transcript.
        let mut tool_names: std::collections::HashMap<&str, &str> =
            std::collections::HashMap::new();
        for message in &value.messages {
            for part in &message.content {
                if let InputContent::ToolCall { id, name, .. } = part {
                    tool_names.insert(id.as_str(), name.as_str());
                }
            }
        }

        for message in &value.messages {
            if matches!(message.role, Role::System | Role::Developer) {
                for part in &message.content {
                    match part {
                        InputContent::Text(text) => system.push(text.clone()),
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

            // Text and images accumulate into one user_input or model_output
            // step; tool calls, tool results, and opaque reasoning are steps of
            // their own, so the pending step is flushed before each of them to
            // keep transcript order intact.
            let step_type = if message.role == Role::Assistant {
                "model_output"
            } else {
                "user_input"
            };
            let mut pending: Vec<Value> = Vec::new();

            for part in &message.content {
                match part {
                    InputContent::Text(text) => {
                        if message.role == Role::Tool {
                            return Err(ProviderError::InvalidRequest {
                                provider: config.name.clone(),
                                message: "tool messages may only contain tool results".into(),
                            });
                        }
                        pending.push(serde_json::json!({"type": "text", "text": text}));
                    }
                    InputContent::ImageUrl(url) => {
                        if message.role != Role::User {
                            return Err(ProviderError::UnsupportedCapability {
                                provider: config.name.clone(),
                                capability: "images outside user messages",
                            });
                        }
                        pending.push(serde_json::json!({"type": "image", "uri": url}));
                    }
                    InputContent::ToolCall {
                        id,
                        name,
                        arguments,
                    } => {
                        flush(&mut steps, step_type, &mut pending);
                        steps.push(serde_json::json!({
                            "type": "function_call",
                            "id": id,
                            "name": name,
                            "arguments": parse_json_or_string(arguments),
                        }));
                    }
                    InputContent::ToolResult { call_id, output } => {
                        flush(&mut steps, step_type, &mut pending);
                        let Some(name) = tool_names.get(call_id.as_str()) else {
                            return Err(ProviderError::InvalidRequest {
                                provider: config.name.clone(),
                                message: format!(
                                    "no tool call with id '{call_id}' in the transcript; \
                                     Gemini requires the tool name alongside its result"
                                ),
                            });
                        };
                        steps.push(serde_json::json!({
                            "type": "function_result",
                            "call_id": call_id,
                            "name": name,
                            "result": result_value(output),
                        }));
                    }
                    InputContent::Reasoning { data } => {
                        flush(&mut steps, step_type, &mut pending);
                        steps.push(data.clone());
                    }
                }
            }

            flush(&mut steps, step_type, &mut pending);
        }

        // A lone plain-text user step may be sent as a bare string.
        let input = if steps.len() == 1
            && steps[0]["type"] == "user_input"
            && steps[0]["content"]
                .as_array()
                .is_some_and(|content| content.len() == 1 && content[0]["type"] == "text")
        {
            steps[0]["content"][0]["text"].clone()
        } else {
            Value::Array(steps)
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
            model: config.model_for(value)?,
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
            stream: None,
        })
    }

    /// Marks this body as a streaming request.
    pub(crate) fn streaming(mut self) -> Self {
        self.stream = Some(true);
        self
    }
}

/// Emits the accumulated text and image parts as one step, if any.
fn flush(steps: &mut Vec<Value>, step_type: &str, pending: &mut Vec<Value>) {
    if pending.is_empty() {
        return;
    }
    steps.push(serde_json::json!({
        "type": step_type,
        "content": std::mem::take(pending),
    }));
}

/// Tool arguments travel as strings in the neutral model but as structured
/// values on the wire. Anything that is not valid JSON is sent as a JSON string
/// rather than being rejected.
fn parse_json_or_string(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
}

/// A function_result payload must be an object or a string. Numbers, booleans,
/// and arrays are rejected by the API, so anything that is not a JSON object is
/// sent as the original string.
fn result_value(raw: &str) -> Value {
    match serde_json::from_str::<Value>(raw) {
        Ok(value @ Value::Object(_)) => value,
        _ => Value::String(raw.to_string()),
    }
}

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
    use crate::provider::sse::SseFrame;
    use crate::provider::stream::{RawDelta, StreamDecoder};

    fn decode_all(frames: &[&str]) -> Vec<RawDelta> {
        let mut decoder = crate::provider::gemini::Decoder::default();
        let mut out = Vec::new();
        for data in frames {
            // The Interactions API repeats event_type inside the payload, so
            // the decoder reads it there rather than from the SSE event line.
            let frame = SseFrame {
                event: None,
                data: (*data).to_string(),
            };
            decoder.decode(&frame, &mut out).expect("decodes");
        }
        out
    }

    #[test]
    fn decodes_streaming_text() {
        let deltas = decode_all(&[
            r#"{"index":0,"step":{"type":"model_output"},"event_type":"step.start"}"#,
            r#"{"index":0,"delta":{"type":"text","text":"Hello"},"event_type":"step.delta"}"#,
        ]);

        assert!(
            deltas.iter().any(|d| *d == RawDelta::Text("Hello".into())),
            "{deltas:?}"
        );
    }

    #[test]
    fn decodes_streaming_tool_call() {
        let deltas = decode_all(&[
            r#"{"index":0,"step":{"type":"function_call","id":"un6k8t18","name":"get_weather","arguments":{}},"event_type":"step.start"}"#,
            r#"{"index":0,"delta":{"type":"arguments_delta","arguments":"{\"location\": "},"event_type":"step.delta"}"#,
            r#"{"index":0,"delta":{"type":"arguments_delta","arguments":"\"San Francisco, CA\"}"},"event_type":"step.delta"}"#,
            r#"{"index":0,"event_type":"step.stop"}"#,
        ]);

        assert_eq!(
            deltas,
            vec![
                RawDelta::ToolStart {
                    slot: 0,
                    id: "un6k8t18".into(),
                    name: "get_weather".into(),
                },
                RawDelta::ToolArgs {
                    slot: 0,
                    fragment: "{\"location\": ".into(),
                },
                RawDelta::ToolArgs {
                    slot: 0,
                    fragment: "\"San Francisco, CA\"}".into(),
                },
                RawDelta::ToolEnd { slot: 0 },
            ],
            "arguments_delta fragments accumulate; the docs require it"
        );
    }

    #[test]
    fn decodes_streaming_thought_into_a_replayable_blob() {
        let deltas = decode_all(&[
            r#"{"index":0,"step":{"type":"thought"},"event_type":"step.start"}"#,
            r#"{"index":0,"delta":{"type":"thought_summary","content":{"type":"text","text":"Working it out."}},"event_type":"step.delta"}"#,
            r#"{"index":0,"delta":{"type":"thought_signature","signature":"sig-abc"},"event_type":"step.delta"}"#,
            r#"{"index":0,"event_type":"step.stop"}"#,
        ]);

        assert_eq!(deltas[0], RawDelta::ReasoningText("Working it out.".into()));
        assert_eq!(
            deltas[1],
            RawDelta::ReasoningBlob(serde_json::json!({
                "type": "thought",
                "signature": "sig-abc",
            })),
            "the signature is the part the API requires resent verbatim"
        );
    }

    #[test]
    fn preserves_unrecognised_steps_when_streaming() {
        let deltas = decode_all(&[
            r#"{"index":0,"step":{"type":"safety_report","verdict":"ok"},"event_type":"step.start"}"#,
            r#"{"index":0,"event_type":"step.stop"}"#,
        ]);

        assert_eq!(
            deltas,
            vec![RawDelta::ReasoningBlob(serde_json::json!({
                "type": "safety_report",
                "verdict": "ok",
            }))],
            "the non-streaming parser preserves any unmodeled step verbatim"
        );
    }

    #[test]
    fn decodes_streaming_completion() {
        let deltas = decode_all(&[
            r#"{"interaction":{"id":"v1_abc123","model":"gemini-3.6-flash","status":"completed","usage":{"total_tokens":346,"total_input_tokens":11,"total_output_tokens":90}},"event_type":"interaction.completed"}"#,
        ]);

        assert_eq!(
            deltas,
            vec![RawDelta::Meta {
                id: Some("v1_abc123".into()),
                model: Some("gemini-3.6-flash".into()),
                status: Some(ResponseStatus::Completed),
                usage: Some(Usage {
                    input_tokens: 11,
                    output_tokens: 90,
                    total_tokens: 346,
                }),
            }]
        );
    }

    /// The shipped endpoint for this dialect, so tests cover the real defaults.
    fn config() -> ProviderConfig {
        ProviderType::Gemini.config()
    }

    #[test]
    fn maps_neutral_request_to_gemini_wire_format() {
        let request = GenerateRequest::new()
            .message(Message::text(Role::System, "Be concise"))
            .message(Message::text(Role::User, "Hello"))
            .max_tokens(42);

        let wire = Request::build(&request, &config()).unwrap();
        let json = serde_json::to_value(wire).unwrap();

        assert_eq!(json["model"], "gemini-3.5-flash");
        assert_eq!(json["system_instruction"], "Be concise");
        assert_eq!(json["input"], "Hello");
        assert_eq!(json["max_output_tokens"], 42);
    }

    #[test]
    fn multi_turn_uses_the_step_list_format() {
        let request = GenerateRequest::new()
            .message(Message::text(Role::User, "Hi"))
            .message(Message::text(Role::Assistant, "Hello"))
            .message(Message::text(Role::User, "Bye"));

        let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();
        let steps = json["input"].as_array().unwrap();

        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0]["type"], "user_input");
        assert_eq!(steps[0]["content"][0]["type"], "text");
        assert_eq!(steps[1]["type"], "model_output");
        assert_eq!(steps[2]["type"], "user_input");
    }

    #[test]
    fn maps_a_full_tool_round_trip() {
        let request = GenerateRequest::new()
            .message(Message::text(Role::User, "What is 20 + 22?"))
            .message(Message::new(
                Role::Assistant,
                vec![
                    InputContent::Reasoning {
                        data: serde_json::json!({"type": "thought", "signature": "abc"}),
                    },
                    InputContent::ToolCall {
                        id: "call_1".into(),
                        name: "add".into(),
                        arguments: "{\"a\":20,\"b\":22}".into(),
                    },
                ],
            ))
            .message(Message::tool_result("call_1", "42"));

        let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();
        let steps = json["input"].as_array().unwrap();

        assert_eq!(steps.len(), 4);
        assert_eq!(steps[0]["type"], "user_input");

        // The thought signature is replayed verbatim, ahead of the call.
        assert_eq!(steps[1]["type"], "thought");
        assert_eq!(steps[1]["signature"], "abc");

        assert_eq!(steps[2]["type"], "function_call");
        assert_eq!(steps[2]["id"], "call_1");
        assert_eq!(steps[2]["arguments"]["a"], 20);

        // A result carries call_id and the tool name, and its payload is a
        // string because 42 is not a JSON object.
        assert_eq!(steps[3]["type"], "function_result");
        assert_eq!(steps[3]["call_id"], "call_1");
        assert_eq!(steps[3]["name"], "add");
        assert_eq!(steps[3]["result"], "42");
    }

    #[test]
    fn sends_an_object_result_as_a_struct() {
        let request = GenerateRequest::new()
            .message(Message::new(
                Role::Assistant,
                vec![InputContent::ToolCall {
                    id: "call_1".into(),
                    name: "add".into(),
                    arguments: "{}".into(),
                }],
            ))
            .message(Message::tool_result("call_1", "{\"sum\":42}"));

        let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();
        let steps = json["input"].as_array().unwrap();

        assert_eq!(steps[1]["result"]["sum"], 42);
    }

    #[test]
    fn rejects_a_result_with_no_matching_call() {
        let request = GenerateRequest::new().message(Message::tool_result("missing", "42"));

        assert!(matches!(
            Request::build(&request, &config()),
            Err(ProviderError::InvalidRequest { .. })
        ));
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
    fn preserves_thought_steps_as_reasoning() {
        let wire: Response = serde_json::from_value(serde_json::json!({
            "id": "int_1", "model": "gemini-test", "status": "requires_action",
            "steps": [
                {"type": "thought", "signature": "opaque-blob"},
                {"type": "function_call", "id": "call_1", "name": "add",
                 "arguments": {"a": 20, "b": 22}}
            ]
        }))
        .unwrap();

        let response = GenerateResponse::from(wire);

        assert_eq!(
            response.content[0],
            OutputContent::Reasoning {
                data: serde_json::json!({"type": "thought", "signature": "opaque-blob"}),
            }
        );
        assert!(response.has_tool_calls());

        // to_message carries the signature into the next request unchanged.
        let message = response.to_message();
        assert_eq!(
            message.content[0],
            InputContent::Reasoning {
                data: serde_json::json!({"type": "thought", "signature": "opaque-blob"}),
            }
        );
    }

    #[test]
    fn rejects_capabilities_that_cannot_be_translated() {
        let request = GenerateRequest::new().reasoning_effort(ReasoningEffort::High);

        assert!(matches!(
            Request::build(&request, &config()),
            Err(ProviderError::UnsupportedCapability { .. })
        ));
    }

    #[test]
    fn rejects_text_in_a_tool_message() {
        let request = GenerateRequest::new().message(Message::text(Role::Tool, "42"));

        assert!(matches!(
            Request::build(&request, &config()),
            Err(ProviderError::InvalidRequest { .. })
        ));
    }
}
