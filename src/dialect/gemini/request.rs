//! Outbound wire structures for the Gemini Interactions API.

use crate::dialect::refusal;
use crate::endpoint::EndpointConfig;
use crate::error::Error;
use crate::model::{
    GenerateRequest, InputContent, ReasoningEffort, ResponseFormat, Role, ToolChoice,
};
use serde::Serialize;
use serde_json::Value;

/// Sampling, reasoning, and tool controls, which this API nests rather than
/// taking at the top level. Sending them loose is rejected outright: the
/// endpoint validates the parameter names it is given and answers `Unknown
/// parameter 'temperature'`.
#[derive(Serialize)]
struct GenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    /// Reasoning effort. Serializes lowercase, so all six portable levels have
    /// a spelling; the endpoint accepts `minimal`, `low`, `medium`, and `high`
    /// and answers `'none' is not supported ... Supported values: 'minimal',
    /// 'low', 'medium', 'high'` for the rest. Freyja used to refuse those three
    /// itself, which was a claim about a value rather than about the format.
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_level: Option<ReasoningEffort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<Value>,
}

/// Maps portable tool choice onto this API's `generation_config.tool_choice`.
///
/// The field takes two shapes. A bare mode string covers the first three
/// levels; `allowed_tools` narrows a mode to a named set, which is how a
/// specific tool is demanded, "call something, and the something is this one".
///
/// The accepted values are `auto`, `any`, `none`, and `validated`, lowercase.
/// Anything else, `required` included, comes back as `Invalid enum value`.
///
/// All four shapes were sent live and cleared the endpoint's validation. None
/// of them returned a completion, the free tier ran out during the probing,
/// so `Named` compelling the tool rather than merely permitting it is read off
/// the shape, not observed.
fn tool_choice(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::Auto => Value::from("auto"),
        ToolChoice::None => Value::from("none"),
        ToolChoice::Required => Value::from("any"),
        ToolChoice::Named(name) => serde_json::json!({
            "allowed_tools": {"mode": "any", "tools": [name]}
        }),
    }
}

