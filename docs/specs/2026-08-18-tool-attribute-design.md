---
title: Tool attribute macro
date: 2026-08-18
status: approved
---

# Tool attribute macro — Design

## Problem

Freyja users currently write a `ToolDefinition` JSON Schema and a separate name-based dispatcher by hand. The schema, argument parsing, and Rust function signature can drift apart. The in-progress implementation also duplicates procedural-macro parsing code into `src/model/tools.rs`, causing the runtime crate to reference the macro-only `syn` dependency.

## Goals

- A user can annotate a typed free function with `#[tool(description = "...", strict = true)]` and receive a callable `Tool` value with the same identifier.
- An array such as `let tools = [add, multiply];` contains one uniform runtime type.
- Each generated tool produces a `ToolDefinition` whose name, description, strict setting, and parameter schema match the annotated function.
- Each generated tool deserializes model arguments, invokes the annotated function, and serializes its return value without a handwritten dispatcher.
- `cargo check --workspace` and the macro's compile-time behavior tests pass without adding `syn` to Freyja's runtime dependencies.

## Non-goals

- Async functions, methods with a `self` receiver, generic functions, and destructured parameter patterns.
- Dependency injection or application-state management.
- Automatically running a complete model tool loop inside `Client`.
- Removing the existing `ToolDefinition` API.

## Constraints

- Procedural-macro parsing and expansion live in the `freyja-macros` package.
- Provider-neutral runtime types remain in the main `freyja` package.
- Annotated parameters use explicit Rust types and simple identifier patterns.
- Argument types support deserialization and JSON Schema generation; return types support serialization.
- Existing raw `ToolDefinition` construction remains supported.
- The implementation preserves unrelated uncommitted work in the repository.

## Approach

Keep `ToolDefinition` as the provider-facing description. Add `Tool` as a copyable runtime descriptor containing function pointers that construct its `ToolDefinition` and execute it from a JSON argument string. Add `ToolError` for argument, execution, and result-serialization failures.

The `freyja-macros` package owns `ToolAttrs` and the `#[tool]` entry point. For an annotated function, the macro renames the implementation internally, generates a private argument struct from its typed parameters, derives deserialization and JSON Schema support for that struct, and generates definition and executor functions. It publishes a same-named `Tool` constant, allowing `[add, multiply]` without heterogeneous function-item types.

Freyja re-exports the attribute macro. Generated paths resolve through Freyja-owned exports so users do not need to understand the macro crate's internal dependency layout. `GenerateRequest` continues storing `Vec<ToolDefinition>`; callers derive definitions from their retained `Tool` registry, which also handles dispatch.

The chosen approach implements both schema generation and typed execution. The schema-only alternative was rejected because it leaves the manual dispatcher from `examples/tool_loop.rs` in every application.

## Alternatives considered

- Generate only `ToolDefinition` constructors. This uses less runtime API but does not eliminate name matching or JSON argument parsing.
- Provide `tools![add, multiply]` as a collection macro. This can generate a registry but loses the requested plain-array syntax and makes each function less self-describing.
- Require one explicit argument struct per function. This is simpler for the macro but adds boilerplate for ordinary multi-parameter tools; explicit structs remain usable as parameter types.

## Testing

- Unit-test attribute parsing for required descriptions, strict defaults, supported keys, and malformed input.
- Expansion tests verify that supported typed functions produce syntactically valid Rust and unsupported signatures produce targeted compile errors.
- An integration test defines tools through the public `freyja::tool` re-export, checks generated `ToolDefinition` schemas, executes valid JSON, and observes invalid-argument errors.
- Update `examples/tool_loop.rs` to demonstrate a generated registry and typed dispatch.
- Run formatting, the full workspace test suite, examples compilation, and documentation tests.

## Open questions

N/A — the initial synchronous signature and runtime boundary are fixed by the approved design.
