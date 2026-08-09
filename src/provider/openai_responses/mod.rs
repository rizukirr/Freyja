//! OpenAI backend. Transport lives in [`crate::provider::Client`]; this
//! module owns only the wire format.

mod types;

use crate::provider::{GenerateRequest, GenerateResponse, Provider, ProviderConfig, ProviderError};

pub(crate) struct OpenAiResponsesProvider;

impl Provider for OpenAiResponsesProvider {
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

/// Decodes Responses API SSE frames.
///
/// Stateless: every frame carries its own `output_index`, so no correlation
/// has to be remembered between frames.
pub(crate) struct Decoder;

impl StreamDecoder for Decoder {
    fn decode(
        &mut self,
        frame: &SseFrame,
        out: &mut Vec<RawDelta>,
    ) -> Result<(), crate::provider::ProviderError> {
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
                // This dialect's UsageWire has no `#[serde(default)]` fields
                // (types.rs:264-269), so a partial usage object fails the
                // parser outright; yielding no usage is the closest the
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
                    status: Some(match response["status"].as_str() {
                        // Reproduces parse_status arm for arm.
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
                return Err(crate::provider::ProviderError::Stream {
                    provider: "openai".into(),
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
