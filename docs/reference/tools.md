# Tool calling

Tool calling lets the model ask you to run a function and then use the result. In Freyja the full round trip works on every provider, and it is the foundation the agent loop will be built on.

There is no automatic execution yet. You declare the tools, you run them, you feed the results back. A `Tool` trait and a registry that does the dispatch for you are Phase 2 work.

## The shape of a round trip

1. You send a request with `tools` declared.
2. The model answers with one or more `OutputContent::ToolCall`.
3. You run each call and produce a result string.
4. You append the assistant turn and a `Role::Tool` turn per result.
5. You send the request again. The model uses the results to answer.

Steps 2 through 5 repeat as long as the model keeps asking for tools, which is why real loops need a bound on the number of rounds.

## Declaring a tool

```rust
pub struct ToolDefinition {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Value,
    pub strict: Option<bool>,
}
```

```rust
let add = ToolDefinition::new("add", "adds two numbers together")
    .parameters(serde_json::json!({
        "type": "object",
        "properties": {
            "a": {"type": "integer"},
            "b": {"type": "integer"}
        },
        "required": ["a", "b"]
    }));

let request = GenerateRequest::new()
    .message(Message::text(Role::User, "What is 20 + 22?"))
    .tools([add]);
```

| Method | Effect |
|---|---|
| `ToolDefinition::new(name, description)` | Creates a tool with `parameters` set to `Value::Null` |
| `.parameters(Value)` | Sets the JSON Schema for the arguments |
| `.strict(bool)` | Asks the provider to enforce the schema exactly |

The description is what the model reads to decide when to call the tool. Write it for the model, not for other developers. Schemas are hand written JSON Schema today. Deriving them from a Rust type is Phase 1 work.

## Constraining the choice

```rust
pub enum ToolChoice {
    Auto,
    None,
    Required,
    Named(String),
}
```

| Variant | Effect |
|---|---|
| `Auto` | The model decides whether to call a tool |
| `None` | The model may not call any tool |
| `Required` | The model must call some tool |
| `Named(name)` | The model must call this specific tool |

Do not use `Required` inside a loop. It forces a call on every round, so the model can never produce a final answer and the loop runs until your bound stops it. Use it for a single shot, and `Auto` for agent loops.

```rust
let request = request.tool_choice(ToolChoice::Required);
```

Leave `tool_choice` unset unless you need it. Unset means the provider's own default, which is normally `Auto`, and it keeps the request portable. All four dialects carry it, each with its own spelling: `Required` is `any` on Gemini and Anthropic, and `required` on the two OpenAI formats.

## Reading the calls

A response carries tool calls as `OutputContent::ToolCall`:

```rust
pub enum OutputContent {
    Text(String),
    Refusal(String),
    ToolCall { id: String, name: String, arguments: String },
    Reasoning { data: Value },
}
```

`Reasoning` is in the list because a reasoning model interleaves it with the calls, and it has to go back with them. `to_message()` handles that for you; see below.

`arguments` is a raw JSON string, not a parsed value, because that is what the transcript has to replay verbatim. Parse it yourself:

```rust
if response.has_tool_calls() {
    for (id, name, arguments) in response.tool_calls() {
        let args: serde_json::Value = serde_json::from_str(arguments)?;
        println!("{name} wants {args}");
    }
}
```

| Helper | Returns |
|---|---|
| `has_tool_calls()` | `bool`, whether the model asked for at least one call |
| `tool_calls()` | An iterator of `(&str, &str, &str)`, being id, name, arguments |

## Sending results back

Two turns go back for each round: the assistant turn showing what the model asked for, then one `Role::Tool` turn per result.

```rust
let results: Vec<Message> = response
    .tool_calls()
    .map(|(id, name, arguments)| {
        let output = run_tool(name, arguments);
        Message::tool_result(id, output)
    })
    .collect();

request = request
    .message(response.to_message())
    .extend_messages(results);
```

The assistant turn matters, and for more than one reason. Providers correlate a result to a call by id, and a result whose call is missing from the transcript is an error. It also carries opaque reasoning state, which reasoning models require back verbatim. `to_message()` builds that turn for you, including every tool call and every reasoning block the response contained.

