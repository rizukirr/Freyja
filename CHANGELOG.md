# Changelog

Notable changes per release. Freyja is pre-1.0, so a minor version may break.

## Unreleased

### Security

- **`Client`'s `Debug` no longer prints the HTTP client**, which leaked
  credentials past every other redaction in the struct.
  `reqwest::Client` prints its `default_headers` in full, so a client
  built with an auth header and handed to `Client::with_http_client`, the
  documented way to share a pool or carry a second credential, put that
  header in the logs on one `tracing::debug!(?client)`. It now renders as
  `"<reqwest::Client>"`. This covers what Freyja prints. Logging your own
  `reqwest::Client` still prints its headers, so mark a credential header
  with `HeaderValue::set_sensitive` if you do that.

- **One streaming event may buffer 16 MiB, and one response body 64 MiB**,
  after which the read is abandoned with `Error::Stream` or
  `Error::InvalidResponse`. Neither had a ceiling before. The read timeout
  bounds silence rather than volume, so an endpoint that never ends a
  frame, or never stops sending, was buffered whole until the process ran
  out of memory. Both limits sit orders of magnitude above anything a
  provider sends.

### Fixed

- **A tool taking no arguments is now sendable.** `ToolDefinition::new`
  left `parameters` at `Value::Null`, and all four dialects sent that to
  the wire, where every provider rejects it: OpenAI answers
  `expected an object, but got null`, and Anthropic requires
  `input_schema` to be an object. The constructor now starts at
  `{"type": "object", "properties": {}}`, and because the field is public,
  the dialects substitute the same schema for anything that is not a JSON
  object. `#[tool]` was never affected, since it always generated a
  schema. `Client::check` still passes such a request, correctly: a schema
  is a value the endpoint judges, not a capability the dialect lacks.

- **The Chat Completions streaming decoder no longer deep-copies every
  frame.** It cloned the whole parsed frame into `provider_metadata`, and
  since every chunk in this dialect carries `id` and `model`, that ran
  once per token for a field the assembler overwrites on the next one. It
  now hands over the value it already owns. The other three decoders
  attach metadata once per stream and were never affected.

- **Server-sent event framing is no longer quadratic in frame size.** Each
  arriving chunk rescanned the whole accumulated buffer from the start. A
  cursor now resumes the scan where the last one ended, reaching back
  three bytes so a separator straddling a chunk boundary is still found.
  Small text deltas hid this. A large reasoning blob did not.

- **The Chat Completions request builder no longer clones every text part
  twice.** It built both the string and the array rendering of each turn
  and discarded one. Which rendering a turn gets depends only on whether
  an image is in it, which is knowable up front, so it now builds one. An
  agent loop rebuilds the whole transcript every turn, so the saving
  scales with both.

### Changed

- **`Error`'s `Display` and `Debug` trim the endpoint's response body** to
  `BODY_IN_MESSAGE` bytes, 2048, and say how much they dropped. Providers
  routinely quote the entire offending request back in a 400: one real
  OpenAI rejection turned `error.to_string()` into a single 7173 character
  log line, now 2098. `Debug` is capped too, since `tracing::error!(?error)`
  is at least as common as `"{error}"`, and it also trims
  `OutputMismatch`'s `text`, which is the model's whole answer.

  This bounds the size of what reaches your logs, not the content. A
  provider quoting your user's text back puts it well inside the cap.

  `Error` no longer derives `Debug`. The hand-written impl reproduces the
  derived shape, so this is only visible if you were matching on that
  output.

- **`Error::body`** returns the untrimmed response body on the seven
  status-bearing variants, and `None` on the rest, so nothing the
  renderings drop is out of reach.

## 0.2.1

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

- **`examples/guarded_tools.rs`**, the first runnable example of the
  0.2.0 tool capabilities: a hand-written tool holding state, a `#[tool]`
  reading a newtype out of `Context` under `run_with`, a fallible tool
  whose `Err` the model recovers from, and a guard. The same agent runs
  twice over two contexts, so allow and deny come from one policy.

### Documentation

- **Per-tool timeouts are out of scope, deliberately**, and no longer
  listed as planned. Racing a call against a clock needs a timer, and
  Freyja depends on no async runtime so it has none to reach for. Every
  caller already does, and a short wrapper tool holding the inner one in
  an `Arc` applies your runtime's timeout — wrapping an erased
  `Arc<dyn Tool>` as readily as a tool you wrote. The `Tool`
  documentation carries the implementation as a compiled doctest, so it
  cannot drift from the trait it wraps. A call that runs out of budget
  reaches the model as `error: …`, like any other tool failure.

  The caveat is written down alongside it: a budget bounds a *slow* tool,
  not a *blocking* one. A timeout resolves only when the inner future
  yields, and tool calls are polled on the caller's task, so a tool that
  never awaits starves its siblings and no timer fires.

  This closes Phase 2 of the roadmap.

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
