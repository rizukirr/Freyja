# store-memory: Implementation Plan

**Spec:** docs/specs/2026-11-27-store-memory-design.md
**Goal:** Move the window and summary rules out of the storage types into a `Memory` trait set on `Agent`, give `Storage` a state slot, and add `compact`, a conformance check and a contract page.
**Architecture:** `Storage` keeps its shape and gains two defaulted state methods. A new `src/memory.rs` holds the `Memory` trait, the `State` handle, `Window` and `Summarize`, with the summary logic moved out of `InMemoryStorage::load`. `Conversation` runs `load`, then the agent's memory, then the existing loop, and keeps an in-process state cache.

## Global constraints

- No new dependency.
- Rust 1.88, edition 2024, the MSRV CI checks.
- Every trait keeps boxed futures so it stays usable as `dyn`.
- Pinned turns, group integrity, the repair pass and the refusal of an orphaned tool result behave exactly as today.
- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-features` and `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features` all pass.
- Public API changes are breaking and recorded in `CHANGELOG.md` under Unreleased.
- `CHECK` below means: `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-features && RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features`

### Task 1: State slot on `Storage` and the conformance check → verify: `CHECK` exits 0, and `cargo test --test conformance` exits 0

**Blocked by:** none

**Files:**
- Modify: `src/storage.rs`
- Modify: `src/builtin/memory.rs`
- Create: `src/testing.rs`
- Modify: `src/lib.rs`
- Create: `tests/conformance.rs`

- [x] Step 1: In `src/storage.rs`, add `use serde_json::Value;` and two defaulted methods at the end of `trait Storage`. Extend the `clear` doc with one sentence: "It must also remove everything stored with [`Storage::set_state`]."

```rust
    /// The value stored under `key` in this conversation's state slot, if
    /// this backend keeps state.
    ///
    /// State is what a `Memory` keeps between sends, such as a cached
    /// summary, so a backend that persists it saves a model call after a
    /// restart. The default keeps nothing and returns `None`, which is always
    /// correct: a memory that finds no state rebuilds it.
    fn state<'a>(&'a mut self, key: &'a str) -> StorageFuture<'a, Option<Value>> {
        let _ = key;
        Box::pin(async { Ok(None) })
    }

    /// Stores `value` under `key`, replacing what was there.
    ///
    /// The default discards it. A backend that keeps state returns the
    /// latest value from [`Storage::state`] and removes all of it on
    /// [`Storage::clear`].
    fn set_state<'a>(&'a mut self, key: &'a str, value: Value) -> StorageFuture<'a, ()> {
        let _ = (key, value);
        Box::pin(async { Ok(()) })
    }
```

- [x] Step 2: In both forwarding impls (`&mut T` and `Box<T>`) in `src/storage.rs`, forward the two methods:

```rust
    fn state<'a>(&'a mut self, key: &'a str) -> StorageFuture<'a, Option<Value>> {
        (**self).state(key)
    }
    fn set_state<'a>(&'a mut self, key: &'a str, value: Value) -> StorageFuture<'a, ()> {
        (**self).set_state(key, value)
    }
```

- [x] Step 3: In `src/builtin/memory.rs`, add a field `state: std::collections::HashMap<String, serde_json::Value>` to `InMemoryStorage` (it derives `Default`, so no constructor changes). In `impl Storage for InMemoryStorage`, add `self.state.clear();` inside `clear`, and add:

```rust
    fn state<'a>(&'a mut self, key: &'a str) -> StorageFuture<'a, Option<serde_json::Value>> {
        Box::pin(async move { Ok(self.state.get(key).cloned()) })
    }

    fn set_state<'a>(
        &'a mut self,
        key: &'a str,
        value: serde_json::Value,
    ) -> StorageFuture<'a, ()> {
        Box::pin(async move {
            self.state.insert(key.to_string(), value);
            Ok(())
        })
    }
```

- [x] Step 4: Create `src/testing.rs`:

```rust
//! Checks a crate outside Freyja runs against its own implementations.

use crate::{Message, Role, Storage};
use serde_json::json;

/// Checks that a [`Storage`] honours the rules the trait documents.
///
/// `make` returns a new, empty backend each time it is called. Run it from
/// the backend crate's own tests, under any async runtime:
///
/// ```
/// # async fn run() {
/// freyja::testing::storage_conformance(freyja::InMemoryStorage::new).await;
/// # }
/// ```
///
/// # Panics
///
/// On the first broken rule, with a message naming it.
pub async fn storage_conformance<S: Storage>(mut make: impl FnMut() -> S) {
    let first = || {
        vec![
            Message::text(Role::User, "one"),
            Message::text(Role::Assistant, "two"),
        ]
    };
    let second = || vec![Message::text(Role::User, "three")];

    let mut storage = make();
    assert!(
        storage.load().await.expect("load a new conversation").is_empty(),
        "a new backend must load an empty conversation"
    );

    storage.append(first()).await.expect("append");
    storage.append(second()).await.expect("append");
    let mut expected = first();
    expected.extend(second());
    assert_eq!(
        storage.load().await.expect("load"),
        expected,
        "load must return every appended message, oldest first"
    );

    storage.clear().await.expect("clear a conversation that holds messages");
    assert!(
        storage.load().await.expect("load after clear").is_empty(),
        "clear must empty the conversation"
    );
    storage
        .clear()
        .await
        .expect("clear must succeed on an empty conversation");

    let mut storage = make();
    storage
        .set_state("conformance", json!(1))
        .await
        .expect("set state");
    if storage.state("conformance").await.expect("read state").is_some() {
        storage
            .set_state("conformance", json!(2))
            .await
            .expect("set state");
        assert_eq!(
            storage.state("conformance").await.expect("read state"),
            Some(json!(2)),
            "state must return the latest value set"
        );
        assert_eq!(
            storage.state("unset").await.expect("read state"),
            None,
            "a key never set must read as None"
        );
        storage.clear().await.expect("clear");
        assert_eq!(
            storage.state("conformance").await.expect("read state"),
            None,
            "clear must remove state"
        );
    }
}
```

- [x] Step 5: In `src/lib.rs`, add `pub mod testing;` after `mod summarize;`.

- [x] Step 6: Create `tests/conformance.rs`:

```rust
//! Every backend Freyja ships, and one written outside it, against the
//! published conformance check.

