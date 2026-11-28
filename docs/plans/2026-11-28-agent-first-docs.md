# agent-first-docs: Implementation Plan

**Spec:** docs/specs/2026-11-28-agent-first-docs-design.md
**Goal:** The website is the only copy of the docs, and every example leads with `Agent`, with the low-level calls on one guide page.
**Architecture:** Website tasks run in the freyja-web worktree `WEB = /home/rizukirr/Projects/freyja-web/.vibe-worktrees/agent-first-docs` on branch `repl-run/agent-first-docs`. Crate tasks run in the freyja worktree `REPO = /home/rizukirr/Projects/freyja/.vibe-worktrees/agent-first-docs` on branch `repl-run/agent-first-docs`. Paths in a task are relative to the worktree that task names.

## Global constraints
- Rust 2024, MSRV 1.88. No new dependency in either repo.
- No live LLM calls. Examples are built, never run. Use `cargo --offline`.
- Plain writing rules in every line written: no em dash or en dash, no semicolon, one line per paragraph with no hard wrapping, straight quotes, sentence-case headings, no bold label lists.
- Keep each page's structure. Change examples and the sentences around them, not unrelated prose.
- Website URL scheme for links from outside the site: `https://rizukirr.github.io/freyja-web/docs/<path without .md>`. `docs/README.md` maps to `https://rizukirr.github.io/freyja-web/docs/` and `docs/providers/README.md` to `https://rizukirr.github.io/freyja-web/docs/providers/`. Anchors keep their `#fragment`.
- The `Agent` API, from `REPO/src/agent.rs` and `REPO/src/conversation.rs`: `Agent::new(model)`, builders `tool`, `tools`, `model`, `max_tokens`, `temperature`, `top_p`, `reasoning_effort`, `tool_choice`, `extra_for`, `system`, `max_turns`, `token_budget`, `memory`, `guard`, `hook`, then `ask(message) -> Result<Run, Error>`, `conversation(storage)` giving `send`, `send_with`, `send_streaming(message, |event| ...)`, `send_streaming_with`. `Run` has `answer`, `stop`, `usage`, `turns`. Read those files before writing Rust code that uses them.

### Task 1: The low-level guide and the Recorder section (WEB) → verify: in WEB, `npm run build` exits 0 and `grep -c "ScriptedModel::replay" docs/extending.md` prints a number at least 1

**Blocked by:** none

**Files:**
- Create: `docs/calling-the-model-directly.md`
- Modify: `sidebars.js`
- Modify: `docs/README.md`
- Modify: `docs/extending.md`

- [x] Step 1: Read `docs/getting-started.md`, `docs/building-an-agent.md`, `docs/concepts.md`, `docs/reference/client.md` and `docs/reference/streaming.md` in WEB, and `REPO/examples/tool_loop.rs`, to collect every low-level example the site shows today.
- [x] Step 2: Create `docs/calling-the-model-directly.md` with the H1 `# Calling the model directly`. Open with one paragraph saying `Agent` is the way to build with Freyja and this page is the layer under it, for when you want one request and one response, typed output, raw stream events, a pre-flight check, or your own tool loop. Then sections, each with a short explanation and a Rust example: `## One request, one response` (`Client::generate` with a `GenerateRequest`, reading `output_text()` and `usage`), `## Typed output` (`Client::generate_as`), `## Streaming events` (`Client::stream` and draining events, `into_response`), `## Checking a request before sending` (`Client::check`), `## Your own tool loop` (the manual loop: send, run `tool_calls()`, push `Message::tool_result`, repeat, matching `REPO/examples/tool_loop.rs`). End with `## Where to go next` linking `building-an-agent.md`, `reference/client.md`, `reference/requests.md`, `reference/responses.md` and `reference/streaming.md`. Reuse code from the pages read in Step 1 where it exists, and check every method name against `REPO/src/client.rs`.
- [x] Step 3: In `sidebars.js`, change `'building-an-agent', 'extending'` to `'building-an-agent', 'calling-the-model-directly', 'extending'`.
- [x] Step 4: In `docs/README.md`, in the Learn table, add a row after the Building an agent row: `| [Calling the model directly](calling-the-model-directly.md) | The low-level calls under Agent: one request, typed output, raw streams, your own loop |`.
- [x] Step 5: In `docs/extending.md`, in `## Testing with a scripted model`, after the paragraph that starts "`respond` adds any other response shape", add a blank line, then this paragraph on one line: "`Recorder` turns a real run into a script. Wrap the model in it, run the agent once against the real provider, and save what the model answered. `ScriptedModel::replay` loads the file, so the test runs on those answers with no network and no key. Only answers are saved: replay hands them out in order whatever the requests are, and a call that failed is not saved. A recorder answers a streaming caller all at once, after the whole response has arrived." Then a blank line and this block:

