---
title: model
date: 2026-11-27
status: approved
---

# model: Design

## Problem

`Agent` and `Summarizer` hold a concrete `Client` and call `Client::generate` and `Client::stream` directly (`src/agent.rs`, `src/summarize.rs`). A crate outside Freyja cannot put anything else in that seat: not a provider Freyja has no dialect for, not a cache or a router wrapped around a `Client`, and not a fake model that lets an agent be tested without a local HTTP server. `Storage`, `Memory`, `Tool` and `TokenCounter` are already traits a crate can implement. The model is the one part of an agent that is not.

## Goals

1. **A `Model` trait the agent and the summarizer call.** `Agent::new` and `Summarizer::new` accept any `impl Model + 'static`, and `Client` implements `Model`. Criterion: every existing call site that passes a `Client` compiles unchanged, and the full test suite passes.
2. **A model written outside Freyja runs an agent with no server.** Criterion: a `Model` implemented in `tests/` that returns scripted responses drives `Conversation::send` through one tool call round trip to a final answer, and the test starts no listener.
3. **Streaming works with any model.** `Model::stream` has a default that replays the finished `generate` response as events. Criterion: `send_streaming` over the scripted model from goal 2 hands the sink a `StreamEvent::Done` and the same answer `send` returns.
4. **Models compose.** `Arc<T>` and `Box<T>` forward `Model` to `T`, so one model value can be shared and wrapped. Criterion: a test wrapper that counts calls around the scripted model, shared by an agent and a summarizer through `Arc`, counts both callers' requests.
5. **Errors name the model.** What `Agent` reports for a storage or memory failure names `Model::name()`, as it names the endpoint today. Criterion: a storage failure under the scripted model yields an `Error::InvalidRequest` whose `endpoint` is the scripted model's name.
6. **The contract page covers it.** `docs/extending.md` lists `Model`, its rules, and an example crate idea. `CHANGELOG.md` and the README roadmap record it. Criterion: `docs/extending.md` names `Model`, and `tests/public_api.rs` checks `Client: Model`.

## Non-goals

- A mock or scripted model shipped in the crate. That is the Phase 5 mock provider.
- Incremental streaming from a custom model. A model whose provider streams can still only replay through the default for now. A constructor taking a stream of events can be added later without a break.
- `Client::generate_as` and `Client::check` on the trait. They stay on `Client`.
- Hooks, the `send` builder, retrieval. Later steps.
- Any live call to a paid model.

## Constraints

- No new dependency.
- Rust 1.88, edition 2024, the MSRV CI checks.
- Every trait keeps boxed futures so it stays usable as `dyn`.
- The agent loop, pinned turns, the repair pass, tool dispatch and the request built for each turn behave exactly as today.
- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-features` and `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features` all pass.
- Public API changes are recorded in `CHANGELOG.md` under Unreleased.
- Any call to a real LLM server uses as few tokens as possible: send "hi" and have the model answer only "hi". This run makes none.

Run limits: at most 3 verify rounds, at most $0 on paid runs

## Approach

Pushback outcome: no simpler framing. The user asked for this to be extensible by other crates, which a trait is the minimum for. The trait is kept to one required method (`generate`) plus `name`, with `stream` defaulted, which is the smallest contract that serves both the agent's paths.

### Units

- **`Model` trait** (`src/provider.rs`, new, since `src/model/` already holds the message and request types). `fn name(&self) -> &str`, `fn generate<'a>(&'a self, request: &'a GenerateRequest) -> ModelFuture<'a, GenerateResponse>`, and a defaulted `fn stream<'a>(&'a self, request: &'a GenerateRequest) -> ModelFuture<'a, EventStream>` that awaits `generate` and returns `EventStream::from_response`. `ModelFuture<'a, T>` is `Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'a>>`. The trait is `Send + Sync`. Errors are `freyja::Error`, so a model reports failure in the same type every caller already handles.
- **`EventStream::from_response(GenerateResponse)`** (`src/stream/event.rs`). Public. Builds a stream whose events are the response's content in order (text as one `TextDelta`, refusal as one `RefusalDelta`, each tool call, reasoning) then `Done`, and whose `into_response` returns the response. Implemented as a third `Body` state holding queued events and the finished response.
- **`impl Model for Client`** forwards to the existing inherent `generate` and `stream`, with `name` returning the endpoint's configured name.
- **Forwarding impls** for `Arc<T>` and `Box<T>`, `T: Model + ?Sized`.
- **`Agent`** holds `Arc<dyn Model>` in place of `Client`. `Agent::new(model: impl Model + 'static)`. The three call sites (`generate`, `stream`, the endpoint name in `storage_error`) go through the trait.
- **`Summarizer`** holds `Arc<dyn Model>`. `Summarizer::new(model: impl Model + 'static)`. Its `Debug` is written by hand and prints the model's name.
- **Docs.** `docs/extending.md`, `docs/reference/` pages that show `Agent::new(client)` stay valid and gain one sentence where the agent is introduced. `CHANGELOG.md`, README roadmap.

### Data flow

Unchanged except at one point: where the loop called `self.client.generate(&request)` or `self.client.stream(request)`, it calls `self.model.generate(&request)` or `self.model.stream(&request)`.

### Errors

A model returns `freyja::Error`. `Client`'s errors are unchanged. A custom model picks the variant that fits, such as `Error::InvalidResponse` or `Error::Api`.

## Alternatives considered

- **A generic `Agent<M: Model>`.** No boxing and no dynamic dispatch on a call that already costs a network round trip. But `M` spreads into `Conversation<S>`, `Summarize` and every signature a user writes, and two agents with different models no longer share a type. The cost it saves is negligible next to an HTTP request.
- **A trait over the dialect, not the model.** Lets a crate add a wire format while reusing `Client`'s transport. It serves new providers but not caches, routers or fakes, and exposes the dialect internals as public contract.

## Testing

- The whole existing suite passes with `Client` passed where it was before (goal 1).
- `tests/model.rs`, new: a scripted `Model` in the test file, a tool round trip with `send` (goal 2), the same with `send_streaming` (goal 3), an `Arc` counting wrapper shared by an agent and a summarizer (goal 4), a failing storage naming the model (goal 5).
- Unit test for `EventStream::from_response`: events in order, then `into_response` equal to the input.
- `tests/public_api.rs`: `Client: Model`, `Arc<Client>: Model`.

## Open questions

None.
