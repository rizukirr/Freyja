//! Inbound wire format for the OpenAI Chat Completions API.

use crate::endpoint::EndpointConfig;
use crate::error::Error;
use crate::model::{GenerateResponse, OutputContent, ResponseStatus, Usage};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
pub(crate) struct Response {
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
                "freyja_note".into(),
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
pub(crate) fn parse_finish_reason(reason: Option<String>) -> ResponseStatus {
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
    use crate::dialect::openai_chat::request::Request;
    use crate::{Dialect, GenerateRequest, Message, Role};

    fn config() -> EndpointConfig {
        EndpointConfig::new(Dialect::OpenAiChat, "test-endpoint", "https://api.test/v1")
            .default_model("test-model")
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
}
