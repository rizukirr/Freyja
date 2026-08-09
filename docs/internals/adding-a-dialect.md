# Adding a provider

There are two different jobs behind that phrase, and they cost very different amounts of work. Work out which one you have before writing anything.

| You want to reach | You need | Effort |
|---|---|---|
| A vendor whose API copies OpenAI or Anthropic | Nothing, just `Client::custom` | One call |
| A vendor with its own wire format | A **dialect**, a new module | A day |

Most hosted inference APIs fall in the first row. They copy an existing format so that existing client libraries work unchanged, so Freyja usually already speaks to them and only needs the URL.

## Reaching a compatible endpoint

No code required. Build a `ProviderConfig` and pass it where a `ProviderType` would go:

```rust
let config = ProviderConfig::new(ProviderDialect::Anthropic, "my-gateway", "https://gw.test/v1")
    .api_key_env("GATEWAY_API_KEY")
    .default_model("claude-opus-5");

let client = Client::from_env(config).expect("GATEWAY_API_KEY");
```

Full detail in [Custom endpoints](../providers/custom.md).

Do not send a pull request adding it to `src/provider/presets.rs`. That file covers the three first-party vendors Freyja tests against, deliberately, because a preset is a standing promise that a URL and a default model are still current. Widely used endpoints belong in the table in [Custom endpoints](../providers/custom.md), where the caveat that nothing verifies them can be stated honestly.

## Adding a dialect

Only worth doing when the vendor's format is genuinely its own. Everything below is `pub(crate)`; wire types must never escape the crate, or callers start depending on vendor shapes and the neutral model stops being the boundary.

### 1. Create the module

Name it after the format, not the vendor, because a dialect usually outlives its author. `openai_chat` rather than `groq`.

```
src/provider/<dialect>/
├── mod.rs      the Provider impl, a couple of dozen lines, plus the stream decoder
└── types.rs    wire structs and conversions
```

The decoder dominates `mod.rs`: the existing ones run from about 130 lines for `openai_chat` to about 240 for `anthropic`, of which the `Provider` impl is roughly twenty.

Two conversions, then, not one. `types.rs` maps the neutral model onto the vendor's request and response bodies, and `mod.rs` also decodes that vendor's SSE frames. A dialect without the decoder compiles and streams nothing, so treat both as part of the job.

Use `openai_responses/` as the template for a format that flattens tool calls into a sibling list, and `anthropic/` for one that nests them inside messages. Which shape you are facing is the first thing to work out, and it decides most of the file.

### 2. Write the wire types

In `types.rs`, define serde structs matching the vendor's request and response bodies. Use `skip_serializing_if` on every optional field so unset values are omitted rather than sent as null. There are no `PROVIDER` or `DEFAULT_MODEL` constants; both belong to the endpoint and arrive through `ProviderConfig`.

```rust
#[derive(Serialize)]
pub struct Request {
    model: String,
    messages: Vec<MessageWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
}
```

### 3. Convert outbound

```rust
impl Request {
    pub(crate) fn build(
        value: &GenerateRequest,
        config: &ProviderConfig,
    ) -> Result<Self, ProviderError> {
        Ok(Self {
            model: config.model_for(value)?,
            // ...
        })
    }
}
```

Two things come from `config` rather than from a constant:

- `config.model_for(value)?` resolves the model, preferring the request's own choice and falling back to the endpoint default. It errors when neither is set, rather than inventing a name.
- `config.name.clone()` is the `provider` field on every error you raise, so a failure reports the endpoint rather than the dialect. This holds everywhere, not just here: the decoder has no `ProviderConfig`, so it is handed the same name as an argument. Never write a dialect literal.

Refuse what the format cannot express, rather than dropping it:

```rust
return Err(ProviderError::UnsupportedCapability {
    provider: config.name.clone(),
    capability: "server-side conversation continuation",
});
```

### 4. Convert inbound

`From<Response> for GenerateResponse` stays infallible. The body already parsed, so anything unrecognized is preserved rather than rejected.

