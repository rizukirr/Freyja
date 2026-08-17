//! Outbound wire format for the OpenAI Chat Completions API.
//!
//! This is the format the compatible ecosystem actually speaks. Groq, Together,
//! Fireworks, DeepSeek, OpenRouter, Ollama, vLLM, and others implement it, so
//! this one mapping reaches all of them through [`EndpointConfig`].

use crate::dialect::refusal;
use crate::endpoint::{EndpointConfig, TokenLimitField};
use crate::error::Error;
use crate::model::{
    GenerateRequest, InputContent, ReasoningEffort, ResponseFormat, Role, ToolChoice,
};
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize)]
pub struct Request {
    model: String,
    messages: Vec<MessageWire>,
    // Exactly one of these carries the cap, chosen by
    // `EndpointConfig::token_limit_field`. Newer OpenAI models reject the
    // presence of `max_tokens`, not just its value, so sending both to cover
    // the two spellings fails on the endpoint that needs the new one.
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_completion_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<ReasoningEffort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ToolWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<serde_json::Value>,
}

/// Unlike every other dialect Freyja speaks, `system` is a real message role
/// here rather than a separate top-level field, so system turns stay in the
/// array instead of being hoisted out of it.
#[derive(Serialize)]
struct MessageWire {
    role: &'static str,
    /// Serialized as `null` rather than omitted when an assistant turn carries
    /// only tool calls, which is the shape the API documents.
    content: Option<ContentWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ToolCallWire>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

/// Content is a bare string or an array of parts. Freyja sends the string form
/// whenever it can, because the simpler compatible endpoints accept only that.
#[derive(Serialize)]
#[serde(untagged)]
enum ContentWire {
    Text(String),
    Parts(Vec<PartWire>),
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum PartWire {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    Image { image_url: ImageUrlWire },
}

#[derive(Serialize)]
struct ImageUrlWire {
    url: String,
}

#[derive(Serialize)]
struct ToolCallWire {
    id: String,
    #[serde(rename = "type")]
    kind: &'static str,
    function: FunctionCallWire,
}

#[derive(Serialize)]
struct FunctionCallWire {
    name: String,
    /// A JSON string, as in the Responses API and unlike Gemini or Anthropic.
    arguments: String,
}

#[derive(Serialize)]
struct ToolWire {
    #[serde(rename = "type")]
    kind: &'static str,
    function: FunctionWire,
}

#[derive(Serialize)]
struct FunctionWire {
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
        // Chat Completions is stateless; the whole transcript goes every time.
        if value.previous_response_id.is_some() {
            return Err(refusal::unsupported(
                config,
                refusal::CONVERSATION_CONTINUATION,
            ));
        }

        let mut messages = Vec::new();

