---
title: De-glob provider imports
date: 2026-08-13
status: approved
---

# De-glob provider imports — Design

## Problem

Each of the four dialect wire-type modules opens with a wildcard import:

```rust
// src/provider/{anthropic,gemini,openai_chat,openai_responses}/types.rs
use crate::provider::*;
```

That glob resolves to everything public in `src/provider/mod.rs` plus everything
it re-exports: the whole neutral model via `pub use model::*`, and also `Client`,
`Provider`, `Auth`, `ProviderDialect`, `EventStream`, and `StreamEvent`.

`docs/internals/capability-model.md` states the rule the layout is meant to hold:
*the rack dispatches, the engine translates.* A dialect is an engine. It has no
business naming `Client`, and today the import header would not change if it did.

Measured on the current tree, nothing has leaked. Every provider-layer name a
`types.rs` actually uses:

| | `ProviderConfig` | `refusal` | `ProviderDialect` | `TokenLimitField` |
|---|---|---|---|---|
| anthropic | 6 | 9 | 1 | — |
| gemini | 3 | 2 | — | — |
| openai_chat | 7 | 11 | 1 | 5 |
| openai_responses | 3 | 10 | — | — |

`Client`, `Provider`, `Auth`, `EventStream`, and `StreamEvent` have zero uses in
all four. `ProviderType` appears only inside test modules.

So the boundary already holds. This change records what crosses it, in each
file's own header, where the next person editing that file will see it.

## Goals

Each goal is observable by running a command.

1. No wildcard import of the provider module remains in the library.
   `grep -rn 'use crate::provider::\*' src/` produces no output.
2. Every dialect's `types.rs` states its dependencies by name, and that list is
   the one the compiler derived rather than one written by hand.
3. Behaviour is unchanged. `cargo test --all-features` reports the same four
   totals as the pre-change baseline: 144, 4, 4, 17, all passing.
4. The public API is unchanged. `src/lib.rs` is not modified at all.
5. The change is import-only. `git diff --stat` touches exactly four files, and
   the diff contains no change outside `use` statements.

## Non-goals

- **Enforcing the boundary mechanically.** No lint, no guard test. Considered at
  length and rejected; see Alternatives.
- **Moving any file.** No `neutral/` directory, no `src/client.rs`, no
  `provider/streaming/`. Deferred, see Alternatives.
- **Narrowing `pub use model::*` in `provider/mod.rs`.** It stays as-is. Trimming
  it is the front half of the directory extraction and would churn the public
  surface, which goal 4 forbids.
- **Splitting `model.rs` (1,616 lines) or `mod.rs` (1,281 lines).** Both hold more
  than one job. Neither is addressed here.
- **Touching the `use super::*` in the ten `#[cfg(test)] mod tests` blocks.**
  They are why a test module inherits its parent's imports.
- **Any change to `tests/` or `examples/`.** They name only `freyja::X` and are
  unaffected.

## Constraints

- Rust edition 2024, MSRV 1.88. No new dependency.
- No behavioural change is permitted. A wrong import is a compile error, so the
  compiler is the safety net.
- The existing CI jobs must stay green unchanged; none of them is modified.

## Approach

Replace each glob with an explicit list. The list comes from the compiler, not
from a reading of the file: delete the glob, run `cargo check`, and add back
exactly the names it reports as unresolved. Hand-writing the list is the one real
risk here, and this removes it.

That is the whole change. Four files, import headers only.

### Pushback recorded

Two challenges were raised and both changed the design.

**First**, against the larger framing. The original proposal was to extract
`src/neutral/{mod,error,schema}.rs` out of `src/provider/model.rs`, lift `Client`
into `src/client.rs`, and group `sse.rs` with `stream.rs` under
`src/provider/streaming/`. The challenge: the glob, not the layout, is what makes
the boundary invisible, so de-globbing enforces it with zero files moved. The
user chose the smaller framing. The larger version remains available and is
unblocked by this change; de-globbing is its first step either way.

**Second**, against the lint. An earlier version of this spec added
`#![warn(clippy::wildcard_imports)]` to `src/lib.rs`, on the claim that it made
the boundary structural. The user asked whether it was needed. It was not, and
the claim was wrong: the lint bans globs, not rack access, so a dialect writing
`use crate::provider::Client;` passes it silently. It preserves the visibility of
a violation, never its absence. The lint was dropped.

### Files

| File | Change |
|---|---|
| `src/provider/anthropic/types.rs` | glob → explicit: `ProviderConfig`, `ProviderDialect`, `refusal`, neutral types |
| `src/provider/gemini/types.rs` | glob → explicit: `ProviderConfig`, `refusal`, neutral types |
| `src/provider/openai_chat/types.rs` | glob → explicit: `ProviderConfig`, `ProviderDialect`, `TokenLimitField`, `refusal`, neutral types |
| `src/provider/openai_responses/types.rs` | glob → explicit: `ProviderConfig`, `refusal`, neutral types |

## Alternatives considered

**Add `#![warn(clippy::wildcard_imports)]`.** Rejected on three grounds, in
descending order of weight. It does not enforce the rule it was proposed for — an
explicit `use crate::provider::Client;` sails through it. It is crate-wide for a
four-file concern. And it would forbid the legitimate case if the deferred
restructure lands: a dialect writing `use crate::neutral::*` is the boundary being
honored, and the fix would be an `#[allow]`, which is worse than never having had
the lint.

**A guard test grepping the four `types.rs` files for `Client`, `Provider`,
`Auth`, `EventStream`.** This one does target the real rule, and it matches the
ratchet pattern already used in `src/provider/refusal.rs`. Rejected as
speculative: the table under Problem was measured, not assumed, and shows zero
occurrences across the project's history. `refusal.rs`'s ratchet exists because
refusals had actually been wrong twice; nothing here has gone wrong once.

**The full restructure.** Deferred, see Pushback recorded above.

**Grouping under `provider/shared/`.** Proposed and rejected during design.
`shared` names a property rather than a job, so it accumulates anything; and in
this codebase specifically it is where a shared translation helper would
eventually land, which `capability-model.md` forbids. `sse.rs` and `stream.rs`
already carry names that say what they do.

## Testing

No test is added. The change cannot alter behaviour — a wrong import does not
compile — so the existing suite is the regression check, and it covers these
files heavily (626 test lines in `gemini/types.rs` alone).

Verification, in order:

1. `grep -rn 'use crate::provider::\*' src/` → no output
2. `cargo test --all-features` → exit 0, four totals matching the baseline
3. `cargo clippy --all-targets --all-features -- -D warnings` → exit 0
4. `cargo fmt --check` → exit 0
5. `git diff --stat` → exactly 4 files, none of them `src/lib.rs`

Step 3 runs the CI lint job as it already exists. It is a regression check that
the change introduced no new warning, not an enforcement mechanism.

## Open questions

None.
