# Changelog

Notable changes per release. Freyja is pre-1.0, so a minor version may break.

## Unreleased

### Added

- **`Agent::guard`** takes a closure consulted before every tool call,
  receiving the requested name, the model's raw JSON arguments and the
  run's `Context`, and returning the new `Decision` enum. `Allow` runs
  the tool; `Deny(reason)` does not, and sends the model
  `denied: {reason}` as the tool result instead — the same channel it
  reads `error: {error}` from, so a refusal is something it can recover
  from rather than a failure. The guard runs before the tool lookup, so
  it sees every name requested, including tools registered at runtime
  and names matching no tool at all. An agent without a guard is
  unchanged.

## 0.2.0

The published `0.1.1` predates most of the current API: its `src/` held a
single `provider` module. Everything below happened between that release
and this one, so the breaking list is long. If you are upgrading from a
git checkout rather than from crates.io, only the tool section is likely
to affect you.

### Breaking

**The `provider` module is gone**, replaced by modules named for what
they do. Everything is re-exported at the crate root, so `use freyja::X`
keeps working for the types that kept their names.

| 0.1.1 | 0.2.0 |
|---|---|
| `provider::Provider` | `Client`, plus `dialect::Dialect` for the wire format |
| `provider::ProviderConfig` | `endpoint::EndpointConfig` |
| `provider::ProviderDialect` | `dialect::Dialect` |
| `provider::ProviderType` | `endpoint::EndpointPreset` |
| `provider::ProviderError` | `error::Error` and `error::TransportError` |

**`ReasoningEffort::Minimal` was removed.** Only one provider accepted it
and it did not mean the same thing there.

**`Tool` is a trait rather than a struct.** This one only affects code
written against a git checkout — `Tool` was never in a published release.

```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn definition(&self) -> ToolDefinition;
    fn call<'a>(&'a self, arguments: &'a str, cx: &'a Context) -> ToolFuture<'a>;
}
```

`#[tool]` functions are unaffected: the macro emits the impl, and the call
site still reads `search`. Otherwise: `Agent::tools([a, b])` becomes
`.tool(a).tool(b)`, because each `#[tool]` function has its own type and
two types cannot share an array; `Tool::new`, `Tool::new_async` and
`Tool::execute` are gone in favour of `impl Tool` and
`tool.call(arguments, &cx)`; `Tool` is no longer `Copy`, so dispatch loops
drop `.copied()`; and `ToolFuture` gained a lifetime, `ToolFuture<'a>`,
because a trait method borrows `&self`.

### Added

**Tool calling, end to end.** `#[tool]` derives a JSON Schema and a typed
executor from a sync or `async fn`. `Tool` carries it, `ToolChoice`
constrains it, and the full round trip is verified against live APIs.

**`Agent` and `Chat`.** `Agent` drives the tool-calling loop — bounded
turns, parallel tool calls dispatched concurrently, `StopReason` saying
why it stopped, `Usage` summed across turns. The transcript stays yours:
`run` extends a `Vec<Message>` in place. `Chat` keeps its own transcript
if you would rather not.

**Tools that hold state.** Implement `Tool` on a struct and its fields
survive across calls — a database handle, an HTTP client, a rate limiter.

**`Context`, per-run data.** A type-keyed map handed to every tool call
and never sent to the model. `Agent::run_with` and `Chat::ask_with` take
one; `run` and `ask` pass an empty one. A `#[tool]` function may take
`cx: &Context` as its first parameter, excluded from the generated
schema. State known when the tool is built goes in fields; state that
arrives with the request goes in `Context`.

**Tools defined at runtime.** `name` returns `&str` and `definition`
returns a value, so a tool whose name and schema arrive at runtime — the
MCP shape — needs no compile-time type.

**Tools that report failure.** A `#[tool]` returning `Result` maps
`Err(e)` to `ToolError::Execution(e.to_string())`, reaching the model as
text it can recover from. Detection is textual, so a `Result` behind a
type alias is not detected and the value serializes whole.

**`Client::check`**, a pre-flight pass that answers whether a provider
will accept a request without sending it.

**`Client::generate_as`**, deserializing the answer straight into a type.

**`strict_schema`**, rewriting a JSON Schema into the subset strict mode
accepts.

**`provider_metadata`**, an escape hatch for fields the neutral model
does not name.

**Typed errors.** `Error` and `TransportError` classify failures by
cause, with `is_retryable()` and `retry_after()`.

**`ToolError` implements `Display` and `std::error::Error`**, so tool
failures reach the model as bare messages rather than `Debug` output.

### Changed

**Duplicate tool names replace rather than shadow.** Registering two
tools under one name used to keep the first, silently. The last
registered now wins.

### Fixed

- Gemini received a request shape its API actually accepts.
- Gemini carries reasoning effort and tool choice after all.
- Gemini metadata is refused rather than sent as a field it rejects.
- Gemini values the format has a field for are no longer refused.
- Images ride on the turns that accept them.
- The Chat Completions token cap accepts either spelling.
- Strict-schema rewriting no longer renames properties as if they were
  keywords.

### Notes

- Nothing in the tool redesign was verified against a live provider; it
  is exercised against a scripted local endpoint. The examples compile
  but were not run against a real key.
- No example yet demonstrates per-tool state, `Context`, runtime-defined
  tools or fallible tools.

## 0.1.1

First published release. One request model over four wire dialects,
streaming, and the tool round trip, behind a single `provider` module.