- Normalize the vendor's termination signal into `ResponseStatus`, falling back to `ResponseStatus::Other(..)` rather than guessing at a near neighbour.
- Normalize usage onto `Usage`, whatever the vendor names its fields, and compute a total if it does not report one.
- Put unmodeled fields into `provider_metadata`.
- Map unknown content blocks to `OutputContent::Reasoning` rather than skipping them, so signed reasoning state survives into the next request.

Then a small `parse` entry point that attributes failures to the endpoint:

```rust
pub(crate) fn parse(body: &str, config: &ProviderConfig) -> Result<GenerateResponse, ProviderError> {
    let wire: Response = serde_json::from_str(body)
        .map_err(|error| ProviderError::InvalidResponse {
            provider: config.name.clone(),
            message: format!("{error}; body: {body}"),
        })?;
    Ok(wire.into())
}
```

### 5. Implement the trait

There is no transport step. `Client` owns convert, POST, check status, parse for every dialect, so `mod.rs` only wires the two conversions together:

```rust
pub(crate) struct MyProvider;

impl Provider for MyProvider {
    type Request = types::Request;

    fn build(&self, request: &GenerateRequest, config: &ProviderConfig)
        -> Result<Self::Request, ProviderError> {
        types::Request::build(request, config)
    }

    fn parse(&self, body: &str, config: &ProviderConfig)
        -> Result<GenerateResponse, ProviderError> {
        types::parse(body, config)
    }
}
```

### 6. Decode the stream

Streaming is not part of the `Provider` trait. Add a `streaming()` method on the request type that sets whatever the vendor wants, and a `Decoder` in `mod.rs` implementing `StreamDecoder`:

```rust
pub(crate) trait StreamDecoder: Send {
    fn decode(
        &mut self,
        frame: &SseFrame,
        provider: &Arc<str>,
        out: &mut Vec<RawDelta>,
    ) -> Result<(), ProviderError>;

    fn normalizes_tool_arguments(&self) -> bool { false }
}
```

`provider` is the endpoint's configured name, passed in because a decoder has no `ProviderConfig` to read it from. Use it verbatim in any error you raise, never a dialect literal, so a Claude-compatible gateway reports itself and not "anthropic". That is the invariant documented on `ProviderError` in `src/provider/model.rs`, and it is the only reason the argument exists.

You translate frames into `RawDelta`s. Assembling them into events, buffering partial tool arguments, and building the final `GenerateResponse` are shared and already done, so a decoder is a `match` on the vendor's event names and nothing else. Keep it stateless if the frames allow it, as `openai_chat` does, and hold state only for what the vendor spreads across frames, as `gemini` does for its steps.

**If the vendor has a mid-stream error frame, decode it into `ProviderError::Stream`.** Check the vendor's event list for one; `openai_responses` and `anthropic` both have it, and both return:

```rust
"error" => {
    return Err(ProviderError::Stream {
        // The endpoint's own name, never the dialect.
        provider: provider.clone(),
        message: value["message"].as_str().unwrap_or("unknown streaming error").to_string(),
    });
}
```

Miss this and the frame falls through the catch-all arm, the body then closes, and the assembler emits a perfectly ordinary `Done`. The caller sees a short answer and no error at all. `ProviderError::Stream` exists for exactly this case and for a body that ends early — it is distinct from `Api`, which reports a non-success HTTP status before the stream begins, and from `InvalidResponse`, which reports a body that would not parse.

Four more things decide whether the decoder is correct, and all four are places to copy the parser rather than think afresh:

- **Status mapping, arm for arm with `parse`.** Same strings, same neutral variants, same fallback to `ResponseStatus::Other`. Do not share a mapping with another dialect; the strings differ per vendor, and the copies are meant to be read side by side with their own parser.
- **Usage, computed the same way.** If the parser defaults missing fields to zero, default to zero. If it computes a total the vendor does not report, compute it here too.
- **Unmodeled blocks preserved as replayable blobs.** The parser's catch-all maps unknown content onto `OutputContent::Reasoning`; the decoder does the same with `RawDelta::ReasoningBlob`. A vendor whose block streams its payload in deltas needs those merged back into the blob before it is emitted, or you replay a truncated one.
- **Tool arguments normalized identically.** `normalizes_tool_arguments` says whether the parser re-serializes arguments from parsed JSON, which sorts keys and strips whitespace, or hands back the model's own string. Anthropic and Gemini do the first, the OpenAI dialects the second. Get this wrong and a drained stream stops matching `generate` on a byte level while looking right.