use freyja::testing::storage_conformance;
use freyja::{InMemoryStorage, JsonlStorage, Message, Storage, StorageFuture};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The simplest backend a crate outside Freyja could write.
#[derive(Default)]
struct Outside(Vec<Message>);

impl Storage for Outside {
    fn load(&mut self) -> StorageFuture<'_, Vec<Message>> {
        Box::pin(async move { Ok(self.0.clone()) })
    }
    fn append(&mut self, messages: Vec<Message>) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            self.0.extend(messages);
            Ok(())
        })
    }
    fn clear(&mut self) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            self.0.clear();
            Ok(())
        })
    }
}

/// Hands messages back newest first, which the check must catch.
#[derive(Default)]
struct Reversed(Vec<Message>);

impl Storage for Reversed {
    fn load(&mut self) -> StorageFuture<'_, Vec<Message>> {
        Box::pin(async move { Ok(self.0.iter().rev().cloned().collect()) })
    }
    fn append(&mut self, messages: Vec<Message>) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            self.0.extend(messages);
            Ok(())
        })
    }
    fn clear(&mut self) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            self.0.clear();
            Ok(())
        })
    }
}

#[tokio::test]
async fn in_memory_storage_conforms() {
    storage_conformance(InMemoryStorage::new).await;
}

#[tokio::test]
async fn a_plain_vector_conforms() {
    storage_conformance(Vec::<Message>::new).await;
}

#[tokio::test]
async fn a_backend_written_outside_freyja_conforms() {
    storage_conformance(Outside::default).await;
}

#[tokio::test]
async fn jsonl_storage_conforms() {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let make = || {
        let path = std::env::temp_dir().join(format!(
            "freyja-conformance-{}-{}.jsonl",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_file(&path);
        JsonlStorage::open(path).expect("open")
    };
    storage_conformance(make).await;
}

#[tokio::test]
#[should_panic(expected = "oldest first")]
async fn a_backend_that_reverses_order_fails() {
    storage_conformance(Reversed::default).await;
}
```

- [x] Step 7: Run `CHECK`
- [x] Step 8: Run `cargo test --test conformance`
- [x] Step 9: Commit: `Add a state slot to Storage and a published conformance check`

### Task 2: The `Memory` trait, `Window`, `Summarize`, and `Agent::memory` → verify: `CHECK` exits 0, and `cargo test --test memory` exits 0

**Blocked by:** Task 1

**Files:**
- Create: `src/memory.rs`
- Modify: `src/lib.rs`
- Modify: `src/storage.rs`
- Modify: `src/helper/mod.rs`
- Modify: `src/agent.rs`
- Modify: `src/conversation.rs`
- Modify: `tests/memory.rs`

- [x] Step 1: Create `src/memory.rs`:

```rust
//! What the model sees of a conversation, decided apart from where the
//! conversation is kept.

use crate::helper::{cut_by_groups, cut_by_tokens, reassemble, split};
use crate::{
    Message, Role, Storage, StorageError, Summarizer, TokenCounter, window_by_groups,
    window_by_tokens,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

/// What a [`Memory`] failed with.
pub type MemoryError = Box<dyn std::error::Error + Send + Sync>;

/// The future returned by every [`Memory`] method.
pub type MemoryFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, MemoryError>> + Send + 'a>>;

/// The state key [`Summarize`] keeps its summary under.
pub(crate) const SUMMARY_KEY: &str = "freyja.summary";

/// Heads the summary message, so the model reads it as an account of what
/// came before rather than as something the user just said.
const SUMMARY_PREFIX: &str = "Summary of the earlier conversation:\n\n";

/// Decides what the model sees of a conversation on each send.
///
/// Set on an agent with [`crate::Agent::memory`]. It works with any
/// [`Storage`]: the backend keeps every turn, and a memory chooses which of
/// them, or what in their place, reaches the model. What a memory keeps
/// between sends goes in the conversation's state slot through [`State`],
/// never in the memory value, so one value serves every conversation.
///
/// Boxed rather than `async fn` in the trait, for the reason
/// [`crate::ToolFuture`] gives.
pub trait Memory: Send + Sync {
    /// What to send of `messages`, the whole stored conversation, oldest
    /// first. The caller's new turn is added after this returns.
    fn recall<'a>(
        &'a self,
        messages: &'a [Message],
        state: &'a mut State<'_>,
    ) -> MemoryFuture<'a, Vec<Message>>;

    /// Summarizes the older part of `messages` now, keeping the newest `keep`
    /// turn groups as they are, or this memory's own choice when `keep` is
    /// `None`. The next [`Memory::recall`] sends the summary in their place.
    ///
    /// The default refuses: only a memory that summarizes can compact.
    fn compact<'a>(
        &'a self,
        messages: &'a [Message],
        state: &'a mut State<'_>,
        keep: Option<usize>,
    ) -> MemoryFuture<'a, ()> {
        let _ = (messages, state, keep);
        Box::pin(async {
            Err("this memory does not summarize, so it cannot compact; \
                 build one with Window::summarize"
                .into())
        })
    }
}

/// A conversation's state slot, as a [`Memory`] sees it.
///
/// Reads come from a cache in this process first and fall back to
/// [`Storage::state`]. Writes go to both. So a backend that keeps no state
/// still lets one summary serve many sends within one
/// [`crate::Conversation`], and one that keeps it lets the summary outlive
/// the process.
pub struct State<'a> {
    storage: &'a mut dyn Storage,
    cache: Cache<'a>,
}

