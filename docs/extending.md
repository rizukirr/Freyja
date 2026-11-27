# Extending Freyja

Freyja ships four traits a crate outside it can implement, each with a contract written in its doc comment. This page collects the rules so a crate can honour them and check itself.

## What a crate can extend

| Trait | Its job | An example crate |
|---|---|---|
| `Tool` | Runs one function the model can call, and describes it to the model | A crate exposing a search API, or the tools of an MCP server |
| `Storage` | Holds one conversation between sends | A Postgres, SQLite or Redis backend |
| `Memory` | Chooses what the model sees of the stored conversation | Retrieval over embeddings, or a rolling summary with its own rules |
| `TokenCounter` | Estimates the tokens in one message | A wrapper over a real tokenizer |

## The rules each implementation honours

A `Storage` returns messages oldest first from `load`. Order is the backend's contract, so use a strictly monotonic column and never a timestamp with second resolution. `clear` must succeed on a conversation that is already empty, because the only recourse after a failed clear is to call it again. `clear` also removes everything stored with `set_state`. A backend that keeps state returns the latest value for a key from `state`, and a backend that does not can leave both state methods at their defaults. A backend that keeps no state leaves the summary in each conversation's own cache, so a clear made through another handle on a shared backend is not seen by that conversation, and a backend shared between conversations should keep state.

A `Memory` keeps nothing in itself. What it needs between sends goes in the conversation's state slot through `State`, so one memory value serves every conversation an agent starts. `recall` receives the whole stored conversation, oldest first, and returns what to send. The default `compact` refuses, so only a memory that summarizes overrides it.

A `TokenCounter` is synchronous and counts one message at a time. A closure `Fn(&Message) -> usize + Send` already is one.

A `Tool` reports failure as text. A failure the model can recover from is returned as the error text of the call, and it reaches the model as a result rather than ending the run.

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

## Stability

Every trait returns boxed futures, so each stays usable as `dyn`: an agent holds its tools as `Arc<dyn Tool>` and its memory as `Arc<dyn Memory>`, and a backend can be erased behind `Box<dyn Storage>`. A method added to a trait later arrives with a default, so an implementation written today keeps compiling. Enums and error types that may grow are `#[non_exhaustive]`, so a match on one needs a trailing `_` arm.
