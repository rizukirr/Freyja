---
title: store-memory
date: 2026-11-27
status: approved
---

# store-memory: Design

## Problem

Which turns reach the model is decided inside `Storage::load`, and the rules for it (a group window, a token window, a summary of what either drops) are builder methods on `InMemoryStorage` and `JsonlStorage`. A backend written outside the crate, such as a Postgres one, gets none of them: it can call `window_by_groups`, but it must rebuild summarizing itself, and `JsonlStorage` has the summary hard-coded into its file format. There is also no way to summarize on demand, and no published check that a third-party backend honours the rules `Storage` documents.

## Goals

1. **A `Memory` trait decides what the model sees, independent of the backend.** `Agent::memory(m)` sets it. Criterion: a test runs a third-party `Storage` implemented in `tests/` with `Window::groups(2)`, and the request body carries only the last two groups plus pinned turns while the backend still holds every message.
2. **The built-in rules are `Memory` types.** `Window::groups(n)`, `Window::tokens(budget, counter)` and `.summarize(summarizer)` on either produce the same requests the old `InMemoryStorage` builders did. Criterion: every behaviour asserted by the current window and summary tests in `tests/storage.rs`, `tests/summarize.rs` and `tests/jsonl.rs` is asserted again through `Agent::memory` and passes.
3. **A summary survives a restart in any backend that supports it, and costs no extra call in one that does not.** `Storage` gains `state(key)` and `set_state(key, value)` with default implementations. Criterion: a reopened `JsonlStorage` reuses the stored summary with no second summarizing call, and a `Vec<Message>` backend over two consecutive trimming sends on the same `Conversation` makes one summarizing call, not two. A new `Conversation` over a backend with no state slot starts with an empty cache.
4. **`Conversation::compact()` summarizes now.** It is awaitable directly or after `.keep(n)`. The next `send` sends the summary in place of the summarized groups. Criterion: a test calls `compact().keep(2)` on a conversation with no window trimming needed, and the next request carries the summary plus only the last two groups. `keep` omitted keeps the same groups the memory's automatic summary keeps. `compact` on a memory without a summarizer returns an error and makes no request.
5. **A published conformance check for backends.** `freyja::testing::storage_conformance(make)` checks order is preserved across appends, `clear` succeeds on an empty conversation and empties a full one, and state, when supported, returns what was set, takes the latest value, and is gone after `clear`. Criterion: `InMemoryStorage`, `JsonlStorage`, `Vec<Message>` and the `tests/` third-party backend all pass it, and a deliberately broken backend (reversed order) fails it.
6. **A contract page for authors.** `docs/extending.md` lists the public traits a crate can implement (`Tool`, `Storage`, `Memory`, `TokenCounter`), the rules each must honour, and how to run the conformance check. Criterion: the page exists, every trait it names is exported, and `docs/README.md` links it.

## Non-goals

- A shared store keyed by conversation id. `Storage` stays one value per conversation.
- `load_tail` or any partial load. It can be added later as a defaulted method.
- Applying memory before every request inside the tool loop. Memory runs once per `send`, as today.
- `Model`, `Hook`, the `send` builder, and retrieval. Those are later steps of the plan.
- Defaulting the summarizer to the agent's client. `Summarizer::new(client)` stays as it is.
- A migration path for old `JsonlStorage` files. The type is unreleased, so its record format may change.

## Constraints

