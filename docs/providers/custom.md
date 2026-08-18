# Custom endpoints

Most hosted inference APIs do not invent a wire format. They copy one, usually OpenAI's or Anthropic's, so that existing client libraries work unchanged. Freyja separates the format from the endpoint so you can take advantage of that without waiting for a preset.

## The two types

| Type | Answers | Example |
|---|---|---|
| `Dialect` | which wire format | `Anthropic` |
| `EndpointConfig` | which endpoint speaks it | `https://api.z.ai/api/anthropic/v1`, `x-api-key`, `glm-4.6` |

A preset is only a `EndpointConfig` with the fields filled in. There is nothing a preset can do that you cannot.

## The quickest version

When all you need is a dialect, a name, a URL, and a key:

```rust
use freyja::{Client, Dialect};

let client = Client::custom(
    Dialect::OpenAiChat,
    "my-gateway",
    "https://gateway.internal/v1",
    std::env::var("GATEWAY_API_KEY")?,
);
```

Auth follows the dialect, so this is `Authorization: Bearer` without saying so. Everything below is for when you need more than those four things.

## Pointing at a compatible endpoint

```rust
use freyja::{Client, EndpointConfig, Dialect};

let config = EndpointConfig::new(
        Dialect::Anthropic,
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
let config = EndpointConfig::new(
    Dialect::OpenAiChat,
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
| `extra_body` | no | Body fields this endpoint wants on every request, see below |
| `token_limit_field` | defaulted | `OpenAiChat` only: which field carries the output cap |

## Extra body fields

Some endpoints want a field on every request that the neutral model has no name for — a safety configuration, a routing hint, a tier. `body` is the companion to `header`, one layer down:

```rust
let config = EndpointConfig::new(Dialect::Gemini, "Gemini", base_url)
    .body(json!({"safety_settings": [{"category": "HARM_CATEGORY_HARASSMENT"}]}));
```

It is deep-merged into the wire body, so it adds to what the dialect built rather than replacing it. A request's own [`extra_for`](../reference/requests.md#extra_for) overrides it, which is the right way round: the endpoint sets a standing default, the call overrides it.

Use `body` for a property of the deployment and `extra_for` for anything that varies per call.

## Auth

`Auth` defaults to whatever the dialect conventionally uses, `Bearer` for OpenAI and a named header for Gemini and Anthropic. Override it when a compatible endpoint authenticates differently from the vendor it imitates:

```rust
let config = EndpointConfig::new(Dialect::Anthropic, "gw", "https://gw.test/v1")
    .auth(Auth::Bearer);
```

For a local runtime with no credentials at all, use `Auth::None` and `Client::without_key`:

```rust
let config = EndpointConfig::new(Dialect::Anthropic, "local", "http://localhost:8080/v1")
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

Freyja's `UnsupportedCapability` errors are raised by the *dialect*, so they tell you what the format cannot express, not what a particular endpoint declined to implement. Anything in the second category arrives as one of the status-bearing variants with the endpoint's own message. That is deliberate: Freyja will not pretend to know the capabilities of an endpoint it has never seen.

`Client::check` will tell you whether the *dialect* can express a request, before it is sent. It will not tell you whether this endpoint implements everything the dialect can express, because Freyja has never seen it. The honest test of a compatible endpoint is still a real request.

## Starting from a preset

When an endpoint is nearly one Freyja ships, start there and change what differs:

```rust
let config = EndpointPreset::Anthropic.config()
    .default_model("claude-sonnet-5")
    .header("x-trace-id", trace_id);
```

## Do not send a preset PR

Popularity is not the bar. `src/endpoint/presets.rs` deliberately holds only the three first-party vendors whose dialects Freyja implements and tests against, and its header comment says so. A preset is a standing promise that a base URL and a default model are still current, and third-party endpoints change both faster than this crate could verify. A stale preset fails at the vendor with a confusing 404; a missing one fails locally with a clear message, or does not fail at all because you supplied the current URL.

That rule is enforced, not just stated. `presets_cover_only_first_party_vendors` asserts the list stays at three, so a PR adding a fourth fails CI by design.

What is welcome instead: a correction to the [compatible-endpoint table](#known-compatible-endpoints) above, which costs a documentation edit rather than a promise, and a bug report for anything a dialect maps wrongly. An endpoint absent from `presets.rs` is no less supported for it, as the whole of this page is about.

## Errors name your endpoint, not the vendor

`Error` carries the `name` from your `EndpointConfig` in every variant, reachable with `error.endpoint()`. So a Claude-compatible gateway configured as `my-gateway` reports `my-gateway`, never `anthropic`, and you can tell which endpoint broke without reading the URL back.

The enum is `#[non_exhaustive]`, so match with a `_` arm. See [Errors](../reference/errors.md).
