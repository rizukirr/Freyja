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

`load` returns what to send this turn, not necessarily the whole conversation. A backend is free to hand back less than it holds, which is exactly what `InMemoryStorage::window` does below, so `load`'s answer and "everything this backend has ever stored" are two different questions. `append` adds new turns to the end, and is called with exactly the turns one run produced, never the whole transcript again. `clear` empties the conversation. All three are async and fallible, so a backend that talks to a database or a remote store fits the trait as it stands.

Register one with `Agent::memory`:

```rust
let agent = Agent::new(client).memory(InMemoryStorage::new());
```

## `InMemoryStorage`

```rust
impl InMemoryStorage {
    pub fn new() -> Self
    pub fn window(self, groups: usize) -> Self
    pub fn all(&self) -> Vec<Message>
}
```

`InMemoryStorage::new()` starts empty and holds the conversation in a `Mutex<Vec<Message>>` for the life of the process. It is lost when the value is dropped, which makes it the right choice for a short-lived process or a test, and the wrong one for anything that has to survive a restart.

`window(groups)` makes `load` return only the most recent `groups` turn groups, plus every pinned turn, meaning `Role::System` and `Role::Developer`. Nothing is discarded: everything ever appended is still held, and `append` and `clear` behave exactly as they would with no window set. `all()` returns everything held, ignoring the window, so it earns its place only once a window is set. Without one, `load` and `all` agree.

`impl<T: Storage + ?Sized> Storage for Arc<T>` forwards through a shared handle, so a caller can hold an `Arc<dyn Storage>` for itself, inspect it directly between runs, and still install the same handle with `Agent::memory`.

## What a turn group is

A group is a message, except that an assistant turn requesting tools and the results answering it are one group. That rule protects exactly one invariant: a tool result may only answer a call that already happened, and sending a result whose call is missing is rejected by every provider. No provider objects to a question with no answer, or an answer with no question, so nothing else is fused. Pinned turns, meaning `Role::System` and `Role::Developer`, are set aside and belong to no group, so they are never aged out by a window no matter how old they are.

This nine-message transcript is six groups.

| # | Message | Group |
|---|---|---|
| 0 | `System` | pinned, no group |
| 1 | `User "weather?"` | 1 |
| 2 | `Assistant` requesting `call_1` | 2 |
| 3 | `Tool` result for `call_1` | 2, it answers the open call |
| 4 | `Assistant "it is raining"` | 3 |
| 5 | `User "and tomorrow?"` | 4 |
| 6 | `Assistant` requesting `call_2` | 5 |
| 7 | `Tool` result for `call_2` | 5 |
| 8 | `Assistant "sunny"` | 6 |

A window of 2 keeps groups 5 and 6, so `load` returns messages 0, 6, 7 and 8. Messages 1 to 5 are not sent, and they are not removed, since `InMemoryStorage::all` still returns all nine.

Count what one exchange costs to choose a window size. Without tools an exchange is a question and an answer, so two groups. With tools it is a question, a call with its results, and an answer, so three groups, and more if the model calls tools twice before answering. So `window(20)` keeps roughly seven exchanges for a tool-using agent, not twenty, and rather fewer if any exchange calls tools more than once.

## `split` and `window_by_groups`

```rust
pub fn split(history: &[Message]) -> (Vec<&Message>, Vec<&[Message]>);
pub fn window_by_groups(history: &[Message], keep: usize) -> Vec<Message>;
```

Both are public so a backend written outside the crate can trim on safe boundaries without reimplementing turn grouping. `split` divides a transcript into its pinned turns and its groups, in order, which is the building block a backend reaches for if it wants to make its own decision about which groups to keep. `window_by_groups` is what `InMemoryStorage::window` calls internally: pinned turns plus the most recent `keep` groups, cloned into one new `Vec<Message>`. `tests/backend.rs` implements `Storage` from outside the crate and trims with `window_by_groups` rather than reimplementing it.

## The repair pass

`Agent::message` calls `load`, then runs a crate-private repair pass on the result before sending it. The repair pass drops any tool result whose originating call is absent from what `load` returned. This protects a backend nobody at Freyja has reviewed: a backend cut at an arbitrary message boundary, rather than on a group boundary the way `window_by_groups` cuts, can hand back a transcript with a dangling tool result, and that transcript looks correct until the provider rejects it with an error that mentions nothing about the cut. Repair runs unconditionally, once, after every call to `load`, so a hand-written backend that trims without knowing about tool pairing cannot produce a request that fails this way. `InMemoryStorage::window` never triggers the repair pass, because it only ever cuts on group boundaries, but a backend built any other way can, and does not need to guard against it itself.

## `Agent::message` uses it, `Agent::messages` never does

`Agent::messages` and `Agent::messages_with` take the transcript as an argument. They read nothing from storage and write nothing back to it, and they do no filtering at all, so a conversation driven this way is never touched by whatever `Storage` an agent happens to have installed. A caller who wants a window on this path calls `window_by_groups` on their own vector.

`Agent::message` and `Agent::message_with` are the storage-backed pair. Each call loads the conversation, appends one user turn, runs it through the loop, and appends the new turns back, storage last so a failed run never records a turn that was never sent. Because these two never see the caller's own vector and `messages`/`messages_with` never touch storage, a conversation is never held in two places at once: it lives in the caller's `Vec<Message>` on one path, or in the installed `Storage` on the other, and never both.

`Agent::message` with no storage installed returns an error before sending any request, rather than answering with no memory of what came before. An agent that silently forgot every turn would be rarely what was meant, so the failure happens up front: install a backend with `Agent::memory`, or use `Agent::messages` and hold the transcript yourself.

## A backend that needs a key takes it at construction

`Storage` has no parameter for which conversation a call is about, on any of its three methods. A backend that has to tell conversations apart, such as one keyed by user id or session id, takes that key when it is built, not on every call.

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

Token budgets, summarization, retrieval with embeddings and a vector store, and any persistent backend are not implemented. `InMemoryStorage` is the only implementation Freyja ships, and it is gone as soon as the process is. Writing one that persists is possible today against the `Storage` trait as it stands and needs nothing else from this crate: `Message` already derives `Serialize` and `Deserialize`, so a backend only has to move bytes and implement three methods.