enum Cache<'a> {
    Owned(HashMap<String, Value>),
    Borrowed(&'a mut HashMap<String, Value>),
}

impl<'a> State<'a> {
    /// A handle over `storage` with an empty cache of its own.
    ///
    /// What a memory's own tests call. A [`crate::Conversation`] passes one
    /// whose cache lives as long as the conversation does.
    pub fn new(storage: &'a mut dyn Storage) -> Self {
        Self {
            storage,
            cache: Cache::Owned(HashMap::new()),
        }
    }

    pub(crate) fn cached(
        storage: &'a mut dyn Storage,
        cache: &'a mut HashMap<String, Value>,
    ) -> Self {
        Self {
            storage,
            cache: Cache::Borrowed(cache),
        }
    }

    fn map(&mut self) -> &mut HashMap<String, Value> {
        match &mut self.cache {
            Cache::Owned(map) => map,
            Cache::Borrowed(map) => map,
        }
    }

    /// The value under `key`, from the cache or else from the backend.
    pub async fn get(&mut self, key: &str) -> Result<Option<Value>, StorageError> {
        if let Some(value) = self.map().get(key) {
            return Ok(Some(value.clone()));
        }
        let stored = self.storage.state(key).await?;
        if let Some(value) = &stored {
            self.map().insert(key.to_string(), value.clone());
        }
        Ok(stored)
    }

    /// Stores `value` under `key`, in the backend and in the cache.
    pub async fn set(&mut self, key: &str, value: Value) -> Result<(), StorageError> {
        self.storage.set_state(key, value.clone()).await?;
        self.map().insert(key.to_string(), value);
        Ok(())
    }
}

/// Sends only the most recent part of a conversation, plus pinned turns.
///
/// A group is a message, except that an assistant turn requesting tools and
/// the results answering it are one group, so an exchange costs two groups
/// without tools and three with them. Pinned turns (`System` and
/// `Developer`) are always sent.
pub struct Window {
    rule: Rule,
}

enum Rule {
    Groups(usize),
    Tokens {
        budget: usize,
        counter: Box<dyn TokenCounter + Sync>,
    },
}

/// Hand-written because a boxed counter is not `Debug`. The budget is the
/// part worth printing.
impl std::fmt::Debug for Window {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.rule {
            Rule::Groups(groups) => formatter.debug_tuple("Window::groups").field(groups).finish(),
            Rule::Tokens { budget, .. } => formatter
                .debug_struct("Window::tokens")
                .field("budget", budget)
                .finish_non_exhaustive(),
        }
    }
}

impl Window {
    /// The most recent `keep` turn groups. The rule is
    /// [`crate::window_by_groups`].
    pub fn groups(keep: usize) -> Self {
        Self {
            rule: Rule::Groups(keep),
        }
    }

    /// The most recent turn groups fitting an estimated token `budget`. The
    /// rule is [`crate::window_by_tokens`].
    ///
    /// The budget covers the stored conversation only. The agent's system
    /// instruction and every tool schema are added after it, and the reply
    /// comes on top, so subtract all three from the model's context limit and
    /// leave a margin, since [`crate::HeuristicCounter`] undercounts code and
    /// JSON.
    pub fn tokens(budget: usize, counter: impl TokenCounter + Sync + 'static) -> Self {
        Self {
            rule: Rule::Tokens {
                budget,
                counter: Box::new(counter),
            },
        }
    }

    /// Summarizes what this window drops instead of losing it.
    pub fn summarize(self, summarizer: Summarizer) -> Summarize {
        Summarize {
            window: self,
            summarizer,
        }
    }

    fn apply(&self, messages: &[Message]) -> Vec<Message> {
        match &self.rule {
            Rule::Groups(keep) => window_by_groups(messages, *keep),
            Rule::Tokens { budget, counter } => {
                window_by_tokens(messages, *budget, counter.as_ref())
            }
        }
    }

    /// How many leading groups the window must drop, and the deeper cut at
    /// half the window a new summary is made at, so one summary serves
    /// several sends.
    fn cuts(&self, pinned: &[&Message], groups: &[&[Message]]) -> (usize, usize) {
        match &self.rule {
            Rule::Groups(keep) => (
                cut_by_groups(groups, *keep),
                cut_by_groups(groups, keep.div_ceil(2)),
            ),
            Rule::Tokens { budget, counter } => (
                cut_by_tokens(pinned, groups, *budget, counter.as_ref()),
                cut_by_tokens(pinned, groups, budget / 2, counter.as_ref()),
            ),
        }
    }
}

impl Memory for Window {
    fn recall<'a>(
        &'a self,
        messages: &'a [Message],
        _state: &'a mut State<'_>,
    ) -> MemoryFuture<'a, Vec<Message>> {
        Box::pin(async move { Ok(self.apply(messages)) })
    }
}

/// A [`Window`] that sends a summary of what it drops.
///
/// Built by [`Window::summarize`]. Each summary is one extra model call, so
/// it is made at half the window, which leaves room for several sends, and
/// reused until the window needs to drop more than it covers. A new summary
/// is built from the raw turns, never from the previous summary. If the
/// summarizing call fails, the plain window is sent and the conversation
/// carries on.
///
/// The summary goes in as one user message after the pinned turns and before
/// the turns still in view. It is kept in the conversation's state slot, so a
/// backend that persists state reuses it after a restart.
#[derive(Debug)]
pub struct Summarize {
    window: Window,
    summarizer: Summarizer,
}

/// What is kept under [`SUMMARY_KEY`]: the summary, and how many leading
/// groups it stands for. Turns are only appended, so the same count means
/// the same turns.
#[derive(Serialize, Deserialize)]
struct Cached {
    covers: usize,
    text: String,
}