        for message in &value.messages {
            let role = match message.role {
                // "developer" exists on recent OpenAI models but not on most
                // compatible endpoints, so both map to the portable spelling.
                Role::System | Role::Developer => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            };

            let mut text = Vec::new();
            let mut parts = Vec::new();
            let mut tool_calls = Vec::new();
            let mut tool_call_id = None;

            for part in &message.content {
                match part {
                    InputContent::Text(value) => {
                        text.push(value.clone());
                        parts.push(PartWire::Text {
                            text: value.clone(),
                        });
                    }
                    // Every role takes one. Verified live: an image part on an
                    // assistant turn and on a tool turn both returned
                    // completions, as did one in a system turn. Freyja refused
                    // all three until they were tried.
                    InputContent::ImageUrl(url) => {
                        parts.push(PartWire::Image {
                            image_url: ImageUrlWire { url: url.clone() },
                        });
                    }
                    InputContent::ToolCall {
                        id,
                        name,
                        arguments,
                    } => tool_calls.push(ToolCallWire {
                        id: id.clone(),
                        kind: "function",
                        function: FunctionCallWire {
                            name: name.clone(),
                            arguments: arguments.clone(),
                        },
                    }),
                    InputContent::ToolResult { call_id, output } => {
                        // A tool turn answers exactly one call here, unlike
                        // Anthropic where several results share a user turn.
                        if tool_call_id.is_some() {
                            return Err(Error::InvalidRequest {
                                endpoint: config.name.clone(),
                                message: "each tool message may answer only one tool call; \
                                          send one message per result"
                                    .into(),
                            });
                        }
                        tool_call_id = Some(call_id.clone());
                        text.push(output.clone());
                        // Into both, because the two are alternative renderings
                        // of the same turn and an image elsewhere in it decides
                        // which one is sent. Pushing only to `text` would drop
                        // this output whenever the parts form won.
                        parts.push(PartWire::Text {
                            text: output.clone(),
                        });
                    }
                    // No standard place for opaque reasoning state in this
                    // format, and no replay requirement either, so it is left
                    // behind rather than rejected. See the provider docs.
                    InputContent::Reasoning { .. } => {}
                }
            }

            let has_image = parts
                .iter()
                .any(|part| matches!(part, PartWire::Image { .. }));

            let content = if has_image {
                Some(ContentWire::Parts(parts))
            } else if !text.is_empty() {
                Some(ContentWire::Text(text.join("\n")))
            } else {
                None
            };

            if content.is_none() && tool_calls.is_empty() {
                continue;
            }

            messages.push(MessageWire {
                role,
                content,
                tool_calls: (!tool_calls.is_empty()).then_some(tool_calls),
                tool_call_id,
            });
        }

        let response_format = value.response_format.as_ref().map(|format| match format {
            ResponseFormat::Text => serde_json::json!({"type": "text"}),
            ResponseFormat::JsonObject => serde_json::json!({"type": "json_object"}),
            ResponseFormat::JsonSchema {
                name,
                schema,
                strict,
            } => serde_json::json!({
                "type": "json_schema",
                "json_schema": {"name": name, "schema": schema, "strict": strict},
            }),
        });

        let tool_choice = value.tool_choice.as_ref().map(|choice| match choice {
            ToolChoice::Auto => Value::String("auto".into()),
            ToolChoice::None => Value::String("none".into()),
            ToolChoice::Required => Value::String("required".into()),
            ToolChoice::Named(name) => {
                serde_json::json!({"type": "function", "function": {"name": name}})
            }
        });

        Ok(Self {
            model: config.model_for(value)?,
            messages,
            max_tokens: match config.token_limit_field {
                TokenLimitField::MaxTokens => value.max_tokens,
                _ => None,
            },
            max_completion_tokens: match config.token_limit_field {
                TokenLimitField::MaxCompletionTokens => value.max_tokens,
                _ => None,
            },
            temperature: value.temperature,
            top_p: value.top_p,
            reasoning_effort: value.reasoning_effort,
            response_format,
            tools: value
                .tools
                .iter()
                .map(|tool| ToolWire {
                    kind: "function",
                    function: FunctionWire {
                        name: tool.name.clone(),
                        description: tool.description.clone(),
                        parameters: tool.parameters.clone(),
                        strict: tool.strict,
                    },
                })
                .collect(),
            tool_choice,
            metadata: value.metadata.clone(),
            stream: None,
            stream_options: None,
        })
    }

    /// Marks this body as a streaming request.
    ///
    /// `include_usage` is required or the dialect reports no token counts at
    /// all when streaming, which would leave `Done.usage` empty on the most
    /// widely-spoken dialect.
    pub(crate) fn streaming(mut self) -> Self {
        self.stream = Some(true);
        self.stream_options = Some(serde_json::json!({"include_usage": true}));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Dialect, Message};
    /// A stand-in endpoint. This dialect ships no preset, because the vendors
    /// speaking it are third party, so the test builds the config the same way
    /// a caller would.
    fn config() -> EndpointConfig {
        EndpointConfig::new(Dialect::OpenAiChat, "test-endpoint", "https://api.test/v1")
            .default_model("test-model")
    }

