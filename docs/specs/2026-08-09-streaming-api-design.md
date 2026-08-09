---
title: streaming-api
date: 2026-08-09
status: draft
---

# Streaming API — Design

## Problem

Freyja can send a generation request and read the finished answer, but it cannot
observe an answer as it is produced. Every interactive use — a chat UI, a CLI
that prints tokens as they arrive, an agent that dispatches a tool the moment
the model asks for it — needs the response incrementally. `README.md:88` already
names streaming as the next item in the provider layer.

The difficulty is not transport. It is that the four dialects Freyja speaks emit
structurally different server-sent events, and diverge most sharply on the one
thing that matters most: tool calls arrive as fragmented JSON argument strings,
correlated by an integer whose meaning differs per dialect.

For a single call to `get_weather({"location":"NYC"})`:

- **OpenAiChat** sends `id` and `name` once, in the first frame; later frames
  carry only `choices[].delta.tool_calls[].index` and an `arguments` fragment.
  The index counts tool calls, starting at 0.
- **Anthropic** carries structure in event *names* — `content_block_start`
  announces the call, `content_block_delta` with `delta.type ==
  "input_json_delta"` carries fragments under `partial_json`, and
  `content_block_stop` ends it. Its `index` is the *content block* index, shared
  with text blocks, so a response that opens with prose puts the tool call at
  index 1.
- **OpenAiResponses** uses a third scheme with two correlation ids (`item_id`,
  `output_index`) and sends a terminal `response.function_call_arguments.done`
  frame repeating the *complete* arguments — a double-count hazard if the deltas
  are also forwarded.
- **Gemini** Interactions API: frame shape unverified. See Open questions.

Same field name, different meaning. Correlating these into one neutral event is
a decision, not a translation.

## Goals

- `client.stream(&request)` on the existing `Client`, alongside `generate()`.
- A provider-neutral `StreamEvent` enum covering text, tool calls, reasoning,
  and a terminal event with usage and finish reason.
- All four dialects: OpenAiChat, OpenAiResponses, Anthropic, Gemini.
- Zero new dependencies. Cargo.toml's dependency list is unchanged.
- Tool-call arguments arrive fully assembled. Callers never stitch JSON.
- Reasoning models remain usable across turns: the opaque replayable blob is
  reachable from the stream.
- A drained stream converts to the same `GenerateResponse` that `generate()`
  would have returned, so a streaming multi-turn tool loop can reuse the
  existing `GenerateResponse::to_message`.

## Non-goals

- **No `futures_core::Stream` impl.** No combinators, no `impl Stream` interop,
  no `axum::Sse` handoff. An inherent `async fn next()` only. Revisit behind an
  optional cargo feature if callers ask.
- **No opt-out of accumulation.** `EventStream` always buffers the completed
  response so `into_response()` can be called. A caller who only prints tokens
  still holds the full text in memory — the same footprint `generate()` has, but
  no longer avoidable. A non-capturing variant is deliberately not offered; add
  one only if the memory shows up as a real problem.
- **No fragment-level tool events.** Partial arguments are not observable. A UI
  cannot render tool arguments as they type.
- **No retries, backoff, or reconnection.** A dropped stream is an error, not
  something to resume. Retries are a separate roadmap item.
- **No streaming tool *results*.** Tool execution stays entirely with the caller.

## Constraints

- Rust edition 2024, MSRV 1.88, verified by CI.
- Dependencies are `reqwest` (features: `json`), `serde`, `serde_json`. Nothing
  else may be added. In particular `reqwest`'s `stream` feature is off-limits —
  it pulls `futures-util` and `tokio/fs` into every downstream build.
- The crate exposes `async fn` and spawns nothing; the caller picks the runtime.
  Streaming must not change that.
- `generate()`'s serialized request bodies must not change, byte for byte.
- Existing structure: transport lives in `Client`; each dialect owns only
  conversion. `Provider` at `src/provider/mod.rs:201` is stateless.

## Approach

