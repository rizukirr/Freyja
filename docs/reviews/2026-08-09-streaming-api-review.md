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

---

# Resolution

The user asked for all four warns and all three nits to be addressed.

## Closed

**W1 — spec drift.** `docs/specs/...-design.md` now lists all six `StreamEvent`
variants including `RefusalDelta`, with a paragraph recording why it was added
after approval and noting the internal `RawDelta::TextEnd`. Commit `bc48505`.

**W2 and W4 — untested transport.** New `tests/streaming_transport.rs` drives
`Client::stream` against a real `std::net::TcpListener` on a background thread.
No new dependency; `Cargo.toml` untouched. Four tests now cover: the happy path
including assertions that the request body carries `"stream":true` and
`"include_usage":true` and the `Authorization` header; a 429 surfacing as
`ProviderError::Api` from `stream()` itself rather than mid-iteration; Gemini's
`?alt=sse` URL and `Api-Revision` header; and `generate()` *not* requesting SSE.
`Body::Live` — the arm that calls `reqwest::Response::chunk()` — now executes
under `cargo test`. Commits `05cbdb8`, `9fbe6c0`.

Each was proven load-bearing by reverting the code it guards and observing the
specific test fail.

**W3 — plan growth.** The plan header now records that it was approved with 14
tasks, grew to 21, and was repaired three times mid-run, citing each commit.

**N1 — apparent duplication.** All four decoders now carry a comment naming the
parser function they mirror — `parse_status` (Anthropic, OpenAiResponses),
`parse_finish_reason` (OpenAiChat), and for Gemini a note that its parser maps
status inline in `impl From<Response>` with no named function — and stating why
the mapping is deliberately not shared.

**N3 — `stream_query` visibility.** Documented rather than changed: it is public
for the same reason as `path`, `default_auth`, and `required_headers`. Flagged to
the user, who did not ask for it to be narrowed.

## Not closed — N2 withdrawn

The review claimed collapsing the three test helpers would save ~10 lines. It
does not. `drain_for_test`'s signature is pinned by four call sites in the
dialect `types.rs` files, so it cannot itself be the collecting helper; the
collapse came out at **net +3 lines** and silently dropped the explicit
`assert_eq!(..., None)` terminal assertion, meaning a non-terminating stream
would hang rather than fail. Commit `030bd89` was reverted.

**The nit was wrong.** Recorded rather than quietly dropped.

## Incident: concurrent-edit regression, caught and fixed

Tasks 22 and 23 were dispatched concurrently. Task 23's Step 6 required
temporarily editing `src/provider/mod.rs` to prove a test was load-bearing — the
same file Task 22 was editing. This violated the files-disjoint rule enforced for
every other parallel dispatch in this run, and it was an orchestration error.

Task 22's commit `bc48505` captured the transient edit. The result: `Client::run`
— the **non-streaming** path — posted to `stream_url()`, so every `generate()`
call on Gemini would have requested SSE and then tried to parse the event stream
as JSON.

The entire suite passed either way. It was caught only because the Task 23 agent
noticed its own transient edit had been committed by another process and said so.

Fixed in `9fbe6c0`, together with `generate_does_not_request_sse`, which fails if
the URL is swapped back. The underlying reason it could happen silently — that
`generate()`'s HTTP path had no test either — is now closed.

## Post-resolution state

- 92 lib tests, 4 transport tests, 10 doctests — all passing
- `cargo clippy --all-targets --all-features -- -D warnings` — clean
- `cargo fmt --all --check` — clean
- `Cargo.toml` — still unchanged

Remaining from the original self-critique: risk 1 (fixtures encode a reading of
each wire format rather than observed traffic) is **unchanged**. The transport
tests prove the client speaks correct HTTP to a socket; they do not prove the
frame shapes match what the providers actually send. A live smoke run remains the
highest-value next step.

---

# Addendum: the documentation gap

Found only because the user asked "have you updated the documentation?" — after
this review had already reported 0 blocks.

**The answer was no.** The repository carries a 24-file user documentation tree
and this branch had touched none of it. Eight statements across seven files
actively told readers streaming did not exist:

```
docs/README.md:66              "There is no streaming, no retries..."
docs/introduction.md:32        "Streaming, retries... do not exist yet"
docs/features.md:59            "| **Streaming** | Not implemented"
docs/reference/client.md:126   "Streaming is not implemented yet"
docs/providers/openai.md:37    "| Streaming | no |"
docs/providers/openai-chat.md:52   same
docs/providers/anthropic.md:38     same
docs/providers/gemini.md:38        same
```

Three pages were also structurally out of date: `reference/errors.md` listed five
`ProviderError` variants where there are now six plus `#[non_exhaustive]`;
`reference/client.md` documented every method except `stream`; and
`internals/adding-a-dialect.md` described a dialect's `mod.rs` as "the Provider
impl, about 25 lines", which would have led a contributor to ship a dialect that
compiles and silently cannot stream — reproducing the exact stub state Task 7
created and the defect class that took five verification rounds to clear.

## Why every gate missed it

The spec never mentioned `docs/`. So the plan had no task for it, `verify-gate`
had no requirement to check, and this review's Pass 1 measured coverage *against
the spec*. A blind spot in the spec propagated silently through every downstream
gate that was supposed to catch it. The lesson is not "add a docs checklist" — it
is that spec-derived verification cannot see what the spec omits, and something
in the pipeline has to ask what the spec forgot.

## Resolved — Task 25 (`8b59c31`) plus a follow-up

All eight false statements corrected, `docs/reference/streaming.md` written and
linked from the index, and `adding-a-dialect.md` extended with the decoder and
parity-test steps a new dialect now needs.

Four further inaccuracies surfaced during the work that the task list had missed:

- `getting-started.md` said the repo ships "three runnable programs"; there are
  four. Fixed in a follow-up commit.
- `client.md` and `features.md` both described the default HTTP client as a
  "120 second per request timeout". It is a `read_timeout` — an inactivity
  bound — which is exactly the distinction a streaming caller needs. Both
  corrected, and the `with_http_client` sample now shows `.read_timeout()`.
- `features.md` said the error type has "Five variants".
- Neither the capability tables nor the new page implied live-API coverage that
  does not exist. `features.md` and `streaming.md` both now state plainly that
  streaming has never been exercised against a live API on any provider, and that
  every dialect's frames come from vendor documentation and recorded fixtures.

That last point matters: it makes this review's self-critique risk 1 visible to
users rather than only to us.

## State

- User docs: no remaining false statement about streaming
- 92 lib + 4 transport + 10 doctests passing; no code touched by Task 25
