---
title: run-span
date: 2026-11-28
status: approved
---

# run-span: Design

## Problem

`freyja::Tracing` reports each model call and tool call as a flat event. When two runs happen at once, their events interleave and nothing ties an event to its run. A `Hook` cannot fix this, because it never sees a run start or end.

## Goals

1. Behind the existing `tracing` feature, every agent run, whether through `Agent::ask`, `Conversation::send` or `Conversation::send_streaming`, runs inside one `tracing` span named `run`, with target `freyja` and level `INFO`. Criterion: a test in `tests/run_span.rs` records spans under a hand-written subscriber and finds exactly one span named `run` with that target and level after one `Agent::ask`.
2. Every event the `Tracing` hook emits during a run has that run's span as its current span. Criterion: the same test finds the `model call` and `tool call` events recorded with the `run` span's id as the innermost entered span.
3. When the run returns `Ok`, the span records `stop` (the `StopReason` as Debug), `turns`, `input_tokens`, `output_tokens` and `total_tokens`. Criterion: the test reads those fields from the span with the scripted values.
4. When the run returns `Err`, the span records `error` (the error's Display). Criterion: a test runs `Agent::ask` over a `ScriptedModel` with no responses and finds the `error` field on the span.
5. Two runs get two spans. Criterion: a test sends two messages through one `Conversation` and finds two `run` spans with different ids, each holding the events of its own run.
6. Streaming runs are wrapped too. Criterion: a test runs `Conversation::send_streaming` and finds one `run` span holding its events.
7. Without the feature nothing changes. Criterion: `cargo --offline test` with default features passes, and `cargo --offline tree -e normal --depth 1` lists no `tracing`.
8. Documented. `docs/building-an-agent.md` says each run is a `run` span and names its fields, `CHANGELOG.md` gets an Added line under Unreleased, and the README Phase 5 line says spans ship. Criterion: `RUSTDOCFLAGS='-D warnings' cargo --offline doc --no-deps --all-features` passes and each of those three files contains `run` span wording found by `grep -l "span" docs/building-an-agent.md CHANGELOG.md README.md`, listing all three.

## Non-goals

- A span per model call or per tool call.
- A conversation id, agent name, or any other identity field on the span.
- A `tracing-subscriber` dependency.
- Changing the events the `Tracing` hook emits.

## Constraints

- Rust 2024, MSRV 1.88.
- No new dependency, normal or dev. `tracing` stays `default-features = false, features = ["std"]`.
- No live LLM calls.
- Run limits: at most 3 verify rounds, at most $0 on paid runs.

## Approach

Pushback: no simpler framing. A hook sees calls, not runs, so the span has to come from the agent loop.

`Agent::run_loop` in `src/agent.rs` is the one path every run takes. Its body moves into a private `run_inner` with the same signature. Without the feature, `run_loop` calls `run_inner` directly. With it, `run_loop` creates `tracing::info_span!(target: "freyja", "run", stop = Empty, turns = Empty, input_tokens = Empty, output_tokens = Empty, total_tokens = Empty, error = Empty)`, awaits `run_inner(...).instrument(span.clone())`, then records the result fields on the span before returning. Tool calls are polled inside that future, so their events nest under the span too.

## Alternatives considered

- A span opened in `Agent::ask` and `Conversation::send_with` separately. Two places to keep in step, where `run_loop` is one.
- A per-call span inside the `Tracing` hook. A hook's before and after methods have no shared per-call state to hold the span guard across an await.

## Testing

`tests/run_span.rs`, compiled only with the feature, defines a subscriber that gives each span a fresh id, keeps a stack of entered spans, records span names, metadata and fields (including later `record` calls), and stores each event with the innermost entered span id. Tests install it with `tracing::subscriber::set_default` on `current_thread` tokio tests and drive `Agent` over `ScriptedModel` with `Tracing` installed as a hook.

## Open questions

N/A: none left after the decisions above.
