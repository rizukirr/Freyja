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

## Step 1: declare the tool

`#[tool]` turns a typed function into a value implementing `Tool`. The function name becomes the model-visible name, its parameters become a JSON Schema, and its return value is serialized as JSON.

```rust
use freyja::{Context, Tool, tool};
use std::sync::Arc;

#[tool(
    description = "adds two numbers together; use this instead of doing arithmetic yourself",
    strict = true
)]
fn add(a: i64, b: i64) -> i64 {
    a + b
}

let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(add)];
let cx = Context::new();
```

**The description is a prompt, not a comment.** It is the only thing the model reads when deciding whether this tool applies. Write it for the model. Say when to call it, not just what it does: "adds two numbers together" is weaker than "adds two numbers together; use this instead of doing arithmetic yourself".

`strict` defaults to `false`. With `strict = true`, Freyja also rewrites the generated schema into the strict subset accepted by providers that enforce it.

The macro accepts free functions with explicit parameter types and simple identifier parameters, sync or `async fn`. Methods, generics, and destructuring patterns are rejected at compile time. Parameter types must support `Deserialize` and `JsonSchema`; the return type must support `Serialize`.

## Step 2: dispatch requested names

Keep the tools in a collection and look up the name requested by the model. `Tool::call` validates the JSON against the Rust parameter types, calls the function, and serializes its result.

```rust
async fn dispatch(tools: &[Arc<dyn Tool>], name: &str, arguments: &str, cx: &Context) -> String {
    match tools.iter().find(|tool| tool.name() == name) {
        Some(tool) => tool
            .call(arguments, cx)
            .await
            .unwrap_or_else(|error| format!("error: {error}")),
        None => format!("error: unknown tool '{name}'"),
    }
}
```

