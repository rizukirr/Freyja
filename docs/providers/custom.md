# Custom endpoints

Most hosted inference APIs do not invent a wire format. They copy one, usually OpenAI's or Anthropic's, so that existing client libraries work unchanged. Freyja separates the format from the endpoint so you can take advantage of that without waiting for a preset.

## The two types

| Type | Answers | Example |
|---|---|---|
| `ProviderDialect` | which wire format | `Anthropic` |
| `ProviderConfig` | which endpoint speaks it | `https://api.z.ai/api/anthropic/v1`, `x-api-key`, `glm-4.6` |

A preset is only a `ProviderConfig` with the fields filled in. There is nothing a preset can do that you cannot.

## The quickest version

When all you need is a dialect, a name, a URL, and a key:

```rust
use freyja::{Client, ProviderDialect};

let client = Client::custom(
    ProviderDialect::OpenAiChat,
    "my-gateway",
    "https://gateway.internal/v1",
    std::env::var("GATEWAY_API_KEY")?,
);
```

Auth follows the dialect, so this is `Authorization: Bearer` without saying so. Everything below is for when you need more than those four things.

## Pointing at a compatible endpoint

```rust
use freyja::{Client, ProviderConfig, ProviderDialect};

let config = ProviderConfig::new(
        ProviderDialect::Anthropic,
        "my-gateway",
        "https://gateway.internal/anthropic/v1",
    )
    .api_key_env("GATEWAY_API_KEY")
    .default_model("claude-opus-5");

let client = Client::from_env(config).expect("GATEWAY_API_KEY");
```

`base_url` is the **root**. The dialect appends its own path, `/messages` here, so do not include it yourself. Check with `config.url()` if you are unsure.

## Known compatible endpoints

Starting points, not guarantees. These were correct when written and are **not** verified by any test in this crate, which is exactly why they are documentation rather than presets. Check the vendor's current docs before relying on one.

| Endpoint | Dialect | Base URL | Key variable |
|---|---|---|---|
| DeepSeek | `OpenAiChat` | `https://api.deepseek.com/v1` | `DEEPSEEK_API_KEY` |
| Groq | `OpenAiChat` | `https://api.groq.com/openai/v1` | `GROQ_API_KEY` |
| Together | `OpenAiChat` | `https://api.together.xyz/v1` | `TOGETHER_API_KEY` |
| OpenRouter | `OpenAiChat` | `https://openrouter.ai/api/v1` | `OPENROUTER_API_KEY` |
| Ollama | `OpenAiChat` | `http://localhost:11434/v1` | none |
| vLLM, LM Studio | `OpenAiChat` | your own host | usually none |

Fireworks, xAI, Mistral, and Gemini's own OpenAI-compatible endpoint speak `OpenAiChat` too. Several vendors offer drop-in Claude endpoints that speak `Anthropic`.

Model names are omitted on purpose. They change most often of all, and a wrong one is the single most likely reason a first request fails.

## Local runtimes need no key

```rust
let config = ProviderConfig::new(
    ProviderDialect::OpenAiChat,
    "ollama",
    "http://localhost:11434/v1",
);

let client = Client::without_key(config);
```

`without_key` sends no credentials regardless of the config's `auth`, so there is nothing else to set. Add `.auth(Auth::None)` only if you also want `Client::from_env` to succeed for that endpoint.

## The fields

| Field | Required | Notes |
|---|---|---|
| `dialect` | yes | Which format the endpoint speaks, not which vendor built it |
| `name` | yes | Used for error attribution, so a gateway failure reports the gateway |
| `base_url` | yes | Root URL, no trailing path |
| `auth` | defaulted | Taken from the dialect, override when the endpoint differs |
| `api_key_env` | no | Only needed for `Client::from_env` |
| `default_model` | no | Used when a request does not name a model |
| `extra_headers` | no | Attribution or routing hints some gateways want |

## Auth

`Auth` defaults to whatever the dialect conventionally uses, `Bearer` for OpenAI and a named header for Gemini and Anthropic. Override it when a compatible endpoint authenticates differently from the vendor it imitates:

```rust
let config = ProviderConfig::new(ProviderDialect::Anthropic, "gw", "https://gw.test/v1")
    .auth(Auth::Bearer);
```

For a local runtime with no credentials at all, use `Auth::None` and `Client::without_key`:

```rust
let config = ProviderConfig::new(ProviderDialect::Anthropic, "local", "http://localhost:8080/v1")
    .auth(Auth::None);

let client = Client::without_key(config);
```

`Client::from_env` also succeeds without a key when `auth` is `Auth::None`, so the same startup path works for both.

## The default model is not optional in practice

There is no library-wide default model, because a model name only means something on the endpoint serving it. `gpt-5.6-sol` is meaningless on a third party endpoint, and shipping it as a fallback would produce a confusing 404 rather than a clear error.

So either set `default_model` on the config or `model` on every request. If neither is set, the request fails locally before any network call:

```
invalid request for my-gateway: no model set on the request and no default_model on the endpoint
```

## What compatibility does not guarantee

"Compatible" is a spectrum. An endpoint may implement the format but not tools, ignore `strict`, omit `usage`, or reject a field the real vendor accepts.

Freyja's `UnsupportedCapability` errors are raised by the *dialect*, so they tell you what the format cannot express, not what a particular endpoint declined to implement. Anything in the second category arrives as a `ProviderError::Api` with the endpoint's own message. That is deliberate: Freyja will not pretend to know the capabilities of an endpoint it has never seen.

Capability introspection is Phase 1 work. Until then, the honest test of a compatible endpoint is a real request.

## Starting from a preset

When an endpoint is nearly one Freyja ships, start there and change what differs:

```rust
let config = ProviderType::Anthropic.config()
    .default_model("claude-sonnet-5")
    .header("x-trace-id", trace_id);
```

## Adding a preset

If an endpoint is widely used, it belongs in `src/provider/presets.rs`. Adding one is a single match arm and nothing else, no new module, no change to the neutral model, no change to any dialect. That file is the intended place for contributions.
