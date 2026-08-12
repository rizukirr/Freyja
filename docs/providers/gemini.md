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
| `max_tokens` | yes | Sent as `generation_config.max_output_tokens` |
| `temperature`, `top_p` | yes | Nested inside `generation_config` |
| `reasoning_effort` | partly | `Low`, `Medium`, `High` only, as `generation_config.thinking_level`, see below |
| `response_format` | yes | The schema *is* `response_format`, see below |
| Tool declarations | yes | |
| `tool_choice` | yes | Sent as `generation_config.tool_choice`, see below |
| Tool round trip | yes | Verified live, requires thought-signature replay |
| `previous_response_id` | yes | Sent as `previous_interaction_id` |
| `metadata` | **no** | Rejected with `UnsupportedCapability`, see below |
| Usage reporting | yes | Field names normalized |
| Refusals | no | Not carried as a distinct block |
| Streaming | yes | `stream: true` **and** `?alt=sse` on the URL, which is what selects SSE. Verified live for text |

## Reasoning effort is nested, and partial

This API takes reasoning effort as `generation_config.thinking_level`, alongside the sampling controls, and accepts four values. Three of the six portable levels map straight across; the other three have no word here and are refused locally, because the endpoint rejects them by name.

| `ReasoningEffort` | `thinking_level` |
|---|---|
| `Low` | `"low"` |
| `Medium` | `"medium"` |
| `High` | `"high"` |
| `None` | refused |
| `Xhigh` | refused |
| `Max` | refused |

```
Gemini does not support reasoning effort 'max'
```

Verified against the live endpoint in both directions: the three that map returned answers, and the three that do not are rejected by the API with `'none' is not supported ... Supported values: 'minimal', 'low', 'medium', 'high'`.

Gemini's own `minimal` has no portable level to map from and is unreachable. It is the only level any of the three vendors accepts, so there is nothing portable to name it with.

Freyja refused this field outright until the endpoint was actually asked. The refusal was written from the wire format's top level, where `thinking_level` does not exist — the same mistake that sent `max_output_tokens`, `temperature`, and `top_p` loose. Nesting fixed those three and missed this one.

## Tool choice nests too, and takes two shapes

`generation_config.tool_choice` is either a bare mode or an object naming the tools the model may pick from. Freyja uses the first for three of the four portable levels and the second for `Named`:

| `ToolChoice` | Wire |
|---|---|
| `Auto` | `"auto"` |
| `None` | `"none"` |
| `Required` | `"any"` |
| `Named("add")` | `{"allowed_tools": {"mode": "any", "tools": ["add"]}}` |

Note `Required` is **not** `"required"` — that spelling comes back as `Invalid enum value 'required'`. The mode accepts `auto`, `any`, `none`, and `validated`, lowercase only.

This was the second refusal written from the top level of the request, where the field does not exist. Sent loose it answers `Unknown parameter 'tool_choice'`, which is what the old refusal was written from; nested, the same request answers `Invalid enum value`, which is a live field rejecting a value.

**What was verified, precisely.** All four shapes above were sent to the live endpoint and passed its parameter and enum validation, and every wrong spelling tried — `required`, `function`, `ANY`, `mode` as a sibling key, `allowed_tools` as an array — was rejected by name. That establishes the field exists and the shapes are well formed.

It does not establish behavior. No completion came back for these four: the free tier's daily request budget was spent on the probing that found the field. So **`Named` forcing that specific tool is inferred from the shape's own semantics, not observed.** If it turns out `allowed_tools` merely permits rather than compels, this row is what needs revisiting.

## One capability is rejected outright

```rust
// Fails before any network call.
GenerateRequest::new().metadata(serde_json::json!({"trace": "abc"}));
```

```
Gemini does not support request metadata
```

Freyja refuses rather than dropping the field, because a silently ignored field returns an answer that looks fine and is not what you asked for.

This one is not a gap in the wire format: the API has a `labels` field for exactly this purpose, and then declines to accept it.

```
The parameter 'labels' is not available on the Gemini API
but it is available on the Gemini Enterprise Agent Platform.
```

Freyja sent `labels` anyway until that was tried, so any request carrying `metadata` failed at the vendor after a round trip. It is refused locally now, which costs nothing and is caught by `Client::check`. If Google's Enterprise platform ever needs supporting, it is a different endpoint from the one this dialect targets and can be revisited then.

This is why `GenerateRequest::new()` sets no defaults. An earlier version defaulted `tool_choice` and `reasoning_effort`; both were refused at the time, so every default constructed request failed against Gemini. Both refusals turned out to be wrong, which is its own argument for setting only what you asked for.

## Input uses a step list, not turns

The Interactions API at `Api-Revision: 2026-05-20` rejects the older turn-based shape outright:

