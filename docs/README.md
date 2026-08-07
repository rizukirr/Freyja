# Freya documentation

Reference for everything currently available in Freya. Each page covers one feature and can be read on its own.

Freya is at Phase 0: the provider neutral core is stable, two providers are implemented, and tool calling works end to end. There is no agent loop, memory, or orchestration layer yet. See the roadmap in the top level [README](../README.md) for what is planned.

## Start here

| Page | What it covers |
|---|---|
| [Getting started](getting-started.md) | Install, set a key, make your first call |
| [Architecture](architecture.md) | How the crate is laid out and why |

## Core API

| Page | What it covers |
|---|---|
| [Client](client.md) | `Client`, `ProviderType`, credentials, HTTP configuration |
| [Requests](requests.md) | `GenerateRequest` and every builder method |
| [Messages and content](messages.md) | `Message`, `Role`, `InputContent` |
| [Tool calling](tools.md) | `ToolDefinition`, `ToolChoice`, the full round trip |
| [Responses](responses.md) | `GenerateResponse`, `OutputContent`, `ResponseStatus`, `Usage` |
| [Errors](errors.md) | `ProviderError` and how to handle each variant |

## Providers

| Page | What it covers |
|---|---|
| [OpenAI](providers/openai.md) | Responses API mapping, defaults, capability notes |
| [Gemini](providers/gemini.md) | Interactions API mapping, defaults, known gaps |
| [Adding a provider](providers/adding-a-provider.md) | What it takes to add a third backend |

## Conventions used in these docs

Code samples assume the following imports unless stated otherwise:

```rust
use freya::{Client, GenerateRequest, Message, ProviderType, Role};
```

Samples that make a network call are written as if inside an `async fn` that
returns `Result<(), freya::ProviderError>`.
