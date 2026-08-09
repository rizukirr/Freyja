# Verification Report — Streaming API

**Date:** 2026-08-09
**Spec:** `docs/specs/2026-08-09-streaming-api-design.md`
**Plan:** `docs/plans/2026-08-09-streaming-api.md`
**Commit verified:** `1bde629` (branch `vibe/streaming-api`, base `1c3d5b0`)

**Verification depth:** critical-requirements-only, chosen by the user. Eight
critical requirements received three independent passes; nine mechanically
checkable requirements received a single pass and are marked as such. One
deviation from the skill's dispatch shape is disclosed under Method below.

## Method

Each of the three critical passes ran as a **fresh, read-only subagent with no
shared context**, so no pass could anchor another. Pass 3 was additionally
briefed to be adversarial — to assume the implementation is subtly wrong and try
to prove it.

**Disclosed deviation:** the skill specifies one dispatch per requirement per
pass (24 dispatches for 8 critical requirements). This run instead used one
dispatch per *pass*, each judging all 8 requirements (3 dispatches). Cross-pass
independence — the anchoring risk the three-pass design exists to defeat — is
fully preserved. Requirement-level isolation within a single pass is not: an
agent judging CR5 could in principle have been influenced by its own reading of
CR6. The trade was made to keep the run affordable after the user selected
critical-only. It did not suppress disagreement: the passes disagreed on CR5 and
converged on `partial` for CR6, which is the outcome this design is meant to
surface.

## Repo-level checks

All run at commit `1bde629` with a clean working tree.

- **Formatting:** `cargo fmt --all --check` → exit 0
  ```
  (no output)
  ```
- **Tests:** `cargo test --all-features` → exit 0
  ```
  test result: ok. 81 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.08s
  test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
  ```
- **Doctests:** `cargo test --doc` → exit 0
  ```
  test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
  ```
- **Linter:** `cargo clippy --all-targets --all-features -- -D warnings` → exit 0
  ```
      Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.09s
  ```
- **Release build:** `cargo build --release --all-targets` → exit 0
  ```
      Finished `release` profile [optimized] target(s) in 42.85s
  ```
- **MSRV:** `cargo +1.88 check --all-targets` → exit 0
  ```
      Checking freyja v0.1.0 (/home/rizukirr/Projects/freya/.vibe-worktrees/2026-08-09-streaming-api)
      Finished `dev` profile [unoptimized + debuginfo] target(s) in 17.74s
  ```
- **Packaging:** `cargo publish --dry-run` → exit 0
  ```
     Packaged 53 files, 559.0KiB (143.1KiB compressed)
  warning: aborting upload due to dry run
  ```
- **`git status --porcelain`:**
  ```
  (empty)
  ```
- **Surgical-diff pass:** `clean` — zero orphans. Every changed file traces to a
  plan task, to the isolate step (`.gitignore`), to a disclosed plan repair, or
  to Task 14's `cargo fmt --all`.

**Every repo-level check passes.** The blockers below are requirement-level.

## Requirements

### Satisfied — three-pass unanimous

**CR1.** "A provider-neutral `StreamEvent` enum covering text, tool calls, reasoning, and a terminal event with usage and finish reason."
- Passes: yes / yes / yes → **satisfied**
- Evidence: `src/provider/stream.rs:16-47` defines `#[non_exhaustive] StreamEvent`
  with `TextDelta`, `ToolCall{id,name,arguments}`, `ReasoningDelta`,
  `Reasoning{data}`, `Done{id,model,status,usage}`. Pass 3 confirmed usage is
  mapped per dialect with its own field names (Chat `prompt_tokens`/
  `completion_tokens`; Responses `input_tokens`/`output_tokens`; Anthropic
  `input_tokens` from `message_start` combined with `output_tokens` from
  `message_delta`; Gemini `total_input_tokens`/`total_output_tokens`).

**CR2.** "All four dialects: OpenAiChat, OpenAiResponses, Anthropic, Gemini."
- Passes: yes / yes / yes → **satisfied**
- Evidence: four real `StreamDecoder` impls, none stubbed; `Client::stream`
  (`src/provider/mod.rs:417-455`) dispatches to each.
  ```
  test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 67 filtered out; finished in 0.00s
  ```
  Pass 2 confirmed each test encodes that dialect's distinct semantics: Anthropic's
  slot 1 behind prose, Chat's absent end frame, Responses' `ToolReplace`, Gemini's
  `event_type` read from the payload body.

