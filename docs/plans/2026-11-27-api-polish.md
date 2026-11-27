# api-polish: Implementation Plan

**Spec:** docs/specs/2026-11-27-api-polish-design.md
**Goal:** Fix four first-minute rough edges: `from_env` returns a `Result`, `Agent::ask` answers one message, `Window::last` replaces `Window::groups`, and `examples/agent.rs` uses a conversation over storage.
**Architecture:** A new `Error::MissingApiKey` variant carries what `from_env` used to drop. `Agent::ask` is a one-line wrapper over a conversation on an empty `Vec`. The rename and the example are mechanical.

## Global constraints

- No new dependency.
- Rust 1.88, edition 2024, the MSRV CI checks.
- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-features` and `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features` all pass.
- Public API changes are recorded in `CHANGELOG.md` under Unreleased. `Window::groups` is unreleased, so it is renamed with no alias.
- Tests never set environment variables. They use a variable name no environment sets.
- Any call to a real LLM server uses as few tokens as possible: send "hi" and have the model answer only "hi". This run makes none.
- Commit messages carry no Co-Authored-By line and no generated-by footer.
- Prose follows the `plain` rules: no em or en dashes, no semicolons in prose, no hard wrap inside a markdown paragraph.
- `CHECK` below means: `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-features && RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features` (add `--offline` to the cargo commands if the network is unreachable).

### Task 1: `from_env` returns a `Result` → verify: `CHECK` exits 0, and `grep -rn "let Some(client) = Client::from_env\|from_env(.*)\.expect(" src examples docs README.md` finds no match

**Blocked by:** none

**Files:**
- Modify: `src/error.rs`
- Modify: `src/client.rs`
- Modify: `src/lib.rs`
- Modify: `src/endpoint/mod.rs`
- Modify: every file under `examples/` that calls `Client::from_env`
- Modify: `README.md`
- Modify: `CHANGELOG.md`
- Modify: `docs/concepts.md`, `docs/getting-started.md`, `docs/internals/adding-a-dialect.md`, `docs/introduction.md`, `docs/providers/anthropic.md`, `docs/providers/custom.md`, `docs/providers/gemini.md`, `docs/providers/openai.md`, `docs/providers/README.md`, `docs/reference/client.md`, `docs/reference/errors.md`

- [x] Step 1: In `src/error.rs`, add a variant to `enum Error`, after `InvalidRequest`:

```rust
    /// [`crate::Client::from_env`] found no API key: the variable the
    /// endpoint names is unset or empty, or the endpoint names none.
    MissingApiKey {
        endpoint: Arc<str>,
        variable: Option<String>,
    },
```

  Add `Self::MissingApiKey { endpoint, .. }` to the arms of `Error::endpoint`. In `impl fmt::Display for Error`, add an arm following the existing ones' form (read them first) that writes `{endpoint}: {variable} is unset or empty` when `variable` is `Some`, and `{endpoint}: no API key variable is configured` when it is `None`. Leave `is_retryable` as it is (its `_ => false` arm covers it). Fix any other exhaustive `match` on `Error` the compiler reports.

- [x] Step 2: In `src/client.rs`, replace `from_env` with:

```rust
    /// Reads the API key from the endpoint's [`EndpointConfig::api_key_env`].
    ///
    /// Fails with [`Error::MissingApiKey`] when that variable is unset or
    /// empty, or when the endpoint names none. An endpoint with
    /// [`Auth::None`] needs no key and always succeeds.
    pub fn from_env(config: impl Into<EndpointConfig>) -> Result<Self, Error> {
        let config = config.into();
        if config.auth == Auth::None {
            return Ok(Self::build(config, None, crate::transport::default_http()));
        }
        let Some(variable) = config.api_key_env.clone() else {
            return Err(Error::MissingApiKey {
                endpoint: config.name.clone(),
                variable: None,
            });
        };
        match std::env::var(&variable) {
            Ok(key) if !key.is_empty() => Ok(Self::build(
                config,
                Some(key),
                crate::transport::default_http(),
            )),
            _ => Err(Error::MissingApiKey {
                endpoint: config.name.clone(),
                variable: Some(variable.to_string()),
            }),
        }
    }
```

  Read the type of `api_key_env` first and adapt `.clone()` and `.to_string()` to it.

- [x] Step 3: In the `#[cfg(test)]` module of `src/client.rs`, add tests: `from_env_names_the_missing_variable` (a config with `api_key_env("FREYJA_TEST_UNSET_KEY_6F2A")`, assert `Err(Error::MissingApiKey { variable: Some(v), .. })` with `v == "FREYJA_TEST_UNSET_KEY_6F2A"` and that `to_string()` contains it), and `from_env_needs_no_key_without_auth` (a config with `Auth::None`, assert `Ok`). Read how `EndpointConfig` sets auth and the key variable before writing them.

- [x] Step 4: Update every caller. In each example, replace `let Some(client) = Client::from_env(provider) else { eprintln!(...); return; };` (and any `.expect(...)` form) with a `match` that prints the error and returns, for example:

```rust
    let client = match Client::from_env(provider) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("{error}");
            return;
        }
    };
```

  or `?` where the function returns a `Result` whose error converts from `freyja::Error`. In `src/lib.rs`, `README.md` and every listed doc, write `Client::from_env(...)?` in place of `.expect(...)`, adjusting surrounding prose that says it returns `None`. Find every site with `grep -rn "from_env" src examples docs README.md`. In `docs/reference/errors.md`, add `MissingApiKey` beside the other variants in the file's existing form.

