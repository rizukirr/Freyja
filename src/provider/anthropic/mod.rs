//! Anthropic backend. Transport lives in [`crate::provider::Client`]; this
//! module owns only the wire format.

mod types;

use crate::provider::{GenerateRequest, GenerateResponse, Provider, ProviderConfig, ProviderError};

pub(crate) struct AnthropicProvider;

impl Provider for AnthropicProvider {
    type Request = types::Request;

    fn build(
        &self,
        request: &GenerateRequest,
        config: &ProviderConfig,
    ) -> Result<Self::Request, ProviderError> {
        types::Request::build(request, config)
    }

    fn parse(
        &self,
        body: &str,
        config: &ProviderConfig,
    ) -> Result<GenerateResponse, ProviderError> {
        types::parse(body, config)
    }
}

use crate::provider::sse::SseFrame;
use crate::provider::stream::{RawDelta, StreamDecoder};
use crate::provider::{ResponseStatus, Usage};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// A thinking block being reassembled, so the replayable blob can be rebuilt
/// in the same shape the non-streaming parser produces.
#[derive(Default)]
struct PendingThinking {
    thinking: String,
    signature: String,
}

/// Decodes Messages API SSE frames.
///
/// Stateful: `content_block_stop` names only an index, so the decoder has to
/// remember which indices were tool calls and which were thinking blocks.
#[derive(Default)]
pub(crate) struct Decoder {
    tools: HashMap<usize, ()>,
    /// Indices of text blocks, so `content_block_stop` can close the block.
    /// convert_block emits one `OutputContent::Text` per text block, and
    /// without the boundary two adjacent blocks would coalesce into one part.
    texts: HashSet<usize>,
    thinking: HashMap<usize, PendingThinking>,
    /// Blocks whose type this decoder does not model, kept whole so
    /// `content_block_stop` can emit them exactly as the parser would, together
    /// with any `input_json_delta` text that arrives after the start frame.
    opaque: HashMap<usize, (Value, String)>,
    input_tokens: u64,
}

