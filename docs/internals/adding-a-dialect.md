# Adding a dialect

Most new services do not need a new dialect. If a service speaks an existing
wire format, configure it with an `EndpointConfig`:

```rust
use freyja::{Client, Dialect, EndpointConfig};

let config = EndpointConfig::new(
    Dialect::Anthropic,
    "my-gateway",
    "https://gw.test/v1",
)
.api_key_env("GATEWAY_API_KEY")
.default_model("claude-opus-5");

let client = Client::from_env(config).expect("GATEWAY_API_KEY");
```

See [Custom endpoints](../providers/custom.md) for the supported configuration
options. Do not add a maintained `EndpointPreset` for a service Freyja cannot
regularly verify.

A genuinely new wire format is a crate contribution. The public `Dialect` enum
is closed and its internal `WireDialect` trait is private, so downstream crates
cannot register a dialect dynamically. This section describes the module work
needed inside Freyja.

## 1. Create the dialect module

Create `src/dialect/<name>/` with this layout:

```
src/dialect/<name>/
├── mod.rs       # private WireDialect implementation and stream Decoder
├── request.rs   # request wire types and conversion
├── response.rs  # response wire types and normalization
└── stream.rs    # SSE frame decoding
```

Keep all vendor-shaped types in this module. The neutral types live in
`src/model/`; do not add vendor fields there merely to make one conversion
easier.

## 2. Add closed dispatch

Add a public `Dialect` variant in `src/dialect/mod.rs`, then update its
`path`, authentication defaults, required headers, and streaming query where
the format requires them. Add the corresponding arm to the internal dispatch
and decoder selection. A maintained service may also need an `EndpointPreset`
in `src/endpoint/presets.rs`; a compatible third-party endpoint does not.

## 3. Implement conversion

Each module implements private `WireDialect` for its marker type. `build`
receives `GenerateRequest` and `EndpointConfig`, validates only format-level
constraints, and returns a serializable private request type. `parse` receives
the response body and returns `GenerateResponse`.

Use the endpoint name for every error:

```rust
return Err(Error::UnsupportedCapability {
    endpoint: config.name.clone(),
    capability: "portable reasoning effort levels",
});
```

Never attribute an error to a dialect literal. The endpoint may be a gateway
whose name tells callers which deployment failed.

Unknown response fields belong in `provider_metadata`, and opaque reasoning
state belongs in `OutputContent::Reasoning`. Preserve them rather than rejecting
a successful response only because Freyja does not yet model a vendor detail.

## 4. Implement streaming

Implement the crate-private `stream::StreamDecoder` for the dialect decoder.
Decode each SSE frame into `RawDelta` values. The decoder receives the endpoint
name because a mid-stream error has no `EndpointConfig` to inspect:

```rust
fn decode(
    &mut self,
    frame: &SseFrame,
    endpoint: &Arc<str>,
    out: &mut Vec<RawDelta>,
) -> Result<(), Error>;
```

Map the vendor's terminal state to `ResponseStatus` and map a vendor error frame
to `Error::Stream { endpoint: endpoint.clone(), ... }`. Let `Assembler` own
text coalescing, tool-argument completion, and the final `GenerateResponse`;
decoders should describe frames rather than recreate shared stream policy.

## 5. Test the contract

Add focused tests beside the dialect for:

- request conversion and each capability refusal;
- response normalization and opaque-state preservation;
- streamed response parity with the equivalent non-streaming response;
- malformed or vendor-error stream frames, when the format has them.

Update `tests/streaming_transport.rs` only when the HTTP-level request contract
changes, such as a dialect-specific query parameter. Run the repository checks
before submitting the change, including the public API import test.
