# Client

`Client` is the entry point. It holds three things: which provider to talk to, the credentials for it, and the HTTP client used to reach it.

```rust
pub struct Client { /* private fields */ }
```

Derives `Debug` and `Clone`. Cloning is cheap for the HTTP client, since `reqwest::Client` is internally reference counted and clones share one connection pool, but it does copy the API key string. When you need the same client across many tasks, prefer sharing one behind an `Arc` over cloning it repeatedly.

## Constructors

### `Client::new`

```rust
pub fn new(provider: ProviderType, api_key: impl Into<String>) -> Self
```

Builds a client with a pooled `reqwest::Client` and a 120 second per request timeout.

```rust
let client = Client::new(ProviderType::OpenAi, "sk-...");
```

If the HTTP client fails to build, Freya falls back to `reqwest::Client::default()` rather than panicking. The fallback has no timeout.

### `Client::from_env`

```rust
pub fn from_env(provider: ProviderType) -> Option<Self>
```

Reads the key from the variable named by `ProviderType::api_key_env()`. Returns `None` when the variable is unset or set to an empty string, which lets you report a missing key rather than sending an unauthenticated request.

```rust
let Some(client) = Client::from_env(ProviderType::OpenAi) else {
    eprintln!("{} is missing or empty", ProviderType::OpenAi.api_key_env());
    return;
};
```

### `Client::with_http_client`

```rust
pub fn with_http_client(
    provider: ProviderType,
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

### `provider`

```rust
pub fn provider(&self) -> ProviderType
```

Which backend this client talks to. Useful when a caller holds a client built elsewhere and needs to branch on capability.

## ProviderType

```rust
pub enum ProviderType {
    OpenAi,
    Gemini,
}
```

Derives `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`.

### `api_key_env`

```rust
pub fn api_key_env(self) -> &'static str
```

The conventional environment variable for this provider, `OPENAI_API_KEY` or `GEMINI_API_KEY`. Used by `Client::from_env`, and useful on its own for error messages and startup checks.

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
    fn generate(
        &self,
        http: &reqwest::Client,
        api_key: &str,
        request: &GenerateRequest,
    ) -> impl Future<Output = Result<GenerateResponse, ProviderError>> + Send;
}
```

The HTTP client is passed in rather than owned by the provider, which is what lets every request in a process share one pool. The trait uses return position `impl Trait`, so it is not object safe. Dispatch happens through the `ProviderType` match inside `Client::generate`, not through a trait object.

Implementing it is covered in [Adding a provider](providers/adding-a-provider.md).
