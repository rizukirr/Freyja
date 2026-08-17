# Modular public API — Implementation Plan

**Spec:** docs/specs/2026-08-17-modular-public-api-design.md
**Goal:** Give Freyja a modular source and public API while preserving its wire and runtime behavior.
**Architecture:** Keep Freyja as one crate with a curated root API and focused public `model`, `dialect`, `endpoint`, `stream`, and `error` modules. Keep HTTP transport, wire structures, dialect conversion, SSE framing, and stream assembly private behind `Client`.

## Global constraints

- Freyja remains one Cargo crate.
- Add no dependencies and do not introduce provider feature flags.
- Preserve request and response wire JSON, runtime behavior, and performance.
- Preserve `GenerateResponse::provider_metadata` with its current name and behavior.
- Keep wire request and response structures, dialect decoders, SSE framing, response assembly, HTTP transport, and the dialect conversion trait private.
- Keep `#![deny(missing_docs)]` enabled for every public item.
- Keep API keys and secret-looking headers redacted from `Debug` output.
- Remove the old public API without aliases or a deprecated `provider` compatibility module.
- Keep unrelated `.gitignore` and previously untracked documentation changes out of every task commit.
- Run socket-binding integration tests outside the managed sandbox; the unchanged baseline passes there and fails inside the sandbox with `EPERM` at `tests/streaming_transport.rs:16`.

### Task 1: Extract the neutral model and errors → verify: `cargo test --lib` exits successfully with public `model` and `error` modules

**Files:**
- Create: `src/model/mod.rs`
- Create: `src/model/request.rs`
- Create: `src/model/message.rs`
- Create: `src/model/schema.rs`
- Create: `src/model/tools.rs`
- Create: `src/model/response.rs`
- Create: `src/error.rs`
- Modify: `src/lib.rs`
- Delete: `src/provider/model.rs`
- Modify: `src/provider/mod.rs`
- Modify: `src/provider/refusal.rs`
- Modify: `src/provider/stream.rs`
- Modify: `src/provider/anthropic/mod.rs`
- Modify: `src/provider/anthropic/types.rs`
- Modify: `src/provider/gemini/mod.rs`
- Modify: `src/provider/gemini/types.rs`
- Modify: `src/provider/openai_chat/mod.rs`
- Modify: `src/provider/openai_chat/types.rs`
- Modify: `src/provider/openai_responses/mod.rs`
- Modify: `src/provider/openai_responses/types.rs`

- [x] Step 1: Move `GenerateRequest` and its builder into `src/model/request.rs`; move `Message`, `Role`, `InputContent`, and `ReasoningEffort` into `src/model/message.rs`; move `ResponseFormat` and `strict_schema` with its private helpers into `src/model/schema.rs`; move `ToolDefinition` and `ToolChoice` into `src/model/tools.rs`; move `GenerateResponse`, `ResponseStatus`, `OutputContent`, and `Usage` into `src/model/response.rs`.
- [x] Step 2: Define the public model surface in `src/model/mod.rs` with explicit modules and re-exports:

  ```rust
  mod message;
  mod request;
  mod response;
  mod schema;
  mod tools;

  pub use message::{InputContent, Message, ReasoningEffort, Role};
  pub use request::GenerateRequest;
  pub use response::{GenerateResponse, OutputContent, ResponseStatus, Usage};
  pub use schema::{ResponseFormat, strict_schema};
  pub use tools::{ToolChoice, ToolDefinition};
  ```

- [x] Step 3: Move `TransportError` and `ProviderError` with their classification, retry, display, and source implementations into `src/error.rs`; rename `ProviderError` to `Error`, rename error variant fields from `provider` to `endpoint`, and rename the `provider()` accessor to `endpoint()`.
- [x] Step 4: Update model and error references throughout `src/provider/` to import from `crate::model` and `crate::error`; preserve every existing variant, raw response body, retry rule, and redaction behavior.
- [x] Step 5: Remove `src/provider/model.rs` after its code and tests have moved to their owning files.
- [x] Step 6: Expose the modules and ergonomic root imports in `src/lib.rs`:

  ```rust
  pub mod error;
  pub mod model;

  pub use error::{Error, TransportError};
  pub use model::{
      GenerateRequest, GenerateResponse, InputContent, Message, OutputContent,
      ReasoningEffort, ResponseFormat, ResponseStatus, Role, ToolChoice,
      ToolDefinition, Usage, strict_schema,
  };
  ```

