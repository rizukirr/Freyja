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
    /// The model declined to answer. Kept distinct from text because the
    /// non-streaming path does, and because a caller may want to render or
    /// log it differently.
    RefusalDelta(String),
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
    /// The model declined to answer.
    Refusal(String),
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
        provider_metadata: Option<Value>,
    },
}

/// One dialect's translation from SSE frame to [`RawDelta`]s.
///
/// Implementations may hold state — several dialects announce a part's type in
/// one frame and its content in later ones.
pub(crate) trait StreamDecoder: Send {
    /// Appends everything this frame means to `out`.
    fn decode(&mut self, frame: &SseFrame, out: &mut Vec<RawDelta>) -> Result<(), ProviderError>;
}

use crate::provider::{GenerateResponse, OutputContent};
use std::collections::HashMap;
use std::sync::Arc;

/// A tool call being assembled from fragments.
struct PendingCall {
    id: String,
    name: String,
    arguments: String,
}

/// Turns one dialect's [`RawDelta`]s into neutral [`StreamEvent`]s.
///
/// Owns the only mutable state streaming needs: partial tool arguments, and the
/// completed parts that [`EventStream::into_response`] hands back.
struct Assembler {
    provider: Arc<str>,
    pending: HashMap<usize, PendingCall>,
    captured: Vec<OutputContent>,
    id: String,
    model: String,
    status: ResponseStatus,
    usage: Option<Usage>,
    provider_metadata: Option<Value>,
    finished: bool,
}

impl Assembler {
    fn new(provider: Arc<str>) -> Self {
        Self {
            provider,
            pending: HashMap::new(),
            captured: Vec::new(),
            id: String::new(),
            model: String::new(),
            // Overwritten by the terminal frame. A stream that ends without one
            // was cut short, and this is the answer the caller should see.
            status: ResponseStatus::Incomplete,
            usage: None,
            provider_metadata: None,
            finished: false,
        }
    }

    /// Applies one delta, pushing any resulting events onto `out`.
    fn absorb(&mut self, delta: RawDelta, out: &mut Vec<StreamEvent>) {
        match delta {
            RawDelta::Text(text) => {
                // Consecutive deltas coalesce into one content part, so
                // `captured` matches the shape `generate()` produces.
                match self.captured.last_mut() {
                    Some(OutputContent::Text(existing)) => existing.push_str(&text),
                    _ => self.captured.push(OutputContent::Text(text.clone())),
                }
                out.push(StreamEvent::TextDelta(text));
            }
            RawDelta::Refusal(text) => {
                // Refusals never coalesce into a neighbouring Text part: the
                // parser keeps them as their own OutputContent::Refusal.
                match self.captured.last_mut() {
                    Some(OutputContent::Refusal(existing)) => existing.push_str(&text),
                    _ => self.captured.push(OutputContent::Refusal(text.clone())),
                }
                out.push(StreamEvent::RefusalDelta(text));
            }
            RawDelta::ReasoningText(text) => out.push(StreamEvent::ReasoningDelta(text)),
            RawDelta::ReasoningBlob(data) => {
                self.captured
                    .push(OutputContent::Reasoning { data: data.clone() });
                out.push(StreamEvent::Reasoning { data });
            }
            RawDelta::Meta {
                id,
                model,
                status,
                usage,
                provider_metadata,
            } => {
                if let Some(id) = id {
                    self.id = id;
                }
                if let Some(model) = model {
                    self.model = model;
                }
                if let Some(status) = status {
                    self.status = status;
                }
                if usage.is_some() {
                    self.usage = usage;
                }
                if provider_metadata.is_some() {
                    self.provider_metadata = provider_metadata;
                }
            }
            RawDelta::ToolStart { slot, id, name } => {
                self.pending.insert(
                    slot,
                    PendingCall {
                        id,
                        name,
                        arguments: String::new(),
                    },
                );
            }
            RawDelta::ToolArgs { slot, fragment } => {
                if let Some(call) = self.pending.get_mut(&slot) {
                    call.arguments.push_str(&fragment);
                }
            }
            RawDelta::ToolReplace { slot, arguments } => {
                if let Some(call) = self.pending.get_mut(&slot) {
                    call.arguments = arguments;
                }
            }
            RawDelta::ToolEnd { slot } => self.finish_call(slot, out),
        }
    }