`Arc<dyn Tool>` rather than an array of concrete tools: `#[tool]` gives every function its own type, so two tools only share a collection once they are erased. The `Context` carries per-run state your tools may read — a user id, a request id — and an empty `Context::new()` is fine until you have any. See [Tools](reference/tools.md#where-state-goes).

Two rules here, and both matter more than they look.

**Do not unwrap `Tool::call`.** Arguments came from a model. A schema is guidance rather than a guarantee, even with `strict` set.

**Let a failing tool return `Err`, then turn it into output.** A tool that fails has not broken your program; it has produced information. `ToolError::Execution` displays as the message the tool wrote, so the `{error}` formatting above hands that text straight to the model, and it will usually correct itself, ask a clarifying question, or try another approach. What must not happen is the error escaping `dispatch` with `?` and ending the conversation over something recoverable.

## Step 3: the first call

```rust
let mut request = GenerateRequest::new()
    .message(Message::text(Role::User, "What is 20 + 22?"))
    .tools(tools.iter().map(|tool| tool.definition()).collect::<Vec<_>>());

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
let mut results: Vec<Message> = Vec::new();
for (id, name, arguments) in response.tool_calls() {
    let output = dispatch(&tools, name, arguments, &cx).await;
    results.push(Message::tool_result(id, output));
}

request = request
    .message(response.to_message())   // 1. what the model asked for
    .extend_messages(results);        // 2. what happened
```

**`response.to_message()` is load-bearing.** It rebuilds the assistant turn including tool calls *and* the opaque reasoning state some models require back verbatim. Build that turn by hand from the tool calls alone and the state is lost, and your next request is rejected with something unhelpful like `Request contains an invalid argument`.

If you remember one thing from this page, make it that line.

## The complete loop

```rust
let cx = Context::new();
let mut request = GenerateRequest::new()
    .message(Message::text(Role::User, "What is 20 + 22?"))
    .tools(tools.iter().map(|tool| tool.definition()).collect::<Vec<_>>());

for _ in 0..5 {
    let response = client.generate(&request).await?;

    if !response.has_tool_calls() {
        println!("{}", response.output_text());
        break;
    }

    let mut results: Vec<Message> = Vec::new();
    for (id, name, arguments) in response.tool_calls() {
        let output = dispatch(&tools, name, arguments, &cx).await;
        results.push(Message::tool_result(id, output));
    }

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

Trimming what a tool returns bounds the cost of one round. It does not bound the conversation, which grows until the provider rejects it outright. `Agent::conversation` hands out a conversation holding the transcript in this process, `Agent::conversation_in` takes a backend of your own, and `Conversation::window` bounds what reaches the model each turn, keeping pinned turns and the most recent turn groups. See [Storage](reference/storage.md).

### One result per call

Each `Message::tool_result` answers exactly one call id. When the model requests three tools in one turn, you send three result messages. The loop over `tool_calls()` already does this.

### Parallel calls arrive together

A model may ask for several tools at once. `tool_calls()` yields all of them. Run them concurrently if they are independent, but send every result back before the next request; a call left unanswered is an error on most providers.

### A timeout only bounds a tool that awaits

Freyja has no per-tool budget, deliberately: a timer belongs to a runtime, and Freyja depends on none. You add one by wrapping the tool in another tool that applies your runtime's timeout, which the [`Tool`](https://docs.rs/freyja/latest/freyja/trait.Tool.html) documentation spells out. What the wrapper cannot do is interrupt work that never yields. A timeout fires only when the inner future returns to the executor, so a tool that blocks the thread runs to completion past its budget and starves its siblings while it does, because concurrent dispatch drives them all on one task.

## Making it real

The loop above is the skeleton. Three things separate it from something you would deploy.

**Handle errors by kind.** Not every failure is worth retrying. `Http` and an `Api` with 429 or 5xx are transient; `UnsupportedCapability` and `InvalidRequest` never succeed on retry. See [Errors](reference/errors.md).

**Watch the status, not just the content.** A response can come back `Incomplete` because it hit the token cap, or with a refusal. Both are `Ok`, because the call succeeded. See [Responses](reference/responses.md).

**Persist the transcript if the conversation outlives the process.** `Message` is `Serialize` and `Deserialize`, so a `Vec<Message>` goes to disk or a database and comes back without a conversion layer. Keep the reasoning parts when you do; they are part of the transcript, not decoration.

## Agent runs this loop for you

Everything above — the bound, the tool dispatch, sending results back, watching the status — is what `Agent` does on your behalf, and it dispatches parallel tool calls concurrently rather than one at a time. The hand-written loop stays useful: it is what to reach for when you need to see or change what happens between turns, and it is what `Agent` is built from. For the common case, though:

```rust
let agent = Agent::new(client)
    .model("gpt-4o")
    .temperature(0.2)
    .tool(add)
    .max_turns(5);

let mut messages: Vec<Message> = Vec::new();
let run = agent.conversation_in(&mut messages).send("What is 20 + 22?").await?;

println!("{}", run.answer);
```

Model and sampling settings live on the builder alongside the tools. The transcript does not: it is held by whatever `Storage` you pass to `conversation_in`, which is why there is no way to put messages into an agent's configuration.

A guard goes on the same builder. `.guard(|name, _arguments, _cx| ...)` is consulted before every call, and returning `Decision::Deny(reason)` sends the model `denied: {reason}` instead of running the tool — which is how you keep a tool registered for the cases it is meant for while refusing the rest. See [Refusing a call](reference/tools.md#refusing-a-call).

```rust
let agent = Agent::new(client)
    .tool(add)
    .guard(|name: &str, _arguments: &str, _cx: &Context| match name {
        "add" => Decision::Deny("arithmetic is off limits".to_string()),
        _ => Decision::Allow,
    })
    .max_turns(5);
```

`run.stop` tells you why the loop ended, `Answered`, `MaxTurns`, `Refused`, `Incomplete`, or `Failed`, and `Conversation::send`, reached through `Agent::conversation_in`, wraps the same loop with the transcript held in a `Storage` backend instead of a vector you own, for a multi-turn conversation. See `examples/agent.rs`.

## Next

| | |
|---|---|
| Every builder method on the request | [Requests](reference/requests.md) |
| Roles, content parts, transcripts | [Messages](reference/messages.md) |
| Tool types in full | [Tools](reference/tools.md) |
| Reading responses, statuses, usage | [Responses](reference/responses.md) |
| Which errors are worth retrying | [Errors](reference/errors.md) |
| Letting `Agent` run the loop for you | [Agent runs this loop for you](#agent-runs-this-loop-for-you) |