Approach A of three considered: a shared SSE reader, a shared assembler, and a
thin per-dialect frame decoder. Three layers, each with one job.

### Public surface

```rust
impl Client {
    /// Opens a streaming generation. Returns once the provider has accepted the
    /// request; a non-2xx status is surfaced here, not mid-stream.
    pub async fn stream(&self, request: &GenerateRequest)
        -> Result<EventStream, ProviderError>;
}

pub struct EventStream { /* opaque */ }

impl EventStream {
    /// The next event, or `None` when the provider closes the stream.
    pub async fn next(&mut self) -> Result<Option<StreamEvent>, ProviderError>;

    /// The whole response, identical to what `generate()` would have returned.
    ///
    /// Errors with `ProviderError::Stream` if the stream has not been drained
    /// to `None`, because a truncated transcript replayed to a provider fails
    /// in confusing ways far from its cause.
    pub fn into_response(self) -> Result<GenerateResponse, ProviderError>;
}

#[non_exhaustive]
pub enum StreamEvent {
    /// A fragment of generated text, in order.
    TextDelta(String),
    /// A complete tool call. Arguments are fully assembled; dispatch it now.
    ToolCall { id: String, name: String, arguments: String },
    /// Human-readable reasoning text, when the provider exposes it.
    ReasoningDelta(String),
    /// Opaque provider reasoning state, complete and replayable verbatim.
    Reasoning { data: Value },
    /// Terminal event, emitted once before the stream ends.
    Done {
        id: String,
        model: String,
        status: ResponseStatus,
        usage: Option<Usage>,
    },
}
```

Caller-facing shape:

```rust
let client = Client::from_env(ProviderType::OpenAi).unwrap();
let mut stream = client.stream(&request).await?;

while let Some(event) = stream.next().await? {
    match event {
        StreamEvent::TextDelta(text) => print!("{text}"),
        StreamEvent::ToolCall { id, name, arguments } => { /* dispatch now */ }
        StreamEvent::Done { usage, .. } => { /* accounting */ }
        _ => {}
    }
}

// Continue the conversation, reusing the existing non-streaming machinery.
let response = stream.into_response()?;
messages.push(response.to_message());
```

Rationale for each choice:

- **`stream()` is `async` and returns `Result`.** The POST and the status check
  both happen inside it, so a 401 or 429 comes back as `ProviderError::Api`
  exactly as from `generate()`. The caller does not enter a loop to discover the
  request was rejected.
- **`#[non_exhaustive]`.** Retries, typed errors, and capability introspection
  are still ahead on the roadmap. One attribute now avoids a breaking change
  later. Callers need a `_ => {}` arm.
- **`Done` is an event, not an implied end.** `usage` and the finish reason
  arrive in a real frame that would otherwise have nowhere to go. `next()` still
  returns `None` afterwards, so `while let` terminates naturally.
- **Tool calls are emitted complete.** Freyja buffers fragments internally and
  emits one `ToolCall` the moment its arguments are whole — early enough to
  dispatch the tool before the stream closes.
- **Reasoning is split in two.** Providers stream reasoning as human-readable
  text deltas *and* a signed blob completed separately. `ReasoningDelta(String)`
  serves display; `Reasoning { data }` carries the blob that
  `src/provider/model.rs:225-239` documents as mandatory to replay verbatim.
  Without the second variant, a caller who ignores `into_response()` and
  assembles the transcript from events alone would have no way to replay it.
- **`into_response()` closes the multi-turn loop.** Without it, a caller who
  streams, receives a `ToolCall`, runs the tool, and wants to continue has to
  hand-build the assistant turn from the events they observed — including
  placing `Reasoning` blobs in the correct position relative to the tool calls,
  which `src/provider/model.rs:225-239` warns is required and easy to get wrong.
  The assembler already holds the id, model, status, usage, and completed tool
  calls; capturing text and reasoning as well costs one `Vec<OutputContent>` and
  makes the existing `GenerateResponse::to_message` work unchanged.

