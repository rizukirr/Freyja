//! Wire types for the Anthropic Messages API and their conversions to and from
//! the neutral model.

use crate::provider::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Anthropic is the only supported provider that *requires* `max_tokens` on
/// every request, so this is the one place Freyja has to invent a value. It is a
/// cap, not a target: the model stops early on its own. Set
/// [`GenerateRequest::max_tokens`] to override.
const DEFAULT_MAX_TOKENS: u32 = 16_000;

#[derive(Serialize)]
pub struct Request {
    model: String,
    max_tokens: u32,
    messages: Vec<MessageWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_config: Option<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ToolWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

/// Unlike OpenAI and Gemini, Anthropic nests: tool calls and tool results are
/// content blocks inside a message, not siblings of it. There are only two
/// roles on the wire, and tool results ride on a `user` turn.
#[derive(Serialize)]
struct MessageWire {
    role: &'static str,
    content: Vec<BlockWire>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum BlockWire {
    /// A block Freyja models.
    Typed(TypedBlockWire),
    /// A block preserved from a previous response and replayed verbatim, such
    /// as a signed `thinking` block the model requires back unchanged.
    Raw(Value),
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum TypedBlockWire {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { source: Value },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        /// A structured object, not a JSON string as on OpenAI.
        input: Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

#[derive(Serialize)]
struct ToolWire {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    input_schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    strict: Option<bool>,
}

impl Request {
    /// Converts a neutral request into this dialect's wire format.
    pub(crate) fn build(
        value: &GenerateRequest,
        config: &ProviderConfig,
    ) -> Result<Self, ProviderError> {
        // Anthropic keeps no server-side transcript; the full history goes on
        // every request, so there is nothing to continue from.
        if value.previous_response_id.is_some() {
            return Err(ProviderError::UnsupportedCapability {
                provider: config.name.clone(),
                capability: "server-side conversation continuation",
            });
        }

        let mut system = Vec::new();
        let mut messages = Vec::new();

        for message in &value.messages {
            if matches!(message.role, Role::System | Role::Developer) {
                for content in &message.content {
                    match content {
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

            // Tool results are user-turn blocks here, so Role::Tool collapses
            // into "user". Consecutive same-role turns are legal; the API
            // merges them.
            let role = if message.role == Role::Assistant {
                "assistant"
            } else {
                "user"
            };

            let mut content = Vec::new();
            for part in &message.content {
                match part {
                    InputContent::Text(text) => {
                        content.push(BlockWire::Typed(TypedBlockWire::Text {
                            text: text.clone(),
                        }));
                    }
                    InputContent::ImageUrl(url) => {
                        if message.role != Role::User {
                            return Err(ProviderError::UnsupportedCapability {
                                provider: config.name.clone(),
                                capability: "images outside user messages",
                            });
                        }
                        content.push(BlockWire::Typed(TypedBlockWire::Image {
                            source: image_source(url, config)?,
                        }));
                    }
                    InputContent::ToolCall {
                        id,
                        name,
                        arguments,
                    } => {
                        content.push(BlockWire::Typed(TypedBlockWire::ToolUse {
                            id: id.clone(),
                            name: name.clone(),
                            input: parse_arguments(arguments, config)?,
                        }));
                    }
                    InputContent::ToolResult { call_id, output } => {
                        content.push(BlockWire::Typed(TypedBlockWire::ToolResult {
                            tool_use_id: call_id.clone(),
                            content: output.clone(),
                        }));
                    }
                    InputContent::Reasoning { data } => {
                        content.push(BlockWire::Raw(data.clone()));
                    }
                }
            }

            // A turn with no blocks is rejected by the API; drop it instead.
            if !content.is_empty() {
                messages.push(MessageWire { role, content });
            }
        }

        let mut output_config = serde_json::Map::new();
        let mut thinking = None;

        if let Some(effort) = value.reasoning_effort {
            match effort {
                // The closest honest mapping: no reasoning at all.
                ReasoningEffort::None => {
                    thinking = Some(serde_json::json!({"type": "disabled"}));
                }
                ReasoningEffort::Minimal => {
                    return Err(ProviderError::UnsupportedCapability {
                        provider: config.name.clone(),
                        capability: "reasoning effort 'minimal'",
                    });
                }
                other => {
                    output_config.insert("effort".into(), serde_json::to_value(other).unwrap());
                }
            }
        }

        if let Some(format) = &value.response_format {
            match format {
                // The API's own default.
                ResponseFormat::Text => {}
                ResponseFormat::JsonObject => {
                    return Err(ProviderError::UnsupportedCapability {
                        provider: config.name.clone(),
                        capability: "schema-less JSON response format",
                    });
                }
                ResponseFormat::JsonSchema { schema, .. } => {
                    output_config.insert(
                        "format".into(),
                        serde_json::json!({"type": "json_schema", "schema": schema}),
                    );
                }
            }
        }

        let tool_choice = value.tool_choice.as_ref().map(|choice| match choice {
            ToolChoice::Auto => serde_json::json!({"type": "auto"}),
            ToolChoice::None => serde_json::json!({"type": "none"}),
            // "any" is Anthropic's spelling of "some tool, your pick".
            ToolChoice::Required => serde_json::json!({"type": "any"}),
            ToolChoice::Named(name) => serde_json::json!({"type": "tool", "name": name}),
        });

        Ok(Self {
            model: config.model_for(value)?,
            max_tokens: value.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            messages,
            system: (!system.is_empty()).then(|| system.join("\n\n")),
            temperature: value.temperature,
            top_p: value.top_p,
            thinking,
            output_config: (!output_config.is_empty()).then_some(Value::Object(output_config)),
            tools: value
                .tools
                .iter()
                .map(|tool| ToolWire {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    input_schema: tool.parameters.clone(),
                    strict: tool.strict,
                })
                .collect(),
            tool_choice,
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

/// Anthropic wants tool arguments as a structured object, so the neutral JSON
/// string is parsed here. An empty string means "no arguments".
fn parse_arguments(raw: &str, config: &ProviderConfig) -> Result<Value, ProviderError> {
    if raw.trim().is_empty() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    match serde_json::from_str::<Value>(raw) {
        Ok(value @ Value::Object(_)) => Ok(value),
        _ => Err(ProviderError::InvalidRequest {
            provider: config.name.clone(),
            message: format!(
                "tool call arguments must be a JSON object; Anthropic rejects anything else, got '{raw}'"
            ),
        }),
    }
}

/// Builds an image source block. Anthropic distinguishes remote URLs from
/// inline base64, so a data URI has to be split into its media type and payload
/// rather than passed through as a URL.
fn image_source(url: &str, config: &ProviderConfig) -> Result<Value, ProviderError> {
    let Some(rest) = url.strip_prefix("data:") else {
        return Ok(serde_json::json!({"type": "url", "url": url}));
    };

    let invalid = || ProviderError::InvalidRequest {
        provider: config.name.clone(),
        message: "image data URIs must be of the form 'data:<media-type>;base64,<data>'".into(),
    };

    let (media_type, data) = rest.split_once(',').ok_or_else(invalid)?;
    let media_type = media_type.strip_suffix(";base64").ok_or_else(invalid)?;

    Ok(serde_json::json!({
        "type": "base64",
        "media_type": media_type,
        "data": data,
    }))
}

#[derive(Deserialize)]
pub struct Response {
    id: String,
    model: String,
    #[serde(default)]
    stop_reason: Option<String>,
    /// Content blocks stay as raw values so unrecognized ones, `thinking` above
    /// all, can be replayed verbatim on the next request.
    #[serde(default)]
    content: Vec<Value>,
    usage: Option<UsageWire>,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

/// Anthropic reports no total, and `input_tokens` counts only the *uncached*
/// prompt: cached tokens are billed separately and reported in their own
/// fields. The neutral `Usage` therefore sums all three for the true prompt
/// size. The unsummed fields stay available through `provider_metadata`.
#[derive(Deserialize)]
struct UsageWire {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
}

impl From<Response> for GenerateResponse {
    fn from(value: Response) -> Self {
        let content = value.content.into_iter().flat_map(convert_block).collect();

        let usage = value.usage.map(|u| {
            let input_tokens = u.input_tokens.unwrap_or(0)
                + u.cache_creation_input_tokens.unwrap_or(0)
                + u.cache_read_input_tokens.unwrap_or(0);
            let output_tokens = u.output_tokens.unwrap_or(0);
            Usage {
                input_tokens,
                output_tokens,
                total_tokens: input_tokens + output_tokens,
            }
        });

        Self {
            id: value.id,
            model: value.model,
            status: parse_status(value.stop_reason),
            content,
            usage,
            provider_metadata: Some(Value::Object(value.extra)),
        }
    }
}

/// Converts one content block into neutral output parts.
///
/// Anything Freyja does not model becomes [`OutputContent::Reasoning`] rather
/// than being dropped. `thinking` blocks carry a signature the API validates on
/// the next request, and a transcript that omits or rebuilds one is rejected.
fn convert_block(block: Value) -> Vec<OutputContent> {
    match block.get("type").and_then(Value::as_str) {
        Some("text") => block
            .get("text")
            .and_then(Value::as_str)
            .map(|text| vec![OutputContent::Text(text.to_string())])
            .unwrap_or_default(),
        Some("tool_use") => vec![OutputContent::ToolCall {
            id: block
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            name: block
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            // Stringified so tool arguments read the same on every provider.
            arguments: block
                .get("input")
                .map(|input| input.to_string())
                .unwrap_or_else(|| "{}".to_string()),
        }],
        _ => vec![OutputContent::Reasoning { data: block }],
    }
}

/// Maps `stop_reason` onto the neutral status.
///
/// `refusal` and `pause_turn` are preserved rather than flattened: a refusal is
/// a deliberate non-answer rather than a failure, and a paused turn is resumed
/// by re-sending the transcript, not by supplying a tool result.
fn parse_status(stop_reason: Option<String>) -> ResponseStatus {
    match stop_reason.as_deref() {
        Some("end_turn" | "stop_sequence") | None => ResponseStatus::Completed,
        Some("max_tokens") => ResponseStatus::Incomplete,
        Some("tool_use") => ResponseStatus::RequiresAction,
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
    use crate::provider::ProviderType;

    /// The shipped endpoint for this dialect, so tests cover the real defaults.
    fn config() -> ProviderConfig {
        ProviderType::Anthropic.config()
    }

    #[test]
    fn maps_neutral_request_to_anthropic_wire_format() {
        let request = GenerateRequest::new()
            .message(Message::text(Role::System, "Be concise"))
            .message(Message::text(Role::User, "Hello"))
            .max_tokens(42);

        let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();

        assert_eq!(json["model"], "claude-opus-5");
        assert_eq!(json["system"], "Be concise");
        assert_eq!(json["max_tokens"], 42);
        assert_eq!(json["messages"][0]["role"], "user");
        assert_eq!(json["messages"][0]["content"][0]["type"], "text");
        assert_eq!(json["messages"][0]["content"][0]["text"], "Hello");
    }

    #[test]
    fn supplies_the_required_max_tokens_when_the_caller_does_not() {
        let request = GenerateRequest::new().message(Message::text(Role::User, "Hello"));

        let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();

        assert_eq!(json["max_tokens"], DEFAULT_MAX_TOKENS);
    }

    #[test]
    fn omits_capabilities_the_caller_did_not_ask_for() {
        let request = GenerateRequest::new().message(Message::text(Role::User, "Hello"));

        let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();

        assert!(json.get("thinking").is_none());
        assert!(json.get("output_config").is_none());
        assert!(json.get("tools").is_none());
        assert!(json.get("tool_choice").is_none());
        assert!(json.get("system").is_none());
    }

    #[test]
    fn nests_a_full_tool_round_trip_inside_messages() {
        let request = GenerateRequest::new()
            .message(Message::text(Role::User, "What is 20 + 22?"))
            .message(Message::new(
                Role::Assistant,
                vec![
                    InputContent::Text("Let me add those.".into()),
                    InputContent::ToolCall {
                        id: "toolu_1".into(),
                        name: "add".into(),
                        arguments: "{\"a\":20,\"b\":22}".into(),
                    },
                ],
            ))
            .message(Message::tool_result("toolu_1", "42"));

        let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();
        let messages = json["messages"].as_array().unwrap();

        assert_eq!(messages.len(), 3);

        // Text and tool call stay nested in one assistant turn, in order.
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"][0]["type"], "text");
        assert_eq!(messages[1]["content"][1]["type"], "tool_use");
        assert_eq!(messages[1]["content"][1]["id"], "toolu_1");
        // Arguments become a structured object, not a JSON string.
        assert_eq!(messages[1]["content"][1]["input"]["a"], 20);

        // The tool result rides on a user turn.
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(messages[2]["content"][0]["type"], "tool_result");
        assert_eq!(messages[2]["content"][0]["tool_use_id"], "toolu_1");
        assert_eq!(messages[2]["content"][0]["content"], "42");
    }

    #[test]
    fn preserves_thinking_blocks_in_place() {
        let signed = serde_json::json!({
            "type": "thinking",
            "thinking": "20 plus 22 is 42",
            "signature": "EvACCu0CARFNMg",
        });

        let request = GenerateRequest::new()
            .message(Message::text(Role::User, "What is 20 + 22?"))
            .message(Message::new(
                Role::Assistant,
                vec![
                    InputContent::Reasoning {
                        data: signed.clone(),
                    },
                    InputContent::ToolCall {
                        id: "toolu_1".into(),
                        name: "add".into(),
                        arguments: "{\"a\":20,\"b\":22}".into(),
                    },
                ],
            ));

        let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();

        assert_eq!(json["messages"][1]["content"][0], signed);
        assert_eq!(json["messages"][1]["content"][1]["type"], "tool_use");
    }

    #[test]
    fn maps_reasoning_effort_and_tool_choice() {
        let request = GenerateRequest::new()
            .message(Message::text(Role::User, "Hello"))
            .reasoning_effort(ReasoningEffort::Xhigh)
            .tool_choice(ToolChoice::Required);

        let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();

        assert_eq!(json["output_config"]["effort"], "xhigh");
        assert_eq!(json["tool_choice"]["type"], "any");
    }

    #[test]
    fn maps_no_reasoning_effort_onto_disabled_thinking() {
        let request = GenerateRequest::new()
            .message(Message::text(Role::User, "Hello"))
            .reasoning_effort(ReasoningEffort::None);

        let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();

        assert_eq!(json["thinking"]["type"], "disabled");
        assert!(json.get("output_config").is_none());
    }

    #[test]
    fn refuses_capabilities_it_cannot_express() {
        let unsupported = [
            GenerateRequest::new().previous_response_id("msg_1"),
            GenerateRequest::new().reasoning_effort(ReasoningEffort::Minimal),
            GenerateRequest::new().response_format(ResponseFormat::JsonObject),
        ];

        for request in unsupported {
            assert!(matches!(
                Request::build(&request, &config()),
                Err(ProviderError::UnsupportedCapability { .. })
            ));
        }
    }

    #[test]
    fn rejects_tool_arguments_that_are_not_an_object() {
        let request = GenerateRequest::new().message(Message::new(
            Role::Assistant,
            vec![InputContent::ToolCall {
                id: "toolu_1".into(),
                name: "add".into(),
                arguments: "42".into(),
            }],
        ));

        assert!(matches!(
            Request::build(&request, &config()),
            Err(ProviderError::InvalidRequest { .. })
        ));
    }

    #[test]
    fn splits_a_data_uri_into_a_base64_source() {
        let request = GenerateRequest::new().message(Message::new(
            Role::User,
            vec![InputContent::ImageUrl(
                "data:image/png;base64,iVBORw0KGgo=".into(),
            )],
        ));

        let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();
        let source = &json["messages"][0]["content"][0]["source"];

        assert_eq!(source["type"], "base64");
        assert_eq!(source["media_type"], "image/png");
        assert_eq!(source["data"], "iVBORw0KGgo=");
    }

    #[test]
    fn normalizes_anthropic_response() {
        let wire: Response = serde_json::from_value(serde_json::json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "model": "claude-test",
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "hello"}],
            "usage": {"input_tokens": 2, "output_tokens": 1}
        }))
        .unwrap();

        let response = GenerateResponse::from(wire);

        assert_eq!(response.output_text(), "hello");
        assert_eq!(response.status, ResponseStatus::Completed);
        let usage = response.usage.unwrap();
        assert_eq!(usage.total_tokens, 3);
        // Fields Freyja does not model stay reachable.
        assert_eq!(response.provider_metadata.unwrap()["role"], "assistant");
    }

    #[test]
    fn streamed_response_matches_generate() {
        // The same logical answer, expressed both ways.
        let streamed = crate::provider::stream::drain_for_test(
            "anthropic".into(),
            Box::new(crate::provider::anthropic::Decoder::default()),
            vec![
                b"event: message_start\ndata: {\"message\":{\"id\":\"msg_1\",\"model\":\"claude-sonnet-4\",\"usage\":{\"input_tokens\":11,\"output_tokens\":0}}}\n\n".to_vec(),
                b"event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n".to_vec(),
                b"event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n".to_vec(),
                b"event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n".to_vec(),
                b"event: content_block_stop\ndata: {\"index\":0}\n\n".to_vec(),
                b"event: content_block_start\ndata: {\"index\":1,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n\n".to_vec(),
                b"event: content_block_delta\ndata: {\"index\":1,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Considering.\"}}\n\n".to_vec(),
                b"event: content_block_delta\ndata: {\"index\":1,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig-1\"}}\n\n".to_vec(),
                b"event: content_block_stop\ndata: {\"index\":1}\n\n".to_vec(),
                b"event: content_block_start\ndata: {\"index\":2,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"get_weather\",\"input\":{}}}\n\n".to_vec(),
                b"event: content_block_delta\ndata: {\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"city\\\":\\\"NYC\\\"}\"}}\n\n".to_vec(),
                b"event: content_block_stop\ndata: {\"index\":2}\n\n".to_vec(),
                b"event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":9}}\n\n".to_vec(),
            ],
        )
        .expect("drained");

        let config =
            ProviderConfig::new(ProviderDialect::Anthropic, "anthropic", "https://x.test/v1");
        let parsed = parse(
            r#"{"id":"msg_1","model":"claude-sonnet-4","stop_reason":"tool_use","content":[{"type":"text","text":"Hello"},{"type":"thinking","thinking":"Considering.","signature":"sig-1"},{"type":"tool_use","id":"toolu_1","name":"get_weather","input":{"city":"NYC"}}],"usage":{"input_tokens":11,"output_tokens":9}}"#,
            &config,
        )
        .expect("parsed");

        assert_eq!(streamed.id, parsed.id);
        assert_eq!(streamed.model, parsed.model);
        assert_eq!(streamed.status, parsed.status);
        assert_eq!(streamed.usage, parsed.usage);
        assert_eq!(
            streamed.content, parsed.content,
            "content must match part for part, including that two text deltas \
             coalesce into the single OutputContent::Text the parser produces"
        );
        assert_eq!(streamed.output_text(), "Hello");
        assert!(
            streamed.provider_metadata.is_some(),
            "metadata must be populated, not silently dropped as it was before"
        );
        assert_eq!(
            streamed.content.len(),
            3,
            "text, reasoning blob, and tool call must all survive the stream"
        );
        assert!(streamed.has_tool_calls());
        assert_eq!(
            streamed.to_message(),
            parsed.to_message(),
            "the assistant turn replayed into the next request must be identical, \
             which is the whole point of into_response"
        );
    }

    #[test]
    fn counts_cached_prompt_tokens_toward_the_input_total() {
        let wire: Response = serde_json::from_value(serde_json::json!({
            "id": "msg_1", "model": "claude-test", "stop_reason": "end_turn", "content": [],
            "usage": {
                "input_tokens": 10,
                "output_tokens": 5,
                "cache_creation_input_tokens": 100,
                "cache_read_input_tokens": 1000
            }
        }))
        .unwrap();

        let usage = GenerateResponse::from(wire).usage.unwrap();

        assert_eq!(usage.input_tokens, 1110);
        assert_eq!(usage.total_tokens, 1115);
    }

    #[test]
    fn round_trips_a_tool_call_back_into_a_request() {
        let wire: Response = serde_json::from_value(serde_json::json!({
            "id": "msg_1", "model": "claude-test", "stop_reason": "tool_use",
            "content": [
                {"type": "thinking", "thinking": "adding", "signature": "sig"},
                {"type": "tool_use", "id": "toolu_1", "name": "add", "input": {"a": 20, "b": 22}}
            ]
        }))
        .unwrap();

        let response = GenerateResponse::from(wire);
        assert_eq!(response.status, ResponseStatus::RequiresAction);
        assert_eq!(
            response.tool_calls().collect::<Vec<_>>(),
            vec![("toolu_1", "add", "{\"a\":20,\"b\":22}")]
        );

        // The response feeds straight back in as the next turn.
        let follow_up = GenerateRequest::new()
            .message(Message::text(Role::User, "What is 20 + 22?"))
            .message(response.to_message())
            .message(Message::tool_result("toolu_1", "42"));

        let json = serde_json::to_value(Request::build(&follow_up, &config()).unwrap()).unwrap();
        let messages = json["messages"].as_array().unwrap();

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[1]["content"][0]["type"], "thinking");
        assert_eq!(messages[1]["content"][0]["signature"], "sig");
        assert_eq!(messages[1]["content"][1]["type"], "tool_use");
        assert_eq!(messages[2]["content"][0]["tool_use_id"], "toolu_1");
    }

    #[test]
    fn preserves_a_refusal_rather_than_calling_it_a_failure() {
        let wire: Response = serde_json::from_value(serde_json::json!({
            "id": "msg_1", "model": "claude-test", "stop_reason": "refusal", "content": []
        }))
        .unwrap();

        assert_eq!(
            GenerateResponse::from(wire).status,
            ResponseStatus::Other("refusal".into())
        );
    }

    use crate::provider::sse::SseFrame;
    use crate::provider::stream::{RawDelta, StreamDecoder};

    fn decode_all(frames: &[(&str, &str)]) -> Vec<RawDelta> {
        let mut decoder = crate::provider::anthropic::Decoder::default();
        let mut out = Vec::new();
        for (event, data) in frames {
            let frame = SseFrame {
                event: Some((*event).to_string()),
                data: (*data).to_string(),
            };
            decoder.decode(&frame, &mut out).expect("decodes");
        }
        out
    }

    #[test]
    fn decodes_streaming_text() {
        let deltas = decode_all(&[
            (
                "message_start",
                r#"{"message":{"id":"msg_1","model":"claude-sonnet-4","usage":{"input_tokens":11,"output_tokens":0}}}"#,
            ),
            (
                "content_block_start",
                r#"{"index":0,"content_block":{"type":"text","text":""}}"#,
            ),
            (
                "content_block_delta",
                r#"{"index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
            ),
        ]);

        assert!(
            deltas.iter().any(|d| *d == RawDelta::Text("Hello".into())),
            "{deltas:?}"
        );
    }

    #[test]
    fn decodes_streaming_tool_call() {
        let deltas = decode_all(&[
            (
                "content_block_start",
                r#"{"index":0,"content_block":{"type":"text","text":""}}"#,
            ),
            (
                "content_block_delta",
                r#"{"index":0,"delta":{"type":"text_delta","text":"Let me check."}}"#,
            ),
            ("content_block_stop", r#"{"index":0}"#),
            (
                "content_block_start",
                r#"{"index":1,"content_block":{"type":"tool_use","id":"toolu_01","name":"get_weather","input":{}}}"#,
            ),
            (
                "content_block_delta",
                r#"{"index":1,"delta":{"type":"input_json_delta","partial_json":"{\"loc"}}"#,
            ),
            (
                "content_block_delta",
                r#"{"index":1,"delta":{"type":"input_json_delta","partial_json":"ation\":\"NYC\"}"}}"#,
            ),
            ("content_block_stop", r#"{"index":1}"#),
        ]);

        assert_eq!(
            deltas,
            vec![
                RawDelta::Text("Let me check.".into()),
                RawDelta::ToolStart {
                    slot: 1,
                    id: "toolu_01".into(),
                    name: "get_weather".into(),
                },
                RawDelta::ToolArgs {
                    slot: 1,
                    fragment: "{\"loc".into(),
                },
                RawDelta::ToolArgs {
                    slot: 1,
                    fragment: "ation\":\"NYC\"}".into(),
                },
                RawDelta::ToolEnd { slot: 1 },
            ],
            "the index counts content blocks, so prose ahead of the call \
             pushes it to 1; stopping a text block must emit no ToolEnd"
        );
    }

    #[test]
    fn decodes_streaming_thinking_into_a_replayable_blob() {
        let deltas = decode_all(&[
            (
                "content_block_start",
                r#"{"index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}"#,
            ),
            (
                "content_block_delta",
                r#"{"index":0,"delta":{"type":"thinking_delta","thinking":"Let me work through it."}}"#,
            ),
            (
                "content_block_delta",
                r#"{"index":0,"delta":{"type":"signature_delta","signature":"abc123"}}"#,
            ),
            ("content_block_stop", r#"{"index":0}"#),
        ]);

        assert_eq!(
            deltas[0],
            RawDelta::ReasoningText("Let me work through it.".into())
        );
        assert_eq!(
            deltas[1],
            RawDelta::ReasoningBlob(serde_json::json!({
                "type": "thinking",
                "thinking": "Let me work through it.",
                "signature": "abc123",
            })),
            "the blob must be the whole reconstructed block, because that is \
             what the non-streaming parser produces and what must be replayed"
        );
    }

    #[test]
    fn preserves_unrecognised_blocks_when_streaming() {
        let deltas = decode_all(&[
            (
                "content_block_start",
                r#"{"index":0,"content_block":{"type":"redacted_thinking","data":"EncryptedPayload=="}}"#,
            ),
            ("content_block_stop", r#"{"index":0}"#),
        ]);

        assert_eq!(
            deltas,
            vec![RawDelta::ReasoningBlob(serde_json::json!({
                "type": "redacted_thinking",
                "data": "EncryptedPayload==",
            }))],
            "the non-streaming parser preserves any unmodeled block verbatim; \
             streaming must not silently drop one, or a replayed transcript \
             is incomplete and the provider rejects the next turn"
        );
    }

    #[test]
    fn opaque_blocks_capture_their_streamed_input() {
        let deltas = decode_all(&[
            (
                "content_block_start",
                r#"{"index":0,"content_block":{"type":"server_tool_use","id":"srvtoolu_1","name":"web_search","input":{}}}"#,
            ),
            (
                "content_block_delta",
                r#"{"index":0,"delta":{"type":"input_json_delta","partial_json":"{\"query\":"}}"#,
            ),
            (
                "content_block_delta",
                r#"{"index":0,"delta":{"type":"input_json_delta","partial_json":"\"rust\"}"}}"#,
            ),
            ("content_block_stop", r#"{"index":0}"#),
        ]);

        assert_eq!(
            deltas,
            vec![RawDelta::ReasoningBlob(serde_json::json!({
                "type": "server_tool_use",
                "id": "srvtoolu_1",
                "name": "web_search",
                "input": {"query": "rust"},
            }))],
            "an unmodeled block whose input arrives as deltas must replay with \
             that input filled in; the start frame's empty object is not what \
             the non-streaming parser would have stored"
        );
    }

    #[test]
    fn decodes_streaming_error_frame() {
        let mut decoder = crate::provider::anthropic::Decoder::default();
        let mut out = Vec::new();
        let frame = SseFrame {
            event: Some("error".into()),
            data: r#"{"error":{"type":"overloaded_error","message":"Overloaded"}}"#.into(),
        };

        assert!(matches!(
            decoder.decode(&frame, &mut out),
            Err(ProviderError::Stream { .. })
        ));
    }
}