- [x] Step 7: Run `cargo fmt --check`.
- [x] Step 8: Run `cargo test --lib`.
- [x] Step 9: Commit with message `refactor: extract model and errors`.

### Task 2: Extract public streaming and private stream machinery → verify: `cargo test --lib stream` exits successfully and `src/provider/stream.rs` and `src/provider/sse.rs` do not exist

**Files:**
- Create: `src/stream/mod.rs`
- Create: `src/stream/event.rs`
- Create: `src/stream/assembler.rs`
- Create: `src/stream/sse.rs`
- Modify: `src/lib.rs`
- Modify: `src/provider/mod.rs`
- Modify: `src/provider/anthropic/mod.rs`
- Modify: `src/provider/gemini/mod.rs`
- Modify: `src/provider/openai_chat/mod.rs`
- Modify: `src/provider/openai_responses/mod.rs`
- Delete: `src/provider/stream.rs`
- Delete: `src/provider/sse.rs`

- [x] Step 1: Move `StreamEvent` and `EventStream` into `src/stream/event.rs` without changing their public fields or methods.
- [x] Step 2: Move `RawDelta`, `StreamDecoder`, `Assembler`, pending-call state, recorded-body test support, and their tests into `src/stream/assembler.rs`; expose them only as `pub(crate)` where another internal module needs access.
- [x] Step 3: Move `SseFrame`, `SseBuffer`, separator handling, and their tests into private `src/stream/sse.rs`.
- [x] Step 4: Define the public stream surface in `src/stream/mod.rs`:

  ```rust
  mod assembler;
  mod event;
  mod sse;

  pub use event::{EventStream, StreamEvent};
  pub(crate) use assembler::{RawDelta, StreamDecoder};
  pub(crate) use sse::SseFrame;
  ```

- [x] Step 5: Update client and dialect decoder imports to use `crate::stream`; keep SSE framing, raw deltas, decoder traits, and assembly private.
- [x] Step 6: Re-export `EventStream` and `StreamEvent` from `src/lib.rs`.
- [x] Step 7: Run `cargo fmt --check`.
- [x] Step 8: Run `cargo test --lib stream`.
- [x] Step 9: Commit with message `refactor: extract streaming modules`.

### Task 3: Separate endpoint configuration, dialect dispatch, client, and transport → verify: `cargo test --lib` exits successfully and no `src/provider` path remains

**Files:**
- Create: `src/endpoint/mod.rs`
- Create: `src/endpoint/presets.rs`
- Create: `src/dialect/mod.rs`
- Create: `src/dialect/refusal.rs`
- Create: `src/client.rs`
- Create: `src/transport/mod.rs`
- Move: `src/provider/anthropic/mod.rs` to `src/dialect/anthropic/mod.rs`
- Move: `src/provider/anthropic/types.rs` to `src/dialect/anthropic/types.rs`
- Move: `src/provider/gemini/mod.rs` to `src/dialect/gemini/mod.rs`
- Move: `src/provider/gemini/types.rs` to `src/dialect/gemini/types.rs`
- Move: `src/provider/openai_chat/mod.rs` to `src/dialect/openai_chat/mod.rs`
- Move: `src/provider/openai_chat/types.rs` to `src/dialect/openai_chat/types.rs`
- Move: `src/provider/openai_responses/mod.rs` to `src/dialect/openai_responses/mod.rs`
- Move: `src/provider/openai_responses/types.rs` to `src/dialect/openai_responses/types.rs`
- Modify: `src/lib.rs`
- Modify: `src/model/request.rs`
- Delete: `src/provider/mod.rs`
- Delete: `src/provider/presets.rs`
- Delete: `src/provider/refusal.rs`