impl StreamDecoder for Decoder {
    fn decode(
        &mut self,
        frame: &SseFrame,
        out: &mut Vec<RawDelta>,
    ) -> Result<(), crate::provider::ProviderError> {
        let Ok(value) = serde_json::from_str::<Value>(&frame.data) else {
            return Ok(());
        };
        // The event name is authoritative here; unlike the OpenAI dialects,
        // the payload does not repeat it.
        let event = frame.event.as_deref().unwrap_or_default();
        let index = value["index"].as_u64().unwrap_or(0) as usize;

        match event {
            "error" => {
                return Err(crate::provider::ProviderError::Stream {
                    provider: "anthropic".into(),
                    message: value["error"]["message"]
                        .as_str()
                        .unwrap_or("unknown streaming error")
                        .to_string(),
                });
            }
            "message_start" => {
                let message = &value["message"];
                // The parser sums the plain and both cache token fields into the
                // neutral input total; reading input_tokens alone under-reports
                // a cached prompt by orders of magnitude.
                let usage = &message["usage"];
                self.input_tokens = usage["input_tokens"].as_u64().unwrap_or(0)
                    + usage["cache_creation_input_tokens"].as_u64().unwrap_or(0)
                    + usage["cache_read_input_tokens"].as_u64().unwrap_or(0);
                out.push(RawDelta::Meta {
                    id: message["id"].as_str().map(str::to_string),
                    model: message["model"].as_str().map(str::to_string),
                    status: None,
                    usage: None,
                    provider_metadata: Some(message.clone()),
                });
            }
            "content_block_start" => {
                let block = &value["content_block"];
                match block["type"].as_str() {
                    Some("tool_use") => {
                        self.tools.insert(index, ());
                        out.push(RawDelta::ToolStart {
                            slot: index,
                            id: block["id"].as_str().unwrap_or_default().to_string(),
                            name: block["name"].as_str().unwrap_or_default().to_string(),
                        });
                    }
                    Some("thinking") => {
                        self.thinking.insert(index, PendingThinking::default());
                    }
                    // Text is modeled: it arrives through text_delta, so it must
                    // not be kept as an opaque blob. Only the index is needed,
                    // to close the block at content_block_stop.
                    Some("text") => {
                        self.texts.insert(index);
                    }
                    _ => {
                        self.opaque.insert(index, (block.clone(), String::new()));
                    }
                }
            }
            "content_block_delta" => {
                let delta = &value["delta"];
                match delta["type"].as_str() {
                    Some("text_delta") => {
                        if let Some(text) = delta["text"].as_str() {
                            out.push(RawDelta::Text(text.to_string()));
                        }
                    }
                    Some("input_json_delta") => {
                        if let Some(fragment) = delta["partial_json"].as_str() {
                            if let Some((_, buffer)) = self.opaque.get_mut(&index) {
                                buffer.push_str(fragment);
                            } else {
                                out.push(RawDelta::ToolArgs {
                                    slot: index,
                                    fragment: fragment.to_string(),
                                });
                            }
                        }
                    }
                    Some("thinking_delta") => {
                        if let Some(text) = delta["thinking"].as_str() {
                            if let Some(pending) = self.thinking.get_mut(&index) {
                                pending.thinking.push_str(text);
                            }
                            out.push(RawDelta::ReasoningText(text.to_string()));
                        }
                    }
                    Some("signature_delta") => {
                        if let Some(signature) = delta["signature"].as_str()
                            && let Some(pending) = self.thinking.get_mut(&index)
                        {
                            pending.signature.push_str(signature);
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                if self.tools.remove(&index).is_some() {
                    out.push(RawDelta::ToolEnd { slot: index });
                } else if let Some(pending) = self.thinking.remove(&index) {
                    out.push(RawDelta::ReasoningBlob(serde_json::json!({
                        "type": "thinking",
                        "thinking": pending.thinking,
                        "signature": pending.signature,
                    })));
                } else if let Some((mut block, buffer)) = self.opaque.remove(&index) {
                    // Replace the start frame's empty placeholder with what
                    // actually streamed, so the blob matches the parser's.
                    if !buffer.is_empty()
                        && let Ok(input) = serde_json::from_str::<Value>(&buffer)
                        && let Some(object) = block.as_object_mut()
                    {
                        object.insert("input".into(), input);
                    }
                    out.push(RawDelta::ReasoningBlob(block));
                } else if self.texts.remove(&index) {
                    out.push(RawDelta::TextEnd);
                }
            }
            "message_delta" => {
                let status = value["delta"]["stop_reason"]
                    .as_str()
                    .map(|reason| match reason {
                        "end_turn" | "stop_sequence" => ResponseStatus::Completed,
                        "max_tokens" => ResponseStatus::Incomplete,
                        "tool_use" => ResponseStatus::RequiresAction,
                        other => ResponseStatus::Other(other.to_string()),
                    });
                // Every UsageWire field is `Option` with `#[serde(default)]`
                // (types.rs:319-329) and the parser unwraps each to 0, so a
                // usage object missing `output_tokens` still yields usage.
                let usage = value.get("usage").map(|usage| {
                    let output_tokens = usage["output_tokens"].as_u64().unwrap_or(0);
                    Usage {
                        input_tokens: self.input_tokens,
                        output_tokens,
                        total_tokens: self.input_tokens + output_tokens,
                    }
                });
                out.push(RawDelta::Meta {
                    id: None,
                    model: None,
                    status,
                    // Input tokens arrive in message_start and output tokens
                    // here, so the total is only knowable at this point.
                    usage,
                    provider_metadata: None,
                });
            }
            _ => {}
        }
        Ok(())
    }

    /// convert_block re-serializes the parsed `input` object (types.rs:382-385),
    /// which sorts its keys.
    fn normalizes_tool_arguments(&self) -> bool {
        true
    }
}
