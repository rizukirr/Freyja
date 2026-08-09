//! Streaming: neutral events, the shared assembler, and the public stream type.

use crate::provider::sse::SseFrame;
use crate::provider::{ProviderError, ResponseStatus, Usage};
use serde_json::Value;

/// One thing the model produced, as it arrives.
///
/// Fragments are not exposed. Tool-call arguments and reasoning blobs are
/// buffered internally and surface only once complete, so a caller never
/// reassembles partial JSON.
///
/// The enum is `#[non_exhaustive]`: match with a trailing `_ => {}` arm.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// A fragment of generated text, in order.
    TextDelta(String),
    /// A complete tool call. Arguments are fully assembled; dispatch it now.
    ToolCall {
        /// Correlation id to quote back in [`crate::Message::tool_result`].
        id: String,
        /// Name of the tool to run.
        name: String,
        /// Arguments, as a raw JSON string.
        arguments: String,
    },
    /// Human-readable reasoning text, when the provider exposes it.
    ReasoningDelta(String),
    /// Opaque provider reasoning state, complete and replayable verbatim.
    ///
    /// See [`crate::OutputContent::Reasoning`] for why this must be preserved.
    Reasoning {
        /// The provider's own representation, as received.
        data: Value,
    },
    /// Terminal event, emitted once before the stream ends.
    Done {
        /// Provider-assigned response id.
        id: String,
        /// The model that served the request.
        model: String,
        /// Why the response ended.
        status: ResponseStatus,
        /// Token accounting, when the provider reports it.
        usage: Option<Usage>,
    },
}

/// What one dialect's frame meant, before neutralization.
///
/// `slot` is whatever integer the dialect uses to correlate parts: Anthropic's
/// content-block index, OpenAiChat's tool-call index, Responses' output index,
/// Gemini's step index. The meanings differ, which is exactly why this type is
/// private — the assembler only needs the numbers to be consistent within one
/// stream, not to mean the same thing across dialects.
#[derive(Debug, PartialEq)]
pub(crate) enum RawDelta {
    /// Generated text.
    Text(String),
    /// A tool call has begun.
    ToolStart {
        slot: usize,
        id: String,
        name: String,
    },
    /// More argument text for a tool call.
    ToolArgs { slot: usize, fragment: String },
    /// Authoritative complete arguments, replacing anything buffered.
    ToolReplace { slot: usize, arguments: String },
    /// A tool call is complete.
    ToolEnd { slot: usize },
    /// Human-readable reasoning.
    ReasoningText(String),
    /// A complete opaque reasoning blob.
    ReasoningBlob(Value),
    /// Response-level metadata. Any field may arrive in any frame.
    Meta {
        id: Option<String>,
        model: Option<String>,
        status: Option<ResponseStatus>,
        usage: Option<Usage>,
    },
}

/// One dialect's translation from SSE frame to [`RawDelta`]s.
///
/// Implementations may hold state — several dialects announce a part's type in
/// one frame and its content in later ones.
pub(crate) trait StreamDecoder: Send {
    /// Appends everything this frame means to `out`.
    fn decode(&mut self, frame: &SseFrame, out: &mut Vec<RawDelta>)
    -> Result<(), ProviderError>;
}

/// A live stream of [`StreamEvent`]s.
///
/// Replaced with the real implementation in the following task; this shell
/// exists so the crate compiles between commits.
pub struct EventStream {
    pub(crate) _private: (),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_is_non_exhaustive_and_comparable() {
        let delta = StreamEvent::TextDelta("hi".into());
        assert_eq!(delta, StreamEvent::TextDelta("hi".into()));
        assert_ne!(delta, StreamEvent::TextDelta("bye".into()));

        let call = StreamEvent::ToolCall {
            id: "call_1".into(),
            name: "add".into(),
            arguments: "{\"a\":1}".into(),
        };
        assert_ne!(call, delta);
    }
}