- [x] Step 1: Move `Auth`, `TokenLimitField`, configuration builders, URL construction, model resolution, secret-header detection, and redacted `Debug` behavior into `src/endpoint/mod.rs`; rename `ProviderConfig` to `EndpointConfig`.
- [x] Step 2: Move endpoint presets and their tests into `src/endpoint/presets.rs`; rename `ProviderType` to `EndpointPreset` and preserve the existing endpoint values.
- [x] Step 3: Move `ProviderDialect` and its path, authentication, required-header, and streaming-query behavior into `src/dialect/mod.rs`; rename it to `Dialect`.
- [x] Step 4: Rename the internal `Provider` trait to private `WireDialect`, keep its associated request type and `build` and `parse` methods, and move dialect dispatch, decoder dispatch, private dialect modules, and the dispatch macro into `src/dialect/mod.rs`.
- [x] Step 5: Move refusal evidence and validation into private `src/dialect/refusal.rs`, updating it to use `Dialect`, `EndpointConfig`, and `Error`.
- [x] Step 6: Move `Client`, its constructors, request entry points, and redacted `Debug` implementation into `src/client.rs`; make its public signatures use `EndpointConfig`, `EndpointPreset`, `Dialect`, and `Error`.
- [x] Step 7: Move shared HTTP execution, authentication application, retry-after parsing, default HTTP-client construction, body merging, and serialization helpers into private `src/transport/mod.rs`; expose only the crate-private functions required by `Client`.
- [x] Step 8: Define the public endpoint surface by declaring `Auth`, `EndpointConfig`, and `TokenLimitField` as public in `src/endpoint/mod.rs` and re-exporting the preset:

  ```rust
  mod presets;

  pub use presets::EndpointPreset;
  ```

- [x] Step 9: Expose `client`, `dialect`, and `endpoint` from `src/lib.rs`, keep `transport` private, and provide root re-exports:

  ```rust
  mod client;
  pub mod dialect;
  pub mod endpoint;
  mod transport;

  pub use client::Client;
  pub use dialect::Dialect;
  pub use endpoint::{Auth, EndpointConfig, EndpointPreset, TokenLimitField};
  ```

- [x] Step 10: Update every internal `crate::provider` reference to the owning `model`, `error`, `stream`, `endpoint`, `dialect`, `client`, or `transport` module and remove `src/provider` once empty.
- [x] Step 11: Run `cargo fmt --check`.
- [x] Step 12: Run `cargo test --lib`.
- [x] Step 13: Run `test ! -e src/provider`.
- [x] Step 14: Commit with message `refactor: separate client endpoint and dialect`.

### Task 4: Split OpenAI Responses wire responsibilities → verify: `cargo test --lib dialect::openai_responses` exits successfully and `src/dialect/openai_responses/types.rs` does not exist

**Files:**
- Create: `src/dialect/openai_responses/request.rs`
- Create: `src/dialect/openai_responses/response.rs`
- Create: `src/dialect/openai_responses/stream.rs`
- Modify: `src/dialect/openai_responses/mod.rs`
- Delete: `src/dialect/openai_responses/types.rs`

- [x] Step 1: Move outbound request wire structures, request construction, streaming selection, transcript flushing, validation, and their tests from `types.rs` into private `request.rs`.
- [x] Step 2: Move response wire structures, `GenerateResponse` conversion, status parsing, unknown-item preservation, response parsing, and their tests into private `response.rs`.
- [x] Step 3: Move `Decoder`, its `StreamDecoder` implementation, stream-frame parsing, and streaming parity tests from `mod.rs` and `types.rs` into private `stream.rs`.
- [x] Step 4: Make `mod.rs` declare `request`, `response`, and `stream`, implement `WireDialect` using `request::Request` and `response::parse`, and expose its decoder only to crate-internal dispatch.
- [x] Step 5: Run `cargo fmt --check`.
- [x] Step 6: Run `cargo test --lib dialect::openai_responses`.
- [x] Step 7: Commit with message `refactor: split OpenAI Responses dialect`.

