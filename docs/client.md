# Client

`Client` is the entry point. It holds three things: which endpoint to talk to, the credentials for it, and the HTTP client used to reach it.

```rust
pub struct Client { /* private fields */ }
```

Derives `Debug` and `Clone`. Cloning is cheap for the HTTP client, since `reqwest::Client` is internally reference counted and clones share one connection pool, but it does copy the API key string. When you need the same client across many tasks, prefer sharing one behind an `Arc` over cloning it repeatedly.

## Constructors

### `Client::new`

```rust
pub fn new(config: impl Into<ProviderConfig>, api_key: impl Into<String>) -> Self
```

Builds a client with a pooled `reqwest::Client` and a 120 second per request timeout.

```rust
let client = Client::new(ProviderType::OpenAi, "sk-...");
```

If the HTTP client fails to build, Freya falls back to `reqwest::Client::default()` rather than panicking. The fallback has no timeout.

### `Client::without_key`

```rust
pub fn without_key(config: impl Into<ProviderConfig>) -> Self
```

For endpoints that need no credentials, such as a local Ollama or vLLM server.

```rust
let client = Client::without_key(ProviderType::Ollama);
```

### `Client::from_env`

```rust
pub fn from_env(config: impl Into<ProviderConfig>) -> Option<Self>
```

Reads the key from the variable named by the endpoint's `api_key_env`. Returns `None` when the variable is unset or set to an empty string, which lets you report a missing key rather than sending an unauthenticated request. An endpoint whose auth is `Auth::None` needs no key and always succeeds.

```rust
let Some(client) = Client::from_env(ProviderType::OpenAi) else {
    eprintln!("{} is missing or empty", ProviderType::OpenAi.api_key_env());
    return;
};
```

### `Client::with_http_client`

```rust
pub fn with_http_client(
    config: impl Into<ProviderConfig>,
    api_key: impl Into<String>,
    http: reqwest::Client,
) -> Self
```

Use this when you need control over the transport: a different timeout, a proxy, custom TLS, or sharing one connection pool with the rest of your application.

```rust
use std::time::Duration;

let http = reqwest::Client::builder()
    .timeout(Duration::from_secs(30))
    .connect_timeout(Duration::from_secs(5))
    .build()?;

let client = Client::with_http_client(ProviderType::OpenAi, api_key, http);
```

## Methods

### `generate`

```rust
pub async fn generate(&self, request: &GenerateRequest)
    -> Result<GenerateResponse, ProviderError>
```

Sends a request and returns the normalized response. The request is borrowed, so you can reuse and extend it across turns, which is what the tool loop does.

The whole response is buffered before returning. Streaming is not implemented yet.

### `config`

```rust
pub fn config(&self) -> &ProviderConfig
```

The endpoint this client talks to, including its dialect and resolved URL. Useful when a caller holds a client built elsewhere and needs to branch on capability.

## Dialect and endpoint are separate

Freya splits *how* a request is serialized from *where* it is sent, because most hosted inference APIs are wire-compatible with either OpenAI or Anthropic and differ only in URL, credentials, and model names.

| Type | Answers |
|---|---|
| `ProviderDialect` | Which wire format. `OpenAiResponses`, `OpenAiChat`, `Gemini`, `Anthropic` |
| `ProviderConfig` | Which endpoint. Base URL, auth style, key variable, default model, extra headers |
| `ProviderType` | A preset that builds a `ProviderConfig` for an endpoint Freya ships |

Anywhere a config is accepted a `ProviderType` is too, so the short form keeps working and nothing below changes for callers who only use the shipped endpoints.

For pointing Freya at an endpoint it does not ship, see [Custom endpoints](providers/custom-endpoints.md).

## ProviderType

```rust
pub enum ProviderType {
    OpenAi,       // OpenAiResponses
    Gemini,
    Anthropic,
    DeepSeek,     // the five below all speak OpenAiChat
    Groq,
    Together,
    OpenRouter,
    Ollama,       // needs no key
}
```

Derives `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`.

### `api_key_env`

```rust
pub fn api_key_env(self) -> &'static str
```

The conventional environment variable for this endpoint, `OPENAI_API_KEY`, `DEEPSEEK_API_KEY`, and so on. Used by `Client::from_env`, and useful on its own for error messages and startup checks. Empty for `Ollama`, which needs no credentials; `from_env` succeeds for it anyway.

### `dialect` and `config`

```rust
pub fn dialect(self) -> ProviderDialect
pub fn config(self) -> ProviderConfig
```

`config()` is the full endpoint description, and is what `Client` actually consumes. Call it when you want to start from a preset and change one thing:

```rust
let config = ProviderType::Anthropic.config().default_model("claude-sonnet-5");
let client = Client::new(config, key);
```

## Connection pooling

One `reqwest::Client` is built per `Client` and reused for every request, so TLS handshakes and TCP connections are amortized across calls. Do not build a new `Client` per request. Build one at startup and share it.

```rust
use std::sync::Arc;

let client = Arc::new(Client::from_env(ProviderType::OpenAi).unwrap());

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

    fn build(&self, request: &GenerateRequest, config: &ProviderConfig)
        -> Result<Self::Request, ProviderError>;

    fn parse(&self, body: &str, config: &ProviderConfig)
        -> Result<GenerateResponse, ProviderError>;
}
```

There is no transport method. `Client` owns convert, POST, check status, parse for every dialect, so it also owns the one `reqwest::Client` that every request in a process shares. The trait has an associated type, so it is not object safe. Dispatch happens through a `ProviderDialect` match inside `Client::generate`, not through a trait object.

Implementing it is covered in [Adding a provider](providers/adding-a-provider.md).
