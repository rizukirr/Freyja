# OpenAI wire format

The native JSON of the OpenAI Responses API, as Freya speaks it. This page exists so you do not have to read OpenAI's documentation to understand what is going over the wire, or to debug a `ProviderError::Api` body.

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

Only `model` and `input` are required. Freya omits every unset field rather than sending null.

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

This flatness is the main structural difference from Gemini, which nests parts inside typed steps. One neutral `Message` holding both text and a tool call becomes two items here, and Freya splits it while preserving order.

### Content block types differ by role

| Role | Text block type |
|---|---|
| `user` | `input_text` |
| `assistant`, replayed as input | `output_text` |

Sending `input_text` on an assistant turn is wrong. Freya picks the right one from the role automatically.

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

A string for the first three, an object to name a specific tool. Freya maps `ToolChoice` onto these directly.

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

Two ids, and the distinction matters. `id` identifies the output item. `call_id` is the correlation handle you quote back in `function_call_output`. Freya exposes `call_id` as `OutputContent::ToolCall::id`.

`arguments` is a **JSON string**, not an object, the opposite of Gemini. Freya keeps it as a string, so parse it yourself.

### The result, as sent back

```json
{ "type": "function_call_output", "call_id": "call_i2JiY0kp8RK1lo0JvE1s4ywF", "output": "42" }
```

`output` is a string. Unlike Gemini there is no type restriction, so a bare number formatted as a string is fine.

The `function_call` item must be present in the transcript before its output. Freya emits both from `GenerateResponse::to_message()` plus `Message::tool_result()`.

## Reasoning items

Reasoning models emit `reasoning` items in `output`. Like Gemini's thought signatures, these are opaque and are expected back unchanged on the following request when the conversation continues with tool results.

Freya preserves any output item it does not model as `OutputContent::Reasoning { data }` and replays it verbatim, so this is handled without you doing anything. See [Tool calling](../tools.md).

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

Everything Freya does not model, and that is most of the above, stays reachable through `response.provider_metadata`.

### Output item types

| Type | Neutral mapping |
|---|---|
| `message` with `output_text` content | `OutputContent::Text` |
| `message` with `refusal` content | `OutputContent::Refusal` |
| `function_call` | `OutputContent::ToolCall` |
| anything else, including `reasoning` | `OutputContent::Reasoning` |

### Status is not a tool-call signal

The response above has a pending tool call and still reports `"status": "completed"`. OpenAI does not use `requires_action` here the way Gemini does.

This is why `response.has_tool_calls()` is the correct loop condition and `response.status` is not. See [Responses](../responses.md).

### Usage

```json
"usage": { "input_tokens": 42, "output_tokens": 15, "total_tokens": 57 }
```

Field names map straight onto the neutral `Usage`. Reasoning models add `output_tokens_details.reasoning_tokens`, reachable through `provider_metadata`.

## Errors

```json
{ "error": { "message": "Rate limit reached ...", "type": "rate_limit_error", "code": "rate_limit_exceeded" } }
```

Freya preserves the whole body in `ProviderError::Api` alongside the HTTP status. It does not parse the body into typed variants yet, so branch on the status code and read `body` when you need the detail. See [Errors](../errors.md).
