# Streaming API Implementation Plan

> **For executing agents:** implement this plan task-by-task. Each step uses checkbox (`- [ ]`) syntax. Do not skip steps. Do not batch commits across tasks.
>
> **Vacuous-pass guard.** `cargo test` exits 0 when its filter matches nothing — `test result: ok. 0 passed; N filtered out` is a *failure* to verify, not a pass. After every test command in this plan, check that the count of tests run is greater than zero and that the expected test names appear in the output. If a filter matches nothing, stop and report it rather than treating the exit code as success. Test paths include the module: a test in `mod tests` inside `src/provider/stream.rs` is `provider::stream::tests::<name>`, and inside `src/provider/gemini/types.rs` is `provider::gemini::types::tests::<name>`.
> **This plan grew during execution.** It was approved with 14 tasks. Tasks 15-21
> were appended after verification runs found defects, and the plan was repaired
> three times mid-run: cargo-test filters that matched nothing (`570c841`),
> doctests and clippy deferred past Task 6 because the code they check does not
> exist yet at that point (`4fe3c3b`), and Task 14's reachability checks
> corrected after a clippy fix changed what they were grepping for (`a7d9c6b`).
> Every change is a separate commit with its reasoning. Read the follow-up
> sections as part of the plan, not as an appendix.

**Goal:** Add `client.stream(&request)` returning an `EventStream` of provider-neutral `StreamEvent`s across all four dialects, with tool-call arguments assembled internally and no new dependencies.

**Architecture:** Three layers. `sse.rs` turns response bytes into SSE frames. Each dialect turns a frame into internal `RawDelta`s. A shared `Assembler` turns `RawDelta`s into public `StreamEvent`s, buffering tool-call argument fragments and capturing completed parts so `into_response()` can return the same `GenerateResponse` that `generate()` would have.

**Tech stack:** Rust edition 2024, MSRV 1.88. Dependencies unchanged: `reqwest` (features `json`), `serde`, `serde_json`. Streaming reads bytes via `reqwest::Response::chunk()`, which is inherent and needs no cargo feature.

**Spec:** `docs/specs/2026-08-09-streaming-api-design.md` (status: approved).

---

## Premortem

**Hidden assumptions:**
- The spec's timeout mitigation ("build the streaming request with `.read_timeout(DEFAULT_TIMEOUT)`") is not implementable. Verified against `~/.cargo/registry/src/*/reqwest-0.13.4/src/async_impl/client.rs:1456` and `request.rs:294`: `read_timeout` exists on `ClientBuilder` only; `RequestBuilder` has `timeout` alone. — Task 1 changes `default_http()` to use `.read_timeout(DEFAULT_TIMEOUT)` in place of `.timeout(DEFAULT_TIMEOUT)`, so both paths are bounded by *inactivity* rather than total duration. This is a deliberate behavior change to `generate()` and is called out again under Spec-misalignment.
- Gemini's streaming frame shape was an open question in the spec. Resolved during planning from the [official streaming guide](https://ai.google.dev/gemini-api/docs/interactions/streaming): SSE, events `step.start` / `step.delta` / `step.stop` / `interaction.completed`, with `event_type` duplicated inside the JSON payload. — Task 12 encodes those exact frames as fixtures; if the live API disagrees, the fixture test still passes and only integration reveals it. Accepted: no network tests exist in this repo and adding them is out of scope.
- Each dialect's non-streaming parser turns unmodeled blocks into `OutputContent::Reasoning { data: <whole block> }` (`anthropic/types.rs:378`, `gemini/types.rs:325`, `openai_responses/types.rs:325`). `into_response()` claims parity with `generate()`, which requires each streaming decoder to reconstruct that *same whole block* from its start frame plus deltas, not just the signature. — Tasks 10, 12, 13 each reconstruct the full block object; the Task 6 assembler test asserts blob equality, not merely presence.
- `openai_chat` produces no `OutputContent::Reasoning` at all — `types.rs:183` discards `InputContent::Reasoning` and the parser never emits one. — Task 11's decoder therefore emits no reasoning deltas, and its test asserts that, so a future contributor does not "fix" a non-bug.

**Irreversible / risky steps:**
- Task 1 adds a variant to the public `ProviderError` and marks it `#[non_exhaustive]`. Both are breaking changes for downstream matchers. — The crate is at `0.1.0` and was published days ago; the spec approves this explicitly. Revertable with `git revert` in one commit, but a release published in between would not be. Do not publish mid-plan.
- Task 1 changes the default HTTP client's timeout semantics for `generate()` as well as `stream()`. — A single-line revert restores `.timeout()`. The existing test suite does not cover timeout behavior, so the change is invisible to CI; it is documented on `Client::new` in the same task.
- Everything else creates new files or appends to existing ones; `git revert <commit>` is sufficient with no follow-up.

**Spec-misalignment:**
- **Timeout.** The spec specifies per-request `read_timeout`; the plan applies it to the shared client builder instead, because reqwest 0.13.4 offers no other place to put it. The observable difference: callers who pass their own client via `with_http_client` now keep *whatever they configured*, including a total `.timeout()` that will cut a long stream short. The spec promised that outcome; the plan delivers it, but by omission rather than by design. — Documented on `Client::stream` in Task 8, with the explicit instruction to use `read_timeout` on a custom client.
- `into_response()`'s error condition. The spec says it errors "if the stream has not been drained to `None`". The plan interprets "drained" as the assembler's `finished` flag, set when `chunk()` returns `None` — not when the caller has observed the `Done` event. A caller who reads the final `Done` but never loops once more still gets an error. — Task 6's test locks this interpretation in by name (`into_response_before_drain_errors`), and `Client::stream`'s docs state it.
- The spec's `Done` event and `into_response()` overlap: both carry id, model, status, usage. The plan emits both rather than choosing. — Deliberate; the spec lists both under Goals.

**Verify-clause weakness:**
- "Tests pass" would pass on an empty test file, so every verify clause below names the specific test function(s) by name and the assertion they make.
- Task 9's group of four decoder tasks could each pass while the dialect is never reachable from `Client::stream`. — Task 8 wires the dispatch with stubs first and Task 14 asserts every dialect returns a decoder rather than the stub, so a forgotten wiring fails.
- `cargo test` alone would not catch a decoder that compiles but emits nothing. — Each decoder task's verify clause names an assertion on the *emitted `RawDelta` sequence*, not merely on absence of panic.

---

## File structure

New:
- `src/provider/sse.rs` — SSE framing: bytes in, `SseFrame` out. No dialect knowledge.
- `src/provider/stream.rs` — public `StreamEvent` and `EventStream`; internal `RawDelta`, `StreamDecoder` trait, and the shared `Assembler`.
- `examples/streaming.rs` — runnable example.

Modified:
- `src/provider/model.rs:470-504` — add `ProviderError::Stream`, mark the enum `#[non_exhaustive]`, add its `Display` arm.
- `src/provider/mod.rs:28-29` — timeout constant doc; `:408-413` `default_http()`; `:12-21` module declarations and re-exports; `:264-406` `Client::stream`, the shared `post()` helper, and `ProviderDialect::stream_query()`.
- `src/provider/openai_chat/types.rs` — `stream` + `stream_options` fields, `streaming()`, decoder, fixtures.
- `src/provider/openai_chat/mod.rs` — `Decoder` type.
- `src/provider/anthropic/types.rs` — `stream` field, `streaming()`, decoder, fixtures.
- `src/provider/anthropic/mod.rs` — `Decoder` type.
- `src/provider/openai_responses/types.rs` — `stream` field, `streaming()`, decoder, fixtures.
- `src/provider/openai_responses/mod.rs` — `Decoder` type.
- `src/provider/gemini/types.rs` — `stream` field, `streaming()`, decoder, fixtures.
- `src/provider/gemini/mod.rs` — `Decoder` type.
- `src/lib.rs:64-68` — re-export `EventStream` and `StreamEvent`; `:33-58` add a streaming doc section.
- `README.md:88` — drop streaming from the "remaining" list.

---

### Task 1: ProviderError::Stream and timeout semantics → verify: `cargo test --all-features` passes; `error_stream_displays_provider_and_message` asserts the `Display` output is `"acme stream failed: boom"`

**Files:**
- Modify: `src/provider/model.rs:469-529`
- Modify: `src/provider/mod.rs:28-29`, `src/provider/mod.rs:264-271`, `src/provider/mod.rs:408-413`

- [x] **Step 1: Write the failing test**

Append to the `mod tests` block at the end of `src/provider/model.rs` (after `tool_result_builds_a_tool_turn`, before the closing `}`):

```rust
    #[test]
    fn error_stream_displays_provider_and_message() {
        let error = ProviderError::Stream {
            provider: "acme".into(),
            message: "boom".into(),
        };

        assert_eq!(error.to_string(), "acme stream failed: boom");
    }
```

- [x] **Step 2: Run the test to verify it fails**

Run: `cargo test --all-features error_stream_displays_provider_and_message`
Expected: FAIL to compile, with an error naming `Stream` as not a variant of `ProviderError`.

- [x] **Step 3: Add the variant**

In `src/provider/model.rs`, replace the attribute and opening line of the enum at line 469-470:

```rust
#[derive(Debug)]
pub enum ProviderError {
```

with:

```rust
#[derive(Debug)]
#[non_exhaustive]
pub enum ProviderError {
```

Then add this variant immediately before the closing `}` of the enum (after the `InvalidResponse` variant that ends at line 503):

```rust
    /// The response streamed successfully up to a point, then failed.
    ///
    /// Covers a provider's own mid-stream error frame and a body that ends
    /// before the response is complete. Distinct from [`Self::Api`], which
    /// reports a non-success HTTP status, and from [`Self::InvalidResponse`],
    /// which reports a body that could not be parsed at all.
    Stream {
        /// Endpoint whose stream failed.
        provider: Arc<str>,
        /// What went wrong.
        message: String,
    },
```

- [x] **Step 4: Add the Display arm**

In the same file, inside `impl fmt::Display for ProviderError`, add this arm immediately before the closing `}` of the `match` (after the `InvalidResponse` arm):

```rust
            Self::Stream { provider, message } => {
                write!(f, "{provider} stream failed: {message}")
            }
```

- [x] **Step 5: Run the test to verify it passes**

Run: `cargo test --all-features error_stream_displays_provider_and_message`
Expected: PASS.

- [x] **Step 6: Switch the default client to a read timeout**

In `src/provider/mod.rs`, replace lines 28-29:

```rust
/// Default per-request timeout applied by [`Client::new`].
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
```

with:

```rust
/// Default inactivity timeout applied by [`Client::new`].
///
/// This bounds the gap between bytes, not the total duration of a request.
/// A total timeout would cap how long a response may take to generate, which
/// is wrong for [`Client::stream`]: a long generation is not a stalled one.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
```

Then replace `default_http` at lines 408-413:

```rust
fn default_http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(DEFAULT_TIMEOUT)
        .build()
        .unwrap_or_default()
}
```

with:

```rust
fn default_http() -> reqwest::Client {
    reqwest::Client::builder()
        .read_timeout(DEFAULT_TIMEOUT)
        .build()
        .unwrap_or_default()
}
```

- [x] **Step 7: Document the change on the constructor**

In `src/provider/mod.rs`, replace the doc comment on `Client::new` at lines 265-268:

```rust
    /// Creates a client with a pooled HTTP client and a 120 second timeout.
    ///
    /// Accepts anything that converts into a [`ProviderConfig`], including a
    /// [`ProviderType`] preset.
```

with:

```rust
    /// Creates a client with a pooled HTTP client and a 120 second inactivity
    /// timeout.
    ///
    /// The timeout bounds silence, not total duration, so a slow generation is
    /// not cut short. Use [`Client::with_http_client`] to impose a total cap.
    ///
    /// Accepts anything that converts into a [`ProviderConfig`], including a
    /// [`ProviderType`] preset.
```

- [x] **Step 8: Run the whole suite**

Run: `cargo test --all-features`
Expected: PASS, all tests.

- [x] **Step 9: Commit**

```bash
git add src/provider/model.rs src/provider/mod.rs
git commit -m "feat: add ProviderError::Stream and bound requests by inactivity

Marks ProviderError #[non_exhaustive] so later error work does not break
downstream matchers again, and switches the default HTTP client from a
total timeout to a read timeout so a long generation is not mistaken for
a stalled connection."
```

---

### Task 2: SSE framing → verify: `cargo test --all-features sse::` passes; `splits_frames_across_pushes` and `handles_codepoint_split_across_chunks` both assert the reassembled data equals `"café"`

**Files:**
- Create: `src/provider/sse.rs`
- Modify: `src/provider/mod.rs:12-18`

- [x] **Step 1: Declare the module**

In `src/provider/mod.rs`, replace lines 17-18:

```rust
mod model;
mod presets;
```

with:

```rust
mod model;
mod presets;
mod sse;
```

- [x] **Step 2: Write the failing tests**

Create `src/provider/sse.rs` containing only this:

```rust
//! Server-sent event framing, shared by every streaming dialect.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_frames_across_pushes() {
        let mut buffer = SseBuffer::default();

        buffer.push(b"event: delta\ndata: ca");
        assert!(buffer.next_frame().is_none(), "frame is not complete yet");

        buffer.push("fé\n\ndata: second\n\n".as_bytes());

        let first = buffer.next_frame().expect("first frame");
        assert_eq!(first.event.as_deref(), Some("delta"));
        assert_eq!(first.data, "café");

        let second = buffer.next_frame().expect("second frame");
        assert_eq!(second.event, None);
        assert_eq!(second.data, "second");

        assert!(buffer.next_frame().is_none());
    }

    #[test]
    fn handles_codepoint_split_across_chunks() {
        let mut buffer = SseBuffer::default();
        let text = "data: café\n\n".as_bytes();

        // 'é' is two bytes; split the buffer between them.
        let split = text.len() - 4;
        buffer.push(&text[..split]);
        assert!(buffer.next_frame().is_none());

        buffer.push(&text[split..]);
        assert_eq!(buffer.next_frame().expect("frame").data, "café");
    }

    #[test]
    fn joins_multiple_data_lines_and_skips_comments() {
        let mut buffer = SseBuffer::default();
        buffer.push(b": keepalive\ndata: one\ndata: two\n\n");

        assert_eq!(buffer.next_frame().expect("frame").data, "one\ntwo");
    }

    #[test]
    fn accepts_crlf_separators() {
        let mut buffer = SseBuffer::default();
        buffer.push(b"event: ping\r\ndata: hi\r\n\r\n");

        let frame = buffer.next_frame().expect("frame");
        assert_eq!(frame.event.as_deref(), Some("ping"));
        assert_eq!(frame.data, "hi");
    }
}
```

- [x] **Step 3: Run the tests to verify they fail**

Run: `cargo test --all-features sse::`
Expected: FAIL to compile, with errors naming `SseBuffer` as not found in this scope.

- [x] **Step 4: Write the implementation**

In `src/provider/sse.rs`, insert this above the `#[cfg(test)] mod tests` block, directly under the `//!` doc comment:

```rust
/// One decoded server-sent event.
pub(crate) struct SseFrame {
    /// The `event:` name, when the frame carried one.
    pub(crate) event: Option<String>,
    /// The `data:` payload, with multiple `data:` lines joined by newlines.
    pub(crate) data: String,
}

/// Accumulates response bytes and yields complete frames.
///
/// Bytes are held as `Vec<u8>` rather than `String` because a chunk boundary
/// can land in the middle of a multi-byte codepoint. UTF-8 is only interpreted
/// once a whole frame is in hand.
#[derive(Default)]
pub(crate) struct SseBuffer {
    bytes: Vec<u8>,
}

impl SseBuffer {
    /// Appends raw bytes from the response body.
    pub(crate) fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend_from_slice(chunk);
    }

    /// Splits off the next complete frame, or `None` when more bytes are needed.
    pub(crate) fn next_frame(&mut self) -> Option<SseFrame> {
        let (end, next) = separator(&self.bytes)?;
        let raw: Vec<u8> = self.bytes.drain(..next).collect();
        let text = String::from_utf8_lossy(&raw[..end]);

        let mut event = None;
        let mut data = String::new();
        for line in text.lines() {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.is_empty() || line.starts_with(':') {
                continue;
            }
            if let Some(rest) = line.strip_prefix("event:") {
                event = Some(rest.trim().to_string());
            } else if let Some(rest) = line.strip_prefix("data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
            }
        }

        Some(SseFrame { event, data })
    }
}

/// Finds the blank line ending a frame.
///
/// Returns the offset where the frame's text ends and the offset where the next
/// frame begins, which differ by the length of the separator.
fn separator(bytes: &[u8]) -> Option<(usize, usize)> {
    for index in 0..bytes.len().saturating_sub(1) {
        if bytes[index..].starts_with(b"\r\n\r\n") {
            return Some((index, index + 4));
        }
        if bytes[index..].starts_with(b"\n\n") {
            return Some((index, index + 2));
        }
    }
    None
}
```

