---
title: tracing-hook
date: 2026-11-27
status: approved
---

# tracing-hook: Design

## Problem

An agent run is invisible from outside: when it loops, picks the wrong tool, or is slow or expensive, nothing reports which call did what. The `Hook` trait lets a user write that reporting, but every user would write the same thing, and most Rust applications already route diagnostics through the `tracing` crate.

## Goals

1. Behind an optional cargo feature named `tracing`, off by default, `freyja::Tracing` implements `Hook`. Criterion: `cargo --offline build` with no features does not compile the `tracing` crate as a direct dependency of freyja (`cargo --offline tree -e normal --depth 1` lists no `tracing`), and `cargo --offline build --features tracing` succeeds.
2. After each model call, `Tracing` emits one `INFO` event with target `freyja` and message `model call`, with fields `model` (the response's model), `status`, `elapsed_ms`, and `input_tokens`, `output_tokens` and `total_tokens` when the response reports usage. Criterion: a test in `tests/tracing.rs` runs an agent over `ScriptedModel` under a recording subscriber and finds one such event per model call with those fields.
3. After each tool call, including one the guard refused, `Tracing` emits one `INFO` event with target `freyja` and message `tool call`, with fields `tool`, `elapsed_ms` and `output_bytes`. Criterion: the same test finds one such event per tool call with those fields.
4. By default no event carries message text, tool arguments or tool output. `Tracing::new().with_content()` adds `text` to the model event and `arguments` and `output` to the tool event. Criterion: a test checks that no field named `text`, `arguments` or `output` appears by default, and that each appears with the scripted values after `with_content()`.
5. `Tracing` never changes a run. Every method returns `Flow::Continue`. Criterion: the test's run ends with `StopReason::Answered` and the scripted answer.
6. Documented as a worked example. `docs/building-an-agent.md` shows enabling the feature and `.hook(Tracing::new())`, and points to `src/tracing_hook.rs` as an example of a real hook. `CHANGELOG.md` gets an Added line under Unreleased, and the README's Phase 5 line says the tracing hook ships. Criterion: `RUSTDOCFLAGS='-D warnings' cargo --offline doc --no-deps --all-features` passes and each of those three files mentions `Tracing`.

## Non-goals

- Spans around a run or a call. Events only.
- A turn number, a run id, or any field from `Context`.
- Events for a model call that failed. `after_model` is not called for one.
- A `tracing-subscriber` dependency or any subscriber setup. The application installs its own.

## Constraints

- Rust 2024, MSRV 1.88.
- `tracing = { version = "0.1.44", optional = true, default-features = false, features = ["std"] }`. No other new dependency, normal or dev.
- No live LLM calls.
- Run limits: at most 3 verify rounds, at most $0 on paid runs.

## Approach

Pushback: a user can write this hook from the docs' Timing example in a few lines, so shipping it is optional. Decided to ship it, because the user asked for it and `tracing` 0.1.44 is already in the dependency tree through hyper and h2, so enabling the feature adds no new crate.

`src/tracing_hook.rs`, compiled under `#[cfg(feature = "tracing")]`, holds `pub struct Tracing { content: bool }` with `new()`, `with_content()`, `Default`, `Debug`, `Clone` and `Copy`. `after_model` and `after_tool` each call `tracing::info!(target: "freyja", ...)`. Usage fields are `Option<u64>`, which `tracing` records only when present. `lib.rs` re-exports `Tracing` under the same cfg.

## Alternatives considered

- A span per run with events inside. Needs the run boundary, which `Hook` does not expose.
- Logging at `DEBUG`. Quieter, but a user who installed the hook wants to see it at the default `INFO` filter.

## Testing

`tests/tracing.rs`, compiled only with the feature, defines a minimal `tracing::Subscriber` that records each event's target, level and fields as strings, installs it with `tracing::subscriber::set_default` inside a single-threaded tokio test, and runs `Agent` over `ScriptedModel` with one tool call and an answer.

## Open questions

N/A: none left after the decisions above.
