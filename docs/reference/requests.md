# Requests

`GenerateRequest` describes what you want from a model, without naming a vendor. Every field is public, and every field has a chainable builder method.

```rust
pub struct GenerateRequest {
    pub model: Option<String>,
    pub messages: Vec<Message>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub response_format: Option<ResponseFormat>,
    pub tools: Vec<ToolDefinition>,
    pub tool_choice: Option<ToolChoice>,
    pub previous_response_id: Option<String>,
    pub metadata: Option<Value>,
}
```

Derives `Debug`, `Clone`, `Default`, `PartialEq`.

## No invented defaults

`GenerateRequest::new()` is exactly `Default::default()`. It sets nothing.

This is deliberate. A `None` field means "let the provider decide", which keeps a request portable. If `new()` populated `reasoning_effort` or `tool_choice`, a request built without thinking about it would carry capabilities that some providers reject, and a plain "say hello" call would fail on a backend that cannot express them. Freyja never fills in a value you did not ask for.

The practical consequence: only set a field when you actually care about it.

## Builder methods

Every method takes `self` and returns `Self`, so calls chain.

| Method | Sets |
|---|---|
| `model(impl Into<String>)` | `model` |
| `message(Message)` | pushes one message |
| `messages(impl Into<Vec<Message>>)` | replaces the whole conversation |
| `extend_messages(impl IntoIterator<Item = Message>)` | appends several messages |
| `max_tokens(u32)` | `max_tokens` |
| `temperature(f32)` | `temperature` |
| `top_p(f32)` | `top_p` |
| `tools(impl Into<Vec<ToolDefinition>>)` | replaces the tool list |
| `tool_choice(ToolChoice)` | `tool_choice` |
| `reasoning_effort(ReasoningEffort)` | `reasoning_effort` |
| `response_format(ResponseFormat)` | `response_format` |
| `previous_response_id(impl Into<String>)` | `previous_response_id` |
| `metadata(Value)` | `metadata` |

```rust
let request = GenerateRequest::new()
    .model("gpt-5.6-sol")
    .message(Message::text(Role::System, "Be concise."))
    .message(Message::text(Role::User, "Summarize this release."))
    .max_tokens(512)
    .temperature(0.2);
```

Note the split between `message` and `messages`: `message` appends one, `messages` replaces everything. `extend_messages` appends many, which is what the tool loop uses to add a batch of tool results in one step.

## Fields in detail

### `model`

The model identifier, passed through untouched. When `None`, each provider substitutes its own default. Those defaults are listed on the provider pages, and they will change as vendors ship new models, so pin a model explicitly if you need stability.

### `messages`

The conversation, in order. See [Messages and content](../reference/messages.md).

System and developer turns are not sent as ordinary turns. Each provider hoists them into its native system instruction field, joining multiple ones with a blank line between them.

### `max_tokens`

An upper bound on generated tokens. It does not limit the prompt. Hitting it usually surfaces as `ResponseStatus::Incomplete`.

### `temperature` and `top_p`

Sampling controls, forwarded as given. Freyja does not clamp or validate the range, so an out of range value comes back as a provider `Api` error.

### `reasoning_effort`

How much internal reasoning the model should spend before answering.

```rust
pub enum ReasoningEffort {
    None,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}
```

Serialized lowercase. Not every provider accepts every level:

| Level | OpenAI Responses | OpenAI Chat | Gemini | Anthropic |
|---|---|---|---|---|
| `None` | yes | yes | vendor 400 | disables thinking |
| `Low`, `Medium`, `High` | yes | yes | yes | yes |
| `Xhigh` | yes | yes | vendor 400 | yes |
| `Max` | yes | vendor 400 | vendor 400 | yes |

Every cell is a request that gets sent. Freyja refuses none of them: all four dialects have somewhere to put this field — Gemini `generation_config.thinking_level`, Anthropic `output_config.effort` — so which values a given endpoint likes is the endpoint's answer to give, and it changes faster than a table in a library could. See [Gemini](../providers/gemini.md#reasoning-effort-is-nested-and-half-of-it-is-rejected) and [Anthropic](../providers/anthropic.md).

There was a `Minimal` here until it was removed, on the grounds that nothing accepted it. That reason was wrong — Gemini's `thinking_level` takes `minimal`, and the probe missed it because the dialect refused the whole field before sending anything. The conclusion survives for a better reason: one vendor of the three accepts it, and a rung only one vendor has is not a rung on a portable ladder. Gemini's `minimal` is unreachable until there is an escape hatch to reach it through.

The levels are not uniform even within one vendor. OpenAI's Responses API accepts `Max` and its Chat Completions API does not, on the same model — which is why Freyja passes the value through and lets the endpoint answer rather than keeping a table of who takes what.

### `response_format`

Constrains the shape of the answer.

```rust
pub enum ResponseFormat {
    Text,
    JsonObject,
    JsonSchema { name: String, schema: Value, strict: bool },
}
```

```rust
let request = GenerateRequest::new()
    .message(Message::text(Role::User, "Extract the name and age."))
    .response_format(ResponseFormat::JsonSchema {
        name: "person".into(),
        schema: serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "age": {"type": "integer"}
            },
            "required": ["name", "age"],
            "additionalProperties": false
        }),
        strict: true,
    });
```

