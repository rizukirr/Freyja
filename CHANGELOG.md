# Changelog

Notable changes per release. Freyja is pre-1.0, so a minor version may break.

## Unreleased

### Security

- **A redirect no longer carries the credential to another origin.** `reqwest`
  strips `Authorization`, `Cookie`, `Proxy-Authorization` and
  `WWW-Authenticate` when a redirect crosses an origin, and it cannot strip a
  header it has no way to recognize. `Auth::Header` puts the key in `x-api-key`
  or `x-goog-api-key`, so an endpoint answering `307` with a `Location`
  elsewhere was handed it. Measured against two local servers, the Anthropic
  key arrived at the second host and the call returned `Ok` to the caller,
  while the same test on `Auth::Bearer` did not. Freyja's own client now
  follows a redirect only when scheme, host and port all match, which is
  `reqwest`'s own definition of the boundary it strips at, so the hop refused
  and the hop stripped for are the same hop. A refused hop surfaces as
  `Error::Api` with the 3xx status rather than a 401 from a host nobody named.
  A client supplied through `Client::with_http_client` keeps `reqwest`'s
  default policy, which is now documented beside that constructor.

- **`Retry-After` is clamped to `MAX_RETRY_AFTER`, one day.** It is a number
  the endpoint hands the caller expecting them to sleep for it, and the pattern
  in the errors reference does exactly that, so `Retry-After: 99999999999`
  parked a task for three thousand years. A gateway that meant milliseconds and
  wrote seconds does the same without meaning to. Only the `delay-seconds` form
  is read, and the `HTTP-date` form RFC 9110 also allows still reads as `None`,
  which callers already handle as "use your own backoff".

- **One turn's tool calls run eight at a time.** They all ran at once, and how
  many a turn requests is the model's choice, so a tool holding a socket or a
  file handle turned that choice into pressure on the caller's process. Nothing
  is refused or reordered: calls past the limit wait for a slot and answer in
  the order they were requested.

  The three above are one omission seen three times. Freyja bounds what an
  endpoint it has never met can make it *hold*, in `MAX_BODY_BYTES`,
  `MAX_FRAME_BYTES` and `MAX_STREAM_BYTES`, and it did not bound what that
  endpoint can make it *do*. These bound time, concurrency and where the
  credential travels.


- **The API key is marked sensitive under every auth style.** `bearer_auth`
  set the flag and `header` did not, so the guarantee held for `Auth::Bearer`
  and for neither of the two other ways a credential reaches a header. Two of
  the three shipped presets take their key through `Auth::Header`, so Anthropic
  and Gemini sent it unmarked while OpenAI did not. It now prints as
  `Sensitive` in any middleware that renders a `HeaderMap`, and HPACK sends it
  literal rather than indexing it into the dynamic table for the life of the
  connection. A value classified with `secret_header` is marked the same way.
  The code was unchanged since 0.1.0, so nothing regressed here, it was never
  right.

- **`EndpointConfig::url` withholds a credential-shaped query value.** A
  `secret_query` value was redacted in `Debug` and in transport errors and
  printed in full by `url()`, which is the one method documented as safe to
  print and the one people reach for to say where requests go. It now reads
  `REDACTED`, matching the predicate the other two renderings use, so a
  credential is hidden in all three or none. The URL a request reaches is
  unchanged. 0.3.0 shipped the claim that a classified value is withheld from
  everything Freyja prints, and this is that claim becoming true.

## 0.3.0 - 2026-08-31

Two themes. A conversation now lives behind a `Storage` backend rather than in
a vector the caller threads through every call, and the transcript that reaches
a provider is repaired rather than assumed well formed. Separately, an endpoint
can describe a URL and a credential its dialect did not anticipate.

Pre-1.0, so this breaks. The rename table in the README is the upgrade path.

### Added

- **`Storage`** is where a conversation lives: `load`, `append` and `clear`,
  each returning a boxed future so the trait stays `dyn`-compatible. One value
  is one conversation. `InMemoryStorage` implements it for this process, and
  `InMemoryStorage::window(n)` keeps pinned turns and the most recent `n` turn
  groups, applied inside `load` so the stored transcript is never shortened. A
  backend of your own may cut anywhere, because the repair pass drops both
  halves of a pair a cut separated.

- **`Conversation`**, from `agent.conversation(storage)`. `send` takes anything
  that becomes a `Message`, including a bare `&str`, so `send("hi")` and
  `send(Message::new(..))` are one method. `Conversation::clear` empties the
  backend and leaves the conversation usable: the next `send` loads an empty
  transcript and continues with the same agent, backend and window.
  `Storage::clear` now documents that an implementation must succeed on an
  already-empty conversation, because `Conversation::clear` tells callers a
  retry is safe, and that sentence is only true of backends honouring it.

- **`Agent::system`**, plus `model`, `max_tokens`, `temperature`, `top_p`,
  `reasoning_effort`, `tool_choice` and `extra_for` directly on the agent.