```rust
use freyja::testing::{Recorder, ScriptedModel};
use std::sync::Arc;

// Once, against the real provider.
let model = Arc::new(Recorder::new(Client::from_env()?));
Agent::new(model.clone()).tool(add).ask("add 1 and 2").await?;
model.save("tests/fixtures/add.jsonl")?;

// In the test, from then on.
let model = ScriptedModel::replay("tests/fixtures/add.jsonl")?;
let run = Agent::new(model).tool(add).ask("add 1 and 2").await?;
```

- [x] Step 6: Run `npm run build` in WEB.
- [x] Step 7: Commit the four files in WEB as "Add Calling the model directly, and the Recorder section".

### Task 2: Stop syncing from the freyja repo (WEB) → verify: in WEB, `npm test` exits 0 and `grep -rn "sync-docs\|rizukirr/freyja/edit" package.json README.md docusaurus.config.js` finds no match

**Blocked by:** Task 1

**Files:**
- Delete: `scripts/sync-docs.mjs`
- Delete: `test/sync-docs.test.mjs`
- Modify: `package.json`
- Modify: `README.md`
- Modify: `docusaurus.config.js`

- [ ] Step 1: `git rm scripts/sync-docs.mjs test/sync-docs.test.mjs`.
- [ ] Step 2: In `package.json`, delete the line `"sync-docs": "node scripts/sync-docs.mjs",`.
- [ ] Step 3: In `README.md`, replace the whole `## Documentation source` section (its heading through the paragraph that ends "before committing the snapshot.") with the heading `## Documentation source` and this paragraph on one line: "The `docs/` directory is the Freyja documentation. Edit it here. The Rust repository no longer carries a copy."
- [ ] Step 4: In `docusaurus.config.js`, change `editUrl: 'https://github.com/rizukirr/freyja/edit/main/'` to `editUrl: 'https://github.com/rizukirr/freyja-web/edit/main/'`.
- [ ] Step 5: Run `npm test` and `npm run build` in WEB.
- [ ] Step 6: Commit in WEB as "Edit the docs here, and stop syncing from the freyja repo".

### Task 3: Guide pages lead with Agent (WEB) → verify: in WEB, `npm run build` exits 0 and `grep -ln "client\.generate\|client\.stream" docs/getting-started.md docs/concepts.md docs/building-an-agent.md docs/introduction.md docs/features.md docs/extending.md` finds no match

**Blocked by:** Task 2

**Files:**
- Modify: `docs/getting-started.md`
- Modify: `docs/concepts.md`
- Modify: `docs/building-an-agent.md`
- Modify: `docs/introduction.md`
- Modify: `docs/features.md`
- Modify: `docs/extending.md`

