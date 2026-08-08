# Freyja

A provider-neutral LLM client for Rust, and the foundation for building agents on top of it.

> [!WARNING]
> Under active development and **not ready for production use**. The public API is unstable and will change without notice before `0.1.0`.

You write one request. Freyja translates it into whatever wire format the model you picked actually speaks, sends it, and translates the answer back. Changing vendor is changing one line.

```rust
let client = Client::from_env(ProviderType::OpenAi)?;
// or Anthropic, or Gemini, or any compatible endpoint. Nothing else changes.
```

That matters because every vendor invented a different shape for the same ideas. A tool call is a flat item on OpenAI, a typed step on Gemini, a nested block on Anthropic, and a fourth arrangement on the Chat Completions format most other vendors copy. Your code sees none of it.

## Quick start

```toml
[dependencies]
freyja = { git = "https://github.com/rizukirr/Freyja" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

```rust
use freyja::{Client, GenerateRequest, Message, ProviderType, Role};

#[tokio::main]
async fn main() {
    let client = Client::from_env(ProviderType::OpenAi).expect("OPENAI_API_KEY");

    let request = GenerateRequest::new()
        .message(Message::text(Role::User, "Name three Rust crates."));

    match client.generate(&request).await {
        Ok(response) => println!("{}", response.output_text()),
        Err(error) => eprintln!("request failed: {error}"),
    }
}
```

Add tools and a loop and you have an agent. That is [Building an agent](docs/building-an-agent.md), and it is about fifteen lines.

```bash
cargo run --example simple           # one question, one answer
cargo run --example tool_loop        # a bounded agent loop
cargo run --example custom_endpoint  # an endpoint with no preset
```

## Documentation

Full docs in [`docs/`](docs/README.md), written to be read in order:

| | |
|---|---|
| [Introduction](docs/introduction.md) | What Freyja is, what it is not, and why it exists |
| [Features](docs/features.md) | What works today, and what does not |
| [Getting started](docs/getting-started.md) | Install, set a key, make a call |
| [Concepts](docs/concepts.md) | The five ideas everything else follows from |
| [Building an agent](docs/building-an-agent.md) | Tools, the loop, and what will bite you |

Then [providers](docs/providers/README.md), the [API reference](docs/README.md#reference), and [internals](docs/internals/architecture.md) for working on Freyja itself.

## Status

Phase 0 is complete: the neutral core is stable, four wire dialects are implemented, and tool calling works end to end.

| | |
|---|---|
| Built-in providers | OpenAI, Gemini, Anthropic, all verified against live APIs |
| Other endpoints | DeepSeek, Groq, OpenRouter, Ollama and friends via `Client::custom` |
| Tool calling | Full round trip, verified live on four endpoints |
| Dependencies | Three: `reqwest`, `serde`, `serde_json` |
| Not implemented | Streaming, retries, automatic tool dispatch, orchestration |

`cargo test`: 54 unit tests and 7 doctests. `cargo clippy --all-targets -- -D warnings` clean. [Features](docs/features.md) has the honest boundary, including which capabilities each provider refuses.

## Roadmap

The goal: everything you need to build an AI agent in Rust, with no vendor lock-in.

**Phase 0, stabilize the core.** Complete. Portable defaults, tool round trips, opaque reasoning state, pooled HTTP, live verification on every provider.

**Phase 1, production-grade provider layer.** Four dialects and the dialect/endpoint split are done. Remaining: streaming, retries with backoff, typed API errors, capability introspection, and derive-based structured output.

**Phase 2, the agent.** A `Tool` trait and registry, a `#[tool]` macro deriving schemas from function signatures, an `Agent` type, and a bounded loop with per-tool timeouts and approval hooks.

**Phase 3, memory and context.** A `Memory` trait, context-window management with truncation and summarization, persistent backends, and retrieval with embeddings and a vector store.

**Phase 4, orchestration.** The namesake. Multi-agent handoff, workflow primitives for chains and fan-out, shared state, propagated cancellation and budgets, and human-in-the-loop pause and resume.

**Phase 5, observability and release.** `tracing` instrumentation, cost accounting, record and replay for deterministic tests, a mock provider, and publishing to crates.io.

Out of scope: prompt-template DSLs, a built-in vector database, a web UI or server, and fine-tuning orchestration. Freyja is a library, not a platform.

## Development

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Rust edition 2024, minimum toolchain 1.88, verified in CI. `tokio` and `dotenvy` are dev-dependencies used only by the examples, so a consumer does not inherit them.

Contributions: [Architecture](docs/internals/architecture.md) explains the layout, [Adding a dialect](docs/internals/adding-a-dialect.md) covers new wire formats, and reaching a new vendor usually needs no code at all, see [Custom providers](docs/providers/custom.md).

## License

MIT. See [LICENSE](LICENSE).