**CR3.** "Zero new dependencies. Cargo.toml's dependency list is unchanged."
- Passes: yes / yes / yes → **satisfied**
- Evidence: `git diff 1c3d5b0..HEAD -- Cargo.toml Cargo.lock` is empty.
  Dependencies remain `reqwest {features=["json"]}`, `serde`, `serde_json`.
  Bytes are read via the inherent `reqwest::Response::chunk()`, so the `stream`
  feature is never needed.

**CR4.** "Tool-call arguments arrive fully assembled. Callers never stitch JSON."
- Passes: yes / yes / yes → **satisfied**
- Evidence: `Assembler` buffers `ToolArgs` into `PendingCall` and emits
  `StreamEvent::ToolCall` only at `ToolEnd` or `close`.
  ```
  assembler_assembles_fragmented_arguments             ok. 1 passed
  assembler_keeps_concurrent_calls_apart               ok. 1 passed
  assembler_flushes_unended_calls                      ok. 1 passed
  ```
- Pass 3 noted one residual risk, not a defect against this spec: an
  OpenAI-*compatible* server that repeats `id` on every tool-call chunk would
  reset the buffer, because the decoder treats a present `id` as a call start.
  No conforming implementation does this. Recorded, not blocking.

**CR7.** "`generate()`'s serialized request bodies must not change, byte for byte."
- Passes: yes / yes / yes → **satisfied**
- Evidence: each dialect's new `stream` (and Chat's `stream_options`) field is
  declared with `#[serde(skip_serializing_if = "Option::is_none")]` and left
  `None` by `build()`. `git diff` over the four `types.rs` files shows no deleted
  or altered pre-existing assertion, and every pre-existing wire-format test
  still passes. Pass 1 noted no *dedicated* byte-identity regression test was
  added; the pre-existing assertions cover it.

**CR8.** "No fragment-level tool events. Partial arguments are not observable."
- Passes: yes / yes / yes → **satisfied**
- Evidence: `RawDelta`, `StreamDecoder`, `PendingCall`, and `Assembler` are all
  private or `pub(crate)`. Only `EventStream` and `StreamEvent` are re-exported.
  `StreamEvent` has no fragment-carrying variant, and `EventStream` derives no
  `Debug` and exposes no fields.

### Satisfied — single pass (weaker evidence)

These nine received one verdict, not three, per the user's critical-only choice.
A single-pass `no` or `partial` would still have blocked; all returned `yes`.

| Req | Requirement | Evidence |
|---|---|---|
| R-G1 | "`client.stream(&request)` on the existing `Client`, alongside `generate()`." | `src/provider/mod.rs:417`, exported at `src/lib.rs:93` |
| R-C1 | "Rust edition 2024, MSRV 1.88, verified by CI." | `Cargo.toml` `rust-version = "1.88"`; `cargo +1.88 check --all-targets` exit 0; CI msrv job |
| R-C2 | "Dependencies are `reqwest` (features: `json`), `serde`, `serde_json`." | Cargo.toml diff empty; `Response::chunk()` at `stream.rs:362` |
| R-C3 | "The crate exposes `async fn` and spawns nothing." | tokio only in `[dev-dependencies]`; no `spawn` anywhere in `src/` |
| R-C5 | "Transport lives in `Client`; each dialect owns only conversion." | `grep -rln reqwest src/` matches only `provider/mod.rs` and `provider/stream.rs` |
| R-N1 | "No `futures_core::Stream` impl." | inherent `pub async fn next` at `stream.rs:313`; no `futures`/`impl Stream`/`poll_next` in `src/` or `examples/` |
| R-N2 | "No opt-out of accumulation." | one streaming entry point; `Assembler` unconditionally constructed in `EventStream::new` |
| R-N4 | "No retries, backoff, or reconnection." | no retry/backoff/reconnect symbols; `pump_bytes` maps failure straight to `ProviderError::Http` |
| R-N5 | "No streaming tool *results*." | `StreamEvent` has no tool-result variant |

