# Tool calling

Tool calling lets the model ask you to run a function and then use the result. In Freyja the full round trip works on every provider, and it is what [`Agent`](../building-an-agent.md) is built on.

`#[tool]` generates typed argument parsing and execution. You can drive the loop yourself — choose which tools are available, dispatch requested names, feed the results back — or hand the whole cycle to `Agent`. This page is the tool half; the loop is [Building an agent](../building-an-agent.md).

## The shape of a round trip

1. You send a request with `tools` declared.
2. The model answers with one or more `OutputContent::ToolCall`.
3. You run each call and produce a result string.
4. You append the assistant turn and a `Role::Tool` turn per result.
5. You send the request again. The model uses the results to answer.

Steps 2 through 5 repeat as long as the model keeps asking for tools, which is why real loops need a bound on the number of rounds.

## Declaring a typed tool

```rust
use freyja::{Tool, tool};

#[tool(description = "adds two numbers together", strict = true)]
fn add(a: i64, b: i64) -> i64 {
    a + b
}

let tools = [add];
let definitions = tools.iter().map(|tool| tool.definition()).collect::<Vec<_>>();

let request = GenerateRequest::new()
    .message(Message::text(Role::User, "What is 20 + 22?"))
    .tools(definitions);
```

The macro keeps your function and adds a same-named value whose type implements `Tool`. Its typed parameters generate the object schema, `description` tells the model when to use it, and `strict` defaults to `false`. `Tool::call` deserializes the model's JSON arguments, calls the original function, and serializes the return value as JSON.

Supported functions are free functions, sync or `async fn`, with explicit types and simple identifier parameters. Methods with `self`, generics, and destructured parameters produce compile errors. Argument types must support serde deserialization and `schemars::JsonSchema`; return values must support serde serialization.

`Tool` is a trait, so a tool is any type that answers three questions: what it is called, what the model should be told about it, and what happens when it is called.

```rust
pub type ToolFuture<'a> = Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>>;

pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn definition(&self) -> ToolDefinition;
    fn call<'a>(&'a self, arguments: &'a str, cx: &'a Context) -> ToolFuture<'a>;
}
```

The future is boxed rather than written as `async fn` in the trait. `async fn` in traits is stable, but it is not `dyn`-compatible, and `Agent` keeps its tools as `Arc<dyn Tool>`. It is `Send` so a turn's calls can be driven at once, and it borrows the tool, the arguments and the context — so a call you spawn onto its own task has to own all three first: clone the `Arc<dyn Tool>` and the argument string before spawning.

`ToolError::Arguments` means the model's JSON did not match the Rust parameters. `ToolError::Result` means the return value could not be serialized. `ToolError::Execution` carries a runtime failure. `ToolError` implements `Display` and `std::error::Error`, so format it with `{error}` rather than `{error:?}`: `Execution` then renders as its bare message, which is the text you want the model to read.

## Declaring a raw definition

The existing manual API remains available when a schema does not map cleanly onto a Rust function:

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
| `ToolDefinition::new(name, description)` | Creates a tool taking no arguments, with `parameters` set to the empty object schema |
| `.parameters(Value)` | Sets the JSON Schema for the arguments |
| `.strict(bool)` | Asks the provider to enforce the schema exactly |

The description is what the model reads to decide when to call the tool. Write it for the model, not for other developers. Raw and generated definitions use the same provider-neutral request path.

### A tool taking no arguments still has a schema

`ToolDefinition::new` leaves `parameters` at `{"type": "object", "properties": {}}` rather than at `null`, because a tool with no arguments is a tool whose arguments are an empty object. That distinction is not cosmetic: every dialect sends this field, and none of the four providers accepts `null` in it. OpenAI answers `expected an object, but got null`, and Anthropic requires `input_schema` to be an object.

`parameters` is a public field, so it can still be set to something unusable by hand. Freyja substitutes the empty object schema on the way to the wire for anything that is not a JSON object, so a definition cannot produce a body the provider rejects on this ground.