### Layer 1 — `src/provider/sse.rs`

Framing, and nothing else.

```rust
pub(crate) struct SseFrame { pub event: Option<String>, pub data: String }

pub(crate) struct SseBuffer { bytes: Vec<u8> }

impl SseBuffer {
    fn push(&mut self, chunk: &[u8]);
    fn next_frame(&mut self) -> Option<SseFrame>;   // None = need more bytes
}
```

The buffer holds `Vec<u8>`, not `String`: a chunk boundary can land mid-codepoint,
so UTF-8 is validated only once a complete frame is in hand. Frames are
separated by a blank line; `data:` lines within one frame concatenate; `event:`
names the frame type when present. SSE comment lines (`:`) are dropped.

Bytes come from `reqwest::Response::chunk()`, which is inherent and requires no
cargo feature — this is what keeps the dependency list unchanged.

### Layer 2 — per-dialect decoders

One narrow trait. Implementations live in each dialect's existing `mod.rs`; any
new streaming wire structs go in that dialect's `types.rs`, alongside the
others. No new dialect files.

```rust
pub(crate) trait StreamDecoder: Send {
    fn decode(&mut self, frame: &SseFrame, out: &mut Vec<RawDelta>)
        -> Result<(), ProviderError>;
}

pub(crate) enum RawDelta {
    Text(String),
    ToolStart   { slot: usize, id: String, name: String },
    ToolArgs    { slot: usize, fragment: String },
    ToolReplace { slot: usize, arguments: String },
    ToolEnd     { slot: usize },
    ReasoningText(String),
    ReasoningBlob(Value),
    Meta { id: Option<String>, model: Option<String>,
           status: Option<ResponseStatus>, usage: Option<Usage> },
}
```

`slot` is whatever integer the dialect uses — Anthropic's content-block index,
OpenAiChat's `tool_calls[].index`, Responses' `output_index`. It is
`pub(crate)` and never escapes, so the index-semantics mismatch stays a private
detail of four small functions rather than a problem pushed onto callers.

`ToolReplace` exists for Responses' authoritative
`response.function_call_arguments.done` frame: it overwrites the buffer instead
of appending, so the double-count hazard is one line in one decoder rather than
a separate code path.

Per-dialect mapping:

| Dialect | Text | Tool start | Tool fragments | Tool end |
|---|---|---|---|---|
| OpenAiChat | `choices[].delta.content` | first frame with `tool_calls[].id` | `tool_calls[].function.arguments` | none — flush at close |
| OpenAiResponses | `response.output_text.delta` | `response.output_item.added` | `response.function_call_arguments.delta` | `...arguments.done` (as `ToolReplace` + `ToolEnd`) |
| Anthropic | `content_block_delta` / `text_delta` | `content_block_start` / `tool_use` | `input_json_delta.partial_json` | `content_block_stop` |
| Gemini | TBD — see Open questions | TBD | TBD | TBD |

### Layer 3 — the assembler, `src/provider/stream.rs`

Written once, tested once, shared by all four.

```rust
struct Assembler {
    pending: HashMap<usize, PendingCall>,   // id, name, argument buffer
    captured: Vec<OutputContent>,           // completed parts, in arrival order
    id: String,
    model: String,
    status: ResponseStatus,
    usage: Option<Usage>,
    finished: bool,                         // set when the body closes
}
```

`ToolEnd` emits a complete `StreamEvent::ToolCall`. Dialects that never send an
end frame — OpenAiChat closes the stream instead — are flushed when the body
closes, before `Done`.

`captured` records every completed part as it is emitted: text deltas append to
a trailing `OutputContent::Text` (coalescing consecutive fragments into one
part, matching what `generate()` produces), while `ToolCall` and `Reasoning`
push new parts in arrival order — which is the ordering `to_message()` depends
on. `into_response()` checks `finished` and, if set, assembles a
`GenerateResponse` from `captured` plus the recorded id, model, status, and
usage. If `finished` is false the caller broke out of the loop early, and
returning a response that looks complete but is not would be the worse
behaviour, so it errors instead.

