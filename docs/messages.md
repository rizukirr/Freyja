# Messages and content

A conversation is a `Vec<Message>`. A message pairs a role with one or more content parts.

```rust
pub struct Message {
    pub role: Role,
    pub content: Vec<InputContent>,
}
```

Derives `Debug`, `Clone`, `Serialize`, `Deserialize`, `PartialEq`. Being serializable means you can persist a transcript to disk or a database and load it back without writing a conversion layer.

## Role

```rust
pub enum Role {
    System,
    Developer,
    User,
    Assistant,
    Tool,
}
```

Derives `Debug`, `Clone`, `Copy`, `Serialize`, `Deserialize`, `PartialEq`, `Eq`. Serialized lowercase.

| Role | Meaning |
|---|---|
| `System` | Instructions framing the whole conversation |
| `Developer` | Application instructions, ranked above the user |
| `User` | The end user |
| `Assistant` | The model |
| `Tool` | The result of running a tool the model asked for |

### System and developer turns are hoisted

No provider sends these as ordinary turns. Each one lifts them into its native system instruction field, `instructions` for OpenAI, `system_instruction` for Gemini, and `system` for Anthropic. When there are several, their text is joined with a blank line between them, in the order you supplied.

Position in the message list does not matter for these two roles, so a system turn placed halfway through a transcript still applies to the whole conversation. Keep them at the front anyway, so the transcript reads the way it behaves.

Only text is allowed in a system or developer turn. Anything else returns `ProviderError::UnsupportedCapability`.

## InputContent

```rust
pub enum InputContent {
    Text(String),
    ImageUrl(String),
    ToolCall { id: String, name: String, arguments: String },
    ToolResult { call_id: String, output: String },
    Reasoning { data: Value },
}
```

| Variant | Belongs on | Purpose |
|---|---|---|
| `Text` | any role except `Tool` | Plain text |
| `ImageUrl` | `User` only | An image, by URL or data URI |
| `ToolCall` | `Assistant` | A call the model made, echoed back into the transcript |
| `ToolResult` | `Tool` | The output of running that call |
| `Reasoning` | `Assistant` | Opaque provider state, replayed verbatim |

`ToolCall` and `ToolResult` exist so a tool round trip can be replayed to the model on the next request. See [Tool calling](tools.md).

## Constructors

### `Message::text`

```rust
pub fn text(role: Role, text: impl Into<String>) -> Self
```

A single part text message, which is the common case.

```rust
Message::text(Role::User, "What is 20 + 22?")
```

### `Message::new`

```rust
pub fn new(role: Role, content: impl Into<Vec<InputContent>>) -> Self
```

A message with arbitrary parts, for multimodal turns or an assistant turn carrying both text and a tool call.

```rust
Message::new(Role::User, vec![
    InputContent::Text("What is in this picture?".into()),
    InputContent::ImageUrl("https://example.com/cat.png".into()),
])
```

### `Message::tool_result`

```rust
pub fn tool_result(call_id: impl Into<String>, output: impl Into<String>) -> Self
```

Builds the `Role::Tool` turn that carries a tool's output back. `call_id` must match the `id` of the `OutputContent::ToolCall` you are answering.

```rust
Message::tool_result("call_1", "42")
```

`output` is a string. Send JSON as a JSON string when the result is structured, and plain text otherwise. Both providers handle both.

## Multimodal input

Images ride alongside text in a user turn:

```rust
let request = GenerateRequest::new().message(Message::new(Role::User, vec![
    InputContent::Text("Describe this.".into()),
    InputContent::ImageUrl("https://example.com/photo.jpg".into()),
]));
```

Data URIs work the same way, which is how you send local files:

```rust
InputContent::ImageUrl(format!("data:image/png;base64,{encoded}"))
```

Images are only accepted on `Role::User`. On any other role Freya returns `UnsupportedCapability` with the capability `"images outside user messages"`.

## Validation done before the network

Both providers reject malformed transcripts during conversion, so you get an error locally instead of a rejection from the vendor:

| Problem | Error |
|---|---|
| Non text content in a system or developer turn | `UnsupportedCapability` |
| An image outside a user turn | `UnsupportedCapability` |
| Text on a `Role::Tool` turn | `InvalidRequest` |

A tool turn may only carry `ToolResult` parts. If you want to add commentary alongside a tool result, put it in a separate user or assistant turn.

## Building a transcript

```rust
let mut messages = vec![
    Message::text(Role::System, "You are a terse assistant."),
    Message::text(Role::User, "What is 20 + 22?"),
];

// after a response that asked for a tool
messages.push(response.to_message());
messages.push(Message::tool_result("call_1", "42"));

let request = GenerateRequest::new().messages(messages);
```

`GenerateResponse::to_message()` converts a response into the assistant turn, including any tool calls it contained, so you never assemble that turn by hand. See [Responses](responses.md).
