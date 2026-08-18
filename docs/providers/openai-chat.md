# OpenAI Chat Completions

The dialect the compatible ecosystem speaks. One mapping, many endpoints.

| | |
|---|---|
| Path | `POST <base_url>/chat/completions` |
| Auth | `Authorization: Bearer <key>` |
| Extra header | none |
| Default model | none, comes from the endpoint |
| Source | `src/dialect/openai_chat/` |

This is not the same as the [OpenAI](../providers/openai.md) page. That one covers OpenAI's own Responses API, which is OpenAI-specific. This page covers Chat Completions, which almost every third party vendor implements, and which OpenAI also still serves.

## There is no preset for this dialect

Freyja ships presets only for the three first-party vendors it tests against. Every endpoint speaking this dialect is third party, so you point at it yourself:

```rust
let client = Client::custom(
    Dialect::OpenAiChat,
    "DeepSeek",
    "https://api.deepseek.com/v1",
    std::env::var("DEEPSEEK_API_KEY")?,
);
```

That is not a lesser path. A preset is only a `EndpointConfig` with the fields filled in, and this dialect works identically either way.

The reason is maintenance honesty rather than effort. A preset is a standing promise that a base URL and a default model are still current, and these vendors change both faster than this crate could verify. A stale preset fails at the vendor with a confusing 404; a missing one fails locally with a clear message, or does not fail at all because you supplied the current URL.

[Custom endpoints](../providers/custom.md) has a table of base URLs to start from, and covers keyless local runtimes.

## Capability support

| Capability | Supported | Notes |
|---|---|---|
| Text generation | yes | |
| Images, any role | yes | Sent as `image_url` parts. User, system, assistant, and tool all verified live |
| System and developer turns | yes | **Not hoisted**, they stay as messages |
| `max_tokens` | yes | `max_tokens` or `max_completion_tokens`, see below |
| `temperature`, `top_p` | yes | Forwarded unchanged |
| `reasoning_effort` | yes | Forwarded, support varies by endpoint |
| `response_format` | yes | All three variants map |
| Tool declarations | yes | Nested under a `function` key |
| `tool_choice` | yes | |
| Tool round trip | yes | Verified live against DeepSeek |
| `previous_response_id` | **no** | Rejected with `UnsupportedCapability` |
| `metadata` | yes | Forwarded unchanged |
| Usage reporting | yes | Field names normalized |
| Refusals | yes | From the message's `refusal` field |
| Streaming | yes | `stream: true`, plus `stream_options.include_usage` so token counts still arrive. Verified live for text |

## System turns are not hoisted

This is the one behavioural difference visible to callers, and the only place where Freyja's handling of system turns changes by dialect.

OpenAI Responses hoists them into `instructions`, Gemini into `system_instruction`, Anthropic into `system`. Here `system` is a real message role, so the turns stay in the array in the position you put them:

```json
{"messages": [
  {"role": "system", "content": "Be concise"},
  {"role": "user",   "content": "Hello"}
]}
```

`Role::Developer` maps onto `system` too. OpenAI's own newer models accept a `developer` role, but most compatible endpoints do not, and the portable spelling matters more here than the distinction.

Position therefore does matter on this dialect, where it does not on the other three. Keep system turns at the front.

## Every role maps to something different across dialects

`Role::Tool` gets a real `tool` role here, with a `tool_call_id`:

| Neutral | OpenAI Chat | Anthropic | Gemini |
|---|---|---|---|
| `Tool` | `tool` role | `user` turn | `user_input` step |

All four dialects handle it differently, which is reasonable evidence the neutral `Role::Tool` was worth having.

## One tool result per message

Anthropic packs several `tool_result` blocks into one user turn. Chat Completions gives each result its own message with a single `tool_call_id`, so a `Role::Tool` message carrying two results is rejected locally:

```
invalid request for DeepSeek: each tool message may answer only one tool call;
send one message per result
```

`Message::tool_result` builds exactly one per call, so the loop in `examples/tool_loop.rs` already does the right thing. This only bites if you hand-assemble a tool turn.

## Reasoning blocks are dropped, not replayed

This dialect has no standard place for opaque reasoning state, and no replay requirement either. DeepSeek exposes a `reasoning_content` field and explicitly says not to send it back.

So `InputContent::Reasoning` is **skipped** rather than rejected. That matters when a transcript moves between providers: a conversation started on Anthropic or Gemini carries signed blocks that this dialect cannot express, and refusing them would make switching endpoints mid-conversation impossible.

This is the one deliberate exception to the no-silent-degradation rule, and it is narrow. Freyja is dropping state the target format neither accepts nor requires, not a capability you asked for. Anthropic behaves the same way when a Claude thinking block reaches a different model.

## The token cap has two spellings

This format has had two fields for the output cap. `max_tokens` came first and is what the compatible ecosystem implements. OpenAI later replaced it with `max_completion_tokens`, and its newer models now **reject** the old spelling rather than ignoring it:

```
Unsupported parameter: 'max_tokens' is not supported with this model.
Use 'max_completion_tokens' instead.
```

