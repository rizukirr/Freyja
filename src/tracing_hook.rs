//! A ready-made hook that reports every call as a `tracing` event.

use crate::{Context, Flow, GenerateResponse, Hook};
use std::time::Duration;

/// A [`Hook`] that emits one `tracing` event per model call and per tool
/// call, at `INFO` with target `freyja`.
///
/// A model call reports `model`, `status`, `elapsed_ms`, and `input_tokens`,
/// `output_tokens` and `total_tokens` when the response carries usage. A tool
/// call reports `tool`, `elapsed_ms` and `output_bytes`, including a call the
/// guard refused. No message text, tool arguments or tool output is logged
/// unless [`Tracing::with_content`] is set, because they can hold user data
/// and secrets. It never stops a run.
///
/// Freyja installs no subscriber: events go wherever the application's
/// `tracing` subscriber sends them. Requires the `tracing` feature.
///
/// ```
/// # fn run(client: freyja::Client) {
/// let agent = freyja::Agent::new(client).hook(freyja::Tracing::new());
/// # }
/// ```
///
/// The source is a short example of a complete hook.
#[derive(Debug, Default, Clone, Copy)]
pub struct Tracing {
    content: bool,
}

impl Tracing {
    /// Reports calls without their content.
    pub fn new() -> Self {
        Self::default()
    }

    /// Also logs the model's text as `text`, and each tool call's
    /// `arguments` and `output`.
    pub fn with_content(mut self) -> Self {
        self.content = true;
        self
    }
}

impl Hook for Tracing {
    fn after_model(&self, response: &GenerateResponse, elapsed: Duration, _cx: &Context) -> Flow {
        let usage = response.usage.as_ref();
        let input_tokens = usage.map(|usage| usage.input_tokens);
        let output_tokens = usage.map(|usage| usage.output_tokens);
        let total_tokens = usage.map(|usage| usage.total_tokens);
        let elapsed_ms = elapsed.as_millis() as u64;
        if self.content {
            tracing::info!(
                target: "freyja",
                model = %response.model,
                status = ?response.status,
                elapsed_ms,
                input_tokens,
                output_tokens,
                total_tokens,
                text = %response.output_text(),
                "model call"
            );
        } else {
            tracing::info!(
                target: "freyja",
                model = %response.model,
                status = ?response.status,
                elapsed_ms,
                input_tokens,
                output_tokens,
                total_tokens,
                "model call"
            );
        }
        Flow::Continue
    }

    fn after_tool(
        &self,
        name: &str,
        arguments: &str,
        output: &str,
        elapsed: Duration,
        _cx: &Context,
    ) -> Flow {
        let elapsed_ms = elapsed.as_millis() as u64;
        if self.content {
            tracing::info!(
                target: "freyja",
                tool = name,
                elapsed_ms,
                output_bytes = output.len(),
                arguments,
                output,
                "tool call"
            );
        } else {
            tracing::info!(
                target: "freyja",
                tool = name,
                elapsed_ms,
                output_bytes = output.len(),
                "tool call"
            );
        }
        Flow::Continue
    }
}
