---
title: tool-macro-paths
date: 2026-11-28
status: approved
---

# tool-macro-paths: Design

## Problem

A crate whose only dependencies are `freyja` and `tokio`, as `docs/getting-started.md` instructs, cannot compile a `#[tool]` function. The macro in `macros/src/tools.rs` derives `::freyja::__private::serde::Deserialize` and `::freyja::__private::schemars::JsonSchema`, but both derives expand to paths starting with `serde` and `schemars`, which resolve only when the user's crate depends on them directly. Freyja's own tests compile inside the freyja package, which has both, so nothing catches it.

## Goals

1. A crate depending only on `freyja` compiles a `#[tool]` function and uses it as a `freyja::Tool`. Criterion: a new workspace member `downstream/`, whose `[dependencies]` holds only `freyja` by path, defines a `#[tool]` function and a test asserting its `Tool::name`, and `cargo --offline test -p downstream` exits 0.
2. The guard fails without the fix. Criterion: with the two attribute lines removed from `macros/src/tools.rs`, `cargo --offline build -p downstream` exits non-zero.
3. CI runs the guard. Criterion: `.github/workflows/ci.yml`'s test job runs `cargo test -p downstream`.
4. The guard is never published. Criterion: `downstream/Cargo.toml` has `publish = false`.
5. Existing behaviour is unchanged. Criterion: `cargo --offline test --all-features` and `cargo --offline clippy --all-features --all-targets -- -D warnings` exit 0.
6. `CHANGELOG.md` gets a `### Fixed` entry under Unreleased naming `#[tool]`. Criterion: `grep -n "tool" CHANGELOG.md` shows the line between `## Unreleased` and `## 0.5.0`.

## Non-goals

- A version bump or release.
- Changes to the `#[tool]` attribute syntax or the generated tool's behaviour.

## Constraints

- Rust 2024, MSRV 1.88.
- No new dependency for freyja or freyja-macros.
- No live LLM calls.
- Run limits: at most 3 verify rounds, at most $0 on paid runs.

## Approach

Pushback: no simpler framing, the fix is two attributes. The generated argument struct gets `#[serde(crate = "::freyja::__private::serde")]` and `#[schemars(crate = "::freyja::__private::schemars")]`, which both derives support. An experiment applied them to a copy of the repo and an outside crate depending only on freyja and tokio built clean. The guard is a workspace member because only a separate package has its own dependency list.

## Alternatives considered

- Documenting that users must add `serde` and `schemars`. Leaks the macro's internals into every user's manifest.
- A trybuild test. Adds a dev dependency, where a workspace member needs none.

## Testing

`downstream/` is the regression test. Goal 2 is checked once at verify time by removing the fix.

## Open questions

N/A: none left after the decisions above.