`Client::check` says nothing about this. It reports what the *dialect* can carry, and a schema is a value the endpoint judges. See [the capability model](../internals/capability-model.md).

## Implementing the trait by hand

`#[tool]` covers a plain function. Write the impl yourself when the tool needs something a function cannot hold — a counter, a connection pool, a client with its own rate limiter:

```rust
use freyja::{Context, Tool, ToolDefinition, ToolFuture};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counter {
    calls: Arc<AtomicUsize>,
}

impl Tool for Counter {
    fn name(&self) -> &str {
        "counter"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("counter", "counts the calls it has served")
    }

    fn call<'a>(&'a self, _arguments: &'a str, _cx: &'a Context) -> ToolFuture<'a> {
        let served = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        Box::pin(async move { Ok(served.to_string()) })
    }
}
```

Note where the work sits. Anything touching `&self` synchronously happens before `Box::pin`, and the future is left owning what it needs. Parsing the arguments outside the future and moving the parsed value in is the same trick, and it is what the macro generates.

## Where state goes

Two kinds of state, two homes, and picking the wrong one is the usual source of lifetime pain.

State known when the tool is *built* goes in the struct's fields, as `Counter` holds its `AtomicUsize`. It belongs to the tool, and it lives as long as the agent does.

State that does not exist until a request arrives — a user id, a tenant, a cancellation token, a tracing span — goes in a `Context`, which is handed to every call of one run:

```rust
struct UserId(String);

let mut cx = Context::new();
cx.insert(UserId("u_42".to_string()));

let run = agent.conversation_in(&mut messages).send_with("what's the weather?", &cx).await?;
```

`Conversation::send_with` takes the same context. `Conversation::send` is the same call with an empty one.

`Context` is keyed by type, the way `http::Extensions` is, so two values of one type collide and the second replaces the first. Give distinct values distinct newtypes — `UserId(String)` above, never a bare `String`. `get::<T>()` returns `Option<&T>`; `require::<T>()` returns a `ToolError::Execution` naming the missing type, which is usually what a tool wants, since that message is what the model ends up reading.

A `#[tool]` function reaches the context by taking `cx: &Context` as its **first** parameter:

```rust
#[tool(description = "names the user this run belongs to")]
fn whoami(cx: &Context) -> Result<String, ToolError> {
    Ok(cx.require::<UserId>()?.0.clone())
}
```

It is recognised by type, and only in first position. It is excluded from the generated schema, so the model sees `whoami` as a tool that takes no arguments and cannot be talked into supplying its own user id.

## Fallible tools

If the last path segment of the return type *as you wrote it* is `Result`, the macro maps `Err(e)` to `ToolError::Execution(e.to_string())`. The error type only has to implement `Display`:

```rust
#[tool(description = "opens the vault")]
fn unseal(code: String) -> Result<String, String> {
    Err("the vault is sealed".to_string())
}
```

The model reads `error: the vault is sealed`. `ToolError::Execution` displays as its bare message, and `Agent` formats a failed call with `{error}`.

The detection is textual, because a macro runs before name resolution and cannot see through an alias. So this one is *not* fallible:

```rust
type Outcome = Result<String, String>;

#[tool(description = "opens the vault")]
fn unseal(code: String) -> Outcome {
    Err("the vault is sealed".to_string())
}
```

It succeeds and serializes the whole `Result` as JSON — `{"Err":"the vault is sealed"}` — which is the behaviour every tool had before the mapping existed. Write `Result<T, E>` out in the signature if you want the mapping.

## Tools defined at runtime

A tool whose name and schema are not known until the program runs — read from configuration, or fetched from a remote registry — is a struct with those values in fields:

```rust
struct Runtime {
    name: String,
}

impl Tool for Runtime {
    fn name(&self) -> &str {
        &self.name
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name.clone(), "defined at runtime")
    }

    fn call<'a>(&'a self, _arguments: &'a str, _cx: &'a Context) -> ToolFuture<'a> {
        Box::pin(async move { Ok("ran the runtime tool".to_string()) })
    }
}

let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(Runtime { name })];
let agent = Agent::new(client).tools(tools);
```