### CR5 — DISAGREEMENT: escalate

**Requirement (verbatim):** "Reasoning models remain usable across turns: the
opaque replayable blob is reachable from the stream."

- Pass 1: **yes** — "`RawDelta::ReasoningBlob` -> `StreamEvent::Reasoning{data}`
  emitted and also captured into `OutputContent::Reasoning`; anthropic/gemini/
  openai_responses decoders all emit it."
- Pass 2: **partial** — "only 'thinking'/'thought'/'reasoning' item types are
  reconstructed field-by-field … any other reasoning-bearing block, e.g.
  Anthropic `redacted_thinking` or an unknown block type, is silently dropped by
  the decoder whereas the non-streaming parsers preserve the whole block
  verbatim (anthropic/types.rs:387, gemini/types.rs:334)."
- Pass 3: **partial** — "Anthropic `redacted_thinking` content blocks are
  silently ignored by the decoder (anthropic/mod.rs content_block_start matches
  only tool_use/thinking) … and Gemini's blob is only `{type,signature}` versus
  the whole step the parser stores."

**Confirmed independently.** `src/provider/anthropic/mod.rs:92-103` matches only
`Some("tool_use")` and `Some("thinking")`, with `_ => {}` discarding the rest.
The non-streaming parser at `src/provider/anthropic/types.rs:387` is a catch-all:

```rust
        _ => vec![OutputContent::Reasoning { data: block }],
```

So a block type the streaming path does not name — `redacted_thinking` is a real
Anthropic block type — is **preserved when calling `generate()` and dropped when
streaming**. A caller who streams such a response and continues the conversation
sends an incomplete transcript, which is the exact failure
`src/provider/model.rs:225-239` warns about.

**Action required:** decide whether the streaming decoders must mirror the
non-streaming catch-all (preserve any unrecognised block as a whole-block blob),
or whether the spec's replay guarantee should be narrowed to the named block
types. This is a design decision, not a mechanical fix.

### CR6 — PARTIAL (unanimous)

**Requirement (verbatim):** "A drained stream converts to the same
`GenerateResponse` that `generate()` would have returned, so a streaming
multi-turn tool loop can reuse the existing `GenerateResponse::to_message`."

- Pass 1: **partial** — "`into_response` always sets `provider_metadata: None`
  whereas every dialect's `generate()` sets `Some(extra)`, and no test compares
  `into_response` output against a real dialect's non-streaming fixture."
- Pass 2: **partial** — same, plus "OpenAiChat refusal deltas are never decoded
  so `OutputContent::Refusal` cannot appear."
- Pass 3: **partial** — same, plus "tool-argument string formatting also differs
  for Gemini/Anthropic, which re-serialize the object."

Three passes agreeing `partial` → status **partial**, which blocks `ready`.

**Confirmed independently.** `src/provider/stream.rs:250`:

```rust
            provider_metadata: None,
```

against `src/provider/openai_responses/types.rs:285`,
`src/provider/anthropic/types.rs:353`, and `src/provider/openai_chat/types.rs:379`:

```rust
            provider_metadata: Some(Value::Object(value.extra)),
```

The spec says "the same `GenerateResponse`". It provably is not the same: one
field always differs. The narrower claim the spec actually needs — that
`to_message()` produces a replayable assistant turn — is likely still true, since
`to_message` ignores `provider_metadata`. But that is not what the spec says, and
the plan's own Testing section promised a test comparing a drained stream against
the equivalent non-streaming fixture. **That test was never written.** The only
drain test uses a synthetic `TestDecoder` and asserts `output_text` and `model`.

**Action required:** choose one — (a) populate `provider_metadata` in
`into_response` and add the missing parity test, (b) narrow the spec's claim from
"the same `GenerateResponse`" to "an equivalent assistant turn", and add a test
asserting *that*, or (c) accept the gap explicitly and document it on
`into_response`.

## Overall verdict

**not ready**

Blockers:

