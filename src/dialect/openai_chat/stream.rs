//! Streaming decoder for the OpenAI Chat Completions API.

use crate::error::Error;
use crate::model::{ResponseStatus, Usage};
use crate::stream::{RawDelta, SseFrame, StreamDecoder};

/// Decodes Chat Completions SSE frames.
///
/// Stateless: `id` and `name` arrive in the first frame of a call and are
/// forwarded as they come, so nothing needs remembering between frames.
pub(crate) struct Decoder;

impl StreamDecoder for Decoder {
    fn decode(
        &mut self,
        frame: &SseFrame,
        provider: &std::sync::Arc<str>,
        out: &mut Vec<RawDelta>,
    ) -> Result<(), Error> {
        // The sentinel is not JSON and carries nothing.
        if frame.data.trim() == "[DONE]" {
            return Ok(());
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&frame.data) else {
            return Ok(());
        };

        // This dialect has no `event:` line to carry a failure, so an error
        // arrives as an ordinary frame with an `error` object in it. Without
        // this the frame matches nothing, the body then closes, and the caller
        // is handed a silently truncated answer reporting `Incomplete` --
        // on the dialect the widest range of third parties speaks.
        if let Some(error) = value.get("error").filter(|error| !error.is_null()) {
            return Err(Error::Stream {
                // The endpoint's own name, never the dialect: see the
                // invariant on Error in model.rs.
                endpoint: provider.clone(),
                message: error["message"]
                    .as_str()
                    .unwrap_or("unknown streaming error")
                    .to_string(),
            });
        }

        let id = value["id"].as_str().map(str::to_string);
        let model = value["model"].as_str().map(str::to_string);
        // Every field of `UsageWire` in response.rs is `#[serde(default)]`, so
        // the parser turns a partial usage object into zeros rather than into no
        // usage at all. Default the same way instead of collapsing to None.
        let usage = value.get("usage").map(|usage| Usage {
            input_tokens: usage["prompt_tokens"].as_u64().unwrap_or(0),
            output_tokens: usage["completion_tokens"].as_u64().unwrap_or(0),
            total_tokens: usage["total_tokens"].as_u64().unwrap_or(0),
        });

        let choice = &value["choices"][0];
        // Mirrors `parse_finish_reason` in response.rs. Kept as its own match
        // rather than shared with the other dialects: the strings differ per
        // provider, and every divergence found in review came from this
        // mapping drifting from the parser's.
        let status = choice["finish_reason"].as_str().map(|reason| match reason {
            "stop" => ResponseStatus::Completed,
            "length" => ResponseStatus::Incomplete,
            // "function_call" is the pre-2023 spelling the parser also accepts.
            "tool_calls" | "function_call" => ResponseStatus::RequiresAction,
            other => ResponseStatus::Other(other.to_string()),
        });

        if let Some(text) = choice["delta"]["content"].as_str()
            && !text.is_empty()
        {
            out.push(RawDelta::Text(text.to_string()));
        }
        if let Some(text) = choice["delta"]["refusal"].as_str()
            && !text.is_empty()
        {
            out.push(RawDelta::Refusal(text.to_string()));
        }
        if let Some(calls) = choice["delta"]["tool_calls"].as_array() {
            for call in calls {
                // This dialect's index counts tool calls only, unlike
                // Anthropic's, which counts content blocks.
                let slot = call["index"].as_u64().unwrap_or(0) as usize;
                if let Some(id) = call["id"].as_str() {
                    out.push(RawDelta::ToolStart {
                        slot,
                        id: id.to_string(),
                        name: call["function"]["name"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                    });
                }
                if let Some(fragment) = call["function"]["arguments"].as_str()
                    && !fragment.is_empty()
                {
                    out.push(RawDelta::ToolArgs {
                        slot,
                        fragment: fragment.to_string(),
                    });
                }
            }
        }

        if id.is_some() || model.is_some() || status.is_some() || usage.is_some() {
            out.push(RawDelta::Meta {
                id,
                model,
                status,
                usage,
                provider_metadata: Some(value.clone()),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::openai_chat::response::parse;
    use crate::dialect::sse::SseFrame;
    use crate::{Dialect, EndpointConfig};

    fn config() -> EndpointConfig {
        EndpointConfig::new(Dialect::OpenAiChat, "test-endpoint", "https://api.test/v1")
            .default_model("test-model")
    }

    /// Streaming must reproduce, part for part, what `parse()` builds from the
    /// non-streaming body describing the same logical turn.
    #[test]
    fn streamed_response_matches_generate() {
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
        let streamed = crate::dialect::stream::drain_for_test(
            "test-endpoint".into(),
            Box::new(Decoder),
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
        assert_eq!(streamed.content, parsed.content);
        assert_eq!(streamed.to_message(), parsed.to_message());
    }

    fn decode_all(frames: &[&str]) -> Vec<RawDelta> {
        let mut decoder = Decoder;
        let mut out = Vec::new();
        for data in frames {
            decoder
                .decode(
                    &SseFrame {
                        event: None,
                        data: (*data).to_string(),
                    },
                    &"test-endpoint".into(),
                    &mut out,
                )
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
        assert!(deltas.iter().any(|d| *d == RawDelta::Text("Hel".into())));
        assert!(deltas.iter().any(|d| *d == RawDelta::Text("lo".into())));
        assert!(
            !deltas
                .iter()
                .any(|d| matches!(d, RawDelta::Text(t) if t == "[DONE]"))
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
            ]
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
        assert!(deltas.iter().any(|d| matches!(
            d,
            RawDelta::Meta {
                status: Some(ResponseStatus::Completed),
                ..
            }
        )));
    }

    #[test]
    fn a_mid_stream_error_frame_fails_the_stream() {
        let mut decoder = Decoder;
        let mut out = Vec::new();
        let result = decoder.decode(
            &SseFrame {
                event: None,
                data: r#"{"error":{"message":"model overloaded","type":"server_error"}}"#.into(),
            },
            &"groq".into(),
            &mut out,
        );
        assert!(matches!(
            result,
            Err(Error::Stream { ref endpoint, ref message })
                if &**endpoint == "groq" && message == "model overloaded"
        ));
    }

    #[test]
    fn an_explicit_null_error_is_not_a_failure() {
        let deltas = decode_all(&[
            r#"{"id":"chatcmpl-1","model":"gpt-4o","error":null,"choices":[{"delta":{"content":"hi"}}]}"#,
        ]);
        assert!(deltas.iter().any(|d| *d == RawDelta::Text("hi".into())));
    }

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
            .expect("a partial usage object still yields usage");
        assert_eq!(usage.input_tokens, 11);
        assert_eq!(usage.output_tokens, 9);
        assert_eq!(usage.total_tokens, 0);
    }
}
