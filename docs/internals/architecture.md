# Architecture

## Layout

```text
├── Cargo.toml
├── examples/               # runnable consumer examples
├── tests/                  # integration and public-API checks
└── src/
    ├── lib.rs              # crate docs and public re-exports
    ├── client.rs           # request lifecycle and dialect dispatch
    ├── dialect/
    │   ├── mod.rs          # public Dialect and private WireDialect
    │   ├── refusal.rs      # format-level capability refusals
    │   ├── openai_responses/
    │   ├── openai_chat/
    │   ├── gemini/
    │   └── anthropic/
    ├── endpoint/
    │   ├── mod.rs          # EndpointConfig, Auth, URL and model resolution
    │   └── presets.rs      # maintained EndpointPreset values
    ├── error.rs            # Error and TransportError
    ├── model/              # neutral request and response model
    ├── stream/             # SSE framing, events, decoding, assembly
    └── transport/          # HTTP transport support
```

The crate supports both root re-exports, such as `freyja::EndpointConfig`, and categorized paths, such as `freyja::endpoint::EndpointConfig`. The categorized modules make ownership clear; the root exports keep ordinary applications concise.

`Dialect::path()` is appended to `EndpointConfig::base_url`. Version prefixes, such as Gemini's `v1beta`, belong in the endpoint URL rather than in the dialect.

## Ownership

`model` is neutral: it does not know which endpoint or wire format will receive a request. `endpoint` answers where to send it, how to authenticate, and which defaults apply. `dialect` converts the neutral model to and from a specific wire format. `client` owns the shared request lifecycle: conversion, HTTP, status classification, parsing, and streaming dispatch.

When a dialect cannot represent a requested capability, it returns `Error::UnsupportedCapability` before any HTTP request. It must not silently drop a caller's request. Endpoint or model-specific rejection remains an endpoint response and is classified from its HTTP status.

## Private wire implementation

`Dialect` is the public, closed selector. Each private dialect module implements the crate-private `WireDialect` trait:

```rust
pub(crate) trait WireDialect: Send + Sync {
    type Request: serde::Serialize + Send;

    fn build(
        &self,
        request: &GenerateRequest,
        config: &EndpointConfig,
    ) -> Result<Self::Request, Error>;

    fn parse(
        &self,
        body: &str,
        config: &EndpointConfig,
    ) -> Result<GenerateResponse, Error>;
}
```

The trait is deliberately private. Consumers configure a `Dialect`; they do not implement a public plugin interface. `Client` selects the matching private module for generation and its matching decoder for streaming.

Each dialect module separates request conversion, response conversion, and stream decoding into private focused files. This keeps the wire representation inside the dialect boundary while the public model and streaming events remain stable.

## Streaming

`stream` owns byte-safe SSE framing, the public `StreamEvent` and `EventStream`, and shared response assembly. A dialect decoder translates its native frames into neutral events. `Client::stream` checks the HTTP status before it returns an `EventStream`, so rejected requests surface as `Error` from `stream()`; failures after the stream starts surface from `next()`.

Calling `into_response()` before a stream drains returns `Error::Stream`. This prevents a partial answer from being replayed as a completed turn.

## Design rules

### The neutral model never bends to a vendor

Adding a dialect should translate existing model concepts, not make one vendor's vocabulary public. A field that only one endpoint needs belongs in `EndpointConfig::body` or `GenerateRequest::extra_for`; a field that has the same meaning across formats may belong in the neutral model.

### No silent degradation

Refuse only when the wire format has no way to express a requested capability. A deployment's policy or a model's limits are not format-level refusals.

### No invented defaults

`GenerateRequest::new()` leaves optional behavior unset. An `EndpointConfig` may provide endpoint-specific defaults such as a model, but the request model does not guess a vendor-specific value.
