//! Code that runs around every model call and tool call an agent makes.

use crate::{Context, GenerateRequest, GenerateResponse};
use std::time::Duration;

/// What a [`Hook`] tells the loop to do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Flow {
    /// Carry on.
    Continue,
    /// End the run here. It returns `Ok` with
    /// [`crate::StopReason::Stopped`] carrying this reason.
    ///
    /// From `before_model`, no request is made, so the user's turn is
    /// stored with no reply. From `after_model`, the response is kept, and
    /// any tool calls it asked for are left unanswered, which the next send
    /// repairs. From either tool method, the turn's other calls still finish,
    /// so every call has a result.
    Stop(String),
}

/// Runs your code around every model call and tool call an agent makes,
/// in every conversation, for logging, tracing, cost accounting, auditing,
/// or ending a run that has spent enough.
///
/// Set with [`crate::Agent::hook`]. Every method has a default that returns
/// [`Flow::Continue`], so a hook implements only what it needs. Methods are
/// synchronous because they run inline in the loop: a hook that must do I/O
/// hands the work to a channel or a task.
pub trait Hook: Send + Sync {
    /// Before each request to the model.
    fn before_model(&self, request: &GenerateRequest, cx: &Context) -> Flow {
        let _ = (request, cx);
        Flow::Continue
    }

    /// After each response, with how long the call took.
    fn after_model(&self, response: &GenerateResponse, elapsed: Duration, cx: &Context) -> Flow {
        let _ = (response, elapsed, cx);
        Flow::Continue
    }

    /// Before each tool call the guard allowed.
    fn before_tool(&self, name: &str, arguments: &str, cx: &Context) -> Flow {
        let _ = (name, arguments, cx);
        Flow::Continue
    }

    /// After each tool call, with the text the model will read and how long
    /// the call took. Also called for a call the guard refused, with output
    /// `denied: <reason>` and a zero duration, so an audit sees every request.
    fn after_tool(
        &self,
        name: &str,
        arguments: &str,
        output: &str,
        elapsed: Duration,
        cx: &Context,
    ) -> Flow {
        let _ = (name, arguments, output, elapsed, cx);
        Flow::Continue
    }
}

/// Calls `each` on every hook in order and returns the first stop reason.
/// Every hook is called, so one that only records still sees the point a
/// run stopped at.
pub(crate) fn first_stop<'a>(
    hooks: impl IntoIterator<Item = &'a std::sync::Arc<dyn Hook>>,
    mut each: impl FnMut(&dyn Hook) -> Flow,
) -> Option<String> {
    let mut stop = None;
    for hook in hooks {
        if let Flow::Stop(reason) = each(hook.as_ref())
            && stop.is_none()
        {
            stop = Some(reason);
        }
    }
    stop
}
