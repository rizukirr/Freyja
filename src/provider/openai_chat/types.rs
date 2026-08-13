//! Wire types for the OpenAI Chat Completions API and their conversions to and
//! from the neutral model.
//!
//! This is the format the compatible ecosystem actually speaks. Groq, Together,
//! Fireworks, DeepSeek, OpenRouter, Ollama, vLLM, and others implement it, so
//! this one mapping reaches all of them through [`ProviderConfig`].

use crate::provider::refusal;
use crate::provider::{
    GenerateRequest, GenerateResponse, InputContent, OutputContent, ProviderConfig, ProviderError,
    ReasoningEffort, ResponseFormat, ResponseStatus, Role, TokenLimitField, ToolChoice, Usage,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize)]
pub struct Request {
    model: String,
    messages: Vec<MessageWire>,
    // Exactly one of these carries the cap, chosen by
    // `ProviderConfig::token_limit_field`. Newer OpenAI models reject the
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
    pub(crate) fn build(
        value: &GenerateRequest,
        config: &ProviderConfig,
    ) -> Result<Self, ProviderError> {
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
                            return Err(ProviderError::InvalidRequest {
                                provider: config.name.clone(),
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

#[derive(Deserialize)]
pub struct Response {
    id: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    choices: Vec<ChoiceWire>,
    usage: Option<UsageWire>,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

#[derive(Deserialize)]
struct ChoiceWire {
    #[serde(default)]
    message: ChoiceMessageWire,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct ChoiceMessageWire {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    refusal: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ResponseToolCallWire>,
}

#[derive(Deserialize)]
struct ResponseToolCallWire {
    #[serde(default)]
    id: String,
    #[serde(default)]
    function: ResponseFunctionWire,
}

#[derive(Deserialize, Default)]
struct ResponseFunctionWire {
    #[serde(default)]
    name: String,
    #[serde(default)]
    arguments: String,
}

#[derive(Deserialize)]
struct UsageWire {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

impl From<Response> for GenerateResponse {
    fn from(value: Response) -> Self {
        let mut content = Vec::new();
        let mut status = ResponseStatus::Completed;
        let mut extra = value.extra;

        // Freyja's neutral response models one answer, so the first choice wins.
        // `n > 1` is not expressible in the neutral request, so there is never
        // more than one in practice.
        if let Some(choice) = value.choices.into_iter().next() {
            status = parse_finish_reason(choice.finish_reason);

            if let Some(text) = choice.message.content.filter(|text| !text.is_empty()) {
                content.push(OutputContent::Text(text));
            }
            if let Some(refusal) = choice.message.refusal.filter(|text| !text.is_empty()) {
                content.push(OutputContent::Refusal(refusal));
            }
            for call in choice.message.tool_calls {
                content.push(OutputContent::ToolCall {
                    id: call.id,
                    name: call.function.name,
                    arguments: if call.function.arguments.is_empty() {
                        "{}".to_string()
                    } else {
                        call.function.arguments
                    },
                });
            }
        } else {
            extra.insert(
                "freya_note".into(),
                Value::String("no choices returned".into()),
            );
        }

        Self {
            id: value.id,
            model: value.model,
            status,
            content,
            usage: value.usage.map(|u| Usage {
                input_tokens: u.prompt_tokens,
                output_tokens: u.completion_tokens,
                total_tokens: u.total_tokens,
            }),
            provider_metadata: Some(Value::Object(extra)),
        }
    }
}

/// Maps `finish_reason` onto the neutral status.
///
/// `content_filter` stays as `Other` rather than becoming `Failed`, because the
/// request succeeded and the endpoint chose to withhold part of the answer.
fn parse_finish_reason(reason: Option<String>) -> ResponseStatus {
    match reason.as_deref() {
        Some("stop") | None => ResponseStatus::Completed,
        Some("length") => ResponseStatus::Incomplete,
        // "function_call" is the pre-2023 spelling, still emitted by some
        // compatible endpoints.
        Some("tool_calls" | "function_call") => ResponseStatus::RequiresAction,
        Some(other) => ResponseStatus::Other(other.to_string()),
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
    use crate::provider::{Message, ProviderDialect};
    /// A stand-in endpoint. This dialect ships no preset, because the vendors
    /// speaking it are third party, so the test builds the config the same way
    /// a caller would.
    fn config() -> ProviderConfig {
        ProviderConfig::new(
            ProviderDialect::OpenAiChat,
            "test-endpoint",
            "https://api.test/v1",
        )
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
        // This dialect is reached only through an explicit ProviderConfig --
        // ProviderType::OpenAi is the Responses dialect -- so the default
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
            Err(ProviderError::UnsupportedCapability { .. })
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
            Err(ProviderError::InvalidRequest { .. })
        ));
    }

    #[test]
    fn normalizes_a_chat_completion() {
        let wire: Response = serde_json::from_value(serde_json::json!({
            "id": "chatcmpl-1",
            "object": "chat.completion",
            "model": "deepseek-chat",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "hello"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 2, "completion_tokens": 1, "total_tokens": 3}
        }))
        .unwrap();

        let response = GenerateResponse::from(wire);

        assert_eq!(response.output_text(), "hello");
        assert_eq!(response.status, ResponseStatus::Completed);
        assert_eq!(response.usage.unwrap().total_tokens, 3);
        assert_eq!(
            response.provider_metadata.unwrap()["object"],
            "chat.completion"
        );
    }

    #[test]
    fn round_trips_a_tool_call_back_into_a_request() {
        let wire: Response = serde_json::from_value(serde_json::json!({
            "id": "chatcmpl-1", "model": "deepseek-chat",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1", "type": "function",
                        "function": {"name": "add", "arguments": "{\"a\":20,\"b\":22}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }))
        .unwrap();

        let response = GenerateResponse::from(wire);
        assert_eq!(response.status, ResponseStatus::RequiresAction);
        assert_eq!(
            response.tool_calls().collect::<Vec<_>>(),
            vec![("call_1", "add", "{\"a\":20,\"b\":22}")]
        );

        let follow_up = GenerateRequest::new()
            .message(Message::text(Role::User, "What is 20 + 22?"))
            .message(response.to_message())
            .message(Message::tool_result("call_1", "42"));

        let json = serde_json::to_value(Request::build(&follow_up, &config()).unwrap()).unwrap();
        let messages = json["messages"].as_array().unwrap();

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[1]["tool_calls"][0]["id"], "call_1");
        assert_eq!(messages[2]["role"], "tool");
        assert_eq!(messages[2]["tool_call_id"], "call_1");
    }

    #[test]
    fn maps_finish_reasons() {
        let cases = [
            (Some("stop"), ResponseStatus::Completed),
            (Some("length"), ResponseStatus::Incomplete),
            (Some("tool_calls"), ResponseStatus::RequiresAction),
            (Some("function_call"), ResponseStatus::RequiresAction),
            (None, ResponseStatus::Completed),
            (
                Some("content_filter"),
                ResponseStatus::Other("content_filter".into()),
            ),
        ];

        for (wire, expected) in cases {
            assert_eq!(parse_finish_reason(wire.map(String::from)), expected);
        }
    }

    #[test]
    fn carries_a_refusal_through() {
        let wire: Response = serde_json::from_value(serde_json::json!({
            "id": "chatcmpl-1", "model": "gpt-test",
            "choices": [{
                "message": {"role": "assistant", "content": null, "refusal": "I cannot help"},
                "finish_reason": "stop"
            }]
        }))
        .unwrap();

        let response = GenerateResponse::from(wire);

        assert_eq!(
            response.content,
            vec![OutputContent::Refusal("I cannot help".into())]
        );
        // output_text deliberately excludes refusals.
        assert_eq!(response.output_text(), "");
    }

    /// Streaming must reproduce, part for part, what `parse()` builds from the
    /// non-streaming body describing the same logical turn.
    ///
    /// The expectations below are derived from the parser, which is the
    /// specification. `parse_finish_reason` maps `stop` and a missing reason
    /// onto [`ResponseStatus::Completed`], `length` onto
    /// [`ResponseStatus::Incomplete`], `tool_calls` and `function_call` onto
    /// [`ResponseStatus::RequiresAction`], and anything else onto
    /// [`ResponseStatus::Other`]. Usage is a straight copy of `prompt_tokens` /
    /// `completion_tokens` / `total_tokens`. `From<Response>` builds content in
    /// a fixed order — non-empty `content` as [`OutputContent::Text`], then
    /// non-empty `refusal` as [`OutputContent::Refusal`], then every entry of
    /// `tool_calls` — and models nothing else, so there is no reasoning shape
    /// for this dialect to cover.
    #[test]
    fn streamed_response_matches_generate() {
        // A tool-calling turn: text in two deltas, a refusal, and a call whose
        // arguments arrive fragmented.
        //
        // No second text block here: this dialect's parser builds a single
        // OutputContent::Text from `message.content`, so it has no block
        // boundaries to preserve.
        //
        // Unlike Anthropic and Gemini, this dialect's parser hands back the
        // model's raw `arguments` string untouched, so the streamed order is
        // deliberately left as the model emitted it and is not re-sorted.
        let frames = [
            r#"{"id":"chatcmpl-1","model":"gpt-4o","choices":[{"index":0,"delta":{"role":"assistant","content":"Hel"}}]}"#,
            r#"{"id":"chatcmpl-1","model":"gpt-4o","choices":[{"index":0,"delta":{"content":"lo"}}]}"#,
            r#"{"id":"chatcmpl-1","model":"gpt-4o","choices":[{"index":0,"delta":{"refusal":"I cannot help"}}]}"#,
            r#"{"id":"chatcmpl-1","model":"gpt-4o","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"get_weather","arguments":""}}]}}]}"#,
            r#"{"id":"chatcmpl-1","model":"gpt-4o","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"loc"}}]}}]}"#,
            r#"{"id":"chatcmpl-1","model":"gpt-4o","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"ation\":\"NYC\"}"}}]}}]}"#,
            r#"{"id":"chatcmpl-1","model":"gpt-4o","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
            r#"{"id":"chatcmpl-1","model":"gpt-4o","choices":[],"usage":{"prompt_tokens":11,"completion_tokens":9,"total_tokens":20}}"#,
            "[DONE]",
        ];

        let streamed = crate::provider::stream::drain_for_test(
            "test-endpoint".into(),
            Box::new(crate::provider::openai_chat::Decoder),
            frames
                .iter()
                .map(|frame| format!("data: {frame}\n\n").into_bytes())
                .collect(),
        )
        .expect("drained");

        let parsed = parse(
            r#"{"id":"chatcmpl-1","model":"gpt-4o","choices":[{"index":0,"message":{"role":"assistant","content":"Hello","refusal":"I cannot help","tool_calls":[{"id":"call_1","type":"function","function":{"name":"get_weather","arguments":"{\"location\":\"NYC\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":11,"completion_tokens":9,"total_tokens":20}}"#,
            &config(),
        )
        .expect("parsed");

        assert_eq!(streamed.id, parsed.id);
        assert_eq!(streamed.model, parsed.model);
        assert_eq!(streamed.status, parsed.status);
        assert_eq!(streamed.usage, parsed.usage);
        assert_eq!(
            streamed.content, parsed.content,
            "content must match part for part: two text deltas coalesce into \
             one OutputContent::Text, the refusal becomes OutputContent::Refusal \
             as the parser produces it, and the fragmented call is assembled"
        );
        assert_eq!(
            streamed.to_message(),
            parsed.to_message(),
            "the assistant turn replayed into the next request must be identical"
        );
    }

    use crate::provider::sse::SseFrame;
    use crate::provider::stream::{RawDelta, StreamDecoder};

    fn decode_all(frames: &[&str]) -> Vec<RawDelta> {
        let mut decoder = crate::provider::openai_chat::Decoder;
        let mut out = Vec::new();
        for data in frames {
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
            r#"{"id":"chatcmpl-1","model":"gpt-4o","choices":[{"delta":{"content":"Hel"}}]}"#,
            r#"{"id":"chatcmpl-1","model":"gpt-4o","choices":[{"delta":{"content":"lo"}}]}"#,
            "[DONE]",
        ]);

        assert!(
            deltas.iter().any(|d| *d == RawDelta::Text("Hel".into())),
            "{deltas:?}"
        );
        assert!(
            deltas.iter().any(|d| *d == RawDelta::Text("lo".into())),
            "{deltas:?}"
        );
        assert!(
            !deltas
                .iter()
                .any(|d| matches!(d, RawDelta::Text(t) if t == "[DONE]")),
            "the sentinel must not become text: {deltas:?}"
        );
    }

    #[test]
    fn decodes_streaming_tool_call() {
        let deltas = decode_all(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_abc","function":{"name":"get_weather","arguments":""}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"loc"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"ation\":\"NYC\"}"}}]}}]}"#,
        ]);

