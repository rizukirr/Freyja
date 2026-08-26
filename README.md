# Freyja

[![crates.io](https://img.shields.io/crates/v/freyja.svg)](https://crates.io/crates/freyja)
[![docs.rs](https://img.shields.io/docsrs/freyja)](https://docs.rs/freyja)
[![CI](https://github.com/rizukirr/Freyja/actions/workflows/ci.yml/badge.svg)](https://github.com/rizukirr/Freyja/actions/workflows/ci.yml)
[![license](https://img.shields.io/crates/l/freyja.svg)](LICENSE)

A provider-neutral LLM client for Rust, and the foundation for building agents on top of it.

> [!WARNING]
> Under active development and **not ready for production use**. The public API is unstable and will change without notice before `1.0.0`.

## Migration

Update imports and error matching with this rename table:

| Before | After |
|---|---|
| `ProviderDialect` | `Dialect` |
| `ProviderConfig` | `EndpointConfig` |
| `ProviderType` | `EndpointPreset` |
| `ProviderError` | `Error` |
| error field `provider` | error field `endpoint` |
| `error.provider()` | `error.endpoint()` |
| `Agent::request(GenerateRequest::new().model(..))` | `Agent::model(..)`, and the same for `max_tokens`, `temperature`, `top_p`, `reasoning_effort`, `tool_choice`, `extra_for` |
| a system prompt on an `Agent` template | `Agent::system(..)` |
| `Agent::run`, `Agent::run_with` | `Agent::messages`, `Agent::messages_with` |
| `Agent::chat()` and `Chat::ask` | `Agent::memory(InMemoryStorage::new())` and `Agent::message` |
| `Memory`, `MemoryError`, `MemoryFuture` | `Filter`, `FilterError`, `FilterFuture` |
| `Agent::memory(impl Memory)` | `Agent::filter(impl Filter)`, and `Agent::memory` now takes `Storage` |

You write one request. Freyja translates it into whatever wire format the model you picked actually speaks, sends it, and translates the answer back. Changing vendor is changing one line.

```rust
let client = Client::from_env(EndpointPreset::OpenAi).expect("OPENAI_API_KEY");
// or Anthropic, or Gemini, or any compatible endpoint. Nothing else changes.
```

That matters because every vendor invented a different shape for the same ideas. A tool call is a flat item on OpenAI, a typed step on Gemini, a nested block on Anthropic, and a fourth arrangement on the Chat Completions format most other vendors copy. Your code sees none of it.

## Quick start

```bash
cargo add freyja
cargo add tokio --features macros,rt-multi-thread
```

```rust
use freyja::{Client, GenerateRequest, Message, EndpointPreset, Role};

#[tokio::main]
async fn main() {
    let client = Client::from_env(EndpointPreset::OpenAi).expect("OPENAI_API_KEY");

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

Add typed tools and a bounded loop and you have an agent. `#[tool]` derives the argument schema and JSON dispatcher from an ordinary Rust function. See [Building an agent](docs/building-an-agent.md).

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
cargo run --example async_tools      # several tool calls running at once
cargo run --example agent            # the loop driven by Agent
cargo run --example guarded_tools    # tool state, run context, failures, and a guard
cargo run --example memory           # bounding what reaches the model, transcript kept whole
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

Phases 0 through 2 are complete, and Phase 3 has started: the neutral core is stable, four wire dialects are implemented, tool calling works end to end, typed `#[tool]` functions derive their schemas and dispatchers and may be sync or async, every dialect streams, failures are classified by cause, and `Memory` decides what part of a transcript reaches the model on each turn.

| Area | State |
|---|---|
| Built-in providers | OpenAI, Gemini, Anthropic, all verified against live APIs |
| Other endpoints | DeepSeek, Groq, OpenRouter, Ollama and friends via `Client::custom` |
| Tool calling | Typed `#[tool]` declarations and the full round trip |
| Streaming | All four dialects, text verified live; tool calls offline only |
| Dependencies | `reqwest`, `serde`, `serde_json`, `schemars`, and the companion macro crate |
| Errors | Classified by cause, with `is_retryable()` and `Retry-After` |
| Pre-flight checks | `client.check(&request)`, no network call |
| Structured output | `strict_schema()` plus `generate_as::<T>()` |
| Vendor-only fields | `extra_for()`, without forking |

The workspace test suite covers the core, macro expansion, public typed-tool behavior, examples, and doctests. [Features](docs/features.md) has the honest boundary, including which capabilities each provider refuses.

## Roadmap

The goal: everything you need to build an AI agent in Rust, with no vendor lock-in.

**Phase 0, stabilize the core.** Complete. Portable defaults, tool round trips, opaque reasoning state, pooled HTTP, live verification on every provider.

**Phase 1, production-grade provider layer.** Complete. Four dialects, the dialect/endpoint split, streaming, typed errors, pre-flight checking, typed responses, and strict-mode schema rewriting.

**Phase 2, the agent.** Complete. `Tool` and `#[tool]` derive schemas from sync or async function signatures and provide typed execution, and `Agent` drives the tool-calling loop automatically, dispatching parallel tool calls concurrently. `Tool` is now a trait, so a tool can hold state in its fields, be built at runtime, and report failure as text the model recovers from; `Context` carries per-run data to every call without exposing it to the model. `Agent::guard` vets every requested call before dispatch, so a policy can refuse one and the model reads why.

**Phase 3, memory and context.** Started. `Memory` decides what reaches the model each turn and `Window` bounds a conversation by turn group, with the caller's transcript kept whole. Token-aware windows, summarization, persistent backends, and retrieval with embeddings and a vector store are not built.

**Phase 4, orchestration.** The namesake. Multi-agent handoff, workflow primitives for chains and fan-out, shared state, propagated cancellation and budgets, and human-in-the-loop pause and resume.

**Phase 5, observability and release.** `tracing` instrumentation, cost accounting, record and replay for deterministic tests, and a mock provider for testing agents without network access.

Out of scope: prompt-template DSLs, a built-in vector database, a web UI or server, and fine-tuning orchestration. Freyja is a library, not a platform.

Also out of scope: **automatic retries**. Backing off means sleeping, and Freyja exposes `async fn` without spawning so the caller picks the runtime; retrying internally would take that choice away to save ten lines. `Error::is_retryable()` and `retry_after()` make the decision cheap instead, and compose with `backon` or `tower::retry`. See [Errors](docs/reference/errors.md#retries).

## Development

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Rust edition 2024, minimum toolchain 1.88, verified in CI. `tokio` and `dotenvy` are dev-dependencies used only by the examples, so a consumer does not inherit them.

Contributions: [Architecture](docs/internals/architecture.md) explains the layout, [Capability model](docs/internals/capability-model.md) explains what Freyja is allowed to refuse and why that is almost nothing, [Adding a dialect](docs/internals/adding-a-dialect.md) covers new wire formats, and reaching a new vendor usually needs no code at all, see [Custom providers](docs/providers/custom.md).

## License

MIT. See [LICENSE](LICENSE).
