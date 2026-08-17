# Adding a dialect

A dialect is a wire format Freyja maintains internally. It is not an application extension point: `WireDialect` and every wire module are crate-private. Add one only when a format is stable enough for Freyja to test and document.

For a new endpoint that already speaks a supported format, add no dialect. Build an `EndpointConfig` instead:

```rust
use freyja::{Dialect, EndpointConfig};

let config = EndpointConfig::new(
    Dialect::Anthropic,
    "my-gateway",
    "https://gw.test/v1",
)
.default_model("test-model");
```

## 1. Create the dialect module

Create `src/dialect/<dialect>/` with private `request.rs`, `response.rs`, and `stream.rs` modules plus a small `mod.rs`. Keep wire structs and conversions private: callers use `GenerateRequest`, `GenerateResponse`, and `StreamEvent`, never native request or response types.

The request module converts `GenerateRequest` into the serde-serializable native body and returns `Error` when the format cannot express a requested capability. Attribute errors to `config.name.clone()` through the `endpoint` field:

```rust
return Err(Error::UnsupportedCapability {
    endpoint: config.name.clone(),
    capability: "schema-less JSON response format",
});
```

The response module parses the endpoint body and converts it to `GenerateResponse`. Preserve unknown response data according to the established dialect pattern and use `Error::InvalidResponse` for an unusable successful body.

## 2. Implement the private trait

In the new module's `mod.rs`, implement `super::WireDialect` for a zero-sized dialect type. Its `build` delegates to the request module and its `parse` delegates to the response module. The trait is crate-private, so do not expose the type or its wire types.

Add the module declaration, a `Dialect` variant, its `path()`, `default_auth()`, `required_headers()`, and `stream_query()` arms in `src/dialect/mod.rs`. Update the internal dispatch there so `Client` selects the new builder, parser, and decoder.

Add an `EndpointPreset` only for an endpoint Freyja can maintain and test. A preset is a maintained `EndpointConfig`, not a requirement for using the dialect.

## 3. Decode streams

Implement the dialect decoder in `stream.rs` using the crate-private `StreamDecoder` interface. Its endpoint-name argument must be used verbatim for errors:

```rust
return Err(Error::Stream {
    endpoint: endpoint.clone(),
    message: "the endpoint reported a stream error".into(),
});
```

The decoder must agree with the non-streaming parser for status, usage, text and refusal boundaries, opaque reasoning data, and tool-call argument handling. Use the existing dialect parity tests as the template.

## 4. Test and document

Add focused conversion tests in the dialect module and a streamed-response parity test. Cover a plain request, tool round trip, unsupported capabilities, and unknown response data appropriate to the format.

Run:

```bash
cargo test
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

Then add a provider page, wire-format reference, and documentation index entries. Live verification remains necessary before claiming an endpoint accepts its requests: recorded tests prove the JSON Freyja generated, not that a vendor accepts it.