- [x] Step 5: In `CHANGELOG.md` under `## Unreleased`, add `Error::MissingApiKey` under `### Added` and, under `### Changed`, that `Client::from_env` returns `Result<Client, Error>` in place of `Option<Client>`. Follow the file's entry style.

- [x] Step 6: Run `CHECK`
- [x] Step 7: Run the grep in this task's verify clause
- [x] Step 8: Commit: `Return an error naming the variable from Client::from_env`

### Task 2: `Agent::ask` → verify: `CHECK` exits 0, and `cargo test --test model ask` exits 0, and `cargo test --test model -- --list | grep -c ask` prints at least 1

**Blocked by:** Task 1

**Files:**
- Modify: `src/agent.rs`
- Modify: `tests/model.rs`
- Modify: `CHANGELOG.md`

- [ ] Step 1: In `src/agent.rs`, add after `conversation`:

```rust
    /// Answers one message with no history.
    ///
    /// Sends the agent's configuration and `message`, runs the tool loop, and
    /// keeps nothing, so a second call knows nothing of the first. For a
    /// chat, start a [`Agent::conversation`].
    ///
    /// ```no_run
    /// # async fn run(agent: freyja::Agent) -> Result<(), freyja::Error> {
    /// println!("{}", agent.ask("What is 20 + 22?").await?.answer);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn ask(&self, message: impl Into<Message>) -> Result<Run, Error> {
        self.conversation(Vec::new()).send(message).await
    }
```

  adapting imports.

- [ ] Step 2: In `tests/model.rs`, give the scripted model a record of the requests it receives (for example a `Mutex<Vec<usize>>` pushing `request.messages.len()` minus system messages on each `generate`, or the messages themselves), without changing what existing tests assert. Add `ask_keeps_no_history`: an agent over a script of two plain text answers, two `ask` calls, assert both answers, and assert the second recorded request carried exactly one non-system message.

- [ ] Step 3: In `CHANGELOG.md` under `### Added`, add `Agent::ask`, following the file's entry style.

- [ ] Step 4: Run `CHECK`
- [ ] Step 5: Run `cargo test --test model ask` and `cargo test --test model -- --list | grep -c ask`
- [ ] Step 6: Commit: `Add Agent::ask for one message with no history`

### Task 3: `Window::last` replaces `Window::groups` → verify: `CHECK` exits 0, and `grep -rn "Window::groups" src tests examples README.md CHANGELOG.md docs --exclude-dir=specs --exclude-dir=plans` finds no match

**Blocked by:** Task 2

**Files:**
- Modify: `src/memory.rs`
- Modify: `src/builtin/jsonl.rs`
- Modify: `src/helper/window.rs`
- Modify: `tests/agent.rs`, `tests/compact.rs`, `tests/jsonl.rs`, `tests/memory.rs`, `tests/storage.rs`, `tests/summarize.rs`
- Modify: `examples/memory.rs`, `examples/persist.rs`
- Modify: `README.md`, `CHANGELOG.md`, `docs/building-an-agent.md`, `docs/features.md`, `docs/reference/storage.md`, `docs/extending.md`

- [ ] Step 1: In `src/memory.rs`, rename `Window::groups` to `Window::last` and give it this doc:

```rust
    /// The last `n` turns, plus pinned turns.
    ///
    /// A turn is one message, except that a tool call and the results
    /// answering it count as one, so a window never splits them. The rule is
    /// [`crate::window_by_groups`].
```

  Update the `Debug` impl's label from `Window::groups` to `Window::last`.

- [ ] Step 2: Replace every other `Window::groups` with `Window::last` in the listed files, found with `grep -rn "Window::groups" src tests examples README.md CHANGELOG.md docs --exclude-dir=specs --exclude-dir=plans`. Where prose around a call explains "groups", reword it to "turns" only where it describes `Window::last`. The `CHANGELOG.md` entries that added `Window::groups` under Unreleased now name `Window::last`.

- [ ] Step 3: Run `CHECK`
- [ ] Step 4: Run the grep in this task's verify clause
- [ ] Step 5: Commit: `Rename Window::groups to Window::last`

### Task 4: `examples/agent.rs` over a conversation → verify: `CHECK` exits 0, `cargo build --examples` exits 0, and `grep -c "Vec<Message>" examples/agent.rs` prints 0

**Blocked by:** Task 3

**Files:**
- Modify: `examples/agent.rs`

- [ ] Step 1: Read `examples/agent.rs`. Build `let mut chat = agent.conversation(InMemoryStorage::new());` once before the input loop, send each line with `chat.send(line).await`, and on `/reset` call `chat.clear().await` and report a failure if it returns one. Remove the `Vec<Message>` and every fresh conversation built per turn. Rewrite the module doc so it describes this: `Agent` drives the tool loop, and the conversation keeps the history in `InMemoryStorage`. Keep the tools, the system prompt, the stop-reason handling and the commands as they are.
- [ ] Step 2: Run `CHECK`
- [ ] Step 3: Run `cargo build --examples` and `grep -c "Vec<Message>" examples/agent.rs`
- [ ] Step 4: Commit: `Drive the agent example through a conversation`