    /// Emits a completed tool call, if `slot` has one pending.
    fn finish_call(&mut self, slot: usize, out: &mut Vec<StreamEvent>) {
        let Some(call) = self.pending.remove(&slot) else {
            return;
        };
        self.captured.push(OutputContent::ToolCall {
            id: call.id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
        });
        out.push(StreamEvent::ToolCall {
            id: call.id,
            name: call.name,
            arguments: call.arguments,
        });
    }

    /// Called when the body closes: flushes calls the dialect never ended, then
    /// emits the terminal event.
    ///
    /// OpenAiChat has no end frame at all, so without this its tool calls would
    /// be silently dropped.
    fn close(&mut self, out: &mut Vec<StreamEvent>) {
        let mut slots: Vec<usize> = self.pending.keys().copied().collect();
        slots.sort_unstable();
        for slot in slots {
            self.finish_call(slot, out);
        }

        self.finished = true;
        out.push(StreamEvent::Done {
            id: self.id.clone(),
            model: self.model.clone(),
            status: self.status.clone(),
            usage: self.usage,
        });
    }

    /// The whole response, once the stream has closed.
    fn into_response(self) -> Result<GenerateResponse, ProviderError> {
        if !self.finished {
            return Err(ProviderError::Stream {
                provider: self.provider,
                message: "into_response called before the stream was drained".into(),
            });
        }
        Ok(GenerateResponse {
            id: self.id,
            model: self.model,
            status: self.status,
            content: self.captured,
            usage: self.usage,
            provider_metadata: self.provider_metadata,
        })
    }
}

/// Where an [`EventStream`] gets its bytes.
///
/// The test variant exists because `reqwest::Response` cannot be constructed
/// from recorded bytes, and streaming is far too stateful to leave untested.
enum Body {
    Live(reqwest::Response),
    #[cfg(test)]
    Recorded(std::collections::VecDeque<Vec<u8>>),
}

/// A live stream of [`StreamEvent`]s.
///
/// Drive it with [`EventStream::next`] until it returns `None`, then call
/// [`EventStream::into_response`] if you need the whole response.
///
/// ```no_run
/// # async fn run(client: freyja::Client, request: freyja::GenerateRequest)
/// #     -> Result<(), freyja::ProviderError> {
/// use freyja::StreamEvent;
///
/// let mut stream = client.stream(&request).await?;
/// while let Some(event) = stream.next().await? {
///     if let StreamEvent::TextDelta(text) = event {
///         print!("{text}");
///     }
/// }
/// # Ok(())
/// # }
/// ```
pub struct EventStream {
    body: Body,
    buffer: crate::provider::sse::SseBuffer,
    decoder: Box<dyn StreamDecoder>,
    assembler: Assembler,
    queued: std::collections::VecDeque<StreamEvent>,
    closed: bool,
}

impl EventStream {
    pub(crate) fn new(
        provider: Arc<str>,
        decoder: Box<dyn StreamDecoder>,
        response: reqwest::Response,
    ) -> Self {
        Self {
            body: Body::Live(response),
            buffer: crate::provider::sse::SseBuffer::default(),
            decoder,
            assembler: Assembler::new(provider),
            queued: std::collections::VecDeque::new(),
            closed: false,
        }
    }

