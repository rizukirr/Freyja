# De-glob provider imports — Implementation Plan

**Spec:** docs/specs/2026-08-13-deglob-provider-imports-design.md
**Goal:** Replace the four `use crate::provider::*` globs in the dialect wire-type modules with the explicit imports the compiler derives.
**Architecture:** No file moves and no type relocations. Four `types.rs` files lose a wildcard import and gain a named one. `src/lib.rs` and `provider/mod.rs` are not touched, so the public surface is unchanged and nothing outside `src/provider/*/types.rs` is affected.

## Global constraints

- Rust edition 2024, MSRV 1.88 (read from `Cargo.toml`).
- No new dependency, and no change to any file under `.github/`.
- Baseline test counts, observed by running `cargo test --all-features` on `main` before any edit: 144 + 4 + 4 + 17, all passing. Any change to these totals is a regression.
- `src/lib.rs` must not be modified.
- No edit outside `use` statements.
- Do not touch the `use super::*;` inside any `#[cfg(test)] mod tests` block.
- Commit messages carry a subject and body only. No trailers.
- `docs/plans` and `docs/specs` are both listed in `.gitignore`, so neither this plan nor the spec is committed. Do not force-add them.

---

### Task 1: Replace the four globs with explicit imports → verify: `cargo test --all-features` exits 0, and `grep -rn 'use crate::provider::\*' src/` exits non-zero

**Files:**
- Modify: `src/provider/anthropic/types.rs:4`
- Modify: `src/provider/gemini/types.rs:4`
- Modify: `src/provider/openai_responses/types.rs:4`
- Modify: `src/provider/openai_chat/types.rs:8`

Each line above is the file's sole `use crate::provider::*;`. Line numbers were read with `grep -n 'use crate::provider::\*;' src/provider/*/types.rs`; note that `openai_chat` differs from the other three.

- [x] Step 1: Delete the `use crate::provider::*;` line in `src/provider/anthropic/types.rs`.
- [x] Step 2: Run `cargo check --all-features`.
- [x] Step 3: In place of the deleted line, add one `use` block naming exactly the paths reported by the Step 2 diagnostics — every `cannot find type`, `cannot find value`, `failed to resolve` and `use of undeclared` entry, and nothing else. Do not add a name the compiler did not ask for. Group them as the surrounding files do: one `use crate::provider::{...};` for items re-exported from the provider root, and separate lines for anything reached through a submodule path.
- [x] Step 4: Run `cargo check --all-features`. If it still reports unresolved names, return to Step 3 and add those too. Repeat until it exits 0.
- [x] Step 5: Repeat Steps 1 through 4 for `src/provider/gemini/types.rs`.
- [x] Step 6: Repeat Steps 1 through 4 for `src/provider/openai_responses/types.rs`.
- [x] Step 7: Repeat Steps 1 through 4 for `src/provider/openai_chat/types.rs`.
- [x] Step 8: Run `grep -rn 'use crate::provider::\*' src/`. It must produce no output.
- [x] Step 9: Run `git diff` and read it. Every changed hunk must be inside a `use` statement, and no hunk may touch `src/lib.rs`. If any hunk fails either check, revert that hunk.
- [x] Step 10: Run `cargo test --all-features` and compare each of the four reported totals against the baseline in Global constraints.
- [x] Step 11: Run `cargo fmt --check`. If it fails, run `cargo fmt` and re-run Step 10.
- [x] Step 12: Run `cargo clippy --all-targets --all-features -- -D warnings`. This is the CI lint job as it already exists; it must exit 0.
- [x] Step 13: Commit.

Notes for the implementer:

The compiler produces the import list; this plan deliberately does not state it. A hand-written list is the one real failure mode here, and Step 3 exists to avoid it.

The four `#[cfg(test)] mod tests` blocks begin at `anthropic/types.rs:403`, `openai_responses/types.rs:355`, `gemini/types.rs:393`, and `openai_chat/types.rs:430` (read with `grep -n '^#\[cfg(test)\]'`). Each has its own `use super::*;` and its own explicit `use crate::provider::…` lines. Leave all of them alone — the `use super::*;` re-exports the parent module's new imports into the test module automatically, which is why the tests keep compiling.

If a test module reports an unresolved name after Step 3, the cause is a name the parent used only through the glob and that Step 3 missed. Fix it in the parent's `use` block, not by adding an import to the test module.

No lint and no guard test accompany this change. Both were considered and rejected in the spec's Alternatives section; do not add one.

---

## Verification after the task

- [x] Run `grep -rn 'use crate::provider::\*' src/` — no output.
- [x] Run `cargo test --all-features` — exit 0, totals matching the baseline.
- [x] Run `cargo clippy --all-targets --all-features -- -D warnings` — exit 0.
- [x] Run `cargo fmt --check` — exit 0.
- [x] Run `git diff main --stat` — the changed-file list is exactly the four `types.rs` paths above.