#[derive(Serialize)]
pub struct Request {
    model: String,
    input: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GenerationConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Value>,
    /// Request metadata. This endpoint answers "The parameter 'labels' is not
    /// available on the Gemini API but it is available on the Gemini Enterprise
    /// Agent Platform", a deployment gating a field it has, not a format
    /// missing one, so it is sent and the endpoint answers.
    #[serde(skip_serializing_if = "Option::is_none")]
    labels: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_interaction_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

impl Request {
    /// Converts a neutral request into this dialect's wire format.
    pub(crate) fn build(value: &GenerateRequest, config: &EndpointConfig) -> Result<Self, Error> {
        let tool_choice = value.tool_choice.as_ref().map(tool_choice);

        let mut system = Vec::new();
        let mut steps: Vec<Value> = Vec::new();

        // A function_result must carry the name of the tool it answers, but the
        // neutral model only records the call id, so the name is resolved from
        // the matching tool call. Filled as the transcript is walked rather than
        // in a pass ahead of it: a result may only answer a call that already
        // happened, and pre-scanning accepted one that answered a call still to
        // come -- a transcript the endpoint then rejected.
        let mut tool_names: std::collections::HashMap<&str, &str> =
            std::collections::HashMap::new();

        for message in &value.messages {
            if matches!(message.role, Role::System | Role::Developer) {
                for part in &message.content {
                    match part {
                        InputContent::Text(text) => system.push(text.clone()),
                        _ => {
                            return Err(refusal::unsupported(config, refusal::NON_TEXT_SYSTEM));
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
                            return Err(Error::InvalidRequest {
                                endpoint: config.name.clone(),
                                message: "tool messages may only contain tool results".into(),
                            });
                        }
                        pending.push(serde_json::json!({"type": "text", "text": text}));
                    }
                    // A `model_output` step takes typed content just as
                    // `user_input` does, and the endpoint validates an image in
                    // one exactly as it does the other. A tool turn is still
                    // refused, for the same reason its text is: the step it
                    // becomes carries a result and nothing else.
                    InputContent::ImageUrl(url) => {
                        if message.role == Role::Tool {
                            return Err(Error::InvalidRequest {
                                endpoint: config.name.clone(),
                                message: "tool messages may only contain tool results".into(),
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
                        tool_names.insert(id.as_str(), name.as_str());
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
                            return Err(Error::InvalidRequest {
                                endpoint: config.name.clone(),
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

        // `response_format` on this API *is* the schema: its `type` takes the
        // JSON Schema type names -- object, string, array and the rest -- and
        // rejects the `json_schema` wrapper the other dialects use. `name` and
        // `strict` have nowhere to go and are dropped, as they are on Anthropic.
        let response_format = value.response_format.as_ref().map(|format| match format {
            ResponseFormat::Text => serde_json::json!({"type": "text"}),
            ResponseFormat::JsonObject => serde_json::json!({"type": "object"}),
            ResponseFormat::JsonSchema { schema, .. } => schema.clone(),
        });

        Ok(Self {
            model: config.model_for(value)?,
            input,
            system_instruction: (!system.is_empty()).then(|| system.join("\n\n")),
            // Omitted entirely when the caller set none of the five, rather
            // than sent as an empty object.
            generation_config: (value.max_tokens.is_some()
                || value.temperature.is_some()
                || value.top_p.is_some()
                || value.reasoning_effort.is_some()
                || tool_choice.is_some())
            .then_some(GenerationConfig {
                max_output_tokens: value.max_tokens,
                temperature: value.temperature,
                top_p: value.top_p,
                thinking_level: value.reasoning_effort,
                tool_choice,
            }),
            response_format,
            tools: value
                .tools
                .iter()
                .map(|tool| {
                    serde_json::json!({
                        "type": "function",
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.schema(),
                    })
                })
                .collect(),
            labels: value.metadata.clone(),
            previous_interaction_id: value.previous_response_id.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::gemini::response::{Response, parse};
    use crate::dialect::sse::SseFrame;
    use crate::dialect::stream::{RawDelta, StreamDecoder};
    use crate::model::{GenerateResponse, OutputContent, ResponseStatus, Usage};
    use crate::{EndpointPreset, Message};

    fn decode_all(frames: &[&str]) -> Vec<RawDelta> {
        let mut decoder = crate::dialect::gemini::Decoder::default();
        let mut out = Vec::new();
        for data in frames {
            // The Interactions API repeats event_type inside the payload, so
            // the decoder reads it there rather than from the SSE event line.
            let frame = SseFrame {
                event: None,
                data: (*data).to_string(),
            };
            decoder
                .decode(&frame, &"test-endpoint".into(), &mut out)
                .expect("decodes");
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
                // The terminal frame's own object, carried whole so a caller
                // can read fields Freyja does not model.
                provider_metadata: Some(serde_json::json!({
                    "id": "v1_abc123",
                    "model": "gemini-3.6-flash",
                    "status": "completed",
                    "usage": {
                        "total_tokens": 346,
                        "total_input_tokens": 11,
                        "total_output_tokens": 90,
                    },
                })),
            }]
        );
    }

    /// The shipped endpoint for this dialect, so tests cover the real defaults.
    fn config() -> EndpointConfig {
        EndpointPreset::Gemini.config()
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
        // Nested, not loose: the endpoint rejects `max_output_tokens` at the
        // top level by name.
        assert_eq!(json["generation_config"]["max_output_tokens"], 42);
        assert!(json.get("max_output_tokens").is_none());
    }

    #[test]
    fn sampling_controls_are_nested_together() {
        let request = GenerateRequest::new()
            .message(Message::text(Role::User, "Hi"))
            .max_tokens(42)
            .temperature(0.2)
            .top_p(0.9);

        let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();

        let config = &json["generation_config"];
        assert_eq!(config["max_output_tokens"], 42);
        // Compared through f32: the neutral field is f32, and widening it for
        // JSON leaves 0.2 as 0.20000000298023224.
        assert_eq!(config["temperature"].as_f64(), Some(0.2f32 as f64));
        assert_eq!(config["top_p"].as_f64(), Some(0.9f32 as f64));

        for loose in ["max_output_tokens", "temperature", "top_p"] {
            assert!(json.get(loose).is_none(), "{loose} must not ride loose");
        }
    }

    #[test]
    fn reasoning_effort_is_nested_as_thinking_level() {
        for (effort, expected) in [
            (ReasoningEffort::Low, "low"),
            (ReasoningEffort::Medium, "medium"),
            (ReasoningEffort::High, "high"),
        ] {
            let request = GenerateRequest::new()
                .message(Message::text(Role::User, "Hi"))
                .reasoning_effort(effort);

            let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();

            assert_eq!(json["generation_config"]["thinking_level"], expected);
            // The same trap the sampling controls fell into: loose at the top
            // level the endpoint answers `Unknown parameter`.
            assert!(json.get("thinking_level").is_none());
            assert!(json.get("reasoning_effort").is_none());
        }
    }

    #[test]
    fn an_image_rides_a_model_output_step_as_well_as_a_user_one() {
        // Refused here until the endpoint was asked, which validated an image
        // in a model_output step exactly as it does one in user_input.
        let request = GenerateRequest::new()
            .message(Message::text(Role::User, "Draw a cat"))
            .message(Message::new(
                Role::Assistant,
                vec![InputContent::ImageUrl("https://e.test/cat.png".into())],
            ));

        let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();

        assert_eq!(json["input"][1]["type"], "model_output");
        assert_eq!(json["input"][1]["content"][0]["type"], "image");
        assert_eq!(
            json["input"][1]["content"][0]["uri"],
            "https://e.test/cat.png"
        );
    }

    #[test]
    fn a_tool_turn_still_takes_nothing_but_results() {
        // Not a capability limit and not a refusal: the step a tool turn
        // becomes carries a result, so an image in one is a caller mistake,
        // exactly as text in one already was.
        let request = GenerateRequest::new().message(Message::new(
            Role::Tool,
            vec![InputContent::ImageUrl("https://e.test/a.png".into())],
        ));

        match Request::build(&request, &config()).err() {
            Some(Error::InvalidRequest { message, .. }) => {
                assert_eq!(message, "tool messages may only contain tool results");
            }
            other => panic!("expected InvalidRequest, got {other:?}"),
        }
    }

    #[test]
    fn tool_choice_is_nested_and_named_choice_uses_allowed_tools() {
        let cases = [
            (ToolChoice::Auto, serde_json::json!("auto")),
            (ToolChoice::None, serde_json::json!("none")),
            // Not "required": this API answers `Invalid enum value 'required'`.
            (ToolChoice::Required, serde_json::json!("any")),
            (
                ToolChoice::Named("add".into()),
                serde_json::json!({"allowed_tools": {"mode": "any", "tools": ["add"]}}),
            ),
        ];

        for (choice, expected) in cases {
            let request = GenerateRequest::new()
                .message(Message::text(Role::User, "Hi"))
                .tool_choice(choice.clone());

            let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();

            assert_eq!(
                json["generation_config"]["tool_choice"], expected,
                "{choice:?} mapped wrong"
            );
            assert!(
                json.get("tool_choice").is_none(),
                "loose at the top level this is `Unknown parameter 'tool_choice'`"
            );
        }
    }

    #[test]
    fn reasoning_effort_alone_still_builds_a_generation_config() {
        // It is the fourth field to nest there, and the gate that omits an
        // empty object has to know about it.
        let request = GenerateRequest::new()
            .message(Message::text(Role::User, "Hi"))
            .reasoning_effort(ReasoningEffort::Low);

        let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();

        assert_eq!(json["generation_config"]["thinking_level"], "low");
        assert!(json["generation_config"].get("temperature").is_none());
    }

    #[test]
    fn no_sampling_controls_means_no_generation_config() {
        // An empty object would be sent otherwise, which says something the
        // caller did not.
        let request = GenerateRequest::new().message(Message::text(Role::User, "Hi"));

        let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();

        assert!(json.get("generation_config").is_none());
    }

    #[test]
    fn response_format_carries_the_schema_itself() {
        // This API's `response_format.type` takes JSON Schema type names --
        // object, string, array -- and rejects the `json_schema` wrapper the
        // other three dialects use. So the schema goes there whole.
        let schema = serde_json::json!({
            "type": "object",
            "properties": {"name": {"type": "string"}},
            "required": ["name"],
            "additionalProperties": false
        });
        let request = GenerateRequest::new()
            .message(Message::text(Role::User, "Hi"))
            .response_format(ResponseFormat::JsonSchema {
                name: "person".into(),
                schema: schema.clone(),
                strict: true,
            });

        let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();

        assert_eq!(json["response_format"], schema);
        // `name` and `strict` have nowhere to go here, as on Anthropic.
        assert!(json["response_format"].get("name").is_none());
        assert!(json["response_format"].get("strict").is_none());
    }

    #[test]
    fn the_looser_json_modes_use_plain_type_names() {
        for (format, expected) in [
            (ResponseFormat::Text, "text"),
            (ResponseFormat::JsonObject, "object"),
        ] {
            let request = GenerateRequest::new()
                .message(Message::text(Role::User, "Hi"))
                .response_format(format);

            let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();

            assert_eq!(
                json["response_format"],
                serde_json::json!({"type": expected})
            );
        }
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
            Err(Error::InvalidRequest { .. })
        ));
    }

    #[test]
    fn rejects_a_result_that_answers_a_later_call() {
        // A result may only answer a call that already happened. Scanning the
        // whole transcript up front accepted this and sent it, and the endpoint
        // rejected the step order -- a round trip spent to learn what is
        // decidable here.
        let request = GenerateRequest::new()
            .message(Message::tool_result("call_1", "42"))
            .message(Message::new(
                Role::Assistant,
                vec![InputContent::ToolCall {
                    id: "call_1".into(),
                    name: "add".into(),
                    arguments: "{}".into(),
                }],
            ));

        assert!(matches!(
            Request::build(&request, &config()),
            Err(Error::InvalidRequest { .. })
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

    /// Streaming must reproduce, part for part, what `parse()` builds from the
    /// non-streaming body describing the same logical turn.
    ///
    /// The expectations below are derived from the parser, which is the
    /// specification. `From<Response>` maps `completed` onto
    /// [`ResponseStatus::Completed`], `incomplete` and `budget_exceeded` onto
    /// [`ResponseStatus::Incomplete`], `requires_action` onto
    /// [`ResponseStatus::RequiresAction`], `failed` and `cancelled` onto
    /// [`ResponseStatus::Failed`], and anything else onto
    /// [`ResponseStatus::Other`]. Usage is a straight copy of
    /// `total_input_tokens` / `total_output_tokens` / `total_tokens`.
    /// `convert_step` models exactly `model_output` (its `text` parts) and
    /// `function_call`, and preserves every other step verbatim as
    /// [`OutputContent::Reasoning`]. No shape in this dialect yields
    /// [`OutputContent::Refusal`], so the fixture has none.
    #[test]
    fn streamed_response_matches_generate() {
        // A tool-calling turn: text in two deltas, a thought with a signature,
        // a function call with fragmented arguments, an unmodeled step whose
        // payload arrives as a delta, and a terminal `requires_action`.
        //
        // The call's arguments stream in non-alphabetical key order:
        // convert_step maps them through `Value::to_string`, which sorts the
        // keys, so the streaming path has to sort too.
        let frames = [
            r#"{"index":0,"step":{"type":"model_output"},"event_type":"step.start"}"#,
            r#"{"index":0,"delta":{"type":"text","text":"Hel"},"event_type":"step.delta"}"#,
            r#"{"index":0,"delta":{"type":"text","text":"lo"},"event_type":"step.delta"}"#,
            r#"{"index":0,"event_type":"step.stop"}"#,
            // A second, adjacent model_output step. The parser makes one
            // OutputContent::Text per text part of a step, so the step boundary
            // has to survive the stream rather than merging into the one before.
            r#"{"index":1,"step":{"type":"model_output"},"event_type":"step.start"}"#,
            r#"{"index":1,"delta":{"type":"text","text":"Bye"},"event_type":"step.delta"}"#,
            r#"{"index":1,"event_type":"step.stop"}"#,
            r#"{"index":2,"step":{"type":"thought"},"event_type":"step.start"}"#,
            r#"{"index":2,"delta":{"type":"thought_summary","content":{"type":"text","text":"Working it out."}},"event_type":"step.delta"}"#,
            r#"{"index":2,"delta":{"type":"thought_signature","signature":"sig-abc"},"event_type":"step.delta"}"#,
            r#"{"index":2,"event_type":"step.stop"}"#,
            r#"{"index":3,"step":{"type":"function_call","id":"call_1","name":"get_weather","arguments":{}},"event_type":"step.start"}"#,
            r#"{"index":3,"delta":{"type":"arguments_delta","arguments":"{\"unit\":\"c\","},"event_type":"step.delta"}"#,
            r#"{"index":3,"delta":{"type":"arguments_delta","arguments":"\"location\":\"NYC\"}"},"event_type":"step.delta"}"#,
            r#"{"index":3,"event_type":"step.stop"}"#,
            r#"{"index":4,"step":{"type":"code_execution","language":"python"},"event_type":"step.start"}"#,
            r#"{"index":4,"delta":{"type":"code","code":"print(1)"},"event_type":"step.delta"}"#,
            r#"{"index":4,"event_type":"step.stop"}"#,
            r#"{"interaction":{"id":"v1_abc123","model":"gemini-3.6-flash","status":"requires_action","usage":{"total_input_tokens":11,"total_output_tokens":90,"total_tokens":101}},"event_type":"interaction.completed"}"#,
        ];

        let streamed = crate::dialect::stream::drain_for_test(
            "gemini".into(),
            Box::new(crate::dialect::gemini::Decoder::default()),
            frames
                .iter()
                .map(|frame| format!("data: {frame}\n\n").into_bytes())
                .collect(),
        )
        .expect("drained");

        let parsed = parse(
            r#"{"id":"v1_abc123","model":"gemini-3.6-flash","status":"requires_action","steps":[{"type":"model_output","content":[{"type":"text","text":"Hello"}]},{"type":"model_output","content":[{"type":"text","text":"Bye"}]},{"type":"thought","signature":"sig-abc"},{"type":"function_call","id":"call_1","name":"get_weather","arguments":{"unit":"c","location":"NYC"}},{"type":"code_execution","language":"python","code":"print(1)"}],"usage":{"total_input_tokens":11,"total_output_tokens":90,"total_tokens":101}}"#,
            &config(),
        )
        .expect("parsed");

        assert_eq!(streamed.id, parsed.id);
        assert_eq!(streamed.model, parsed.model);
        assert_eq!(
            streamed.status, parsed.status,
            "the parser maps requires_action, budget_exceeded and cancelled; a \
             tool-calling turn is the case a streaming caller hits most"
        );
        assert_eq!(streamed.usage, parsed.usage);
        assert_eq!(
            streamed.content, parsed.content,
            "content must match part for part, including that two text deltas \
             coalesce into one OutputContent::Text while a second model_output \
             step starts a part of its own, and that an unmodeled step replays \
             with the payload that streamed into it"
        );
        assert_eq!(
            streamed.to_message(),
            parsed.to_message(),
            "the assistant turn replayed into the next request must be identical"
        );
    }

    #[test]
    fn every_effort_level_reaches_the_wire() {
        // Including the three this endpoint rejects. `thinking_level` exists,
        // so which values it likes is the endpoint's answer to give -- Freyja
        // refusing them was a claim about a value, not about the format.
        for (effort, expected) in [
            (ReasoningEffort::None, "none"),
            (ReasoningEffort::Low, "low"),
            (ReasoningEffort::Medium, "medium"),
            (ReasoningEffort::High, "high"),
            (ReasoningEffort::Xhigh, "xhigh"),
            (ReasoningEffort::Max, "max"),
        ] {
            let request = GenerateRequest::new()
                .message(Message::text(Role::User, "Hi"))
                .reasoning_effort(effort);

            let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();

            assert_eq!(json["generation_config"]["thinking_level"], expected);
        }
    }

    #[test]
    fn metadata_reaches_the_wire_as_labels() {
        // This endpoint answers "not available on the Gemini API but it is
        // available on the Gemini Enterprise Agent Platform" -- a deployment
        // gating a field it has, which is the endpoint's business and not a
        // gap in the format.
        let request = GenerateRequest::new()
            .message(Message::text(Role::User, "Hi"))
            .metadata(serde_json::json!({"trace": "abc"}));

        let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();

        assert_eq!(json["labels"]["trace"], "abc");
    }

    #[test]
    fn no_metadata_means_no_labels_key() {
        // Omitted rather than sent as null, so a request that asked for nothing
        // does not carry an empty one.
        let request = GenerateRequest::new().message(Message::text(Role::User, "Hi"));

        let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();

        assert!(json.get("labels").is_none());
    }

    #[test]
    fn rejects_text_in_a_tool_message() {
        let request = GenerateRequest::new().message(Message::text(Role::Tool, "42"));

        assert!(matches!(
            Request::build(&request, &config()),
            Err(Error::InvalidRequest { .. })
        ));
    }
}