    /// The next event, or `None` once the provider has closed the stream.
    ///
    /// Frames carrying nothing a caller can act on — keepalives, comments,
    /// sentinels — are consumed without producing an event.
    pub async fn next(&mut self) -> Result<Option<StreamEvent>, ProviderError> {
        loop {
            if let Some(event) = self.queued.pop_front() {
                return Ok(Some(event));
            }
            if self.closed {
                return Ok(None);
            }
            if !self.pump_frame()? && !self.pump_bytes().await? {
                // The body ended: flush pending calls and emit Done.
                self.closed = true;
                let mut events = Vec::new();
                self.assembler.close(&mut events);
                self.queued.extend(events);
            }
        }
    }

    /// The whole response, identical to what [`crate::Client::generate`] would
    /// have returned.
    ///
    /// Errors with [`ProviderError::Stream`] if [`EventStream::next`] has not
    /// yet returned `None`. A response that looks complete but is not, replayed
    /// to a provider, fails in ways that are hard to trace back to here.
    ///
    /// `provider_metadata` carries the provider's own terminal object. It is not
    /// byte-identical to the non-streaming path's value: `generate()` collects
    /// the fields Freyja does not model, while a stream carries the object
    /// whole. Every field a tool loop depends on — id, model, status, content,
    /// usage — does match, and `to_message()` produces the same assistant turn.
    pub fn into_response(self) -> Result<GenerateResponse, ProviderError> {
        self.assembler.into_response()
    }

    /// Decodes one buffered frame, if a complete one is available.
    fn pump_frame(&mut self) -> Result<bool, ProviderError> {
        let Some(frame) = self.buffer.next_frame() else {
            return Ok(false);
        };
        let mut deltas = Vec::new();
        self.decoder.decode(&frame, &mut deltas)?;

        let mut events = Vec::new();
        for delta in deltas {
            self.assembler.absorb(delta, &mut events);
        }
        self.queued.extend(events);
        Ok(true)
    }

    /// Pulls more bytes. Returns `false` when the body is exhausted.
    async fn pump_bytes(&mut self) -> Result<bool, ProviderError> {
        match &mut self.body {
            Body::Live(response) => {
                let chunk = response
                    .chunk()
                    .await
                    .map_err(|error| ProviderError::Http(error.to_string()))?;
                match chunk {
                    Some(bytes) => {
                        self.buffer.push(&bytes);
                        Ok(true)
                    }
                    None => Ok(false),
                }
            }
            #[cfg(test)]
            Body::Recorded(chunks) => match chunks.pop_front() {
                Some(bytes) => {
                    self.buffer.push(&bytes);
                    Ok(true)
                }
                None => Ok(false),
            },
        }
    }

    #[cfg(test)]
    fn for_test(provider: Arc<str>, decoder: Box<dyn StreamDecoder>, chunks: Vec<Vec<u8>>) -> Self {
        Self {
            body: Body::Recorded(chunks.into()),
            buffer: crate::provider::sse::SseBuffer::default(),
            decoder,
            assembler: Assembler::new(provider),
            queued: std::collections::VecDeque::new(),
            closed: false,
        }
    }

    /// Drives [`Self::next`] to completion without a runtime.
    ///
    /// The recorded body never yields `Pending`, so a no-op waker is enough and
    /// the test suite needs no async runtime of its own.
    #[cfg(test)]
    fn next_blocking(&mut self) -> Result<Option<StreamEvent>, ProviderError> {
        use std::future::Future;
        use std::pin::pin;
        use std::task::{Context, Poll, Waker};

        let mut future = pin!(self.next());
        let mut context = Context::from_waker(Waker::noop());
        match future.as_mut().poll(&mut context) {
            Poll::Ready(result) => result,
            Poll::Pending => panic!("a recorded body never pends"),
        }
    }
}

