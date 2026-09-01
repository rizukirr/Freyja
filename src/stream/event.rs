use super::assembler::{Assembler, StreamDecoder};
use super::sse::SseBuffer;
use crate::error::Error;
use crate::model::{GenerateResponse, ResponseStatus, Usage};
use serde_json::Value;
use std::sync::Arc;

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

/// The most a whole streaming body may carry before the stream is abandoned.
///
/// The same ceiling `crate::transport::MAX_BODY_BYTES` puts on a non-streaming
/// body, and deliberately the same number: one request sent two ways is bounded
/// two ways otherwise, which is an omission rather than a design.
///
/// `MAX_FRAME_BYTES` bounds one frame, which catches an endpoint that never
/// emits a separator. It does not catch an endpoint that emits well-formed
/// frames forever, and a gateway stuck in a retry loop does exactly that
/// without anyone intending harm. The read timeout bounds silence, not volume.
///
/// Counted on the bytes arriving rather than on what the assembler retains,
/// because nothing downstream can hold what never arrived, and one rule in one
/// place is easier to keep true than three.
pub(super) const MAX_STREAM_BYTES: usize = crate::transport::MAX_BODY_BYTES;

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
/// #     -> Result<(), freyja::Error> {
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
    /// The endpoint's configured name, handed to the decoder so a streaming
    /// error names the gateway rather than the dialect.
    endpoint: Arc<str>,
    body: Body,
    buffer: SseBuffer,
    decoder: Box<dyn StreamDecoder>,
    assembler: Assembler,
    queued: std::collections::VecDeque<StreamEvent>,
    closed: bool,
    /// Body bytes taken from the socket so far, across every frame.
    read: usize,
}

impl EventStream {
    pub(crate) fn new(
        endpoint: Arc<str>,
        decoder: Box<dyn StreamDecoder>,
        response: reqwest::Response,
    ) -> Self {
        let normalize_arguments = decoder.normalizes_tool_arguments();
        Self {
            endpoint: endpoint.clone(),
            body: Body::Live(response),
            buffer: SseBuffer::default(),
            decoder,
            assembler: Assembler::new(endpoint, normalize_arguments),
            queued: std::collections::VecDeque::new(),
            closed: false,
            read: 0,
        }
    }

    /// The next event, or `None` once the provider has closed the stream.
    ///
    /// Frames carrying nothing a caller can act on, keepalives, comments,
    /// sentinels, are consumed without producing an event.
    pub async fn next(&mut self) -> Result<Option<StreamEvent>, Error> {
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
    /// Errors with [`Error::Stream`] if [`EventStream::next`] has not
    /// yet returned `None`. A response that looks complete but is not, replayed
    /// to a provider, fails in ways that are hard to trace back to here.
    ///
    /// `provider_metadata` carries the provider's own terminal object. It is not
    /// byte-identical to the non-streaming path's value: `generate()` collects
    /// the fields Freyja does not model, while a stream carries the object
    /// whole. Every field a tool loop depends on, id, model, status, content,
    /// usage, does match, and `to_message()` produces the same assistant turn.
    pub fn into_response(self) -> Result<GenerateResponse, Error> {
        self.assembler.into_response()
    }

    /// Decodes one buffered frame, if a complete one is available.
    fn pump_frame(&mut self) -> Result<bool, Error> {
        let Some(frame) = self.buffer.next_frame() else {
            return Ok(false);
        };
        let mut deltas = Vec::new();
        // Cloned first: passing `&self.endpoint` would borrow `self` while
        // `self.decoder` is borrowed mutably.
        let endpoint = self.endpoint.clone();
        self.decoder.decode(&frame, &endpoint, &mut deltas)?;

        let mut events = Vec::new();
        for delta in deltas {
            self.assembler.absorb(delta, &mut events);
        }
        self.queued.extend(events);
        Ok(true)
    }

    /// Pulls more bytes. Returns `false` when the body is exhausted.
    async fn pump_bytes(&mut self) -> Result<bool, Error> {
        match &mut self.body {
            Body::Live(response) => {
                let chunk = response
                    .chunk()
                    .await
                    .map_err(|error| Error::transport(self.endpoint.clone(), &error, None))?;
                match chunk {
                    Some(bytes) => {
                        self.take(&bytes)?;
                        Ok(true)
                    }
                    None => Ok(false),
                }
            }
            #[cfg(test)]
            Body::Recorded(chunks) => match chunks.pop_front() {
                Some(bytes) => {
                    self.take(&bytes)?;
                    Ok(true)
                }
                None => Ok(false),
            },
        }
    }

    /// Buffers a chunk and refuses a body that has outgrown either ceiling.
    fn take(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.read += bytes.len();
        if self.read > MAX_STREAM_BYTES {
            return Err(Error::Stream {
                endpoint: self.endpoint.clone(),
                message: format!("the stream grew past {MAX_STREAM_BYTES} bytes"),
            });
        }
        self.buffer.push(bytes);
        self.check_buffer()
    }

    /// Fails the stream when one frame has grown past any plausible size.
    ///
    /// An endpoint that never emits a frame separator would otherwise be
    /// buffered whole, and the read timeout does not catch it: it bounds
    /// silence, and this endpoint is not silent.
    fn check_buffer(&self) -> Result<(), Error> {
        if self.buffer.overflowed() {
            return Err(Error::Stream {
                endpoint: self.endpoint.clone(),
                message: format!(
                    "a single event grew past {} bytes without ending",
                    super::sse::MAX_FRAME_BYTES
                ),
            });
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn for_test(
        endpoint: Arc<str>,
        decoder: Box<dyn StreamDecoder>,
        chunks: Vec<Vec<u8>>,
    ) -> Self {
        let normalize_arguments = decoder.normalizes_tool_arguments();
        Self {
            endpoint: endpoint.clone(),
            body: Body::Recorded(chunks.into()),
            buffer: SseBuffer::default(),
            decoder,
            assembler: Assembler::new(endpoint, normalize_arguments),
            queued: std::collections::VecDeque::new(),
            closed: false,
            read: 0,
        }
    }

    /// Drives [`Self::next`] to completion without a runtime.
    ///
    /// The recorded body never yields `Pending`, so a no-op waker is enough and
    /// the test suite needs no async runtime of its own.
    #[cfg(test)]
    pub(super) fn next_blocking(&mut self) -> Result<Option<StreamEvent>, Error> {
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
