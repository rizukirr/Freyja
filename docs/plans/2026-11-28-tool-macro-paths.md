# tool-macro-paths: Implementation Plan

**Spec:** docs/specs/2026-11-28-tool-macro-paths-design.md
**Goal:** `#[tool]` compiles in a crate whose only dependency is `freyja`, with a workspace member that proves it in CI.
**Architecture:** Two `crate = ...` attributes on the argument struct the macro generates point the serde and schemars derives at freyja's re-exports. A `publish = false` workspace member, `downstream/`, depends only on freyja and uses `#[tool]`, and CI tests it.

## Global constraints
- Rust 2024, MSRV 1.88.
- No new dependency for freyja or freyja-macros.
- No live LLM calls. Use `cargo --offline`.

### Task 1: Fix the derive paths and add the guard → verify: `cargo --offline test -p downstream` exits 0, `cargo --offline test --all-features` exits 0, and `cargo --offline clippy --all-features --all-targets -- -D warnings` exits 0

**Blocked by:** none

**Files:**
- Modify: `macros/src/tools.rs`
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Create: `downstream/Cargo.toml`
- Create: `downstream/src/lib.rs`
- Modify: `.github/workflows/ci.yml`
- Modify: `CHANGELOG.md`

- [x] Step 1: In `macros/src/tools.rs`, inside the `quote!` that generates the argument struct, directly after the closing `)]` of the `#[derive(::freyja::__private::serde::Deserialize, ::freyja::__private::schemars::JsonSchema,)]` attribute and before `struct #arguments_name {`, add:

```rust
        #[serde(crate = "::freyja::__private::serde")]
        #[schemars(crate = "::freyja::__private::schemars")]
```

- [x] Step 2: In the root `Cargo.toml`, change `members = ["macros"]` under `[workspace]` to `members = ["macros", "downstream"]`.

- [x] Step 3: Create `downstream/Cargo.toml`:

```toml
# Compiles `#[tool]` the way a user's crate does, with freyja as the only
# dependency. Freyja's own tests cannot catch a macro that needs more, because
# they see freyja's dependencies.
[package]
name = "downstream"
version = "0.0.0"
edition = "2024"
publish = false

[dependencies]
freyja = { path = ".." }
```

- [x] Step 4: Create `downstream/src/lib.rs`:

```rust
//! A `#[tool]` function in a crate that depends on nothing but freyja.
//!
//! The tool sits in the test module because `#[tool]` makes everything it
//! generates private, so outside tests it would be dead code.

#[cfg(test)]
mod tests {
    use freyja::{Tool, tool};

    #[tool(description = "adds two numbers together")]
    fn add(a: i64, b: i64) -> i64 {
        a + b
    }

    #[test]
    fn the_tool_builds_with_freyja_as_the_only_dependency() {
        assert_eq!(add.name(), "add");
    }
}
```

- [x] Step 5: In `.github/workflows/ci.yml`, in the test job, directly after the line `      - run: cargo test --all-features`, add the two lines:

```yaml
      # A crate with freyja as its only dependency, as a user's would be.
      - run: cargo test -p downstream
```

- [x] Step 6: In `CHANGELOG.md`, under `## Unreleased`, after the last bullet of `### Added` and before `## 0.5.0`, add a blank line, the heading `### Fixed`, a blank line, and this bullet on one line: "- **`#[tool]` compiles in a crate whose only dependency is freyja.** The serde and schemars derives it generates named `serde` and `schemars` directly, so a crate without those two dependencies failed with `can't find crate for serde`. They now go through freyja's re-exports, and a workspace crate with freyja as its only dependency checks this in CI."

- [x] Step 7: Run `cargo --offline fmt --all`, then `cargo --offline test -p downstream`, `cargo --offline test --all-features` and `cargo --offline clippy --all-features --all-targets -- -D warnings`.

- [x] Step 8: Commit all files in the Files block, `Cargo.lock` included, as "#[tool] compiles in a crate whose only dependency is freyja".
