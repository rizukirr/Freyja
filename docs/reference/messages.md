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

Most dialects do not send these as ordinary turns. They lift them into a native system instruction field, `instructions` for OpenAI Responses, `system_instruction` for Gemini, and `system` for Anthropic. When there are several, their text is joined with a blank line between them, in the order you supplied.

**OpenAI Chat Completions is the exception.** There `system` is a real message role, so the turns stay in the array where you put them, and `Developer` maps onto `system` because most compatible endpoints do not know a `developer` role. See [OpenAI Chat Completions](../providers/openai-chat.md).

On the hoisting dialects, position in the message list does not matter for these two roles, so a system turn placed halfway through a transcript still applies to the whole conversation. On OpenAI Chat Completions it does matter, since the turn stays where you put it. Keep system turns at the front and the behaviour is the same everywhere.

Only text is allowed in a system or developer turn on the hoisting dialects, since the target field is a plain string. Anything else returns `Error::UnsupportedCapability`. Keep to text regardless, so the transcript stays portable.

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

`ToolCall` and `ToolResult` exist so a tool round trip can be replayed to the model on the next request. See [Tool calling](../reference/tools.md).

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

`output` is a string. Send JSON as a JSON string when the result is structured, and plain text otherwise. Every dialect handles both.

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

Which roles may carry an image depends on the dialect, and the answer came from asking the endpoints rather than from reading them:

| Dialect | Roles that take an image |
|---|---|
| OpenAI Chat Completions | any, user, system, assistant, and tool all verified live |
| Gemini | user and assistant; a tool turn is `InvalidRequest`, as its text already was |
| OpenAI Responses | user only, an assistant turn takes `output_text` and `refusal` and nothing else |
| Anthropic | user only, **unverified**, the refusal predates any probe and no key has been available to settle it |

Where a dialect will not carry one, Freyja returns `UnsupportedCapability` with the capability `"images outside user messages"`.

## Validation done before the network

Conversion checks the transcript before the request leaves the process, so you get an error locally instead of a rejection from the vendor:

| Problem | Error | Where |
|---|---|---|
| Non text content in a system or developer turn | `UnsupportedCapability` | all except OpenAI Chat Completions |
| An image outside a user turn | `UnsupportedCapability` | all |
| Text on a `Role::Tool` turn | `InvalidRequest` | OpenAI Responses, Gemini |

The last row is not universal. Anthropic collapses a `Role::Tool` turn into a user turn, where the text becomes an ordinary text block, and OpenAI Chat Completions folds it into the tool message's own text. Neither one errors. Keep tool turns to `ToolResult` parts anyway, or the same transcript means different things on different backends. If you want commentary alongside a tool result, put it in a separate user or assistant turn.

The first row is uneven for the same kind of reason: OpenAI Chat Completions keeps system turns as ordinary messages rather than hoisting them into a text-only field, so it has nothing to refuse. See [Errors](../reference/errors.md).

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

`GenerateResponse::to_message()` converts a response into the assistant turn, including any tool calls it contained, so you never assemble that turn by hand. See [Responses](../reference/responses.md).
