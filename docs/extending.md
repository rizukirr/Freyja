# Extending Freyja

Freyja ships six traits a crate outside it can implement, each with a contract written in its doc comment. This page collects the rules so a crate can honour them and check itself.

## What a crate can extend

| Trait | Its job | An example crate |
|---|---|---|
| `Tool` | Runs one function the model can call, and describes it to the model | A crate exposing a search API, or the tools of an MCP server |
| `Storage` | Holds one conversation between sends | A Postgres, SQLite or Redis backend |
| `Memory` | Chooses what the model sees of the stored conversation | Retrieval over embeddings, or a rolling summary with its own rules |
| `TokenCounter` | Estimates the tokens in one message | A wrapper over a real tokenizer |
| `Model` | Answers a request | A provider Freyja has no dialect for, or a cache in front of a `Client` |
| `Hook` | Runs around every model and tool call and can stop a run | An OpenTelemetry exporter, or a per-user token budget |

## The rules each implementation honours

A `Storage` returns messages oldest first from `load`. Order is the backend's contract, so use a strictly monotonic column and never a timestamp with second resolution. `clear` must succeed on a conversation that is already empty, because the only recourse after a failed clear is to call it again. `clear` also removes everything stored with `set_state`. A backend that keeps state returns the latest value for a key from `state`, and a backend that does not can leave both state methods at their defaults. A backend that keeps no state leaves the summary in each conversation's own cache, so a clear made through another handle on a shared backend is not seen by that conversation, and a backend shared between conversations should keep state.

A `Memory` keeps nothing in itself. What it needs between sends goes in the conversation's state slot through `State`, so one memory value serves every conversation an agent starts. `recall` receives the whole stored conversation, oldest first, and returns what to send. The default `compact` refuses, so only a memory that summarizes overrides it.

A `TokenCounter` is synchronous and counts one message at a time. A closure `Fn(&Message) -> usize + Send` already is one.

A `Model` is `Send + Sync` and is shared through `Arc`, which a forwarding impl for `Arc<T>` and `Box<T>` makes transparent. `name` is what errors carry. `generate` returns `freyja::Error`. `stream` may be left to its default, which awaits `generate` and replays the finished response, so a model that cannot stream still serves a streaming caller, all at once. A server that speaks the OpenAI wire format, such as Ollama or vLLM, needs no `Model` at all, only a `Client` with an `EndpointConfig`. A `Model` is for an in-process model such as llama.cpp bindings or candle, a provider with its own wire format or auth, a wrapper such as a cache, fallback or router, or a fake model for tests.

A `Tool` reports failure as text. A failure the model can recover from is returned as the error text of the call, and it reaches the model as a result rather than ending the run.

A `Hook` has four methods, `before_model`, `after_model`, `before_tool` and `after_tool`, and they are synchronous, so none may block. A hook that must do I/O hands the work to a channel or a task. Every method defaults to `Flow::Continue`, so a hook implements only what it needs. Every hook is called at each point, and when several return `Flow::Stop` the first one wins. A hook sees requests and responses but does not change them.

A hook sees all conversation content and the run's `Context`, and never an API key, because keys stay inside `Client`. A third-party hook crate is trusted like any dependency. The model controls the text it writes, so escape it before a hook writes it to a log. `after_tool` also reports a call the guard refused, with the output `denied: <reason>` and a zero duration, so an audit sees every request. A hook that panics ends the run as a panicking tool does.

## Checking a storage backend

`freyja::testing::storage_conformance` runs the rules above against a backend and panics on the first one broken, naming it. Call it from the backend crate's own tests, with a function that builds a new, empty backend on each call, under any async runtime:

```rust
use freyja::testing::storage_conformance;

#[tokio::test]
async fn my_backend_conforms() {
    storage_conformance(MyBackend::new).await;
}
```

A backend that needs a resource per instance, such as a file or a schema, builds a fresh one in the closure it passes.

## Testing with a scripted model

`freyja::testing::ScriptedModel` answers from a script with no network, so an agent test runs without a key. Each call to the model takes the next response in the script, and `requests()` returns every request it received, in order:

```rust
use freyja::testing::ScriptedModel;
use freyja::{Agent, tool};
use serde_json::json;
use std::sync::Arc;

#[tool(description = "adds two numbers together")]
fn add(a: i64, b: i64) -> i64 {
    a + b
}

#[tokio::test]
async fn the_agent_adds() {
    let model = Arc::new(
        ScriptedModel::new()
            .tool_call("add", json!({"a": 1, "b": 2}))
            .text("3"),
    );
    let run = Agent::new(model.clone()).tool(add).ask("add 1 and 2").await.unwrap();

    assert_eq!(run.answer, "3");
    assert_eq!(model.requests().len(), 2);
}
```

`respond` adds any other response shape from a `GenerateResponse`. A call after the script runs out returns `Error::InvalidResponse`.

## Stability

Every trait returns boxed futures, so each stays usable as `dyn`: an agent holds its tools as `Arc<dyn Tool>`, its memory as `Arc<dyn Memory>` and its model as `Arc<dyn Model>`, and a backend can be erased behind `Box<dyn Storage>`. A method added to a trait later arrives with a default, so an implementation written today keeps compiling. Enums and error types that may grow are `#[non_exhaustive]`, so a match on one needs a trailing `_` arm.