- **`EndpointConfig::path` and `EndpointConfig::query`.** `path` replaces the
  path the dialect would append, for a deployment-scoped URL that resembles
  neither the dialect nor the vendor. `query` pins a parameter on every
  request, percent-encoded, with the joining done here so a URL never grows a
  second `?`.

- **`Auth::Query(name)`**, for an endpoint that takes its key in the URL. The
  key still comes from `api_key_env` or `Client::new`, and only the
  presentation changes. It is applied when the request is sent rather than when
  the URL is built, so `EndpointConfig::url` stays free of credentials and safe
  to print. A `query` entry of the same name is replaced.

- **`secret_header` and `secret_query`**, for a credential a gateway wants
  beside the key. Identical to `header` and `query` on the wire; the difference
  is that the value is withheld from `Debug` and from error messages whatever
  its name looks like. The classified names are readable as `secret_headers`
  and `secret_query`, and `is_secret_header` and `is_secret_query` answer the
  whole question, which is wider than either set because the name heuristic
  still applies underneath.

### Changed

- **`Storage`'s methods take `&mut self`.** A conversation owns its backend, so
  no interior mutability is required and the bound is `Send` rather than
  `Send + Sync`.

- **A header named by two layers goes on the wire once.** `reqwest`'s `header`
  appends rather than replaces, so a name written by both the dialect and the
  endpoint went out twice, and a server rejecting the request said nothing
  about which copy it read. Later wins among required and extra headers, and
  `auth` outranks both.

- **URLs are assembled through `reqwest::Url`** rather than concatenated, so a
  `base_url` carrying a query keeps it and the path lands before it rather than
  inside it. `Dialect::stream_query` returns a `(name, value)` pair instead of
  a preformatted `alt=sse`, which is why nothing in the crate formats a `?` any
  more.

- **`Auth` is `#[non_exhaustive]`.** A downstream `match` on it needs a
  wildcard arm.

- **`EndpointConfig`'s `Debug` withholds more.** Query parameter values are
  covered as well as headers, since `?key=<secret>` is how some endpoints take
  credentials, and headers and query parameters are asked separately, so
  classifying one name says nothing about the other.

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

### Removed

- **`Memory`, `MemoryError`, `MemoryFuture`, `Filter`, `FilterError`,
  `FilterFuture` and `Window`.** A filter named `Memory` stored nothing and
  remembered nothing between calls, while every neighbour in this ecosystem
  puts retention behind that word. Trimming is now something a `Storage`
  backend does inside its own `load`.

- **`Agent::run`, `Agent::run_with`, `Agent::chat`, `Agent::messages`,
  `Agent::memory`, `Agent::filter`, `Agent::conversation_in` and
  `Conversation::window`.** All reachable through `agent.conversation(..)`,
  with the backend always named so nothing chooses one for you.

- **`Arc<dyn Storage>` and `impl Storage for Arc<T>`**, unnecessary once
  `Storage` takes `&mut self`.

- **`split` and `window_by_groups` from the public API.** A backend may cut
  anywhere, so it does not need them.

### Fixed

- **A system instruction set on an `Agent` request template now reaches the
  model.** The template's `messages` and `tools` were overwritten on every
  turn, so a prompt written there was discarded with no error. All three agent
  examples did exactly that, so all three shipped a prompt the model never saw.
  `Agent::system` is the working path.

- **A pinned turn written inside a tool exchange stays in that exchange.** A
  `System` or `Developer` message between a call and its result used to close
  the open group, leaving the result in a group with no call in it, which a
  window could then keep on its own. Every provider rejects that transcript.

- **The repair pass checks ordering, not just presence.** It used to confirm a
  call and its result both existed somewhere. A backend returning a result
  ahead of its call still produced a transcript providers reject. A result is
  now kept only when its call appears strictly earlier, and a call with no
  later result is dropped, since it would be left unanswered.

- **Repair judges the transcript that will be sent.** It ran on what `load`
  returned, before the caller's turn was pushed, so a trailing unanswered tool
  call was deleted one step before the caller supplied the answering result,
  which is exactly the human-in-the-loop approval shape.

- **A `base_url` carrying a query no longer produces a malformed URL.** The
  dialect path was concatenated after the query, landing inside the parameter
  value, and Gemini streaming then appended a second `?`.

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

### Security

- **A whole streaming body is bounded, not only one frame.** `Client::generate`
  refused a body past 64 MiB while `Client::stream` refused only a frame past
  16 MiB, so an endpoint emitting well-formed frames forever, which is what a
  gateway stuck in a retry loop does, was accumulated without limit. Counted on
  arriving bytes, so nothing downstream can hold what never arrived.

- **A credential in the URL is withheld from transport errors.** `reqwest` puts
  the whole URL in its `Display` and Freyja put that in `Error::Http`'s
  message, so a key passed as a query parameter was printed in full by every
  transport failure. `reqwest` already strips userinfo, so the query was what
  remained. Redaction is unconditional rather than gated on the build profile,
  and covers what you classified, the parameter `Auth::Query` uses, and the
  name heuristic.

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
