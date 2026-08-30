# Client

`Client` is the entry point. It holds three things: which endpoint to talk to, the credentials for it, and the HTTP client used to reach it.

```rust
pub struct Client { /* private fields */ }
```

Derives `Clone`. Cloning is cheap for the HTTP client, since `reqwest::Client` is internally reference counted and clones share one connection pool, but it does copy the API key string. When you need the same client across many tasks, prefer sharing one behind an `Arc` over cloning it repeatedly.

`Debug` is implemented by hand rather than derived, and **redacts the API key**:

```
Client { config: EndpointConfig { .. }, api_key: "<redacted>", http: "<reqwest::Client>" }
```

A derived `Debug` would print the key verbatim, so one `tracing::debug!(?client)` would put a live credential in your logs. The redaction still distinguishes `"<redacted>"` from `"<none>"`, since which one you have is the usual explanation for a 401.

The HTTP client is named rather than printed, for the same reason one field over. `reqwest::Client` prints its `default_headers` in full, so a client you built with an auth header and passed to [`Client::with_http_client`](#clientwith_http_client) used to leak that header here, past every other redaction in the struct. Note that this covers what Freyja prints. A `tracing::debug!(?http)` on your own `reqwest::Client` still prints its headers, so mark a credential header sensitive with `HeaderValue::set_sensitive` if you log the client yourself.

Credentials belong in `api_key` or in [`EndpointConfig::auth`](../providers/custom.md#auth), which are redacted unconditionally. For a second credential a gateway wants beside the key, say so with [`secret_header` or `secret_query`](../providers/custom.md#credentials-beside-the-key): a value left in plain `extra_headers` or `query` is redacted only when its name gives it away.

## Constructors

### `Client::new`

```rust
pub fn new(config: impl Into<EndpointConfig>, api_key: impl Into<String>) -> Self
```

Builds a client with a pooled `reqwest::Client` and a 120 second read timeout. That bounds the gap between bytes rather than the total duration of a request, so a long generation is not cut short and a stalled connection still fails.

```rust
let client = Client::new(EndpointPreset::OpenAi, "sk-...");
```

If the HTTP client fails to build, Freyja falls back to `reqwest::Client::default()` rather than panicking. The fallback has no timeout.

### `Client::custom`

```rust
pub fn custom(
    dialect: Dialect,
    name: impl Into<Arc<str>>,
    base_url: impl Into<String>,
    api_key: impl Into<String>,
) -> Self
```

Reaches an endpoint Freyja does not ship, in one call. Shorthand for `EndpointConfig::new` followed by `Client::new`.

```rust
let client = Client::custom(
    Dialect::OpenAiChat,
    "my-gateway",
    "https://gateway.internal/v1",
    std::env::var("GATEWAY_API_KEY")?,
);
```

Use the config builder directly when you also need a default model, a key variable, extra headers, or a non-conventional auth style. See [Custom endpoints](../providers/custom.md).

### `Client::without_key`

```rust
pub fn without_key(config: impl Into<EndpointConfig>) -> Self
```

For endpoints that need no credentials, such as a local Ollama or vLLM server.

```rust
let config = EndpointConfig::new(
    Dialect::OpenAiChat,
    "ollama",
    "http://localhost:11434/v1",
);

let client = Client::without_key(config);
```

### `Client::from_env`

```rust
pub fn from_env(config: impl Into<EndpointConfig>) -> Option<Self>
```

Reads the key from the variable named by the endpoint's `api_key_env`. Returns `None` when the variable is unset or set to an empty string, which lets you report a missing key rather than sending an unauthenticated request. An endpoint whose auth is `Auth::None` needs no key and always succeeds.

```rust
let Some(client) = Client::from_env(EndpointPreset::OpenAi) else {
    eprintln!("{} is missing or empty", EndpointPreset::OpenAi.api_key_env());
    return;
};
```

### `Client::with_http_client`

```rust
pub fn with_http_client(
    config: impl Into<EndpointConfig>,
    api_key: impl Into<String>,
    http: reqwest::Client,
) -> Self
```

Use this when you need control over the transport: a different timeout, a proxy, custom TLS, or sharing one connection pool with the rest of your application.

```rust
use std::time::Duration;

let http = reqwest::Client::builder()
    .read_timeout(Duration::from_secs(30))
    .connect_timeout(Duration::from_secs(5))
    .build()?;

let client = Client::with_http_client(EndpointPreset::OpenAi, api_key, http);
```

The client you supply is used as it is, so its timeouts are yours to get right. Set `read_timeout` rather than `timeout` if you stream: `timeout` caps the whole response body, which on a stream is a cap on how long the model is allowed to talk, and a healthy long generation is killed part-way. See [Streaming](streaming.md#timeouts).

## Methods

### `generate`

```rust
pub async fn generate(&self, request: &GenerateRequest)
    -> Result<GenerateResponse, Error>
```

Sends a request and returns the normalized response. The request is borrowed, so you can reuse and extend it across turns, which is what the tool loop does.

The whole response is buffered before returning. Use `stream` when you want the answer as it arrives.

### `generate_as`

```rust
pub async fn generate_as<T: DeserializeOwned>(&self, request: &GenerateRequest)
    -> Result<T, Error>
```

Sends a request and deserializes the answer into `T`. The companion to `ResponseFormat::JsonSchema`: you constrain the model's output to a shape, and this hands back that shape rather than a string.

```rust
#[derive(Deserialize)]
struct Recommendation { name: String, purpose: String }

let recommendation: Recommendation = client.generate_as(&request).await?;
```

The schema is still yours to write and to keep in step with `T`. Deriving one from the type is not implemented, so nothing stops the two from drifting except the deserialize step failing.

`generate` remains the right call when the raw text matters as much as the value, or when you want the usage figures and the status alongside it.

#### The failure it adds

`OutputMismatch`, and only that one. Everything `generate` can raise passes through untouched — an unreachable endpoint is still a transport error, not a deserialization one.

It exists because "the model's answer was not your shape" is a different problem from every other failure in the enum. The call succeeded. The vendor behaved. What came back is well-formed and not what you asked for, and the fix is the schema, the prompt, or the token cap.

Two things make it more useful than the `serde_json::Error` you would get by hand:

**It keeps the text.** The model's answer is the only record of what actually happened, and it is gone once the parse fails. Log it, show it, or salvage it.

**It separates truncation.** A cut-off answer is still valid text and never valid JSON, and it is the most common way this fails. Freyja checks `ResponseStatus::Incomplete` rather than guessing from the parse error, so a `truncated: true` sends you to `max_tokens` instead of to your schema:

```rust
Err(Error::OutputMismatch { text, truncated: true, .. }) => {
    eprintln!("cut short by max_tokens, got: {text}");
}
```

`is_retryable()` is `false`. The output is nondeterministic, so another attempt might happen to parse — but the request that produced this one will keep producing it, and treating that as transient hides a real problem. Match the variant directly if you want to retry anyway.

### `stream`

```rust
pub async fn stream(&self, request: &GenerateRequest)
    -> Result<EventStream, Error>
```

Opens a streaming generation. Returns once the provider has accepted the request, so a non-success status arrives here, classified by cause, rather than mid-stream.

```rust
use freyja::StreamEvent;

let mut stream = client.stream(&request).await?;
while let Some(event) = stream.next().await? {
    if let StreamEvent::TextDelta(text) = event {
        print!("{text}");
    }
}

let response = stream.into_response()?;
```

A drained stream converts back into the same `GenerateResponse` that `generate` would have returned, so a tool loop needs no second code path. Every dialect supports it. See [Streaming](streaming.md).

### `check`

```rust
pub fn check(&self, request: &GenerateRequest) -> Result<(), Error>
```

Decides whether this endpoint's dialect can carry a request, without sending it. No network call, no credentials used, and cheap enough to run per request: it builds one wire body and drops it. The body is never even serialized to JSON, since that happens on the way to the socket.

```rust
match client.check(&request) {
    Ok(()) => { /* generate() will get as far as the network */ }
    Err(Error::UnsupportedCapability { capability, .. }) => {
        eprintln!("this endpoint cannot express {capability}");
    }
    Err(error) => return Err(error),
}
```

This is Freyja's answer to capability introspection, and it is a different shape from the usual one. There is no table of booleans to consult, because `check` runs the same conversion `generate` and `stream` run and reports what happened. Three consequences follow, and they are the reason for the design:

**It cannot drift.** A hand-maintained capability table is a second description of the dialects, kept in sync by hand. `check` is not a description; it is the code itself.

**It answers questions a table cannot express.** Support is not always a property of the field. Anthropic accepts `response_format`, but not the value `JsonObject`. A `response_format: bool` would answer `true` and the request would still fail. And placement rules — an image belongs on a user turn — depend on the transcript you built, not on the vendor at all.

**It reports the reason.** You get the capability string and the endpoint name, not a bare `false`.

#### What `Ok` promises

That the *dialect* can express this request: every field has somewhere to go in the wire format, and the transcript is shaped the way the format requires.

Not that the endpoint will accept it. Freyja knows the wire format; it has never met your gateway, and will not claim to know what that gateway implements. A request that passes `check` can still come back `BadRequest`. See [Custom providers](../providers/custom.md).

Not that the *model* supports it either. `check` never reads the model name — `EndpointConfig::model_for` picks which string to send and nothing inspects it — so a model that does no reasoning still passes a request carrying `reasoning_effort`, and the vendor settles it.

That boundary is deliberate. Wire formats change on the order of years and are documented. What a given model accepts changes weekly, silently, and differs on every compatible gateway; a table of it would be confidently wrong within a month, which is worse than saying nothing. It is the same reasoning `presets.rs` gives for shipping only three presets: a stale promise fails at the vendor with a confusing error instead of locally with a clear one.

The converse is solid: an `Err` from `check` is an error `generate` would have raised too, before reaching the network.

#### Choosing an endpoint

Because the answer depends on the request rather than on a fixed table, comparing endpoints means checking the same request against each:

```rust
let usable: Vec<_> = [EndpointPreset::OpenAi, EndpointPreset::Gemini]
    .into_iter()
    .filter_map(Client::from_env)
    .filter(|client| client.check(&request).is_ok())
    .collect();
```

`examples/portable.rs` does exactly this against three real endpoints, next to the same request being sent, so the two ways of learning the fact sit side by side. Run it with `cargo run --example portable`.

#### What else it catches

Everything decidable before the network, not only capabilities. A request naming no model, on an endpoint with no `default_model`, is an `InvalidRequest` from `generate` and an `InvalidRequest` from `check`. Reporting it is the honest behaviour: `check` promises that `generate` would get as far as the network, and that request would not.

### `config`

```rust
pub fn config(&self) -> &EndpointConfig
```

The endpoint this client talks to, including its dialect and resolved URL. Useful when a caller holds a client built elsewhere and needs to branch on capability.

## Dialect and endpoint are separate

Freyja splits *how* a request is serialized from *where* it is sent, because most hosted inference APIs are wire-compatible with either OpenAI or Anthropic and differ only in URL, credentials, and model names.

| Type | Answers |
|---|---|
| `Dialect` | Which wire format. `OpenAiResponses`, `OpenAiChat`, `Gemini`, `Anthropic` |
| `EndpointConfig` | Which endpoint. Base URL, path, query, auth style, key variable, default model, extra headers |
| `EndpointPreset` | A preset that builds a `EndpointConfig` for an endpoint Freyja ships |

Anywhere a config is accepted a `EndpointPreset` is too, so the short form keeps working and nothing below changes for callers who only use the shipped endpoints.

For pointing Freyja at an endpoint it does not ship, see [Custom endpoints](../providers/custom.md).

## EndpointPreset

```rust
pub enum EndpointPreset {
    OpenAi,     // OpenAiResponses
    Gemini,
    Anthropic,
}
```

Derives `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`.

Only first-party vendors appear here. Freyja implements a fourth dialect, `OpenAiChat`, with no preset at all, because every endpoint speaking it is third party. Reach those with `Client::custom`, see [Custom endpoints](../providers/custom.md).

### `api_key_env`

```rust
pub fn api_key_env(self) -> &'static str
```

The conventional environment variable for this endpoint, `OPENAI_API_KEY`, `GEMINI_API_KEY`, or `ANTHROPIC_API_KEY`. Used by `Client::from_env`, and useful on its own for error messages and startup checks.

### `dialect` and `config`

```rust
pub fn dialect(self) -> Dialect
pub fn config(self) -> EndpointConfig
```

`config()` is the full endpoint description, and is what `Client` actually consumes. Call it when you want to start from a preset and change one thing:

```rust
let config = EndpointPreset::Anthropic.config().default_model("claude-sonnet-5");
let client = Client::new(config, key);
```

## Connection pooling

One `reqwest::Client` is built per `Client` and reused for every request, so TLS handshakes and TCP connections are amortized across calls. Do not build a new `Client` per request. Build one at startup and share it.

```rust
use std::sync::Arc;

let client = Arc::new(Client::from_env(EndpointPreset::OpenAi).unwrap());

for question in questions {
    let client = Arc::clone(&client);
    tokio::spawn(async move {
        let request = GenerateRequest::new().message(Message::text(Role::User, question));
        let _ = client.generate(&request).await;
    });
}
```

## The Provider trait

`Client` dispatches to an implementation of `Provider`:

```rust
pub trait Provider: Send + Sync {
    type Request: Serialize + Send;

    fn build(&self, request: &GenerateRequest, config: &EndpointConfig)
        -> Result<Self::Request, Error>;

    fn parse(&self, body: &str, config: &EndpointConfig)
        -> Result<GenerateResponse, Error>;
}
```

There is no transport method. `Client` owns convert, POST, check status, parse for every dialect, so it also owns the one `reqwest::Client` that every request in a process shares. The trait has an associated type, so it is not object safe. Dispatch happens through a `Dialect` match inside `Client::generate`, not through a trait object.

Streaming is not part of the trait. Each dialect also owns a decoder that turns its own SSE frames into neutral events, selected by the same match inside `Client::stream`.

Implementing it is covered in [Adding a provider](../internals/adding-a-dialect.md).