### Task 5: Split OpenAI Chat wire responsibilities → verify: `cargo test --lib dialect::openai_chat` exits successfully and `src/dialect/openai_chat/types.rs` does not exist

**Files:**
- Create: `src/dialect/openai_chat/request.rs`
- Create: `src/dialect/openai_chat/response.rs`
- Create: `src/dialect/openai_chat/stream.rs`
- Modify: `src/dialect/openai_chat/mod.rs`
- Delete: `src/dialect/openai_chat/types.rs`

- [x] Step 1: Move outbound message, image, tool, function, and request wire structures with request conversion, token-limit selection, streaming selection, validation, and their tests into private `request.rs`.
- [x] Step 2: Move choice, response message, response tool call, usage, and response wire structures with normalization, finish-reason parsing, response parsing, and their tests into private `response.rs`.
- [x] Step 3: Move `Decoder`, its `StreamDecoder` implementation, usage and finish decoding, stream error handling, and streaming parity tests into private `stream.rs`.
- [x] Step 4: Make `mod.rs` declare the focused modules, implement `WireDialect` through them, and expose its decoder only to crate-internal dispatch.
- [x] Step 5: Run `cargo fmt --check`.
- [x] Step 6: Run `cargo test --lib dialect::openai_chat`.
- [x] Step 7: Commit with message `refactor: split OpenAI Chat dialect`.

### Task 6: Split Anthropic wire responsibilities → verify: `cargo test --lib dialect::anthropic` exits successfully and `src/dialect/anthropic/types.rs` does not exist

**Files:**
- Create: `src/dialect/anthropic/request.rs`
- Create: `src/dialect/anthropic/response.rs`
- Create: `src/dialect/anthropic/stream.rs`
- Modify: `src/dialect/anthropic/mod.rs`
- Delete: `src/dialect/anthropic/types.rs`

- [x] Step 1: Move outbound message, block, tool, and request wire structures with argument parsing, image-source handling, streaming selection, validation, and their tests into private `request.rs`.
- [x] Step 2: Move response, usage, and response-block conversion with status parsing, opaque-block preservation, response parsing, and their tests into private `response.rs`.
- [x] Step 3: Move `Decoder`, pending-thinking state, streamed block tracking, its `StreamDecoder` implementation, stream error handling, and streaming parity tests into private `stream.rs`.
- [x] Step 4: Make `mod.rs` declare the focused modules, implement `WireDialect` through them, and expose its decoder only to crate-internal dispatch.
- [x] Step 5: Run `cargo fmt --check`.
- [x] Step 6: Run `cargo test --lib dialect::anthropic`.
- [x] Step 7: Commit with message `refactor: split Anthropic dialect`.

### Task 7: Split Gemini wire responsibilities → verify: `cargo test --lib dialect::gemini` exits successfully and `src/dialect/gemini/types.rs` does not exist

**Files:**
- Create: `src/dialect/gemini/request.rs`
- Create: `src/dialect/gemini/response.rs`
- Create: `src/dialect/gemini/stream.rs`
- Modify: `src/dialect/gemini/mod.rs`
- Delete: `src/dialect/gemini/types.rs`

- [x] Step 1: Move generation configuration, tool-choice mapping, request wire structures, request conversion, transcript flushing, result parsing, streaming selection, validation, and their tests into private `request.rs`.
- [x] Step 2: Move response and usage wire structures, step conversion, unknown-step preservation, response parsing, and their tests into private `response.rs`.
- [x] Step 3: Move `Step`, streamed-value merging, `Decoder`, its `StreamDecoder` implementation, and streaming parity tests into private `stream.rs`.
- [x] Step 4: Make `mod.rs` declare the focused modules, implement `WireDialect` through them, and expose its decoder only to crate-internal dispatch.
- [x] Step 5: Run `cargo fmt --check`.
- [x] Step 6: Run `cargo test --lib dialect::gemini`.
- [x] Step 7: Commit with message `refactor: split Gemini dialect`.

### Task 8: Migrate consumers and lock the public contract → verify: all repository checks exit successfully and obsolete public names occur only in migration documentation