/// The non-pinned messages in the first `covers` groups. Pinned turns are
/// kept, not summarized.
fn older(groups: &[&[Message]], covers: usize) -> Vec<Message> {
    groups[..covers]
        .iter()
        .flat_map(|group| group.iter())
        .filter(|message| !matches!(message.role, Role::System | Role::Developer))
        .cloned()
        .collect()
}

impl Summarize {
    /// The stored summary, if there is one that still fits the conversation.
    /// One claiming more groups than are held came from somewhere else, such
    /// as a stale file, and is ignored.
    async fn stored(&self, state: &mut State<'_>, groups: usize) -> Option<Cached> {
        let value = state.get(SUMMARY_KEY).await.ok().flatten()?;
        serde_json::from_value::<Cached>(value)
            .ok()
            .filter(|cached| cached.covers <= groups)
    }
}

impl Memory for Summarize {
    fn recall<'a>(
        &'a self,
        messages: &'a [Message],
        state: &'a mut State<'_>,
    ) -> MemoryFuture<'a, Vec<Message>> {
        Box::pin(async move {
            let (pinned, groups) = split(messages);
            let (needed, deeper) = self.window.cuts(&pinned, &groups);

            let cached = self
                .stored(state, groups.len())
                .await
                .filter(|cached| cached.covers >= needed);
            let (from, text) = match cached {
                Some(cached) => (cached.covers, cached.text),
                None if needed == 0 => return Ok(self.window.apply(messages)),
                None => match self.summarizer.summarize(&older(&groups, deeper)).await {
                    Ok(text) => {
                        let record = Cached {
                            covers: deeper,
                            text: text.clone(),
                        };
                        // A summary that fails to store is made again on a
                        // later send, which costs a call and loses nothing.
                        if let Ok(value) = serde_json::to_value(&record) {
                            let _ = state.set(SUMMARY_KEY, value).await;
                        }
                        (deeper, text)
                    }
                    // A summary improves on the window and is never a
                    // condition for it.
                    Err(_) => return Ok(self.window.apply(messages)),
                },
            };

            let kept: usize = groups[from..].iter().map(|group| group.len()).sum();
            let mut sent = reassemble(pinned, &groups, from);
            sent.insert(
                sent.len() - kept,
                Message::text(Role::User, format!("{SUMMARY_PREFIX}{text}")),
            );
            Ok(sent)
        })
    }

    fn compact<'a>(
        &'a self,
        messages: &'a [Message],
        state: &'a mut State<'_>,
        keep: Option<usize>,
    ) -> MemoryFuture<'a, ()> {
        Box::pin(async move {
            let (pinned, groups) = split(messages);
            let covers = match keep {
                Some(keep) => groups.len().saturating_sub(keep),
                None => self.window.cuts(&pinned, &groups).1,
            };
            if covers == 0 {
                return Ok(());
            }
            let text = self.summarizer.summarize(&older(&groups, covers)).await?;
            state
                .set(SUMMARY_KEY, serde_json::to_value(Cached { covers, text })?)
                .await?;
            Ok(())
        })
    }
}
```

- [x] Step 2: `src/helper/mod.rs` already exports `split`, `reassemble`, `cut_by_groups` and `cut_by_tokens` as `pub(crate)`. Update the comment above that line so it names `crate::memory` as the caller, since `crate::builtin::memory` stops calling them in Task 3.

- [x] Step 3: In `src/lib.rs`, add `mod memory;` after `mod helper;` and `pub use memory::{Memory, MemoryError, MemoryFuture, State, Summarize, Window};` after the `pub use helper::...` line. Then, in `src/storage.rs`, turn the plain `Memory` in the `Storage::state` doc into the link [`crate::Memory`], now that it resolves.

- [x] Step 4: In `src/agent.rs`, add a field `memory: Option<Arc<dyn crate::Memory>>` to `Agent`, set to `None` in `Agent::new`, and add after `max_turns`:

```rust
    /// Decides what the model sees of each conversation.
    ///
    /// Unset, every stored turn is sent. Pass a [`crate::Window`] to bound
    /// it, or [`crate::Window::summarize`] to send a summary of what the
    /// window drops. Any [`crate::Memory`] works with any
    /// [`crate::Storage`]. The last memory set is the one used.
    pub fn memory(mut self, memory: impl crate::Memory + 'static) -> Self {
        self.memory = Some(Arc::new(memory));
        self
    }

    /// The memory this agent's conversations use, if one is set.
    pub(crate) fn memory_handle(&self) -> Option<Arc<dyn crate::Memory>> {
        self.memory.clone()
    }
```

- [x] Step 5: In `src/conversation.rs`, add a field `cache: std::collections::HashMap<String, serde_json::Value>` to `Conversation`, initialised empty in `Conversation::new`. In `clear`, after the backend's clear succeeds, add `self.cache.clear();`. In `run`, replace the `let mut history = self.storage.load()...?;` statement with:

```rust
        let stored = self
            .storage
            .load()
            .await
            .map_err(|error| self.agent.storage_error(format!("storage load: {error}")))?;

        let mut history = match self.agent.memory_handle() {
            Some(memory) => {
                let mut state = crate::State::cached(&mut self.storage, &mut self.cache);
                memory
                    .recall(&stored, &mut state)
                    .await
                    .map_err(|error| self.agent.storage_error(format!("memory: {error}")))?
            }
            None => stored,
        };
```

- [x] Step 6: Append to `tests/memory.rs` (read its existing `use` lines and helpers first, and add any import below that it lacks):

```rust
/// A backend written outside Freyja, with no window of its own.
#[derive(Default)]
struct Plain(Vec<freyja::Message>);