- [x] **Step 5: Run the tests to verify they pass**

Run: `cargo test --all-features sse::`
Expected: PASS, 4 tests.

- [x] **Step 6: Commit**

```bash
git add src/provider/sse.rs src/provider/mod.rs
git commit -m "feat: add SSE frame buffering

Holds bytes rather than text so a chunk boundary landing mid-codepoint
cannot corrupt a frame, which is the failure mode the tests pin down."
```

---

### Task 3: StreamEvent and the internal delta model → verify: `cargo build --all-targets` succeeds and `cargo test --all-features stream::tests::event_is_non_exhaustive_and_comparable` passes

**Files:**
- Create: `src/provider/stream.rs`
- Modify: `src/provider/mod.rs:17-21`

- [x] **Step 1: Declare and re-export the module**

In `src/provider/mod.rs`, replace lines 17-21:

```rust
mod model;
mod presets;
mod sse;

pub use model::*;
pub use presets::ProviderType;
```

with:

```rust
mod model;
mod presets;
mod sse;
mod stream;

pub use model::*;
pub use presets::ProviderType;
pub use stream::{EventStream, StreamEvent};
```

Note: `EventStream` does not exist until Task 5. Between this step and Task 5 the crate does not compile; Task 3 Step 5 adds a placeholder so each task still ends green.

- [x] **Step 2: Write the failing test**

Create `src/provider/stream.rs` containing only this:

```rust
//! Streaming: neutral events, the shared assembler, and the public stream type.

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
```

- [x] **Step 3: Run the test to verify it fails**

Run: `cargo test --all-features stream::tests::event_is_non_exhaustive_and_comparable`
Expected: FAIL to compile, with errors naming `StreamEvent` as not found in this scope.

- [x] **Step 4: Write the types**

In `src/provider/stream.rs`, insert this directly under the `//!` doc comment and above the test module:

```rust
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
```

- [x] **Step 5: Add a placeholder so the crate compiles**

`src/provider/mod.rs` now re-exports `EventStream`, which Task 5 defines. Add this to `src/provider/stream.rs`, directly after the `StreamDecoder` trait, so this task ends with a compiling crate:

```rust
/// A live stream of [`StreamEvent`]s.
///
/// Replaced with the real implementation in the following task; this shell
/// exists so the crate compiles between commits.
pub struct EventStream {
    pub(crate) _private: (),
}
```

- [x] **Step 6: Run the test to verify it passes**

Run: `cargo test --all-features stream::tests::event_is_non_exhaustive_and_comparable`
Expected: PASS.

- [x] **Step 7: Confirm the whole crate builds**

Run: `cargo build --all-targets`
Expected: success, no errors.

- [x] **Step 8: Commit**

```bash
git add src/provider/stream.rs src/provider/mod.rs
git commit -m "feat: add the neutral StreamEvent model

RawDelta stays private so each dialect's correlation index -- content
block, tool call, output item -- never has to be reconciled with the
others in public API."
```

---

### Task 4: Assembler, part one — text and metadata → verify: `cargo test --all-features stream::tests::assembler_coalesces_text` passes, asserting two text deltas produce two `TextDelta` events but a single `OutputContent::Text("ab")` in `captured`

**Files:**
- Modify: `src/provider/stream.rs`

- [x] **Step 1: Write the failing test**

In `src/provider/stream.rs`, add to the `mod tests` block:

```rust
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
        assert_eq!(
            assembler.captured,
            vec![OutputContent::Text("ab".into())]
        );
        assert_eq!(assembler.id, "resp_1");
        assert_eq!(assembler.model, "test-model");
    }
```

Also extend the test module's imports — replace `use super::*;` at the top of `mod tests` with:

```rust
    use super::*;
    use crate::provider::OutputContent;
```

- [x] **Step 2: Run the test to verify it fails**

Run: `cargo test --all-features stream::tests::assembler_coalesces_text`
Expected: FAIL to compile, with an error naming `Assembler` as not found in this scope.

- [x] **Step 3: Write the assembler**

In `src/provider/stream.rs`, add this after the `StreamDecoder` trait and before the `EventStream` placeholder:

```rust
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
            provider_metadata: None,
        })
    }
}
```

- [x] **Step 4: Run the test to verify it passes**

Run: `cargo test --all-features stream::tests::assembler_coalesces_text`
Expected: PASS.

- [x] **Step 5: Commit**

```bash
git add src/provider/stream.rs
git commit -m "feat: add the shared streaming assembler

Coalescing consecutive text deltas into one content part is what lets a
drained stream produce the same GenerateResponse the non-streaming path
would have."
```

---

### Task 5: Assembler, part two — tool calls and drain semantics → verify: `cargo test --all-features stream::tests::assembler_` passes all four tests; `assembler_flushes_unended_calls` asserts a call with no `ToolEnd` still emits before `Done`

**Files:**
- Modify: `src/provider/stream.rs`

- [x] **Step 1: Write the failing tests**

In `src/provider/stream.rs`, add to the `mod tests` block:

```rust
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
```

- [x] **Step 2: Run the tests to verify they fail**

Run: `cargo test --all-features stream::tests::assembler_`
Expected: `assembler_coalesces_text` PASSES; the four new tests FAIL. If they instead all pass, the implementation from Task 4 already covers them — verify by reading `absorb` and `close`, then proceed to Step 4.

- [x] **Step 3: Confirm no implementation change is needed**

The assembler written in Task 4 already implements every behavior these tests assert. This step exists to make that explicit rather than leaving a task with no implementation: read `Assembler::absorb`, `Assembler::finish_call`, and `Assembler::close` in `src/provider/stream.rs` and confirm each assertion maps to code that exists. Make no edits unless a test fails.

- [x] **Step 4: Run the tests to verify they pass**

Run: `cargo test --all-features stream::tests::assembler_`
Expected: PASS, 5 tests.

- [x] **Step 5: Commit**

```bash
git add src/provider/stream.rs
git commit -m "test: pin down tool assembly and drain semantics

Covers interleaved concurrent calls, the OpenAiChat case where no end
frame ever arrives, and into_response refusing to hand back a response
from a stream the caller abandoned early."
```

---

### Task 6: EventStream → verify: `cargo test --all-features stream::tests::event_stream_` passes; `event_stream_drains_a_recorded_body` asserts a two-chunk fake body yields `TextDelta("hi")`, `Done`, then `None`

**Files:**
- Modify: `src/provider/stream.rs`

- [x] **Step 1: Write the failing test**

In `src/provider/stream.rs`, add to the `mod tests` block:

```rust
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
```

- [x] **Step 2: Run the test to verify it fails**

Run: `cargo test --all-features stream::tests::event_stream_drains_a_recorded_body`
Expected: FAIL to compile, with errors naming `for_test` and `next_blocking` as not found for `EventStream`.

- [x] **Step 3: Replace the placeholder with the real EventStream**

In `src/provider/stream.rs`, delete the `EventStream` placeholder added in Task 3 Step 5 and put this in its place:

```rust
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
    fn for_test(
        provider: Arc<str>,
        decoder: Box<dyn StreamDecoder>,
        chunks: Vec<Vec<u8>>,
    ) -> Self {
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
```

- [x] **Step 4: Add the test module's SseFrame import**

In `src/provider/stream.rs`, the test module needs `SseFrame` for `TestDecoder`. It is already imported at module scope by `use crate::provider::sse::SseFrame;`, and `use super::*;` re-exports it into the tests. No edit needed — confirm by running the tests.

- [x] **Step 5: Run the test to verify it passes**

Run: `cargo test --all-features stream::tests::event_stream_drains_a_recorded_body`
Expected: PASS.

- [x] **Step 6: Run the unit test suite**

Run: `cargo test --all-features --lib`
Expected: PASS, all tests (66 at this point in the plan).

`--lib` is deliberate and the doctests are NOT run here. The doc example
written on `EventStream` in Step 3 calls `client.stream(&request)` and imports
`freyja::StreamEvent`; the first is added in Task 7 and the second in Task 12.
A `no_run` doctest is still *compiled*, so a full `cargo test --all-features`
cannot pass until Task 12. Task 12 Step 3 runs `cargo test --doc` once both
exist, and Task 14 runs the full suite.

Clippy is likewise deferred. `cargo clippy -- -D warnings` fails here with
`dead_code` on `RawDelta`'s variants, `Assembler::new`, `Body::Live`, and
`EventStream::new`, because the only non-test consumer of any of them is
`Client::stream` (Task 7) and the only constructors of `RawDelta` variants are
the four decoders (Tasks 8-11). Do NOT add `#[allow(dead_code)]` to silence
this — the warnings are correct and disappear on their own once those tasks
land. Task 13 Step 4 is the first point at which clippy can be clean.

- [x] **Step 7: Commit**

```bash
git add src/provider/stream.rs
git commit -m "feat: add EventStream over a pluggable body

The recorded-body variant is test-only but load-bearing: reqwest::Response
cannot be built from bytes, and leaving the frame/decode/assemble loop
untested would put the most stateful code in the crate beyond reach."
```

---

### Task 7: Wire Client::stream with stub decoders → verify: `cargo test --all-features provider::tests::stream_url_appends_alt_sse_for_gemini` passes and `cargo build --all-targets` succeeds with all four dialects reachable

**Files:**
- Modify: `src/provider/mod.rs:56-87`, `src/provider/mod.rs:344-406`

- [x] **Step 1: Write the failing test**

In `src/provider/mod.rs`, add to the `mod tests` block at the end of the file:

```rust
    #[test]
    fn stream_url_appends_alt_sse_for_gemini() {
        let gemini = ProviderConfig::new(ProviderDialect::Gemini, "g", "https://x.test/v1");
        assert_eq!(gemini.url(), "https://x.test/v1/interactions");
        assert_eq!(
            gemini.stream_url(),
            "https://x.test/v1/interactions?alt=sse",
            "Gemini selects SSE by query parameter, not by body field alone"
        );

        // Every other dialect streams from the same URL it generates from.
        let anthropic = ProviderConfig::new(ProviderDialect::Anthropic, "a", "https://x.test/v1");
        assert_eq!(anthropic.stream_url(), anthropic.url());
    }
```

- [x] **Step 2: Run the test to verify it fails**

Run: `cargo test --all-features --lib stream_url_appends_alt_sse_for_gemini`
Expected: FAIL to compile, with an error naming `stream_url` as not found for `ProviderConfig`.

- [x] **Step 3: Add the dialect's stream query and the config helper**

In `src/provider/mod.rs`, add this method to `impl ProviderDialect`, immediately after `required_headers` (which ends at line 86):

```rust
    /// The query string that selects SSE, for dialects that need one.
    ///
    /// Gemini's Interactions API takes `?alt=sse` in addition to the body's
    /// `stream` field; the others are selected by the body alone.
    pub fn stream_query(self) -> Option<&'static str> {
        match self {
            Self::Gemini => Some("alt=sse"),
            Self::OpenAiResponses | Self::OpenAiChat | Self::Anthropic => None,
        }
    }
```

Then add this method to `impl ProviderConfig`, immediately after `url` (which ends at line 182):

```rust
    /// The full URL streaming requests are sent to.
    pub fn stream_url(&self) -> String {
        match self.dialect.stream_query() {
            Some(query) => format!("{}?{}", self.url(), query),
            None => self.url(),
        }
    }
```

- [x] **Step 4: Run the test to verify it passes**

Run: `cargo test --all-features --lib stream_url_appends_alt_sse_for_gemini`
Expected: PASS.

- [x] **Step 5: Factor the shared POST out of `run`**

In `src/provider/mod.rs`, replace the body of `run` at lines 362-405 with this pair of methods:

```rust
    /// Convert, POST, check status, parse. Shared by every dialect, which is
    /// why none of them owns transport code.
    async fn run<P: Provider>(
        &self,
        provider: P,
        request: &GenerateRequest,
    ) -> Result<GenerateResponse, ProviderError> {
        let wire = provider.build(request, &self.config)?;
        let response = self.post(self.config.url(), &wire).await?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| ProviderError::Http(error.to_string()))?;

        if !status.is_success() {
            return Err(ProviderError::Api {
                provider: self.config.name.clone(),
                status: status.as_u16(),
                body,
            });
        }

        provider.parse(&body, &self.config)
    }

    /// Sends one POST with this endpoint's headers and credentials.
    async fn post<T: Serialize>(
        &self,
        url: String,
        wire: &T,
    ) -> Result<reqwest::Response, ProviderError> {
        let mut post = self.http.post(url);
        for (name, value) in self.config.dialect.required_headers() {
            post = post.header(*name, *value);
        }
        for (name, value) in &self.config.extra_headers {
            post = post.header(name, value);
        }
        if let Some(key) = &self.api_key {
            post = match self.config.auth {
                Auth::Bearer => post.bearer_auth(key),
                Auth::Header(name) => post.header(name, key),
                Auth::None => post,
            };
        }

        post.json(wire)
            .send()
            .await
            .map_err(|error| ProviderError::Http(error.to_string()))
    }
```

- [x] **Step 6: Add `Client::stream` with stub decoders**

In `src/provider/mod.rs`, add this method to `impl Client`, immediately after `generate` (which ends at line 358):

```rust
    /// Opens a streaming generation.
    ///
    /// Returns once the provider has accepted the request, so a non-success
    /// status arrives here as [`ProviderError::Api`] rather than mid-stream.
    ///
    /// The default HTTP client bounds *inactivity*, not total duration, so a
    /// long generation is not cut short. A client supplied through
    /// [`Client::with_http_client`] keeps whatever it was built with — set
    /// `read_timeout` rather than `timeout` on it, or a long stream will be
    /// killed part-way.
    ///
    /// ```no_run
    /// # async fn run() -> Result<(), freyja::ProviderError> {
    /// use freyja::{Client, GenerateRequest, Message, ProviderType, Role, StreamEvent};
    ///
    /// let client = Client::from_env(ProviderType::OpenAi).unwrap();
    /// let request = GenerateRequest::new().message(Message::text(Role::User, "Hi"));
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
    pub async fn stream(
        &self,
        request: &GenerateRequest,
    ) -> Result<EventStream, ProviderError> {
        let (wire, decoder): (serde_json::Value, Box<dyn stream::StreamDecoder>) =
            match self.config.dialect {
                ProviderDialect::OpenAiResponses => {
                    let body = openai_responses::OpenAiResponsesProvider
                        .build(request, &self.config)?
                        .streaming();
                    (to_value(&body, &self.config)?, Box::new(openai_responses::Decoder::default()))
                }
                ProviderDialect::OpenAiChat => {
                    let body = openai_chat::OpenAiChatProvider
                        .build(request, &self.config)?
                        .streaming();
                    (to_value(&body, &self.config)?, Box::new(openai_chat::Decoder::default()))
                }
                ProviderDialect::Gemini => {
                    let body = gemini::GeminiProvider
                        .build(request, &self.config)?
                        .streaming();
                    (to_value(&body, &self.config)?, Box::new(gemini::Decoder::default()))
                }
                ProviderDialect::Anthropic => {
                    let body = anthropic::AnthropicProvider
                        .build(request, &self.config)?
                        .streaming();
                    (to_value(&body, &self.config)?, Box::new(anthropic::Decoder::default()))
                }
            };

        let response = self.post(self.config.stream_url(), &wire).await?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .map_err(|error| ProviderError::Http(error.to_string()))?;
            return Err(ProviderError::Api {
                provider: self.config.name.clone(),
                status: status.as_u16(),
                body,
            });
        }

        Ok(EventStream::new(
            self.config.name.clone(),
            decoder,
            response,
        ))
    }
