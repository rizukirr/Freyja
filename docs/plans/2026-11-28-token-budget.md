# token-budget: Implementation Plan

**Spec:** docs/specs/2026-11-28-token-budget-design.md
**Goal:** `Agent::token_budget(n)` stops a run with `StopReason::OverBudget` once its summed `total_tokens` reaches `n`.
**Architecture:** `Agent` gains an `Option<u64>` field and a builder. `run_loop` in `src/agent.rs` checks the run's summed usage after the `after_model` hooks of each call and breaks with the new `StopReason::OverBudget` variant. Streaming shares `run_loop`, so it needs no separate change.

## Global constraints
- Rust 2024, MSRV 1.88.
- No new dependency, normal or dev.
- No live LLM calls. Use `cargo --offline`.
- `StopReason` stays `#[non_exhaustive]`.

### Task 1: The budget and its tests → verify: `cargo --offline test --all-features` exits 0 and `cargo --offline clippy --all-features --all-targets -- -D warnings` exits 0

**Blocked by:** none

**Files:**
- Modify: `src/agent.rs`
- Modify: `tests/agent.rs`

- [ ] Step 1: In `src/agent.rs`, add the variant to `StopReason`, after `Stopped(String)`:

```rust
    /// The run's summed `total_tokens` reached the budget set with
    /// [`Agent::token_budget`].
    OverBudget,
```

- [ ] Step 2: Add the field `token_budget: Option<u64>,` to `struct Agent` after `memory`, and `token_budget: None,` to the struct literal in `Agent::new` after `memory: None,`.

- [ ] Step 3: Add this builder right after `pub fn max_turns`:

```rust
    /// Stops a run once the tokens its model calls report add up to
    /// `tokens`.
    ///
    /// Checked after each model call, against the sum of every call's
    /// `total_tokens` so far in this run. When it is reached, the run ends
    /// with [`StopReason::OverBudget`], no requested tool runs, and the
    /// answer is that call's text. A call that reports no usage counts as
    /// zero. Each run starts from zero, so a conversation's second message
    /// has the whole budget again. Unset, a run has no budget.
    pub fn token_budget(mut self, tokens: u64) -> Self {
        self.token_budget = Some(tokens);
        self
    }
```

- [ ] Step 4: In `run_loop`, directly after the block that runs the `after_model` hooks (the `if let Some(reason) = crate::hook::first_stop(&self.hooks, |hook| { hook.after_model(&response, elapsed, cx) }) { ... break; }` block) and before `if let Some(text) = refusal {`, insert:

```rust
            if self
                .token_budget
                .is_some_and(|budget| usage.total_tokens >= budget)
            {
                answer = response.output_text();
                stop = StopReason::OverBudget;
                break;
            }
```

- [ ] Step 5: In `tests/agent.rs`, add `GenerateResponse`, `OutputContent`, `ResponseStatus` and `Usage` to the `use freyja::{...}` list if absent, add `use freyja::testing::ScriptedModel;` and `use serde_json::json;` if absent, and append these tests at the end of the file:

