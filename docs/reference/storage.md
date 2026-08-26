# Storage

`Storage` is where a conversation lives between calls to [`Agent::message`](../building-an-agent.md). One value is one conversation: a backend holds a single transcript, not a table of them, so an application juggling several conversations builds one `Storage` per conversation rather than asking a shared one to keep them apart.

## The three methods

```rust
pub trait Storage: Send + Sync {
    fn load(&self) -> StorageFuture<'_, Vec<Message>>;
    fn append(&self, messages: Vec<Message>) -> StorageFuture<'_, ()>;
    fn clear(&self) -> StorageFuture<'_, ()>;
}
```

`load` returns the conversation so far, oldest first. `append` adds new turns to the end, and is called with exactly the turns one run produced, never the whole transcript again. `clear` empties the conversation. All three are async and fallible, so a backend that talks to a database or a remote store fits the trait as it stands.

Register one with `Agent::memory`:

```rust
let agent = Agent::new(client).memory(InMemoryStorage::new());
```

## `InMemoryStorage`

```rust
impl InMemoryStorage {
    pub fn new() -> Self
}
```

`InMemoryStorage::new()` starts empty and holds the conversation in a `Mutex<Vec<Message>>` for the life of the process. It is lost when the value is dropped, which makes it the right choice for a short-lived process or a test, and the wrong one for anything that has to survive a restart.

`impl<T: Storage + ?Sized> Storage for Arc<T>` forwards through a shared handle, so a caller can hold an `Arc<dyn Storage>` for itself, inspect it directly between runs, and still install the same handle with `Agent::memory`.

## `Agent::message` uses it, `Agent::messages` never does

`Agent::messages` and `Agent::messages_with` take the transcript as an argument. They read nothing from storage and write nothing back to it, so a conversation driven this way is never touched by whatever `Storage` an agent happens to have installed.

`Agent::message` and `Agent::message_with` are the storage-backed pair. Each call loads the conversation, appends one user turn, runs it through the loop, and appends the new turns back, storage last so a failed run never records a turn that was never sent. Because these two never see the caller's own vector and `messages`/`messages_with` never touch storage, a conversation is never held in two places at once: it lives in the caller's `Vec<Message>` on one path, or in the installed `Storage` on the other, and never both.

`Agent::message` with no storage installed returns an error before sending any request, rather than answering with no memory of what came before. An agent that silently forgot every turn would be rarely what was meant, so the failure happens up front: install a backend with `Agent::memory`, or use `Agent::messages` and hold the transcript yourself.

## A backend that needs a key takes it at construction

`Storage` has no parameter for which conversation a call is about, on any of its three methods. A backend that has to tell conversations apart, such as one keyed by user id or session id, takes that key when it is built, not on every call:

```rust
struct Redis {
    client: redis::Client,
    key: String,
}

impl Redis {
    fn new(client: redis::Client, key: impl Into<String>) -> Self {
        Self { client, key: key.into() }
    }
}
```

One `Agent` then gets one `Storage` built for the conversation it is meant to hold, which keeps the common case of one agent per conversation free of a parameter it would otherwise carry and ignore on every call.

## `StorageError`

```rust
pub type StorageError = Box<dyn std::error::Error + Send + Sync>;
```

A backend fails with a boxed standard error rather than `freyja::Error`. Every variant of `Error` carries an endpoint, because every variant describes something that went wrong talking to a provider, and a storage backend has no endpoint of its own: a database timeout or a disk error is not a provider failure. `Agent` wraps whatever a backend returns into `Error::InvalidRequest`, so callers still only ever see one error type from `Agent::message`.

## What is not built

No persistent backend ships with Freyja. `InMemoryStorage` is the only implementation in the crate, and it is gone as soon as the process is. Writing one that persists is possible today against the `Storage` trait as it stands and needs nothing else from this crate: `Message` already derives `Serialize` and `Deserialize`, so a backend only has to move bytes and implement three methods. `tests/storage.rs` is exactly that, written from outside the crate against the public API.