`EventStream` holds the response, an `SseBuffer`, an `Assembler`, a
`Box<dyn StreamDecoder>`, and a queue of events already decoded but not yet
returned. `next()` is a loop: return a queued event if there is one; otherwise
pull a frame from the buffer and decode it; otherwise pull bytes from the
response; return `None` when the body ends and the queue is drained.

### Request building

Each dialect's wire request struct gains `stream: Option<bool>` with
`#[serde(skip_serializing_if = "Option::is_none")]`, so `generate()` serializes
byte-identically to today.

`Client::stream` reuses `Provider::build`, sets the flag, and shares the
header/auth/POST path with `run()` by factoring that into a private
`post(&wire)` helper. Transport stays in one place, as it is now.

One dialect-specific addition: OpenAiChat reports no usage in a stream unless
the body carries `stream_options: { include_usage: true }`. Freyja sets it, so
`Done.usage` is populated consistently rather than being silently `None` on the
most widely-spoken dialect.

### Errors and edge cases

- **Non-2xx** is caught inside `stream()` before `EventStream` is constructed,
  body read in full, surfaced as `ProviderError::Api` — identical to
  `generate()`.
- **Mid-stream error frames** (Anthropic sends `event: error`) become a new
  `ProviderError::Stream { provider: Arc<str>, message: String }`. `Api` would
  have to lie about the status and `InvalidResponse` means "unparseable".
  `ProviderError` is marked `#[non_exhaustive]` at the same time, so the next
  roadmap item (typed API errors) does not force another breaking change. Both
  changes are breaking; at `0.1.0`, freshly published, this is the moment.
- **Truncated stream** — connection closes with arguments still buffered: flush
  the pending calls, then emit `Done` with `status:
  ResponseStatus::Incomplete`. No silent data loss, and the caller can tell.
- **Ignorable frames** — SSE comments, keepalives, OpenAI's `data: [DONE]`
  sentinel — are consumed without producing an event. `next()` loops rather
  than returning a meaningless `Some`.
- **Timeout.** `DEFAULT_TIMEOUT` at `src/provider/mod.rs:29` is passed to
  `reqwest`'s `.timeout()`, which bounds the *entire* request including body
  read — a stream running longer than 120 seconds would be killed
  mid-generation. Streaming requests use `.read_timeout(DEFAULT_TIMEOUT)`
  instead: 120 seconds of *silence*, not 120 seconds total. Callers who supplied
  their own client via `with_http_client` keep whatever they configured;
  documented on `stream()`.

## Alternatives considered

**Approach B — a `StreamingProvider` trait, each dialect accumulating for
itself.** Mirrors the existing `Provider` trait: `build_stream` plus a stateful
`on_frame(&mut self, frame) -> Result<Vec<StreamEvent>, _>`, with each dialect
owning its own buffering. Most faithful to the current structure, and a
dialect's streaming behaviour would be readable in one file. Rejected because
it writes the tool-argument accumulator four times — the piece most likely to
carry an off-by-one or a dropped final fragment. The research showed
accumulation *is* the difficulty here; duplicating it four ways spends
complexity in the wrong place.

**Approach C — no new trait, one `stream.rs` with a four-arm match.** The
laziest rung that plausibly works, and defensible: `Client::generate` already
dispatches on the dialect enum with a four-arm match. Fewest moving parts.
Rejected because it puts four interleaved state machines in one struct —
Anthropic's event-name-driven parsing beside OpenAiChat's field-shape-driven
parsing — just as the same code path is about to grow retries and typed errors.

**Constructors `Client::stream_from_env()` / `Client::stream_custom()`** (the
originally requested shape). Rejected in favour of `client.stream(&request)`:
the endpoint, credentials, and HTTP pool are identical to `generate()`, so
streaming is a per-request concern. Constructor variants would duplicate all
four constructors and force a client to be either streaming or not.

