# Building an agent

An agent is a model that can act. You give it functions, it decides when to call them, you run them, it uses the results. This page builds one end to end.

Everything here runs. `examples/tool_loop.rs` is the finished version, and `cargo run --example tool_loop` executes it.

## The shape of it

```
   you ──── question + tool declarations ────→ model
   you ←─── "run add(20, 22)" ────────────────  model
   you ──── "42" ──────────────────────────────→ model
   you ←─── "20 + 22 is 42" ───────────────────  model
```

The model never runs anything. It asks, you execute, you report back. That boundary is the whole security model: nothing happens that your code did not choose to do.

## Step 1: describe the tool

A tool is a name, a description, and a JSON Schema for its arguments.

```rust
use freya::ToolDefinition;

let add = ToolDefinition::new("add", "adds two numbers together")
    .parameters(serde_json::json!({
        "type": "object",
        "properties": {
            "a": {"type": "integer"},
            "b": {"type": "integer"}
        },
        "required": ["a", "b"]
    }));
```

**The description is a prompt, not a comment.** It is the only thing the model reads when deciding whether this tool applies. Write it for the model. Say when to call it, not just what it does: "adds two numbers together" is weaker than "adds two numbers together; use this instead of doing arithmetic yourself".

Schemas are hand-written JSON today. Deriving them from a Rust type is planned, not present.

## Step 2: write the function

An ordinary function, plus a dispatcher that maps a name and a JSON string onto it.

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

Two rules here, and both matter more than they look.

**Never unwrap on arguments.** They came from a model. A schema is guidance, not a guarantee, even with `strict` set. A panic in your dispatcher takes down the loop.

**Return errors as output, not as `Err`.** A tool that fails has not broken your program; it has produced information. Hand the model the error text and it will usually correct itself, ask a clarifying question, or try another approach. Propagating the error instead ends the conversation over something recoverable.

## Step 3: the first call

```rust
let mut request = GenerateRequest::new()
    .message(Message::text(Role::User, "What is 20 + 22?"))
    .tools([add]);

let response = client.generate(&request).await?;
```

Note what is not set: no model, no temperature, no `tool_choice`. This request is valid on every provider. See [Concepts](concepts.md#3-unset-means-the-vendor-decides).

## Step 4: read what came back

```rust
if response.has_tool_calls() {
    for (id, name, arguments) in response.tool_calls() {
        println!("{name}({arguments})");
    }
} else {
    println!("{}", response.output_text());
}
```

`has_tool_calls()` is the right condition, on every provider. Do **not** branch on `response.status` instead: OpenAI reports `completed` even when a tool call is pending, so a status check will end your loop one turn early.

## Step 5: feed the results back

Two turns go back for each round, and the order is not optional.

```rust
let results: Vec<Message> = response
    .tool_calls()
    .map(|(id, name, arguments)| {
        let output = dispatch(name, arguments);
        Message::tool_result(id, output)
    })
    .collect();

request = request
    .message(response.to_message())   // 1. what the model asked for
    .extend_messages(results);        // 2. what happened
```

**`response.to_message()` is load-bearing.** It rebuilds the assistant turn including tool calls *and* the opaque reasoning state some models require back verbatim. Build that turn by hand from the tool calls alone and the state is lost, and your next request is rejected with something unhelpful like `Request contains an invalid argument`.

If you remember one thing from this page, make it that line.

## The complete loop

```rust
let mut request = GenerateRequest::new()
    .message(Message::text(Role::User, "What is 20 + 22?"))
    .tools([add]);

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

**The bound is not optional.** A model can keep requesting tools indefinitely, and an unbounded loop spends your budget until something else stops it. Five is a starting point; pick a number that matches the task.

## Things that will bite you

### `ToolChoice::Required` inside a loop

It forces a tool call on **every** round, so the model can never produce a final answer, and the loop runs until your bound stops it. Use it for a single forced extraction. Use `Auto`, or leave it unset, for anything that loops.

### Growing transcripts

Every tool result becomes part of the prompt on the next round, and is billed as input tokens on every round after that. A verbose tool is a recurring cost, not a one-time one. Return what the model needs and nothing more.

### One result per call

Each `Message::tool_result` answers exactly one call id. When the model requests three tools in one turn, you send three result messages. The example's `.map().collect()` already does this.

### Parallel calls arrive together

A model may ask for several tools at once. `tool_calls()` yields all of them. Run them concurrently if they are independent, but send every result back before the next request; a call left unanswered is an error on most providers.

## Making it real

The loop above is the skeleton. Three things separate it from something you would deploy.

**Handle errors by kind.** Not every failure is worth retrying. `Http` and an `Api` with 429 or 5xx are transient; `UnsupportedCapability` and `InvalidRequest` never succeed on retry. See [Errors](reference/errors.md).

**Watch the status, not just the content.** A response can come back `Incomplete` because it hit the token cap, or with a refusal. Both are `Ok`, because the call succeeded. See [Responses](reference/responses.md).

**Persist the transcript if the conversation outlives the process.** `Message` is `Serialize` and `Deserialize`, so a `Vec<Message>` goes to disk or a database and comes back without a conversion layer. Keep the reasoning parts when you do; they are part of the transcript, not decoration.

## Next

| | |
|---|---|
| Every builder method on the request | [Requests](reference/requests.md) |
| Roles, content parts, transcripts | [Messages](reference/messages.md) |
| Tool types in full | [Tools](reference/tools.md) |
| Reading responses, statuses, usage | [Responses](reference/responses.md) |
| Which errors are worth retrying | [Errors](reference/errors.md) |