- [ ] Step 1: In each file, find every code example that calls `client.generate`, `client.generate_as` or `client.stream`, and every sentence that tells the reader to start with them.
- [ ] Step 2: Rewrite each such example with `Agent`: `Agent::new(client)` with builders, then `.ask(...)` for one question, `.conversation(InMemoryStorage::new())` and `.send(...)` for several turns, `.send_streaming(...)` for streaming. Read the answer from `run.answer`, the usage from `run.usage`. Error handling matches on the same `freyja::Error` returned by `ask`. Where `getting-started.md` shows the first call, the first call is `Agent::new(client).ask("...")`.
- [ ] Step 3: In `building-an-agent.md`, the manual tool loop section moves out: replace its code with one paragraph saying `Agent` runs this loop for you, and linking to [Calling the model directly](calling-the-model-directly.md#your-own-tool-loop) for writing it by hand. Keep the tool loop diagram.
- [ ] Step 4: Wherever a page explains something only the low-level call can do (typed output with `generate_as`, raw stream events, `check`), replace the example with one sentence linking the matching section of `calling-the-model-directly.md`.
- [ ] Step 5: Run `npm run build` in WEB.
- [ ] Step 6: Commit in WEB as "Guide pages lead with Agent".

### Task 4: Reference pages lead with Agent (WEB) → verify: in WEB, `npm run build` exits 0 and `grep -rln "client\.generate\|client\.stream" docs --include=*.md | grep -v "^docs/calling-the-model-directly.md$\|^docs/reference/client.md$\|^docs/reference/wire/\|^docs/specs/\|^docs/plans/"` finds no match

**Blocked by:** Task 3

**Files:**
- Modify: `docs/reference/requests.md`
- Modify: `docs/reference/responses.md`
- Modify: `docs/reference/errors.md`
- Modify: `docs/reference/tools.md`
- Modify: `docs/reference/streaming.md`
- Modify: `docs/reference/storage.md`
- Modify: `docs/reference/messages.md`
- Modify: `docs/reference/client.md`
- Modify: `docs/providers/README.md`
- Modify: `docs/providers/openai.md`
- Modify: `docs/providers/anthropic.md`
- Modify: `docs/providers/gemini.md`
- Modify: `docs/providers/openai-chat.md`
- Modify: `docs/providers/custom.md`

- [ ] Step 1: Run `grep -rn "client\.generate\|client\.stream" docs --include=*.md` in WEB to list every remaining call, and read each listed file in the Files block.
- [ ] Step 2: For each example whose point is calling the model, use `Agent` as in Task 3 Step 2. `errors.md` matches on the result of `agent.ask(...)`. `streaming.md` leads with `Conversation::send_streaming` and `AgentEvent`, and its `Client::stream` material stays only as a short section that links to [Calling the model directly](../calling-the-model-directly.md#streaming-events). `requests.md` shows the request fields through the `Agent` builders that set them, keeping its field reference, and its "extend one request across a conversation" example becomes a `Conversation`. `responses.md` explains that `Run` is what an agent returns and `GenerateResponse` is what one model call returns, with the low-level example replaced by a link to the guide. `tools.md` replaces its manual loop with a link to the guide's `#your-own-tool-loop`.
- [ ] Step 3: Leave `docs/reference/wire/` unchanged, and the body of `docs/reference/client.md`. At the top of `docs/reference/client.md`, after its H1, add one line: "These are the low-level calls under `Agent`. For a guided tour, read [Calling the model directly](../calling-the-model-directly.md)."
- [ ] Step 4: Run `npm run build` in WEB.
- [ ] Step 5: Commit in WEB as "Reference pages lead with Agent".

### Task 5: Remove the docs copy from freyja (REPO) → verify: in REPO, `git ls-files docs` prints nothing, `grep -n "](docs/" README.md` finds no match, and `RUSTDOCFLAGS='-D warnings' cargo --offline doc --no-deps --all-features` exits 0

**Blocked by:** none

**Files:**
- Delete: every tracked file under `docs/`
- Modify: `README.md`
- Modify: `CHANGELOG.md`
- Modify: `Cargo.toml`
- Modify: `src/dialect/refusal.rs`
- Modify: `src/endpoint/presets.rs`
- Modify: `src/transport/mod.rs`

- [x] Step 1: In REPO, run `git rm -r -q $(git ls-files docs)`. The gitignored `docs/specs` and `docs/plans` stay on disk.
- [x] Step 2: In `README.md`, rewrite every link target that starts with `docs/` to the website URL using the scheme in Global constraints. The sentence "Full docs in [`docs/`](docs/README.md), written to be read in order:" becomes "Full docs are at [rizukirr.github.io/freyja-web](https://rizukirr.github.io/freyja-web/docs/), written to be read in order:". Add a row for Calling the model directly after the Building an agent row in its table, matching the table's two-column shape, with the text "The low-level calls under Agent".
- [x] Step 3: In `CHANGELOG.md`, rewrite each link target that starts with `docs/` to the website URL using the same scheme. Change nothing else.
- [x] Step 4: In `Cargo.toml`, read the comment near line 17 that says the package includes `docs/`, and rewrite it to describe what the package includes now, in one or two comment lines.
- [x] Step 5: In `src/dialect/refusal.rs`, `src/endpoint/presets.rs` and `src/transport/mod.rs`, replace each mention of a `docs/...md` path in a comment with the website URL for that page.
- [x] Step 6: Run `grep -rn "docs/[a-z/-]*\.md" README.md CHANGELOG.md Cargo.toml src examples` and fix any remaining reference to a deleted file. Then run `RUSTDOCFLAGS='-D warnings' cargo --offline doc --no-deps --all-features` and `cargo --offline test --all-features`.
- [x] Step 7: Commit in REPO as "Move the docs to freyja-web".

### Task 6: Crate docs lead with Agent (REPO) → verify: in REPO, `grep -c "client.generate\|\.generate(&" src/lib.rs` prints 0 and `cargo --offline test --doc --all-features` exits 0

**Blocked by:** Task 5

**Files:**
- Modify: `src/lib.rs`

- [ ] Step 1: Read the `//!` crate documentation in `src/lib.rs`.
- [ ] Step 2: Rewrite each example that calls `client.generate` to use `Agent` as in Task 3 Step 2, keeping doctest attributes (`no_run` and the like) as they are. The manual tool loop example becomes an `Agent` with a `#[tool]` function. Add one paragraph saying `Client::generate` and `Client::stream` are the low-level layer under `Agent`, linking `https://rizukirr.github.io/freyja-web/docs/calling-the-model-directly`. Mention `Client::generate` in prose only, never as a call in a code block.
- [ ] Step 3: Run `cargo --offline test --doc --all-features` and `RUSTDOCFLAGS='-D warnings' cargo --offline doc --no-deps --all-features`.
- [ ] Step 4: Commit in REPO as "Crate docs lead with Agent".

### Task 7: Examples lead with Agent (REPO) → verify: in REPO, `cargo --offline build --examples --all-features` exits 0, `cargo --offline clippy --all-features --all-targets -- -D warnings` exits 0, and `grep -l "\.generate(\|\.generate_as\|\.stream(" examples/*.rs` lists only `examples/tool_loop.rs`, `examples/retry.rs`, `examples/pairing_probe.rs`, `examples/portable.rs`, `examples/structured_output.rs` and `examples/streaming.rs`

**Blocked by:** Task 6

**Files:**
- Modify: `examples/simple.rs`
- Modify: `examples/custom_endpoint.rs`
- Modify: `examples/images.rs`
- Modify: `examples/async_tools.rs`
- Modify: `examples/tool_loop.rs`
- Modify: `examples/retry.rs`
- Modify: `examples/pairing_probe.rs`
- Modify: `examples/portable.rs`
- Modify: `examples/structured_output.rs`
- Modify: `examples/streaming.rs`
- Modify: `README.md`

- [ ] Step 1: Rewrite `simple.rs`, `custom_endpoint.rs`, `images.rs` and `async_tools.rs` to call the model through `Agent` (`ask` for one question). `images.rs` passes its image message to `ask` as a `Message`. `async_tools.rs` registers both tools on the agent, which runs a turn's tool calls concurrently, and keeps its timing output. Keep each file's doc comment accurate and its behaviour otherwise the same.
- [ ] Step 2: In `tool_loop.rs`, `retry.rs`, `pairing_probe.rs`, `portable.rs`, `structured_output.rs` and `streaming.rs`, change no code. Add to the end of each file's first doc paragraph one sentence containing the word "low-level", saying it uses the low-level `Client` calls and that `Agent` is the usual entry point, and naming the example that shows the `Agent` way where one exists (`agent` for `tool_loop`, `agent_streaming` for `streaming`).
- [ ] Step 3: In `README.md`, if a table or list describes the examples, update the descriptions of the changed examples to match. Change nothing else.
- [ ] Step 4: Run `cargo --offline build --examples --all-features`, `cargo --offline clippy --all-features --all-targets -- -D warnings` and `cargo --offline test --all-features`.
- [ ] Step 5: Commit in REPO as "Examples lead with Agent, and the low-level ones say so".