### 7. Wire it into the enums

In `src/provider/mod.rs`, add the variant and its three properties:

```rust
pub enum ProviderDialect {
    OpenAiResponses,
    OpenAiChat,
    Gemini,
    Anthropic,
    MyDialect,
}
```

Then `path()`, `default_auth()`, `required_headers()`, and `stream_query()` each gain an arm, and `Client::generate` and `Client::stream` gain one dispatch arm each. `stream_query()` returns `None` for a dialect selected by the body alone, and `Some("alt=sse")` for one like Gemini that also needs it on the URL. That is all. A preset is only warranted if the dialect belongs to a vendor Freyja can test against, which so far means it usually is not.

### 8. Test it

Conversion tests need a config. Build one the way a caller would, unless the dialect has a preset:

```rust
fn config() -> ProviderConfig {
    ProviderConfig::new(ProviderDialect::MyDialect, "test-endpoint", "https://api.test/v1")
        .default_model("test-model")
}

#[test]
fn maps_a_full_tool_round_trip() {
    let json = serde_json::to_value(Request::build(&request, &config()).unwrap()).unwrap();
    // assert on the wire shape
}
```

Cover, at minimum: a plain request, a full tool round trip, capabilities the format refuses, and a response normalizing back into the neutral model.

### 9. Prove the decoder agrees with the parser

Add `streamed_response_matches_generate` to the dialect's tests, in the shape the other four use. Feed recorded frames through `stream::drain_for_test`, then assert the drained `GenerateResponse` equals what `parse` produces from the non-streaming body describing the same turn: id, model, status, content part for part, and usage.

**Every streaming defect found in review came from a decoder disagreeing with its parser, not from framing or transport.** A sorted-key difference in tool arguments, a status string mapped one way in `parse` and another in `decode`, a block the parser preserves and the decoder drops. None of those fail a decoder-only test, because a decoder-only test asserts what the decoder does rather than what the parser does. The parity test is the one that catches them, and it is why the parser is the specification: when the two disagree, the decoder is wrong.

Make the fixture carry the awkward cases on purpose. Two adjacent text blocks, so block boundaries have to survive. Arguments split mid-token across frames, in non-alphabetical key order. A block type the dialect does not model. A terminal status that is not `completed`.

### 10. Verify it live

**Offline tests prove Freyja sends the JSON it meant to send, not that the vendor accepts it.** The Gemini dialect shipped with passing tests and three real bugs, including an input format that broke every multi-turn conversation. Point `examples/tool_loop.rs` at the new endpoint and run `cargo run --example tool_loop` before calling it done. Run `cargo run --example streaming` against it too: recorded frames prove the decoder handles the frames you recorded, not that the vendor sends those frames.

### 11. Document it

Two pages, matching the existing ones: a mapping page with the capability table and the field mapping, and a wire format page documenting the native JSON so users do not have to read vendor docs. Add both to the index in `docs/README.md`. The capability table has a `Streaming` row; say how this dialect selects it and anything a caller has to know, the way [Gemini](../providers/gemini.md) and [OpenAI Chat Completions](../providers/openai-chat.md) do.

## Before you commit

- `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`
- Every public item documented, `#![deny(missing_docs)]` will tell you
- A `streamed_response_matches_generate` test present and passing for the dialect
- The vendor's mid-stream error frame decoded into `ProviderError::Stream`, carrying the `provider` argument, or a note saying the format has no such frame
- No `unwrap` on anything derived from a network response
- Live verification actually run, or the limitation stated plainly in the docs

## If the neutral model does not fit

Stop and reconsider before editing `model.rs`. Four dialects have landed without changing it, which is the main evidence the abstraction is holding.

The one time it did change was for opaque reasoning state, where Gemini, OpenAI, and Anthropic all independently require signed blocks replayed verbatim. That is the bar: a requirement that shows up in several vendors and cannot be expressed any other way. A field only one vendor wants belongs in `provider_metadata`.
