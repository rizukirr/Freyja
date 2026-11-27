---
title: hooks
date: 2026-11-27
status: approved
---

# hooks: Design

## Problem

Nothing outside the agent loop can see each model call and each tool call, or end a run between them. `Agent::guard` refuses one tool call, and `send_streaming` reports events for one call site, but logging, tracing, cost accounting and budgets need something set once on the agent that sees every conversation. Without it, each of those is rebuilt by every user, and a crate such as an OpenTelemetry bridge has nothing to implement.

## Goals

1. **A `Hook` trait set with `Agent::hook`.** It is called before and after every model call and before and after every tool call, for `send` and `send_streaming` alike. Criterion: a recording hook over a scripted model, on a run with one tool round trip, sees in order: before model, after model, before tool, after tool, before model, after model. The tool hook calls carry the tool's name and arguments, the after-tool call carries the output, and each after call carries an elapsed `Duration`. The same sequence is seen through `send_streaming`.
2. **A hook can stop the run.** Each hook method returns `Flow`, either `Flow::Continue` or `Flow::Stop(reason)`. A stopped run returns `Ok(Run)` with `StopReason::Stopped(reason)`. Criteria: a hook stopping in `after_model` on the first turn leaves the second scripted response unused and the run's `turns` at one. A hook stopping in `before_model` makes no model call. A hook stopping in `before_tool` keeps that tool from running, the model's transcript records `stopped: <reason>` as its result, and no further model call is made.
5. **Audit sees refused calls.** `after_tool` also fires for a call the guard refused, with the output `denied: <reason>`. `before_tool` fires only for calls the guard allowed. Criterion: a guard denying `add` and a recording hook, over the tool round trip, record no `before_tool` and one `after_tool` whose output starts with `denied:`.
6. **The trust a hook carries is documented.** Criterion: `docs/extending.md` says a hook sees all conversation content and the run's `Context` but not API keys, that a third-party hook is trusted like any dependency, that model text a hook logs should be escaped because the model controls it, and that a hook which panics ends the run.
3. **Hooks compose.** Several hooks fire in the order they were added, every one of them is called at each point, and the first `Stop` returned is the one reported. Criterion: two recording hooks both see the full sequence from goal 1, and when both stop, the reason reported is the first hook's.
4. **The contract page covers it.** `docs/extending.md` lists `Hook`, its rules and an example crate idea. `CHANGELOG.md` records `Hook`, `Flow`, `Agent::hook` and the `StopReason` change. Criterion: `docs/extending.md` names `Hook`, and `tests/public_api.rs` checks a type implementing `Hook` is accepted by `Agent::hook`.

## Non-goals

- Rewriting requests or responses from a hook. A `Model` wrapper does that.
- Async hook methods. A hook that must do I/O sends to a channel or spawns.
- Hooks on storage or memory operations.
- A shipped tracing or OpenTelemetry hook. That is Phase 5.
- Any live call to a paid model.

## Constraints

- No new dependency.
- Rust 1.88, edition 2024, the MSRV CI checks.
- With no hook set, the agent loop, the requests it sends, tool dispatch, the guard and the transcript behave exactly as today.
- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-features` and `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features` all pass.
- Public API changes are recorded in `CHANGELOG.md` under Unreleased.
- Any call to a real LLM server uses as few tokens as possible: send "hi" and have the model answer only "hi". This run makes none.

Run limits: at most 3 verify rounds, at most $0 on paid runs

## Approach

Pushback outcome: the user chose watch plus stop, the middle of three options (watch only, watch plus stop, watch plus rewrite). It covers logging, tracing, cost, budgets and audit, and leaves rewriting to `Model` wrappers.

- **`Hook`** (`src/hook.rs`, new). A `Send + Sync` trait with four methods, each defaulted to return `Flow::Continue`, so a hook implements only what it needs:
  - `before_model(&self, request: &GenerateRequest, cx: &Context) -> Flow`
  - `after_model(&self, response: &GenerateResponse, elapsed: Duration, cx: &Context) -> Flow`
  - `before_tool(&self, name: &str, arguments: &str, cx: &Context) -> Flow`
  - `after_tool(&self, name: &str, arguments: &str, output: &str, elapsed: Duration, cx: &Context) -> Flow`
  Synchronous, because a hook runs inline in the loop and must not hold it up. The `Context` lets a hook tell users or tenants apart.
- **`Flow`** (`src/hook.rs`). `enum Flow { Continue, Stop(String) }`.
- **`StopReason::Stopped(String)`**. `StopReason` loses `Copy` to carry the reason and becomes `#[non_exhaustive]`. Comparisons with `==` keep working.
- **`Agent::hook(impl Hook + 'static)`** pushes onto a `Vec<Arc<dyn Hook>>`.
- **Loop wiring** (`src/agent.rs`). Before each model call, every hook's `before_model` runs, and a stop ends the run before the call. After it, every `after_model` runs with the elapsed time, after the response joins the transcript, and a stop ends the run there, with the response's text as the answer. In `dispatch`, a call the guard refuses still reaches every `after_tool`, with its `denied:` output and no `before_tool`. Every `before_tool` runs after the guard allows the call, and a stop replaces the call with the output `stopped: <reason>`. Every `after_tool` runs with the output and elapsed time. A stop from either tool hook ends the run once the turn's tool results are in the transcript, so no call is left without a result.
- **Transcript after a stop.** A stop in `before_model` leaves the user's turn with no reply. A stop in `after_model` on a response requesting tools leaves those calls unanswered, which the existing repair pass handles on the next send. Both are documented on `Flow::Stop`.

## Alternatives considered

- **Async hook methods.** Lets a hook await a database write, but every call boxes a future in the hot loop, and a slow hook stalls every run. A hook that needs I/O hands it to a channel.
- **Stopping as an `Err`.** A budget stop is an expected outcome, and callers already branch on `StopReason`. An error would also discard the run's usage, which is what a budget hook needs reported.

## Testing

- `tests/hook.rs`, new, over a scripted model (no server): the order test through `send` and `send_streaming` (goal 1), the three stop points (goal 2), two hooks (goal 3).
- Every existing test passes untouched except for any exhaustive `match` on `StopReason`, which gains the new variant.
- `tests/public_api.rs`: `Agent::hook` accepts a user type.

## Open questions

None.
