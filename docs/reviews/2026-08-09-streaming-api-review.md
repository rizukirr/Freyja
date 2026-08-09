# Review — Streaming API

**Date:** 2026-08-09
**Spec:** `docs/specs/2026-08-09-streaming-api-design.md`
**Plan:** `docs/plans/2026-08-09-streaming-api.md`
**Verify report:** `docs/verifications/2026-08-09-streaming-api-verify.md` (verdict: ready)
**Commits under review:** `1c3d5b0..HEAD` on `vibe/streaming-api`

## Diff summary

- Files changed: 17 (14 under `src/` and `examples/`)
- Lines added: 4774, removed: 149
- Of which shipped code: **2871 added, 26 removed** — the remainder is the spec,
  plan, and verification documents
- Production vs test split within `src/`: roughly **1440 production / 1430 test**
- Commits: 51
- Largest new construct: `src/provider/stream.rs`, 851 lines (488 implementation,
  362 tests)
- `Cargo.toml`: **unchanged** — zero new dependencies, as the spec required

## Findings

### Block

None.

### Warn

**W1 — the approved spec no longer describes the shipped API.**
`docs/specs/2026-08-09-streaming-api-design.md` still carries `status: approved`
and documents a five-variant `StreamEvent`:

```
    TextDelta(String),
    ToolCall { id, name, arguments },
    ReasoningDelta(String),
    Reasoning { data },
    Done { ... },
```

The crate ships six — `RefusalDelta(String)` was added by Task 19
(`src/provider/stream.rs:19-22`) because without it a streaming caller could not
observe a refusal at all and `into_response()` silently dropped a whole content
part, breaking CR6. `RawDelta::TextEnd` was added by Task 21 but is internal.

Judged `warn` rather than `block`: the extension was surfaced to the user during
verification and not objected to, and `StreamEvent` is `#[non_exhaustive]` so no
downstream matcher breaks. But the committed spec is now stale, and a spec that
misdescribes the code is worse than no spec. Remedy: amend the spec's Approach
section to include `RefusalDelta` and note `TextEnd`, one paragraph.

**W2 — the transport half of streaming has no test coverage.**
`Client::stream` (`src/provider/mod.rs:417`) is referenced only by `no_run`
doctests and `examples/streaming.rs`; **no test executes it**. `Body::Live`
(`src/provider/stream.rs:431-443`) — the arm that actually calls
`reqwest::Response::chunk()` — never runs under `cargo test`. So the following
are unverified by CI: request-body construction with `stream: true`, Gemini's
`?alt=sse` URL, header and auth application, the non-2xx early return, and the
byte-pump loop.

The decode/assemble half is thoroughly covered; this is the other half. Not a
block: the spec has no integration-test requirement and the repo deliberately
has no network tests. Remedy if wanted: a `wiremock`-style local server test, or
accept and document.

**W3 — the plan grew from 14 tasks to 21 during execution.**
Seven follow-up tasks (15–21) were appended after verification found defects, and
the plan file was additionally repaired three times mid-run (test filters that
matched nothing; deferring doctests and clippy past Task 6; correcting Task 14's
reachability checks). Each change is committed separately with its reasoning, so
the history is auditable — but the plan as it now stands is not the plan that was
approved before execution began.

**W4 — timeout semantics changed for `generate()`, untested.**
Task 1 switched `default_http()` from `.timeout()` to `.read_timeout()`
(`src/provider/mod.rs`), so the default client now bounds *inactivity* rather
than total request duration. This was necessary — reqwest 0.13 has no
per-request `read_timeout`, so the spec's stated mitigation was unimplementable —
and it is documented on `Client::new`. But it changes non-streaming behaviour,
and no test covers timeout behaviour in either path, so a regression here is
invisible to CI.

### Nit

**N1 — status and usage mapping repeat structurally across the four decoders.**
Each decoder hand-writes its own status match and `Usage` construction
(`anthropic/mod.rs`, `gemini/mod.rs`, `openai_chat/mod.rs`,
`openai_responses/mod.rs`). This *looks* like duplication but is not: each
mirrors a different parser's arms and field names, and the five rounds of
verification defects were caused precisely by these mappings drifting from their
parsers. Factoring them together would reintroduce the coupling that caused the
bugs. Left as is deliberately.

