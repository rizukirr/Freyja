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
- Modify: `macros/Cargo.toml:1-17`
- Modify: `src/lib.rs:90-109`
- Modify: `src/model/tools.rs:1-182`
- Modify: `src/model/mod.rs:1-15`

- [ ] Remove `syn`, `ToolAttrs`, and its `Parse` implementation from `src/model/tools.rs`; the identical parser remains solely in `macros/src/tools.rs`.
- [ ] Retain `Tool` as a copyable descriptor with `name`, `definition`, and `execute` function pointers, and retain `ToolError` as its public execution error.
- [ ] Add focused runtime tests using local backing functions:

  ```rust
  #[test]
  fn callable_tool_delegates_definition_and_execution() {
      let tool = Tool::new("add", definition, execute);
      assert_eq!(tool.name(), "add");
      assert_eq!(tool.definition().name, "add");
      assert_eq!(tool.execute(r#"{"a":20,"b":22}"#).unwrap(), "42");
  }
  ```

- [ ] Add a documented hidden `__private` module in `src/lib.rs` that publicly re-exports `schemars`, `serde`, and `serde_json` for generated code, and re-export `freyja_macros::tool` at the crate root:

  ```rust
  #[doc(hidden)]
  pub mod __private {
      pub use schemars;
      pub use serde;
      pub use serde_json;
  }

  pub use freyja_macros::tool;
  ```

- [ ] Keep `Tool`, `ToolError`, and the existing `ToolDefinition` exports available through both `freyja::model` and the crate root.
- [ ] Remove `schemars`, `serde`, and `serde_json` from `macros/Cargo.toml`; generated code resolves them through `freyja::__private`, while the proc-macro implementation retains only `proc-macro2`, `quote`, and `syn`.
- [ ] Run `cargo fmt --all --check`.
- [ ] Run `cargo test --lib`.
- [ ] Stage only this task's files and commit with `feat: add callable tool runtime`.

### Task 2: Generate typed callable tools → verify: `cargo test --workspace --all-targets` exits with status 0

**Files:**
- Modify: `macros/src/lib.rs:1`
- Modify: `macros/src/tools.rs:1-53`
- Create: `tests/tool_macro.rs`
- Modify: `examples/tool_loop.rs:14-107`

- [ ] Correct `ToolAttrs`' missing-description diagnostic to `missing description = "..."` and add parser tests covering the required description, `strict` default, explicit strict value, unknown key, and malformed separators.
- [ ] Implement the compiler entry point in `macros/src/lib.rs` as a thin conversion boundary:

  ```rust
  #[proc_macro_attribute]
  pub fn tool(attributes: TokenStream, input: TokenStream) -> TokenStream {
      tools::expand(attributes.into(), input.into())
          .unwrap_or_else(syn::Error::into_compile_error)
          .into()
  }
  ```

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