**Files:**
- Create: `tests/public_api.rs`
- Modify: `src/client.rs`
- Modify: `src/endpoint/mod.rs`
- Modify: `src/stream/assembler.rs`
- Modify: `src/stream/event.rs`
- Modify: `tests/streaming_transport.rs`
- Modify: `tests/typed_output.rs`
- Modify: `examples/chat.rs`
- Modify: `examples/custom_endpoint.rs`
- Modify: `examples/images.rs`
- Modify: `examples/portable.rs`
- Modify: `examples/retry.rs`
- Modify: `examples/simple.rs`
- Modify: `examples/streaming.rs`
- Modify: `examples/structured_output.rs`
- Modify: `examples/tool_loop.rs`
- Modify: `README.md`
- Modify: `docs/README.md`
- Modify: `docs/getting-started.md`
- Modify: `docs/introduction.md`
- Modify: `docs/concepts.md`
- Modify: `docs/internals/architecture.md`
- Modify: `docs/internals/adding-a-dialect.md`
- Modify: `docs/internals/capability-model.md`
- Modify: `docs/providers/README.md`
- Modify: `docs/providers/anthropic.md`
- Modify: `docs/providers/custom.md`
- Modify: `docs/providers/gemini.md`
- Modify: `docs/providers/openai.md`
- Modify: `docs/providers/openai-chat.md`
- Modify: `docs/reference/client.md`
- Modify: `docs/reference/errors.md`
- Modify: `docs/reference/messages.md`
- Modify: `docs/reference/requests.md`
- Modify: `docs/reference/streaming.md`
- Modify: `docs/reference/wire/anthropic.md`
- Modify: `docs/reference/wire/gemini.md`
- Modify: `docs/reference/wire/openai-chat.md`
- Modify: `docs/reference/wire/openai.md`

- [ ] Step 1: Update examples, integration tests, crate docs, and reference docs using the exact public rename map:

  ```text
  ProviderDialect -> Dialect
  ProviderConfig  -> EndpointConfig
  ProviderType    -> EndpointPreset
  ProviderError   -> Error
  error field provider -> endpoint
  error accessor provider() -> endpoint()
  ```

- [ ] Step 2: Rewrite architecture and dialect-extension documentation to describe the new module ownership and private `WireDialect` trait; remove instructions that refer to `src/provider`; qualify the rustdoc links in `src/client.rs` and `src/endpoint/mod.rs` as `crate::EndpointPreset`, `crate::Client::from_env`, and `crate::Client` so the required documentation build resolves them without module-local imports.
- [ ] Step 3: Add `tests/public_api.rs` with one compile-time test importing representative public types from both supported styles:

  ```rust
  use freyja::{Client, Dialect, EndpointConfig, EndpointPreset, Error, GenerateRequest};
  use freyja::dialect::Dialect as CategorizedDialect;
  use freyja::endpoint::{EndpointConfig as CategorizedEndpointConfig, EndpointPreset as CategorizedEndpointPreset};
  use freyja::error::Error as CategorizedError;
  use freyja::model::GenerateRequest as CategorizedGenerateRequest;
  use freyja::stream::StreamEvent;
  ```

  Construct or type-check each import so unused-import warnings cannot hide an inaccessible path.
- [ ] Step 4: Add a migration section to `README.md` that contains the old names only as a direct rename table; all other prose and code use the new vocabulary.
- [ ] Step 5: Run `cargo fmt --check`.
- [ ] Step 6: Run `cargo test --all-targets` outside the managed sandbox so loopback integration tests can bind.
- [ ] Step 7: Run `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps`.
- [ ] Step 8: Run `cargo clippy --all-targets -- -D warnings`.
- [ ] Step 9: Run `rg -n 'freyja::provider|ProviderDialect|ProviderConfig|ProviderType|ProviderError' --glob '!docs/specs/**' --glob '!docs/plans/**'`; confirm every match belongs to the explicit migration section.
- [ ] Step 10: Run `test ! -e src/provider`.
- [ ] Step 11: Commit with message `docs: migrate to modular public API`.
