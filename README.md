# Freyja

[![crates.io](https://img.shields.io/crates/v/freyja.svg)](https://crates.io/crates/freyja)
[![docs.rs](https://img.shields.io/docsrs/freyja)](https://docs.rs/freyja)
[![CI](https://github.com/rizukirr/Freyja/actions/workflows/ci.yml/badge.svg)](https://github.com/rizukirr/Freyja/actions/workflows/ci.yml)
[![license](https://img.shields.io/crates/l/freyja.svg)](LICENSE)

A provider-neutral LLM client for Rust, and the foundation for building agents on top of it.

> [!WARNING]
> Under active development and **not ready for production use**. The public API is unstable and will change without notice before `1.0.0`.

You write one request. Freyja translates it into whatever wire format the model you picked actually speaks, sends it, and translates the answer back. Changing vendor is changing one line.

```rust
let client = Client::from_env(ProviderType::OpenAi).expect("OPENAI_API_KEY");
// or Anthropic, or Gemini, or any compatible endpoint. Nothing else changes.
```

That matters because every vendor invented a different shape for the same ideas. A tool call is a flat item on OpenAI, a typed step on Gemini, a nested block on Anthropic, and a fourth arrangement on the Chat Completions format most other vendors copy. Your code sees none of it.

## Quick start

```bash
cargo add freyja
cargo add tokio --features macros,rt-multi-thread
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

Or take the same answer as it arrives. Tool-call arguments are assembled for you, so nothing hands you half a JSON object:

```rust
use freyja::StreamEvent;

let mut stream = client.stream(&request).await?;
while let Some(event) = stream.next().await? {
    match event {
        StreamEvent::TextDelta(text) => print!("{text}"),
        StreamEvent::ToolCall { name, arguments, .. } => println!("\n{name}({arguments})"),
        _ => {}
    }
}
```

A drained stream converts back with `stream.into_response()?`, so a streaming tool loop reuses the same `to_message()` the non-streaming one does. See [Streaming](docs/reference/streaming.md).

Add tools and a loop and you have an agent. That is [Building an agent](docs/building-an-agent.md), and it is about fifteen lines.

```bash
cargo run --example simple           # one question, one answer
cargo run --example streaming        # the same answer, printed as it arrives
cargo run --example tool_loop        # a bounded agent loop
cargo run --example custom_endpoint  # an endpoint with no preset
cargo run --example retry            # a retry loop over the error classification
cargo run --example chat             # an interactive multi-turn conversation
cargo run --example portable         # one request, every vendor, and its limits
cargo run --example structured_output # JSON constrained by a schema, deserialized
cargo run --example images           # an image in a prompt, by URL or data URI
```

## Documentation

Full docs in [`docs/`](docs/README.md), written to be read in order:

| Page | What it covers |
|---|---|
| [Introduction](docs/introduction.md) | What Freyja is, what it is not, and why it exists |
| [Features](docs/features.md) | What works today, and what does not |
| [Getting started](docs/getting-started.md) | Install, set a key, make a call |
| [Concepts](docs/concepts.md) | The five ideas everything else follows from |
| [Building an agent](docs/building-an-agent.md) | Tools, the loop, and what will bite you |

Then [providers](docs/providers/README.md), the [API reference](docs/README.md#reference), and [internals](docs/internals/architecture.md) for working on Freyja itself.

## Status

Phases 0 and 1 are complete: the neutral core is stable, four wire dialects are implemented, tool calling works end to end, every dialect streams, failures are classified by cause, a request can be checked before it is sent, a JSON schema can be rewritten into the subset a vendor accepts, and an answer can be deserialized straight into your own type. Next is Phase 2, the agent layer.

| Area | State |
|---|---|
| Built-in providers | OpenAI, Gemini, Anthropic, all verified against live APIs |
| Other endpoints | DeepSeek, Groq, OpenRouter, Ollama and friends via `Client::custom` |
| Tool calling | Full round trip, verified live on four endpoints |
| Streaming | All four dialects, text verified live; tool calls offline only |
| Dependencies | Three: `reqwest`, `serde`, `serde_json` |
| Errors | Classified by cause, with `is_retryable()` and `Retry-After` |
| Pre-flight checks | `client.check(&request)`, no network call |
| Structured output | `strict_schema()` plus `generate_as::<T>()` |
| Vendor-only fields | `extra_for()`, without forking |
| Not implemented | Automatic tool dispatch, orchestration |

`cargo test`: 128 unit tests, 8 integration tests, and 15 doctests. `cargo clippy --all-targets -- -D warnings` clean. [Features](docs/features.md) has the honest boundary, including which capabilities each provider refuses.

## Roadmap

The goal: everything you need to build an AI agent in Rust, with no vendor lock-in.

**Phase 0, stabilize the core.** Complete. Portable defaults, tool round trips, opaque reasoning state, pooled HTTP, live verification on every provider.

**Phase 1, production-grade provider layer.** Complete. Four dialects, the dialect/endpoint split, streaming, typed errors, pre-flight checking, typed responses, and strict-mode schema rewriting.

Deriving a JSON schema *from a Rust type* is deliberately not here: `schemars` already does it, and doing it again would mean a proc-macro crate and a derive that has to agree with serde's own attributes. Phase 2's `#[tool]` macro needs the same machinery, so the two get decided together there.

**Phase 2, the agent.** A `Tool` trait and registry, a `#[tool]` macro deriving schemas from function signatures, an `Agent` type, and a bounded loop with per-tool timeouts and approval hooks.

**Phase 3, memory and context.** A `Memory` trait, context-window management with truncation and summarization, persistent backends, and retrieval with embeddings and a vector store.

**Phase 4, orchestration.** The namesake. Multi-agent handoff, workflow primitives for chains and fan-out, shared state, propagated cancellation and budgets, and human-in-the-loop pause and resume.

**Phase 5, observability and release.** `tracing` instrumentation, cost accounting, record and replay for deterministic tests, and a mock provider for testing agents without network access.

Out of scope: prompt-template DSLs, a built-in vector database, a web UI or server, and fine-tuning orchestration. Freyja is a library, not a platform.

Also out of scope: **automatic retries**. Backing off means sleeping, and Freyja exposes `async fn` without spawning so the caller picks the runtime; retrying internally would take that choice away to save ten lines. `ProviderError::is_retryable()` and `retry_after()` make the decision cheap instead, and compose with `backon` or `tower::retry`. See [Errors](docs/reference/errors.md#retries).

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