```

Then add this free function at the end of `src/provider/mod.rs`, immediately after `default_http`:

```rust
/// Erases a dialect's request type so `stream` can pick a decoder and a body in
/// one `match` without four near-identical arms after it.
fn to_value<T: Serialize>(
    wire: &T,
    config: &ProviderConfig,
) -> Result<serde_json::Value, ProviderError> {
    serde_json::to_value(wire).map_err(|error| ProviderError::InvalidRequest {
        provider: config.name.clone(),
        message: error.to_string(),
    })
}
```

- [x] **Step 7: Add stub decoders and `streaming()` to each dialect**

For each of the four dialect modules, add a stub so the crate compiles. In `src/provider/anthropic/mod.rs`, `src/provider/gemini/mod.rs`, `src/provider/openai_chat/mod.rs`, and `src/provider/openai_responses/mod.rs`, append this to each file, adjusting nothing but the module it lives in:

```rust
/// Decodes this dialect's SSE frames. Filled in by its own task.
#[derive(Default)]
pub(crate) struct Decoder;

impl crate::provider::stream::StreamDecoder for Decoder {
    fn decode(
        &mut self,
        _frame: &crate::provider::sse::SseFrame,
        _out: &mut Vec<crate::provider::stream::RawDelta>,
    ) -> Result<(), crate::provider::ProviderError> {
        Ok(())
    }
}
```

In each dialect's `types.rs`, add a `stream` field to its `Request` struct and a `streaming()` method. For `src/provider/anthropic/types.rs`, `src/provider/gemini/types.rs`, and `src/provider/openai_responses/types.rs`, add this field as the last field of `struct Request`:

```rust
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
```

and this method inside each file's `impl Request` block:

```rust
    /// Marks this body as a streaming request.
    pub(crate) fn streaming(mut self) -> Self {
        self.stream = Some(true);
        self
    }
```

For `src/provider/openai_chat/types.rs`, add both of these as the last fields of `struct Request`:

```rust
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<serde_json::Value>,
```

and this method inside its `impl Request` block:

```rust
    /// Marks this body as a streaming request.
    ///
    /// `include_usage` is required or the dialect reports no token counts at
    /// all when streaming, which would leave `Done.usage` empty on the most
    /// widely-spoken dialect.
    pub(crate) fn streaming(mut self) -> Self {
        self.stream = Some(true);
        self.stream_options = Some(serde_json::json!({"include_usage": true}));
        self
    }
```

Each `Request::build` constructs the struct with a struct literal; add `stream: None` (and `stream_options: None` for `openai_chat`) to each constructor so it compiles. Search each file for `Ok(Self {` or `Ok(Request {` to find it.

Also make the two internal modules visible to the dialects: in `src/provider/mod.rs`, change `mod sse;` to `pub(crate) mod sse;` and `mod stream;` to `pub(crate) mod stream;`.

- [x] **Step 8: Verify the build and the existing suite**

Run: `cargo build --all-targets`
Expected: success.

Run: `cargo test --all-features --lib`
Expected: PASS, all tests. In particular the dialects' existing `build` tests must still pass, proving `stream: None` did not change `generate()`'s serialized body.

- [x] **Step 9: Commit**

```bash
git add src/provider/mod.rs src/provider/anthropic src/provider/gemini src/provider/openai_chat src/provider/openai_responses
git commit -m "feat: wire Client::stream with per-dialect decoder stubs

Stubs rather than a single dialect so the four decoder implementations
that follow touch disjoint files and can land independently."
```

---

<!-- parallel-group: dialect-decoders
     rationale: Tasks 8-11 each touch exactly one dialect's mod.rs and types.rs. Task 7 already created the stub, the dispatch, and the Request::streaming method for all four, so no task in this group edits a shared file, references another's output, or touches Cargo.toml. The union of their Files sections has no collision. -->

### Task 8: OpenAiChat decoder → verify: `cargo test --all-features openai_chat::types::tests::decodes_streaming_` passes; `decodes_streaming_tool_call` asserts the emitted `RawDelta` sequence is `ToolStart{slot:0}`, `ToolArgs{"{\"loc"}`, `ToolArgs{"ation\":\"NYC\"}"}` with no `ToolEnd`

**Files:**
- Modify: `src/provider/openai_chat/mod.rs`
- Modify: `src/provider/openai_chat/types.rs`

- [x] **Step 1: Write the failing tests**

In `src/provider/openai_chat/types.rs`, add to the `mod tests` block:

```rust
    use crate::provider::sse::SseFrame;
    use crate::provider::stream::{RawDelta, StreamDecoder};

    fn decode_all(frames: &[&str]) -> Vec<RawDelta> {
        let mut decoder = crate::provider::openai_chat::Decoder::default();
        let mut out = Vec::new();
        for data in frames {
            let frame = SseFrame {
                event: None,
                data: (*data).to_string(),
            };
            decoder.decode(&frame, &mut out).expect("decodes");
        }
        out
    }

    #[test]
    fn decodes_streaming_text() {
        let deltas = decode_all(&[
            r#"{"id":"chatcmpl-1","model":"gpt-4o","choices":[{"delta":{"content":"Hel"}}]}"#,
            r#"{"id":"chatcmpl-1","model":"gpt-4o","choices":[{"delta":{"content":"lo"}}]}"#,
            "[DONE]",
        ]);

        assert!(
            deltas.iter().any(|d| *d == RawDelta::Text("Hel".into())),
            "{deltas:?}"
        );
        assert!(
            deltas.iter().any(|d| *d == RawDelta::Text("lo".into())),
            "{deltas:?}"
        );
        assert!(
            !deltas.iter().any(|d| matches!(d, RawDelta::Text(t) if t == "[DONE]")),
            "the sentinel must not become text: {deltas:?}"
        );
    }

    #[test]
    fn decodes_streaming_tool_call() {
        let deltas = decode_all(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_abc","function":{"name":"get_weather","arguments":""}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"loc"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"ation\":\"NYC\"}"}}]}}]}"#,
        ]);

        assert_eq!(
            deltas,
            vec![
                RawDelta::ToolStart {
                    slot: 0,
                    id: "call_abc".into(),
                    name: "get_weather".into(),
                },
                RawDelta::ToolArgs {
                    slot: 0,
                    fragment: "{\"loc".into(),
                },
                RawDelta::ToolArgs {
                    slot: 0,
                    fragment: "ation\":\"NYC\"}".into(),
                },
            ],
            "this dialect never ends a call; the assembler flushes at close"
        );
    }

    #[test]
    fn decodes_streaming_usage_and_finish_reason() {
        let deltas = decode_all(&[
            r#"{"id":"chatcmpl-1","model":"gpt-4o","choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            r#"{"id":"chatcmpl-1","model":"gpt-4o","choices":[],"usage":{"prompt_tokens":11,"completion_tokens":9,"total_tokens":20}}"#,
        ]);

        let usage = deltas
            .iter()
            .find_map(|d| match d {
                RawDelta::Meta {
                    usage: Some(usage), ..
                } => Some(*usage),
                _ => None,
            })
            .expect("usage arrives when stream_options.include_usage is set");
        assert_eq!(usage.total_tokens, 20);

        assert!(
            deltas.iter().any(|d| matches!(
                d,
                RawDelta::Meta {
                    status: Some(ResponseStatus::Completed),
                    ..
                }
            )),
            "{deltas:?}"
        );
    }
```

- [x] **Step 2: Run the tests to verify they fail**

Run: `cargo test --all-features --lib openai_chat::types::tests::decodes_streaming_`
Expected: FAIL — `decodes_streaming_text` and the others fail their assertions, because the stub decoder emits nothing.

- [x] **Step 3: Write the decoder**

In `src/provider/openai_chat/mod.rs`, replace the stub `Decoder` added in Task 7 with:

```rust
use crate::provider::sse::SseFrame;
use crate::provider::stream::{RawDelta, StreamDecoder};
use crate::provider::{ResponseStatus, Usage};

/// Decodes Chat Completions SSE frames.
///
/// Stateless: `id` and `name` arrive in the first frame of a call and are
/// forwarded as they come, so nothing needs remembering between frames.
#[derive(Default)]
pub(crate) struct Decoder;

impl StreamDecoder for Decoder {
    fn decode(
        &mut self,
        frame: &SseFrame,
        out: &mut Vec<RawDelta>,
    ) -> Result<(), crate::provider::ProviderError> {
        // The sentinel is not JSON and carries nothing.
        if frame.data.trim() == "[DONE]" {
            return Ok(());
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&frame.data) else {
            return Ok(());
        };

        let id = value["id"].as_str().map(str::to_string);
        let model = value["model"].as_str().map(str::to_string);
        let usage = value.get("usage").and_then(|usage| {
            Some(Usage {
                input_tokens: usage["prompt_tokens"].as_u64()?,
                output_tokens: usage["completion_tokens"].as_u64()?,
                total_tokens: usage["total_tokens"].as_u64()?,
            })
        });

        let choice = &value["choices"][0];
        let status = choice["finish_reason"].as_str().map(|reason| match reason {
            "stop" => ResponseStatus::Completed,
            "length" => ResponseStatus::Incomplete,
            "tool_calls" => ResponseStatus::RequiresAction,
            other => ResponseStatus::Other(other.to_string()),
        });

        if let Some(text) = choice["delta"]["content"].as_str()
            && !text.is_empty()
        {
            out.push(RawDelta::Text(text.to_string()));
        }

        if let Some(calls) = choice["delta"]["tool_calls"].as_array() {
            for call in calls {
                // This dialect's index counts tool calls only, unlike
                // Anthropic's, which counts content blocks.
                let slot = call["index"].as_u64().unwrap_or(0) as usize;
                if let Some(id) = call["id"].as_str() {
                    out.push(RawDelta::ToolStart {
                        slot,
                        id: id.to_string(),
                        name: call["function"]["name"].as_str().unwrap_or_default().to_string(),
                    });
                }
                if let Some(fragment) = call["function"]["arguments"].as_str()
                    && !fragment.is_empty()
                {
                    out.push(RawDelta::ToolArgs {
                        slot,
                        fragment: fragment.to_string(),
                    });
                }
            }
        }

        if id.is_some() || model.is_some() || status.is_some() || usage.is_some() {
            out.push(RawDelta::Meta {
                id,
                model,
                status,
                usage,
            });
        }
        Ok(())
    }
}
```

- [x] **Step 4: Run the tests to verify they pass**

Run: `cargo test --all-features --lib openai_chat::`
Expected: PASS, including the pre-existing tests in that module.

- [x] **Step 5: Commit**

```bash
git add src/provider/openai_chat
git commit -m "feat: decode OpenAiChat streaming frames

