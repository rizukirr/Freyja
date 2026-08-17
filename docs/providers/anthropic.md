# Anthropic

Implemented against the Messages API.

| | |
|---|---|
| Endpoint | `POST https://api.anthropic.com/v1/messages` |
| Auth | `x-api-key: <key>` |
| Extra header | `anthropic-version: 2023-06-01` |
| Key variable | `ANTHROPIC_API_KEY` |
| Default model | `claude-opus-5` |
| Source | `src/provider/anthropic/` |

```rust
let client = Client::from_env(ProviderType::Anthropic).expect("ANTHROPIC_API_KEY");
```

Verified against the live endpoint: a full tool round trip completes, prompt to tool call to result to answer. See [Verification status](#verification-status) for what that does and does not cover.

## Capability support

| Capability | Supported | Notes |
|---|---|---|
| Text generation | yes | |
| Images in user turns | yes | URLs and `data:` URIs both handled |
| System and developer turns | yes | Hoisted into the top level `system` field |
| `max_tokens` | yes | **Required by the API**, defaulted when unset |
| `temperature`, `top_p` | yes | Forwarded, but rejected by newer models, see below |
| `reasoning_effort` | yes | Mapped onto `output_config.effort`, except `None` which disables thinking |
| `response_format` | partly | Schema only, `JsonObject` rejected |
| Tool declarations | yes | `parameters` becomes `input_schema` |
| `tool_choice` | yes | `Required` becomes `{"type": "any"}` |
| Tool round trip | yes | Nested in messages, not flat |
| `previous_response_id` | **no** | Rejected with `UnsupportedCapability` |
| `metadata` | yes | Forwarded unchanged |
| Usage reporting | yes | Total computed, cached tokens folded in |
| Refusals | partly | Surfaced as `ResponseStatus::Other("refusal")` |
| Streaming | yes | `stream: true`, decoded from `message_start` / `content_block_*` / `message_delta`. Dialect verified live for text, against a compatible endpoint |

## The one place Freyja invents a value

Anthropic is the only supported provider that requires `max_tokens` on every request. OpenAI and Gemini both treat it as optional, so Freyja's usual rule of never inventing a default cannot hold here.

When `max_tokens` is unset, Freyja sends `16000`. It is a cap and not a target, so the model still stops when it is finished, but it is a real number chosen by the library rather than by you or by the vendor. Set it explicitly on any request where the ceiling matters:

```rust
GenerateRequest::new()
    .message(Message::text(Role::User, "Hello"))
    .max_tokens(1024);
```

The constant lives in `src/provider/anthropic/types.rs`.

## Anthropic nests, the others do not

This is the structural difference that matters most when reading the code.

OpenAI puts messages, tool calls, and tool results side by side in one flat `input` list. Gemini does the same with a flat `step_list`. Anthropic instead nests everything inside a message: a tool call is a `tool_use` block inside an assistant turn, and a tool result is a `tool_result` block inside a user turn.

So the OpenAI and Gemini mappings both have a `flush` helper that emits accumulated text before each tool item, to keep transcript order intact across a flat list. The Anthropic mapping has no such helper, because order is already preserved by the nesting.

There are also only two roles on the wire, `user` and `assistant`. Everything else is either hoisted or collapsed, see [Roles](#roles).

## Sampling parameters may be rejected by the model

`temperature`, `top_p`, and `top_k` were removed on Claude Opus 5, Claude Fable 5, Claude Opus 4.8, and Claude Opus 4.7. Sending any of them to those models returns HTTP 400. Older models such as Claude Sonnet 4.5 still accept them.

This is a per model restriction rather than a per API one, and Freyja does not track which model supports what. So `temperature` and `top_p` are forwarded unchanged and the provider decides. If you get a 400 mentioning one of them, remove the field rather than changing the model.

```
Anthropic returned HTTP 400: {"type":"error","error":{"type":"invalid_request_error", ...}}
```

## Reasoning effort maps onto two different fields

| Neutral | Wire |
|---|---|
| `ReasoningEffort::None` | `"thinking": {"type": "disabled"}` |
| `Low`, `Medium`, `High`, `Xhigh`, `Max` | `"output_config": {"effort": "..."}` |

`None` is the one case that is not an effort level at all, it is an instruction to turn thinking off, so it lands on a different field. Every other level maps straight across.

Note that disabling thinking has its own hazards on Claude Opus 5: the model occasionally writes a tool call into its visible text instead of emitting a `tool_use` block, which means the call silently never runs. Prefer `Low` over `None` unless you have a specific reason.

## Response format is schema only

| Neutral | Wire |
|---|---|
| `ResponseFormat::Text` | omitted, this is the API default |
| `ResponseFormat::JsonObject` | rejected with `UnsupportedCapability` |
| `ResponseFormat::JsonSchema` | `"output_config": {"format": {"type": "json_schema", "schema": ...}}` |

Anthropic has no schema-less JSON mode, so `JsonObject` is refused rather than silently downgraded to free text.

The `name` and `strict` fields on `JsonSchema` are dropped, because Anthropic's structured outputs take neither. The name is a label with no effect on behaviour, and schema enforcement is inherent to the feature rather than opt in. Tool level `strict` is a separate thing and is forwarded.

## Thinking blocks must be replayed

A response from a reasoning model contains `thinking` blocks carrying an opaque `signature`, and `redacted_thinking` blocks carrying an opaque `data` field. When you continue the conversation on the same model, those blocks have to come back unchanged and in position. Editing or reconstructing one is rejected.

This is the same requirement Gemini has with thought signatures, and Freyja solves it the same way. Any block it does not model becomes `OutputContent::Reasoning`, and `GenerateResponse::to_message()` carries it into the next request untouched:

```rust
request = request
    .message(response.to_message())   // carries thinking blocks
    .extend_messages(tool_results);
```

Replaying a Claude thinking block to a *different* model is safe, the server drops it from the prompt rather than erroring, and you are not billed for it. So a provider swap mid conversation degrades quietly rather than failing.

## Usage has no total, and hides cached tokens

Anthropic reports four usage fields and no total:

```json
"usage": {
  "input_tokens": 10,
  "output_tokens": 5,
  "cache_creation_input_tokens": 100,
  "cache_read_input_tokens": 1000
}
```

`input_tokens` is the uncached remainder only, not the whole prompt. A long running agent whose prompt is cached reports a small `input_tokens` while actually having sent a large prompt.

Freyja normalizes this by summing all three prompt fields:

| Neutral | Computed as |
|---|---|
| `Usage::input_tokens` | `input_tokens + cache_creation_input_tokens + cache_read_input_tokens` |
| `Usage::output_tokens` | `output_tokens` |
| `Usage::total_tokens` | the sum of the two above |

The unsummed fields stay available through `provider_metadata` if you need to price cache reads and cache writes separately, which do not cost the same.

## Field mapping

### Outbound

| Neutral | Wire |
|---|---|
| `model` | `model`, defaulting to `claude-opus-5` |
| `max_tokens` | `max_tokens`, defaulting to `16000` |
| system and developer turns | `system`, joined with a blank line |
| other turns | `messages` |
| `temperature` | `temperature` |
| `top_p` | `top_p` |
| `reasoning_effort` | `output_config.effort` or `thinking` |
| `response_format` | `output_config.format` |
| `tools` | `tools`, with `parameters` renamed to `input_schema` |
| `tool_choice` | `tool_choice` |
| `metadata` | `metadata` |

### Inbound

| Wire | Neutral |
|---|---|
| `id` | `id` |
| `model` | `model` |
| `stop_reason` | `status` |
| `content[].text` | `OutputContent::Text` |
| `content[].tool_use` | `OutputContent::ToolCall` |
| every other block | `OutputContent::Reasoning` |
| `usage` | `Usage`, summed as described above |
| everything else | `provider_metadata` |

## Roles

| Neutral role | Wire role |
|---|---|
| `User` | `user` |
| `Assistant` | `assistant` |
| `Tool` | `user` |
| `System`, `Developer` | hoisted, no turn emitted |

Tool results ride on a user turn, the same convention Gemini uses. Consecutive same role turns are legal here and the API merges them, so a tool result immediately after a user message needs no special handling.

A turn that produces no content blocks is dropped rather than sent, because the API rejects an empty `content` array.

## Tool arguments are parsed

The neutral model carries `arguments` as a string, but Anthropic expects a structured object in `tool_use.input`. Freyja parses it on the way out.

Unlike Gemini, which accepts a bare string as a tool result and so can fall back to sending one, Anthropic requires `input` to be an object. Anything else fails locally rather than at the API:

```
invalid request for Anthropic: tool call arguments must be a JSON object;
Anthropic rejects anything else, got '42'
```

An empty or whitespace-only string is treated as `{}`, for tools that take no arguments.

Coming back, `input` arrives as a JSON value and Freyja stringifies it, so `OutputContent::ToolCall::arguments` is a string on every dialect.

## Status mapping

| Wire `stop_reason` | Neutral |
|---|---|
| `end_turn` | `ResponseStatus::Completed` |
| `stop_sequence` | `ResponseStatus::Completed` |
| `max_tokens` | `ResponseStatus::Incomplete` |
| `tool_use` | `ResponseStatus::RequiresAction` |
| `refusal` | `ResponseStatus::Other("refusal")` |
| `pause_turn` | `ResponseStatus::Other("pause_turn")` |
| anything else | `ResponseStatus::Other(String)` |

Two of these deliberately stay as `Other` rather than being flattened.

A `refusal` is not a `Failed`, the request succeeded and the model chose not to answer, and `content` may be empty. Check `status` before reading `content`, or you will read an empty response and think the model returned nothing.

A `pause_turn` is not a `RequiresAction` either. `RequiresAction` in Freyja means the model is waiting on a tool result you supply. A paused turn is resumed by re-sending the transcript unchanged, with no tool result involved, so treating it as `RequiresAction` would send an agent loop looking for tool calls that are not there.

Unlike Gemini, `has_tool_calls()` and `status` agree here: a response with tool calls always carries `stop_reason: "tool_use"`. `has_tool_calls()` is still the right loop condition, for consistency across providers.

## Rejected before the network

| Condition | Error |
|---|---|
| `previous_response_id` set | `UnsupportedCapability` |
| `response_format` is `JsonObject` | `UnsupportedCapability` |
| Non text content in a system or developer turn | `UnsupportedCapability` |
| An image on a non user turn | `UnsupportedCapability`, **unverified**: no key has been available to test it |
| Tool arguments that are not a JSON object | `InvalidRequest` |
| A malformed image data URI | `InvalidRequest` |

`previous_response_id` is refused because Anthropic keeps no server side transcript at all. Every request carries the full history, so there is nothing to continue from. If you are porting from OpenAI and relying on it, you need to keep the transcript yourself.

## Default model

`claude-opus-5` is used when `model` is unset. It is the preset's `default_model` in `src/provider/presets.rs`, not a property of the dialect. Set `model` on the request, or `default_model` on the config, for anything you need to stay stable.

## Verification status

This backend shipped unverified and was confirmed later, once a key was available. A full tool round trip now completes end to end:

```
tool call: add({"a":20,"b":22})
tool result: 42
assistant: 42
usage: 114 tokens
```

That single run covers more than it looks like. It exercises the nested message shape, the `tool_use` and `tool_result` block formats, `tool_use_id` correlation across turns, the invented `max_tokens` default, the summed usage mapping, and the `x-api-key` plus `anthropic-version` header pair.

What it does not cover, and what is therefore still only as good as the offline tests:

| | Status |
|---|---|
| Text generation and tool calling | verified live |
| Streaming, text | verified live, but against a compatible endpoint rather than Anthropic |
| Streaming, tool calls | **not** exercised live, on this or any dialect |
| Thinking block replay | **not** exercised, the run returned none |
| Images, both URL and data URI | not exercised |
| `response_format`, `reasoning_effort`, `tool_choice` | not exercised |
| Refusal and `pause_turn` handling | not exercised, and hard to trigger deliberately |

A text turn has been streamed live through this dialect, but against a Claude-compatible endpoint rather than Anthropic's own service — so the wire format is covered and the vendor is not. Deltas arrived, usage landed on `Done`, and `into_response` rebuilt the same text.

Streamed tool calls have not been run anywhere. The `message_start` / `content_block_*` / `message_delta` event shapes come from Anthropic's documentation and are tested against recorded fixtures, with `streamed_response_matches_generate` asserting that a drained stream matches what `generate` builds from the same turn. That is an offline parity test, not evidence the endpoint sends what Freyja expects. See [Streaming](../reference/streaming.md).

The thinking gap is the other one worth knowing about, since it is the failure mode that cost the most on Gemini. The replay path is shared with Gemini and covered by `preserves_thinking_blocks_in_place`, but a signed Anthropic block has never made a round trip.

To re-check after changes, point `examples/tool_loop.rs` at the endpoint and run it:

```sh
cargo run --example tool_loop
```
