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

This is deliberate. A `None` field means "let the provider decide", which keeps a request portable. If `new()` populated `reasoning_effort` or `tool_choice`, a request built without thinking about it would carry capabilities that some providers reject, and a plain "say hello" call would fail on a backend that cannot express them. Freya never fills in a value you did not ask for.

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

The conversation, in order. See [Messages and content](messages.md).

System and developer turns are not sent as ordinary turns. Each provider hoists them into its native system instruction field, joining multiple ones with a blank line between them.

### `max_tokens`

An upper bound on generated tokens. It does not limit the prompt. Hitting it usually surfaces as `ResponseStatus::Incomplete`.

### `temperature` and `top_p`

Sampling controls, forwarded as given. Freya does not clamp or validate the range, so an out of range value comes back as a provider `Api` error.

### `reasoning_effort`

How much internal reasoning the model should spend before answering.

```rust
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}
```

Serialized lowercase. Not every provider accepts every level. Gemini rejects this field entirely today, and Anthropic rejects `Minimal` while mapping `None` onto disabled thinking rather than an effort level. See [Gemini](providers/gemini.md) and [Anthropic](providers/anthropic.md).

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

You get the result as text from `response.output_text()` and parse it yourself. Typed structured output, where a schema is derived from a Rust type and the response is deserialized for you, is Phase 1 work and does not exist yet.

### `tools` and `tool_choice`

Covered in [Tool calling](tools.md).

### `previous_response_id`

Continues a server side conversation instead of resending the transcript. Pass the `id` from an earlier `GenerateResponse`. Providers name this differently on the wire, `previous_response_id` for OpenAI and `previous_interaction_id` for Gemini, and Freya maps it for you.

Anthropic rejects this field with `UnsupportedCapability`, because it keeps no server side transcript at all. Every request carries the full history, so there is nothing to continue from. Code that relies on this field is not portable to Anthropic and has to keep the transcript itself.

### `metadata`

Arbitrary JSON forwarded to the provider, for labels and trace identifiers. Sent as `metadata` to OpenAI and Anthropic, and as `labels` to Gemini. Freya does not read it.

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
