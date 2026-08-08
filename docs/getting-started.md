# Getting started

From nothing to a working call, then to an agent.

## Install

```bash
cargo add freyja
cargo add tokio --features macros,rt-multi-thread
```

Or in `Cargo.toml` directly:

```toml
[dependencies]
freyja = "0.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

Requires Rust 1.88 or later, verified in CI on every commit.

You supply the async runtime. Freyja exposes `async fn` and never spawns, so it pulls in no runtime of its own and brings three dependencies with it: `reqwest`, `serde`, `serde_json`.

## Set a key

Freyja reads credentials from the environment.

| Provider | Variable |
|---|---|
| `ProviderType::OpenAi` | `OPENAI_API_KEY` |
| `ProviderType::Gemini` | `GEMINI_API_KEY` |
| `ProviderType::Anthropic` | `ANTHROPIC_API_KEY` |

```bash
# .env
OPENAI_API_KEY=sk-...
```

Those three are the whole built-in list, on purpose. Every other endpoint, DeepSeek, Groq, OpenRouter, a local Ollama, or your own gateway, is reached with [`Client::custom`](providers/custom.md) and is no less supported for it.

Add `dotenvy` as a dev-dependency if you want `.env` loaded automatically, as the examples do. Nothing in Freyja requires it.

## Your first call

```rust
use freyja::{Client, GenerateRequest, Message, ProviderType, Role};

#[tokio::main]
async fn main() {
    let provider = ProviderType::OpenAi;
    let Some(client) = Client::from_env(provider) else {
        eprintln!("{} is missing or empty", provider.api_key_env());
        return;
    };

    let request = GenerateRequest::new()
        .message(Message::text(Role::User, "Name three Rust crates."));

    match client.generate(&request).await {
        Ok(response) => println!("{}", response.output_text()),
        Err(error) => eprintln!("request failed: {error}"),
    }
}
```

`from_env` returns `None` rather than panicking when the variable is missing, so a misconfigured deployment tells you what is wrong instead of sending an unauthenticated request.

Note what the request does *not* set: no model, no temperature, no token cap. Every unset field means "the provider decides", which is what makes this same request valid on all three providers. See [Concepts](concepts.md#3-unset-means-the-vendor-decides).

## Switch provider

Change one line:

```rust
let provider = ProviderType::Anthropic;
```

Nothing else moves. Freyja translates the same neutral request into a completely different wire format.

Portable does not mean identical, though. Each provider refuses a different slice of the request, so one using `tool_choice` fails on Gemini and one using `previous_response_id` fails on Anthropic. You get an error before the network call rather than a wrong answer. The [provider pages](providers/README.md) say which is which.

## Reach a provider that is not built in

```rust
use freyja::{Client, ProviderDialect};

let client = Client::custom(
    ProviderDialect::OpenAiChat,
    "DeepSeek",
    "https://api.deepseek.com/v1",
    std::env::var("DEEPSEEK_API_KEY")?,
);
```

Four things: which wire format, a name for error messages, the root URL, and the key. That one call covers most of the hosted inference market, because most vendors copy a format Freyja already speaks. See [Custom providers](providers/custom.md).

## Add a tool

This is the reason to use this library rather than a thinner one. Declare a function, and the model can ask you to run it:

```rust
let add = ToolDefinition::new("add", "adds two numbers together")
    .parameters(serde_json::json!({
        "type": "object",
        "properties": {"a": {"type": "integer"}, "b": {"type": "integer"}},
        "required": ["a", "b"]
    }));

let request = GenerateRequest::new()
    .message(Message::text(Role::User, "What is 20 + 22?"))
    .tools([add]);

let response = client.generate(&request).await?;

for (id, name, arguments) in response.tool_calls() {
    println!("model wants {name}({arguments})");
}
```

The model does not run anything. It asks; you decide. Turning that into a loop is [Building an agent](building-an-agent.md), and it is about fifteen lines.

## Run the examples

The repository ships three runnable programs:

```bash
cargo run --example simple           # one question, one answer
cargo run --example tool_loop        # a bounded agent loop
cargo run --example custom_endpoint  # an endpoint with no preset
```

They are compiled by `cargo test`, so they cannot drift out of date the way README snippets do.

## Next

| | |
|---|---|
| The design in five ideas | [Concepts](concepts.md) |
| The agent loop, properly | [Building an agent](building-an-agent.md) |
| What is not implemented yet | [Features](features.md) |
