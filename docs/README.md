# Freyja documentation

A Rust library for talking to large language models, and for building agents on top of them. One request type, four wire formats, any compatible endpoint.

New here? Read [Introduction](introduction.md), then [Getting started](getting-started.md).

## Learn

Read these in order the first time. About twenty minutes end to end.

| | |
|---|---|
| [Introduction](introduction.md) | What Freyja is, what it is not, and why it exists |
| [Features](features.md) | What works today, and what does not |
| [Getting started](getting-started.md) | Install, set a key, make a call |
| [Concepts](concepts.md) | The five ideas everything else follows from |
| [Building an agent](building-an-agent.md) | Tools, the loop, and what will bite you |

## Providers

| | |
|---|---|
| [Providers overview](providers/README.md) | Built-in versus custom, and how to choose |
| [OpenAI](providers/openai.md) | Responses API |
| [Gemini](providers/gemini.md) | Interactions API |
| [Anthropic](providers/anthropic.md) | Messages API |
| [OpenAI Chat Completions](providers/openai-chat.md) | The format most third-party vendors speak |
| [Custom providers](providers/custom.md) | Reaching any endpoint Freyja does not ship |

## Reference

Look these up rather than reading them through. Generated rustdoc is on [docs.rs](https://docs.rs/freyja).

| | |
|---|---|
| [Client](reference/client.md) | `Client`, `EndpointPreset`, `EndpointConfig`, credentials, HTTP |
| [Requests](reference/requests.md) | `GenerateRequest` and every builder method |
| [Messages](reference/messages.md) | `Message`, `Role`, `InputContent` |
| [Tools](reference/tools.md) | `#[tool]`, `Tool`, `ToolDefinition`, dispatch, and the full round trip |
| [Storage](reference/storage.md) | `Storage`, `Conversation`, and where a conversation lives between calls |
| [Responses](reference/responses.md) | `GenerateResponse`, `OutputContent`, `ResponseStatus`, `Usage` |
| [Streaming](reference/streaming.md) | `Client::stream`, `EventStream`, `StreamEvent`, `into_response` |
| [Errors](reference/errors.md) | `Error` and how to handle each variant |

Wire formats, for debugging an `Api` error against the native JSON: [OpenAI](reference/wire/openai.md), [Chat Completions](reference/wire/openai-chat.md), [Gemini](reference/wire/gemini.md), [Anthropic](reference/wire/anthropic.md).

## Contributing

For working on Freyja itself rather than with it.

| | |
|---|---|
| [Architecture](internals/architecture.md) | How the crate is laid out and why |
| [Capability model](internals/capability-model.md) | What Freyja may decide on a vendor's behalf, and why it is almost nothing |
| [Adding a dialect](internals/adding-a-dialect.md) | Implementing a new wire format |

## Conventions

Samples assume these imports unless stated otherwise:

```rust
use freyja::{Client, GenerateRequest, Message, EndpointPreset, Role};
```

Samples that make a network call are written as if inside an `async fn` returning `Result<(), freyja::Error>`.

## Status

Phases 0 through 2 are complete and Phase 3 has started. Typed tools, the provider-neutral round trip, streaming, and a conversation held between calls through `Storage`, with windowing on `Conversation`, are implemented. Multi-agent orchestration is not, and retries remain deliberately caller-owned. [Features](features.md) has the honest boundary, and the [roadmap](../README.md#roadmap) has what is planned.