That is a rejection of the field's *presence*, not its value, so sending both to cover the two spellings fails on exactly the endpoint that needs the new one. Freyja sends one or the other, chosen by the endpoint:

```rust
use freyja::{EndpointConfig, Dialect, TokenLimitField};

let config = EndpointConfig::new(
        Dialect::OpenAiChat,
        "OpenAI",
        "https://api.openai.com/v1",
    )
    .token_limit_field(TokenLimitField::MaxCompletionTokens);
```

The default is `max_tokens`, because this dialect is reached only through an explicit `EndpointConfig` — `EndpointPreset::OpenAi` is the Responses dialect — so by default it is serving one of the compatible vendors it exists for. Set the field when you point it at OpenAI itself.

Both spellings were verified against `api.openai.com`: the default is rejected there, and `MaxCompletionTokens` is accepted.

If OpenAI's own endpoint is the target, [the Responses API](../providers/openai.md) remains the better fit — it names its cap `max_output_tokens` and has no such history.

## Field mapping

### Outbound

| Neutral | Wire |
|---|---|
| `model` | `model`, from the request or the endpoint default |
| all turns including system | `messages` |
| `max_tokens` | `max_tokens`, or `max_completion_tokens` per `token_limit_field` |
| `temperature` | `temperature` |
| `top_p` | `top_p` |
| `reasoning_effort` | `reasoning_effort` |
| `response_format` | `response_format` |
| `tools` | `tools`, nested under `function` |
| `tool_choice` | `tool_choice` |
| `metadata` | `metadata` |

### Inbound

| Wire | Neutral |
|---|---|
| `id` | `id` |
| `model` | `model` |
| `choices[0].finish_reason` | `status` |
| `choices[0].message.content` | `OutputContent::Text` |
| `choices[0].message.refusal` | `OutputContent::Refusal` |
| `choices[0].message.tool_calls` | `OutputContent::ToolCall` |
| `usage.prompt_tokens` | `Usage::input_tokens` |
| `usage.completion_tokens` | `Usage::output_tokens` |
| everything else | `provider_metadata` |

Only the first choice is read. The neutral request has no way to ask for more than one, so there is never a second.

## Status mapping

| Wire `finish_reason` | Neutral |
|---|---|
| `stop`, absent | `Completed` |
| `length` | `Incomplete` |
| `tool_calls` | `RequiresAction` |
| `function_call` | `RequiresAction` |
| `content_filter` | `Other("content_filter")` |
| anything else | `Other(String)` |

`function_call` is the pre-2023 spelling and is still emitted by some compatible endpoints, so both map to `RequiresAction`.

`content_filter` stays as `Other` rather than becoming `Failed`, because the request succeeded and the endpoint chose to withhold part of the answer.

## Streaming asks for usage explicitly

A streaming request on this dialect gets no token counts unless it asks, so Freyja sends `stream_options: {"include_usage": true}` alongside `stream: true`. Without it the terminal frame carries no usage at all and `StreamEvent::Done` would arrive with `usage: None` on the most widely spoken dialect.

```json
{"model": "...", "messages": [...], "stream": true, "stream_options": {"include_usage": true}}
```

This dialect also has no end frame beyond `[DONE]`, so a tool call is complete only once the body closes. Freyja flushes pending calls at that point, which is why `StreamEvent::ToolCall` can arrive later here than on the other dialects. See [Streaming](../reference/streaming.md).

## Rejected before the network

| Condition | Error |
|---|---|
| `previous_response_id` set | `UnsupportedCapability` |
| A tool message answering more than one call | `InvalidRequest` |
| No model on the request and none on the endpoint | `InvalidRequest` |

## Verification status

Verified live against DeepSeek, full tool round trip:

```
assistant: I'll add those numbers together for you.
tool call: add({"a": 20, "b": 22})
tool result: 42
assistant: 20 + 22 = 42.
usage: 378 tokens
```

That covers the message array, the nested `function` tool schema, `tool_calls` on the assistant message, the `tool` role with `tool_call_id`, and the usage field names.

What it does not cover:

| | Status |
|---|---|
| Text generation and tool calling | verified live, on DeepSeek only |
| Streaming, text | verified live against DeepSeek |
| Streaming, tool calls | **not** exercised live, on this or any dialect |
| Images | an `image_url` part accepted on a user, system, assistant, and tool turn |
| `response_format`, `reasoning_effort` | not exercised, on any endpoint |

What one endpoint cannot tell you is how the others behave. "Compatible" is a spectrum, and Groq, Together, OpenRouter, and Ollama are each unverified here.

A text turn has been streamed live through this dialect against DeepSeek: 191 deltas, usage on `Done` via `stream_options.include_usage`, and `into_response` rebuilding the same text. That is one compatible endpoint out of many, and the risk stays highest here, because "compatible" endpoints diverge most in their streaming frames.

Streamed tool calls have not been run anywhere. The frame shapes and the `[DONE]` sentinel come from vendor documentation and are tested against recorded fixtures, with `streamed_response_matches_generate` asserting that a drained stream matches what `generate` builds from the same turn — an offline parity test.