`Agent::tool` takes any `impl Tool + 'static`, which covers `#[tool]` functions and hand-written structs alike. `Agent::tools` takes `IntoIterator<Item = Arc<dyn Tool>>`, for exactly the case above where something else already erased the types. It is not a bulk form of `tool`: `.tools([add, wait])` does not compile, because `#[tool]` gives each function its own type and those two cannot share an array. Write `.tool(add).tool(wait)`.

There is deliberately no `from_fn` constructor. A closure would have to be higher-ranked over both the argument and the context lifetime *and* return a future borrowing both, which inference handles badly enough that every call site ends up annotating its way out. A struct is a few lines and always works.

## Names are unique

Registering two tools under one name replaces rather than shadows: the later registration wins, and the earlier tool and its definition are gone. Overriding a built-in by name is therefore just registering yours after it, and a duplicate name is one tool rather than an ambiguous dispatch.

`definition()` is called once, when the tool is registered, never per run. The schema the model sees is fixed for the life of the agent, so a definition cannot depend on run data — that is what `Context` is for, and `Context` only reaches `call`.

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

`Agent` (see [Building an agent](../building-an-agent.md)) handles this for you: send `Required` on its template to force the first turn to call a tool, and the loop downgrades it to `Auto` for every turn after the first, so the loop can still end in a final answer.

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

`arguments` is a raw JSON string because that is what the transcript has to replay verbatim. Pass it to the matching generated `Tool`, or parse it yourself when using raw definitions:

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
let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(add), Arc::new(wait)];
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
        let output = match tools.iter().find(|tool| tool.name() == name) {
            Some(tool) => tool
                .call(arguments, &cx)
                .await
                .unwrap_or_else(|error| format!("error: {error}")),
            None => format!("error: unknown tool '{name}'"),
        };
        results.push(Message::tool_result(id, output));
    }

    request = request
        .message(response.to_message())
        .extend_messages(results);
}
```

Two tools of different types share one collection only once they are erased, hence `Vec<Arc<dyn Tool>>`. A loop with a single tool can hold it concretely and skip the `Arc`.

The bound is not optional. A model can keep requesting tools indefinitely, and without a cap the loop can run until your budget is gone.

The same loop works while streaming. Drain the stream, call `into_response()`, and everything from `has_tool_calls()` onwards is unchanged. See [Streaming](streaming.md#a-streaming-tool-loop).

## Returning errors to the model

A failed tool is not an error in your program. Report it as the tool's output and let the model recover:

```rust
let output = add
    .call(arguments, &cx)
    .await
    .unwrap_or_else(|error| format!("error: {error}"));
