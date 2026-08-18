//! Streaming decoder for the Gemini Interactions API.

use crate::error::Error;
use crate::model::{ResponseStatus, Usage};
use crate::stream::{RawDelta, SseFrame, StreamDecoder};
use serde_json::Value;
use std::collections::HashMap;

/// What kind of step an index refers to.
///
/// `step.stop` names only an index, so the decoder has to remember what was
/// started there.
enum Step {
    Tool,
    /// A `model_output` step. convert_step emits one `OutputContent::Text` per
    /// text part of one such step, so its stop closes the text block; without
    /// that, two adjacent model_output steps would coalesce into one part.
    ModelOutput,
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

/// Folds one `step.delta` payload into the step it belongs to.
///
/// `type` names the delta, not the step, so it is never copied. String fields
/// append, since that is what a delta of a string means; anything else is the
/// latest value and replaces what was there.
fn merge_delta(step: &mut Value, delta: &Value) {
    let (Some(fields), Some(target)) = (delta.as_object(), step.as_object_mut()) else {
        return;
    };
    for (key, value) in fields {
        if key == "type" {
            continue;
        }
        match (target.get_mut(key), value.as_str()) {
            (Some(Value::String(existing)), Some(fragment)) => existing.push_str(fragment),
            _ => {
                target.insert(key.clone(), value.clone());
            }
        }
    }
}

impl StreamDecoder for Decoder {
    fn decode(
        &mut self,
        frame: &SseFrame,
        _provider: &std::sync::Arc<str>,
        out: &mut Vec<RawDelta>,
    ) -> Result<(), Error> {
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
                    Some("model_output") => {
                        self.steps.insert(slot, Step::ModelOutput);
                    }
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
                    _ => {
                        if let Some(Step::Opaque(step)) = self.steps.get_mut(&slot) {
                            merge_delta(step, delta);
                        }
                    }
                }
            }
            "step.stop" => match self.steps.remove(&slot) {
                Some(Step::Tool) => out.push(RawDelta::ToolEnd { slot }),
                Some(Step::ModelOutput) => out.push(RawDelta::TextEnd),
                Some(Step::Thought {
                    mut step,
                    signature,
                }) => {
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
                let usage = interaction.get("usage").map(|usage| Usage {
                    input_tokens: usage["total_input_tokens"].as_u64().unwrap_or(0),
                    output_tokens: usage["total_output_tokens"].as_u64().unwrap_or(0),
                    total_tokens: usage["total_tokens"].as_u64().unwrap_or(0),
                });
                out.push(RawDelta::Meta {
                    id: interaction["id"].as_str().map(str::to_string),
                    model: interaction["model"].as_str().map(str::to_string),
                    status: Some(match interaction["status"].as_str() {
                        Some("completed") => ResponseStatus::Completed,
                        Some("incomplete" | "budget_exceeded") => ResponseStatus::Incomplete,
                        Some("requires_action") => ResponseStatus::RequiresAction,
                        Some("failed" | "cancelled") => ResponseStatus::Failed,
                        Some(other) => ResponseStatus::Other(other.to_string()),
                        None => ResponseStatus::Completed,
                    }),
                    usage,
                    provider_metadata: Some(interaction.clone()),
                });
            }
            _ => {}
        }
        Ok(())
    }

    /// `convert_step` in response.rs maps the parsed `arguments` object through
    /// `Value::to_string`, which sorts its keys.
    fn normalizes_tool_arguments(&self) -> bool {
        true
    }
}
