---
title: token-budget
date: 2026-11-28
status: approved
---

# token-budget: Design

## Problem

A run that loops calls the model until `max_turns`, and nothing bounds what those calls cost. `Run::usage` already sums the tokens of every call, but only after the run ends. A `Hook` cannot bound it either, because one hook value serves every run of an agent and has no per-run state, so a counter in a hook adds up across runs.

## Goals

1. `Agent::token_budget(tokens: u64)` sets a per-run token budget. Unset, a run has no budget. Criterion: `cargo --offline test` passes a test in `tests/agent.rs` that runs an agent with no budget over a script whose responses report usage and ends with `StopReason::Answered`.
2. After each model call, when the run's summed `total_tokens` is at or above the budget, the run stops with `StopReason::OverBudget`. No requested tool runs, and `answer` is that call's output text. Criterion: a test scripts a tool call that reports usage over the budget, then an answer, and checks the stop reason, that the tool ran zero times, that `run.turns` is 1, and that `run.usage.total_tokens` is the scripted value.
3. A run under budget is unchanged. Criterion: a test with a budget above the scripted total ends with `StopReason::Answered` and the scripted answer.
4. A call that reports no usage counts as zero tokens. Criterion: a test with a budget of 1 over responses with `usage: None` ends with `StopReason::Answered`.
5. The budget is per run. Criterion: a test sends two messages through one `Conversation` whose calls each stay under the budget but together exceed it, and both runs end with `StopReason::Answered`.
6. Streaming honours the budget the same way. Criterion: a test runs `Conversation::send_streaming` over the script from goal 2 and checks `StopReason::OverBudget`.
7. Documented. `docs/building-an-agent.md` names `token_budget` and `OverBudget`, the `StopReason` list there includes `OverBudget`, `CHANGELOG.md` gets an Added line under Unreleased, and the README Phase 5 line says a token budget ships. Criterion: `RUSTDOCFLAGS='-D warnings' cargo --offline doc --no-deps --all-features` passes and each of those three files mentions `token_budget`.

## Non-goals

- Converting tokens to money, or a price table.
- A budget spanning several runs or a whole conversation.
- Stopping mid-stream, before a call finishes.
- Estimating tokens for a provider that reports no usage.

## Constraints

- Rust 2024, MSRV 1.88.
- No new dependency, normal or dev.
- No live LLM calls.
- `StopReason` stays `#[non_exhaustive]`.
- Run limits: at most 3 verify rounds, at most $0 on paid runs.

## Approach

Pushback: the request was a running total of tokens per run, and `Run::usage` already is one. Decided to build what is missing instead, a budget that stops a run once it has used enough tokens, because that is the part a user cannot write today.

`Agent` gets a field `token_budget: Option<u64>` and a builder `token_budget(self, tokens: u64) -> Self`. In `run_loop`, right after the `after_model` hooks have run for a call and none stopped the run, if a budget is set and `usage.total_tokens >= budget`, the loop sets `answer = response.output_text()` and `stop = StopReason::OverBudget` and breaks. The check runs after the hooks so that `Tracing` still reports the call that spent the budget, and a hook's stop takes precedence. `StopReason` gains the unit variant `OverBudget`. `total_tokens` is the measure because every dialect fills it.

## Alternatives considered

- A `Budget` hook. A hook value is shared by every run, so its counter would need resetting at run start, which `Hook` cannot see.
- A money budget with per-model prices. Prices differ by provider and change often, and the cost of a run is one multiplication over `run.usage`.

## Testing

Tests in `tests/agent.rs` drive `Agent` over `ScriptedModel`, using `ScriptedModel::respond` with a `GenerateResponse` whose `usage` is set, and a tool that counts its calls.

## Open questions

N/A: none left after the decisions above.
