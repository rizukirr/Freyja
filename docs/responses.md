# Responses

`GenerateResponse` is the normalized answer. Whatever the vendor returned, this is the shape you work with.

```rust
pub struct GenerateResponse {
    pub id: String,
    pub model: String,
    pub status: ResponseStatus,
    pub content: Vec<OutputContent>,
    pub usage: Option<Usage>,
    pub provider_metadata: Option<Value>,
}
```

Derives `Debug`, `Clone`, `PartialEq`.

| Field | Meaning |
|---|---|
| `id` | Provider assigned identifier, usable as `previous_response_id` |
| `model` | The model that actually served the request, which can differ from what you asked for |
| `status` | Why the response ended |
| `content` | The parts the model produced, in order |
| `usage` | Token accounting, when the provider reports it |
| `provider_metadata` | Provider fields Freya does not model, preserved verbatim |

## OutputContent

```rust
pub enum OutputContent {
    Text(String),
    Refusal(String),
    ToolCall { id: String, name: String, arguments: String },
}
```

| Variant | Meaning |
|---|---|
| `Text` | Generated text. A response can contain several parts |
| `Refusal` | The model declined to answer |
| `ToolCall` | The model wants a tool executed. `arguments` is a raw JSON string |

`Refusal` is distinct from `Text` on purpose. A refusal is not an answer, and folding it into text would hide that from callers. Note that `output_text()` excludes refusals, so a refusal shows up as an empty string there. Check `content` directly when you need to tell the two apart.

Gemini does not currently emit `Refusal`, since its response format does not carry a separate refusal block that Freya parses.

## Helpers

### `output_text`

```rust
pub fn output_text(&self) -> String
```

Concatenates every `Text` part into one string. Refusals and tool calls are skipped. This is the fast path when you just want the answer.

```rust
println!("{}", response.output_text());
```

### `tool_calls`

```rust
pub fn tool_calls(&self) -> impl Iterator<Item = (&str, &str, &str)>
```

Iterates the tool calls as `(id, name, arguments)`. Borrows, so no allocation.

### `has_tool_calls`

```rust
pub fn has_tool_calls(&self) -> bool
```

Whether the model asked for at least one call. This is the loop condition in an agent loop, and it is more reliable than checking `status`, since providers differ in whether they report `requires_action`.

### `to_message`

```rust
pub fn to_message(&self) -> Message
```

Converts the response into a `Role::Assistant` turn so it can be appended to the transcript. Text stays text, tool calls become `InputContent::ToolCall`, and refusals become text, because that is how a refusal reads in a transcript.

```rust
request = request.message(response.to_message());
```

You need this before sending tool results back. See [Tool calling](tools.md).

## ResponseStatus

```rust
pub enum ResponseStatus {
    Completed,
    Incomplete,
    RequiresAction,
    Failed,
    Other(String),
}
```

| Variant | Meaning |
|---|---|
| `Completed` | The model finished normally |
| `Incomplete` | Cut short, typically by `max_tokens` |
| `RequiresAction` | Waiting on tool results |
| `Failed` | The provider could not produce a response |
| `Other(String)` | A status Freya does not model, preserved verbatim |

`Other` exists so a provider adding a new status does not break parsing. When you match on status, handle it rather than assuming the four known variants are exhaustive.

Provider status strings map like this:

| Wire value | Variant |
|---|---|
| `completed` | `Completed` |
| `incomplete` | `Incomplete` |
| `budget_exceeded` (Gemini) | `Incomplete` |
| `requires_action` | `RequiresAction` |
| `failed` | `Failed` |
| `cancelled` (Gemini) | `Failed` |
| anything else | `Other` |

A non `Completed` status is not an error. The call succeeded, so you get `Ok`. Only transport and API failures produce `Err`. See [Errors](errors.md).

## Usage

```rust
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}
```

Derives `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`.

`Option`, since not every provider reports usage on every response.

```rust
if let Some(usage) = response.usage {
    println!("{} in, {} out, {} total",
        usage.input_tokens, usage.output_tokens, usage.total_tokens);
}
```

Field names are normalized. Gemini reports `total_input_tokens` and `total_output_tokens` on the wire, and Freya maps them onto the same three fields so cost accounting does not have to branch per provider.

Cost calculation is not included. Freya reports tokens, not money.

## provider_metadata

Anything in the response body Freya does not model is captured here rather than dropped, using serde's flatten. Use it to read provider specific fields without waiting for Freya to add support:

```rust
if let Some(meta) = &response.provider_metadata {
    if let Some(fingerprint) = meta.get("system_fingerprint") {
        println!("fingerprint: {fingerprint}");
    }
}
```

The same forward compatibility applies inside `content`. Unknown output item types and unknown content block types are skipped rather than failing deserialization, so a provider shipping a new block type does not break your build. The tradeoff is that new content silently disappears until Freya models it. Check `provider_metadata` when output looks shorter than expected.

## Handling a response

```rust
let response = client.generate(&request).await?;

match response.status {
    ResponseStatus::Completed => println!("{}", response.output_text()),
    ResponseStatus::Incomplete => {
        println!("truncated: {}", response.output_text());
    }
    ResponseStatus::RequiresAction => {
        // run the tools, see docs/tools.md
    }
    ResponseStatus::Failed => eprintln!("the provider failed to answer"),
    ResponseStatus::Other(ref status) => eprintln!("unhandled status: {status}"),
}
```
