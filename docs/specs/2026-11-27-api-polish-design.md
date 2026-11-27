---
title: api-polish
date: 2026-11-27
status: approved
---

# api-polish: Design

## Problem

Four rough edges in the first minutes of using Freyja. `Client::from_env` returns `Option`, so every caller writes `.expect("OPENAI_API_KEY")` or handles a bare `None` that does not say which variable is missing. A one-off question needs a storage and a conversation (`agent.conversation(InMemoryStorage::new()).send(...)`). `Window::groups(n)` makes a newcomer learn what a turn group is before bounding a conversation. `examples/agent.rs` keeps its own `Vec<Message>` and hands it to a fresh conversation each turn, which teaches the pattern storage replaced.

## Goals

1. **`Client::from_env` returns `Result<Client, Error>`.** A missing or empty variable is the new `Error::MissingApiKey { endpoint, variable }`, whose message names the variable. Criterion: a unit test over a config naming an unset variable gets `Err(Error::MissingApiKey { .. })` with that variable, and its `Display` contains the variable name. A config with `Auth::None` still returns `Ok`. The README quick start calls it with `?`.
2. **`Agent::ask` answers one message with no history.** `agent.ask("...").await` returns a `Run`, sends only the agent's configuration plus that message, and keeps nothing. Criterion: a test with a scripted `Model` calls `ask` twice and observes that the second request carries one non-system message.
3. **`Window::last(n)` replaces `Window::groups(n)`.** Same behaviour, a name that reads without knowing what a group is, and a doc that says a tool call and its results count as one. Criterion: `grep -rn "Window::groups"` over `src`, `tests`, `examples`, `docs` (outside `docs/specs` and `docs/plans`), `README.md` and `CHANGELOG.md` finds nothing, and the full suite passes.
4. **`examples/agent.rs` uses a conversation over `InMemoryStorage`.** Criterion: the example holds no `Vec<Message>`, `/reset` calls `Conversation::clear`, and `cargo build --examples` passes.

## Non-goals

- Renaming `window_by_groups`, `window_by_tokens` or `Window::tokens`.
- `ask_with`, `ask_streaming`, or the `send` builder.
- Any live call to a paid model.

## Constraints

- No new dependency.
- Rust 1.88, edition 2024, the MSRV CI checks.
- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-features` and `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features` all pass.
- Public API changes are recorded in `CHANGELOG.md` under Unreleased. `Window::groups` is unreleased, so it is renamed with no alias.
- Tests never set environment variables (`std::env::set_var` is `unsafe` in edition 2024 and races across test threads). They use a variable name no environment sets.
- Any call to a real LLM server uses as few tokens as possible: send "hi" and have the model answer only "hi". This run makes none.

Run limits: at most 3 verify rounds, at most $0 on paid runs

## Approach

Pushback outcome: no simpler framing. The user asked for all four fixes after reading them, which settles the rename that a doc note alone could otherwise have covered.

- **`Error::MissingApiKey { endpoint: Arc<str>, variable: Option<String> }`** in `src/error.rs`. `variable` is `None` when the endpoint names no variable at all. Not retryable. `Error::endpoint` covers it. Its `Display` says the variable is unset or empty, or that the endpoint names none.
- **`Client::from_env`** returns `Result<Self, Error>`, building that error where it returned `None`.
- **Call sites.** Examples that wrote `let Some(client) = Client::from_env(p) else { eprintln!(...); return; }` print the error instead. Docs show `?`.
- **`Agent::ask(&self, message: impl Into<Message>) -> Result<Run, Error>`** is `self.conversation(Vec::new()).send(message).await`, with a doc saying it keeps no history and pointing to `conversation` for a chat.
- **`Window::last`** is `Window::groups` renamed, with every use and doc updated.
- **`examples/agent.rs`** builds `agent.conversation(InMemoryStorage::new())` once, sends each line through it, and clears it on `/reset`.

## Alternatives considered

- **Reuse `Error::InvalidRequest` for a missing key.** No new variant, but a caller cannot match on it, and the message would be free text. A new variant on a `#[non_exhaustive]` enum breaks nothing.
- **Keep `Window::groups` and only improve its doc.** No churn, but the name stays jargon in every call a user writes.

## Testing

- Unit tests in `src/client.rs` for the missing variable, the empty endpoint variable, and `Auth::None`.
- A test in `tests/model.rs` for `ask`, reusing the scripted model there and recording each request.
- The existing suite covers the rename.

## Open questions

None.