impl freyja::Storage for Plain {
    fn load(&mut self) -> freyja::StorageFuture<'_, Vec<freyja::Message>> {
        Box::pin(async move { Ok(self.0.clone()) })
    }
    fn append(&mut self, messages: Vec<freyja::Message>) -> freyja::StorageFuture<'_, ()> {
        Box::pin(async move {
            self.0.extend(messages);
            Ok(())
        })
    }
    fn clear(&mut self) -> freyja::StorageFuture<'_, ()> {
        Box::pin(async move {
            self.0.clear();
            Ok(())
        })
    }
}

fn sent_messages(raw: &str) -> Vec<serde_json::Value> {
    let body = &raw[raw.rfind("\r\n").expect("a header line") + 2..];
    let sent: serde_json::Value = serde_json::from_str(body).expect("json body");
    sent["messages"].as_array().expect("messages array").clone()
}

#[tokio::test]
async fn a_memory_bounds_a_backend_written_outside_freyja() {
    let (base, requests) = common::serve(&[
        common::ok_response(),
        common::ok_response(),
        common::ok_response(),
    ]);
    let config = freyja::EndpointConfig::new(freyja::Dialect::OpenAiChat, "local", base)
        .default_model("test-model");
    let agent = freyja::Agent::new(freyja::Client::new(config, "sk-test"))
        .memory(freyja::Window::groups(2));
    let mut chat = agent.conversation(Plain::default());

    chat.send("first").await.expect("run");
    chat.send("second").await.expect("run");
    chat.send("third").await.expect("run");

    requests.recv().expect("first request");
    requests.recv().expect("second request");
    let last = sent_messages(&requests.recv().expect("third request"));

    // Two stored groups survive the window, then the new turn.
    assert_eq!(last.len(), 3, "{last:?}");
    assert_eq!(last[0]["content"], "second");
    assert_eq!(last[2]["content"], "third");
    // The backend still holds every turn.
    assert_eq!(chat.storage().0.len(), 6);
}

#[tokio::test]
async fn one_summary_serves_two_sends_over_a_backend_with_no_state() {
    let summary = {
        let body = r#"{"id":"x","model":"test-model","choices":[{"message":{"role":"assistant","content":"- SUMMARY"},"finish_reason":"stop"}]}"#;
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        Box::leak(head.into_boxed_str()) as &'static str
    };
    // Exactly one summarizing reply. A second summarizing call would take the
    // answer meant for the last send, and that send would then find no
    // server and fail.
    let (base, requests) = common::serve(&[
        common::ok_response(),
        common::ok_response(),
        common::ok_response(),
        summary,
        common::ok_response(),
        common::ok_response(),
    ]);
    let config = freyja::EndpointConfig::new(freyja::Dialect::OpenAiChat, "local", base)
        .default_model("test-model");
    let client = freyja::Client::new(config, "sk-test");
    let agent = freyja::Agent::new(client.clone())
        .memory(freyja::Window::groups(4).summarize(freyja::Summarizer::new(client)));

    let mut messages: Vec<freyja::Message> = Vec::new();
    let mut chat = agent.conversation(&mut messages);
    for turn in ["one", "two", "three", "four", "five"] {
        chat.send(turn).await.expect("run");
    }

    // The server records a request after answering it, so wait for each.
    let raw: Vec<String> = (0..6).map(|_| requests.recv().expect("a request")).collect();
    let last = &raw[5];
    assert!(last.contains("- SUMMARY"), "{last}");
}
```

- [x] Step 7: Run `CHECK`
- [x] Step 8: Run `cargo test --test memory`
- [x] Step 9: Commit: `Add the Memory trait, Window and Summarize, set on Agent`

### Task 3: Storage types become pure stores, and tests move to `Agent::memory` → verify: `CHECK` exits 0, and `grep -rn "\.window(\|window_by_tokens(\|\.summarize(" src tests examples` finds no call on `InMemoryStorage` or `JsonlStorage`

**Blocked by:** Task 2

**Files:**
- Modify: `src/builtin/memory.rs`
- Modify: `src/builtin/jsonl.rs`
- Modify: `tests/storage.rs`
- Modify: `tests/summarize.rs`
- Modify: `tests/jsonl.rs`
- Modify: `examples/memory.rs`
- Modify: `examples/persist.rs`
- Modify: `tests/agent.rs`
- Modify: `src/conversation.rs`
- Modify: any `src/` doc comment that links a removed item, found by the `cargo doc` run in `CHECK`

- [x] Step 1: In `src/builtin/memory.rs`, delete the `Window` enum and its `Debug` impl, the `Summary` struct, `SUMMARY_PREFIX`, the fields `window`, `summarizer` and `summary`, the methods `window`, `window_by_tokens`, `summarize`, `summary`, `cached` and `windowed`, and the imports they alone used. `InMemoryStorage` keeps `messages` and `state`. Its `load` becomes `Box::pin(async move { Ok(self.messages.clone()) })`, and its `clear` clears both fields. Replace `restore` with:

```rust
    /// A conversation read back from somewhere that outlived the process.
    pub(crate) fn restore(
        messages: Vec<Message>,
        state: std::collections::HashMap<String, serde_json::Value>,
    ) -> Self {
        Self { messages, state }
    }
```

Rewrite the type's doc comment to describe a pure store: every turn held in this process, lost when dropped, state kept for a memory, and a pointer to `crate::Agent::memory` for windows and summaries.

- [x] Step 2: In `src/builtin/jsonl.rs`, replace the record variant `Summary { covers: usize, text: String }` with `State { key: String, value: serde_json::Value }` and its doc line with "A state slot value. The last one per key wins." In `open`, collect `Record::State { key, value }` into a `HashMap` (insert, so the last wins) and pass it to `InMemoryStorage::restore(messages, state)`. Delete `window`, `window_by_tokens`, `summarize` and `summary`. In `impl Storage for JsonlStorage`, `load` becomes `self.inner.load()`, and add:

```rust
    fn state<'a>(&'a mut self, key: &'a str) -> StorageFuture<'a, Option<serde_json::Value>> {
        self.inner.state(key)
    }

    fn set_state<'a>(
        &'a mut self,
        key: &'a str,
        value: serde_json::Value,
    ) -> StorageFuture<'a, ()> {
        Box::pin(async move {
            // The file first, so memory never holds state the disk lacks.
            write(
                &mut self.file,
                &[Record::State {
                    key: key.to_string(),
                    value: value.clone(),
                }],
            )?;
            self.inner.set_state(key, value).await
        })
    }
