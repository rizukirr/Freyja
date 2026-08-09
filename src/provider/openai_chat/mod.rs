//! OpenAI Chat Completions backend. Transport lives in
//! [`crate::provider::Client`]; this module owns only the wire format.

mod types;

use crate::provider::sse::SseFrame;
use crate::provider::stream::{RawDelta, StreamDecoder};
use crate::provider::{GenerateRequest, GenerateResponse, Provider, ProviderConfig, ProviderError};
use crate::provider::{ResponseStatus, Usage};

pub(crate) struct OpenAiChatProvider;

impl Provider for OpenAiChatProvider {
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

/// Decodes Chat Completions SSE frames.
///
/// Stateless: `id` and `name` arrive in the first frame of a call and are
/// forwarded as they come, so nothing needs remembering between frames.
pub(crate) struct Decoder;

impl StreamDecoder for Decoder {
    fn decode(
        &mut self,
        frame: &SseFrame,
        out: &mut Vec<RawDelta>,
    ) -> Result<(), crate::provider::ProviderError> {
        // The sentinel is not JSON and carries nothing.
        if frame.data.trim() == "[DONE]" {
            return Ok(());
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&frame.data) else {
            return Ok(());
        };

        let id = value["id"].as_str().map(str::to_string);
        let model = value["model"].as_str().map(str::to_string);
        // Every UsageWire field is `#[serde(default)]` (types.rs:322-331), so
        // the parser turns a partial usage object into zeros rather than into
        // no usage at all. Default the same way instead of collapsing to None.
        let usage = value.get("usage").map(|usage| Usage {
            input_tokens: usage["prompt_tokens"].as_u64().unwrap_or(0),
            output_tokens: usage["completion_tokens"].as_u64().unwrap_or(0),
            total_tokens: usage["total_tokens"].as_u64().unwrap_or(0),
        });

        let choice = &value["choices"][0];
        // Mirrors `parse_finish_reason` in types.rs. Kept as its own match
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

        // The parser keeps a refusal as its own content part, after the text.
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
