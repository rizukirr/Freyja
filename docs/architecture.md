# Architecture

## Layout

```
src/
├── lib.rs                  # crate docs, public re-exports, #![deny(missing_docs)]
├── main.rs                 # runnable example, a one tool agent loop
└── provider/
    ├── mod.rs              # Provider trait, ProviderType, Client
    ├── model.rs            # the neutral request, response, and error types
    ├── openai/
    │   ├── mod.rs          # HTTP transport for /v1/responses
    │   └── types.rs        # wire types, TryFrom and From conversions
    └── gemini/
        ├── mod.rs          # HTTP transport for /v1beta/interactions
        └── types.rs        # wire types, TryFrom and From conversions
```

Provider modules are `pub(crate)`. Their wire types never escape the crate, so `OpenAiProvider`, `types::Request`, and everything like it are invisible to callers. The only thing consumers see is the neutral model.

## The three design rules

### The neutral model never bends to a vendor

`model.rs` does not know OpenAI, Gemini, or Anthropic exist. It describes generation in terms that make sense on their own, and every provider is responsible for reaching that shape.

Adding Anthropic tested this claim rather than restating it. The whole backend was one new module plus one `ProviderType` variant plus one match arm, with no edits to `model.rs` at all.

The alternative, letting whichever vendor you integrated first define the types, looks cheaper right up until the second provider arrives and every field turns out to be shaped wrong.

### No silent degradation

When a provider cannot express a capability, it returns `ProviderError::UnsupportedCapability` instead of dropping the field.

Dropping is worse than failing. A request with `tool_choice: Required` that silently becomes `Auto` still returns a plausible answer, and nothing in the response says the constraint was ignored. A refusal is loud, and loud is debuggable.

### No invented defaults

`GenerateRequest::new()` sets nothing. `None` means "the provider decides".

This was learned the hard way. An earlier version had `new()` default `tool_choice` to `Auto` and `reasoning_effort` to `Medium`, both fields Gemini rejects, so every default constructed request failed against Gemini before it reached the network. Populating a field the caller never asked for makes the request non portable, which defeats the point of a neutral model.

## Conversion

Each provider owns two conversions:

```rust
impl TryFrom<&GenerateRequest> for Request   // neutral to wire, can fail
impl From<Response> for GenerateResponse     // wire to neutral, cannot fail
```

The asymmetry is deliberate. Going out, a request may ask for something untranslatable, so the conversion is fallible. Coming back, the body already parsed, so anything unrecognized is preserved rather than rejected. Unknown fields go into `provider_metadata`, and unknown output or content types are skipped.

Validation lives in the outbound conversion, before any network call. Content in the wrong place, images on the wrong role, or a capability the backend lacks all fail locally.

## Transport

Transport is shared. `Client::run` does the same four steps for every dialect: convert, POST, check status, parse.

It did not start that way. Each provider used to own its own forty line copy, and this page said the duplication was intentional because a shared helper would need parameterizing over auth style, headers, and error attribution, then added "revisit it at four". Splitting endpoint from dialect is what made it worth doing, because those three parameters became fields on `ProviderConfig` rather than arguments to invent. The dialects now implement only `build` and `parse`, and the `Provider` trait has no transport method at all.

The `reqwest::Client` is passed into `Provider::generate` rather than owned by the provider, so every request in a process shares one connection pool:

```rust
pub trait Provider: Send + Sync {
    fn generate(
        &self,
        http: &reqwest::Client,
        api_key: &str,
        request: &GenerateRequest,
    ) -> impl Future<Output = Result<GenerateResponse, ProviderError>> + Send;
}
```

## Dispatch

`Client::generate` matches on `ProviderType` and calls the right implementation.

The trait uses return position `impl Trait`, so it is not object safe, which rules out `Box<dyn Provider>`. The match is fine at this scale: it is static dispatch, and adding a provider means adding one arm. If dynamic provider selection is ever needed at runtime, the trait would have to move to boxed futures.

## Tool calling across two wire shapes

The same neutral transcript maps onto two structurally different formats.

**OpenAI** uses a flat list of input items. Messages, tool calls, and tool results are all siblings:

```json
[
  {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "..."}]},
  {"type": "function_call", "call_id": "call_1", "name": "add", "arguments": "{...}"},
  {"type": "function_call_output", "call_id": "call_1", "output": "42"}
]
```

Since a neutral `Message` can hold text and a tool call together, the converter accumulates text and image parts and flushes them as a message item before emitting each tool item, which keeps transcript order intact.

**Gemini** uses a flat list of typed steps with no role field, where the step type carries the role:

```json
[
  {"type": "user_input", "content": [{"type": "text", "text": "..."}]},
  {"type": "thought", "signature": "..."},
  {"type": "function_call", "id": "...", "name": "add", "arguments": {"a": 20, "b": 22}},
  {"type": "function_result", "call_id": "...", "name": "add", "result": "42"}
]
```

**Anthropic** nests instead of flattening. Tool calls and results are content blocks inside a message rather than siblings of it, and only two roles exist on the wire:

```json
[
  {"role": "user", "content": [{"type": "text", "text": "..."}]},
  {"role": "assistant", "content": [
    {"type": "thinking", "thinking": "...", "signature": "..."},
    {"type": "tool_use", "id": "toolu_1", "name": "add", "input": {"a": 20, "b": 22}}]},
  {"role": "user", "content": [
    {"type": "tool_result", "tool_use_id": "toolu_1", "content": "42"}]}
]
```

All three reach the same neutral shape. This is the clearest case for keeping wire types private: the difference is invisible to callers.

It also shows why the flushing logic lives in the providers rather than the core. OpenAI and Gemini both need it, because a neutral `Message` holding text and a tool call has to be split across sibling items. Anthropic needs none of it, because the nesting preserves order for free. A core that had standardized on flattening would have been carrying machinery Anthropic does not want.

### Opaque state is carried, not modeled

One provider requirement does reach the neutral model. Reasoning models emit signed internal state and reject a follow-up request that drops it or rebuilds it by hand, so it cannot live in `provider_metadata` where it would be lost at the next turn.

`InputContent::Reasoning` and `OutputContent::Reasoning` hold that blob as an opaque `Value`. Freya never inspects it. Any output item a provider returns that Freya does not model becomes one of these, so it survives into the next request.

The alternative designs were a provider-side cache keyed by response id, rejected because it adds hidden state and breaks the moment a transcript is persisted or moved between processes, and relying on server-side continuation, rejected because it makes the agent loop behave differently per provider.

## Testing

Tests live beside the code in `#[cfg(test)]` modules, and every one is offline. They test conversion, which is where the logic is, not transport.

Coverage is currently 16 unit tests and 4 doctests:

- Neutral to wire mapping per provider
- Wire to neutral normalization per provider
- Capability rejection and malformed request rejection
- Full tool round trips, response to assistant turn to tool result to next request

There is no HTTP mocking, so the transport modules are not exercised by tests. A recording and replay layer plus a mock provider are Phase 5 work.

## Adding a provider

The core is stable enough that a third backend is additive. See [Adding a provider](providers/adding-a-provider.md).