- No new dependency.
- Rust 1.88, edition 2024, the MSRV CI checks.
- Every trait keeps boxed futures so it stays usable as `dyn`.
- Pinned turns, group integrity, the repair pass and the refusal of an orphaned tool result behave exactly as today.
- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-features` and `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features` all pass.
- Public API changes are breaking and recorded in `CHANGELOG.md` under Unreleased.

Run limits: at most 3 verify rounds, at most $0 on paid runs

## Approach

Pushback outcome: the smaller framing was taken. `Storage` keeps its shape (one value per conversation, `&mut self`, three required methods). Only the memory rules move out, which is what extensibility needs.

### Units

- **`Memory` trait** (`src/memory.rs`, new). `fn recall<'a>(&'a self, messages: &'a [Message], state: &'a mut State<'_>) -> MemoryFuture<'a, Vec<Message>>` returns what is sent. A default `fn compact(...)` returns an error naming the memory as unable to compact. `MemoryFuture` and `MemoryError` follow `StorageFuture` and `StorageError`.
- **`State`** (`src/memory.rs`). A handle passed to memory that reads and writes the conversation's state slot. It reads the in-process cache first, falls back to `Storage::state`, and writes both. It is how one summary serves many sends whether or not the backend persists state.
- **`Window`** (`src/memory.rs`). `Window::groups(n)` and `Window::tokens(budget, counter)` implement `Memory` by calling the existing `window_by_groups` and `window_by_tokens`.
- **`Summarize`** (`src/memory.rs`). Built by `Window::summarize(summarizer)`. It holds the window and a `Summarizer`, and carries the logic now in `InMemoryStorage::load`: the needed cut, the deeper cut at half the window, reuse while the cached summary covers at least the needed cut, fallback to the plain window when summarizing fails, and the summary message placed after pinned turns. The cached summary is a JSON value `{covers, text}` under the state key `freyja.summary`. A cached summary whose `covers` exceeds the groups held is ignored. A valid cached summary is used even when the window needs no trimming, which is what makes `compact` take effect. It implements `compact`.
- **`Storage` trait** (`src/storage.rs`). Gains `state` and `set_state` with defaults that keep nothing. `clear` must also clear state. The `&mut T` and `Box<T>` forwarding impls forward the new methods.
- **`InMemoryStorage`** keeps messages and a state map, and drops `window`, `window_by_tokens`, `summarize` and `summary`. **`JsonlStorage`** drops the same builders. Its `summary` record becomes a `state` record `{key, value}`, where the last one per key wins.
- **`Agent::memory(impl Memory + 'static)`** stores an `Arc<dyn Memory>`. None set means the whole transcript is sent, as `Storage` returns it.
- **`Conversation`** holds the in-process state cache. Its run calls `storage.load()`, then `memory.recall(...)`, then continues as today. `Conversation::compact()` returns a `Compact` builder with `keep(n)` that implements `IntoFuture`. `Conversation::summary()` returns the cached summary text, if any.
- **`freyja::testing`** (`src/testing.rs`, new, public). `storage_conformance` is an `async fn` that panics with the broken rule named.
- **Docs.** `docs/extending.md` is new. `docs/reference/storage.md`, `docs/building-an-agent.md`, `examples/memory.rs`, `examples/persist.rs`, the README roadmap and `CHANGELOG.md` are updated.

### Data flow per send

`load` (everything stored), then `Memory::recall` (window, cached or new summary), then push the turn, then repair, then refuse if the turn was damaged, then the tool loop, then `append`. Only the second step is new, and it is the code `InMemoryStorage::load` runs today, moved.

### Errors

A storage failure is wrapped as it is today. A summarizing failure inside `recall` falls back to the plain window, as today. A failure in `compact`, either from the summarizer or from an unsupported memory, is returned as `Error::InvalidRequest` through the same wrapper storage errors use.

## Alternatives considered

- **A keyed, shared `Store`.** One store serving many conversations by id. It is a bigger break that removes `impl Storage for Vec<Message>` and the borrowed-vector conversation, for no gain: a Postgres crate can build one cheap value per id over a shared pool.
- **Public helper functions, no new trait.** It is the smallest change, but every backend must call the helpers inside its own `load`, so the policy stays tangled with persistence, which is the friction this step exists to remove.

## Testing

- Migrate the window and summary tests from `tests/storage.rs`, `tests/summarize.rs` and `tests/jsonl.rs` to `Agent::memory`, keeping every assertion.
- New tests: a third-party backend with `Window::groups(2)` (goal 1), `Vec<Message>` with `Summarize` making one call over two trimming sends (goal 3), `compact().keep(2)` with no trimming needed, `compact` with default keep, and `compact` on a plain window erroring with no request (goal 4), conformance over every shipped backend plus a reversed-order backend failing (goal 5).
- A doc test for `docs/extending.md` is not possible in markdown. Instead, goal 6's exports are checked by a `tests/public_api.rs` addition that names each trait.

## Open questions

None. Every question was settled in the run's decision log.