```
When using the steps-based API version, use step_list input format instead of turn_list.
```

So `input` is a flat array of typed steps, `user_input`, `model_output`, `function_call`, `function_result`, rather than an array of `{role, content}` turns. A single plain text user turn may still be sent as a bare string, which Freyja does automatically.

Gemini examples elsewhere that use `role` and `parts` target the older `generateContent` endpoint and do not apply. Full detail in [Gemini wire format](../reference/wire/gemini.md).

## Thought signatures must be replayed

A tool-calling response includes a `thought` step carrying an opaque `signature`. When you send the tool result back, that step has to come along unchanged and in position. Dropping it, or rebuilding the `function_call` without it, fails with `Request contains an invalid argument`.

Freyja handles this through `OutputContent::Reasoning`, which preserves any step it does not model, and `GenerateResponse::to_message()`, which carries it into the next request. As long as you append `response.to_message()` before your tool results, the round trip works.

This is the one place where a provider requirement reaches into the neutral model, and it is not Gemini specific. Anthropic thinking blocks and OpenAI reasoning items have the same property.

## Tool results need the tool name

A `function_result` must carry `call_id`, `name`, and a `result` that is an object or a string. The neutral `InputContent::ToolResult` only records the call id, so Freyja resolves the name from the matching `ToolCall` earlier in the transcript.

If no matching call is present, for instance because you are continuing through `previous_response_id` without replaying the call, the request fails locally with `InvalidRequest`:

```
invalid request for Gemini: no tool call with id 'call_1' in the transcript;
Gemini requires the tool name alongside its result
```

A bare number is also rejected as a `result`, so Freyja sends a JSON object through unchanged and anything else as a string.

## Field mapping

### Outbound

| Neutral | Wire |
|---|---|
| `model` | `model`, defaulting to `gemini-3.5-flash` |
| system and developer turns | `system_instruction`, joined with a blank line |
| other turns | `input` step list |
| `max_tokens` | `generation_config.max_output_tokens` |
| `temperature` | `generation_config.temperature` |
| `top_p` | `generation_config.top_p` |
| `response_format` | `response_format`, carrying the schema itself |
| `tools` | `tools`, each with `"type": "function"` |
| `previous_response_id` | `previous_interaction_id` |
| `metadata` | not sent; refused before the network |

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

The neutral model carries `arguments` and `output` as strings, but Gemini expects structured values. Freyja parses each one as JSON on the way out. Anything that is not valid JSON is sent as a JSON string rather than being rejected:

```rust
Message::tool_result("call_1", "42")          // sent as 42
Message::tool_result("call_1", "not json")    // sent as "not json"
```

Coming back, `arguments` arrives as a JSON value and Freyja stringifies it, so `OutputContent::ToolCall::arguments` is a string on every dialect.

## Status mapping

Gemini has two statuses OpenAI does not:

| Wire | Neutral |
|---|---|
| `budget_exceeded` | `ResponseStatus::Incomplete` |
| `cancelled` | `ResponseStatus::Failed` |

## Streaming needs the URL as well as the body

Gemini is the only dialect here where `stream: true` in the body is not enough. The Interactions API also takes `?alt=sse` on the URL, and that query parameter is what selects SSE framing. `Client::stream` appends it for you, which is why `ProviderConfig::stream_url` exists alongside `url`.

Frames repeat their event name inside the payload as `event_type`, so the SSE event line is redundant and Freyja reads the body. Steps arrive as `step.start` / `step.delta` / `step.stop`, with the interaction's terminal frame carrying id, model, status, and usage. Thought signatures stream in as deltas and are merged back into the step before it surfaces, so what you replay is what the API sent.

A text turn has been run against the live endpoint, `?alt=sse` and all: deltas arrived, usage landed on `Done`, and `into_response` rebuilt the same text. Streamed tool calls have not been. Their frame shapes come from Google's documentation and are tested against recorded fixtures, with `streamed_response_matches_generate` asserting that a drained stream matches what `generate` builds from the same turn — an offline parity test, so treat the live tool round trip above as covering `generate` only. See [Streaming](../reference/streaming.md).

## Rejected before the network

| Condition | Error |
|---|---|
| `reasoning_effort` set to `None`, `Xhigh`, or `Max` | `UnsupportedCapability` |
| Non text content in a system or developer turn | `UnsupportedCapability` |
| An image on a non user turn | `UnsupportedCapability` |
| Text content on a `Role::Tool` turn | `InvalidRequest` |

## Default model

`gemini-3.5-flash` is used when `model` is unset. It is the preset's `default_model` in `src/provider/presets.rs`, not a property of the dialect. Set `model` on the request, or `default_model` on the config, for anything you need to stay stable.
