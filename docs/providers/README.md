# Providers

Freyja reaches a provider in one of two ways. Both use the same code paths, and neither is more supported than the other.

| | Use when | How |
|---|---|---|
| **Built-in** | OpenAI, Gemini, Anthropic | `ProviderType::OpenAi` |
| **Custom** | Everything else | `Client::custom(dialect, name, url, key)` |

## Built-in providers

Three presets, for the three first-party vendors whose endpoints Freyja is willing to promise stay current. That is not the same as the number of wire formats: Freyja implements four dialects, and `OpenAiChat` is implemented and tested but has no preset, because every endpoint speaking it is third party. See [the four wire dialects](#the-four-wire-dialects).

| Provider | Key variable | Default model | Notes |
|---|---|---|---|
| [OpenAI](openai.md) | `OPENAI_API_KEY` | `gpt-5.6-sol` | Responses API, the most complete mapping |
| [Gemini](gemini.md) | `GEMINI_API_KEY` | `gemini-3.5-flash` | Refuses `tool_choice` and `reasoning_effort` |
| [Anthropic](anthropic.md) | `ANTHROPIC_API_KEY` | `claude-opus-5` | Requires `max_tokens`, defaulted for you |

```rust
let client = Client::from_env(ProviderType::Anthropic).expect("ANTHROPIC_API_KEY");
```

All three complete a live tool round trip. Read the provider page before relying on a capability; coverage is not uniform, and each page has a table saying exactly what is refused.

## Everything else

Most hosted inference APIs copy a format Freyja already speaks, so they need no code at all:

```rust
let client = Client::custom(
    ProviderDialect::OpenAiChat,
    "DeepSeek",
    "https://api.deepseek.com/v1",
    api_key,
);
```

That covers DeepSeek, Groq, Together, Fireworks, OpenRouter, Ollama, vLLM, LM Studio, xAI, Mistral, and Gemini's own OpenAI-compatible endpoint, among others. [Custom providers](custom.md) has base URLs to start from and the full builder.

**Why these are not built in.** A preset is a standing promise that a URL and a default model are still current, and third-party endpoints change both faster than this library could verify. A stale preset fails at the vendor with a confusing 404; a missing one fails locally with a clear message, or does not fail at all because you supplied the current URL. Keeping the list short is what keeps it trustworthy.

## The four wire dialects

You pick a dialect only when using a custom endpoint. With a built-in provider it is implied.

| Dialect | Shape | Who speaks it |
|---|---|---|
| [`OpenAiChat`](openai-chat.md) | Nested, dedicated `tool` role | Most third-party vendors |
| [`OpenAiResponses`](openai.md) | Flat item list | OpenAI only |
| [`Anthropic`](anthropic.md) | Nested content blocks | Anthropic and Claude gateways |
| [`Gemini`](gemini.md) | Flat step list, no roles | Google only |

If a vendor's docs show `messages` with `choices` in the response, it is `OpenAiChat`. That is the safe first guess for anything unfamiliar.

### Streaming

All four dialects stream, through the same `Client::stream` call and the same `StreamEvent` sequence. Three differences reach the caller:

| Dialect | What differs |
|---|---|
| `Gemini` | Needs `?alt=sse` on the URL as well as `stream: true` in the body; `Client::stream` appends it |
| `OpenAiChat` | Sends `stream_options: {"include_usage": true}`, without which the stream reports no tokens |
| `OpenAiChat` | Has no end-of-call frame, so `StreamEvent::ToolCall` arrives only when the body closes |

All four have been run against a live endpoint for a text turn. Streamed tool calls have not, and remain covered by recorded fixtures taken from vendor documentation. Full detail in [Streaming](../reference/streaming.md).

## Choosing between them

Portability has a price, and it is worth knowing which fields cost you.

| If you use | You lose |
|---|---|
| `tool_choice` | Gemini |
| `reasoning_effort` | Gemini |
| `previous_response_id` | Anthropic, and every `OpenAiChat` endpoint |
| `response_format` as free JSON | Anthropic |

Set none of them and your request runs anywhere. Each is refused before the network, so you find out at once rather than getting an answer that ignored you.

## Debugging a provider

When a request fails and the vendor's message is not enough, the wire reference documents the native JSON Freyja sends and receives, so you do not have to read vendor documentation to interpret an error body.

[OpenAI](../reference/wire/openai.md) · [Chat Completions](../reference/wire/openai-chat.md) · [Gemini](../reference/wire/gemini.md) · [Anthropic](../reference/wire/anthropic.md)