/// Drives a decoder over recorded frames and returns the assembled response,
/// so a dialect's tests can compare streaming against its own parser.
#[cfg(test)]
pub(crate) fn drain_for_test(
    provider: Arc<str>,
    decoder: Box<dyn StreamDecoder>,
    chunks: Vec<Vec<u8>>,
) -> Result<GenerateResponse, ProviderError> {
    let mut stream = EventStream::for_test(provider, decoder, chunks);
    while stream.next_blocking()?.is_some() {}
    stream.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::OutputContent;

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

    #[test]
    fn assembler_coalesces_text() {
        let mut assembler = Assembler::new("acme".into());
        let mut out = Vec::new();

        assembler.absorb(RawDelta::Text("a".into()), &mut out);
        assembler.absorb(RawDelta::Text("b".into()), &mut out);
        assembler.absorb(
            RawDelta::Meta {
                id: Some("resp_1".into()),
                model: Some("test-model".into()),
                status: Some(ResponseStatus::Completed),
                usage: None,
                provider_metadata: None,
            },
            &mut out,
        );

        assert_eq!(
            out,
            vec![
                StreamEvent::TextDelta("a".into()),
                StreamEvent::TextDelta("b".into()),
            ],
            "metadata produces no event of its own"
        );

        // Deltas are separate events but one content part, matching generate().
        assert_eq!(assembler.captured, vec![OutputContent::Text("ab".into())]);
        assert_eq!(assembler.id, "resp_1");
        assert_eq!(assembler.model, "test-model");
    }

    #[test]
    fn assembler_assembles_fragmented_arguments() {
        let mut assembler = Assembler::new("acme".into());
        let mut out = Vec::new();

        assembler.absorb(
            RawDelta::ToolStart {
                slot: 0,
                id: "call_1".into(),
                name: "get_weather".into(),
            },
            &mut out,
        );
        assembler.absorb(
            RawDelta::ToolArgs {
                slot: 0,
                fragment: "{\"loc".into(),
            },
            &mut out,
        );
        assembler.absorb(
            RawDelta::ToolArgs {
                slot: 0,
                fragment: "ation\":\"NYC\"}".into(),
            },
            &mut out,
        );

        assert!(out.is_empty(), "nothing is emitted until the call ends");

        assembler.absorb(RawDelta::ToolEnd { slot: 0 }, &mut out);

        assert_eq!(
            out,
            vec![StreamEvent::ToolCall {
                id: "call_1".into(),
                name: "get_weather".into(),
                arguments: "{\"location\":\"NYC\"}".into(),
            }]
        );
    }

    #[test]
    fn assembler_keeps_concurrent_calls_apart() {
        let mut assembler = Assembler::new("acme".into());
        let mut out = Vec::new();

        for (slot, id, name) in [(0, "call_a", "alpha"), (1, "call_b", "beta")] {
            assembler.absorb(
                RawDelta::ToolStart {
                    slot,
                    id: id.into(),
                    name: name.into(),
                },
                &mut out,
            );
        }
        // Interleaved fragments must not cross-contaminate.
        assembler.absorb(
            RawDelta::ToolArgs {
                slot: 0,
                fragment: "{\"a\":".into(),
            },
            &mut out,
        );
        assembler.absorb(
            RawDelta::ToolArgs {
                slot: 1,
                fragment: "{\"b\":".into(),
            },
            &mut out,
        );
        assembler.absorb(
            RawDelta::ToolArgs {
                slot: 0,
                fragment: "1}".into(),
            },
            &mut out,
        );
        assembler.absorb(
            RawDelta::ToolArgs {
                slot: 1,
                fragment: "2}".into(),
            },
            &mut out,
        );
        assembler.absorb(RawDelta::ToolEnd { slot: 1 }, &mut out);
        assembler.absorb(RawDelta::ToolEnd { slot: 0 }, &mut out);

        assert_eq!(
            out,
            vec![
                StreamEvent::ToolCall {
                    id: "call_b".into(),
                    name: "beta".into(),
                    arguments: "{\"b\":2}".into(),
                },
                StreamEvent::ToolCall {
                    id: "call_a".into(),
                    name: "alpha".into(),
                    arguments: "{\"a\":1}".into(),
                },
            ]
        );
    }

    #[test]
    fn assembler_flushes_unended_calls() {
        let mut assembler = Assembler::new("acme".into());
        let mut out = Vec::new();

        assembler.absorb(
            RawDelta::ToolStart {
                slot: 0,
                id: "call_1".into(),
                name: "add".into(),
            },
            &mut out,
        );
        assembler.absorb(
            RawDelta::ToolArgs {
                slot: 0,
                fragment: "{}".into(),
            },
            &mut out,
        );

        // OpenAiChat never sends an end frame; the body simply closes.
        assembler.close(&mut out);

        assert_eq!(
            out,
            vec![
                StreamEvent::ToolCall {
                    id: "call_1".into(),
                    name: "add".into(),
                    arguments: "{}".into(),
                },
                StreamEvent::Done {
                    id: String::new(),
                    model: String::new(),
                    status: ResponseStatus::Incomplete,
                    usage: None,
                },
            ],
            "the call must be flushed before Done, and a stream with no \
             terminal frame reports Incomplete"
        );
    }

    /// A decoder over a trivial `data: <text>` protocol, standing in for a real
    /// dialect so the stream machinery can be tested without a network.
    #[derive(Default)]
    struct TestDecoder;

    impl StreamDecoder for TestDecoder {
        fn decode(
            &mut self,
            frame: &SseFrame,
            out: &mut Vec<RawDelta>,
        ) -> Result<(), ProviderError> {
            if frame.data == "[DONE]" {
                out.push(RawDelta::Meta {
                    id: Some("resp_1".into()),
                    model: Some("test-model".into()),
                    status: Some(ResponseStatus::Completed),
                    usage: None,
                    provider_metadata: None,
                });
            } else {
                out.push(RawDelta::Text(frame.data.clone()));
            }
            Ok(())
        }
    }

    #[test]
    fn event_stream_drains_a_recorded_body() {
        let mut stream = EventStream::for_test(
            "acme".into(),
            Box::new(TestDecoder),
            vec![b"data: h".to_vec(), b"i\n\ndata: [DONE]\n\n".to_vec()],
        );

        assert_eq!(
            stream.next_blocking().expect("event"),
            Some(StreamEvent::TextDelta("hi".into()))
        );
        assert_eq!(
            stream.next_blocking().expect("event"),
            Some(StreamEvent::Done {
                id: "resp_1".into(),
                model: "test-model".into(),
                status: ResponseStatus::Completed,
                usage: None,
            })
        );
        assert_eq!(stream.next_blocking().expect("end"), None);

        let response = stream.into_response().expect("drained");
        assert_eq!(response.output_text(), "hi");
        assert_eq!(response.model, "test-model");
    }

    #[test]
    fn assembler_into_response_requires_a_drained_stream() {
        let mut assembler = Assembler::new("acme".into());
        let mut out = Vec::new();
        assembler.absorb(RawDelta::Text("hi".into()), &mut out);

        assert!(matches!(
            Assembler::new("acme".into()).into_response(),
            Err(ProviderError::Stream { .. })
        ));

        assembler.absorb(
            RawDelta::Meta {
                id: Some("resp_1".into()),
                model: Some("test-model".into()),
                status: Some(ResponseStatus::Completed),
                usage: Some(Usage {
                    input_tokens: 1,
                    output_tokens: 2,
                    total_tokens: 3,
                }),
                provider_metadata: None,
            },
            &mut out,
        );
        assembler.close(&mut out);

        let response = assembler.into_response().expect("drained");
        assert_eq!(response.id, "resp_1");
        assert_eq!(response.model, "test-model");
        assert_eq!(response.status, ResponseStatus::Completed);
        assert_eq!(response.output_text(), "hi");
        assert_eq!(response.usage.expect("usage").total_tokens, 3);
    }
}
