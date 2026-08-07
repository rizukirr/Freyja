# Gemini

Implemented against the Interactions API.

| | |
|---|---|
| Endpoint | `POST https://generativelanguage.googleapis.com/v1beta/interactions` |
| Auth | `x-goog-api-key: <key>` |
| Extra header | `Api-Revision: 2026-05-20` |
| Key variable | `GEMINI_API_KEY` |
| Default model | `gemini-3.5-flash` |
| Source | `src/provider/gemini/` |

```rust
let client = Client::from_env(ProviderType::Gemini).expect("GEMINI_API_KEY");
```

This backend is less complete than OpenAI. Read the gaps below before relying on it.

## Capability support

| Capability | Supported | Notes |
|---|---|---|
| Text generation | yes | |
| Images in user turns | yes | Sent as `{"type": "image", "uri": ...}` |
| System and developer turns | yes | Hoisted into `system_instruction` |
| `max_tokens` | yes | Sent as `max_output_tokens` |
| `temperature`, `top_p` | yes | Forwarded unchanged |
| `reasoning_effort` | **no** | Rejected with `UnsupportedCapability` |
| `response_format` | yes | Mapped onto `response_format` |
| Tool declarations | yes | |
| `tool_choice` | **no** | Rejected with `UnsupportedCapability` |
| Tool round trip | unverified | Mapping is implemented but untested against the live API |
| `previous_response_id` | yes | Sent as `previous_interaction_id` |
| `metadata` | yes | Sent as `labels` |
| Usage reporting | yes | Field names normalized |
| Refusals | no | Not parsed as a distinct block |
| Streaming | no | Not implemented in Freya |

## Two capabilities are rejected outright

```rust
// Both of these fail before any network call.
GenerateRequest::new().reasoning_effort(ReasoningEffort::High);
GenerateRequest::new().tool_choice(ToolChoice::Required);
```

```
Gemini does not support portable reasoning effort levels
Gemini does not support portable tool choice
```

Freya refuses rather than dropping the field, because a silently ignored `tool_choice: Required` returns an answer that looks fine and is not what you asked for.

This is why `GenerateRequest::new()` sets no defaults. An earlier version defaulted both fields, so every default constructed request failed against Gemini. If you need either capability, use OpenAI.

## The tool mapping is unverified

Tool calls and tool results are mapped onto per turn content parts:

```json
{"role": "model", "content": [
  {"type": "function_call", "id": "call_1", "name": "add", "arguments": {"a": 20, "b": 22}}
]}
{"role": "user", "content": [
  {"type": "function_result", "id": "call_1", "result": 42}
]}
```

This mirrors the shape of the response format Freya already parses, but it has **not been confirmed against a live endpoint**. The Interactions API and its `Api-Revision: 2026-05-20` are newer than the reference material this mapping was written from.

If Gemini rejects these parts, the fix is contained to `src/provider/gemini/types.rs`, in the `InputContent::ToolCall` and `InputContent::ToolResult` arms of `TryFrom<&GenerateRequest>`. Nothing in the neutral model or in the OpenAI backend is affected.

Verifying this is the one open item left in Phase 0.

## Field mapping

### Outbound

| Neutral | Wire |
|---|---|
| `model` | `model`, defaulting to `gemini-3.5-flash` |
| system and developer turns | `system_instruction`, joined with a blank line |
| other turns | `input` |
| `max_tokens` | `max_output_tokens` |
| `temperature` | `temperature` |
| `top_p` | `top_p` |
| `response_format` | `response_format` |
| `tools` | `tools`, each with `"type": "function"` |
| `previous_response_id` | `previous_interaction_id` |
| `metadata` | `labels` |

### Inbound

| Wire | Neutral |
|---|---|
| `id` | `id` |
| `model` | `model`, defaults to empty when absent |
| `status` | `status` |
| `steps[].model_output.content[].text` | `OutputContent::Text` |
| `steps[].function_call` | `OutputContent::ToolCall` |
| `usage.total_input_tokens` | `Usage::input_tokens` |
| `usage.total_output_tokens` | `Usage::output_tokens` |
| `usage.total_tokens` | `Usage::total_tokens` |
| everything else | `provider_metadata` |

Gemini reports its output as `steps` rather than `output`, and names its usage fields with a `total_` prefix. Both are normalized, so cost accounting does not branch per provider.

## Roles

| Neutral role | Wire role |
|---|---|
| `User` | `user` |
| `Assistant` | `model` |
| `Tool` | `user` |
| `System`, `Developer` | hoisted, no turn emitted |

Tool results ride on a user turn, matching the convention that anything not produced by the model is reported as user input.

## The single turn shortcut

When the whole conversation is one plain text user turn, `input` is sent as a bare string rather than an array:

```json
{"model": "gemini-3.5-flash", "input": "Hello", "system_instruction": "Be concise"}
```

Any additional turn, image, tool call, or tool result switches it to the full array form. This is transparent to callers.

## Tool arguments and results are parsed

The neutral model carries `arguments` and `output` as strings, but Gemini expects structured values. Freya parses each one as JSON on the way out. Anything that is not valid JSON is sent as a JSON string rather than being rejected:

```rust
Message::tool_result("call_1", "42")          // sent as 42
Message::tool_result("call_1", "not json")    // sent as "not json"
```

Coming back, `arguments` arrives as a JSON value and Freya stringifies it, so
`OutputContent::ToolCall::arguments` is a string on both providers.

## Status mapping

Gemini has two statuses OpenAI does not:

| Wire | Neutral |
|---|---|
| `budget_exceeded` | `ResponseStatus::Incomplete` |
| `cancelled` | `ResponseStatus::Failed` |

## Rejected before the network

| Condition | Error |
|---|---|
| `reasoning_effort` set | `UnsupportedCapability` |
| `tool_choice` set | `UnsupportedCapability` |
| Non text content in a system or developer turn | `UnsupportedCapability` |
| An image on a non user turn | `UnsupportedCapability` |
| Text content on a `Role::Tool` turn | `InvalidRequest` |

## Default model

`gemini-3.5-flash` is used when `model` is unset. It is a constant in `src/provider/gemini/types.rs`. Set `model` explicitly for anything you need to stay stable.
