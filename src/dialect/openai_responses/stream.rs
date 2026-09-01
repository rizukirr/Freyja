//! Streaming decoder for the OpenAI Responses API.

use crate::error::Error;
use crate::model::{ResponseStatus, Usage};
use crate::stream::{RawDelta, SseFrame, StreamDecoder};
use serde_json::Value;

/// Decodes Responses API SSE frames.
///
/// Stateless: every frame carries its own `output_index`, so no correlation
/// has to be remembered between frames.
pub(crate) struct Decoder;

impl StreamDecoder for Decoder {
    fn decode(
        &mut self,
        frame: &SseFrame,
        provider: &std::sync::Arc<str>,
        out: &mut Vec<RawDelta>,
    ) -> Result<(), Error> {
        let Ok(value) = serde_json::from_str::<Value>(&frame.data) else {
            return Ok(());
        };
        let event = frame.event.as_deref().unwrap_or_default();
        let slot = value["output_index"].as_u64().unwrap_or(0) as usize;

        match event {
            "response.output_text.delta" => {
                if let Some(text) = value["delta"].as_str() {
                    out.push(RawDelta::Text(text.to_string()));
                }
            }
            // convert_item maps every `output_text` part of a message item to
            // its own OutputContent::Text, and this frame is what ends one such
            // part, so it is the block boundary for this dialect.
            "response.output_text.done" => out.push(RawDelta::TextEnd),
            "response.refusal.delta" => {
                if let Some(text) = value["delta"].as_str() {
                    out.push(RawDelta::Refusal(text.to_string()));
                }
            }
            "response.reasoning_summary_text.delta" => {
                if let Some(text) = value["delta"].as_str() {
                    out.push(RawDelta::ReasoningText(text.to_string()));
                }
            }
            "response.output_item.added" => {
                let item = &value["item"];
                if item["type"] == "function_call" {
                    out.push(RawDelta::ToolStart {
                        slot,
                        // call_id is the id quoted back in a tool result; id is
                        // the item's own handle and is not interchangeable.
                        id: item["call_id"].as_str().unwrap_or_default().to_string(),
                        name: item["name"].as_str().unwrap_or_default().to_string(),
                    });
                }
            }
            "response.output_item.done" => {
                let item = &value["item"];
                // convert_item models exactly `message` and `function_call`;
                // everything else it preserves whole, so streaming must too.
                match item["type"].as_str() {
                    Some("message") | Some("function_call") => {}
                    _ => out.push(RawDelta::ReasoningBlob(item.clone())),
                }
            }
            "response.function_call_arguments.delta" => {
                if let Some(fragment) = value["delta"].as_str() {
                    out.push(RawDelta::ToolArgs {
                        slot,
                        fragment: fragment.to_string(),
                    });
                }
            }
            "response.function_call_arguments.done" => {
                if let Some(arguments) = value["arguments"].as_str() {
                    out.push(RawDelta::ToolReplace {
                        slot,
                        arguments: arguments.to_string(),
                    });
                }
                out.push(RawDelta::ToolEnd { slot });
            }
            "response.completed" | "response.incomplete" | "response.failed" => {
                let response = &value["response"];
                // This dialect's `UsageWire` in response.rs has no
                // `#[serde(default)]` fields, so a partial usage object fails
                // the parser outright; yielding no usage is the closest the
                // streaming path can come, and the `?` below does that.
                let usage = response.get("usage").and_then(|usage| {
                    Some(Usage {
                        input_tokens: usage["input_tokens"].as_u64()?,
                        output_tokens: usage["output_tokens"].as_u64()?,
                        total_tokens: usage["total_tokens"].as_u64()?,
                    })
                });
                out.push(RawDelta::Meta {
                    id: response["id"].as_str().map(str::to_string),
                    model: response["model"].as_str().map(str::to_string),
                    // Mirrors `parse_status` in response.rs, arm for arm. Kept as
                    // its own match rather than shared with the other dialects:
                    // the strings differ per provider, and every divergence
                    // found in review came from this mapping drifting from the
                    // parser's.
                    status: Some(match response["status"].as_str() {
                        Some("completed") => ResponseStatus::Completed,
                        Some("incomplete") => ResponseStatus::Incomplete,
                        Some("requires_action") => ResponseStatus::RequiresAction,
                        Some("failed") => ResponseStatus::Failed,
                        Some(other) => ResponseStatus::Other(other.to_string()),
                        None => ResponseStatus::Completed,
                    }),
                    usage,
                    provider_metadata: Some(response.clone()),
                });
            }
            "error" => {
                return Err(Error::Stream {
                    // The endpoint's own name, never the dialect: see the
                    // invariant on Error in model.rs.
                    endpoint: provider.clone(),
                    message: value["message"]
                        .as_str()
                        .unwrap_or("unknown streaming error")
                        .to_string(),
                });
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EndpointPreset;

    fn config() -> crate::endpoint::EndpointConfig {
        EndpointPreset::OpenAi.config()
    }

    /// Streaming must reproduce, part for part, what `parse()` builds from the
    /// non-streaming body describing the same logical turn.
    ///
    /// The expectations below are derived from the parser, which is the
    /// specification. `parse_status` maps `completed`, `incomplete`,
    /// `requires_action` and `failed` onto their neutral counterparts and
    /// anything else onto [`ResponseStatus::Other`]. Usage is a straight copy of
    /// `input_tokens` / `output_tokens` / `total_tokens`. `convert_item` models
    /// exactly `message`, whose `output_text` parts become
    /// [`OutputContent::Text`] and whose `refusal` parts become
    /// [`OutputContent::Refusal`], and `function_call`, preserving every other
    /// item verbatim as [`OutputContent::Reasoning`].
    #[test]
    fn streamed_response_matches_generate() {
        // A tool-calling turn: a reasoning item, text in two deltas, a refusal,
        // a call with fragmented arguments, and a terminal `requires_action`.
        //
        // Unlike Anthropic and Gemini, this dialect's parser hands back the
        // model's raw `arguments` string untouched, so the streamed order is
        // deliberately left as the model emitted it and is not re-sorted.
        let frames = [
            (
                "response.created",
                r#"{"response":{"id":"resp_1","model":"gpt-5","status":"in_progress"}}"#,
            ),
            (
                "response.output_item.added",
                r#"{"output_index":0,"item":{"type":"reasoning","id":"rs_1","summary":[]}}"#,
            ),
            (
                "response.reasoning_summary_text.delta",
                r#"{"item_id":"rs_1","output_index":0,"delta":"Thinking."}"#,
            ),
            (
                "response.output_item.done",
                r#"{"output_index":0,"item":{"type":"reasoning","id":"rs_1","summary":[{"type":"summary_text","text":"Thinking."}]}}"#,
            ),
            (
                "response.output_item.added",
                r#"{"output_index":1,"item":{"type":"message","id":"msg_1","role":"assistant","content":[]}}"#,
            ),
            (
                "response.output_text.delta",
                r#"{"item_id":"msg_1","output_index":1,"delta":"Hel"}"#,
            ),
            (
                "response.output_text.delta",
                r#"{"item_id":"msg_1","output_index":1,"delta":"lo"}"#,
            ),
            // convert_item makes one OutputContent::Text per output_text part,
            // so this frame ends the first part and the next delta opens a
            // second one rather than merging into it.
            (
                "response.output_text.done",
                r#"{"item_id":"msg_1","output_index":1,"content_index":0,"text":"Hello"}"#,
            ),
            (
                "response.output_text.delta",
                r#"{"item_id":"msg_1","output_index":1,"content_index":1,"delta":"Bye"}"#,
            ),
            (
                "response.output_text.done",
                r#"{"item_id":"msg_1","output_index":1,"content_index":1,"text":"Bye"}"#,
            ),
            (
                "response.refusal.delta",
                r#"{"item_id":"msg_1","output_index":1,"delta":"I cannot help"}"#,
            ),
            (
                "response.output_item.done",
                r#"{"output_index":1,"item":{"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"Hello"},{"type":"output_text","text":"Bye"},{"type":"refusal","refusal":"I cannot help"}]}}"#,
            ),
            (
                "response.output_item.added",
                r#"{"output_index":2,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"get_weather","arguments":""}}"#,
            ),
            (
                "response.function_call_arguments.delta",
                r#"{"item_id":"fc_1","output_index":2,"delta":"{\"loc"}"#,
            ),
            (
                "response.function_call_arguments.done",
                r#"{"item_id":"fc_1","output_index":2,"arguments":"{\"location\":\"NYC\"}"}"#,
            ),
            (
                "response.output_item.done",
                r#"{"output_index":2,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"get_weather","arguments":"{\"location\":\"NYC\"}"}}"#,
            ),
            (
                "response.completed",
                r#"{"response":{"id":"resp_1","model":"gpt-5","status":"requires_action","usage":{"input_tokens":11,"output_tokens":9,"total_tokens":20}}}"#,
            ),
        ];

        let streamed = crate::dialect::stream::drain_for_test(
            "openai".into(),
            Box::new(Decoder),
            frames
                .iter()
                .map(|(event, data)| format!("event: {event}\ndata: {data}\n\n").into_bytes())
                .collect(),
        )
        .expect("drained");

        let parsed = crate::dialect::openai_responses::response::parse(
            r#"{"id":"resp_1","model":"gpt-5","status":"requires_action","output":[{"type":"reasoning","id":"rs_1","summary":[{"type":"summary_text","text":"Thinking."}]},{"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"Hello"},{"type":"output_text","text":"Bye"},{"type":"refusal","refusal":"I cannot help"}]},{"type":"function_call","id":"fc_1","call_id":"call_1","name":"get_weather","arguments":"{\"location\":\"NYC\"}"}],"usage":{"input_tokens":11,"output_tokens":9,"total_tokens":20}}"#,
            &config(),
        )
        .expect("parsed");

        assert_eq!(streamed.id, parsed.id);
        assert_eq!(streamed.model, parsed.model);
        assert_eq!(
            streamed.status, parsed.status,
            "the parser maps requires_action; a tool-calling turn is the case a \
             streaming caller hits most"
        );
        assert_eq!(streamed.usage, parsed.usage);
        assert_eq!(
            streamed.content, parsed.content,
            "content must match part for part: two text deltas coalesce into one \
             OutputContent::Text while a second output_text part becomes a part \
             of its own, the refusal becomes OutputContent::Refusal as the \
             parser produces it, and the unmodeled reasoning item survives"
        );
        assert_eq!(
            streamed.to_message(),
            parsed.to_message(),
            "the assistant turn replayed into the next request must be identical"
        );
    }

    fn decode_all(frames: &[(&str, &str)]) -> Vec<RawDelta> {
        let mut decoder = Decoder;
        let mut out = Vec::new();
        for (event, data) in frames {
            let frame = SseFrame {
                event: Some((*event).to_string()),
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
            (
                "response.created",
                r#"{"response":{"id":"resp_1","model":"gpt-5","status":"in_progress"}}"#,
            ),
            (
                "response.output_text.delta",
                r#"{"item_id":"msg_1","output_index":0,"delta":"Hello"}"#,
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
                "response.output_item.added",
                r#"{"output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_abc","name":"get_weather","arguments":""}}"#,
            ),
            (
                "response.function_call_arguments.delta",
                r#"{"item_id":"fc_1","output_index":0,"delta":"{\"loc"}"#,
            ),
            (
                "response.function_call_arguments.done",
                r#"{"item_id":"fc_1","output_index":0,"arguments":"{\"location\":\"NYC\"}"}"#,
            ),
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
                RawDelta::ToolReplace {
                    slot: 0,
                    arguments: "{\"location\":\"NYC\"}".into(),
                },
                RawDelta::ToolEnd { slot: 0 },
            ],
            "the done frame repeats the complete arguments, so it must replace \
             the buffer rather than append and double-count"
        );
    }

    #[test]
    fn decodes_streaming_completion() {
        let deltas = decode_all(&[(
            "response.completed",
            r#"{"response":{"id":"resp_1","model":"gpt-5","status":"completed","usage":{"input_tokens":11,"output_tokens":9,"total_tokens":20}}}"#,
        )]);

        assert_eq!(
            deltas,
            vec![RawDelta::Meta {
                id: Some("resp_1".into()),
                model: Some("gpt-5".into()),
                status: Some(ResponseStatus::Completed),
                usage: Some(Usage {
                    input_tokens: 11,
                    output_tokens: 9,
                    total_tokens: 20,
                }),
                // The terminal frame's own object, carried whole so a caller
                // can read fields Freyja does not model.
                provider_metadata: Some(serde_json::json!({
                    "id": "resp_1",
                    "model": "gpt-5",
                    "status": "completed",
                    "usage": {
                        "input_tokens": 11,
                        "output_tokens": 9,
                        "total_tokens": 20,
                    },
                })),
            }]
        );
    }

    #[test]
    fn preserves_unrecognised_items_when_streaming() {
        let deltas = decode_all(&[(
            "response.output_item.done",
            r#"{"output_index":0,"item":{"type":"web_search_call","id":"ws_1","status":"completed"}}"#,
        )]);

        assert_eq!(
            deltas,
            vec![RawDelta::ReasoningBlob(serde_json::json!({
                "type": "web_search_call",
                "id": "ws_1",
                "status": "completed",
            }))],
            "convert_item catch-alls every item that is not message or \
             function_call; streaming must preserve the same set or a replayed \
             transcript loses items the provider expects back"
        );
    }
}
