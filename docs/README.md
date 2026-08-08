# Freya documentation

Reference for everything currently available in Freya. Each page covers one feature and can be read on its own.

Freya is at Phase 0: the provider neutral core is stable, three providers are implemented, and tool calling works end to end. There is no agent loop, memory, or orchestration layer yet. See the roadmap in the top level [README](../README.md) for what is planned.

The Anthropic backend is the one page to read with a caveat attached, it has not been exercised against the live endpoint yet. See [Verification status](providers/anthropic.md#verification-status).

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
| [OpenAI wire format](providers/openai-wire.md) | The native Responses API JSON, field by field |
| [Gemini](providers/gemini.md) | Interactions API mapping, defaults, known gaps |
| [Gemini wire format](providers/gemini-wire.md) | The native Interactions API JSON, field by field |
| [Anthropic](providers/anthropic.md) | Messages API mapping, defaults, capability notes |
| [Anthropic wire format](providers/anthropic-wire.md) | The native Messages API JSON, field by field |
| [Custom endpoints](providers/custom-endpoints.md) | Pointing a dialect at any compatible endpoint |
| [Adding a provider](providers/adding-a-provider.md) | What it takes to add another wire dialect |

## Conventions used in these docs

Code samples assume the following imports unless stated otherwise:

```rust
use freya::{Client, GenerateRequest, Message, ProviderType, Role};
```

Samples that make a network call are written as if inside an `async fn` that returns `Result<(), freya::ProviderError>`.
