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
| `provider_metadata` | Provider fields Freyja does not model, preserved verbatim |

## OutputContent

```rust
pub enum OutputContent {
    Text(String),
    Refusal(String),
    ToolCall { id: String, name: String, arguments: String },
    Reasoning { data: Value },
}
```

| Variant | Meaning |
|---|---|
| `Text` | Generated text. A response can contain several parts |
| `Refusal` | The model declined to answer |
| `ToolCall` | The model wants a tool executed. `arguments` is a raw JSON string |
| `Reasoning` | Opaque provider state that must be replayed verbatim |

`Refusal` is distinct from `Text` on purpose. A refusal is not an answer, and folding it into text would hide that from callers. Note that `output_text()` excludes refusals, so a refusal shows up as an empty string there. Check `content` directly when you need to tell the two apart.

Only OpenAI emits `Refusal` today. Gemini's response format carries no separate refusal block that Freyja parses. Anthropic signals a refusal through `stop_reason` instead, which arrives as `ResponseStatus::Other("refusal")` with `content` empty or partial, so check `status` before reading `content` or you will mistake a refusal for an empty answer.

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

`Reasoning` is where anything Freyja does not model ends up, rather than being dropped. Gemini thought signatures and OpenAI reasoning items both land here. Ignore it unless you are assembling a transcript by hand, in which case preserve it exactly and in order. See [Tool calling](../reference/tools.md).

### `to_message`

```rust
pub fn to_message(&self) -> Message
```

Converts the response into a `Role::Assistant` turn so it can be appended to the transcript. Text stays text, tool calls become `InputContent::ToolCall`, and refusals become text, because that is how a refusal reads in a transcript.

```rust
request = request.message(response.to_message());
```

You need this before sending tool results back. See [Tool calling](../reference/tools.md).

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
| `Other(String)` | A status Freyja does not model, preserved verbatim |

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
| `end_turn`, `stop_sequence` (Anthropic) | `Completed` |
| `max_tokens` (Anthropic) | `Incomplete` |
| `tool_use` (Anthropic) | `RequiresAction` |
| `refusal`, `pause_turn` (Anthropic) | `Other` |
| anything else | `Other` |

Anthropic reports its reason as `stop_reason` rather than `status`, and two of its values deliberately stay as `Other` rather than being flattened into a near neighbour. A `refusal` is not a `Failed`, the request succeeded and the model chose not to answer. A `pause_turn` is not a `RequiresAction` either, since it is resumed by re-sending the transcript rather than by supplying a tool result, so treating it as one would send an agent loop hunting for tool calls that are not there.

A non `Completed` status is not an error. The call succeeded, so you get `Ok`. Only transport and API failures produce `Err`. See [Errors](../reference/errors.md).

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

Field names are normalized. Gemini reports `total_input_tokens` and `total_output_tokens` on the wire, and Freyja maps them onto the same three fields so cost accounting does not have to branch per provider.

Anthropic needs more than renaming. It reports no total at all, so Freyja computes one, and its `input_tokens` counts only the *uncached* part of the prompt, with cached tokens reported in two separate fields. Freyja sums all three, so `input_tokens` means the same thing on every provider. The unsummed fields stay in `provider_metadata`, which matters if you price cache reads and cache writes separately, since they do not cost the same. See [Anthropic](../providers/anthropic.md).

Cost calculation is not included. Freyja reports tokens, not money.

## provider_metadata

Anything in the response body Freyja does not model is captured here rather than dropped, using serde's flatten. Use it to read provider specific fields without waiting for Freyja to add support:

```rust
if let Some(meta) = &response.provider_metadata {
    if let Some(fingerprint) = meta.get("system_fingerprint") {
        println!("fingerprint: {fingerprint}");
    }
}
```

The same forward compatibility applies inside `content`. Unknown output item types and unknown content block types are skipped rather than failing deserialization, so a provider shipping a new block type does not break your build. The tradeoff is that new content silently disappears until Freyja models it. Check `provider_metadata` when output looks shorter than expected.

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