```rust
fn used(total: u64, status: ResponseStatus, content: Vec<OutputContent>) -> GenerateResponse {
    GenerateResponse {
        id: "scripted".into(),
        model: "scripted".into(),
        status,
        content,
        usage: Some(Usage {
            input_tokens: total,
            output_tokens: 0,
            total_tokens: total,
        }),
        provider_metadata: None,
    }
}

fn call(name: &str, total: u64) -> GenerateResponse {
    used(
        total,
        ResponseStatus::RequiresAction,
        vec![OutputContent::ToolCall {
            id: "call_1".into(),
            name: name.into(),
            arguments: json!({"a": 1, "b": 2}).to_string(),
        }],
    )
}

fn answer(total: u64) -> GenerateResponse {
    used(
        total,
        ResponseStatus::Completed,
        vec![OutputContent::Text("3".into())],
    )
}

#[tokio::test]
async fn a_run_without_a_budget_answers() {
    let model = ScriptedModel::new().respond(call("add", 500)).respond(answer(500));
    let run = Agent::new(model).tool(add).ask("add").await.unwrap();
    assert_eq!(run.stop, StopReason::Answered);
}

#[tokio::test]
async fn a_run_over_budget_stops_before_its_tools_run() {
    let calls = Arc::new(AtomicUsize::new(0));
    let model = ScriptedModel::new().respond(call("counter", 150)).respond(answer(10));
    let run = Agent::new(model)
        .tool(Counter { calls: calls.clone() })
        .token_budget(100)
        .ask("add")
        .await
        .unwrap();
    assert_eq!(run.stop, StopReason::OverBudget);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(run.turns, 1);
    assert_eq!(run.usage.total_tokens, 150);
}

#[tokio::test]
async fn a_run_under_budget_answers() {
    let model = ScriptedModel::new().respond(call("add", 10)).respond(answer(10));
    let run = Agent::new(model)
        .tool(add)
        .token_budget(100)
        .ask("add")
        .await
        .unwrap();
    assert_eq!(run.stop, StopReason::Answered);
    assert_eq!(run.answer, "3");
}

#[tokio::test]
async fn a_call_without_usage_counts_as_zero() {
    let model = ScriptedModel::new().tool_call("add", json!({"a": 1, "b": 2})).text("3");
    let run = Agent::new(model)
        .tool(add)
        .token_budget(1)
        .ask("add")
        .await
        .unwrap();
    assert_eq!(run.stop, StopReason::Answered);
}

#[tokio::test]
async fn the_budget_starts_again_for_each_run() {
    let model = ScriptedModel::new().respond(answer(60)).respond(answer(60));
    let agent = Agent::new(model).token_budget(100);
    let mut chat = agent.conversation(InMemoryStorage::new());
    assert_eq!(chat.send("one").await.unwrap().stop, StopReason::Answered);
    assert_eq!(chat.send("two").await.unwrap().stop, StopReason::Answered);
}

#[tokio::test]
async fn a_streaming_run_honours_the_budget() {
    let model = ScriptedModel::new().respond(call("add", 150)).respond(answer(10));
    let agent = Agent::new(model).tool(add).token_budget(100);
    let mut chat = agent.conversation(InMemoryStorage::new());
    let run = chat.send_streaming("add", |_event| {}).await.unwrap();
    assert_eq!(run.stop, StopReason::OverBudget);
}
```

`Counter` is the counting tool already defined in `tests/agent.rs`, named `counter`, with the field `calls: Arc<AtomicUsize>`. `add` is the `#[tool]` function already defined there.

- [ ] Step 6: Run `cargo --offline fmt`, then `cargo --offline test --all-features` and `cargo --offline clippy --all-features --all-targets -- -D warnings`.
- [ ] Step 7: Commit `src/agent.rs` and `tests/agent.rs` as "Agent::token_budget stops a run that has used enough tokens".

### Task 2: Documentation → verify: `RUSTDOCFLAGS='-D warnings' cargo --offline doc --no-deps --all-features` exits 0 and `grep -l token_budget docs/building-an-agent.md CHANGELOG.md README.md` lists all three files

**Blocked by:** Task 1

**Files:**
- Modify: `docs/building-an-agent.md`
- Modify: `CHANGELOG.md`
- Modify: `README.md`

- [ ] Step 1: In `docs/building-an-agent.md`, in the paragraph that starts "`run.stop` tells you why the loop ended", replace "`Failed`, or `Stopped` by a hook" with "`Failed`, `Stopped` by a hook, or `OverBudget`". Then add after that paragraph, as its own paragraph: "`.token_budget(n)` on the builder bounds what one run spends. After each model call the agent adds that call's `total_tokens` to the run's sum, and once the sum reaches `n` the run ends with `StopReason::OverBudget` before any requested tool runs. A provider that reports no usage counts as zero, and each run, including each message of a conversation, starts from zero. `run.usage` holds the sum either way, so the cost of a run is `run.usage` times your provider's prices."
- [ ] Step 2: In `CHANGELOG.md`, under `## Unreleased` and its existing `### Added` heading, append the bullet: "- **`Agent::token_budget`, which ends a run once its model calls have used that many tokens.** The run stops with the new `StopReason::OverBudget` after the call that reached the budget, before any tool it requested runs. Each run starts from zero."
- [ ] Step 3: In `README.md`, in the line starting "**Phase 5, observability and release.**", replace "Spans, cost accounting, and record and replay" with "`Agent::token_budget` ends a run once it has used a given number of tokens. Spans, money cost accounting, and record and replay".
- [ ] Step 4: Run `RUSTDOCFLAGS='-D warnings' cargo --offline doc --no-deps --all-features`.
- [ ] Step 5: Commit the three files as "Document the token budget".
