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

Every method takes `&mut self`, not `&self`. A `Conversation` owns its backend outright, so nothing else can reach it while a call is in flight, and the borrow checker enforces that a conversation never has two `send` calls running at once. That exclusivity is also what removes the need for interior mutability: a backend holding a plain vector needs no lock and no `Mutex`, which is why `Vec<Message>` can implement this trait directly rather than wrapping itself in one.

`load` returns what to send this turn, not necessarily the whole conversation, since a backend is free to trim before handing anything back. `append` adds new turns to the end and is called with exactly the turns one run produced, never the whole transcript again. `clear` empties the conversation. All three are async and fallible, so a backend that talks to a database or a remote store fits the trait as it stands.

## `Vec<Message>` as the built-in backend

```rust
impl Storage for Vec<Message> {
    // load clones the vector, append extends it, clear empties it
}
```

A plain vector is a complete `Storage` implementation. It holds the conversation for as long as the value lives and loses it the moment the value drops, which is the right tradeoff for a short-lived process, a script, or a test, and the wrong one for anything that has to survive a restart. `&mut T` and `Box<T>` also implement `Storage` by forwarding to the `T` underneath, so a caller can pass a borrowed vector they keep for themselves, or erase the backend behind `Box<dyn Storage>` when the concrete type is chosen at run time.

## Starting a conversation

```rust
let mut chat = agent.conversation(InMemoryStorage::new());
```

`Agent::conversation(storage)` is the only constructor, and it always takes the backend as an argument, so nothing chooses one for you. Pass `InMemoryStorage::new()` for a conversation held in this process, a borrowed vector, or a backend of your own. All return a `Conversation<S>` over whichever `Storage` you passed, and all are driven the same way from there.

```rust
let run = chat.send("what's the weather?").await?;
```

`Conversation::send` loads from storage, repairs the transcript, applies a window if one is set, appends the new turn, runs the tool-calling loop, then writes the turns the run produced back to storage, storage last, so a run that fails partway records nothing. `Conversation::send_with` is the same call with per-run `Context` attached, and `send` is exactly `send_with` with an empty one.

## `window` lives on the backend, and shapes what is sent

```rust
let mut chat = agent.conversation(InMemoryStorage::new().window(20));
```

Windowing is a feature of the backend, not of `Conversation`, because a backend of your own decides its own trimming inside its own `load`, where it can push the limit into the query rather than fetching everything first. `InMemoryStorage::window(groups)` is the built-in backend's version of that same choice: the window is applied inside `InMemoryStorage::load`, not on the way in, so calling `window(groups)` does not discard anything. Everything ever appended is still held, and reachable by dereferencing the backend, `InMemoryStorage` implements `Deref<Target = [Message]>`, so `chat.storage().len()` or iterating `chat.storage()` sees the whole transcript even when a window is set. Only what one `send` puts on the wire is shaped. A turn group is a message, except that an assistant turn requesting tools and the results answering it are one group, so an exchange without tools costs two groups and one with tools costs three or more. `window(20)` therefore keeps roughly seven exchanges for a tool-using agent, not twenty. A backend of your own has no obligation to be group aware at all: it can trim by count, by age, by token budget, or not trim, since the repair pass downstream cleans up whatever cut it made.

## `storage()` returns the backend

```rust
let held = chat.storage();
```

`Conversation::storage` returns a reference to the backend itself. For `InMemoryStorage` that reference derefs to `[Message]`, including everything a window would leave out of the next request, which is how a caller checks what has actually accumulated rather than what was last sent.

## The repair pass

`send` runs a crate-private repair pass on whatever `load` returns, before sending it. The pass drops any tool result whose originating call is absent, and any tool call whose result is absent, removing a message left with no content once its dropped half is gone. It also checks position, not only presence: a result is kept only when the call it answers strictly precedes it, so a backend that hands back a result ahead of its call loses both messages, the call going too because a call with no usable answer is rejected on the wire anyway. This protects a hand-written backend that trims on the wrong boundary or sorts on the wrong column: a backend cut at an arbitrary message rather than a group boundary can hand back a dangling call or a dangling result, and a backend ordering by a timestamp can hand back a pair in the wrong order, and every provider rejects both shapes with an error that says nothing about trimming or ordering. Repair runs unconditionally, once, after every `load`, before any window is applied, so a backend that knows nothing about tool pairing cannot produce a request that fails this way.

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

A caller who deliberately shares one backend between two conversations still races: `Storage`'s `&mut self` methods only stop two `send` calls on the same `Conversation` from overlapping, they do nothing about two different `Conversation` values pointed at the same underlying rows, and a backend built to allow that has to serialize it itself.

`Conversation::send` always appends a turn before running the loop, there is no way to replay a stored transcript with nothing new added. A run that stopped at `StopReason::MaxTurns` therefore cannot be resumed by calling `send` again with nothing to say, the next `send` both continues the run and adds a turn to it, so resuming a cut-off run means sending something, even if that something is a short nudge like "continue".

## `StorageError`

```rust
pub type StorageError = Box<dyn std::error::Error + Send + Sync>;
```

A backend fails with a boxed standard error rather than `freyja::Error`, because every variant of `Error` carries an endpoint, and every variant describes something that went wrong talking to a provider, which a storage backend has no part of: a database timeout or a disk error is not a provider failure. `Conversation::send` wraps whatever a backend returns into `Error::InvalidRequest`, so callers still only ever see one error type back from `send`.

## What is not built

Token budgets, summarization, retrieval with embeddings and a vector store, and any persistent backend are not implemented. Freyja ships `InMemoryStorage` (holding a `Vec<Message>` and an optional window), `impl Storage for Vec<Message>` so a transcript you hold yourself can be passed as `&mut history`, and forwarding impls for `&mut T` and `Box<T>`. None of them survive the process. Writing one that persists is possible today against the `Storage` trait as it stands and needs nothing else from this crate: `Message` already derives `Serialize` and `Deserialize`, so a backend only has to move bytes and implement three methods.
