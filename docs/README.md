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
| [Client](reference/client.md) | `Client`, `ProviderType`, `ProviderConfig`, credentials, HTTP |
| [Requests](reference/requests.md) | `GenerateRequest` and every builder method |
| [Messages](reference/messages.md) | `Message`, `Role`, `InputContent` |
| [Tools](reference/tools.md) | `ToolDefinition`, `ToolChoice`, the round trip in full |
| [Responses](reference/responses.md) | `GenerateResponse`, `OutputContent`, `ResponseStatus`, `Usage` |
| [Streaming](reference/streaming.md) | `Client::stream`, `EventStream`, `StreamEvent`, `into_response` |
| [Errors](reference/errors.md) | `ProviderError` and how to handle each variant |

Wire formats, for debugging an `Api` error against the native JSON: [OpenAI](reference/wire/openai.md), [Chat Completions](reference/wire/openai-chat.md), [Gemini](reference/wire/gemini.md), [Anthropic](reference/wire/anthropic.md).

## Contributing

For working on Freyja itself rather than with it.

| | |
|---|---|
| [Architecture](internals/architecture.md) | How the crate is laid out and why |
| [Adding a dialect](internals/adding-a-dialect.md) | Implementing a new wire format |

## Conventions

Samples assume these imports unless stated otherwise:

```rust
use freyja::{Client, GenerateRequest, Message, ProviderType, Role};
```

Samples that make a network call are written as if inside an `async fn` returning `Result<(), freyja::ProviderError>`.

## Status

Phase 0. The neutral core is stable, four dialects are implemented, tool calling works end to end against live APIs, and every dialect streams. There are no retries and no orchestration layer. [Features](features.md) has the honest boundary; the [roadmap](../README.md#roadmap-to-mvp) has what is planned.
