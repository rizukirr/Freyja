# Tool attribute macro — Implementation Plan

**Spec:** docs/specs/2026-08-18-tool-attribute-design.md
**Goal:** Users declare typed synchronous tools with `#[tool]`, collect uniform `Tool` values, generate provider definitions, and execute model JSON without handwritten dispatch code.
**Architecture:** The main crate owns provider-neutral runtime types and privately re-exports code-generation dependencies. The proc-macro crate parses attributes and function signatures, then emits a same-named `Tool` constant backed by generated definition and execution functions.

## Global constraints

- Procedural-macro parsing and expansion live in the `freyja-macros` package.
- Provider-neutral runtime types remain in the main `freyja` package.
- Annotated parameters use explicit Rust types and simple identifier patterns.
- Argument types support deserialization and JSON Schema generation; return types support serialization.
- Existing raw `ToolDefinition` construction remains supported.
- Preserve unrelated uncommitted work in the repository.
- Async functions, receivers, generic functions, and destructured parameter patterns remain unsupported.

### Task 1: Establish the runtime boundary → verify: `cargo test --lib` exits with status 0

**Files:**
- Modify: `Cargo.toml:1-27`
- Modify: `Cargo.lock`
- Create: `macros/Cargo.toml`
- Create: `macros/src/lib.rs`
- Create: `macros/src/tools.rs`
- Modify: `src/lib.rs:90-109`
- Modify: `src/model/tools.rs:1-182`
- Modify: `src/model/mod.rs:1-15`

- [x] Keep `src/model/tools.rs` free of `syn`, `ToolAttrs`, and procedural-macro parsing code.
- [x] Create the `freyja-macros` proc-macro package with only `proc-macro2`, `quote`, and `syn` dependencies; register it as a workspace member and a main-crate dependency.
- [x] Create `macros/src/tools.rs` with the `ToolAttrs` parser for required `description`, optional `strict`, unknown-key errors, and comma-separated attributes; keep this parser solely in the macro crate.
- [x] Create `macros/src/lib.rs` with `mod tools;` as the compileable package entry point; Task 2 adds the public attribute entry point after expansion exists.
- [x] Retain `Tool` as a copyable descriptor with `name`, `definition`, and `execute` function pointers, and retain `ToolError` as its public execution error.
- [x] Add focused runtime tests using local backing functions:

  ```rust
  #[test]
  fn callable_tool_delegates_definition_and_execution() {
      let tool = Tool::new("add", definition, execute);
      assert_eq!(tool.name(), "add");
      assert_eq!(tool.definition().name, "add");
      assert_eq!(tool.execute(r#"{"a":20,"b":22}"#).unwrap(), "42");
  }
  ```

- [x] Add a documented hidden `__private` module in `src/lib.rs` that publicly re-exports `schemars`, `serde`, and `serde_json` for generated code:

  ```rust
  #[doc(hidden)]
  pub mod __private {
      pub use schemars;
      pub use serde;
      pub use serde_json;
  }
  ```

- [x] Keep `Tool`, `ToolError`, and the existing `ToolDefinition` exports available through both `freyja::model` and the crate root.
- [x] Generated code dependencies remain in the main package: add `schemars` beside the existing `serde` and `serde_json`; do not add those runtime libraries to `macros/Cargo.toml` because expansion will resolve them through `freyja::__private`.
- [x] Run `cargo fmt --all --check`.
- [x] Run `cargo test --lib`.
- [x] Stage only this task's files and commit with `feat: add callable tool runtime`.

### Task 2: Generate typed callable tools → verify: `cargo test --workspace --all-targets` exits with status 0

**Files:**
- Modify: `macros/src/lib.rs:1`
- Modify: `macros/src/tools.rs:1-53`
- Modify: `src/lib.rs:90-109`
- Create: `tests/tool_macro.rs`
- Modify: `examples/tool_loop.rs:14-107`

- [ ] Derive `Debug` for `ToolAttrs`, correct its missing-description diagnostic to `missing description = "..."`, and add parser tests covering the required description, `strict` default, explicit strict value, unknown key, and malformed separators; the derive satisfies `Result::unwrap_err` in negative parser tests.
- [ ] Implement the compiler entry point in `macros/src/lib.rs` as a thin conversion boundary:

  ```rust
  #[proc_macro_attribute]
  pub fn tool(attributes: TokenStream, input: TokenStream) -> TokenStream {
      tools::expand(attributes.into(), input.into())
          .unwrap_or_else(syn::Error::into_compile_error)
          .into()
  }
  ```

- [ ] Re-export the completed attribute at the Freyja crate root with `pub use freyja_macros::tool;`.

- [ ] Implement `tools::expand` with `proc_macro2`, `syn`, and `quote`: reject async, generic, receiver, and non-identifier parameters; rename the original function; generate a private argument struct; generate definition and executor functions; and emit a same-named `Tool` constant. Generated derives and calls use `::freyja::__private`; generated schemas pass through `strict_schema` when `strict = true`.
- [ ] Add expansion unit tests that parse generated output as `syn::File` for a supported function and assert errors for every unsupported signature category.
- [ ] Add `tests/tool_macro.rs` through the public `freyja::tool` export. Define typed tools, retain them in an array, assert names, descriptions, strict flags, object properties and required fields, successful JSON execution, and invalid-argument `ToolError` behavior.
- [ ] Rewrite `examples/tool_loop.rs` around:

  ```rust
  #[tool(description = "adds two numbers together", strict = true)]
  fn add(a: i64, b: i64) -> i64 {
      a + b
  }

  let tools = [add];
  let definitions = tools.iter().map(Tool::definition).collect::<Vec<_>>();
  ```

  Dispatch requested calls by finding `Tool::name` in `tools` and calling `Tool::execute`; remove raw schema JSON and the handwritten name match.
- [ ] Run `cargo fmt --all --check`.
- [ ] Run `cargo test --workspace --all-targets`.
- [ ] Run `cargo test --doc --workspace`.
- [ ] Run `git diff --check`.
- [ ] Stage only this task's files and commit with `feat: add tool attribute macro`.
