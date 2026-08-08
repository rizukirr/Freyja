# Freya

A multi-LLM agent orchestration framework written from scratch in Rust.

> [!WARNING]
> This project is under active development and is **not ready for production use**.
> The public API is unstable and will change without notice before `0.1.0`.

Freya's goal is to be everything you need to build an AI agent in Rust: one neutral request/response model, pluggable providers, tools, memory, and an execution loop, with no hidden magic and no dependency on any single vendor's SDK.

---

## Documentation

Full reference lives in [`docs/`](docs/README.md), one page per feature: [getting started](docs/getting-started.md), [architecture](docs/architecture.md), [client](docs/client.md), [requests](docs/requests.md), [messages](docs/messages.md), [tool calling](docs/tools.md), [responses](docs/responses.md), [errors](docs/errors.md), and the provider pages for [OpenAI](docs/providers/openai.md), [Gemini](docs/providers/gemini.md), [Anthropic](docs/providers/anthropic.md), [OpenAI Chat Completions](docs/providers/openai-chat.md), [custom endpoints](docs/providers/custom-endpoints.md), and [adding a provider](docs/providers/adding-a-provider.md). The native wire formats are documented too, so you do not have to read vendor docs: [OpenAI wire format](docs/providers/openai-wire.md), [Gemini wire format](docs/providers/gemini-wire.md), [Anthropic wire format](docs/providers/anthropic-wire.md), and [OpenAI Chat wire format](docs/providers/openai-chat-wire.md).

## Table of contents