Do not hand-assemble this turn. A `Message` you build yourself from the tool calls alone will be missing the reasoning parts, and Gemini rejects the request outright with `Request contains an invalid argument`. Anthropic behaves the same way with its signed `thinking` blocks. Rebuilding an identical looking tool call is not enough, since the signature is what gets validated.

Order matters too. The assistant turn has to come before the results.

## A complete loop

This is the pattern in `examples/tool_loop.rs`, reduced to its essentials:

```rust
let mut request = GenerateRequest::new()
    .message(Message::text(Role::User, "What is 20 + 22?"))
    .tools([add_tool]);

for _ in 0..5 {
    let response = client.generate(&request).await?;

    if !response.has_tool_calls() {
        println!("{}", response.output_text());
        break;
    }

    let results: Vec<Message> = response
        .tool_calls()
        .map(|(id, name, arguments)| Message::tool_result(id, dispatch(name, arguments)))
        .collect();

    request = request
        .message(response.to_message())
        .extend_messages(results);
}
```

The bound is not optional. A model can keep requesting tools indefinitely, and without a cap the loop can run until your budget is gone.

The same loop works while streaming. Drain the stream, call `into_response()`, and everything from `has_tool_calls()` onwards is unchanged. See [Streaming](streaming.md#a-streaming-tool-loop).

## Returning errors to the model

A failed tool is not an error in your program. Report it as the tool's output and let the model recover:

```rust
fn dispatch(name: &str, arguments: &str) -> String {
    let parsed: Value = match serde_json::from_str(arguments) {
        Ok(value) => value,
        Err(error) => return format!("error: arguments were not valid JSON: {error}"),
    };

    match name {
        "add" => match (parsed["a"].as_i64(), parsed["b"].as_i64()) {
            (Some(a), Some(b)) => (a + b).to_string(),
            _ => "error: both 'a' and 'b' must be integers".to_string(),
        },
        other => format!("error: unknown tool '{other}'"),
    }
}
```

Never unwrap on arguments. They come from a model, and a schema is guidance rather than a guarantee, even with `strict` set.

## Result formatting

`output` is a string. Send JSON when the result is structured, plain text otherwise. Keep it small, since every result becomes part of the prompt on the next round and is billed as input tokens on every subsequent turn.

## Provider differences

**OpenAI** maps this onto flat Responses API input items. A tool call becomes a top level `function_call` item and a result becomes a `function_call_output` item, both siblings of the message items rather than nested inside them. Freyja splits your messages accordingly and preserves transcript order.

**Gemini** maps it onto a flat list of typed steps, `function_call` and `function_result` sitting alongside `user_input` and `model_output`. A result must also repeat the tool `name`, which Freyja resolves from the matching call in the transcript. Verified against the live API. See [Gemini](../providers/gemini.md) and [Gemini wire format](../reference/wire/gemini.md).

**Anthropic** is the one that nests. A tool call is a `tool_use` block inside an assistant message and a result is a `tool_result` block inside a user message, rather than either sitting beside the messages. Because nesting already preserves order, this mapping needs no splitting pass at all. The correlation field is spelled `tool_use_id`, a third spelling after two rounds of `call_id`. Not yet verified against the live API. See [Anthropic](../providers/anthropic.md) and [Anthropic wire format](../reference/wire/anthropic.md).

**OpenAI Chat Completions** nests tool calls on the assistant message like Anthropic, but keeps `arguments` as a JSON string like the Responses API, and is the only dialect giving tool results their own `tool` role. It also wants one result per message rather than several in one turn. Verified live on DeepSeek. See [OpenAI Chat Completions](../providers/openai-chat.md) and [its wire format](../reference/wire/openai-chat.md).

All four native formats are documented in full, so you do not have to read vendor docs to debug a request body.

Two things differ per provider in the tool arguments themselves. OpenAI sends `arguments` as a JSON string, while Gemini and Anthropic both want a structured object, and Freyja converts in each direction so `OutputContent::ToolCall::arguments` is a string everywhere. Anthropic additionally requires that object to be a real object, so a tool call whose arguments are a bare number fails locally with `InvalidRequest` rather than at the API.

## What does not exist yet

- No `Tool` trait, no registry, no automatic dispatch by name
- No macro to derive a schema from a function signature
- No parallel execution of tool calls
- No per tool timeouts or approval hooks
- No enforcement that a result's `call_id` matches a real call

These are Phase 2 items on the [roadmap](../../README.md#roadmap).