**N2 — three test-only helpers where two would do.**
`EventStream::for_test`, `EventStream::next_blocking`, and the free
`drain_for_test` (`src/provider/stream.rs:455-500`) total ~35 lines.
`next_blocking` exists only to drive `next()` without a runtime; `drain_for_test`
wraps it. Could collapse to two functions. Immaterial.

**N3 — `ProviderDialect::stream_query()` is public with one internal caller**
(`src/provider/mod.rs:99`, called at `:204`). Consistent with its siblings
`path()`, `default_auth()`, and `required_headers()`, which are also public
descriptions of a dialect, so it fits the existing shape.

## Pass 4 — simplicity verdict

Largest construct is `stream.rs` at 488 implementation lines across three layers:
SSE framing, a per-dialect decoder trait, and the shared assembler. A senior
engineer could not halve it without collapsing those layers — and the layer split
is what let each of the eleven parity defects be fixed in exactly one place
rather than four. The per-dialect decoders (96–202 lines each) are dense mapping
code with no abstraction to remove.

No `delete:`, `stdlib:`, `native:`, or `yagni:` candidates found. One `shrink:`
candidate at N2 worth ~10 lines.

**net: -10 lines possible.** Effectively lean already.

## Pass 5 — surgical diff

`clean`, zero orphans, re-run over the full 51-commit branch. Every changed file
traces to a plan task, to the isolate step (`.gitignore`), to a disclosed plan
repair, or to the two orchestrator fixes (`850558c` clippy, `523a61b`
argumentless call), both of which are recorded in the verification report.

## Self-critique (three risks the tests would not catch)

1. **The fixtures encode my reading of each wire format, not observed traffic.**
   Every SSE fixture was hand-authored from provider documentation and research.
   If my understanding of a dialect's frame shape is wrong, the decoder and the
   test agree with the same wrong assumption and CI stays green. This is
   sharpest for Gemini: its Interactions API format was researched during
   planning, pinned at `Api-Revision: 2026-05-20`, and never observed live.
   *Mitigation: none.* Follow-up: one recorded-from-live fixture per dialect, or
   a manual smoke run of `cargo run --example streaming` against each provider
   before release. This is the deepest risk in the change.

2. **`generate()` and `stream()` could drift apart again.** The four parity tests
   pin the shapes the fixtures construct. Three fixes are pinned by *no* test —
   Anthropic/Gemini partial-usage defaulting, `provider_metadata` population for
   the three non-Anthropic dialects, and the OpenAI dialects'
   `normalizes_tool_arguments = false`. Reverting any of those passes CI.
   *Mitigation: partial.* Follow-up: extend each dialect's parity fixture to
   cover its own usage-defaulting and metadata, so all four are symmetric.

3. **Nothing verifies the crate against a real endpoint.** Combined with W2, the
   entire path from `client.stream(&request)` to the first byte is exercised only
   by compilation. A wrong header, a malformed body field, or a broken
   `?alt=sse` URL would ship green. *Mitigation: none.* Follow-up: the smoke run
   in risk 1 covers this too, which is why that one item is the highest-value
   next step.

## Diff

Full diff: `git -C .vibe-worktrees/2026-08-09-streaming-api diff 1c3d5b0..HEAD`

Shipped code only: `git -C .vibe-worktrees/2026-08-09-streaming-api diff 1c3d5b0..HEAD -- src examples Cargo.toml README.md`

Per-file line counts:

```
1	1	README.md
36	0	examples/streaming.rs
33	3	src/lib.rs
202	0	src/provider/anthropic/mod.rs
295	0	src/provider/anthropic/types.rs
197	0	src/provider/gemini/mod.rs
227	0	src/provider/gemini/types.rs
177	23	src/provider/mod.rs
26	0	src/provider/model.rs
96	0	src/provider/openai_chat/mod.rs
211	0	src/provider/openai_chat/types.rs
125	0	src/provider/openai_responses/mod.rs
270	0	src/provider/openai_responses/types.rs
125	0	src/provider/sse.rs
851	0	src/provider/stream.rs
```

## Sign-off

- [ ] User reviewed findings.
- [ ] User reviewed diff.
- [ ] User approves proceeding to finish-branch.