**`impl futures_core::Stream`.** Buys combinators and `axum::Sse` interop, costs
`futures-core` plus `reqwest`'s `stream` feature (`futures-util`, `tokio/fs`) in
every downstream build, and requires a hand-written `poll_next` state machine.
Rejected: the inherent `async fn next()` covers the streaming-UI case at zero
dependency cost. A `stream-trait` cargo feature can add it later without
breaking anyone.

**Fragment-level tool events** (`ToolCallDelta`, or Anthropic's delta+snapshot
pair). Rejected after surveying four reference implementations:

- [genai](https://docs.rs/genai/latest/genai/chat/index.html) — the closest
  analogue, a multi-provider Rust crate normalizing OpenAI, Anthropic, and
  Gemini native protocols — emits `ToolCallChunk(ToolChunk)` where `ToolChunk`
  wraps a **complete `ToolCall`**, the same type its non-streaming path returns.
- [openai-oxide](https://github.com/fortunto2/openai-oxide) emits
  `ToolCallDone { name, arguments, .. }`; its docs state "no manual chunk
  stitching — tool call arguments are automatically assembled from index-based
  deltas".
- The [Anthropic Python
  SDK](https://github.com/anthropics/anthropic-sdk-python/blob/main/helpers.md)
  exposes fragments but never alone: its `input_json` event carries both
  `partial_json` and an accumulated `snapshot`, backed by a hidden `__json_buf`
  per tool-use block.
- The [Vercel AI SDK](https://ai-sdk.dev/docs/ai-sdk-core/tools-and-tool-calling)
  is the most granular — `tool-input-start` / `tool-input-delta` /
  `tool-input-end` plus a complete `tool-call` part — but correlates by an id it
  assigns at start, never by the raw wire index. Its own issue tracker documents
  the cost: the Responses API streams arguments character by character,
  producing hundreds to 1000+ delta events per request.

The consensus is unanimous: nobody hands the caller bare fragments. Absorbing
the divergence is *less* public API than propagating it.

## Testing

No network in any test. Fixtures are recorded SSE frames as string literals.

- **`sse.rs`** — framing edges: a frame split across two `push` calls, and a
  multi-byte codepoint split across a chunk boundary. This is the bug the
  `Vec<u8>` buffer exists to prevent, so it is tested directly.
- **Per-dialect decoders** — fixtures in each dialect's `types.rs` test module
  covering plain text, a tool call whose arguments span several frames, two
  concurrent tool calls, reasoning, and the terminal frame. Tests assert the
  emitted `RawDelta` sequence.
- **Assembler** — fragmented arguments reassemble into exactly one `ToolCall`
  with the right `id` and `name`; two interleaved calls do not cross-contaminate;
  an early close flushes pending calls and emits `Done` with
  `ResponseStatus::Incomplete`.
- **`into_response()`** — a drained stream produces the same `GenerateResponse`
  the equivalent non-streaming fixture parses into: same text (consecutive
  deltas coalesced into one `OutputContent::Text`), same tool calls, same
  reasoning blobs *in the same order*, same usage. Calling it before the stream
  is drained errors rather than returning a truncated response.
- **Regression** — `generate()`'s serialized body is byte-identical before and
  after the `stream: Option<bool>` field is added.
- **`examples/streaming.rs`** alongside the three existing examples, and a
  `no_run` doctest on `Client::stream`.

## Open questions

- **Gemini's Interactions API streaming frame shape is unverified.** The code
  pins `Api-Revision: 2026-05-20` at `src/provider/mod.rs:83`, which is recent,
  and its streaming format was not confirmed during design. Verifying it is the
  first task in the implementation plan. If it turns out not to be SSE at all,
  its decoder needs different framing beneath layer 1 — which Approach A absorbs
  by swapping the frame source for that dialect, but the plan must budget for
  it. If the format cannot be confirmed, Gemini returns
  `ProviderError::UnsupportedCapability` and ships in a follow-up rather than
  blocking the other three.
