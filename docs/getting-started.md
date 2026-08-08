# Getting started

## Requirements

- Rust with edition 2024 support
- An API key for at least one provider

Freya depends on `tokio`, `reqwest`, `serde`, `serde_json`, and `dotenvy`. All requests are async, so you need a Tokio runtime.

## Add the dependency

Freya is not published to crates.io yet, so depend on it by path or by git:

```toml
[dependencies]
freya = { path = "../freya" }
tokio = { version = "1", features = ["full"] }
serde_json = "1"
```

## Provide a key

Freya reads keys from the environment. The variable name per provider is given by `ProviderType::api_key_env()`:

| Provider | Variable |
|---|---|
| `ProviderType::OpenAi` | `OPENAI_API_KEY` |
| `ProviderType::Gemini` | `GEMINI_API_KEY` |
| `ProviderType::Anthropic` | `ANTHROPIC_API_KEY` |
| `ProviderType::DeepSeek` | `DEEPSEEK_API_KEY` |
| `ProviderType::Groq` | `GROQ_API_KEY` |
| `ProviderType::Together` | `TOGETHER_API_KEY` |
| `ProviderType::OpenRouter` | `OPENROUTER_API_KEY` |
| `ProviderType::Ollama` | none, it is a local server |

Put them in a `.env` file at the project root:

```bash
OPENAI_API_KEY=sk-...
GEMINI_API_KEY=...
```

`.env` is not loaded automatically. Call `dotenvy::dotenv().ok()` once at startup if you want it read.

## Your first call

```rust
use freya::{Client, GenerateRequest, Message, ProviderType, Role};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let client = Client::from_env(ProviderType::OpenAi).expect("OPENAI_API_KEY");

    let request = GenerateRequest::new()
        .message(Message::text(Role::System, "Answer in one sentence."))
        .message(Message::text(Role::User, "Why is the sky blue?"));

    match client.generate(&request).await {
        Ok(response) => println!("{}", response.output_text()),
        Err(error) => eprintln!("request failed: {error}"),
    }
}
```

`Client::from_env` returns `None` when the variable is unset or empty, so you can fail with a clear message instead of sending an unauthenticated request.

## Switching providers

Nothing about the request changes. Swap the `ProviderType` and Freya translates the same neutral request into the other vendor's wire format:

```rust
let client = Client::from_env(ProviderType::Gemini).expect("GEMINI_API_KEY");
let client = Client::from_env(ProviderType::Anthropic).expect("ANTHROPIC_API_KEY");
```

Portable does not mean identical. Each provider refuses a different slice of the neutral model, so a request that uses `tool_choice` fails on Gemini and one that uses `previous_response_id` fails on Anthropic. The capability tables on [OpenAI](providers/openai.md), [Gemini](providers/gemini.md), and [Anthropic](providers/anthropic.md) say which is which, and you get a `UnsupportedCapability` error rather than a wrong answer.

Requests stay portable as long as you do not ask for a capability the target provider cannot express. When you do, Freya returns `ProviderError::UnsupportedCapability` rather than quietly dropping the field. See [Errors](errors.md) and the per provider pages for what each backend supports.

## Run the bundled example

`src/main.rs` is a working one tool agent loop. It asks a question the model cannot answer alone, runs the tool the model requests, feeds the result back, and prints the final answer:

```bash
cargo run
```

It needs `OPENAI_API_KEY`. Read [Tool calling](tools.md) for how it works.

## Next steps

- [Requests](requests.md) for the full set of knobs
- [Tool calling](tools.md) to let the model call your code
- [Client](client.md) to control timeouts, proxies, and connection pooling
