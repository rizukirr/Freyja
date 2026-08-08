# Adding a provider

Adding a backend is additive. You write one directory and touch two enums. Nothing in the neutral model changes, and no existing provider is affected.

This page used Anthropic as its worked example before that backend existed. It now does, and it landed exactly as described here, one new directory and two enum arms with no edits to `model.rs`. So `src/provider/anthropic/` is a real reference for every step below, not a sketch.

Use `src/provider/openai/` as the template for a vendor that flattens tool calls into a sibling list, and `src/provider/anthropic/` for one that nests them inside messages. Which shape you are facing is the first thing to work out.

## The steps

### 1. Create the module

```
src/provider/anthropic/
├── mod.rs      # transport
└── types.rs    # wire types and conversions
```

Register it in `src/provider/mod.rs`:

```rust
pub(crate) mod anthropic;
```

Keep it `pub(crate)`. Wire types must never escape the crate, or callers start depending on vendor shapes and the neutral model stops being the boundary.

### 2. Write the wire types

In `types.rs`, define serde structs matching the vendor's request and response bodies. Two constants at the top keep the rest readable:

```rust
const PROVIDER: &str = "Anthropic";
const DEFAULT_MODEL: &str = "claude-sonnet-5";
```

Use `skip_serializing_if` on every optional field so unset values are omitted rather than sent as null:

```rust
#[derive(Serialize)]
pub struct Request {
    model: String,
    messages: Vec<MessageWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ToolWire>,
}
```

On the response side, capture unknown fields instead of dropping them, and tolerate unknown variants:

```rust
#[derive(Deserialize)]
pub struct Response {
    id: String,
    #[serde(default)]
    content: Vec<ContentWire>,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ContentWire {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(other)]
    Unknown,
}
```

The `#[serde(other)]` arm is what keeps a vendor shipping a new block type from breaking your build.

### 3. Convert outbound

```rust
impl TryFrom<&GenerateRequest> for Request {
    type Error = ProviderError;

    fn try_from(value: &GenerateRequest) -> Result<Self, Self::Error> { /* ... */ }
}
```

This is where the work is. Checklist:

- **Hoist system and developer turns** into the vendor's system field, joined with `"\n\n"`. Reject non text content in them with `UnsupportedCapability`.
- **Handle all four `InputContent` variants.** The compiler enforces this. Do not add a catch all arm, because an exhaustive match is what tells you what to fix when the neutral model grows.
- **Map tool calls and results** onto whatever the vendor uses. Anthropic nests `tool_use` and `tool_result` blocks inside message content, closer to Gemini than to OpenAI's flat item list.
- **Reject text on `Role::Tool`** with `InvalidRequest`.
- **Reject images outside user turns** with `UnsupportedCapability`.
- **Default the model** when `value.model` is `None`.
- **Never invent a value** for a field the caller left as `None`.
- **Refuse, do not degrade.** If the vendor cannot express a capability, return `UnsupportedCapability`. Do not drop the field.

### 4. Convert inbound

```rust
impl From<Response> for GenerateResponse {
    fn from(value: Response) -> Self { /* ... */ }
}
```

Infallible on purpose. The body already parsed, so anything unrecognized is preserved rather than rejected.

- Normalize the status string into `ResponseStatus`, falling back to `ResponseStatus::Other(status)`.
- Normalize usage onto `Usage`, whatever the vendor names its fields.
- Put `extra` into `provider_metadata`.
- Skip unknown content blocks rather than failing.

### 5. Write the transport

`mod.rs` is convert, POST, check status, parse. Copy `openai/mod.rs` and change the URL, the auth header, and the provider name.

