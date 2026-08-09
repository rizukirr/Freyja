# OpenAI wire format

The native JSON of the OpenAI Responses API, as Freyja speaks it. This page exists so you do not have to read OpenAI's documentation to understand what is going over the wire, or to debug a `ProviderError::Api` body.

The request shapes and the response payload below were captured from live calls.

## Endpoint

```http
POST https://api.openai.com/v1/responses
Authorization: Bearer <key>
Content-Type: application/json
```

This is the Responses API, not Chat Completions. The two are different endpoints with different bodies. If you find examples using `messages` and `choices`, they target `/v1/chat/completions` and do not apply here.

## Request body

```json
{
  "model": "gpt-5.6-sol",
  "input": [],
  "instructions": "Be concise",
  "max_output_tokens": 512,
  "temperature": 0.2,
  "top_p": 0.9,
  "reasoning": { "effort": "medium" },
  "text": { "format": { "type": "json_schema", "name": "person", "schema": {}, "strict": true } },
  "tools": [ { "type": "function", "name": "add", "description": "...", "parameters": {}, "strict": true } ],
  "tool_choice": "auto",
  "previous_response_id": "resp_...",
  "metadata": {}
}
```

Only `model` and `input` are required. Freyja omits every unset field rather than sending null.

The request also carries `stream`, which `generate()` leaves unset and which is therefore omitted rather than sent as `false`. Every body on this page is byte-accurate for a `generate()` call. See [Streaming](#streaming).

Note the naming: `instructions` rather than a system message, `max_output_tokens` rather than `max_tokens`, and response format nested under `text.format` rather than at the top level.

## Input is a flat item list

`input` is an array of items. Messages, tool calls, and tool results are all siblings at the top level. Nothing nests inside a message.

```json
{
  "input": [
    { "type": "message", "role": "user",
      "content": [ { "type": "input_text", "text": "What is 20 + 22?" } ] },

    { "type": "message", "role": "assistant",
      "content": [ { "type": "output_text", "text": "Let me add those." } ] },

    { "type": "function_call", "call_id": "call_i2JiY0kp8RK1lo0JvE1s4ywF",
      "name": "add", "arguments": "{\"a\":20,\"b\":22}" },

    { "type": "function_call_output", "call_id": "call_i2JiY0kp8RK1lo0JvE1s4ywF",
      "output": "42" }
  ]
}
```

This flatness is the main structural difference from Gemini, which nests parts inside typed steps. One neutral `Message` holding both text and a tool call becomes two items here, and Freyja splits it while preserving order.

### Content block types differ by role

| Role | Text block type |
|---|---|
| `user` | `input_text` |
| `assistant`, replayed as input | `output_text` |

Sending `input_text` on an assistant turn is wrong. Freyja picks the right one from the role automatically.

Images use a third type, and only on user turns:

```json
{ "type": "input_image", "image_url": "https://example.com/cat.png" }
```

A data URI works in the same field, which is how you send a local file.

## Tool calling

### Declaring tools

Note that the function fields are flat, not nested under a `function` key the way Chat Completions does it:

```json
"tools": [
  {
    "type": "function",
    "name": "add",
    "description": "adds two numbers together",
    "parameters": {
      "type": "object",
      "properties": { "a": { "type": "integer" }, "b": { "type": "integer" } },
      "required": ["a", "b"],
      "additionalProperties": false
    },
    "strict": true
  }
]
```

With `"strict": true`, OpenAI requires `additionalProperties: false` and every property listed in `required`. A schema that violates this returns HTTP 400.

### tool_choice

```json
"tool_choice": "auto"
"tool_choice": "none"
"tool_choice": "required"
"tool_choice": { "type": "function", "name": "add" }
```

A string for the first three, an object to name a specific tool. Freyja maps `ToolChoice` onto these directly.

Be careful with `required` inside a loop. It forces a tool call on **every** round, so the model can never produce a final answer and the loop runs until your bound stops it. Use `auto` for agent loops.

### The call, as returned

```json
{
  "id": "fc_0bd198ae8650b84d006a760bf371d08199b16d11b4c586b45b",
  "type": "function_call",
  "status": "completed",
  "call_id": "call_i2JiY0kp8RK1lo0JvE1s4ywF",
  "name": "add",
  "arguments": "{\"a\":20,\"b\":22}"
}
```

Two ids, and the distinction matters. `id` identifies the output item. `call_id` is the correlation handle you quote back in `function_call_output`. Freyja exposes `call_id` as `OutputContent::ToolCall::id`.

`arguments` is a **JSON string**, not an object, the opposite of Gemini. Freyja keeps it as a string, so parse it yourself.

### The result, as sent back

```json
{ "type": "function_call_output", "call_id": "call_i2JiY0kp8RK1lo0JvE1s4ywF", "output": "42" }
```

`output` is a string. Unlike Gemini there is no type restriction, so a bare number formatted as a string is fine.

The `function_call` item must be present in the transcript before its output. Freyja emits both from `GenerateResponse::to_message()` plus `Message::tool_result()`.

## Reasoning items

Reasoning models emit `reasoning` items in `output`. Like Gemini's thought signatures, these are opaque and are expected back unchanged on the following request when the conversation continues with tool results.

Freyja preserves any output item it does not model as `OutputContent::Reasoning { data }` and replays it verbatim, so this is handled without you doing anything. See [Tool calling](../../reference/tools.md).

The alternative is `previous_response_id`, which lets OpenAI keep the transcript server side so nothing needs replaying.

## Response body

Trimmed from a live call:

```json
{
  "id": "resp_0bd198ae8650b84d006a760bf26a248199b503c217f964bfe0",
  "object": "response",
  "model": "gpt-5.6-sol",
  "status": "completed",
  "created_at": 1786121202,
  "completed_at": 1786121203,
  "error": null,
  "incomplete_details": null,
  "instructions": null,
  "max_output_tokens": null,
  "parallel_tool_calls": true,
  "previous_response_id": null,
  "prompt_cache_retention": "24h",
  "reasoning": { "effort": "medium", "mode": "standard", "summary": null, "context": "all_turns" },
  "service_tier": "default",
  "store": true,
  "temperature": 1.0,
  "text": { "format": { "type": "text" }, "verbosity": "medium" },
  "output": [
    {
      "id": "fc_0bd198ae...",
      "type": "function_call",
      "status": "completed",
      "call_id": "call_i2JiY0kp8RK1lo0JvE1s4ywF",
      "name": "add",
      "arguments": "{\"a\":20,\"b\":22}"
    }
  ],
  "usage": { "input_tokens": 42, "output_tokens": 15, "total_tokens": 57 }
}
```

Every **top-level** field Freyja does not model, and that is most of the above, stays reachable through `response.provider_metadata`. Nested fields do not: `provider_metadata` is built by flattening the body's unknown top-level keys, so anything inside a named field such as `usage` or inside an `output` item is dropped once the field itself is deserialized. See [Usage](#usage).

### Output item types

| Type | Neutral mapping |
|---|---|
| `message` with `output_text` content | `OutputContent::Text` |
| `message` with `refusal` content | `OutputContent::Refusal` |
| `function_call` | `OutputContent::ToolCall` |
| anything else, including `reasoning` | `OutputContent::Reasoning` |

### Status is not a tool-call signal

The response above has a pending tool call and still reports `"status": "completed"`. OpenAI does not use `requires_action` here the way Gemini does.

This is why `response.has_tool_calls()` is the correct loop condition and `response.status` is not. See [Responses](../../reference/responses.md).

### Usage

```json
"usage": { "input_tokens": 42, "output_tokens": 15, "total_tokens": 57 }
```

Field names map straight onto the neutral `Usage`.

Reasoning models add `output_tokens_details.reasoning_tokens`, and **that field is discarded, not preserved**. `usage` is a named field deserialized into a struct holding exactly `input_tokens`, `output_tokens`, and `total_tokens`, with no catch-all, so any other subfield is dropped. It is not in `provider_metadata`, which only ever holds unknown *top-level* keys. Reasoning tokens are billed as output tokens and are included in `output_tokens`, so the total is right; the breakdown is what is gone. Read the raw body if you need it.

## Streaming

`Client::stream()` sends the same body to the same URL with `"stream": true` added. There is no query parameter and no extra header.

The response is SSE with **semantic event names on the `event:` line**, one per kind of change, rather than one repeating chunk shape. Freyja consumes these:

| Event | What the decoder does with it |
|---|---|
| `response.output_text.delta` | A fragment of text, from `delta` |
| `response.output_text.done` | Ends that text part. This is the block boundary for this dialect, so two `output_text` parts stay two parts |
| `response.refusal.delta` | A fragment of a refusal, kept separate from text |
| `response.reasoning_summary_text.delta` | A fragment of human-readable reasoning |
| `response.output_item.added` | Starts a tool call when `item.type` is `function_call`, taking `call_id` and `name` |
| `response.output_item.done` | Any item that is not `message` or `function_call`, `reasoning` above all, is preserved whole for replay |
| `response.function_call_arguments.delta` | A fragment of the arguments JSON string |
| `response.function_call_arguments.done` | Replaces the accumulated fragments with the authoritative `arguments` string and closes the call |
| `response.completed`, `response.incomplete`, `response.failed` | Terminal. Carries the whole `response` object: `id`, `model`, `status`, and `usage` |
| `error` | Fails the stream as `ProviderError::Stream`, attributed to the endpoint's name |

Every other event, and there are many, is ignored.

Calls are correlated by `output_index`, which each frame carries, so nothing has to be remembered between frames.

Note that `response.function_call_arguments.done` carries the complete `arguments` string and Freyja prefers it over the deltas it just assembled. If a stream's arguments look truncated, that frame is the one to check.

The terminal frame's `usage` is read strictly: this dialect's usage struct has no defaulted fields, so a partial `usage` object yields no `Usage` at all rather than zeros. That mirrors the non-streaming parser, which fails outright on the same input.

See [Streaming](../streaming.md).

## Errors

```json
{ "error": { "message": "Rate limit reached ...", "type": "rate_limit_error", "code": "rate_limit_exceeded" } }
```

Freyja preserves the whole body in `ProviderError::Api` alongside the HTTP status. It does not parse the body into typed variants yet, so branch on the status code and read `body` when you need the detail. See [Errors](../../reference/errors.md).
