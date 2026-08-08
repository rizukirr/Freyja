# Adding a provider

There are two different jobs behind that phrase, and they cost very different amounts of work. Work out which one you have before writing anything.

| You want to reach | You need | Effort |
|---|---|---|
| A vendor whose API copies OpenAI or Anthropic | Nothing, just `Client::custom` | One call |
| A vendor with its own wire format | A **dialect**, a new module | A day |

Most hosted inference APIs fall in the first row. They copy an existing format so that existing client libraries work unchanged, so Freya usually already speaks to them and only needs the URL.

## Reaching a compatible endpoint

No code required. Build a `ProviderConfig` and pass it where a `ProviderType` would go:

```rust
let config = ProviderConfig::new(ProviderDialect::Anthropic, "my-gateway", "https://gw.test/v1")
    .api_key_env("GATEWAY_API_KEY")
    .default_model("claude-opus-5");

let client = Client::from_env(config).expect("GATEWAY_API_KEY");
```

Full detail in [Custom endpoints](../providers/custom.md).

Do not send a pull request adding it to `src/provider/presets.rs`. That file covers the three first-party vendors Freya tests against, deliberately, because a preset is a standing promise that a URL and a default model are still current. Widely used endpoints belong in the table in [Custom endpoints](../providers/custom.md), where the caveat that nothing verifies them can be stated honestly.

## Adding a dialect

Only worth doing when the vendor's format is genuinely its own. Everything below is `pub(crate)`; wire types must never escape the crate, or callers start depending on vendor shapes and the neutral model stops being the boundary.

### 1. Create the module

Name it after the format, not the vendor, because a dialect usually outlives its author. `openai_chat` rather than `groq`.

```
src/provider/<dialect>/
├── mod.rs      the Provider impl, about 25 lines
└── types.rs    wire structs and conversions
```

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
- `config.name.clone()` is the `provider` field on every error you raise, so a failure reports the endpoint rather than the dialect.

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

### 6. Wire it into the enums

In `src/provider/mod.rs`, add the variant and its three properties:

```rust
pub enum ProviderDialect {
    OpenAiResponses,
    Gemini,
    Anthropic,
    MyDialect,
}
```

Then `path()`, `default_auth()`, and `required_headers()` each gain an arm, and `Client::generate` gains one dispatch arm. That is all. A preset is only warranted if the dialect belongs to a vendor Freya can test against, which so far means it usually is not.

### 7. Test it

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

### 8. Verify it live

**Offline tests prove Freya sends the JSON it meant to send, not that the vendor accepts it.** The Gemini dialect shipped with passing tests and three real bugs, including an input format that broke every multi-turn conversation. Point `examples/tool_loop.rs` at the new endpoint and run `cargo run --example tool_loop` before calling it done.

### 9. Document it

Two pages, matching the existing ones: a mapping page with the capability table and the field mapping, and a wire format page documenting the native JSON so users do not have to read vendor docs. Add both to the index in `docs/README.md`.

## Before you commit

- `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`
- Every public item documented, `#![deny(missing_docs)]` will tell you
- No `unwrap` on anything derived from a network response
- Live verification actually run, or the limitation stated plainly in the docs

## If the neutral model does not fit

Stop and reconsider before editing `model.rs`. Four dialects have landed without changing it, which is the main evidence the abstraction is holding.

The one time it did change was for opaque reasoning state, where Gemini, OpenAI, and Anthropic all independently require signed blocks replayed verbatim. That is the bar: a requirement that shows up in several vendors and cannot be expressed any other way. A field only one vendor wants belongs in `provider_metadata`.