```

Never unwrap on arguments. They come from a model, and a schema is guidance rather than a guarantee, even with `strict` set.

This is why a tool returns `Err` rather than encoding the failure in its success value: `ToolError::Execution` displays as the message you wrote, so the same `{error}` formatting hands it to the model as text.

## Result formatting

`output` is a string. Send JSON when the result is structured, plain text otherwise. Keep it small, since every result becomes part of the prompt on the next round and is billed as input tokens on every subsequent turn.

## Provider differences

**OpenAI** maps this onto flat Responses API input items. A tool call becomes a top level `function_call` item and a result becomes a `function_call_output` item, both siblings of the message items rather than nested inside them. Freyja splits your messages accordingly and preserves transcript order.

**Gemini** maps it onto a flat list of typed steps, `function_call` and `function_result` sitting alongside `user_input` and `model_output`. A result must also repeat the tool `name`, which Freyja resolves from the matching call in the transcript. Verified against the live API. See [Gemini](../providers/gemini.md) and [Gemini wire format](../reference/wire/gemini.md).

**Anthropic** is the one that nests. A tool call is a `tool_use` block inside an assistant message and a result is a `tool_result` block inside a user message, rather than either sitting beside the messages. Because nesting already preserves order, this mapping needs no splitting pass at all. The correlation field is spelled `tool_use_id`, a third spelling after two rounds of `call_id`. Not yet verified against the live API. See [Anthropic](../providers/anthropic.md) and [Anthropic wire format](../reference/wire/anthropic.md).

**OpenAI Chat Completions** nests tool calls on the assistant message like Anthropic, but keeps `arguments` as a JSON string like the Responses API, and is the only dialect giving tool results their own `tool` role. It also wants one result per message rather than several in one turn. Verified live on DeepSeek. See [OpenAI Chat Completions](../providers/openai-chat.md) and [its wire format](../reference/wire/openai-chat.md).

All four native formats are documented in full, so you do not have to read vendor docs to debug a request body.

Two things differ per provider in the tool arguments themselves. OpenAI sends `arguments` as a JSON string, while Gemini and Anthropic both want a structured object, and Freyja converts in each direction so `OutputContent::ToolCall::arguments` is a string everywhere. Anthropic additionally requires that object to be a real object, so a tool call whose arguments are a bare number fails locally with `InvalidRequest` rather than at the API.

## Refusing a call

`Agent::guard` takes a closure consulted before every tool call. It is handed the requested name, the model's raw JSON arguments and the run's `Context`, and answers `Decision::Allow` or `Decision::Deny`:

```rust
use freyja::{Agent, Context, Decision};

let agent = Agent::new(client)
    .tool(add)
    .guard(|name: &str, _arguments: &str, cx: &Context| match name {
        "wipe" => Decision::Deny("destructive tools are off in this run".to_string()),
        _ if cx.get::<UserId>().is_none() => Decision::Deny("no user on this run".to_string()),
        _ => Decision::Allow,
    });
```

The guard runs before the lookup, so it sees every name the model asks for: tools registered up front, tools handed to `Agent::tools` at runtime, and names matching no tool at all. Deny a name nothing answers to and the model reads your reason; the `error: unknown tool '…'` it would otherwise have got never happens.

A `Deny(reason)` reaches the model as the tool result `denied: {reason}`, on the same channel it already reads `error: {error}` and `error: unknown tool '{name}'` from. Nothing else moves: parallel calls are still dispatched concurrently, the guard simply being the first thing each dispatch does, and an agent with no guard behaves exactly as it did before.

It is a closure rather than a trait, so whatever state a policy needs it captures. It is also synchronous. There is no pause for a human to approve a call, and a policy that has to ask a database first is a different feature, one that does not exist.

Two consequences are better read here than discovered in a bill.

**A denied call still costs a turn.** The guard stops the tool, not the loop. A model that keeps asking for the same forbidden tool is stopped by `max_turns`, never by the guard, so the bound stays load-bearing.

**The reason is the model's only way out.** It is everything the model learns about why the call failed. `"denied"` teaches it nothing and it will ask again; `"wipe is disabled, use archive instead"` gets you a different call next turn. A reason the model cannot act on spends the turns you just paid for.

What the guard does not get is the tool's parsed Rust argument struct. Parsing happens inside the tool, after the guard has already decided, so a policy that turns on a field reads the raw JSON and pulls the field out itself. That is the price of one chokepoint that also covers the tools you did not write, whose argument types you do not have.

## What does not exist yet

**Per-tool timeouts are out of scope, deliberately.** Racing a call against a clock needs a timer, and Freyja depends on no async runtime so it has none to reach for. Every caller already does. A short wrapper tool that holds the inner one in an `Arc` and applies your runtime's timeout wraps an erased `Arc<dyn Tool>` as readily as a tool you wrote yourself; the [`Tool`](https://docs.rs/freyja/latest/freyja/trait.Tool.html) documentation carries the implementation. A call that runs out of budget reaches the model as `error: …`, the same channel every other tool failure uses.

What is missing is enforcement that a result's `call_id` matches a real call. Nothing checks it today.