    #[test]
    fn keeps_system_turns_in_the_message_array() {
        let request = GenerateRequest::new()
            .message(Message::text(Role::System, "Be concise"))
            .message(Message::text(Role::User, "Hello"));

        let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();

        // Every other dialect hoists these out. This one must not.
        assert!(json.get("system").is_none());
        assert!(json.get("instructions").is_none());
        assert_eq!(json["messages"][0]["role"], "system");
        assert_eq!(json["messages"][0]["content"], "Be concise");
        assert_eq!(json["messages"][1]["role"], "user");
    }

    #[test]
    fn the_token_cap_defaults_to_the_field_the_ecosystem_implements() {
        // This dialect is reached only through an explicit EndpointConfig --
        // EndpointPreset::OpenAi is the Responses dialect -- so the default
        // serves the compatible vendors it exists for.
        let request = GenerateRequest::new()
            .message(Message::text(Role::User, "Hi"))
            .max_tokens(16);

        let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();

        assert_eq!(json["max_tokens"], 16);
        assert!(
            json.get("max_completion_tokens").is_none(),
            "only one spelling may be sent"
        );
    }

    #[test]
    fn the_token_cap_moves_to_the_field_openai_now_requires() {
        // Newer OpenAI models reject the presence of `max_tokens`, so a
        // request carrying both fails on exactly the endpoint that needs the
        // new one. Sending one or the other is the whole point.
        let config = config().token_limit_field(TokenLimitField::MaxCompletionTokens);
        let request = GenerateRequest::new()
            .message(Message::text(Role::User, "Hi"))
            .max_tokens(16);

        let json = serde_json::to_value(Request::build(&request, &config).unwrap()).unwrap();

        assert_eq!(json["max_completion_tokens"], 16);
        assert!(
            json.get("max_tokens").is_none(),
            "the old spelling must not ride along"
        );
    }

    #[test]
    fn an_unset_cap_sends_neither_field() {
        let request = GenerateRequest::new().message(Message::text(Role::User, "Hi"));

        for field in [
            TokenLimitField::MaxTokens,
            TokenLimitField::MaxCompletionTokens,
        ] {
            let config = config().token_limit_field(field);
            let json = serde_json::to_value(Request::build(&request, &config).unwrap()).unwrap();

            assert!(json.get("max_tokens").is_none(), "{field:?}");
            assert!(json.get("max_completion_tokens").is_none(), "{field:?}");
        }
    }

    #[test]
    fn maps_developer_onto_system_for_portability() {
        let request = GenerateRequest::new().message(Message::text(Role::Developer, "Rules"));

        let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();

        assert_eq!(json["messages"][0]["role"], "system");
    }

    #[test]
    fn omits_capabilities_the_caller_did_not_ask_for() {
        let request = GenerateRequest::new().message(Message::text(Role::User, "Hello"));

        let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();

        assert!(json.get("tools").is_none());
        assert!(json.get("tool_choice").is_none());
        assert!(json.get("reasoning_effort").is_none());
        assert!(json.get("response_format").is_none());
        assert!(json.get("max_tokens").is_none());
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
        let messages = json["messages"].as_array().unwrap();

        assert_eq!(messages.len(), 3);

        // Tool calls nest on the assistant message, beside its text.
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"], "Let me add those.");
        assert_eq!(messages[1]["tool_calls"][0]["id"], "call_1");
        assert_eq!(messages[1]["tool_calls"][0]["type"], "function");
        assert_eq!(messages[1]["tool_calls"][0]["function"]["name"], "add");
        // Arguments stay a JSON string here, unlike Gemini and Anthropic.
        assert_eq!(
            messages[1]["tool_calls"][0]["function"]["arguments"],
            "{\"a\":20,\"b\":22}"
        );

        // The result gets its own `tool` role message, a role no other dialect uses.
        assert_eq!(messages[2]["role"], "tool");
        assert_eq!(messages[2]["tool_call_id"], "call_1");
        assert_eq!(messages[2]["content"], "42");
    }