1. **CR5 disagreement (yes / partial / partial).** The Anthropic streaming
   decoder drops unrecognised content blocks that the non-streaming parser
   preserves, so the spec's cross-turn replay guarantee does not hold for block
   types other than `thinking`. Gemini's blob is a reconstructed
   `{type,signature}` rather than the whole step.
2. **CR6 partial (unanimous).** `into_response()` does not return the same
   `GenerateResponse` as `generate()` — `provider_metadata` is always `None`.
   The parity test the plan's Testing section promised was never written.

Neither blocker is a regression: all 81 tests, 10 doctests, clippy, MSRV, and
packaging pass, and the surgical-diff pass is clean. Both are gaps between what
the spec claims and what the implementation delivers.

Suggested next step: amend the spec's CR5 and CR6 claims to match the intended
scope, then run a short follow-up plan implementing whichever of the three CR6
options and the CR5 catch-all the user chooses.

---

# Round 3 — still not ready

**Date:** 2026-08-09
**Commit verified:** `5eb4c19`
**Fix commits since round 1:** `9365b86`, `b4f570b` (round 1), `6d95921` (round 2)

Repo-level checks all still pass: 86 lib tests, 10 doctests, clippy `-D warnings`
clean, `cargo fmt --all --check` clean, working tree clean.

## Verdicts

- **CR5** — yes / partial / yes → **disagreement: escalate**
- **CR6** — partial / no / yes → **disagreement: escalate**

## The pattern, stated plainly

Three rounds have each found the same class of defect: **a streaming decoder
disagrees with its own dialect's non-streaming parser.** Each round fixed the
instances the reviewers named; the next round found more. That is a sign the
fixes have been treating symptoms.

Root cause: the four decoders were written from recorded SSE fixtures and
provider documentation, not derived from the parsers they must agree with.
Nothing in the plan required a field-by-field comparison of decoder against
parser, so every divergence had to be discovered one at a time by review.

## Confirmed remaining defects

All four verified directly against the source, not taken from a reviewer's report.

**1. Status mapping omits `requires_action` — Gemini and OpenAiResponses.**
Both parsers map it (`gemini/types.rs:280`, `openai_responses/types.rs:342`):

```rust
                "requires_action" => ResponseStatus::RequiresAction,
```

Neither decoder does — `grep requires_action` over `gemini/mod.rs` and
`openai_responses/mod.rs` returns nothing. A tool-calling turn therefore streams
as `ResponseStatus::Other("requires_action")` where `generate()` returns
`RequiresAction`. This hits the single most important streaming case. Gemini
additionally omits `budget_exceeded` and `cancelled`.

**2. Anthropic usage ignores cache tokens.** The parser sums them
(`anthropic/types.rs:337-338`):

```rust
                + u.cache_creation_input_tokens.unwrap_or(0)
                + u.cache_read_input_tokens.unwrap_or(0);
```

The decoder reads only `usage.input_tokens`. Under prompt caching the two paths
report different input totals — and `into_response`'s doc comment, added in
round 2, now explicitly claims usage matches. The documentation is wrong.

**3. Refusals are dropped when streaming.** `openai_chat/types.rs` and
`openai_responses/types.rs` both produce `OutputContent::Refusal`; neither
decoder mentions `refusal` at all. A refused response arrives as content through
`generate()` and as nothing through `stream()`.

**4. Gemini's opaque steps have the stale-snapshot bug already fixed for
Anthropic.** `Step::Opaque(step.clone())` is inserted at `gemini/mod.rs:98` and
emitted unchanged at `:148`, with no delta arm writing into it between. An
unmodeled step whose payload streams as deltas replays empty — exactly the defect
`6d95921` fixed for Anthropic's content blocks, left uncorrected one file over.

## Overall verdict

**not ready.**

Both CR5 and CR6 remain escalated. Nothing here is a regression — every repo-level
check passes and all 86 tests are green — but the spec's cross-turn replay
guarantee and its `generate()`-parity guarantee do not hold as written.

The remaining work is now fully characterised and is a bounded list, not an open
search: bring each decoder's status map, usage map, refusal handling, and opaque
accumulation into agreement with its parser, and add a parity test per dialect
rather than for Anthropic alone. The durable fix is the per-dialect parity test:
it converts this class of defect from something review must catch into something
CI catches.
