#[cfg(test)]
use super::event::EventStream;
use super::event::StreamEvent;
use super::sse::SseFrame;
use crate::error::Error;
use crate::model::{GenerateResponse, OutputContent, ResponseStatus, Usage};
use serde_json::Value;

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
    /// The end of one text block. Text continues to coalesce within a block;
    /// this starts a new `OutputContent::Text`, matching one part per block.
    TextEnd,
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
    ///
    /// `endpoint` is the endpoint's configured name, not the dialect. Any
    /// [`Error`] this raises must carry it verbatim: a Claude-compatible
    /// gateway has to report itself and not "anthropic", which is the invariant
    /// documented on [`Error`] and which the non-streaming path honours through
    /// [`EndpointConfig::name`](crate::EndpointConfig::name).
    fn decode(
        &mut self,
        frame: &SseFrame,
        endpoint: &Arc<str>,
        out: &mut Vec<RawDelta>,
    ) -> Result<(), Error>;

    /// Whether this dialect's parser re-serializes tool arguments from parsed
    /// JSON rather than passing the raw string through.
    ///
    /// Anthropic and Gemini call `Value::to_string` on the parsed object, which
    /// sorts keys and strips whitespace; the OpenAI dialects hand back exactly
    /// what the model emitted. The streaming path has to make the same choice
    /// per dialect or a drained stream stops matching `generate()`.
    fn normalizes_tool_arguments(&self) -> bool {
        false
    }
}

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
pub(super) struct Assembler {
    endpoint: Arc<str>,
    pending: HashMap<usize, PendingCall>,
    captured: Vec<OutputContent>,
    id: String,
    model: String,
    status: ResponseStatus,
    usage: Option<Usage>,
    provider_metadata: Option<Value>,
    finished: bool,
    /// Mirrors [`StreamDecoder::normalizes_tool_arguments`] for the dialect
    /// this stream belongs to.
    normalize_arguments: bool,
    /// Whether the trailing captured part is a text block still being filled.
    text_open: bool,
}

impl Assembler {
    pub(super) fn new(endpoint: Arc<str>, normalize_arguments: bool) -> Self {
        Self {
            endpoint,
            normalize_arguments,
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
            text_open: false,
        }
    }

    /// Applies one delta, pushing any resulting events onto `out`.
    pub(super) fn absorb(&mut self, delta: RawDelta, out: &mut Vec<StreamEvent>) {
        match delta {
            RawDelta::Text(text) => {
                // Consecutive deltas coalesce into one content part, so
                // `captured` matches the shape `generate()` produces — but only
                // within a block, since the parsers emit one part per block.
                match self.captured.last_mut() {
                    Some(OutputContent::Text(existing)) if self.text_open => {
                        existing.push_str(&text)
                    }
                    _ => self.captured.push(OutputContent::Text(text.clone())),
                }
                self.text_open = true;
                out.push(StreamEvent::TextDelta(text));
            }
            RawDelta::TextEnd => self.text_open = false,
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
        let Some(mut call) = self.pending.remove(&slot) else {
            return;
        };
        // A call taking no arguments streams as an empty buffer, but every
        // dialect's parser normalizes that to an empty object. Match it here,
        // once, rather than in four decoders: `get_current_time()` must read
        // the same whether it arrived streamed or whole.
        if call.arguments.is_empty() {
            call.arguments.push_str("{}");
        }
        // Anthropic and Gemini parse tool input and re-serialize it, which
        // sorts keys. Round-trip the streamed fragments the same way so the two
        // paths agree; leave a body that is not valid JSON untouched rather
        // than discarding what the model sent.
        if self.normalize_arguments
            && let Ok(value) = serde_json::from_str::<Value>(&call.arguments)
        {
            call.arguments = value.to_string();
        }
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
    pub(super) fn close(&mut self, out: &mut Vec<StreamEvent>) {
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
    pub(super) fn into_response(self) -> Result<GenerateResponse, Error> {
        if !self.finished {
            return Err(Error::Stream {
                endpoint: self.endpoint,
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

/// Drives a decoder over recorded frames and returns the assembled response,
/// so a dialect's tests can compare streaming against its own parser.
#[cfg(test)]
pub(crate) fn drain_for_test(
    endpoint: Arc<str>,
    decoder: Box<dyn StreamDecoder>,
    chunks: Vec<Vec<u8>>,
) -> Result<GenerateResponse, Error> {
    let mut stream = EventStream::for_test(endpoint, decoder, chunks);
    while stream.next_blocking()?.is_some() {}
    stream.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::OutputContent;

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
        let mut assembler = Assembler::new("acme".into(), false);
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
    fn assembler_keeps_text_blocks_separate() {
        let mut assembler = Assembler::new("acme".into(), false);
        let mut out = Vec::new();

        // One block arriving in two deltas, then a second block.
        assembler.absorb(RawDelta::Text("A".into()), &mut out);
        assembler.absorb(RawDelta::Text("a".into()), &mut out);
        assembler.absorb(RawDelta::TextEnd, &mut out);
        assembler.absorb(RawDelta::Text("B".into()), &mut out);
        assembler.absorb(RawDelta::TextEnd, &mut out);

        assert_eq!(
            assembler.captured,
            vec![
                OutputContent::Text("Aa".into()),
                OutputContent::Text("B".into()),
            ],
            "deltas within a block coalesce, but a block boundary starts a new \
             part, because that is one OutputContent::Text per block as the \
             parsers produce"
        );
        assert_eq!(
            out,
            vec![
                StreamEvent::TextDelta("A".into()),
                StreamEvent::TextDelta("a".into()),
                StreamEvent::TextDelta("B".into()),
            ],
            "the boundary is internal bookkeeping and produces no event"
        );
    }

    #[test]
    fn assembler_assembles_fragmented_arguments() {
        let mut assembler = Assembler::new("acme".into(), false);
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
    fn assembler_normalizes_an_argumentless_call() {
        let mut assembler = Assembler::new("acme".into(), false);
        let mut out = Vec::new();

        assembler.absorb(
            RawDelta::ToolStart {
                slot: 0,
                id: "call_1".into(),
                name: "get_current_time".into(),
            },
            &mut out,
        );
        assembler.absorb(RawDelta::ToolEnd { slot: 0 }, &mut out);

        assert_eq!(
            out,
            vec![StreamEvent::ToolCall {
                id: "call_1".into(),
                name: "get_current_time".into(),
                arguments: "{}".into(),
            }],
            "every dialect's parser turns absent arguments into an empty \
             object, so a streamed call with no fragments must too, or the \
             same tool reads differently depending on how it arrived"
        );
    }

    #[test]
    fn assembler_keeps_concurrent_calls_apart() {
        let mut assembler = Assembler::new("acme".into(), false);
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
        let mut assembler = Assembler::new("acme".into(), false);
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
            _endpoint: &Arc<str>,
            out: &mut Vec<RawDelta>,
        ) -> Result<(), Error> {
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
        let mut assembler = Assembler::new("acme".into(), false);
        let mut out = Vec::new();
        assembler.absorb(RawDelta::Text("hi".into()), &mut out);

        assert!(matches!(
            Assembler::new("acme".into(), false).into_response(),
            Err(Error::Stream { .. })
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
