//! OpenAI Chat Completions backend. Transport lives in
//! [`crate::provider::Client`]; this module owns only the wire format.

mod types;

use crate::provider::{GenerateRequest, GenerateResponse, Provider, ProviderConfig, ProviderError};

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

use crate::provider::sse::SseFrame;
use crate::provider::stream::{RawDelta, StreamDecoder};
use crate::provider::{ResponseStatus, Usage};

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
        let usage = value.get("usage").and_then(|usage| {
            Some(Usage {
                input_tokens: usage["prompt_tokens"].as_u64()?,
                output_tokens: usage["completion_tokens"].as_u64()?,
                total_tokens: usage["total_tokens"].as_u64()?,
            })
        });

        let choice = &value["choices"][0];
        let status = choice["finish_reason"].as_str().map(|reason| match reason {
            "stop" => ResponseStatus::Completed,
            "length" => ResponseStatus::Incomplete,
            "tool_calls" => ResponseStatus::RequiresAction,
            other => ResponseStatus::Other(other.to_string()),
        });

        if let Some(text) = choice["delta"]["content"].as_str()
            && !text.is_empty()
        {
            out.push(RawDelta::Text(text.to_string()));
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
            });
        }
        Ok(())
    }
}
