---
title: Modular public API
date: 2026-08-17
status: draft
---

# Modular public API — Design

## Problem

Freyja's architecture separates a provider-neutral model, wire dialects, endpoint
configuration, shared transport, and streaming, but its source and public module
layout do not reflect those boundaries. `src/provider/mod.rs` combines endpoint
configuration, dialect dispatch, transport, and the client API, while
`src/provider/model.rs` combines every neutral request, response, tool, schema,
and error type. Each dialect's `types.rs` also combines wire schemas with inbound
and outbound conversion.

The public name `provider` compounds the problem by referring at different times
to a wire format, a configured endpoint, and an internal conversion
implementation. Consumers can use concise crate-root imports, but the categorized
path `freyja::provider::*` gives unrelated concepts the same owner.

## Goals

- A consumer can import every common type through `freyja::{...}` after the
  refactor, demonstrated by compiling all examples and doctests.
- A consumer can discover the same public types through focused `model`,
  `dialect`, `endpoint`, `stream`, and `error` modules, demonstrated by a
  compile-time integration test using categorized imports.
- Public names distinguish wire formats from configured services:
  `ProviderDialect` becomes `Dialect`, `ProviderConfig` becomes
  `EndpointConfig`, `ProviderType` becomes `EndpointPreset`, and
  `ProviderError` becomes `Error`.
- Source files give the neutral model, endpoint configuration, dialect
  conversion, transport, client, streaming, and errors separate ownership,
  verified by the target module layout and the absence of a public `provider`
  module.
- Request JSON, response normalization, validation, error classification,
  streaming assembly, and secret redaction remain behaviorally unchanged,
  verified by the existing unit and integration test suite.
- Documentation and examples use the new vocabulary, verified by repository
  search for obsolete public paths and names outside migration documentation.

## Non-goals

- Adding or removing an endpoint or wire dialect.
- Changing request or response wire JSON.
- Adding dependencies or splitting the package into a Cargo workspace.
- Introducing provider feature flags.
- Changing runtime behavior or performance.
- Preserving the old API through aliases or a deprecated compatibility module.

## Constraints

- Freyja remains one Cargo crate.
- The change deliberately breaks the public API while the crate is at `0.1.x`.
- The ergonomic crate-root API remains the primary documented interface.
- Wire request and response structures, dialect decoders, SSE framing, response
  assembly, HTTP transport, and the dialect conversion trait remain private.
- `#![deny(missing_docs)]` continues to apply to the public API.
- API keys and secret-looking headers remain redacted from `Debug` output.
- `GenerateResponse::provider_metadata` keeps its current name because it
  describes opaque provider-returned data rather than configuration ownership.

## Approach

The user chose a curated crate root plus focused public modules. This is the
larger framing considered during design: both the internal organization and the
public vocabulary change. The rejected smaller framing would have reorganized
only private files and preserved the overloaded `provider` namespace.

The target top-level layout is:

```text
src/
├── lib.rs
├── client.rs
├── model/
├── dialect/
├── endpoint/
├── stream/
├── error.rs
└── transport/
```

The public modules own these concepts:

- `model` owns provider-neutral requests, messages, responses, tools, and JSON
  Schema helpers.
- `dialect` exposes `Dialect`, the identity and public properties of a wire
  format. Private submodules implement each supported format.
- `endpoint` exposes `EndpointConfig`, `EndpointPreset`, `Auth`, and
  `TokenLimitField`.
- `stream` exposes `EventStream` and `StreamEvent`. SSE framing, dialect
  decoding, and assembly remain private.
- `error` exposes `Error` and `TransportError`.
- `client.rs` exposes `Client`; the crate root re-exports it and all commonly
  used public types.

Normal usage stays concise:

```rust
use freyja::{
    Client, Dialect, EndpointConfig, EndpointPreset, Error,
    GenerateRequest, Message, Role, StreamEvent,
};
```

Consumers that prefer categorized imports can use:

```rust
use freyja::dialect::Dialect;
use freyja::endpoint::{Auth, EndpointConfig, EndpointPreset, TokenLimitField};
use freyja::error::{Error, TransportError};
use freyja::model::{GenerateRequest, Message, Role};
use freyja::stream::{EventStream, StreamEvent};
```

The internal conversion trait is renamed from the public-sounding `Provider` to
an implementation-oriented private name such as `WireDialect`. It converts a
neutral request to a dialect request and parses a successful dialect response.
HTTP execution does not belong to that trait.

Non-streaming data continues to flow through one path:

```text
GenerateRequest
  -> Client
  -> private dialect adapter and outbound validation
  -> shared HTTP transport
  -> dialect response conversion
  -> GenerateResponse
```

Streaming continues through the parallel shared path:

```text
GenerateRequest
  -> Client
  -> private dialect adapter and streaming wire request
  -> shared HTTP transport
  -> SSE framing
  -> dialect decoder
  -> shared assembler
  -> StreamEvent and final GenerateResponse
```

`Client::check` uses the same outbound conversion as network requests.
`Client` continues to own the reusable `reqwest::Client`. Endpoint configuration
continues to supply URL, authentication, headers, defaults, and endpoint-level
body fields.

`ProviderError` becomes `Error` without changing its semantic variants. Variant
fields named `provider` become `endpoint`, because they identify the configured
service rather than its wire dialect. Retry classification, retry delay parsing,
raw diagnostic bodies, and `TransportError` behavior remain unchanged.

The implementation moves code mechanically before modifying names. It first
separates neutral model modules, then endpoint and error ownership, then client
and transport, then dialect directories, and finally streaming. Tests move with
the code they exercise. Public re-exports are updated once the owning modules
exist, followed by examples and documentation.

## Alternatives considered

### Curated root only

All implementation modules could remain private while the crate root re-exported
the renamed types. This creates the smallest public contract and is the laziest
viable structure. It was rejected because focused modules improve discovery as
the library grows, while common imports remain equally concise at the root.

### Fully namespaced API

The crate could expose focused modules and re-export only `Client` and `Error` at
the root. This enforces strict organization but makes routine imports verbose and
conflicts with the chosen ergonomic `freyja::{...}` style.

### Preserve `freyja::provider`

The existing public module and names could remain as deprecated compatibility
aliases. This would reduce immediate migration work but preserve the ambiguous
vocabulary and expand the documented API surface. The project will instead make
the break explicit at `0.1.x` and document a direct rename table.

## Testing

- Move existing unit tests with their owning modules and run the full suite.
- Update integration tests, examples, and doctests to the new imports.
- Add a compile-time integration test that imports representative types from
  each focused public module and from the crate root.
- Run `cargo test --all-targets`.
- Run `cargo doc --no-deps` with warnings treated as errors where supported by
  the existing toolchain.
- Run `cargo fmt --check`.
- Run `cargo clippy --all-targets -- -D warnings`.
- Search the repository for `freyja::provider`, `ProviderDialect`,
  `ProviderConfig`, `ProviderType`, and `ProviderError`; allow occurrences only
  in explicit migration documentation.
- Compare existing dialect serialization and streaming parity tests before and
  after the move to confirm the refactor does not alter wire behavior.

## Open questions

N/A — the public vocabulary, compatibility policy, module ownership, data flow,
error behavior, and verification scope were approved during design.
