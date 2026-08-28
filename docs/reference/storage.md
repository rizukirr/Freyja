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
let mut chat = agent.conversation();
```

`Agent::conversation()` starts a conversation backed by a fresh `Vec<Message>` the conversation owns. It is the easy path, and it names no storage type at all.

```rust
let mut chat = agent.conversation_in(my_backend);
```

`Agent::conversation_in(storage)` starts one over any `Storage` you already have, including a borrowed vector or a backend of your own. Both return a `Conversation`, and both are driven the same way from there.

```rust
let run = chat.send("what's the weather?").await?;
```

`Conversation::send` loads from storage, repairs the transcript, applies a window if one is set, appends the new turn, runs the tool-calling loop, then writes the turns the run produced back to storage, storage last, so a run that fails partway records nothing. `Conversation::send_with` is the same call with per-run `Context` attached, and `send` is exactly `send_with` with an empty one.

## `window` lives on the conversation, and shapes what is sent

```rust
let chat = agent.conversation().window(20);
```

`window` exists only on `Conversation<Vec<Message>>`, the kind `agent.conversation()` returns, because a backend of your own decides its own trimming inside its own `load` instead, where it can push the limit into the query rather than fetching everything first. Calling `window(groups)` does not discard anything: everything ever appended is still held, and only what one `send` puts on the wire is shaped. A group is a message, except that an assistant turn requesting tools and the results answering it are one group, so an exchange without tools costs two groups and one with tools costs three or more. `window(20)` therefore keeps roughly seven exchanges for a tool-using agent, not twenty.

## `storage()` returns the backend

```rust
let held: &Vec<Message> = chat.storage();
```

`Conversation::storage` returns a reference to the backend itself. For `agent.conversation()` that is the whole `Vec<Message>`, including everything a window would leave out of the next request, which is how a caller checks what has actually accumulated rather than what was last sent.

## The repair pass

`send` runs a crate-private repair pass on whatever `load` returns, before sending it. The pass drops any tool result whose originating call is absent, and any tool call whose result is absent, removing a message left with no content once its dropped half is gone. It also checks position, not only presence: a result is kept only when the call it answers strictly precedes it, so a backend that hands back a result ahead of its call loses both messages, the call going too because a call with no usable answer is rejected on the wire anyway. This protects a hand-written backend that trims on the wrong boundary or sorts on the wrong column: a backend cut at an arbitrary message rather than a group boundary can hand back a dangling call or a dangling result, and a backend ordering by a timestamp can hand back a pair in the wrong order, and every provider rejects both shapes with an error that says nothing about trimming or ordering. Repair runs unconditionally, once, after every `load`, before any window is applied, so a backend that knows nothing about tool pairing cannot produce a request that fails this way.

## `split` and `window_by_groups`

```rust
pub fn split(history: &[Message]) -> (Vec<&Message>, Vec<&[Message]>);
pub fn window_by_groups(history: &[Message], keep: usize) -> Vec<Message>;
```

Both are public so a backend written outside the crate can trim on safe boundaries without reimplementing turn grouping. `split` divides a transcript into its pinned turns, meaning `Role::System` and `Role::Developer`, and its groups, in order. `window_by_groups` is what `Conversation::window` calls internally: pinned turns plus the most recent `keep` groups, cloned into one new `Vec<Message>`. A backend that trims for itself calls `window_by_groups` inside its own `load` rather than reimplementing the grouping rule.

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
            let history: Vec<Message> = rows.into_iter().map(|row| row.payload).collect();
            Ok(window_by_groups(&history, 40))
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

`pool` is a clone of a connection pool the application already holds, so building a `PgStorage` per conversation is cheap. Trimming happens twice, and both halves matter. The `LIMIT 200` bounds what the database sends, so a long conversation does not stream its entire history across the wire on every turn. It is deliberately far larger than the window, because a raw row limit can cut between a tool call and the result answering it, and a transcript missing half a pair is rejected by every provider. `window_by_groups` then makes the real cut, on a boundary that never splits a group. Pick the `LIMIT` generously enough that the window is always reached before it, and treat it as a ceiling on transfer rather than as the trimming itself. Ordering is by `seq`, a column the schema defines as strictly monotonic, generated by the database on insert, never by a timestamp column: a timestamp only carries second, or at best millisecond, resolution, and two inserts issued close together can commit in either order relative to their clock reading, so a `SELECT ... ORDER BY inserted_at` can return a tool result ahead of the call it answers even though the call was appended first. A strictly monotonic sequence has no such window.

## Two limits

A caller who deliberately shares one backend between two conversations still races: `Storage`'s `&mut self` methods only stop two `send` calls on the same `Conversation` from overlapping, they do nothing about two different `Conversation` values pointed at the same underlying rows, and a backend built to allow that has to serialize it itself.

`Conversation::send` always appends a turn before running the loop, there is no way to replay a stored transcript with nothing new added. A run that stopped at `StopReason::MaxTurns` therefore cannot be resumed by calling `send` again with nothing to say, the next `send` both continues the run and adds a turn to it, so resuming a cut-off run means sending something, even if that something is a short nudge like "continue".

## `StorageError`

```rust
pub type StorageError = Box<dyn std::error::Error + Send + Sync>;
```

A backend fails with a boxed standard error rather than `freyja::Error`, because every variant of `Error` carries an endpoint, and every variant describes something that went wrong talking to a provider, which a storage backend has no part of: a database timeout or a disk error is not a provider failure. `Conversation::send` wraps whatever a backend returns into `Error::InvalidRequest`, so callers still only ever see one error type back from `send`.

## What is not built

Token budgets, summarization, retrieval with embeddings and a vector store, and any persistent backend are not implemented. `Vec<Message>` is the only implementation Freyja ships, and it is gone as soon as the value is. Writing one that persists is possible today against the `Storage` trait as it stands and needs nothing else from this crate: `Message` already derives `Serialize` and `Deserialize`, so a backend only has to move bytes and implement three methods.