```rust
impl Provider for AnthropicProvider {
    async fn generate(
        &self,
        http: &reqwest::Client,
        api_key: &str,
        request: &GenerateRequest,
    ) -> Result<GenerateResponse, ProviderError> {
        let wire_request = types::Request::try_from(request)?;

        let response = http
            .post(MESSAGES_URL)
            .header("x-api-key", api_key)
            .header("anthropic-version", API_VERSION)
            .json(&wire_request)
            .send()
            .await
            .map_err(|error| ProviderError::Http(error.to_string()))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| ProviderError::Http(error.to_string()))?;

        if !status.is_success() {
            return Err(ProviderError::Api {
                provider: PROVIDER,
                status: status.as_u16(),
                body,
            });
        }

        let wire: types::Response =
            serde_json::from_str(&body).map_err(|error| ProviderError::InvalidResponse {
                provider: PROVIDER,
                message: format!("{error}; body: {body}"),
            })?;

        Ok(wire.into())
    }
}
```

Take the `reqwest::Client` as a parameter. Never build one inside `generate`, or you lose connection pooling.

Read the body as text before checking status, so a failure preserves the raw body in the error.

### 6. Wire it into the enums

Two edits in `src/provider/mod.rs`:

```rust
pub enum ProviderType {
    OpenAi,
    Gemini,
    Anthropic,
}

impl ProviderType {
    pub fn api_key_env(self) -> &'static str {
        match self {
            Self::OpenAi => "OPENAI_API_KEY",
            Self::Gemini => "GEMINI_API_KEY",
            Self::Anthropic => "ANTHROPIC_API_KEY",
        }
    }
}
```

And one arm in `Client::generate`:

```rust
ProviderType::Anthropic => {
    AnthropicProvider.generate(&self.http, &self.api_key, request).await
}
```

The compiler finds every match that needs updating.

### 7. Test it

Tests go in a `#[cfg(test)]` module at the bottom of `types.rs`, and they stay offline. Mirror the six that each existing provider has:

| Test | Asserts |
|---|---|
| `maps_neutral_request_to_..._wire_format` | Model default, system hoisting, basic fields |
| `normalizes_..._response` | Text, status, and usage come back correct |
| `maps_a_full_tool_round_trip` | Three turns produce the right wire items in order |
| `round_trips_a_tool_call_back_into_a_request` | Response feeds back through `to_message` |
| `rejects_text_in_a_tool_message` | Returns `InvalidRequest` |
| `rejects_capabilities_that_cannot_be_translated` | Returns `UnsupportedCapability`, if any apply |

The round trip test is the important one. It is what proves an agent loop will work on the new backend:

```rust
#[test]
fn maps_a_full_tool_round_trip() {
    let request = GenerateRequest::new()
        .message(Message::text(Role::User, "What is 20 + 22?"))
        .message(Message::new(Role::Assistant, vec![InputContent::ToolCall {
            id: "call_1".into(),
            name: "add".into(),
            arguments: "{\"a\":20,\"b\":22}".into(),
        }]))
        .message(Message::tool_result("call_1", "42"));

    let json = serde_json::to_value(Request::try_from(&request).unwrap()).unwrap();
    // assert the vendor specific shape
}
```

### 8. Document it

Add `docs/providers/<name>.md` following the shape of the existing two: endpoint table, capability table, field mapping tables, and an explicit list of what is rejected before the network. Link it from `docs/README.md` and update the status table in the top level `README.md`.

## Before you commit

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

`#![deny(missing_docs)]` is on, so any new public item without a doc comment fails the build. Provider internals are `pub(crate)` and do not need docs, but anything you add to the neutral model does.

## If the neutral model does not fit

Sometimes a new provider exposes something the neutral model cannot express. Order of preference:

1. **Map it onto an existing field.** Vendors name the same idea differently far more often than they invent a new one.
2. **Return `UnsupportedCapability`** and note it in the provider doc. Being honest about a gap beats a leaky abstraction.
3. **Read it from `provider_metadata`** when it is response only. Callers who need it can reach it without a model change.
4. **Extend the neutral model,** last resort. It means touching every provider, so it needs to be a genuinely general concept rather than one vendor's feature.

Never add a vendor specific field to `GenerateRequest`. The moment it holds `openai_seed` or `anthropic_top_k`, it is no longer neutral and the next provider inherits fields that mean nothing to it.