- [Documentation](#documentation)
- [Status](#status)
- [What works today](#what-works-today)
- [Quick start](#quick-start)
- [Architecture](#architecture)
- [Known issues](#known-issues)
- [Roadmap to MVP](#roadmap-to-mvp)
- [Development](#development)

---

## Status

| Area | State |
|---|---|
| Neutral request/response model | Stable |
| OpenAI provider (Responses API) | Implemented |
| Gemini provider (Interactions API) | Implemented and verified live, partial capability coverage |
| Anthropic provider (Messages API) | Implemented and verified live |
| OpenAI Chat Completions dialect | Implemented and verified live, no preset by design |
| Compatible endpoints | Reached with `Client::custom`, not shipped as presets |
| Function / tool calling | Full round trip, verified live on four endpoints |
| Pooled HTTP client, timeouts | Implemented |
| Rustdoc on the public API | `#![deny(missing_docs)]` |
| Streaming | Not started |
| Agent loop, memory, orchestration | Not started |

**Phase 0 is complete**, and the Anthropic backend proved it: adding a third dialect touched one new module and two enum arms, with no edits to the neutral model. `cargo test`: 54 unit tests + 7 doctests, all passing. OpenAI, Gemini, Anthropic, and a DeepSeek endpoint reached with `Client::custom` each complete a real tool round trip end to end. `cargo clippy --all-targets -- -D warnings` is clean.

---

## What works today

### Provider abstraction

- **Wire dialect and endpoint are separate types.** `ProviderDialect` is the JSON shape, `ProviderConfig` is where to send it, `ProviderType` is a preset that builds one. Most hosted APIs copy OpenAI or Anthropic, so a single dialect reaches many vendors on a base URL and a key.
- `Provider` trait with `build` and `parse`, no transport method. `Client` owns convert, POST, check, parse for every dialect, so a new dialect is roughly 25 lines of wiring.
- Vendor wire formats live behind private `types` modules, so the neutral model is the only thing consumers see.

### Neutral request model (`GenerateRequest`)

Builder-style, chainable, with `Default`:

| Field | Purpose |
|---|---|
| `model` | Override the provider's default model |
| `messages` | Conversation turns |
| `max_tokens` | Output token cap |
| `temperature`, `top_p` | Sampling controls |
| `reasoning_effort` | `None` … `Max`, seven levels |
| `response_format` | `Text`, `JsonObject`, or strict `JsonSchema` |
| `tools`, `tool_choice` | Function-calling declarations |
| `previous_response_id` | Server-side conversation continuation |
| `metadata` | Free-form provider metadata / labels |

Builder methods exist for every field: `model`, `message`, `messages`, `extend_messages`, `max_tokens`, `temperature`, `top_p`, `tools`, `tool_choice`, `reasoning_effort`, `response_format`, `previous_response_id`, and `metadata`.

`GenerateRequest::new()` sets **no** defaults. A `None` field means "the provider decides", Freya does not invent values, because a value that looks harmless on one provider may be rejected outright by another.

### Messages and content

- `Role`: `System`, `Developer`, `User`, `Assistant`, `Tool`.
- `InputContent`: `Text`, `ImageUrl`, `ToolCall { id, name, arguments }`, `ToolResult { call_id, output }`, and `Reasoning { data }` for opaque provider state.
- Constructors: `Message::new(role, parts)`, `Message::text(role, text)`, and `Message::tool_result(call_id, output)`.
- System/developer turns are automatically hoisted into the provider's native system-instruction field (`instructions` for OpenAI, `system_instruction` for Gemini, `system` for Anthropic) rather than being sent as ordinary turns.
- Misplaced content is rejected up front: images outside user turns, non-text in system turns, and text in tool turns all fail before a request leaves the process.

### Tools

- `ToolDefinition` with `name`, `description`, JSON Schema `parameters`, and an optional `strict` flag, built via `ToolDefinition::new(..).parameters(..).strict(..)`.
- `ToolChoice`: `Auto`, `None`, `Required`, `Named(String)`.
- Tool calls come back as `OutputContent::ToolCall { id, name, arguments }` with `arguments` as a raw JSON string, ready for the caller to dispatch.
- **The full round trip works.** `GenerateResponse::to_message()` turns the model's answer into the assistant turn, `Message::tool_result(id, output)` carries the result back, and each dialect maps both onto its own wire format, OpenAI's flat `function_call` items, Gemini's flat step list, Anthropic's nested `tool_use` and `tool_result` blocks. This is the prerequisite for every agent loop.

### Neutral response model (`GenerateResponse`)

- `id`, `model`, `status`, `content`, `usage`, `provider_metadata`.
- `OutputContent`: `Text`, `Refusal`, `ToolCall`, `Reasoning`.
- `ResponseStatus`: `Completed`, `Incomplete`, `RequiresAction`, `Failed`, `Other(String)`, provider status strings are normalized, unknown ones preserved.
- `Usage` with input/output/total token counts, normalized across providers.
- Helpers: `output_text()` concatenates all text parts, `tool_calls()` iterates `(id, name, arguments)`, `has_tool_calls()` short-circuits the loop, and `to_message()` folds the response back into the transcript.
- Unrecognized provider fields are captured into `provider_metadata` instead of being dropped, and unknown output/content variants are skipped rather than failing deserialization, so a provider adding a new block type doesn't break you.

### Errors

`ProviderError` covers the five real failure modes, all with provider attribution:

- `UnsupportedCapability { provider, capability }`, the request asked for something this provider can't express, refused up front instead of silently dropped.
- `InvalidRequest { provider, message }`, the request is malformed and was rejected before leaving the process.
- `Http(String)`, transport failure.
- `Api { provider, status, body }`, non-2xx with the raw body preserved.
- `InvalidResponse { provider, message }`, deserialization failure, with the body included for debugging.

Implements `Display` and `std::error::Error`.

### Transport

- One pooled `reqwest::Client` per `Client`, with a 120 second default timeout, connections are reused instead of rebuilt per request.
- `Client::with_http_client` to supply your own (custom timeouts, proxies, TLS).
- `Client::from_env(config)` reads the key from the endpoint's `api_key_env`, `Client::custom(dialect, name, base_url, key)` reaches an endpoint Freya does not ship in one call, and `Client::without_key` covers local runtimes that need none.
- `Client`'s `Debug` is hand written and **redacts the API key**, so `tracing::debug!(?client)` cannot leak a live credential into your logs.

---

## Quick start

```bash
# .env
OPENAI_API_KEY=sk-...
# GEMINI_API_KEY=...
# ANTHROPIC_API_KEY=sk-ant-...
# any other endpoint is reached with Client::custom, no key variable convention needed
```

```rust
use freya::{Client, GenerateRequest, Message, ProviderType, Role, ToolDefinition};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    let client = Client::from_env(ProviderType::OpenAi).expect("OPENAI_API_KEY");

    let add = ToolDefinition::new("add", "adds two numbers together")
        .parameters(serde_json::json!({
            "type": "object",
            "properties": { "a": {"type": "integer"}, "b": {"type": "integer"} },
            "required": ["a", "b"]
        }));

    let mut request = GenerateRequest::new()
        .message(Message::text(Role::User, "What is 20 + 22?"))
        .tools([add]);

    // A tool round trip: ask, run what the model requests, ask again.
    let response = client.generate(&request).await.unwrap();

    if response.has_tool_calls() {
        let results: Vec<Message> = response
            .tool_calls()
            .map(|(id, _name, arguments)| {
                let args: serde_json::Value = serde_json::from_str(arguments).unwrap();
                let sum = args["a"].as_i64().unwrap() + args["b"].as_i64().unwrap();
                Message::tool_result(id, sum.to_string())
            })
            .collect();

        request = request
            .message(response.to_message())
            .extend_messages(results);
    }

    println!("{}", client.generate(&request).await.unwrap().output_text());
}
```

`src/main.rs` contains a runnable version with a bounded multi-round loop and real error handling.

---

## Known issues

- **Live verification covers text and tool calling only.** Four endpoints complete a real tool round trip, but thinking block replay, images, `response_format`, and refusal handling are covered by offline tests alone. See each provider's verification section.
- **Anthropic requires `max_tokens`,** the only provider that does, so Freya defaults it to 16000 when unset. This is the one place the library invents a value rather than letting the provider decide.
- **Gemini rejects `tool_choice` and `reasoning_effort`.** Both are refused with `UnsupportedCapability` rather than silently dropped. Only hit when a caller explicitly asks for them.
- **Gemini tool results need the tool name**, which the neutral `ToolResult` does not carry, so Freya resolves it from the matching call in the transcript. Continuing through `previous_response_id` without replaying the call fails locally with `InvalidRequest`.
- **No streaming.** `generate` buffers the whole response.
- **No retries.** A 429 or 5xx surfaces as `Api { .. }` for the caller to handle.
- **No capability introspection.** You discover an unsupported capability by getting an error back, not by asking first. That is Phase 1.

---

## Roadmap to MVP

The MVP target: *everything you need to build an AI agent*, a developer can define tools, hand Freya a goal, and get a correct multi-step agent loop with memory, observability, and no vendor lock-in.

### Phase 0, Stabilize the core ✅ complete

- [x] Remove the capability defaults from `GenerateRequest::new()` so a default request is portable across providers
- [x] `cargo test` green, `cargo clippy --all-targets -- -D warnings` clean
- [x] Share one `reqwest::Client` per provider, with configurable timeouts
- [x] Add `Role::Tool`, `InputContent::ToolCall`, and `InputContent::ToolResult` so tool results can be fed back, the prerequisite for every agent loop
- [x] `GenerateResponse::to_message`, `tool_calls`, and `has_tool_calls` helpers
- [x] `ProviderError::InvalidRequest` for malformed requests caught before dispatch
- [x] Round-trip conversation tests on both providers
- [x] Rustdoc on every public item, enforced with `#![deny(missing_docs)]`
- [x] Verify both providers against the live API, prompt to tool call to result to answer
- [x] Add `InputContent::Reasoning` and `OutputContent::Reasoning` so opaque provider state (Gemini thought signatures, OpenAI reasoning items) is replayed verbatim instead of dropped
- [x] Correct the Gemini input format from `turn_list` to `step_list`, which had left every multi-turn Gemini conversation broken

### Phase 1, Production-grade provider layer

- [x] **Anthropic provider** (Messages API) as the third dialect, an additive change as predicted: one module and two enum arms, no edits to the neutral model
- [x] **Verify the Anthropic provider against the live API**, prompt to tool call to result to answer, the same bar the other two cleared
- [x] **Separate wire dialect from endpoint** so one mapping serves every compatible vendor, with `ProviderConfig` carrying base URL, auth, key variable, and default model
- [x] **OpenAI Chat Completions dialect**, the format the compatible ecosystem actually speaks, reached through `Client::custom` rather than shipped presets
- [ ] **Streaming**: `generate_stream` returning a `Stream<Item = StreamEvent>`
(text deltas, tool-call deltas, usage, completion)
- [ ] **Capability introspection**: `Provider::capabilities()` so callers can query
support instead of discovering it via `UnsupportedCapability` at runtime
- [ ] **Retries and backoff**: honor `Retry-After`, exponential backoff on 429/5xx
- [ ] **Typed API errors**: rate-limit / auth / context-length / content-filter variants
parsed out of `Api { body }`
- [ ] **Structured output ergonomics**: derive-based `schema_of::<T>()` and
`response.parse::<T>()` instead of hand-written JSON Schema
- [ ] **Configurable base URL** per provider (proxies, gateways, Azure, self-hosted)

### Phase 2, The agent

- [ ] **`Tool` trait**: a Rust function plus its schema, registered in a `ToolRegistry`,
invoked by name with JSON in / JSON out
- [ ] **`#[tool]` proc macro** deriving name, description, and JSON Schema from the
function signature and doc comment
- [ ] **`Agent`**: system prompt + model config + tool registry + memory
- [ ] **Agent loop**: call → detect tool calls → execute (concurrently) → append
results → repeat until `Completed`, bounded by max-steps and a token budget
- [ ] **Tool execution policy**: per-tool timeouts, parallel vs. serial, approval hooks
for side-effecting tools, error-to-model formatting so the agent can self-correct
- [ ] **`AgentResult`**: final output, full step trace, aggregate usage, stop reason

### Phase 3, Memory and context

- [ ] **`Memory` trait** with an in-memory conversation buffer as the default
- [ ] **Context-window management**: token counting, truncation and summarization
strategies when history exceeds the window
- [ ] **Persistent memory**: pluggable backends behind feature flags
- [ ] **Retrieval**: embeddings API in the provider trait, a `VectorStore` trait, and
a retrieval tool that plugs straight into the registry

### Phase 4, Orchestration (the namesake)

- [ ] **Multi-agent handoff**: an agent can delegate to another agent as a tool
- [ ] **Workflow primitives**: sequential chains, parallel fan-out/fan-in, routing,
and a supervisor that plans and dispatches sub-agents
- [ ] **Shared state** across agents in a run, with clear ownership rules
- [ ] **Cancellation and budgets** propagated through the whole tree
- [ ] **Human-in-the-loop**: pause, surface a decision, resume from a serialized run

### Phase 5, Observability and release

- [ ] **`tracing` instrumentation** on every request, tool call, and agent step
- [ ] **Cost accounting** from normalized usage, per model and per run
- [ ] **Recording and replay** of provider traffic for deterministic tests
- [ ] **Mock provider** for testing agents without network access
- [ ] **Examples**: chatbot, tool-using agent, RAG agent, multi-agent workflow
- [ ] **CI**: fmt, clippy `-D warnings`, tests, docs; publish `0.1.0` to crates.io

### Explicitly out of scope for MVP

Prompt-template DSLs, a built-in vector database, a web UI or server, fine-tuning orchestration, and framework-specific integrations. Freya is a library, not a platform.

---

## Development

```bash
cargo build
cargo test
cargo run          # runs the example in src/main.rs (needs OPENAI_API_KEY)
cargo clippy --all-targets
```

Rust edition 2024. Dependencies: `tokio`, `reqwest`, `serde`, `serde_json`, `dotenvy`.
