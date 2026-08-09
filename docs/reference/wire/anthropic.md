# Anthropic wire format

The native JSON of the Anthropic Messages API, as Freyja speaks it. This page exists so you do not have to read Anthropic's documentation to understand what is going over the wire, or to debug a `ProviderError::Api` body.

> The format below is confirmed: a live tool round trip completes against this endpoint. The individual payloads are illustrative rather than captured verbatim, unlike the [OpenAI](../../reference/wire/openai.md) and [Gemini](../../reference/wire/gemini.md) pages. See [Verification status](../../providers/anthropic.md#verification-status) for what the live run did and did not cover.

## Endpoint

```http
POST https://api.anthropic.com/v1/messages
x-api-key: <key>
anthropic-version: 2023-06-01
Content-Type: application/json
```

`anthropic-version` is required on every request and is a date string, not a semantic version. `2023-06-01` is still the current value; it pins the response shape rather than the model generation.

A fourth header, `anthropic-beta`, gates preview features. Freyja sends none of them, so nothing here needs it.

## Request body

```json
{
  "model": "claude-opus-5",
  "max_tokens": 16000,
  "messages": [],
  "system": "Be concise",
  "temperature": 0.2,
  "top_p": 0.9,
  "output_config": { "effort": "high", "format": {} },
  "tools": [],
  "tool_choice": { "type": "auto" },
  "metadata": {}
}
```

`model`, `max_tokens`, and `messages` are required. Freyja omits every unset optional field rather than sending null.

**`thinking` is absent from that sample on purpose.** Freyja populates it in exactly one case: `ReasoningEffort::None` becomes `"thinking": {"type": "disabled"}`. Every other effort level goes to `output_config.effort` and leaves `thinking` unset. The two fields are never sent together, and Freyja never sends `{"type": "adaptive"}` or any `display` setting.

The request also carries `stream`, which `generate()` leaves unset and therefore off the wire. Every body on this page is byte-accurate for a `generate()` call. See [Streaming](#streaming).

Note the naming: `system` is a top level field rather than a message role, `max_tokens` is mandatory rather than optional, and there is no equivalent of OpenAI's `previous_response_id` or Gemini's `previous_interaction_id`. The API is fully stateless, so the whole transcript goes on every request.

## Messages nest, they do not flatten

This is the main structural difference from the flat formats, OpenAI Responses and Gemini. OpenAI Chat Completions nests too. Tool calls and tool results are content blocks *inside* a message rather than siblings of it.

```json
{
  "messages": [
    { "role": "user", "content": [
        { "type": "text", "text": "What is 20 + 22?" } ] },

    { "role": "assistant", "content": [
        { "type": "thinking", "thinking": "I should add these.", "signature": "ErUBCkYIBB..." },
        { "type": "text", "text": "Let me add those." },
        { "type": "tool_use", "id": "toolu_01A09q90qw90lq917835lq9",
          "name": "add", "input": { "a": 20, "b": 22 } } ] },

    { "role": "user", "content": [
        { "type": "tool_result", "tool_use_id": "toolu_01A09q90qw90lq917835lq9",
          "content": "42" } ] }
  ]
}
```

Compare this with OpenAI, where the same exchange is four flat `input` items, and Gemini, where it is four flat steps. Here it is three messages, each holding its own blocks.

One neutral `Message` maps to exactly one wire message, so Freyja needs no reordering pass. `content` may also be a bare string as a shorthand for a single text block, which Freyja does not use.

### Only two roles exist

| Role | Carries |
|---|---|
| `user` | text, images, `tool_result` blocks |
| `assistant` | text, `tool_use` blocks, `thinking` blocks |

There is no `system` role and no `tool` role. System instructions go in the top level `system` field, and tool results go on a `user` turn.

**The first message must be `user`.** Consecutive same role messages are legal and the API merges them into a single turn, so a tool result immediately following a user message needs no padding.

### Content block types

Input blocks Freyja sends:

```json
{ "type": "text",  "text": "hello" }
{ "type": "image", "source": { "type": "url", "url": "https://example.com/cat.png" } }
{ "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "iVBOR..." } }
{ "type": "tool_use",    "id": "toolu_...", "name": "add", "input": { "a": 20 } }
{ "type": "tool_result", "tool_use_id": "toolu_...", "content": "42" }
```

Images take a nested `source` object rather than a flat URL field, and the two source shapes are distinct. A `data:` URI is not a valid `url`, it has to be split into `media_type` and `data` and sent as `base64`. Freyja does that split for you.

`tool_result` also accepts `"is_error": true`, which tells the model the tool failed rather than returning that text as a result. Freyja does not model this, so a failing tool should return its error as ordinary output text.

## Tool calling

### Declaring tools

```json
"tools": [
  {
    "name": "add",
    "description": "adds two numbers together",
    "input_schema": {
      "type": "object",
      "properties": { "a": { "type": "integer" }, "b": { "type": "integer" } },
      "required": ["a", "b"]
    }
  }
]
```

The schema field is `input_schema`, not `parameters` as on OpenAI and Gemini, and there is no `"type": "function"` wrapper because custom tools are the default. Anthropic-defined server tools do carry a versioned `type`, for example `web_search_20260209`, and Freyja does not expose those.

Optional `"strict": true` guarantees the input validates exactly, and requires `additionalProperties: false` plus every property listed in `required`.

**Detailed descriptions matter more here than elsewhere.** Anthropic's own guidance is that under-description is the most common tool failure, and that recent Claude models reach for tools conservatively. Say when to call the tool, not just what it does.

### tool_choice

```json
"tool_choice": { "type": "auto" }
"tool_choice": { "type": "none" }
"tool_choice": { "type": "any" }
"tool_choice": { "type": "tool", "name": "add" }
```

All four are objects, unlike OpenAI where the first three are bare strings. `any` is Anthropic's spelling of "some tool, your pick", which is what `ToolChoice::Required` maps to.

Any of them also accepts `"disable_parallel_tool_use": true`, which caps the model at one tool call per response. By default it may request several.

### The call, as returned

```json
{
  "type": "tool_use",
  "id": "toolu_01A09q90qw90lq917835lq9",
  "name": "add",
  "input": { "a": 20, "b": 22 }
}
```

One id, not two. OpenAI returns both an item `id` and a correlation `call_id` and you must quote back the second; here `id` is the correlation handle and there is nothing else to confuse it with.

`input` is a **structured object**, like Gemini and unlike OpenAI. Freyja stringifies it so `OutputContent::ToolCall::arguments` behaves the same everywhere.

### The result, as sent back

```json
{ "type": "tool_result", "tool_use_id": "toolu_01A09q90qw90lq917835lq9", "content": "42" }
```

The field is `tool_use_id`, a third spelling after OpenAI's `call_id` and Gemini's `call_id`. Unlike Gemini, the tool's `name` is **not** required alongside the result, so Freyja does not need the transcript prepass that the Gemini mapping performs.

`content` is a string or an array of blocks. A bare number is not valid, but `"42"` is, so unlike Gemini there is no type restriction to work around beyond quoting it.

**Parallel tool calls must all be answered in one user message.** One assistant turn may contain several `tool_use` blocks; return every matching `tool_result` in a single following turn. Splitting them across turns is accepted but trains the model to stop making parallel calls. If a tool failed, still return a result for it with `"is_error": true` rather than omitting it.

## Thinking blocks must be replayed

This is the most important thing on this page, and it is the same requirement Gemini has.

A reasoning model returns thinking blocks in `content`:

```json
{ "type": "thinking", "thinking": "The user wants 20 + 22...", "signature": "ErUBCkYIBBgCIkAr..." }
{ "type": "redacted_thinking", "data": "EvwBCkYIBRgCKkBmMH..." }
```

When you continue the conversation on the same model, those blocks come back in `messages` **unchanged and in position**. The `signature` is what the API validates; editing the text, dropping the block, or rebuilding an equivalent by hand is rejected.

Reading the thinking text and displaying it is fine. Only modification is a problem.

Two details specific to Anthropic:

- **The text may be empty.** `thinking.display` defaults to `"omitted"` on current models, so blocks arrive with an empty `thinking` string. Replay them anyway; the signature is what matters. Readable text needs `"thinking": {"type": "adaptive", "display": "summarized"}` on the request, and **Freyja never sends that** — it only ever sends `thinking` to disable reasoning, see [Request body](#request-body). Through Freyja, expect the empty string.
- **Replaying to a different model is safe.** Other models drop the block from the prompt rather than erroring, and it is not billed. Gemini, by contrast, hard fails.

Freyja handles all of this with `OutputContent::Reasoning { data }`, which preserves any block it does not model, and `GenerateResponse::to_message()`, which carries it into the next request. Append `response.to_message()` before your tool results and it works. See [Tool calling](../../reference/tools.md).

## Response body

```json
{
  "id": "msg_01XFDUDYJgAACzvnptvVoYEL",
  "type": "message",
  "role": "assistant",
  "model": "claude-opus-5",
  "content": [
    { "type": "thinking", "thinking": "", "signature": "ErUBCkYIBBgCIkAr..." },
    { "type": "tool_use", "id": "toolu_01A09q90qw90lq917835lq9",
      "name": "add", "input": { "a": 20, "b": 22 } }
  ],
  "stop_reason": "tool_use",
  "stop_sequence": null,
  "stop_details": null,
  "usage": {
    "input_tokens": 42,
    "output_tokens": 15,
    "cache_creation_input_tokens": 0,
    "cache_read_input_tokens": 0
  }
}
```

Output arrives in `content`, not `output` as on OpenAI or `steps` as on Gemini. The top level shape is the same object you echo back as an assistant message, which is why the round trip is simpler here.

Everything Freyja does not model at the **top level**, including `type`, `role`, `stop_sequence`, and `stop_details`, stays reachable through `response.provider_metadata`. Nested fields do not: see [Usage](#usage) for what that costs.

### Output block types

| Type | Neutral mapping |
|---|---|
| `text` | `OutputContent::Text` |
| `tool_use` | `OutputContent::ToolCall` |
| `thinking`, `redacted_thinking` | `OutputContent::Reasoning` |
| `server_tool_use`, `*_tool_result`, `compaction`, `fallback`, anything else | `OutputContent::Reasoning` |

There is no refusal block. A refusal is signalled by `stop_reason` instead, see below.

### stop_reason

| Value | Meaning | Neutral `ResponseStatus` |
|---|---|---|
| `end_turn` | finished naturally | `Completed` |
| `stop_sequence` | hit a custom stop sequence | `Completed` |
| `max_tokens` | hit the output cap | `Incomplete` |
| `tool_use` | wants a tool executed | `RequiresAction` |
| `refusal` | declined on safety grounds | `Other("refusal")` |
| `pause_turn` | server tool loop paused, resumable | `Other("pause_turn")` |

Unlike OpenAI, where a pending tool call still reports `"status": "completed"`, `stop_reason` here is accurate: a response with tool calls always says `tool_use`. `has_tool_calls()` remains the right loop condition for portability, but on this provider `status` would work too.

**`refusal` returns HTTP 200 with an empty or partial `content`.** Code that reads `content[0]` unconditionally breaks on it. When `stop_reason` is `refusal`, `stop_details` carries a category such as `"cyber"`, `"bio"`, or `"reasoning_extraction"`, reachable through `provider_metadata`. It can be `null` even on a refusal, so branch on `stop_reason` and treat `stop_details` as informational.

**`pause_turn` is resumed by re-sending**, not by supplying a tool result. Append the assistant turn to the transcript and send the same request again. Do not add a "Continue" message; the API detects the trailing block and resumes on its own.

### Usage

```json
"usage": {
  "input_tokens": 10,
  "output_tokens": 5,
  "cache_creation_input_tokens": 100,
  "cache_read_input_tokens": 1000
}
```

**There is no `total_tokens` field**, and `input_tokens` is the *uncached remainder only*. The true prompt size is `input_tokens + cache_creation_input_tokens + cache_read_input_tokens`, which is 1110 in the example above rather than 10.

Freyja sums them into `Usage::input_tokens` and computes `total_tokens` itself.

**The three raw fields do not survive that.** `provider_metadata` is built by flattening the *unknown top-level keys* of the response body. `usage` is a named field, deserialized into a struct with no catch-all, so any subfield of it that Freyja does not map is dropped on the floor. `input_tokens`, `cache_creation_input_tokens`, and `cache_read_input_tokens` are read, summed, and then gone; only the sum is left.

That is worth knowing because the three are priced differently: a cache read costs roughly a tenth of an uncached input token, while a cache write costs roughly 1.25 times one. A Freyja `Usage` cannot tell you what a turn cost.

It also blocks the standard caching check. If `cache_read_input_tokens` is zero across requests that share a prefix, caching is silently not happening, usually because something volatile such as a timestamp sits early in the prompt. **You cannot run that check through Freyja** — the field is not in `provider_metadata` and not in `Usage`. Read the raw body, from a proxy or a direct `curl`, to see it.

## Streaming

`Client::stream()` sends the same body to the same URL with `"stream": true` added. There is no query parameter and no extra header. `generate()` leaves `stream` unset, which is why the bodies above are exactly what it puts on the wire.

The response is SSE, and **the event name is on the SSE `event:` line**, which is authoritative here; the JSON payload does not repeat it. That is the opposite of Gemini, where the name is in the body.

| Event | What the decoder does with it |
|---|---|
| `message_start` | Reads `message.id`, `message.model`, and the three `message.usage` input token fields, summed the same way the non-streaming parser sums them |
| `content_block_start` | Opens a block at `index`. `tool_use` starts a call, `thinking` starts a reconstruction, `text` is noted so its end can be marked, anything else is held whole to be replayed |
| `content_block_delta` | `text_delta` → text, `input_json_delta` → a fragment of tool arguments, `thinking_delta` → reasoning text, `signature_delta` → the thinking signature |
| `content_block_stop` | Closes the block at `index`. This is what separates two adjacent text blocks into two parts rather than one |
| `message_delta` | Carries `delta.stop_reason` and `usage.output_tokens` |
| `error` | Fails the stream as `ProviderError::Stream`, attributed to the endpoint's name |

Everything else, including `ping` and `message_stop`, is ignored.

`index` counts **content blocks**, not tool calls. Prose before a tool call pushes the call's index up, unlike the OpenAI Chat Completions dialect, where the index counts calls only.

Two details that bite when debugging:

**The token total is only knowable at the end.** `usage.input_tokens` arrives in `message_start`, `usage.output_tokens` in `message_delta`. Neither frame has both. Freyja holds the input count from the first and combines it at the second, which is why `StreamEvent::Done` is the only place a complete `Usage` exists.

**Thinking blocks are reconstructed, not echoed.** Streaming does not send the block whole. It sends `thinking_delta` text fragments and `signature_delta` signature fragments, and Freyja rebuilds `{"type": "thinking", "thinking": ..., "signature": ...}` from them at `content_block_stop`. Given that this page's headline requirement is replaying thinking blocks verbatim, that is worth stating plainly: on the streaming path "verbatim" means the reassembled block, which is byte-identical to the non-streaming one for the same turn and is what `into_response().to_message()` replays. Do not assemble it yourself from `StreamEvent::ReasoningDelta` text — that is the human-readable half, without the signature.

See [Streaming](../streaming.md).

## Errors

```json
{
  "type": "error",
  "error": { "type": "invalid_request_error", "message": "..." },
  "request_id": "req_011CSHoEeqs5C35K2UUqR7Fy"
}
```

Freyja preserves the whole body in `ProviderError::Api` alongside the HTTP status. It does not parse the body into typed variants yet, so branch on the status code and read `body` when you need the detail. See [Errors](../../reference/errors.md).

`error.type` is finer grained than the status code, for instance `billing_error` and `permission_error` both arrive as 403. Include `request_id` when reporting a problem to Anthropic, it traces the request end to end.

| Status | `error.type` | Retryable |
|---|---|---|
| 400 | `invalid_request_error` | no |
| 401 | `authentication_error` | no |
| 403 | `permission_error` | no |
| 404 | `not_found_error` | no |
| 413 | `request_too_large` | no |
| 429 | `rate_limit_error` | yes, honour `retry-after` |
| 500 | `api_error` | yes |
| 529 | `overloaded_error` | yes, with backoff |

529 is specific to Anthropic and means the service is temporarily saturated rather than broken. Back off and retry rather than failing the request.

## What Freyja does not send

`stop_sequences`, `top_k`, `container`, `mcp_servers`, `context_management`, `fallbacks`, `speed`, and `cache_control` are all left off. (`stream` used to be on this list and no longer is: it is part of the request, set by `Client::stream()`, and unset by `generate()`.) Prompt caching in particular is worth knowing about if your prompts are long and stable, and it is the most likely thing to be added next. See [Anthropic](../../providers/anthropic.md) for the capability table.
