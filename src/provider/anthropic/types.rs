//! Wire types for the Anthropic Messages API and their conversions to and from
//! the neutral model.

use crate::provider::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const PROVIDER: &str = "Anthropic";
const DEFAULT_MODEL: &str = "claude-opus-5";

/// Anthropic is the only supported provider that *requires* `max_tokens` on
/// every request, so this is the one place Freya has to invent a value. It is a
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
    /// A block Freya models.
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

impl TryFrom<&GenerateRequest> for Request {
    type Error = ProviderError;

    fn try_from(value: &GenerateRequest) -> Result<Self, Self::Error> {
        // Anthropic keeps no server-side transcript; the full history goes on
        // every request, so there is nothing to continue from.
        if value.previous_response_id.is_some() {
            return Err(ProviderError::UnsupportedCapability {
                provider: PROVIDER,
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
                                provider: PROVIDER,
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
                                provider: PROVIDER,
                                capability: "images outside user messages",
                            });
                        }
                        content.push(BlockWire::Typed(TypedBlockWire::Image {
                            source: image_source(url)?,
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
                            input: parse_arguments(arguments)?,
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
                        provider: PROVIDER,
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
                        provider: PROVIDER,
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
            model: value
                .model
                .clone()
                .unwrap_or_else(|| DEFAULT_MODEL.to_string()),
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
        })
    }
}

/// Anthropic wants tool arguments as a structured object, so the neutral JSON
/// string is parsed here. An empty string means "no arguments".
fn parse_arguments(raw: &str) -> Result<Value, ProviderError> {
    if raw.trim().is_empty() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    match serde_json::from_str::<Value>(raw) {
        Ok(value @ Value::Object(_)) => Ok(value),
        _ => Err(ProviderError::InvalidRequest {
            provider: PROVIDER,
            message: format!(
                "tool call arguments must be a JSON object; Anthropic rejects anything else, got '{raw}'"
            ),
        }),
    }
}

/// Builds an image source block. Anthropic distinguishes remote URLs from
/// inline base64, so a data URI has to be split into its media type and payload
/// rather than passed through as a URL.
fn image_source(url: &str) -> Result<Value, ProviderError> {
    let Some(rest) = url.strip_prefix("data:") else {
        return Ok(serde_json::json!({"type": "url", "url": url}));
    };

    let invalid = || ProviderError::InvalidRequest {
        provider: PROVIDER,
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
/// Anything Freya does not model becomes [`OutputContent::Reasoning`] rather
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_neutral_request_to_anthropic_wire_format() {
        let request = GenerateRequest::new()
            .message(Message::text(Role::System, "Be concise"))
            .message(Message::text(Role::User, "Hello"))
            .max_tokens(42);

        let json = serde_json::to_value(Request::try_from(&request).unwrap()).unwrap();

        assert_eq!(json["model"], DEFAULT_MODEL);
        assert_eq!(json["system"], "Be concise");
        assert_eq!(json["max_tokens"], 42);
        assert_eq!(json["messages"][0]["role"], "user");
        assert_eq!(json["messages"][0]["content"][0]["type"], "text");
        assert_eq!(json["messages"][0]["content"][0]["text"], "Hello");
    }

    #[test]
    fn supplies_the_required_max_tokens_when_the_caller_does_not() {
        let request = GenerateRequest::new().message(Message::text(Role::User, "Hello"));

        let json = serde_json::to_value(Request::try_from(&request).unwrap()).unwrap();

        assert_eq!(json["max_tokens"], DEFAULT_MAX_TOKENS);
    }

    #[test]
    fn omits_capabilities_the_caller_did_not_ask_for() {
        let request = GenerateRequest::new().message(Message::text(Role::User, "Hello"));

        let json = serde_json::to_value(Request::try_from(&request).unwrap()).unwrap();

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

        let json = serde_json::to_value(Request::try_from(&request).unwrap()).unwrap();
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

        let json = serde_json::to_value(Request::try_from(&request).unwrap()).unwrap();

        assert_eq!(json["messages"][1]["content"][0], signed);
        assert_eq!(json["messages"][1]["content"][1]["type"], "tool_use");
    }

    #[test]
    fn maps_reasoning_effort_and_tool_choice() {
        let request = GenerateRequest::new()
            .message(Message::text(Role::User, "Hello"))
            .reasoning_effort(ReasoningEffort::Xhigh)
            .tool_choice(ToolChoice::Required);

        let json = serde_json::to_value(Request::try_from(&request).unwrap()).unwrap();

        assert_eq!(json["output_config"]["effort"], "xhigh");
        assert_eq!(json["tool_choice"]["type"], "any");
    }

    #[test]
    fn maps_no_reasoning_effort_onto_disabled_thinking() {
        let request = GenerateRequest::new()
            .message(Message::text(Role::User, "Hello"))
            .reasoning_effort(ReasoningEffort::None);

        let json = serde_json::to_value(Request::try_from(&request).unwrap()).unwrap();

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
                Request::try_from(&request),
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
            Request::try_from(&request),
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

        let json = serde_json::to_value(Request::try_from(&request).unwrap()).unwrap();
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
        // Fields Freya does not model stay reachable.
        assert_eq!(response.provider_metadata.unwrap()["role"], "assistant");
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

        let json = serde_json::to_value(Request::try_from(&follow_up).unwrap()).unwrap();
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
}
