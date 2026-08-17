//! Outbound wire format for the OpenAI Responses API.

use crate::dialect::refusal;
use crate::endpoint::EndpointConfig;
use crate::error::Error;
use crate::model::{
    GenerateRequest, InputContent, ReasoningEffort, ResponseFormat, Role, ToolChoice,
};
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize)]
pub(crate) struct Request {
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
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
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
    pub(crate) fn build(value: &GenerateRequest, config: &EndpointConfig) -> Result<Self, Error> {
        let mut instructions = Vec::new();
        let mut input = Vec::new();

        for message in &value.messages {
            if matches!(message.role, Role::System | Role::Developer) {
                for content in &message.content {
                    match content {
                        InputContent::Text(text) => instructions.push(text.clone()),
                        _ => {
                            return Err(refusal::unsupported(config, refusal::NON_TEXT_SYSTEM));
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
                            return Err(refusal::unsupported(config, refusal::IMAGES_OUTSIDE_USER));
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
                return Err(Error::InvalidRequest {
                    endpoint: config.name.clone(),
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
            stream: None,
        })
    }

    /// Marks this body as a streaming request.
    pub(crate) fn streaming(mut self) -> Self {
        self.stream = Some(true);
        self
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EndpointPreset, Message};

    fn config() -> EndpointConfig {
        EndpointPreset::OpenAi.config()
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
            Err(Error::InvalidRequest { .. })
        ));
    }
}
