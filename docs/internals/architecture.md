# Architecture

## Layout

```
src/
├── lib.rs                  # crate docs and flat public re-exports
├── client.rs               # Client: conversion, transport, and dispatch
├── dialect/
│   ├── mod.rs              # public Dialect and private WireDialect
│   ├── refusal.rs          # capability refusals
│   ├── openai_responses/   # request, response, and stream conversions
│   ├── openai_chat/
│   ├── gemini/
│   └── anthropic/
├── endpoint/
│   ├── mod.rs              # EndpointConfig, Auth, and endpoint behaviour
│   └── presets.rs          # maintained EndpointPreset values
├── error.rs                # Error and TransportError
├── model/                  # neutral requests, responses, messages, and tools
├── stream/                 # SSE framing, decoding, assembly, and public events
└── transport/              # shared HTTP request and status handling
```

The crate exposes both a flat style and categorized modules. For example,
`freyja::EndpointConfig` and `freyja::endpoint::EndpointConfig` name the same
type. The categorized modules describe ownership; the flat re-exports keep
ordinary applications compact.

`EndpointConfig` owns deployment-specific details: its `Dialect`, base URL,
credentials, default model, and endpoint name. `EndpointPreset` supplies those
details for maintained endpoints. A `Dialect` owns only the wire format, so one
dialect can serve many compatible endpoints.

## Boundaries

The neutral model in `model/` does not know which wire formats exist. Dialect
modules translate between that model and vendor JSON. Their request, response,
and streaming wire types are crate-private, so callers cannot accidentally
depend on a particular endpoint format.

`dialect::WireDialect` is also crate-private. It defines the two operations the
client needs to share:

```rust
trait WireDialect {
    type Request: Serialize + Send;

    fn build(&self, request: &GenerateRequest, config: &EndpointConfig)
        -> Result<Self::Request, Error>;
    fn parse(&self, body: &str, config: &EndpointConfig)
        -> Result<GenerateResponse, Error>;
}
```

It is an implementation seam, not an extension API. `Client` dispatches over
the closed public `Dialect` enum and owns HTTP transport. This keeps request
conversion, status classification, connection reuse, and error attribution in
one place.

## Request flow

```
GenerateRequest
  -> Dialect selected by EndpointConfig
  -> private WireDialect::build
  -> transport::post
  -> private WireDialect::parse
  -> GenerateResponse
```

`Client::check` uses the same build path without sending a request. A dialect
may reject a request only when its wire format has nowhere to represent a
requested capability. Endpoint or model-specific validation remains the remote
service's responsibility.

`Error` carries the configured endpoint name in its `endpoint` field and
`endpoint()` accessor. A gateway is therefore reported by its own name instead
of the name of the wire format it happens to speak.

## Streaming

Streaming has three private layers beneath the public `EventStream` and
`StreamEvent` types:

```
response bytes
  -> stream::SseBuffer       frames, independent of dialect
  -> dialect::<name>::Decoder RawDelta values, dialect-specific
  -> stream::Assembler       neutral events and GenerateResponse
```

The decoder converts frames to private `RawDelta` values. The assembler then
coalesces text, completes tool-call arguments, and captures the normalized
response. Keeping those rules in one assembler makes a drained stream agree
with `Client::generate` across every dialect.

`SseBuffer` stores bytes, not strings, because a transport chunk can split a
UTF-8 codepoint. It interprets text only after a complete frame arrives.

## Adding support

Adding a compatible endpoint normally needs only an `EndpointConfig`; see
[Custom endpoints](../providers/custom.md). Adding a new wire format changes
the crate's private `dialect/` implementation and the closed `Dialect` dispatch
set. [Adding a dialect](adding-a-dialect.md) documents that contributor path.

## Testing

Dialect unit tests cover neutral-to-wire conversion, normalized responses,
capability refusals, and stream parity. `tests/streaming_transport.rs` uses a
local `TcpListener` to cover request URLs, status handling, and live byte
pumping. `tests/public_api.rs` compiles representative imports from both public
API styles, preserving the public contract independently of internal layout.
