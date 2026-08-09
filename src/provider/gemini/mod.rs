//! Gemini backend. Transport lives in [`crate::provider::Client`]; this
//! module owns only the wire format.

mod types;

use crate::provider::{GenerateRequest, GenerateResponse, Provider, ProviderConfig, ProviderError};

pub(crate) struct GeminiProvider;

impl Provider for GeminiProvider {
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
use std::collections::HashMap;

/// What kind of step an index refers to.
///
/// `step.stop` names only an index, so the decoder has to remember what was
/// started there.
enum Step {
    Tool,
    /// The step as it arrived, plus the signature accumulated from its deltas.
    /// Kept whole because the non-streaming parser stores the whole step and
    /// the API requires model-generated steps replayed exactly as received.
    Thought {
        step: Value,
        signature: String,
    },
    /// A step type this decoder does not model, kept verbatim.
    Opaque(Value),
}

/// Decodes Interactions API SSE frames.
#[derive(Default)]
pub(crate) struct Decoder {
    steps: HashMap<usize, Step>,
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
        // This dialect repeats the event name inside the payload, so the SSE
        // event line is redundant and the body is the single source.
        let event = value["event_type"].as_str().unwrap_or_default();
        let slot = value["index"].as_u64().unwrap_or(0) as usize;

        match event {
            "step.start" => {
                let step = &value["step"];
                match step["type"].as_str() {
                    Some("function_call") => {
                        self.steps.insert(slot, Step::Tool);
                        out.push(RawDelta::ToolStart {
                            slot,
                            id: step["id"].as_str().unwrap_or_default().to_string(),
                            name: step["name"].as_str().unwrap_or_default().to_string(),
                        });
                    }
                    Some("thought") => {
                        self.steps.insert(
                            slot,
                            Step::Thought {
                                step: step.clone(),
                                signature: String::new(),
                            },
                        );
                    }
                    // Model output is modeled: it arrives through text deltas
                    // and must not be kept as an opaque blob.
                    Some("model_output") => {}
                    _ => {
                        self.steps.insert(slot, Step::Opaque(step.clone()));
                    }
                }
            }
            "step.delta" => {
                let delta = &value["delta"];
                match delta["type"].as_str() {
                    Some("text") => {
                        if let Some(text) = delta["text"].as_str() {
                            out.push(RawDelta::Text(text.to_string()));
                        }
                    }
                    Some("arguments_delta") => {
                        if let Some(fragment) = delta["arguments"].as_str() {
                            out.push(RawDelta::ToolArgs {
                                slot,
                                fragment: fragment.to_string(),
                            });
                        }
                    }
                    Some("thought_summary") => {
                        if let Some(text) = delta["content"]["text"].as_str() {
                            out.push(RawDelta::ReasoningText(text.to_string()));
                        }
                    }
                    Some("thought_signature") => {
                        if let Some(value) = delta["signature"].as_str()
                            && let Some(Step::Thought { signature, .. }) = self.steps.get_mut(&slot)
                        {
                            signature.push_str(value);
                        }
                    }
                    _ => {}
                }
            }
            "step.stop" => match self.steps.remove(&slot) {
                Some(Step::Tool) => out.push(RawDelta::ToolEnd { slot }),
                Some(Step::Thought {
                    mut step,
                    signature,
                }) => {
                    // Merge the streamed signature back into the step the API
                    // sent, so the blob matches what the parser would store.
                    if !signature.is_empty()
                        && let Some(object) = step.as_object_mut()
                    {
                        object.insert("signature".into(), Value::String(signature));
                    }
                    out.push(RawDelta::ReasoningBlob(step));
                }
                Some(Step::Opaque(step)) => out.push(RawDelta::ReasoningBlob(step)),
                None => {}
            },
            "interaction.completed" | "interaction.failed" | "interaction.incomplete" => {
                let interaction = &value["interaction"];
                let usage = interaction.get("usage").and_then(|usage| {
                    Some(Usage {
                        input_tokens: usage["total_input_tokens"].as_u64()?,
                        output_tokens: usage["total_output_tokens"].as_u64()?,
                        total_tokens: usage["total_tokens"].as_u64()?,
                    })
                });
                out.push(RawDelta::Meta {
                    id: interaction["id"].as_str().map(str::to_string),
                    model: interaction["model"].as_str().map(str::to_string),
                    status: Some(match interaction["status"].as_str() {
                        Some("completed") => ResponseStatus::Completed,
                        Some("incomplete") => ResponseStatus::Incomplete,
                        Some("failed") => ResponseStatus::Failed,
                        Some(other) => ResponseStatus::Other(other.to_string()),
                        None => ResponseStatus::Completed,
                    }),
                    usage,
                });
            }
            _ => {}
        }
        Ok(())
    }
}