```

Remove `Summarizer` and `TokenCounter` from its imports if unused, and update the type's doc example to `agent.memory(freyja::Window::groups(20)).conversation(JsonlStorage::open("chat.jsonl")?)`.

- [x] Step 3: Migrate `tests/storage.rs`:
  - `window_shapes_what_is_sent_while_the_backend_keeps_everything` and `a_window_survives_a_clear`: build the agent as `agent_for(base).memory(Window::groups(1))` and the conversation over `InMemoryStorage::new()`.
  - `a_token_window_shapes_what_is_sent_while_the_backend_keeps_everything`: `agent_for(base).memory(Window::tokens(budget, HeuristicCounter))` over `InMemoryStorage::new()`.
  - Replace `the_last_window_builder_called_replaces_the_earlier_one` with this test, which asserts the same rule on the new builder:

```rust
#[tokio::test]
async fn the_last_memory_set_replaces_the_earlier_one() {
    let (base, requests) = serve(&[ok_response(), ok_response(), ok_response()]);
    // A window of one group, then one large enough for everything: if the
    // first were kept, the last request would carry one stored group.
    let agent = agent_for(base)
        .memory(Window::groups(1))
        .memory(Window::groups(100));
    let mut chat = agent.conversation(InMemoryStorage::new());

    chat.send("first").await.expect("run");
    chat.send("second").await.expect("run");
    chat.send("third").await.expect("run");

    requests.recv().expect("first request");
    requests.recv().expect("second request");
    let last = requests.recv().expect("third request");
    let split_at = last.rfind("\r\n").expect("a header line") + 2;
    let sent: serde_json::Value = serde_json::from_str(&last[split_at..]).expect("json body");
    let sent_len = sent["messages"].as_array().expect("messages array").len();
    assert_eq!(sent_len, chat.storage().messages().len() - 1);
}
```

  Read `agent_for` first: if it does not return an `Agent` that `.memory` can chain onto, call `.memory` on its result the same way. Add `Window` to the `use freyja::{...}` list.

- [x] Step 4: Migrate `tests/summarize.rs`. Add `State`, `Window`, `Memory` to its imports.
  - `dropped_turns_reach_the_model_as_a_summary`: the agent is `Agent::new(client.clone()).memory(Window::groups(1).summarize(Summarizer::new(client)))` (build `Summarizer` before moving `client`, or clone as needed), the conversation is over `InMemoryStorage::new()`, and `chat.storage().summary()` becomes `chat.summary().await.expect("summary")` compared with `Some("- The user is planning three days in Bergen".to_string())`. Keep every other assertion.
  - The three tests that call `storage.load()` directly (`a_summary_is_reused_while_the_cut_stays_and_cleared_with_the_conversation`, `a_failed_summary_falls_back_to_the_plain_window`, `one_summary_serves_several_turns`): build `let memory = Window::groups(N).summarize(summarizer_for(base));` with the window size each test used, keep `let mut storage = InMemoryStorage::new();` and its `append` calls, and replace each `storage.load().await.expect(...)` with:

```rust
        let held = storage.load().await.expect("load");
        let sent = memory
            .recall(&held, &mut State::new(&mut storage))
            .await
            .expect("recall");
```

  Replace each `storage.summary()` assertion: an expected `Some("- SUMMARY")` is already shown by the summary message in `sent`, so drop that line only where the test already asserts the summary text in `sent`, and otherwise assert `sent` contains the summary message. For the clear check in `a_summary_is_reused_while_the_cut_stays_and_cleared_with_the_conversation`, after `storage.clear()`, append the same two messages again and assert that a recall returns the plain window (`sent == vec![Message::text(Role::Assistant, "two")]`): the server holds one reply, so a summary that survived `clear` would be reused, and one that did not must be made again, fail, and fall back.

- [x] Step 5: Migrate `tests/jsonl.rs`. Add `State`, `Window`, `Memory` to its imports.
  - `a_reopened_conversation_reuses_the_stored_summary`: open plain `JsonlStorage`, use `let memory = Window::groups(1).summarize(summarizer_for(base));` for both opens (one summarizer is enough, since only the first recall reaches the server), and replace each `storage.load()` with the recall form from Task 3 Step 4. Replace `storage.summary()` checks by asserting the first recall's `sent[0]` carries `- SUMMARY`. After `clear`, reopen, append the two messages, and assert a recall returns the plain window, as in Step 4.
  - `a_summary_covering_more_than_the_file_holds_is_dropped`: write the file as `{"header":{"version":1}}\n{"state":{"key":"freyja.summary","value":{"covers":9,"text":"stale"}}}\n`, open it, and assert that `Window::groups(1).summarize(summarizer_for("http://127.0.0.1:9".into())).recall(&[], &mut State::new(&mut storage))` returns an empty `Vec`.

- [x] Step 6: In `examples/memory.rs`, build each agent with `.memory(...)` (`Window::groups(2)`, `Window::tokens(200, HeuristicCounter)`, `Window::groups(2).summarize(Summarizer::new(client))`) and each conversation over `InMemoryStorage::new()`, and replace `summarized_chat.storage().summary()` with `summarized_chat.summary().await` handled as a `Result<Option<String>, Error>`. In `examples/persist.rs`, set `.memory(Window::groups(20))` on the agent and pass `storage` without `.window(20)`. Update the comments that described the storage window.

- [x] Step 7: Add `Conversation::summary` in `src/conversation.rs`, needed by Steps 4 and 6:

```rust
    /// The summary the agent's memory made last for this conversation, if
    /// there is one.
    ///
    /// `None` when the memory does not summarize, or has not needed to yet.
    pub async fn summary(&mut self) -> Result<Option<String>, Error> {
        let mut state = crate::State::cached(&mut self.storage, &mut self.cache);
        let value = state
            .get(crate::memory::SUMMARY_KEY)
            .await
            .map_err(|error| self.agent.storage_error(format!("storage state: {error}")))?;
        Ok(value.and_then(|value| value.get("text")?.as_str().map(str::to_owned)))
    }