This dialect never signals the end of a tool call, so the decoder emits
no ToolEnd and relies on the assembler's flush-at-close. The test pins
that down so it does not read as an omission."
```

---

### Task 9: Anthropic decoder → verify: `cargo test --all-features anthropic::types::tests::decodes_streaming_` passes; `decodes_streaming_tool_call` asserts the tool call lands at `slot: 1` behind a text block at slot 0 and ends with `ToolEnd { slot: 1 }`

**Files:**
- Modify: `src/provider/anthropic/mod.rs`
- Modify: `src/provider/anthropic/types.rs`

- [x] **Step 1: Write the failing tests**

In `src/provider/anthropic/types.rs`, add to the `mod tests` block:

```rust
    use crate::provider::sse::SseFrame;
    use crate::provider::stream::{RawDelta, StreamDecoder};

    fn decode_all(frames: &[(&str, &str)]) -> Vec<RawDelta> {
        let mut decoder = crate::provider::anthropic::Decoder::default();
        let mut out = Vec::new();
        for (event, data) in frames {
            let frame = SseFrame {
                event: Some((*event).to_string()),
                data: (*data).to_string(),
            };
            decoder.decode(&frame, &mut out).expect("decodes");
        }
        out
    }

    #[test]
    fn decodes_streaming_text() {
        let deltas = decode_all(&[
            (
                "message_start",
                r#"{"message":{"id":"msg_1","model":"claude-sonnet-4","usage":{"input_tokens":11,"output_tokens":0}}}"#,
            ),
            (
                "content_block_start",
                r#"{"index":0,"content_block":{"type":"text","text":""}}"#,
            ),
            (
                "content_block_delta",
                r#"{"index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
            ),
        ]);

        assert!(
            deltas.iter().any(|d| *d == RawDelta::Text("Hello".into())),
            "{deltas:?}"
        );
    }

    #[test]
    fn decodes_streaming_tool_call() {
        let deltas = decode_all(&[
            (
                "content_block_start",
                r#"{"index":0,"content_block":{"type":"text","text":""}}"#,
            ),
            (
                "content_block_delta",
                r#"{"index":0,"delta":{"type":"text_delta","text":"Let me check."}}"#,
            ),
            ("content_block_stop", r#"{"index":0}"#),
            (
                "content_block_start",
                r#"{"index":1,"content_block":{"type":"tool_use","id":"toolu_01","name":"get_weather","input":{}}}"#,
            ),
            (
                "content_block_delta",
                r#"{"index":1,"delta":{"type":"input_json_delta","partial_json":"{\"loc"}}"#,
            ),
            (
                "content_block_delta",
                r#"{"index":1,"delta":{"type":"input_json_delta","partial_json":"ation\":\"NYC\"}"}}"#,
            ),
            ("content_block_stop", r#"{"index":1}"#),
        ]);

        assert_eq!(
            deltas,
            vec![
                RawDelta::Text("Let me check.".into()),
                RawDelta::ToolStart {
                    slot: 1,
                    id: "toolu_01".into(),
                    name: "get_weather".into(),
                },
                RawDelta::ToolArgs {
                    slot: 1,
                    fragment: "{\"loc".into(),
                },
                RawDelta::ToolArgs {
                    slot: 1,
                    fragment: "ation\":\"NYC\"}".into(),
                },
                RawDelta::ToolEnd { slot: 1 },
            ],
            "the index counts content blocks, so prose ahead of the call \
             pushes it to 1; stopping a text block must emit no ToolEnd"
        );
    }

    #[test]
    fn decodes_streaming_thinking_into_a_replayable_blob() {
        let deltas = decode_all(&[
            (
                "content_block_start",
                r#"{"index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}"#,
            ),
            (
                "content_block_delta",
                r#"{"index":0,"delta":{"type":"thinking_delta","thinking":"Let me work through it."}}"#,
            ),
            (
                "content_block_delta",
                r#"{"index":0,"delta":{"type":"signature_delta","signature":"abc123"}}"#,
            ),
            ("content_block_stop", r#"{"index":0}"#),
        ]);

        assert_eq!(
            deltas[0],
            RawDelta::ReasoningText("Let me work through it.".into())
        );
        assert_eq!(
            deltas[1],
            RawDelta::ReasoningBlob(serde_json::json!({
                "type": "thinking",
                "thinking": "Let me work through it.",
                "signature": "abc123",
            })),
            "the blob must be the whole reconstructed block, because that is \
             what the non-streaming parser produces and what must be replayed"
        );
    }

    #[test]
    fn decodes_streaming_error_frame() {
        let mut decoder = crate::provider::anthropic::Decoder::default();
        let mut out = Vec::new();
        let frame = SseFrame {
            event: Some("error".into()),
            data: r#"{"error":{"type":"overloaded_error","message":"Overloaded"}}"#.into(),
        };

        assert!(matches!(
            decoder.decode(&frame, &mut out),
            Err(ProviderError::Stream { .. })
        ));
    }
```

- [x] **Step 2: Run the tests to verify they fail**

Run: `cargo test --all-features --lib anthropic::types::tests::decodes_streaming_`
Expected: FAIL — the stub decoder emits nothing and returns `Ok`, so every assertion fails.

- [x] **Step 3: Write the decoder**

In `src/provider/anthropic/mod.rs`, replace the stub `Decoder` added in Task 7 with:

```rust
use crate::provider::sse::SseFrame;
use crate::provider::stream::{RawDelta, StreamDecoder};
use crate::provider::{ResponseStatus, Usage};
use serde_json::Value;
use std::collections::HashMap;

/// A thinking block being reassembled, so the replayable blob can be rebuilt
/// in the same shape the non-streaming parser produces.
#[derive(Default)]
struct PendingThinking {
    thinking: String,
    signature: String,
}

/// Decodes Messages API SSE frames.
///
/// Stateful: `content_block_stop` names only an index, so the decoder has to
/// remember which indices were tool calls and which were thinking blocks.
#[derive(Default)]
pub(crate) struct Decoder {
    tools: HashMap<usize, ()>,
    thinking: HashMap<usize, PendingThinking>,
    input_tokens: u64,
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
        // The event name is authoritative here; unlike the OpenAI dialects,
        // the payload does not repeat it.
        let event = frame.event.as_deref().unwrap_or_default();
        let index = value["index"].as_u64().unwrap_or(0) as usize;

        match event {
            "error" => {
                return Err(crate::provider::ProviderError::Stream {
                    provider: "anthropic".into(),
                    message: value["error"]["message"]
                        .as_str()
                        .unwrap_or("unknown streaming error")
                        .to_string(),
                });
            }
            "message_start" => {
                let message = &value["message"];
                self.input_tokens = message["usage"]["input_tokens"].as_u64().unwrap_or(0);
                out.push(RawDelta::Meta {
                    id: message["id"].as_str().map(str::to_string),
                    model: message["model"].as_str().map(str::to_string),
                    status: None,
                    usage: None,
                });
            }
            "content_block_start" => {
                let block = &value["content_block"];
                match block["type"].as_str() {
                    Some("tool_use") => {
                        self.tools.insert(index, ());
                        out.push(RawDelta::ToolStart {
                            slot: index,
                            id: block["id"].as_str().unwrap_or_default().to_string(),
                            name: block["name"].as_str().unwrap_or_default().to_string(),
                        });
                    }
                    Some("thinking") => {
                        self.thinking.insert(index, PendingThinking::default());
                    }
                    _ => {}
                }
            }
            "content_block_delta" => {
                let delta = &value["delta"];
                match delta["type"].as_str() {
                    Some("text_delta") => {
                        if let Some(text) = delta["text"].as_str() {
                            out.push(RawDelta::Text(text.to_string()));
                        }
                    }
                    Some("input_json_delta") => {
                        if let Some(fragment) = delta["partial_json"].as_str() {
                            out.push(RawDelta::ToolArgs {
                                slot: index,
                                fragment: fragment.to_string(),
                            });
                        }
                    }
                    Some("thinking_delta") => {
                        if let Some(text) = delta["thinking"].as_str() {
                            if let Some(pending) = self.thinking.get_mut(&index) {
                                pending.thinking.push_str(text);
                            }
                            out.push(RawDelta::ReasoningText(text.to_string()));
                        }
                    }
                    Some("signature_delta") => {
                        if let Some(signature) = delta["signature"].as_str()
                            && let Some(pending) = self.thinking.get_mut(&index)
                        {
                            pending.signature.push_str(signature);
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                if self.tools.remove(&index).is_some() {
                    out.push(RawDelta::ToolEnd { slot: index });
                } else if let Some(pending) = self.thinking.remove(&index) {
                    out.push(RawDelta::ReasoningBlob(serde_json::json!({
                        "type": "thinking",
                        "thinking": pending.thinking,
                        "signature": pending.signature,
                    })));
                }
            }
            "message_delta" => {
                let status = value["delta"]["stop_reason"].as_str().map(|reason| match reason {
                    "end_turn" | "stop_sequence" => ResponseStatus::Completed,
                    "max_tokens" => ResponseStatus::Incomplete,
                    "tool_use" => ResponseStatus::RequiresAction,
                    other => ResponseStatus::Other(other.to_string()),
                });
                let output_tokens = value["usage"]["output_tokens"].as_u64();
                out.push(RawDelta::Meta {
                    id: None,
                    model: None,
                    status,
                    // Input tokens arrive in message_start and output tokens
                    // here, so the total is only knowable at this point.
                    usage: output_tokens.map(|output_tokens| Usage {
                        input_tokens: self.input_tokens,
                        output_tokens,
                        total_tokens: self.input_tokens + output_tokens,
                    }),
                });
            }
            _ => {}
        }
        Ok(())
    }
}
```

- [x] **Step 4: Run the tests to verify they pass**

Run: `cargo test --all-features --lib anthropic::`
Expected: PASS, including the pre-existing tests in that module.

- [x] **Step 5: Commit**

```bash
git add src/provider/anthropic
git commit -m "feat: decode Anthropic streaming frames

content_block_stop names only an index, so the decoder tracks which
indices were tool calls and which were thinking blocks. Thinking blocks
are rebuilt whole rather than reduced to a signature, because that whole
block is what the provider requires replayed."
```

---

### Task 10: OpenAiResponses decoder → verify: `cargo test --all-features openai_responses::types::tests::decodes_streaming_` passes; `decodes_streaming_tool_call` asserts the terminal frame produces `ToolReplace` then `ToolEnd`, never a second `ToolArgs`

**Files:**
- Modify: `src/provider/openai_responses/mod.rs`
- Modify: `src/provider/openai_responses/types.rs`

- [x] **Step 1: Write the failing tests**

In `src/provider/openai_responses/types.rs`, add to the `mod tests` block:

```rust
    use crate::provider::sse::SseFrame;
    use crate::provider::stream::{RawDelta, StreamDecoder};

    fn decode_all(frames: &[(&str, &str)]) -> Vec<RawDelta> {
        let mut decoder = crate::provider::openai_responses::Decoder::default();
        let mut out = Vec::new();
        for (event, data) in frames {
            let frame = SseFrame {
                event: Some((*event).to_string()),
                data: (*data).to_string(),
            };
            decoder.decode(&frame, &mut out).expect("decodes");
        }
        out
    }

    #[test]
    fn decodes_streaming_text() {
        let deltas = decode_all(&[
            (
                "response.created",
                r#"{"response":{"id":"resp_1","model":"gpt-5","status":"in_progress"}}"#,
            ),
            (
                "response.output_text.delta",
                r#"{"item_id":"msg_1","output_index":0,"delta":"Hello"}"#,
            ),
        ]);

        assert!(
            deltas.iter().any(|d| *d == RawDelta::Text("Hello".into())),
            "{deltas:?}"
        );
    }

    #[test]
    fn decodes_streaming_tool_call() {
        let deltas = decode_all(&[
            (
                "response.output_item.added",
                r#"{"output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_abc","name":"get_weather","arguments":""}}"#,
            ),
            (
                "response.function_call_arguments.delta",
                r#"{"item_id":"fc_1","output_index":0,"delta":"{\"loc"}"#,
            ),
            (
                "response.function_call_arguments.done",
                r#"{"item_id":"fc_1","output_index":0,"arguments":"{\"location\":\"NYC\"}"}"#,
            ),
        ]);

        assert_eq!(
            deltas,
            vec![
                RawDelta::ToolStart {
                    slot: 0,
                    id: "call_abc".into(),
                    name: "get_weather".into(),
                },
                RawDelta::ToolArgs {
                    slot: 0,
                    fragment: "{\"loc".into(),
                },
                RawDelta::ToolReplace {
                    slot: 0,
                    arguments: "{\"location\":\"NYC\"}".into(),
                },
                RawDelta::ToolEnd { slot: 0 },
            ],
            "the done frame repeats the complete arguments, so it must replace \
             the buffer rather than append and double-count"
        );
    }

    #[test]
    fn decodes_streaming_completion() {
        let deltas = decode_all(&[(
            "response.completed",
            r#"{"response":{"id":"resp_1","model":"gpt-5","status":"completed","usage":{"input_tokens":11,"output_tokens":9,"total_tokens":20}}}"#,
        )]);

        assert_eq!(
            deltas,
            vec![RawDelta::Meta {
                id: Some("resp_1".into()),
                model: Some("gpt-5".into()),
                status: Some(ResponseStatus::Completed),
                usage: Some(Usage {
                    input_tokens: 11,
                    output_tokens: 9,
                    total_tokens: 20,
                }),
            }]
        );
    }
```

- [x] **Step 2: Run the tests to verify they fail**

Run: `cargo test --all-features --lib openai_responses::types::tests::decodes_streaming_`
Expected: FAIL — the stub emits nothing.

- [x] **Step 3: Write the decoder**

In `src/provider/openai_responses/mod.rs`, replace the stub `Decoder` added in Task 7 with:

```rust
use crate::provider::sse::SseFrame;
use crate::provider::stream::{RawDelta, StreamDecoder};
use crate::provider::{ResponseStatus, Usage};
use serde_json::Value;

/// Decodes Responses API SSE frames.
///
/// Stateless: every frame carries its own `output_index`, so no correlation
/// has to be remembered between frames.
#[derive(Default)]
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
                // Reasoning items must be replayed exactly as received, which
                // is what the non-streaming parser stores for them too.
                if item["type"] == "reasoning" {
                    out.push(RawDelta::ReasoningBlob(item.clone()));
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
                        Some("completed") => ResponseStatus::Completed,
                        Some("incomplete") => ResponseStatus::Incomplete,
                        Some("failed") => ResponseStatus::Failed,
                        Some(other) => ResponseStatus::Other(other.to_string()),
                        None => ResponseStatus::Completed,
                    }),
                    usage,
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
```

- [x] **Step 4: Run the tests to verify they pass**

Run: `cargo test --all-features --lib openai_responses::`
Expected: PASS, including the pre-existing tests in that module.

- [x] **Step 5: Commit**

```bash
git add src/provider/openai_responses
git commit -m "feat: decode OpenAiResponses streaming frames

The arguments.done frame repeats the complete arguments, so it maps to
ToolReplace rather than ToolArgs; appending it would double every set of
tool arguments this dialect produces."
```

---

### Task 11: Gemini decoder → verify: `cargo test --all-features gemini::types::tests::decodes_streaming_` passes; `decodes_streaming_tool_call` asserts `step.start` with `type: function_call` yields `ToolStart` and `step.stop` yields `ToolEnd`

**Files:**
- Modify: `src/provider/gemini/mod.rs`
- Modify: `src/provider/gemini/types.rs`

- [x] **Step 1: Write the failing tests**

In `src/provider/gemini/types.rs`, add to the `mod tests` block:

```rust
    use crate::provider::sse::SseFrame;
    use crate::provider::stream::{RawDelta, StreamDecoder};

    fn decode_all(frames: &[&str]) -> Vec<RawDelta> {
        let mut decoder = crate::provider::gemini::Decoder::default();
        let mut out = Vec::new();
        for data in frames {
            // The Interactions API repeats event_type inside the payload, so
            // the decoder reads it there rather than from the SSE event line.
            let frame = SseFrame {
                event: None,
                data: (*data).to_string(),
            };
            decoder.decode(&frame, &mut out).expect("decodes");
        }
        out
    }

    #[test]
    fn decodes_streaming_text() {
        let deltas = decode_all(&[
            r#"{"index":0,"step":{"type":"model_output"},"event_type":"step.start"}"#,
            r#"{"index":0,"delta":{"type":"text","text":"Hello"},"event_type":"step.delta"}"#,
        ]);

        assert!(
            deltas.iter().any(|d| *d == RawDelta::Text("Hello".into())),
            "{deltas:?}"
        );
    }

    #[test]
    fn decodes_streaming_tool_call() {
        let deltas = decode_all(&[
            r#"{"index":0,"step":{"type":"function_call","id":"un6k8t18","name":"get_weather","arguments":{}},"event_type":"step.start"}"#,
            r#"{"index":0,"delta":{"type":"arguments_delta","arguments":"{\"location\": "},"event_type":"step.delta"}"#,
            r#"{"index":0,"delta":{"type":"arguments_delta","arguments":"\"San Francisco, CA\"}"},"event_type":"step.delta"}"#,
            r#"{"index":0,"event_type":"step.stop"}"#,
        ]);

        assert_eq!(
            deltas,
            vec![
                RawDelta::ToolStart {
                    slot: 0,
                    id: "un6k8t18".into(),
                    name: "get_weather".into(),
                },
                RawDelta::ToolArgs {
                    slot: 0,
                    fragment: "{\"location\": ".into(),
                },
                RawDelta::ToolArgs {
                    slot: 0,
                    fragment: "\"San Francisco, CA\"}".into(),
                },
                RawDelta::ToolEnd { slot: 0 },
            ],
            "arguments_delta fragments accumulate; the docs require it"
        );
    }

    #[test]
    fn decodes_streaming_thought_into_a_replayable_blob() {
        let deltas = decode_all(&[
            r#"{"index":0,"step":{"type":"thought"},"event_type":"step.start"}"#,
            r#"{"index":0,"delta":{"type":"thought_summary","content":{"type":"text","text":"Working it out."}},"event_type":"step.delta"}"#,
            r#"{"index":0,"delta":{"type":"thought_signature","signature":"sig-abc"},"event_type":"step.delta"}"#,
            r#"{"index":0,"event_type":"step.stop"}"#,
        ]);

        assert_eq!(
            deltas[0],
            RawDelta::ReasoningText("Working it out.".into())
        );
        assert_eq!(
            deltas[1],
            RawDelta::ReasoningBlob(serde_json::json!({
                "type": "thought",
                "signature": "sig-abc",
            })),
            "the signature is the part the API requires resent verbatim"
        );
    }

    #[test]
    fn decodes_streaming_completion() {
        let deltas = decode_all(&[
            r#"{"interaction":{"id":"v1_abc123","model":"gemini-3.6-flash","status":"completed","usage":{"total_tokens":346,"total_input_tokens":11,"total_output_tokens":90}},"event_type":"interaction.completed"}"#,
        ]);

        assert_eq!(
            deltas,
            vec![RawDelta::Meta {
                id: Some("v1_abc123".into()),
                model: Some("gemini-3.6-flash".into()),
                status: Some(ResponseStatus::Completed),
                usage: Some(Usage {
                    input_tokens: 11,
                    output_tokens: 90,
                    total_tokens: 346,
                }),
            }]
        );
    }
```

- [x] **Step 2: Run the tests to verify they fail**

Run: `cargo test --all-features --lib gemini::types::tests::decodes_streaming_`
Expected: FAIL — the stub emits nothing.

- [x] **Step 3: Write the decoder**

In `src/provider/gemini/mod.rs`, replace the stub `Decoder` added in Task 7 with:

```rust
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
    Thought { signature: String },
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
                                signature: String::new(),
                            },
                        );
                    }
                    _ => {}
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
                            && let Some(Step::Thought { signature }) = self.steps.get_mut(&slot)
                        {
                            signature.push_str(value);
                        }
                    }
                    _ => {}
                }
            }
            "step.stop" => match self.steps.remove(&slot) {
                Some(Step::Tool) => out.push(RawDelta::ToolEnd { slot }),
                Some(Step::Thought { signature }) => {
                    out.push(RawDelta::ReasoningBlob(serde_json::json!({
                        "type": "thought",
                        "signature": signature,
                    })));
                }
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
```

- [x] **Step 4: Run the tests to verify they pass**

Run: `cargo test --all-features --lib gemini::`
Expected: PASS, including the pre-existing tests in that module.

- [x] **Step 5: Commit**

```bash
git add src/provider/gemini
git commit -m "feat: decode Gemini streaming frames

Resolves the spec's open question: the Interactions API streams SSE with
step.start / step.delta / step.stop and repeats event_type inside each
payload, so the SSE event line is redundant here."
```

<!-- /parallel-group -->

---

### Task 12: Public exports and crate docs → verify: `cargo test --doc` passes and `cargo doc --no-deps --all-features` emits no warnings

**Files:**
- Modify: `src/lib.rs:33-58`, `src/lib.rs:64-68`

- [x] **Step 1: Re-export the streaming types**

In `src/lib.rs`, replace lines 64-68:

```rust
pub use provider::{
    Auth, Client, GenerateRequest, GenerateResponse, InputContent, Message, OutputContent,
    Provider, ProviderConfig, ProviderDialect, ProviderError, ProviderType, ReasoningEffort,
    ResponseFormat, ResponseStatus, Role, ToolChoice, ToolDefinition, Usage,
};
```

with:

```rust
pub use provider::{
    Auth, Client, EventStream, GenerateRequest, GenerateResponse, InputContent, Message,
    OutputContent, Provider, ProviderConfig, ProviderDialect, ProviderError, ProviderType,
    ReasoningEffort, ResponseFormat, ResponseStatus, Role, StreamEvent, ToolChoice,
    ToolDefinition, Usage,
};
```

- [x] **Step 2: Document streaming in the crate docs**

In `src/lib.rs`, insert this section immediately before the `#![deny(missing_docs)]` line, after the tool-calling section that ends at line 58:

```rust
//! # Streaming
//!
//! [`Client::stream`] returns the same answer incrementally. Fragments are not
//! exposed: tool-call arguments and reasoning blobs are assembled internally and
//! surface only once complete, so no caller ever stitches partial JSON.
//!
//! ```no_run
//! # async fn run(client: freyja::Client, request: freyja::GenerateRequest)
//! #     -> Result<(), freyja::ProviderError> {
//! use freyja::StreamEvent;
//!
//! let mut stream = client.stream(&request).await?;
//! while let Some(event) = stream.next().await? {
//!     match event {
//!         StreamEvent::TextDelta(text) => print!("{text}"),
//!         StreamEvent::ToolCall { name, arguments, .. } => {
//!             println!("\ncalling {name} with {arguments}");
//!         }
//!         _ => {}
//!     }
//! }
//!
//! // Drained streams convert back to the non-streaming response, so a tool
//! // loop can reuse `to_message` unchanged.
//! let response = stream.into_response()?;
//! # Ok(())
//! # }
//! ```
```

- [x] **Step 3: Run the doc tests**

Run: `cargo test --doc`
Expected: PASS.

- [x] **Step 4: Check the docs build clean**

Run: `cargo doc --no-deps --all-features`
Expected: success with no warnings. `#![deny(missing_docs)]` means any undocumented public item fails the build instead.

- [x] **Step 5: Commit**

```bash
git add src/lib.rs
git commit -m "docs: export the streaming types and document the loop"
```

---

### Task 13: Runnable example → verify: `cargo build --all-targets` builds `examples/streaming.rs`, and `cargo clippy --all-targets --all-features -- -D warnings` is clean

**Files:**
- Create: `examples/streaming.rs`

- [x] **Step 1: Read an existing example for house style**

Run: `cat examples/simple.rs`
Expected: prints the file. Match its structure — `dotenvy` load, `Client::from_env`, `#[tokio::main]`.

- [x] **Step 2: Write the example**

Create `examples/streaming.rs`:

```rust
//! Prints a model's answer as it arrives.
//!
//! Run with: `cargo run --example streaming`

use freyja::{Client, GenerateRequest, Message, ProviderType, Role, StreamEvent};
use std::io::Write;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    let client = Client::from_env(ProviderType::OpenAi).ok_or("OPENAI_API_KEY is unset")?;
    let request = GenerateRequest::new()
        .message(Message::text(Role::User, "Name three primary colors."))
        .max_tokens(128);

    let mut stream = client.stream(&request).await?;
    while let Some(event) = stream.next().await? {
        match event {
            StreamEvent::TextDelta(text) => {
                print!("{text}");
                // Deltas arrive mid-line, so nothing appears without a flush.
                std::io::stdout().flush()?;
            }
            StreamEvent::ToolCall {
                name, arguments, ..
            } => println!("\n[tool] {name}({arguments})"),
            StreamEvent::Done { usage, .. } => {
                if let Some(usage) = usage {
                    println!("\n\n[{} tokens]", usage.total_tokens);
                }
            }
            _ => {}
        }
    }

    Ok(())
}
```

- [x] **Step 3: Build it**

Run: `cargo build --all-targets`
Expected: success.

- [x] **Step 4: Lint**

Run: `cargo clippy --all-targets --all-features -- -D warnings`
Expected: no warnings.

- [x] **Step 5: Commit**

```bash
git add examples/streaming.rs
git commit -m "docs: add a streaming example"
```

---

### Task 14: Full verification and roadmap update → verify: `cargo fmt --all --check`, `cargo test --all-features`, `cargo test --doc`, and `cargo clippy --all-targets --all-features -- -D warnings` all pass; `README.md:88` no longer lists streaming as remaining

**Files:**
- Modify: `README.md:88`

- [x] **Step 1: Confirm every dialect is reachable**

Run: `grep -c "::Decoder" src/provider/mod.rs`
Expected: `4` — one per dialect in `Client::stream`'s match. A lower number means a
dialect is still dispatching to nothing.

Run: `cargo test --all-features --lib decodes_streaming_`
Expected: `14 passed` — three for OpenAiChat, four for Anthropic, three for
OpenAiResponses, four for Gemini. Every one of those tests asserts on an emitted
`RawDelta` sequence, so a decoder still returning the Task 7 stub's bare `Ok(())`
would fail rather than pass. Confirm the count is 14 and not 0; a filter that
matches nothing also exits 0.

Run: `grep -rn "pub(crate) struct Decoder" src/provider/*/mod.rs`
Expected: four matches, one per dialect module. Two are unit structs
(`Decoder;` — OpenAiChat and OpenAiResponses are stateless) and two carry state
(`Decoder {` — Anthropic tracks which content-block indices are tool calls vs
thinking blocks, Gemini tracks the same for step indices). That asymmetry is
correct, not a defect.

- [x] **Step 2: Update the roadmap**

In `README.md`, replace line 88:

```markdown
**Phase 1, production-grade provider layer.** Four dialects and the dialect/endpoint split are done. Remaining: streaming, retries with backoff, typed API errors, capability introspection, and derive-based structured output.
```

with:

```markdown
**Phase 1, production-grade provider layer.** Four dialects, the dialect/endpoint split, and streaming are done. Remaining: retries with backoff, typed API errors, capability introspection, and derive-based structured output.
```

- [x] **Step 3: Run formatting**

Run: `cargo fmt --all`
Expected: no output. Then run `cargo fmt --all --check` and expect no output and exit code 0.

- [x] **Step 4: Run the full suite**

Run: `cargo test --all-features`
Expected: PASS, all tests.

Run: `cargo test --doc`
Expected: PASS.

- [x] **Step 5: Run the linter**

Run: `cargo clippy --all-targets --all-features -- -D warnings`
Expected: no warnings.

- [x] **Step 6: Confirm the package still publishes** — *note: this step must run
  AFTER Step 7's commit. `cargo publish --dry-run` refuses a dirty working tree,
  and Steps 2-3 leave the README edit and rustfmt changes uncommitted. Commit
  first, then run this; do not reach for `--allow-dirty`.*

Run: `cargo publish --dry-run`
Expected: success. This is the check CI runs; it catches bad metadata and broken intra-doc links.

- [x] **Step 7: Commit**

```bash
git add README.md src
git commit -m "docs: mark streaming done in the roadmap"
```

---

## Self-review notes

Checked against the spec:

- Every spec Goal maps to a task: `client.stream` (7), neutral `StreamEvent` (3), all four dialects (8-11), zero new dependencies (no task touches `Cargo.toml`), complete tool arguments (4-5), reasoning replay (9, 10, 11), `into_response` (5, 6).
- Every file in the File structure section is touched by at least one task, and no task touches a file absent from it.
- Type names are consistent across tasks: `SseFrame`, `SseBuffer`, `RawDelta`, `StreamDecoder`, `Assembler`, `EventStream`, `StreamEvent`, `Decoder`.
- Task 5 deliberately contains no implementation — the assembler written in Task 4 already satisfies it. Step 3 says so explicitly rather than leaving the executing agent to invent a change.
- Predicted failure modes: Tasks 1, 2, 3, 6, 7 predict compilation failures with named missing symbols, which follow with certainty from the code not existing yet. Tasks 8-11 predict assertion failures rather than naming exact messages, because the stub returns `Ok(())` and the specific assertion that trips first is not worth guessing.
- `cargo` commands were taken from `.github/workflows/*.yml` and match what CI runs.
- Two MSRV-sensitive constructs appear in the planned code: let-chains (`if let ... && let ...`, Tasks 8 and 11) and `std::task::Waker::noop()` (Task 6). Both were compiled against `rustc 1.88.0` with `--edition 2024` before this plan was finalized and both succeed, so `cargo +1.88 check --all-targets` in CI will not trip on them.

---

# Follow-up: closing the two verification blockers

> Added after the verification run at commit `1bde629` returned **not ready**.
> See `docs/verifications/2026-08-09-streaming-api-verify.md` for the evidence.

### Task 15: Preserve unrecognised reasoning blocks → verify: `cargo test --all-features --lib preserves_unrecognised` passes with 2 tests; the Anthropic test asserts a `redacted_thinking` block reaches `RawDelta::ReasoningBlob` with all its fields intact

**Files:**
- Modify: `src/provider/anthropic/mod.rs`
- Modify: `src/provider/anthropic/types.rs`
- Modify: `src/provider/gemini/mod.rs`
- Modify: `src/provider/gemini/types.rs`

The non-streaming parsers end with a catch-all — `anthropic/types.rs:387` and
`gemini/types.rs:334` both read `_ => vec![OutputContent::Reasoning { data: block }]`
— so any block type Freyja does not model survives verbatim. The streaming
decoders instead match only the types they name and drop the rest, which loses
`redacted_thinking` and anything Anthropic or Google adds later. The fix is to
mirror the parsers: remember the whole block at start, emit it whole at stop.

- [x] **Step 1: Write the failing Anthropic test**

In `src/provider/anthropic/types.rs`, add to `mod tests`:

```rust
    #[test]
    fn preserves_unrecognised_blocks_when_streaming() {
        let deltas = decode_all(&[
            (
                "content_block_start",
                r#"{"index":0,"content_block":{"type":"redacted_thinking","data":"EncryptedPayload=="}}"#,
            ),
            ("content_block_stop", r#"{"index":0}"#),
        ]);

        assert_eq!(
            deltas,
            vec![RawDelta::ReasoningBlob(serde_json::json!({
                "type": "redacted_thinking",
                "data": "EncryptedPayload==",
            }))],
            "the non-streaming parser preserves any unmodeled block verbatim; \
             streaming must not silently drop one, or a replayed transcript \
             is incomplete and the provider rejects the next turn"
        );
    }
```

- [x] **Step 2: Run it and confirm it fails**

Run: `cargo test --all-features --lib preserves_unrecognised_blocks_when_streaming`
Expected: FAIL. The assertion reports `left: []` because the decoder's
`_ => {}` arm discards the block.

- [x] **Step 3: Make the Anthropic decoder preserve unknown blocks**

In `src/provider/anthropic/mod.rs`, extend the decoder's state to remember whole
blocks. Change the `Decoder` struct to add a field alongside `tools` and `thinking`:

```rust
    /// Blocks whose type this decoder does not model, kept whole so
    /// `content_block_stop` can emit them exactly as the parser would.
    opaque: HashMap<usize, Value>,
```

In `content_block_start`, replace the `_ => {}` arm with:

```rust
                    _ => {
                        self.opaque.insert(index, block.clone());
                    }
```

In `content_block_stop`, add a final branch after the `thinking` branch:

```rust
                } else if let Some(block) = self.opaque.remove(&index) {
                    out.push(RawDelta::ReasoningBlob(block));
                }
```

- [x] **Step 4: Confirm the Anthropic test passes**

Run: `cargo test --all-features --lib preserves_unrecognised_blocks_when_streaming`
Expected: PASS, 1 test.

- [x] **Step 5: Write the failing Gemini test**

In `src/provider/gemini/types.rs`, add to `mod tests`:

```rust
    #[test]
    fn preserves_unrecognised_steps_when_streaming() {
        let deltas = decode_all(&[
            r#"{"index":0,"step":{"type":"safety_report","verdict":"ok"},"event_type":"step.start"}"#,
            r#"{"index":0,"event_type":"step.stop"}"#,
        ]);

        assert_eq!(
            deltas,
            vec![RawDelta::ReasoningBlob(serde_json::json!({
                "type": "safety_report",
                "verdict": "ok",
            }))],
            "the non-streaming parser preserves any unmodeled step verbatim"
        );
    }
```

- [x] **Step 6: Run it and confirm it fails**

Run: `cargo test --all-features --lib preserves_unrecognised_steps_when_streaming`
Expected: FAIL with `left: []`.

- [x] **Step 7: Make the Gemini decoder preserve unknown steps and whole thoughts**

In `src/provider/gemini/mod.rs`, change the `Step` enum so a thought carries the
original step object rather than only its signature:

```rust
enum Step {
    Tool,
    /// The step as it arrived, plus the signature accumulated from its deltas.
    /// Kept whole because the non-streaming parser stores the whole step and
    /// the API requires model-generated steps replayed exactly as received.
    Thought { step: Value, signature: String },
    /// A step type this decoder does not model, kept verbatim.
    Opaque(Value),
}
```

In `step.start`, the `Some("thought")` arm becomes:

```rust
                    Some("thought") => {
                        self.steps.insert(
                            slot,
                            Step::Thought {
                                step: step.clone(),
                                signature: String::new(),
                            },
                        );
                    }
```

and the `_ => {}` arm becomes:

```rust
                    _ => {
                        self.steps.insert(slot, Step::Opaque(step.clone()));
                    }
```

In the `thought_signature` delta arm, update the pattern to the new shape:

```rust
                        if let Some(value) = delta["signature"].as_str()
                            && let Some(Step::Thought { signature, .. }) = self.steps.get_mut(&slot)
                        {
                            signature.push_str(value);
                        }
```

In `step.stop`, replace the match with:

```rust
            "step.stop" => match self.steps.remove(&slot) {
                Some(Step::Tool) => out.push(RawDelta::ToolEnd { slot }),
                Some(Step::Thought { mut step, signature }) => {
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
```

- [x] **Step 8: Update the existing Gemini thought test for the new blob shape**

The blob is now the whole step with the signature merged in, not a synthesised
`{type,signature}` pair. In `src/provider/gemini/types.rs`, the expected value in
`decodes_streaming_thought_into_a_replayable_blob` becomes:

```rust
            RawDelta::ReasoningBlob(serde_json::json!({
                "type": "thought",
                "signature": "sig-abc",
            })),
```

which is unchanged *only if* the fixture's `step.start` carried no other fields.
Read the fixture; if `step` has more fields, include them. Do not weaken the
assertion to make it pass — the point is that the whole step survives.

- [x] **Step 9: Confirm both new tests and the whole suite pass**

Run: `cargo test --all-features --lib preserves_unrecognised`
Expected: PASS, 2 tests. Confirm the count is 2 and not 0.

Run: `cargo test --all-features`
Expected: PASS, all tests.

- [x] **Step 10: Commit**

```bash
git add src/provider/anthropic src/provider/gemini
git commit -m "fix: preserve unrecognised reasoning blocks when streaming

The non-streaming parsers keep any block they do not model as a verbatim
replayable blob. The streaming decoders matched only the types they named
and dropped the rest, so an Anthropic redacted_thinking block survived
generate() and vanished through stream() -- producing an incomplete
transcript that the provider rejects on the next turn."
```

---

### Task 16: Capture provider metadata and prove response parity → verify: `cargo test --all-features --lib streamed_response_matches_generate` passes with 1 test, asserting a drained stream and the non-streaming parser produce equal id, model, status, content, and usage

**Files:**
- Modify: `src/provider/stream.rs`
- Modify: `src/provider/anthropic/mod.rs`
- Modify: `src/provider/anthropic/types.rs`

`into_response` sets `provider_metadata: None` where every parser sets
`Some(Value::Object(extra))`, and no test compares the two paths. Note the limit
up front: a parser's `extra` is a `#[serde(flatten)]` map of fields Freyja does
*not* model, whereas a stream's terminal frame carries the provider object whole.
The two are not byte-identical and this task does not pretend otherwise. What it
delivers is (a) metadata that is actually populated rather than always `None`,
and (b) a test proving the fields a tool loop depends on — id, model, status,
content in order, usage — are equal across both paths.

- [x] **Step 1: Add the field to the internal delta and the assembler**

In `src/provider/stream.rs`, add to `RawDelta::Meta`'s field list:

```rust
        provider_metadata: Option<Value>,
```

Add to `Assembler`:

```rust
    provider_metadata: Option<Value>,
```

initialised to `None` in `Assembler::new`. In `absorb`'s `RawDelta::Meta` arm,
add alongside the other field updates:

```rust
                if provider_metadata.is_some() {
                    self.provider_metadata = provider_metadata;
                }
```

and in `into_response` replace `provider_metadata: None` with:

```rust
            provider_metadata: self.provider_metadata,
```

Every existing `RawDelta::Meta { .. }` construction across the four decoders and
the test modules must gain `provider_metadata: None`; the compiler will list them.

- [x] **Step 2: Populate it in the Anthropic decoder**

In `src/provider/anthropic/mod.rs`, the `message_start` arm carries the provider's
own message object. Change its `RawDelta::Meta` to pass it through:

```rust
                    provider_metadata: Some(message.clone()),
```

Leave the other three decoders passing `None` for now; Anthropic is the one this
task's parity test exercises, and a partial rollout is visible rather than a
silent claim of completeness.

- [x] **Step 3: Expose a drain helper for cross-module tests**

In `src/provider/stream.rs`, add next to the other `#[cfg(test)]` helpers:

```rust
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
```

Note it must sit outside the `impl EventStream` block or be an associated
function on it — either is fine, but it must be reachable as
`crate::provider::stream::drain_for_test` or `EventStream::drain_for_test`.

- [x] **Step 4: Write the failing parity test**

In `src/provider/anthropic/types.rs`, add to `mod tests`:

```rust
    #[test]
    fn streamed_response_matches_generate() {
        // The same logical answer, expressed both ways.
        let streamed = crate::provider::stream::drain_for_test(
            "anthropic".into(),
            Box::new(crate::provider::anthropic::Decoder::default()),
            vec![
                b"event: message_start\ndata: {\"message\":{\"id\":\"msg_1\",\"model\":\"claude-sonnet-4\",\"usage\":{\"input_tokens\":11,\"output_tokens\":0}}}\n\n".to_vec(),
                b"event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n".to_vec(),
                b"event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n".to_vec(),
                b"event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n".to_vec(),
                b"event: content_block_stop\ndata: {\"index\":0}\n\n".to_vec(),
                b"event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":9}}\n\n".to_vec(),
            ],
        )
        .expect("drained");

        let config = ProviderConfig::new(ProviderDialect::Anthropic, "anthropic", "https://x.test/v1");
        let parsed = parse(
            r#"{"id":"msg_1","model":"claude-sonnet-4","stop_reason":"end_turn","content":[{"type":"text","text":"Hello"}],"usage":{"input_tokens":11,"output_tokens":9}}"#,
            &config,
        )
        .expect("parsed");

        assert_eq!(streamed.id, parsed.id);
        assert_eq!(streamed.model, parsed.model);
        assert_eq!(streamed.status, parsed.status);
        assert_eq!(streamed.usage, parsed.usage);
        assert_eq!(
            streamed.content, parsed.content,
            "content must match part for part, including that two text deltas \
             coalesce into the single OutputContent::Text the parser produces"
        );
        assert_eq!(streamed.output_text(), "Hello");
        assert!(
            streamed.provider_metadata.is_some(),
            "metadata must be populated, not silently dropped as it was before"
        );
    }
```

- [x] **Step 5: Run it and confirm it fails, then passes**

Run: `cargo test --all-features --lib streamed_response_matches_generate`
Expected before Steps 1-3 are complete: FAIL to compile. After: PASS, 1 test.
If the content assertion fails, do NOT weaken it — a mismatch means the
coalescing or ordering is genuinely wrong and is the bug this task exists to find.

- [x] **Step 6: Run the whole suite and the linter**

Run: `cargo test --all-features`
Expected: PASS, all tests.

Run: `cargo clippy --all-targets --all-features -- -D warnings`
Expected: no warnings.

Run: `cargo fmt --all` then `cargo fmt --all --check`
Expected: the check produces no output.

- [x] **Step 7: Document the remaining limit**

On `EventStream::into_response` in `src/provider/stream.rs`, add to the doc comment:

```rust
    /// `provider_metadata` carries the provider's own terminal-frame object
    /// where the dialect supplies one. It is not byte-identical to the
    /// non-streaming path's value: `generate()` collects the fields Freyja does
    /// not model, while a stream carries the object whole. Every field a tool
    /// loop depends on — id, model, status, content, usage — does match.
```

- [x] **Step 8: Commit**

```bash
git add src/provider/stream.rs src/provider/anthropic
git commit -m "fix: populate provider_metadata and test streaming parity

into_response hardcoded provider_metadata to None while every parser set
Some(extra), so the spec's claim that a drained stream returns the same
GenerateResponse was false. Adds the field to the assembler, populates it
from Anthropic's message_start, and adds the parity test the plan's
Testing section promised but never delivered."
```

---

### Task 17: Close the residual parity gaps → verify: `cargo test --all-features --lib preserves_unrecognised` passes with 3 tests (one per dialect that has a parser catch-all), and `streamed_response_matches_generate` asserts equality of content containing text, a tool call, AND a reasoning blob

**Files:**
- Modify: `src/provider/openai_responses/mod.rs`
- Modify: `src/provider/openai_responses/types.rs`
- Modify: `src/provider/anthropic/mod.rs`
- Modify: `src/provider/anthropic/types.rs`
- Modify: `src/provider/gemini/mod.rs`
- Modify: `src/provider/openai_chat/mod.rs`

Re-verification after Tasks 15-16 found three residual gaps. Each is the same
asymmetry as before, in a place the earlier fix did not reach.

**Gap A — OpenAiResponses was never fixed.** `convert_item`
(`openai_responses/types.rs`) models exactly `message` and `function_call`, and
catch-alls everything else into a replayable blob. The decoder
(`openai_responses/mod.rs:80`) emits a blob only when `item["type"] ==
"reasoning"`, so `web_search_call`, `mcp_call`, `code_interpreter_call` and
anything OpenAI adds later are dropped when streaming and kept when not.

**Gap B — opaque blobs are snapshotted too early.** Anthropic's decoder stores an
unmodeled block at `content_block_start`, when its `input` is still empty, and
emits that snapshot at `content_block_stop`. A block filled by
`input_json_delta` — `server_tool_use` is the live example — therefore replays
with empty input, where the parser would have the finished object.

**Gap C — `provider_metadata` is Anthropic-only.** The other three dialects still
pass `None`, so the CR6 claim holds for one dialect out of four.

- [x] **Step 1: Write the failing OpenAiResponses test**

In `src/provider/openai_responses/types.rs`, add to `mod tests`:

```rust
    #[test]
    fn preserves_unrecognised_items_when_streaming() {
        let deltas = decode_all(&[(
            "response.output_item.done",
            r#"{"output_index":0,"item":{"type":"web_search_call","id":"ws_1","status":"completed"}}"#,
        )]);

        assert_eq!(
            deltas,
            vec![RawDelta::ReasoningBlob(serde_json::json!({
                "type": "web_search_call",
                "id": "ws_1",
                "status": "completed",
            }))],
            "convert_item catch-alls every item that is not message or \
             function_call; streaming must preserve the same set or a replayed \
             transcript loses items the provider expects back"
        );
    }
```

- [x] **Step 2: Run it, confirm it fails**

Run: `cargo test --all-features --lib preserves_unrecognised_items_when_streaming`
Expected: FAIL with `left: []`.

- [x] **Step 3: Mirror the parser in the Responses decoder**

In `src/provider/openai_responses/mod.rs`, replace the `response.output_item.done`
arm's body with a check that mirrors `convert_item`'s modeled set:

```rust
            "response.output_item.done" => {
                let item = &value["item"];
                // convert_item models exactly `message` and `function_call`;
                // everything else it preserves whole, so streaming must too.
                match item["type"].as_str() {
                    Some("message") | Some("function_call") => {}
                    _ => out.push(RawDelta::ReasoningBlob(item.clone())),
                }
            }
```

- [x] **Step 4: Confirm it passes**

Run: `cargo test --all-features --lib preserves_unrecognised`
Expected: PASS, 3 tests (anthropic, gemini, openai_responses). Confirm the count
is 3, not 0 and not 2.

- [x] **Step 5: Write the failing test for the early-snapshot gap**

In `src/provider/anthropic/types.rs`, add to `mod tests`:

```rust
    #[test]
    fn opaque_blocks_capture_their_streamed_input() {
        let deltas = decode_all(&[
            (
                "content_block_start",
                r#"{"index":0,"content_block":{"type":"server_tool_use","id":"srvtoolu_1","name":"web_search","input":{}}}"#,
            ),
            (
                "content_block_delta",
                r#"{"index":0,"delta":{"type":"input_json_delta","partial_json":"{\"query\":"}}"#,
            ),
            (
                "content_block_delta",
                r#"{"index":0,"delta":{"type":"input_json_delta","partial_json":"\"rust\"}"}}"#,
            ),
            ("content_block_stop", r#"{"index":0}"#),
        ]);

        assert_eq!(
            deltas,
            vec![RawDelta::ReasoningBlob(serde_json::json!({
                "type": "server_tool_use",
                "id": "srvtoolu_1",
                "name": "web_search",
                "input": {"query": "rust"},
            }))],
            "an unmodeled block whose input arrives as deltas must replay with \
             that input filled in; the start frame's empty object is not what \
             the non-streaming parser would have stored"
        );
    }
```

- [x] **Step 6: Run it, confirm it fails**

Run: `cargo test --all-features --lib opaque_blocks_capture_their_streamed_input`
Expected: FAIL — the emitted blob carries `"input": {}` because it was snapshotted
at start.

- [x] **Step 7: Accumulate deltas into opaque blocks**

In `src/provider/anthropic/mod.rs`, change the `opaque` map to hold the block plus
an argument buffer. Replace the field declaration with:

```rust
    /// Blocks whose type this decoder does not model, kept whole so
    /// `content_block_stop` can emit them exactly as the parser would, together
    /// with any `input_json_delta` text that arrives after the start frame.
    opaque: HashMap<usize, (Value, String)>,
```

In `content_block_start`'s catch-all arm:

```rust
                    _ => {
                        self.opaque.insert(index, (block.clone(), String::new()));
                    }
```

In `content_block_delta`'s `input_json_delta` arm, accumulate for opaque blocks as
well as emitting `ToolArgs` for modeled ones:

```rust
                    Some("input_json_delta") => {
                        if let Some(fragment) = delta["partial_json"].as_str() {
                            if let Some((_, buffer)) = self.opaque.get_mut(&index) {
                                buffer.push_str(fragment);
                            } else {
                                out.push(RawDelta::ToolArgs {
                                    slot: index,
                                    fragment: fragment.to_string(),
                                });
                            }
                        }
                    }
```

In `content_block_stop`'s opaque branch, merge the buffer back in:

```rust
                } else if let Some((mut block, buffer)) = self.opaque.remove(&index) {
                    // Replace the start frame's empty placeholder with what
                    // actually streamed, so the blob matches the parser's.
                    if !buffer.is_empty()
                        && let Ok(input) = serde_json::from_str::<Value>(&buffer)
                        && let Some(object) = block.as_object_mut()
                    {
                        object.insert("input".into(), input);
                    }
                    out.push(RawDelta::ReasoningBlob(block));
                }
```

- [x] **Step 8: Confirm it passes**

Run: `cargo test --all-features --lib opaque_blocks_capture_their_streamed_input`
Expected: PASS, 1 test.

- [x] **Step 9: Populate provider_metadata in the other three dialects**

`src/provider/openai_responses/mod.rs`, in the `response.completed`/`.incomplete`/
`.failed` arm's `RawDelta::Meta`:

```rust
                    provider_metadata: Some(response.clone()),
```

`src/provider/gemini/mod.rs`, in the `interaction.completed`/`.failed`/
`.incomplete` arm's `RawDelta::Meta`:

```rust
                    provider_metadata: Some(interaction.clone()),
```

`src/provider/openai_chat/mod.rs`, in its `RawDelta::Meta`:

```rust
                provider_metadata: Some(value.clone()),
```

Chat emits `Meta` on many chunks and the assembler takes the last non-`None`, so
this lands the final chunk's object — the one carrying usage and finish reason.

- [x] **Step 10: Extend the parity test to cover a tool call and a reasoning blob**

In `src/provider/anthropic/types.rs`, replace the fixtures inside
`streamed_response_matches_generate` so both sides carry text, a thinking block,
and a tool call. The SSE side gains, after the existing text block's stop:

```rust
                b"event: content_block_start\ndata: {\"index\":1,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n\n".to_vec(),
                b"event: content_block_delta\ndata: {\"index\":1,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Considering.\"}}\n\n".to_vec(),
                b"event: content_block_delta\ndata: {\"index\":1,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig-1\"}}\n\n".to_vec(),
                b"event: content_block_stop\ndata: {\"index\":1}\n\n".to_vec(),
                b"event: content_block_start\ndata: {\"index\":2,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"get_weather\",\"input\":{}}}\n\n".to_vec(),
                b"event: content_block_delta\ndata: {\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"city\\\":\\\"NYC\\\"}\"}}\n\n".to_vec(),
                b"event: content_block_stop\ndata: {\"index\":2}\n\n".to_vec(),
```

and the non-streaming body's `content` array becomes:

```json
[{"type":"text","text":"Hello"},
 {"type":"thinking","thinking":"Considering.","signature":"sig-1"},
 {"type":"tool_use","id":"toolu_1","name":"get_weather","input":{"city":"NYC"}}]
```

with `"stop_reason":"tool_use"` so both sides report `RequiresAction`.

Keep every existing assertion and add:

```rust
        assert_eq!(
            streamed.content.len(),
            3,
            "text, reasoning blob, and tool call must all survive the stream"
        );
        assert!(streamed.has_tool_calls());
        assert_eq!(
            streamed.to_message(),
            parsed.to_message(),
            "the assistant turn replayed into the next request must be identical, \
             which is the whole point of into_response"
        );
```

If `content` or `to_message` differ, DO NOT weaken the assertion — report the
verbatim difference. Note the tool-call `arguments` string is produced by
`Value::to_string()` on the parser side and by concatenated fragments on the
streaming side; if they differ only by whitespace, that is a real finding worth
reporting, not something to paper over.

- [x] **Step 11: Run everything**

Run: `cargo test --all-features --lib streamed_response_matches_generate`
Expected: PASS, 1 test.

Run: `cargo test --all-features`
Expected: PASS, all tests.

Run: `cargo clippy --all-targets --all-features -- -D warnings`
Expected: no warnings.

Run: `cargo fmt --all` then `cargo fmt --all --check`
Expected: no output.

- [x] **Step 12: Update the into_response doc to match reality**

In `src/provider/stream.rs`, the doc comment currently hedges with "where the
dialect supplies one". All four now supply one, so replace that clause:

```rust
    /// `provider_metadata` carries the provider's own terminal object. It is not
    /// byte-identical to the non-streaming path's value: `generate()` collects
    /// the fields Freyja does not model, while a stream carries the object
    /// whole. Every field a tool loop depends on — id, model, status, content,
    /// usage — does match, and `to_message()` produces the same assistant turn.
```

- [x] **Step 13: Commit**

```bash
git add src/provider
git commit -m "fix: close the remaining streaming/parsing asymmetries

Three gaps the first fix round missed. OpenAiResponses kept only
reasoning items where its parser keeps every unmodeled one. Anthropic
snapshotted opaque blocks at content_block_start, so a block whose input
streamed as deltas replayed empty. provider_metadata was populated for
Anthropic alone. The parity test now covers text, a reasoning blob, and
a tool call, and asserts to_message() equality across both paths."
```

---

### Task 18: Parity test per dialect — enumerate every divergence → verify: a test named `streamed_response_matches_generate` exists in all four dialects' `types.rs`, `cargo test --all-features --lib streamed_response_matches_generate` reports 4 tests run, and every failure is reported verbatim WITHOUT fixing any decoder

**Files:**
- Modify: `src/provider/anthropic/types.rs`
- Modify: `src/provider/gemini/types.rs`
- Modify: `src/provider/openai_chat/types.rs`
- Modify: `src/provider/openai_responses/types.rs`

Three rounds of review each found a streaming decoder disagreeing with its own
parser, and each round fixed only the instances named. This task inverts the
order: write the test that would have caught all of them, for every dialect, and
let the failures enumerate the full list in one pass.

**This task fixes nothing.** Its deliverable is the tests plus a complete,
verbatim list of what they reveal. Resist every urge to correct a decoder.

- [x] **Step 1: Read each parser to derive the expected behaviour**

For each dialect, read its `parse` and the helpers it calls, and write down:
- the full status-string map (every arm, including `requires_action` and any
  dialect-specific values)
- exactly how `usage` is computed (Anthropic sums cache tokens into the input
  total; the others may differ)
- whether the parser can produce `OutputContent::Refusal`, and from what shape
- which content/item/step types it models explicitly, and what its catch-all does

The parser is the specification for the decoder. Do not infer expected behaviour
from the decoder.

- [x] **Step 2: Add a parity test to each dialect**

Name it `streamed_response_matches_generate` in all four modules, so one filter
runs the set. Anthropic already has one — extend it rather than duplicating.

Each test drains an SSE fixture through `crate::provider::stream::drain_for_test`
and compares against `parse()` of a non-streaming body representing the *same
logical response*. Each fixture must cover, to the extent the dialect supports it:

- text, in at least two deltas, so coalescing is exercised
- a tool call with fragmented arguments
- a reasoning / thinking / thought block, or an unmodeled block if the dialect
  has no reasoning type
- a refusal, for the dialects whose parser produces `OutputContent::Refusal`
- usage, including Anthropic's `cache_creation_input_tokens` and
  `cache_read_input_tokens`
- a terminal status of `requires_action` (or the dialect's equivalent for a
  tool-calling turn), because that is the case a streaming caller hits most

Assertions, in this order:

```rust
        assert_eq!(streamed.id, parsed.id);
        assert_eq!(streamed.model, parsed.model);
        assert_eq!(streamed.status, parsed.status);
        assert_eq!(streamed.usage, parsed.usage);
        assert_eq!(streamed.content, parsed.content);
        assert_eq!(streamed.to_message(), parsed.to_message());
```

Do not assert on `provider_metadata`; it is documented as differing by shape.

- [x] **Step 3: Run them and record every failure**

Run: `cargo test --all-features --lib streamed_response_matches_generate`
Expected: 4 tests run. Some or all FAIL — that is the point of the task.

For each failure, capture the assertion message and the full `left:` / `right:`
values verbatim. Do not truncate. Do not summarise.

- [x] **Step 4: Commit the tests, failing**

The tests must not be left in a state that hides the problem, and they must not
be `#[ignore]`d. If the repo cannot hold failing tests, mark each failing test
`#[should_panic]` with a `// vibekit:` comment naming the divergence and the fix
that will remove the attribute — but PREFER committing them failing and say so.

```bash
git add src/provider
git commit -m "test: add streamed-vs-parsed parity tests for all four dialects

Written parser-first: each expectation is derived from what parse()
produces, not from what the decoder happens to do. Committed failing so
the divergences are enumerated in one place rather than discovered one
review at a time."
```

---

### Task 19: Make every decoder agree with its parser → verify: `cargo test --all-features --lib streamed_response_matches_generate` reports 4 tests run and 4 passed, with no fixture weakened

Task 18's four parity tests are committed failing. This task makes them pass by
correcting the decoders — never by editing a fixture or relaxing an assertion.
If a test can only be made to pass by changing what it expects, stop and report:
that means the parser and the spec disagree, which is a different problem.

**Files:**
- Modify: `src/provider/stream.rs`
- Modify: `src/provider/anthropic/mod.rs`
- Modify: `src/provider/gemini/mod.rs`
- Modify: `src/provider/openai_chat/mod.rs`
- Modify: `src/provider/openai_responses/mod.rs`
- Modify: `src/lib.rs` (re-export only, see Step 1)

- [x] **Step 1: Add a refusal to the event model**

Neither `RawDelta` nor `StreamEvent` can currently express a refusal, so no
decoder fix alone can close that gap. The parsers keep `OutputContent::Refusal`
distinct from text, and `to_message()` renders it as text; streaming must be able
to do the same.

This extends the public API beyond the approved spec, deliberately: without it a
streaming caller cannot observe a refusal at all, and `into_response()` silently
loses a whole content part. `StreamEvent` is `#[non_exhaustive]`, so the addition
breaks no downstream matcher.

In `src/provider/stream.rs`, add to `RawDelta`:

```rust
    /// The model declined to answer.
    Refusal(String),
```

and to `StreamEvent`, after `TextDelta`:

```rust
    /// The model declined to answer. Kept distinct from text because the
    /// non-streaming path does, and because a caller may want to render or
    /// log it differently.
    RefusalDelta(String),
```

In `Assembler::absorb`, handle it alongside `Text` but WITHOUT coalescing into a
neighbouring `Text` part — the parser emits refusals as their own part:

```rust
            RawDelta::Refusal(text) => {
                match self.captured.last_mut() {
                    Some(OutputContent::Refusal(existing)) => existing.push_str(&text),
                    _ => self.captured.push(OutputContent::Refusal(text.clone())),
                }
                out.push(StreamEvent::RefusalDelta(text));
            }
```

`src/lib.rs` needs no change if `StreamEvent` is already re-exported; confirm it is.

- [x] **Step 2: Anthropic — sum the cache tokens**

`src/provider/anthropic/mod.rs` stores `self.input_tokens` from `message_start`.
The parser sums `input_tokens + cache_creation_input_tokens + cache_read_input_tokens`.
Mirror it exactly:

```rust
                let usage = &message["usage"];
                self.input_tokens = usage["input_tokens"].as_u64().unwrap_or(0)
                    + usage["cache_creation_input_tokens"].as_u64().unwrap_or(0)
                    + usage["cache_read_input_tokens"].as_u64().unwrap_or(0);
```

- [x] **Step 3: Gemini — complete the status map**

Read the parser's status match and reproduce every arm. It maps at least
`requires_action` → `RequiresAction`, `budget_exceeded` → `Incomplete`, and
`cancelled` → `Failed`, in addition to the three the decoder already has. Copy
the parser's arms verbatim rather than paraphrasing them.

- [x] **Step 4: Gemini — accumulate deltas into opaque steps**

`Step::Opaque(Value)` is snapshotted at `step.start` and never updated. Give it a
delta buffer the way Anthropic's opaque blocks now have one. The concrete case
the test exercises is a `code_execution` step whose `code` field arrives in a
`step.delta`. Merge whatever fields a delta carries into the stored step before
emitting it at `step.stop`, so the blob matches what the parser would have
stored. Read the failing test's fixture for the exact delta shape.

- [x] **Step 5: OpenAiChat — decode refusals and complete the status map**

Add a refusal read alongside the existing content read:

```rust
        if let Some(text) = choice["delta"]["refusal"].as_str()
            && !text.is_empty()
        {
            out.push(RawDelta::Refusal(text.to_string()));
        }
```

and add the parser's missing finish-reason arm:

```rust
            "function_call" => ResponseStatus::RequiresAction,
```

- [x] **Step 6: OpenAiResponses — decode refusals and map requires_action**

Add `"requires_action" => ResponseStatus::RequiresAction` to the status match.
For refusals, read the failing test's fixture: it carries both a
`response.refusal.delta` event and a refusal part inside the message item on
`response.output_item.done`. Decode the delta form; make sure the `output_item.done`
path does not double-count it.

- [x] **Step 7: Run the parity suite**

Run: `cargo test --all-features --lib streamed_response_matches_generate`
Expected: `running 4 tests` and `4 passed`. Confirm the count is 4, not 0.

If any test still fails, report the verbatim `left:`/`right:` and stop. Do not
edit the fixture.

- [x] **Step 8: Run everything**

Run: `cargo test --all-features` → PASS, all tests.
Run: `cargo clippy --all-targets --all-features -- -D warnings` → no warnings.
Run: `cargo fmt --all` then `cargo fmt --all --check` → no output.

- [x] **Step 9: Correct the into_response doc**

The doc comment claims usage matches. That is now true for Anthropic; confirm the
wording is accurate for all four and adjust if not.

- [x] **Step 10: Commit**

```bash
git add src/provider src/lib.rs
git commit -m "fix: make every streaming decoder agree with its parser

Closes the seven divergences the per-dialect parity tests enumerated:
Anthropic's usage ignored both cache token fields; Gemini and
OpenAiResponses never mapped requires_action; Gemini's opaque steps were
snapshotted before their payload streamed in; neither OpenAI dialect
decoded refusals, which the event model could not express at all.

Adds StreamEvent::RefusalDelta -- a deliberate extension beyond the
approved spec, since without it a streaming caller cannot observe a
refusal and into_response drops a whole content part."
```

---

### Task 20: Match the parsers' remaining normalisations → verify: `cargo test --all-features --lib streamed_response_matches_generate` reports 4 run / 4 passed with fixtures using NON-alphabetical tool-argument keys, and `cargo test --all-features --lib usage_defaults_missing_fields` passes

Two divergences remain, both of the same kind that has produced every previous
round: a transformation a parser applies that the streaming path does not.

**Gap A — tool-argument key order.** `serde_json` here uses `BTreeMap`
(`preserve_order` is not enabled), so `Value::to_string()` emits **key-sorted,
whitespace-stripped** JSON. Anthropic re-serializes (`anthropic/types.rs:382-385`,
`input.to_string()`) and so does Gemini (`gemini/types.rs:328-331`,
`.map(Value::to_string)`). The OpenAI dialects instead pass the raw string
through (`openai_chat/types.rs:355-359`, and `openai_responses`). The streaming
path always concatenates raw fragments, so for Anthropic and Gemini a model
emitting `{"unit":"c","location":"NYC"}` yields `{"location":"NYC","unit":"c"}`
from `generate()` and the raw order from a drained stream.

Because the behaviour differs *per dialect*, this must NOT be fixed in the shared
assembler — doing so would break parity for the two OpenAI dialects, which
deliberately preserve the raw string.

**Gap B — usage subfield defaults.** `openai_chat/types.rs:322-331` and the
Gemini equivalent mark every usage subfield `#[serde(default)]`, so a body
carrying only two of three fields still parses to `Some(Usage{..0})`. Both
decoders use `as_u64()?` inside `and_then(|usage| Some(..))`, so one missing
subfield collapses the whole thing to `None`.

- [x] **Step 1: Teach the decoder trait which dialects normalise**

In `src/provider/stream.rs`, add to `StreamDecoder`:

```rust
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
```

Override it to return `true` in `anthropic::Decoder` and `gemini::Decoder` only,
each with a one-line comment pointing at the parser line it mirrors.

- [x] **Step 2: Apply it in the assembler**

Give `Assembler` a `normalize_arguments: bool` field, set from the decoder when
the stream is built (`EventStream::new`, `for_test`, and `drain_for_test` all
construct one — thread it through all three). In `finish_call`, after the
existing empty-to-`{}` normalisation:

```rust
        // Anthropic and Gemini parse tool input and re-serialize it, which
        // sorts keys. Round-trip the streamed fragments the same way so the two
        // paths agree; leave a body that is not valid JSON untouched rather
        // than discarding what the model sent.
        if self.normalize_arguments
            && let Ok(value) = serde_json::from_str::<Value>(&call.arguments)
        {
            call.arguments = value.to_string();
        }
```

- [x] **Step 3: Mirror each parser's usage defaults**

In `openai_chat/mod.rs` and `gemini/mod.rs`, replace the `?` on each usage
subfield with `.unwrap_or(0)`, so a `usage` object that is present but partial
yields `Some(Usage{..})` exactly as the parser does. Check
`openai_responses/mod.rs` and `anthropic/mod.rs` against their own parsers too
and align whichever differ — read each parser's usage struct rather than
assuming.

- [x] **Step 4: Make the fixtures catch Gap A**

In `anthropic/types.rs` and `gemini/types.rs`, change the tool-call fixture in
`streamed_response_matches_generate` so the streamed argument fragments spell a
**non-alphabetical** key order, e.g. `{"unit":"c","location":"NYC"}`, while the
non-streaming body carries the same object. Before this task the assertion would
fail on key order; after it, both sides read `{"location":"NYC","unit":"c"}`.

Leave the two OpenAI fixtures' argument order alone, and add a brief comment in
each noting that those dialects deliberately preserve the raw string.

- [x] **Step 5: Add a regression test for Gap B**

In `openai_chat/types.rs`, add:

```rust
    #[test]
    fn usage_defaults_missing_fields() {
        let deltas = decode_all(&[
            r#"{"id":"chatcmpl-1","model":"gpt-4o","choices":[],"usage":{"prompt_tokens":11,"completion_tokens":9}}"#,
        ]);

        let usage = deltas
            .iter()
            .find_map(|d| match d {
                RawDelta::Meta { usage: Some(u), .. } => Some(*u),
                _ => None,
            })
            .expect("a partial usage object still yields usage, as the parser's \
                     #[serde(default)] fields do — not None");
        assert_eq!(usage.input_tokens, 11);
        assert_eq!(usage.output_tokens, 9);
        assert_eq!(usage.total_tokens, 0);
    }
```

- [x] **Step 6: Run everything**

Run: `cargo test --all-features --lib streamed_response_matches_generate` → 4 run, 4 passed.
Run: `cargo test --all-features --lib usage_defaults_missing_fields` → 1 run, 1 passed.
Run: `cargo test --all-features` → all pass.
Run: `cargo clippy --all-targets --all-features -- -D warnings` → clean.
Run: `cargo fmt --all` then `cargo fmt --all --check` → no output.

- [x] **Step 7: Commit**

```bash
git add src/provider
git commit -m "fix: match the parsers' tool-argument and usage normalisation

serde_json sorts keys, so Anthropic's and Gemini's parsers -- which
re-serialize tool input via Value::to_string -- returned a different
argument string than the streamed fragments. The OpenAI dialects
deliberately pass the raw string through, so the choice is per dialect
rather than shared. Separately, both those decoders collapsed usage to
None when any subfield was absent, where the parsers default it to zero."
```

---

### Task 21: Preserve text-block boundaries → verify: `cargo test --all-features --lib streamed_response_matches_generate` reports 4 run / 4 passed with fixtures carrying TWO adjacent text blocks, and `assembler_keeps_text_blocks_separate` passes

The parsers emit one `OutputContent::Text` per text block
(`anthropic/types.rs:363-370`, and the equivalent in `gemini/types.rs` and
`openai_responses/types.rs`). `Assembler::absorb` coalesces every consecutive
`RawDelta::Text` into one part, and no decoder signals where a block ends. A
response carrying two text blocks — Anthropic produces these with citations and
after server tool use — drains to `[Text("AB")]` where `generate()` returns
`[Text("A"), Text("B")]`.

The coalescing itself must stay: within one block, consecutive deltas have to
merge or every fragment becomes its own part. What is missing is the boundary.

**Files:**
- Modify: `src/provider/stream.rs`
- Modify: `src/provider/anthropic/mod.rs`, `src/provider/anthropic/types.rs`
- Modify: `src/provider/gemini/mod.rs`, `src/provider/gemini/types.rs`
- Modify: `src/provider/openai_responses/mod.rs`, `src/provider/openai_responses/types.rs`

OpenAiChat is deliberately excluded: its parser builds a single `Text` from
`message.content`, so it has no block boundaries to preserve. Confirm that by
reading its parser before deciding to skip it.

- [x] **Step 1: Write the failing assembler test**

In `src/provider/stream.rs`, add to `mod tests`:

```rust
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
```

- [x] **Step 2: Run it, confirm it fails**

Run: `cargo test --all-features --lib assembler_keeps_text_blocks_separate`
Expected: FAIL to compile — `TextEnd` is not a variant of `RawDelta`.

- [x] **Step 3: Add the boundary**

In `src/provider/stream.rs`, add to `RawDelta`:

```rust
    /// The end of one text block. Text continues to coalesce within a block;
    /// this starts a new `OutputContent::Text`, matching one part per block.
    TextEnd,
```

Add a field to `Assembler`:

```rust
    /// Whether the trailing captured part is a text block still being filled.
    text_open: bool,
```

initialised `false` in `new`. Change the `RawDelta::Text` arm to respect it:

```rust
            RawDelta::Text(text) => {
                match self.captured.last_mut() {
                    Some(OutputContent::Text(existing)) if self.text_open => {
                        existing.push_str(&text)
                    }
                    _ => self.captured.push(OutputContent::Text(text.clone())),
                }
                self.text_open = true;
                out.push(StreamEvent::TextDelta(text));
            }
```

and add:

```rust
            RawDelta::TextEnd => self.text_open = false,
```

- [x] **Step 4: Confirm the assembler test passes**

Run: `cargo test --all-features --lib assembler_keeps_text_blocks_separate`
Expected: PASS, 1 test.

- [x] **Step 5: Emit the boundary from each decoder that has blocks**

`anthropic/mod.rs` — `content_block_start` currently has `Some("text") => {}`.
Record the index in a `texts: HashSet<usize>` there, and at `content_block_stop`
emit `RawDelta::TextEnd` when the index was a text block. Keep the existing
tool-call and thinking branches ahead of it.

`gemini/mod.rs` — text arrives as `step.delta` with `delta.type == "text"` inside
a `model_output` step. Record `model_output` step indices at `step.start` (the arm
is currently `Some("model_output") => {}`) and emit `RawDelta::TextEnd` at
`step.stop` for those.

`openai_responses/mod.rs` — read the parser first: `convert_item` maps each
`output_text` part of a message item to its own `OutputContent::Text`. Emit
`RawDelta::TextEnd` on the frame that ends one such part — check whether that is
`response.output_text.done` or `response.content_part.done` in the fixtures and
use whichever the dialect actually sends.

`openai_chat/mod.rs` — no change. Its parser produces one `Text` per response.

- [x] **Step 6: Extend the parity fixtures to two text blocks**

In `anthropic/types.rs`, `gemini/types.rs`, and `openai_responses/types.rs`, add a
SECOND text block to both sides of `streamed_response_matches_generate`: two
blocks on the streaming side with the boundary between them, and two
corresponding text entries in the non-streaming body. Keep every existing
assertion and every existing part of the fixture.

Leave the OpenAiChat fixture alone and add a one-line comment there noting that
this dialect has a single text part by construction.

- [x] **Step 7: Run everything**

Run: `cargo test --all-features --lib streamed_response_matches_generate` → 4 run, 4 passed.
Run: `cargo test --all-features` → all pass.
Run: `cargo clippy --all-targets --all-features -- -D warnings` → clean.
Run: `cargo fmt --all` then `cargo fmt --all --check` → no output.

- [x] **Step 8: Prove the fixtures catch the bug**

Temporarily comment out the `RawDelta::TextEnd` emission in `anthropic/mod.rs`,
re-run the parity filter, and confirm the Anthropic test FAILS. Restore it and
confirm the suite is green again. Report both observations. A fixture that passes
either way proves nothing.

- [x] **Step 9: Commit**

```bash
git add src/provider
git commit -m "fix: keep streamed text blocks as separate content parts

The parsers emit one OutputContent::Text per text block; the assembler
coalesced every consecutive delta into one, so a response with two text
blocks -- Anthropic sends these with citations and after server tool use
-- drained to a single merged part. Coalescing within a block is still
required, so the fix is a block boundary rather than removing it."
```

---

# Review follow-up: close the warns and nits

> Added after `docs/reviews/2026-08-09-streaming-api-review.md` returned
> 0 blocks, 4 warns, 3 nits, and the user asked for all of them addressed.

### Task 22: Reconcile the documents with what shipped → verify: the spec's `## Approach` lists six `StreamEvent` variants including `RefusalDelta`, the plan header records that the plan grew mid-run, and `cargo test --doc` still passes

Closes W1, W3, N1, N3.

**Files:**
- Modify: `docs/specs/2026-08-09-streaming-api-design.md`
- Modify: `docs/plans/2026-08-09-streaming-api.md`
- Modify: `src/provider/anthropic/mod.rs`, `src/provider/gemini/mod.rs`,
  `src/provider/openai_chat/mod.rs`, `src/provider/openai_responses/mod.rs`
- Modify: `src/provider/mod.rs`

- [x] **Step 1: W1 — bring the spec's event model up to date**

In `docs/specs/2026-08-09-streaming-api-design.md`, the `## Approach` section's
`StreamEvent` block lists five variants. Add `RefusalDelta` in the position it
occupies in the code (immediately after `TextDelta`) so the spec matches
`src/provider/stream.rs`:

```rust
    /// The model declined to answer.
    RefusalDelta(String),
```

Then add this paragraph immediately after that code block:

```markdown
`RefusalDelta` was not in the originally approved event model. It was added
during implementation because both OpenAI parsers produce
`OutputContent::Refusal` and the streaming path could not express one at all:
a refused response arrived as content through `generate()` and as nothing
through `stream()`, which also made `into_response()` drop a whole content part.
`StreamEvent` is `#[non_exhaustive]`, so the addition breaks no downstream
matcher. The internal `RawDelta::TextEnd` was added for the same class of reason
— the parsers emit one `OutputContent::Text` per text block, so the stream needs
a block boundary — and carries no public event.
```

Do NOT change the spec's `status: approved` frontmatter; the design was approved
and this records what implementation revealed.

- [x] **Step 2: W3 — make the plan honest about its own growth**

In `docs/plans/2026-08-09-streaming-api.md`, immediately after the
"Vacuous-pass guard" blockquote in the header, add:

```markdown
> **This plan grew during execution.** It was approved with 14 tasks. Tasks 15-21
> were appended after verification runs found defects, and the plan was repaired
> three times mid-run: cargo-test filters that matched nothing (`570c841`),
> doctests and clippy deferred past Task 6 because the code they check does not
> exist yet at that point (`4fe3c3b`), and Task 14's reachability checks
> corrected after a clippy fix changed what they were grepping for (`a7d9c6b`).
> Every change is a separate commit with its reasoning. Read the follow-up
> sections as part of the plan, not as an appendix.
```

- [x] **Step 3: N1 — make the per-dialect mapping duplication explicitly deliberate**

Each decoder hand-writes its own status match and `Usage` construction. That
looks like duplication but each mirrors a *different* parser, and five rounds of
verification defects came from these drifting apart. Add a comment above the
status match in each of the four `mod.rs` decoders, naming the parser function
and file it mirrors, in this shape:

```rust
            // Mirrors `parse_status` in types.rs. Kept as its own match rather
            // than shared with the other dialects: the strings differ per
            // provider, and every divergence found in review came from this
            // mapping drifting from the parser's.
```

Use the actual function name each dialect's parser uses — read it first rather
than assuming they are all called `parse_status`.

- [x] **Step 4: N3 — record why `stream_query` is public**

In `src/provider/mod.rs`, extend the doc comment on
`ProviderDialect::stream_query` with a sentence explaining the choice:

```rust
    /// Public for the same reason as [`Self::path`], [`Self::default_auth`], and
    /// [`Self::required_headers`]: it describes the dialect, and a caller
    /// building a request by hand needs it.
```

- [x] **Step 5: Verify**

Run: `cargo test --doc` → PASS.
Run: `cargo test --all-features` → PASS, all tests.
Run: `cargo fmt --all --check` → no output.
Run: `cargo clippy --all-targets --all-features -- -D warnings` → clean.

- [x] **Step 6: Commit**

```bash
git add docs src
git commit -m "docs: reconcile spec and plan with what shipped

The spec described a five-variant StreamEvent; the crate ships six, and
the sixth exists because refusals were otherwise unobservable. Records
that too, and that the plan grew from 14 tasks to 21 during execution."
```

---

### Task 23: Test the streaming transport end to end → verify: `cargo test --all-features --test streaming_transport` reports 3 tests run and 3 passed, exercising `Client::stream` against a real local HTTP server

Closes W2 and W4, and the review's self-critique risk 3.

`Client::stream` is currently referenced only by `no_run` doctests and the
example binary — **no test executes it**, and `Body::Live` (the arm that calls
`reqwest::Response::chunk()`) never runs under `cargo test`. Request-body
construction, the `?alt=sse` URL, headers, auth, the non-2xx early return, and
the byte pump are all unverified.

**Files:**
- Create: `tests/streaming_transport.rs`

Constraint: **do not modify `Cargo.toml`.** No new dependency and no new feature.
Use `std::net::TcpListener` on a background `std::thread` to serve a canned
response; `tokio` is already a dev-dependency with `macros` and `rt-multi-thread`,
so `#[tokio::test]` is available. Bind to `127.0.0.1:0` and read back the assigned
port so the tests cannot collide.

- [x] **Step 1: Write the harness and the happy-path test**

Create `tests/streaming_transport.rs`. The server helper should accept exactly
one connection, read the request head (so the client is not left blocked on a
half-closed socket), hand the raw request back to the test for assertions, and
write the supplied response bytes.

```rust
//! End-to-end transport tests for `Client::stream`.
//!
//! The rest of the suite drives decoders over recorded bytes; nothing else
//! exercises the HTTP path. These use a real socket so that request building,
//! headers, auth, the status check, and the byte pump are all covered.

use freyja::{Client, GenerateRequest, Message, ProviderConfig, ProviderDialect, Role, StreamEvent};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::mpsc;

/// Serves one request and returns what the client sent.
fn serve_once(response: &'static str) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    let (tx, rx) = mpsc::channel();

    std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("accept");
        let mut reader = BufReader::new(socket.try_clone().expect("clone"));

        // Read the head, then the body if the client announced a length.
        let mut head = String::new();
        let mut length = 0usize;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).expect("read") == 0 || line == "\r\n" {
                break;
            }
            if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = value.trim().parse().unwrap_or(0);
            }
            head.push_str(&line);
        }
        let mut body = vec![0u8; length];
        if length > 0 {
            std::io::Read::read_exact(&mut reader, &mut body).expect("body");
        }
        head.push_str(&String::from_utf8_lossy(&body));

        socket.write_all(response.as_bytes()).expect("write");
        socket.flush().expect("flush");
        let _ = tx.send(head);
    });

    (base, rx)
}

#[tokio::test]
async fn streams_text_from_a_live_server() {
    let (base, request) = serve_once(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/event-stream\r\n\
         Connection: close\r\n\r\n\
         data: {\"id\":\"chatcmpl-1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n\
         data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}},{\"finish_reason\":\"stop\"}]}\n\n\
         data: [DONE]\n\n",
    );

    let config = ProviderConfig::new(ProviderDialect::OpenAiChat, "local", base)
        .default_model("test-model");
    let client = Client::new(config, "sk-test");

    let mut stream = client
        .stream(&GenerateRequest::new().message(Message::text(Role::User, "Hi")))
        .await
        .expect("stream opens");

    let mut text = String::new();
    while let Some(event) = stream.next().await.expect("no stream error") {
        if let StreamEvent::TextDelta(delta) = event {
            text.push_str(&delta);
        }
    }
    assert_eq!(text, "Hello");

    let sent = request.recv().expect("captured request");
    assert!(sent.starts_with("POST /chat/completions "), "{sent}");
    assert!(sent.contains("authorization: Bearer sk-test") || sent.contains("Authorization: Bearer sk-test"), "{sent}");
    assert!(sent.contains("\"stream\":true"), "the body must ask for a stream: {sent}");
    assert!(sent.contains("\"include_usage\":true"), "usage must be requested: {sent}");

    let response = stream.into_response().expect("drained");
    assert_eq!(response.output_text(), "Hello");
}
```

- [x] **Step 2: Run it**

Run: `cargo test --all-features --test streaming_transport`
Expected: PASS, 1 test. If the client hangs, the server helper is not reading the
request body before writing — fix the helper, not the client.

- [x] **Step 3: Add the non-2xx test**

An error status must surface from `stream()` itself, not mid-iteration:

```rust
#[tokio::test]
async fn surfaces_an_error_status_before_streaming() {
    let (base, _request) = serve_once(
        "HTTP/1.1 429 Too Many Requests\r\n\
         Content-Type: application/json\r\n\
         Connection: close\r\n\r\n\
         {\"error\":{\"message\":\"slow down\"}}",
    );

    let config = ProviderConfig::new(ProviderDialect::OpenAiChat, "local", base)
        .default_model("test-model");
    let client = Client::new(config, "sk-test");

    let error = client
        .stream(&GenerateRequest::new().message(Message::text(Role::User, "Hi")))
        .await
        .expect_err("a 429 must not produce a stream");

    match error {
        freyja::ProviderError::Api { status, body, provider } => {
            assert_eq!(status, 429);
            assert_eq!(&*provider, "local");
            assert!(body.contains("slow down"), "{body}");
        }
        other => panic!("expected ProviderError::Api, got {other:?}"),
    }
}
```

- [x] **Step 4: Add the Gemini URL test**

Gemini selects SSE by query parameter as well as by body field; nothing currently
proves the client actually sends it.

```rust
#[tokio::test]
async fn gemini_requests_sse_by_query_parameter() {
    let (base, request) = serve_once(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/event-stream\r\n\
         Connection: close\r\n\r\n\
         data: {\"interaction\":{\"id\":\"v1_1\",\"model\":\"gemini-test\",\"status\":\"completed\"},\"event_type\":\"interaction.completed\"}\n\n",
    );

    let config = ProviderConfig::new(ProviderDialect::Gemini, "local", base)
        .default_model("test-model");
    let client = Client::new(config, "key");

    let mut stream = client
        .stream(&GenerateRequest::new().message(Message::text(Role::User, "Hi")))
        .await
        .expect("stream opens");
    while stream.next().await.expect("no error").is_some() {}

    let sent = request.recv().expect("captured request");
    assert!(
        sent.starts_with("POST /interactions?alt=sse "),
        "Gemini needs ?alt=sse on the URL, not only stream:true in the body: {sent}"
    );
    assert!(sent.contains("Api-Revision: 2026-05-20") || sent.contains("api-revision: 2026-05-20"), "{sent}");
}
```

- [x] **Step 5: Run the suite**

Run: `cargo test --all-features --test streaming_transport`
Expected: `running 3 tests` and `3 passed`. Confirm the count is 3, not 0.

Run: `cargo test --all-features` → PASS, all tests.
Run: `cargo clippy --all-targets --all-features -- -D warnings` → clean.
Run: `cargo fmt --all` then `cargo fmt --all --check` → no output.

- [x] **Step 6: Prove the tests are load-bearing**

Temporarily change `Client::stream` to POST to `self.config.url()` instead of
`self.config.stream_url()`, re-run, and confirm `gemini_requests_sse_by_query_parameter`
FAILS. Restore it and confirm green. Report both observations verbatim. A test
that passes either way proves nothing.

- [x] **Step 7: Commit**

```bash
git add tests/streaming_transport.rs
git commit -m "test: cover the streaming transport end to end

Client::stream was reachable only from no_run doctests and the example
binary, so request building, the ?alt=sse URL, headers, auth, the non-2xx
early return, and the byte pump were all unverified. Serves canned
responses over a real socket using std::net, so no new dependency."
```

---

### Task 24: Collapse the redundant test helper → verify: `cargo test --all-features` passes with `next_blocking` removed and its two callers unaffected

Closes N2.

`EventStream::for_test`, `EventStream::next_blocking`, and the free
`drain_for_test` total ~35 lines for what two functions can do.

**Files:**
- Modify: `src/provider/stream.rs`

- [x] **Step 1: Read the three helpers and their callers**

Run: `grep -rn "next_blocking\|for_test\|drain_for_test" src/`

Decide which of the two remaining shapes is smaller, given every caller:
either fold `next_blocking`'s poll loop into `drain_for_test` and have the one
event-by-event test drive it directly, or keep `next_blocking` and drop the
separate `drain_for_test` wrapper. Pick the one that removes more lines without
making a call site harder to read.

- [x] **Step 2: Make the change and confirm nothing else moved**

Run: `cargo test --all-features`
Expected: PASS, same test count as before the change. A changed count means a
test was lost — restore it.

Run: `cargo clippy --all-targets --all-features -- -D warnings` → clean.
Run: `cargo fmt --all --check` → no output.

- [x] **Step 3: Commit**

```bash
git add src/provider/stream.rs
git commit -m "refactor: collapse a redundant streaming test helper"
```