        assert_eq!(
            deltas,
            vec![
                RawDelta::ToolStart {
                    slot: 0,
                    id: "call_abc".into(),
                    name: "get_weather".into(),
                },
                RawDelta::ToolArgs {
                    slot: 0,
                    fragment: "{\"loc".into(),
                },
                RawDelta::ToolArgs {
                    slot: 0,
                    fragment: "ation\":\"NYC\"}".into(),
                },
            ],
            "this dialect never ends a call; the assembler flushes at close"
        );
    }

    #[test]
    fn decodes_streaming_usage_and_finish_reason() {
        let deltas = decode_all(&[
            r#"{"id":"chatcmpl-1","model":"gpt-4o","choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            r#"{"id":"chatcmpl-1","model":"gpt-4o","choices":[],"usage":{"prompt_tokens":11,"completion_tokens":9,"total_tokens":20}}"#,
        ]);

        let usage = deltas
            .iter()
            .find_map(|d| match d {
                RawDelta::Meta {
                    usage: Some(usage), ..
                } => Some(*usage),
                _ => None,
            })
            .expect("usage arrives when stream_options.include_usage is set");
        assert_eq!(usage.total_tokens, 20);

        assert!(
            deltas.iter().any(|d| matches!(
                d,
                RawDelta::Meta {
                    status: Some(ResponseStatus::Completed),
                    ..
                }
            )),
            "{deltas:?}"
        );
    }

    /// The parser's `UsageWire` marks every subfield `#[serde(default)]`, so a
    /// partial usage object still yields a `Usage`. The decoder must default
    /// the same way rather than collapsing the whole thing to `None`.
    #[test]
    fn usage_defaults_missing_fields() {
        let deltas = decode_all(&[
            r#"{"id":"chatcmpl-1","model":"gpt-4o","choices":[],"usage":{"prompt_tokens":11,"completion_tokens":9}}"#,
        ]);

        let usage = deltas
            .iter()
            .find_map(|d| match d {
                RawDelta::Meta { usage: Some(u), .. } => Some(*u),
                _ => None,
            })
            .expect(
                "a partial usage object still yields usage, as the parser's \
                 #[serde(default)] fields do — not None",
            );
        assert_eq!(usage.input_tokens, 11);
        assert_eq!(usage.output_tokens, 9);
        assert_eq!(usage.total_tokens, 0);
    }
}
