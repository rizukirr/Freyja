---
title: record-replay
date: 2026-11-28
status: approved
---

# record-replay: Design

## Problem

A test of an agent either calls a real provider, which needs a key, costs money and answers differently each time, or uses `ScriptedModel` with responses written by hand, which may not match what a real model says. Nothing turns a real run into a script a test can replay.

## Goals

1. `freyja::testing::Recorder::new(model)` wraps any `Model`, answers exactly as the wrapped model does, and keeps a copy of every response it returns. Criterion: a test in `tests/record_replay.rs` runs an agent with a tool call over `Recorder::new(ScriptedModel)` and gets the scripted answer.
2. `Recorder::save(path)` writes the responses to a file, and `ScriptedModel::replay(path)` loads it into a script that answers them in order. Criterion: a test saves a recorded run and replays it into a new agent, and the replayed `Run` has the same `answer`, `stop`, `usage` and `turns`.
3. Every response shape survives the file unchanged. Criterion: a test records responses covering every `OutputContent` variant, every `ResponseStatus` including `Other`, usage and `provider_metadata`, and each replayed response equals the recorded one with `==`.
4. A failed call is not recorded. Criterion: a test records a run whose model fails, and the replayed script has no responses.
5. A streaming run is recorded. Criterion: a test records `Conversation::send_streaming` and replays the same answer.
6. `replay` rejects a file that is not a version 1 recording, or holds a line that is not a response, with `std::io::ErrorKind::InvalidData` and a message naming the line. Criterion: tests for a missing header, a newer version and a bad second line check the kind and the line number in the message.
7. `GenerateResponse`, `ResponseStatus`, `OutputContent` and `Usage` implement `Serialize` and `Deserialize`. Criterion: goal 3's test compiles and passes.
8. Documented. `docs/extending.md` shows recording once and replaying in tests, `CHANGELOG.md` gets an Added line under Unreleased, and the README Phase 5 line says record and replay ships. Criterion: `RUSTDOCFLAGS='-D warnings' cargo --offline doc --no-deps --all-features` passes and `grep -l Recorder docs/extending.md CHANGELOG.md README.md` lists all three.

## Non-goals

- Recording or matching requests. Replay answers in order whatever is asked.
- Recording failed calls, or replaying errors.
- Recording a stream event by event. A recorder answers a streaming caller all at once.
- Choosing between recording and replaying from an environment variable.

## Constraints

- Rust 2024, MSRV 1.88.
- No new dependency, normal or dev.
- No live LLM calls.
- Run limits: at most 3 verify rounds, at most $0 on paid runs.

## Approach

Pushback: replay needs no new model. `ScriptedModel` already answers from a queue and keeps the requests, so `ScriptedModel::replay` fills one from a file.

`Recorder<M>` holds the model and a `Mutex<Vec<GenerateResponse>>`. Its `generate` awaits the inner model and pushes a clone of each successful response. Its `stream` is the trait default, so streaming goes through `generate` and is recorded. `save` returns `std::io::Result` because a file write is not a model error. The file is JSON Lines: a header `{"freyja_recording":1}`, then one `GenerateResponse` per line, matching the versioned header pattern of `JsonlStorage`. The response types get serde derives, externally tagged like `InputContent`.

## Alternatives considered

- Writing each response to the file as it arrives. A write failure would have to surface through `generate` as an `Error`, and no variant describes it.
- Private wire types in `testing.rs` in place of public derives. Duplicates four types to avoid two additive trait impls.

## Testing

`tests/record_replay.rs` uses `ScriptedModel` as the recorded model and files under `std::env::temp_dir()`, as `tests/jsonl.rs` does.

## Open questions

N/A: none left after the decisions above.
