# Storage

`Storage` is where a conversation lives between calls to `Conversation::send`. One value is one conversation: a backend holds a single transcript, not a table of them, so an application juggling several conversations builds one `Storage` per conversation rather than asking a shared one to keep them apart.

## The three methods

```rust
pub trait Storage: Send {
    fn load(&mut self) -> StorageFuture<'_, Vec<Message>>;
    fn append(&mut self, messages: Vec<Message>) -> StorageFuture<'_, ()>;
    fn clear(&mut self) -> StorageFuture<'_, ()>;
}
```

The trait also has two defaulted methods, `state` and `set_state`, covered under [the state slot](#the-state-slot). A backend that does not keep state implements only these three.

Every method takes `&mut self`, not `&self`. A `Conversation` owns its backend outright, so nothing else can reach it while a call is in flight, and the borrow checker enforces that a conversation never has two `send` calls running at once. That exclusivity is also what removes the need for interior mutability: a backend holding a plain vector needs no lock and no `Mutex`, which is why `Vec<Message>` can implement this trait directly rather than wrapping itself in one.

`load` returns what to send this turn, not necessarily the whole conversation, since a backend is free to trim before handing anything back. `append` adds new turns to the end and is called with exactly the turns one run produced, never the whole transcript again. `clear` empties the conversation. All three are async and fallible, so a backend that talks to a database or a remote store fits the trait as it stands.

## `Vec<Message>` as the built-in backend

```rust
impl Storage for Vec<Message> {
    // load clones the vector, append extends it, clear empties it
}
```

A plain vector is a complete `Storage` implementation. It holds the conversation for as long as the value lives and loses it the moment the value drops, which is the right tradeoff for a short-lived process, a script, or a test, and the wrong one for anything that has to survive a restart, which is what `JsonlStorage` is for. `&mut T` and `Box<T>` also implement `Storage` by forwarding to the `T` underneath, so a caller can pass a borrowed vector they keep for themselves, or erase the backend behind `Box<dyn Storage>` when the concrete type is chosen at run time.

## Starting a conversation

```rust
let mut chat = agent.conversation(InMemoryStorage::new());
```

`Agent::conversation(storage)` is the only constructor, and it always takes the backend as an argument, so nothing chooses one for you. Pass `InMemoryStorage::new()` for a conversation held in this process, `JsonlStorage::open(path)?` for one held in a file, a borrowed vector, or a backend of your own. All return a `Conversation<S>` over whichever `Storage` you passed, and all are driven the same way from there.

```rust
let run = chat.send("what's the weather?").await?;
```

`Conversation::send` loads from storage, passes the whole transcript through the agent's memory if one is set, repairs what the memory returns, appends the new turn, runs the tool-calling loop, then writes the turns the run produced back to storage, storage last, so a run that fails partway records nothing. `Conversation::send_with` is the same call with per-run `Context` attached, and `send` is exactly `send_with` with an empty one. `send_streaming` and `send_streaming_with` are the same calls with every request streamed to a callback, and they load, repair and store exactly as `send` does.

## `Agent::memory` shapes what is sent

```rust
let agent = Agent::new(client).memory(Window::last(20));
let mut chat = agent.conversation(InMemoryStorage::new());
```

What reaches the model is the job of a `Memory`, set on the agent, and where the conversation lives is the job of a `Storage`. The two are independent, so any memory works with any backend. The backend keeps every turn. On each send the conversation loads the whole stored transcript, hands it to the memory, and sends what `Memory::recall` returns, so a window discards nothing. `InMemoryStorage::messages` still borrows the whole transcript, and `chat.storage().messages().len()` sees it even when a window is set. Without a memory the whole transcript is sent. `Agent::memory` replaces any memory set before it.

A turn group is a message, except that an assistant turn requesting tools and the results answering it are one group, so an exchange without tools costs two groups and one with tools costs three or more. `Window::last(20)` therefore keeps roughly seven exchanges for a tool-using agent, not twenty. Pinned turns, which are the `System` and `Developer` ones, are always sent. The repair pass described below still runs on whatever the memory returns, so a memory that cuts between a tool call and its result loses only that pair.

A memory keeps nothing in itself. What it needs between sends goes through `State`, the conversation's state slot, which is why one memory value serves every conversation an agent starts. A memory of your own implements `recall`, and its `compact` method defaults to refusing:

```rust
use freyja::{Memory, MemoryFuture, Message, State};

struct LastTen;

impl Memory for LastTen {
    fn recall<'a>(
        &'a self,
        messages: &'a [Message],
        _state: &'a mut State<'_>,
    ) -> MemoryFuture<'a, Vec<Message>> {
        Box::pin(async move {
            let from = messages.len().saturating_sub(10);
            Ok(messages[from..].to_vec())
        })
    }
}
```

`freyja::window_by_groups` and `freyja::window_by_tokens` stay public, and are the rules `Window` applies, for a memory that wants the same group-aware cut inside its own `recall`.

## `Window::tokens` bounds by an estimated budget instead

```rust
let agent = Agent::new(client).memory(Window::tokens(4_000, HeuristicCounter));
```

`Window::tokens(budget, counter)` is the token-aware counterpart to `Window::last`. `counter` is anything implementing `TokenCounter`, a trait with one method, `count(&self, message: &Message) -> usize`, and a blanket impl covers any `Fn(&Message) -> usize + Send` closure, so a real tokenizer needs no wrapper type. `HeuristicCounter` is the dependency-free estimate Freyja ships: roughly four bytes per token plus a small per-message constant, which undercounts code, JSON, and CJK text, so leave a margin.

The budget covers the stored transcript only. The agent's system instruction is added after the memory returns, every tool schema lives on the `Agent`, and the model's reply and the provider's own envelope both ride on top again, so none of the four count against the number you pass. Subtract all of them yourself when choosing a budget.

Two guarantees make the budget a target rather than a cap. Every `System` and `Developer` turn survives at any budget, including zero, matching `Window::last`. And the newest turn group survives even when it alone is over budget, because a request that is too large may still fail on the wire while an empty one certainly does.

## `Window::summarize` sends a summary of what the window drops

```rust
let agent = Agent::new(client).memory(
    Window::last(20).summarize(Summarizer::new(summarizer_client)),
);
```

`Window::summarize(summarizer)` returns a `Summarize`, a memory that turns what the window drops into one summary instead of losing it. The dropped turns, minus pinned ones, go to the `Summarizer`, and the result is sent as one user message headed "Summary of the earlier conversation:", after the pinned turns and before the turns still in view. It works over either kind of window. The backend still holds every raw turn.

Each summary is one extra model call, so it is made at a deeper cut than the window needs, half the window: half the groups for `Window::last`, rounded up, and half the budget for `Window::tokens`. That leaves room for the next several turns, and the summary is reused until the window needs to drop more than it covers. A small window has little room to give. `Window::last(2)` keeps one group after summarizing and one exchange adds two, so it summarizes again on nearly every turn. A new summary is built from the raw turns, never from the previous summary, so detail does not decay, at the price of a longer summarizing input as the conversation grows. The summary rides on top of a token budget rather than inside it.

If the summarizing call fails, the plain window is sent and the conversation carries on.

The summary is kept in the state slot under the key `freyja.summary`. Within one `Conversation` it is cached in the process, so a backend that keeps no state still reuses it across sends. A backend that persists state, as `JsonlStorage` does, lets a later process reuse it too. `Conversation::summary` returns the text of the summary last made, or `None` when the memory does not summarize or has not needed to yet. `clear` drops the summary along with the turns.

`Conversation::compact` summarizes now instead of when the window needs it:

```rust
chat.compact().keep(4).await?;
```

`keep(n)` leaves the newest `n` turn groups as they are and summarizes the rest, and without it the memory keeps what its own automatic summary would. The next send sends the summary in place of the summarized turns, even when the window would not have dropped them. Every turn stays in storage. Only a memory that summarizes can compact. Any other memory, or none, returns an error and makes no request.

`Summarizer` owns its own `Client` and builds its own request, so nothing from the conversation carries over: not the agent's system instruction, not its tools, not its token cap. `model`, `prompt` and `max_tokens` set its own. It sends the dropped turns as one block of text under its own instruction rather than replaying them as a conversation, which would have the model answer the last turn instead of summarizing it, and it leaves reasoning parts out, since those may be dropped but never summarized. A reply cut short, or one with no text, is an `Error::InvalidResponse`. A memory of your own can call it inside its own `recall`:

```rust
let summary = summarizer.summarize(&dropped).await?;
```

## The state slot

`Storage` has two more methods, `state(key)` and `set_state(key, value)`, which hold `serde_json::Value`s for a `Memory`. Both are defaulted. The default `state` returns `None` and the default `set_state` discards the value, which is always correct, since a memory that finds no state rebuilds it. A backend that keeps state must return the latest value for a key and must remove all of it in `clear`. `State` is the handle a memory sees: reads come from a cache in the conversation first and fall back to `Storage::state`, and writes go to both.

## `JsonlStorage` keeps the conversation in a file

```rust
let mut chat = agent.conversation(JsonlStorage::open("chat.jsonl")?);
```

`JsonlStorage::open(path)` opens the conversation stored at `path`, creating the file if it does not exist, so a later process opening the same path continues where this one stopped. `cargo run --example persist -- "a message"` shows it: run it twice and the second run remembers the first. One file is one conversation, the same way one `Storage` value is. An application with several conversations picks a path per conversation, and one that builds the path from an id a client supplied validates the id first, because it becomes part of a path. On Unix a file `open` creates is readable and writable by its owner alone (mode 0600), and a file that already exists keeps the mode it has.

The whole transcript is read at `open` and held in an `InMemoryStorage`, and every `append` is written to the file before it is held. It is a pure store: windows and summaries belong to `Agent::memory`, and the state slot is written to the file too. The file keeps every turn, whatever memory is set.

```
{"header":{"version":1}}
{"message":{"role":"user","content":[{"Text":"What is 20 + 22?"}]}}
{"message":{"role":"assistant","content":[{"Text":"42."}]}}
{"state":{"key":"freyja.summary","value":{"covers":1,"text":"- The user asked for 20 + 22"}}}
```

The file is JSON Lines. The first line carries a format version, and a file written by a version this build does not know is refused at `open` instead of being misread. Each turn is one line. A state value is a `state` line of its own, written by `set_state`, and the last line for a key is the one a later `open` restores, so a process that reopens the conversation does not pay for a summary that was already made. `clear` empties the file along with the transcript and the state.

One `append` is one write, followed by a sync. A process killed inside that write leaves a final line with no newline after it, and `open` cuts that fragment off, so a crash loses at most the tail of the last run. If the cut separates a tool call from its result, the repair pass drops the half that is left. A line that does not parse anywhere else in the file is corruption, and `open` returns an error for it. A state value that fails to write is ignored by `Summarize`, since the next process makes the summary again at the cost of one model call.

`open` returns a `StorageError`, not a `freyja::Error`, because no endpoint is involved. A `main` returning `Box<dyn std::error::Error + Send + Sync>` takes `?` on both.

Two limits are specific to a file. It is read and written with blocking `std::fs` calls, since Freyja has no runtime to hand them to, and each is one small write next to a model call that takes seconds. Nothing locks the file either, so two processes over one path race the way two conversations over one backend do.

## `storage()` returns the backend

```rust
let held = chat.storage();
```

`Conversation::storage` returns a reference to the backend itself. For `InMemoryStorage`, `messages` on that reference borrows the whole transcript, including everything a window would leave out of the next request, which is how a caller checks what has actually accumulated rather than what was last sent. It is an inherent method rather than a `Deref` to `[Message]`, because a struct holding a vector is not a smart pointer and a `Deref` would commit this type's public surface to every method a slice has.

## The repair pass

`send` runs a crate-private repair pass on whatever `load` returns, before sending it. The pass drops any tool result whose originating call is absent, and any tool call whose result is absent, removing a message left with no content once its dropped half is gone. It also checks position, not only presence: a result is kept only when the call it answers strictly precedes it, so a backend that hands back a result ahead of its call loses both messages, the call going too because a call with no usable answer is rejected on the wire anyway. This protects a hand-written backend that trims on the wrong boundary or sorts on the wrong column: a backend cut at an arbitrary message rather than a group boundary can hand back a dangling call or a dangling result, and a backend ordering by a timestamp can hand back a pair in the wrong order, and every provider rejects both shapes with an error that says nothing about trimming or ordering. Repair runs unconditionally, once, after every `load`, on whatever the agent's memory and the backend's own `load` already cut, so a backend that knows nothing about tool pairing cannot produce a request that fails this way.

## A worked external backend

This backend keys its row by a conversation id bound once at construction, so `load`, `append`, and `clear` need no parameter for which conversation they mean.

```rust
struct PgStorage {
    pool: PgPool,
    conversation_id: Uuid,
}

impl PgStorage {
    fn new(pool: PgPool, conversation_id: Uuid) -> Self {
        Self { pool, conversation_id }
    }
}

impl Storage for PgStorage {
    fn load(&mut self) -> StorageFuture<'_, Vec<Message>> {
        Box::pin(async move {
            let rows = sqlx::query_as::<_, MessageRow>(
                "SELECT payload, seq FROM (
                     SELECT payload, seq FROM turns
                     WHERE conversation_id = $1
                     ORDER BY seq DESC
                     LIMIT 200
                 ) recent
                 ORDER BY seq ASC",
            )
            .bind(self.conversation_id)
            .fetch_all(&self.pool)
            .await?;
            Ok(rows.into_iter().map(|row| row.payload).collect())
        })
    }

    fn append(&mut self, messages: Vec<Message>) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            for message in messages {
                sqlx::query("INSERT INTO turns (conversation_id, payload) VALUES ($1, $2)")
                    .bind(self.conversation_id)
                    .bind(sqlx::types::Json(message))
                    .execute(&self.pool)
                    .await?;
            }
            Ok(())
        })
    }

    fn clear(&mut self) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            sqlx::query("DELETE FROM turns WHERE conversation_id = $1")
                .bind(self.conversation_id)
                .execute(&self.pool)
                .await?;
            Ok(())
        })
    }
}
```

`pool` is a clone of a connection pool the application already holds, so building a `PgStorage` per conversation is cheap. The `LIMIT 200` bounds what the database sends, so a long conversation does not stream its entire history across the wire on every turn, and that is the only trimming this backend does. The cut it makes may land anywhere, including between a tool call and the result answering it, and a raw row limit has no way to know the difference. That is fine: the crate-private repair pass that runs inside `Conversation::send` drops both halves of a pair the cut separated, before the request is built, so a backend author never needs to know what a turn group is or reach for anything group aware to stay safe. Ordering is by `seq`, a column the schema defines as strictly monotonic, generated by the database on insert, never by a timestamp column: a timestamp only carries second, or at best millisecond, resolution, and two inserts issued close together can commit in either order relative to their clock reading, so a `SELECT ... ORDER BY inserted_at` can return a tool result ahead of the call it answers even though the call was appended first. A strictly monotonic sequence has no such window.

## Two limits

A caller who deliberately shares one backend between two conversations still races: `Storage`'s `&mut self` methods only stop two `send` calls, or a `send` and a `clear`, on the same `Conversation` from overlapping, they do nothing about two different `Conversation` values pointed at the same underlying rows, and a backend built to allow that has to serialize it itself.

`Conversation::send` always appends a turn before running the loop, there is no way to replay a stored transcript with nothing new added. A run that stopped at `StopReason::MaxTurns` therefore cannot be resumed by calling `send` again with nothing to say, the next `send` both continues the run and adds a turn to it, so resuming a cut-off run means sending something, even if that something is a short nudge like "continue".

## `StorageError`

```rust
pub type StorageError = Box<dyn std::error::Error + Send + Sync>;
```

A backend fails with a boxed standard error rather than `freyja::Error`, because every variant of `Error` carries an endpoint, and every variant describes something that went wrong talking to a provider, which a storage backend has no part of: a database timeout or a disk error is not a provider failure. `Conversation::send` wraps whatever a backend returns into `Error::InvalidRequest`, so callers still only ever see one error type back from `send`.

## What is not built

Retrieval with embeddings and a vector store, and a database backend, are not implemented. Freyja ships `InMemoryStorage` (holding a `Vec<Message>` and its state slot), `JsonlStorage` (the same, written through to a file), `impl Storage for Vec<Message>` so a transcript you hold yourself can be passed as `&mut history`, and forwarding impls for `&mut T` and `Box<T>`. Only `JsonlStorage` survives the process. A database driver brings a runtime with it and Freyja depends on none, so a database backend is written against the `Storage` trait as it stands and needs nothing else from this crate: `Message` already derives `Serialize` and `Deserialize`, so a backend only has to move bytes and implement three methods.
