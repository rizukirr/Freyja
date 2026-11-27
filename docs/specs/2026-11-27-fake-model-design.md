---
title: fake-model
date: 2026-11-27
status: approved
---

# fake-model: Design

## Problem

Testing an agent, a tool, a hook or a memory against a real `Agent` loop needs a model. A real one costs money, needs a key and a network, and answers differently each time. Today every test writes its own scripted `Model`: tests/model.rs and tests/hook.rs each carry a copy, and a crate outside Freyja has to write a third.

## Goals

1. `freyja::testing::ScriptedModel` implements `Model` and hands out scripted responses in the order they were added. Criterion: a test in `tests/scripted.rs` drives `Agent::ask` through a tool call and a final answer with it, and the run's final text and tool result are the scripted ones.
2. Builders cover the common shapes. `ScriptedModel::new()` starts an empty script, `.text(s)` adds a completed response holding the text `s`, `.tool_call(name, arguments)` adds a `RequiresAction` response holding one tool call with arguments given as a `serde_json::Value` and an id unique within the script, and `.respond(response)` adds any `GenerateResponse` as given. Criterion: a test per builder checks the response the model returns for it.
3. Every request is recorded. `ScriptedModel::requests()` returns a clone of each `GenerateRequest` received, oldest first. Criterion: a test, reading through an `Arc<ScriptedModel>` after the agent run, sees one request per model call, and the second request carries the tool result.
4. A script that runs out returns `Error::InvalidResponse` whose message says the script ran out. Criterion: a test calls `generate` on an empty script and matches that error.
5. Streaming works through the default `Model::stream`. Criterion: a test runs the same tool call and answer through the agent's streaming run and gets the same final text.
6. Documented. `docs/extending.md` gets a section on testing with `ScriptedModel`, `CHANGELOG.md` gets an Added line, and the README's Phase 5 line says a scripted model ships. Criterion: `cargo doc --no-deps` with `RUSTDOCFLAGS=-D warnings` passes and each file mentions `ScriptedModel`.

## Non-goals

- A model that chooses its reply from the request, such as a closure. `respond` and a hand-written `Model` cover that.
- Simulated latency, failures partway through a stream, or token counts beyond what `respond` carries.
- Moving tests/model.rs or tests/hook.rs onto `ScriptedModel`.
- A cargo feature gate. `freyja::testing` is always compiled today.

## Constraints

- Rust 2024, MSRV 1.88. No new dependency.
- Public API additions only. No existing item changes.
- No live LLM calls.
- Run limits: at most 3 verify rounds, at most $0 on paid runs.

## Approach

Pushback: a test can write a scripted model in about 30 lines, so shipping one is optional. Decided to ship it, because two copies already exist in this repo and every outside crate would write another.

`ScriptedModel` holds a `Mutex<VecDeque<GenerateResponse>>`, a `Mutex<Vec<GenerateRequest>>` and a counter for tool call ids. Builders take `self` and return `Self`. `generate` records a clone of the request, then pops the front response or returns `Error::InvalidResponse` with endpoint `scripted`. `name` returns `scripted`. `stream` keeps the default. A test that needs `requests()` after handing the model to an agent wraps it in `Arc`, which already implements `Model`.

## Alternatives considered

- A closure model, `FnModel(|request| response)`. More flexible, but every test then writes its own sequencing, and the queue covers every test in the repo.
- Only documenting how to write one. Costs nothing to build, but leaves every crate writing the same 30 lines.

## Testing

`tests/scripted.rs` holds the tests named in the goals, using only the public API and no network.

## Open questions

N/A: none left after the decisions above.