    #[test]
    fn sends_null_content_for_a_tool_only_assistant_turn() {
        let request = GenerateRequest::new().message(Message::new(
            Role::Assistant,
            vec![InputContent::ToolCall {
                id: "call_1".into(),
                name: "add".into(),
                arguments: "{}".into(),
            }],
        ));

        let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();

        // Present and null, not omitted.
        assert!(json["messages"][0].get("content").is_some());
        assert!(json["messages"][0]["content"].is_null());
    }

    #[test]
    fn uses_the_parts_array_only_when_an_image_is_present() {
        let text_only = GenerateRequest::new().message(Message::text(Role::User, "Hello"));
        let json = serde_json::to_value(Request::build(&text_only, &config()).unwrap()).unwrap();
        assert!(json["messages"][0]["content"].is_string());

        let with_image = GenerateRequest::new().message(Message::new(
            Role::User,
            vec![
                InputContent::Text("What is this?".into()),
                InputContent::ImageUrl("https://example.com/cat.png".into()),
            ],
        ));
        let json = serde_json::to_value(Request::build(&with_image, &config()).unwrap()).unwrap();
        let content = &json["messages"][0]["content"];
        assert!(content.is_array());
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "image_url");
        assert_eq!(
            content[1]["image_url"]["url"],
            "https://example.com/cat.png"
        );
    }

    #[test]
    fn every_role_carries_an_image() {
        // Refused here until the endpoint was asked. An image part on an
        // assistant turn and on a tool turn both came back with completions,
        // as did one in a system turn.
        for role in [Role::System, Role::Assistant, Role::Tool, Role::User] {
            let request = GenerateRequest::new().message(Message::new(
                role,
                vec![InputContent::ImageUrl("https://e.test/a.png".into())],
            ));

            let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();

            assert_eq!(
                json["messages"][0]["content"][0]["type"], "image_url",
                "{role:?} should carry an image"
            );
        }
    }

    #[test]
    fn a_tool_result_survives_an_image_in_the_same_turn() {
        // The turn has two renderings and an image decides which one is sent,
        // so the result has to be in both. Reaching only the string form would
        // drop the tool's answer the moment an image joined it.
        let request = GenerateRequest::new().message(Message::new(
            Role::Tool,
            vec![
                InputContent::ToolResult {
                    call_id: "call_1".into(),
                    output: "42".into(),
                },
                InputContent::ImageUrl("https://e.test/a.png".into()),
            ],
        ));

        let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();
        let content = &json["messages"][0]["content"];

        assert_eq!(json["messages"][0]["tool_call_id"], "call_1");
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "42");
        assert_eq!(content[1]["type"], "image_url");
    }

    #[test]
    fn drops_reasoning_blocks_carried_from_another_dialect() {
        let request = GenerateRequest::new().message(Message::new(
            Role::Assistant,
            vec![
                InputContent::Reasoning {
                    data: serde_json::json!({"type": "thinking", "signature": "sig"}),
                },
                InputContent::Text("Hello".into()),
            ],
        ));

        let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();

        // No replay requirement here, so a transcript from Anthropic or Gemini
        // still sends rather than failing.
        assert_eq!(json["messages"][0]["content"], "Hello");
    }

    #[test]
    fn refuses_capabilities_it_cannot_express() {
        let request = GenerateRequest::new().previous_response_id("chatcmpl-1");

        assert!(matches!(
            Request::build(&request, &config()),
            Err(Error::UnsupportedCapability { .. })
        ));
    }

    #[test]
    fn rejects_a_tool_message_answering_two_calls() {
        let request = GenerateRequest::new().message(Message::new(
            Role::Tool,
            vec![
                InputContent::ToolResult {
                    call_id: "call_1".into(),
                    output: "1".into(),
                },
                InputContent::ToolResult {
                    call_id: "call_2".into(),
                    output: "2".into(),
                },
            ],
        ));

        assert!(matches!(
            Request::build(&request, &config()),
            Err(Error::InvalidRequest { .. })
        ));
    }
}