```

- [x] Step 8: Run `CHECK`, and fix every broken intra-doc link it reports in `src/` by pointing it at `crate::Agent::memory`, `crate::Window` or `crate::Summarize`.
- [x] Step 9: Run `grep -rn "\.window(\|window_by_tokens(\|\.summarize(" src tests examples` and confirm no match is a call on `InMemoryStorage` or `JsonlStorage`.
- [x] Step 10: Commit: `Make InMemoryStorage and JsonlStorage pure stores`

### Task 4: `Conversation::compact` → verify: `CHECK` exits 0, and `cargo test --test compact` exits 0

**Blocked by:** Task 3

**Files:**
- Modify: `src/conversation.rs`
- Modify: `src/lib.rs`
- Create: `tests/compact.rs`

- [x] Step 1: In `src/conversation.rs`, add the builder and the method:

```rust
/// A summary made now rather than when the window needs one. Built by
/// [`Conversation::compact`], and run by awaiting it.
#[must_use = "a compaction does nothing until it is awaited"]
pub struct Compact<'c, S: Storage> {
    chat: &'c mut Conversation<S>,
    keep: Option<usize>,
}

impl<S: Storage> Compact<'_, S> {
    /// Keep the newest `groups` turn groups as they are, and summarize the
    /// rest. Unset, the memory keeps what its own automatic summary would.
    pub fn keep(mut self, groups: usize) -> Self {
        self.keep = Some(groups);
        self
    }
}

impl<'c, S: Storage + 'c> std::future::IntoFuture for Compact<'c, S> {
    type Output = Result<(), Error>;
    type IntoFuture =
        std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Error>> + Send + 'c>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { self.chat.compact_now(self.keep).await })
    }
}
```

and inside `impl<S: Storage> Conversation<S>`:

```rust
    /// Summarizes the older part of the conversation now.
    ///
    /// The next [`Conversation::send`] sends the summary in place of the
    /// summarized turns, even when the window would not have needed to drop
    /// them. Every turn stays in storage. Needs a memory that summarizes,
    /// built with [`crate::Window::summarize`]: any other memory, or none,
    /// returns an error and makes no request.
    ///
    /// ```no_run
    /// # async fn run(mut chat: freyja::Conversation<freyja::InMemoryStorage>) -> Result<(), freyja::Error> {
    /// chat.compact().keep(4).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn compact(&mut self) -> Compact<'_, S> {
        Compact {
            chat: self,
            keep: None,
        }
    }

    async fn compact_now(&mut self, keep: Option<usize>) -> Result<(), Error> {
        let Some(memory) = self.agent.memory_handle() else {
            return Err(self.agent.storage_error(
                "compact: this agent has no memory; set one with Agent::memory".into(),
            ));
        };
        let stored = self
            .storage
            .load()
            .await
            .map_err(|error| self.agent.storage_error(format!("storage load: {error}")))?;
        let mut state = crate::State::cached(&mut self.storage, &mut self.cache);
        memory
            .compact(&stored, &mut state, keep)
            .await
            .map_err(|error| self.agent.storage_error(format!("compact: {error}")))
    }
```

- [x] Step 2: In `src/lib.rs`, change `pub use conversation::Conversation;` to `pub use conversation::{Compact, Conversation};`.

- [x] Step 3: Create `tests/compact.rs`:

```rust
//! Summarizing on demand with `Conversation::compact`.

mod common;
use common::{ok_response, serve};
use freyja::{Agent, Client, Dialect, EndpointConfig, InMemoryStorage, Summarizer, Window};

fn reply(content: &str) -> &'static str {
    let body = format!(
        r#"{{"id":"x","model":"test-model","choices":[{{"message":{{"role":"assistant","content":"{content}"}},"finish_reason":"stop"}}]}}"#
    );
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    Box::leak(head.into_boxed_str())
}

fn client(base: String) -> Client {
    Client::new(
        EndpointConfig::new(Dialect::OpenAiChat, "local", base).default_model("test-model"),
        "sk-test",
    )
}

fn sent_messages(raw: &str) -> Vec<serde_json::Value> {
    let body = &raw[raw.rfind("\r\n").expect("a header line") + 2..];
    let sent: serde_json::Value = serde_json::from_str(body).expect("json body");
    sent["messages"].as_array().expect("messages array").clone()
}

#[tokio::test]
async fn compact_takes_effect_before_the_window_needs_it() {
    let (base, requests) = serve(&[ok_response(), ok_response(), reply("- SUMMARY"), ok_response()]);
    let client = client(base);
    let agent = Agent::new(client.clone())
        .memory(Window::groups(10).summarize(Summarizer::new(client)));
    let mut chat = agent.conversation(InMemoryStorage::new());

    chat.send("first").await.expect("run");
    chat.send("second").await.expect("run");
    chat.compact().keep(2).await.expect("compact");
    chat.send("third").await.expect("run");

    // The server records a request after answering it, so wait for each.
    let raw: Vec<String> = (0..4).map(|_| requests.recv().expect("a request")).collect();
    let last = sent_messages(&raw[3]);
    // The summary, the two kept groups, then the new turn.
    assert_eq!(last.len(), 4, "{last:?}");
    assert!(last[0]["content"].as_str().unwrap_or_default().contains("- SUMMARY"));
    assert_eq!(last[1]["content"], "second");
    assert_eq!(last[3]["content"], "third");
    assert_eq!(chat.summary().await.expect("summary"), Some("- SUMMARY".to_string()));
}