[`Client::generate_as`](client.md#generate_as) deserializes the answer into your type. The manual route is still there — `output_text()` and `serde_json::from_str` — and is what you want when the raw text matters as much as the value.

The half that does not exist is *writing* the schema. It is written by hand above and must be kept in step with the struct it describes; deriving one from a Rust type is not implemented.

Making a schema acceptable is handled: [`strict_schema`](#strict_schema) rewrites one into the subset strict mode takes.

### `tools` and `tool_choice`

Covered in [Tool calling](../reference/tools.md).

### `previous_response_id`

Continues a server side conversation instead of resending the transcript. Pass the `id` from an earlier `GenerateResponse`. Providers name this differently on the wire, `previous_response_id` for OpenAI and `previous_interaction_id` for Gemini, and Freyja maps it for you.

Anthropic and OpenAI Chat Completions both reject this field with `UnsupportedCapability { capability: "server-side conversation continuation" }`, because neither keeps a server side transcript. Every request carries the full history, so there is nothing to continue from. Code that relies on this field runs on OpenAI Responses and Gemini only, and has to keep the transcript itself everywhere else.

### `metadata`

Arbitrary JSON forwarded to the provider, for labels and trace identifiers. Sent as `metadata` to OpenAI and Anthropic. Freyja does not read it.

Gemini sends it as `labels`, and the public Interactions API declines the field — see [Gemini](../providers/gemini.md#metadata-is-sent-and-this-endpoint-declines-it). The rejection is the endpoint's, not Freyja's.

### `extra_for`

The escape hatch, for capabilities the neutral model does not name.

```rust
use freyja::{GenerateRequest, Dialect};
use serde_json::json;

let request = GenerateRequest::new()
    .extra_for(Dialect::Gemini, json!({"generation_config": {"seed": 42}}));
```

A field earns a place in `GenerateRequest` by meaning the same thing on more than one dialect. Gemini's `seed` and `safety_settings`, Anthropic's memory tool, and OpenAI's `context_management` do not, so they are not modelled — and this is how to reach them without forking.

**The merge is deep.** Nested objects merge key by key, so the example above adds `seed` to the `generation_config` Freyja already built rather than replacing it. Anything that is not an object replaces what was there, including arrays: an override of `tools` is a replacement, never an append.

**It is scoped to a dialect, and that is what keeps the request portable.** The same `GenerateRequest` still runs against OpenAI, which never sees the field. An application that switches vendors at runtime does not have to strip its extras first.

**Nothing here is checked.** `Client::check` reports what the wire format can carry; this is by definition outside what Freyja knows about the format, so a wrong key comes back from the endpoint:

```
Gemini rejected the request: {"error":{"message":"Unknown parameter 'not_a_real_parameter'."}}
```

Calls accumulate, and a later one wins a collision. For fields an endpoint always wants rather than one call, use [`EndpointConfig::body`](../providers/custom.md#extra-body-fields) instead; a request's own extras override it.

## `strict_schema`

```rust
pub fn strict_schema(schema: Value) -> Value
```

OpenAI's strict mode accepts a *subset* of JSON Schema, and a schema written to spec is rejected rather than trimmed:

```
Invalid schema for response_format: 'additionalProperties' is required
to be supplied and to be false.
```

Freyja does not generate schemas. This takes one you already have — hand-written, `schemars`, anything — and rewrites it into the subset. It is idempotent, so a schema already in the subset passes through unchanged.

### What it changes

| | |
|---|---|
| `additionalProperties: false` | Added to every object, nested ones included |
| `required` | Every property moved into it |
| Optional properties | Gain `null` in their type first, so "may be absent" becomes "may be null" rather than "must be present" |
| `oneOf` | Renamed to `anyOf`, which is the only one strict mode permits |
| `uniqueItems`, `minProperties`, `maxProperties`, `dependentRequired`, `dependentSchemas` | Removed |

Each removal is a keyword strict mode rejects *and* which only narrows a value, so the type survives the round trip.

All of it applies at schema positions only. A property *named* `oneOf` or `uniqueItems` is a name you chose, not a keyword, and is left exactly as written — as is anything under `enum`, `const`, or `default`, which are values rather than schemas.

### What it leaves alone

`allOf`, `not`, `if`/`then`/`else`, `contains`, and `propertyNames` are rejected by strict mode too, and are **not** removed. Dropping them would change which documents the schema describes, and quietly sending a different contract than you wrote is worse than the endpoint refusing. They arrive as `BadRequest` naming the keyword.

It also leaves alone everything strict mode accepts, which is more than it looks: `format`, `pattern`, `enum`, `const`, `minimum`, `maximum`, `minItems`, `maxItems`, `minLength`, `maxLength`, `default`, `examples`, `title`, `description`, `$ref`, `$defs`, `anyOf`, `prefixItems`, `patternProperties`. Those were probed one at a time against the live endpoint, because stripping a constraint the model could have used is a real cost.

### Scope

The rules are OpenAI's. Gemini accepts schemas with or without them, and Anthropic is unverified, so one function serves all three. If a vendor wants something different it gains a dialect parameter.

## Reusing a request across turns

Because `generate` borrows the request, you can keep extending one across a conversation:

```rust
let mut request = GenerateRequest::new()
    .message(Message::text(Role::User, "What is 20 + 22?"));

let response = client.generate(&request).await?;

request = request
    .message(response.to_message())
    .message(Message::text(Role::User, "Now double it."));

let second = client.generate(&request).await?;
```

This is the same pattern the tool loop uses, and it keeps model, tools, and sampling settings pinned across every turn without repeating them.