#[tokio::test]
async fn compact_without_keep_keeps_what_the_automatic_summary_would() {
    // A window of four keeps two groups when it summarizes on its own.
    let (base, requests) = serve(&[ok_response(), ok_response(), reply("- SUMMARY"), ok_response()]);
    let client = client(base);
    let agent = Agent::new(client.clone())
        .memory(Window::groups(4).summarize(Summarizer::new(client)));
    let mut chat = agent.conversation(InMemoryStorage::new());

    chat.send("first").await.expect("run");
    chat.send("second").await.expect("run");
    chat.compact().await.expect("compact");
    chat.send("third").await.expect("run");

    let raw: Vec<String> = (0..4).map(|_| requests.recv().expect("a request")).collect();
    let last = sent_messages(&raw[3]);
    assert_eq!(last.len(), 4, "{last:?}");
    assert_eq!(last[1]["content"], "second");
}

#[tokio::test]
async fn compact_needs_a_memory_that_summarizes() {
    // A spare reply, so a request compact wrongly made would be recorded.
    let (base, requests) = serve(&[ok_response(), ok_response()]);
    let plain = Agent::new(client(base.clone())).memory(Window::groups(4));
    let mut chat = plain.conversation(InMemoryStorage::new());
    chat.send("first").await.expect("run");
    assert!(chat.compact().keep(0).await.is_err());

    let none = Agent::new(client(base));
    let mut chat = none.conversation(InMemoryStorage::new());
    assert!(chat.compact().await.is_err());

    // Only the one send reached the server. Compact returned before any
    // request it could have made, so a short wait is enough to see one.
    requests.recv().expect("the send");
    assert!(
        requests
            .recv_timeout(std::time::Duration::from_millis(300))
            .is_err()
    );
}
```

- [x] Step 4: Run `CHECK`
- [x] Step 5: Run `cargo test --test compact`
- [x] Step 6: Commit: `Add Conversation::compact`

### Task 5: Contract page and docs → verify: `CHECK` exits 0, `test -f docs/extending.md` exits 0, and `grep -c "extending.md" docs/README.md` prints at least 1

**Blocked by:** Task 4

**Files:**
- Create: `docs/extending.md`
- Modify: `docs/README.md`
- Modify: `docs/reference/storage.md`
- Modify: `docs/building-an-agent.md`
- Modify: `docs/features.md`
- Modify: `README.md`
- Modify: `CHANGELOG.md`
- Modify: `tests/public_api.rs`

- [x] Step 1: Create `docs/extending.md` with these sections, in prose that follows `plain`: what a crate can extend (a table of `Tool`, `Storage`, `Memory`, `TokenCounter`, one line each on its job and an example crate idea), the rules each implementation must honour (copied from each trait's doc comment: `Storage` order, `clear` on empty, `clear` removes state, state returns the latest value; `Memory` keeps nothing in itself and uses `State`; `TokenCounter` is sync and per message; `Tool` reports failure as text), how to run `freyja::testing::storage_conformance` from a backend crate's tests with a code block, and the stability rules (boxed futures for `dyn`, new trait methods arrive with defaults, `#[non_exhaustive]` types).
- [x] Step 2: In `docs/README.md`, add a link to `extending.md` in its table of contents, following the form of the existing entries (read the file first).
- [x] Step 3: Rewrite every passage in `docs/reference/storage.md`, `docs/building-an-agent.md`, `docs/features.md` and `README.md` that describes `InMemoryStorage::window`, `window_by_tokens`, `summarize` or `summary`, or `JsonlStorage`'s summary record, to describe `Agent::memory`, `Window`, `Summarize`, the state slot, `Conversation::summary` and `Conversation::compact`. Find them with `grep -n "window\|summar" docs/reference/storage.md docs/building-an-agent.md docs/features.md README.md`. In `README.md`'s roadmap Phase 3 paragraph, say memory is now a `Memory` trait that works with any `Storage`, and that a backend can be checked with `freyja::testing`.
- [x] Step 4: In `CHANGELOG.md` under `## Unreleased`, add under `### Added` entries for `Memory`, `Window`, `Summarize`, `State`, the `Storage` state slot, `Agent::memory`, `Conversation::compact`, `Conversation::summary` and `freyja::testing::storage_conformance`, and add a `### Changed` entry (create the heading if absent) saying `InMemoryStorage` and `JsonlStorage` are pure stores, their `window`, `window_by_tokens`, `summarize` and `summary` methods are removed in favour of `Agent::memory`, and `JsonlStorage` writes a `state` record in place of its `summary` record. Edit the existing `JsonlStorage` and `Summarizer` entries so they no longer name the removed builders.
- [x] Step 5: Append to `tests/public_api.rs`:

```rust
#[test]
fn the_extension_traits_are_public() {
    fn storage<T: freyja::Storage>() {}
    fn memory<T: freyja::Memory>() {}
    storage::<freyja::InMemoryStorage>();
    storage::<freyja::JsonlStorage>();
    memory::<freyja::Window>();
    memory::<freyja::Summarize>();
    let _: fn(&dyn freyja::TokenCounter) = |_| {};
    let _: Option<&dyn freyja::Tool> = None;
    let _: Option<freyja::State<'static>> = None;
}
```

- [x] Step 6: Run `CHECK`
- [x] Step 7: Run `test -f docs/extending.md` and `grep -c "extending.md" docs/README.md`
- [x] Step 8: Commit: `Document the extension contract and the memory split`
