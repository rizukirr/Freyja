# Gemini wire format

The native JSON of the Gemini Interactions API, as Freya speaks it. This page exists so you do not have to read Google's documentation to understand what is going over the wire, or to debug a `ProviderError::Api` body.

Everything here was verified against the live endpoint at `Api-Revision: 2026-05-20`. Where behavior was surprising, it is called out.

## Endpoint

```http
POST https://generativelanguage.googleapis.com/v1beta/interactions
x-goog-api-key: <key>
Api-Revision: 2026-05-20
Content-Type: application/json
```

The `Api-Revision` header selects the API generation. It is what puts the endpoint in steps mode, described below.

## Request body

```json
{
  "model": "gemini-3.5-flash",
  "input": "...",
  "system_instruction": "Be concise",
  "max_output_tokens": 512,
  "temperature": 0.2,
  "top_p": 0.9,
  "response_format": { "type": "json_schema", "name": "person", "json_schema": {}, "strict": true },
  "tools": [ { "type": "function", "name": "add", "description": "...", "parameters": {} } ],
  "previous_interaction_id": "v1_...",
  "labels": {}
}
```

Only `model` and `input` are required. Freya omits every unset field rather than sending null.

Note the naming: `system_instruction` rather than a system turn, `max_output_tokens` rather than `max_tokens`, `previous_interaction_id` rather than `previous_response_id`, and `labels` rather than `metadata`.

## Input takes two shapes

### A bare string

For a single plain text user turn, `input` is just a string:

```json
{ "model": "gemini-3.5-flash", "input": "What is 20 + 22?" }
```

Freya uses this automatically when the conversation is one text-only user message.

### A step list

Anything longer is an array of **steps**, not turns. This is the part most likely to trip you up.

```json
{
  "input": [
    { "type": "user_input",   "content": [ { "type": "text", "text": "What is 20 + 22?" } ] },
    { "type": "thought",      "signature": "EvACCu0CARFNMg..." },
    { "type": "function_call", "id": "mbbykw8q", "name": "add", "arguments": { "a": 20, "b": 22 } },
    { "type": "function_result", "call_id": "mbbykw8q", "name": "add", "result": "42" },
    { "type": "model_output", "content": [ { "type": "text", "text": "The answer is 42." } ] }
  ]
}
```

Each step is a flat object with a `type`. There is no `role` field, the step type carries that meaning: `user_input` for the user, `model_output` for the model.

**The older turn-based shape is rejected.** Sending `[{"role": "user", "content": [...]}]` returns:

```
When using the steps-based API version, use step_list input format instead of turn_list.
```

If you find Gemini examples online using `role` and `parts`, they target the older `generateContent` endpoint, not this one.

### Step types accepted in input

The API reports the full set when you send an invalid one:

```
transcription, document, google_search_result, google_search_call, function_result,
google_maps_result, text, file_search_call, video, playback_awaiting,
url_context_result, retrieval_result, code_execution_call, retrieval_call,
url_context_call, elicitation_call, file_search_result, model_output,
code_execution_result, function_call, user_input, mcp_server_tool_result,
compaction, audio, audio_truncation, google_maps_call, elicitation_result,
playback_interruption, content, mcp_server_tool_call, image, thought,
playback_complete
```

Freya emits five of these: `user_input`, `model_output`, `function_call`, `function_result`, and whatever opaque steps it replays, in practice `thought`.

### Content part types

Inside `user_input` and `model_output`, `content` is an array of parts:

```json
{ "type": "text",  "text": "hello" }
{ "type": "image", "uri": "https://example.com/cat.png" }
```

The accepted part types are `text`, `image`, `audio`, `video`, `document`, `thought`, `function_call`, `function_result`, and the various search, maps, code execution, file search, url context, and MCP result types.

## Tool calling

### Declaring tools

```json
"tools": [
  {
    "type": "function",
    "name": "add",
    "description": "adds two numbers together",
    "parameters": {
      "type": "object",
      "properties": { "a": { "type": "integer" }, "b": { "type": "integer" } },
      "required": ["a", "b"]
    }
  }
]
```

### The call, as returned

```json
{ "type": "function_call", "id": "mbbykw8q", "name": "add", "arguments": { "a": 20, "b": 22 } }
```

`arguments` is a **structured object**, not a JSON string. Freya stringifies it so `OutputContent::ToolCall::arguments` behaves the same across providers.

### The result, as sent back

```json
{ "type": "function_result", "call_id": "mbbykw8q", "name": "add", "result": "42" }
```

Three requirements, each of which the API enforces:

| Field | Rule | Error if wrong |
|---|---|---|
| `call_id` | Required. Not `id` | `Unknown parameter 'id'` / `Missing call_id in content of type function_result` |
| `name` | Required. The tool's name, repeated | `Missing name in content of type function_result` |
| `result` | Must be an object, a `FunctionResultSubcontent[]`, or a string | `'result' must be a Struct, FunctionResultSubcontent[], or string` |

That last one catches people out. A bare number or boolean is rejected, so `42` fails and `"42"` succeeds. Freya sends a JSON object through unchanged and sends anything else as a string.

### Ordering is enforced

```
Please ensure that function call turn comes immediately after a user turn
or after a function response turn.
```

A `function_call` step cannot open a conversation or follow arbitrary steps.

## Thought signatures must be replayed

This is the most important thing on this page.

A response to a tool-calling prompt contains a `thought` step carrying an opaque `signature`:

```json
"steps": [
  { "type": "thought", "signature": "EvACCu0CARFNMg+gcIqp9LOeN37O777Mv012FS4SXRUGoRN1n6+4CMDZ0Rr..." },
  { "type": "function_call", "id": "mbbykw8q", "name": "add", "arguments": { "b": 22, "a": 20 } }
]
```

When you send the tool result back, that `thought` step must be included, verbatim, in the same position. Measured behavior:

| Input | Result |
|---|---|
| Model's steps echoed back unchanged | `completed`, correct answer |
| `thought` step dropped | `Request contains an invalid argument` |
| `function_call` rebuilt by hand, no signature | `Request contains an invalid argument` |

A semantically identical call is not good enough. The signature is what the API validates, and it cannot be reconstructed.

Freya handles this with `OutputContent::Reasoning { data }`, which preserves any step it does not model, and `GenerateResponse::to_message()`, which carries it into the next request. As long as you append `response.to_message()` before your tool results, it works. See [Tool calling](../tools.md).

## Response body

```json
{
  "id": "v1_ChczQWgyYXVuck9adnZnOFVQOWFydG1Bdx...",
  "object": "interaction",
  "model": "gemini-3.5-flash",
  "status": "requires_action",
  "created": "2026-08-07T16:33:32Z",
  "updated": "2026-08-07T16:33:32Z",
  "service_tier": "standard",
  "steps": [
    { "type": "thought", "signature": "..." },
    { "type": "function_call", "id": "mbbykw8q", "name": "add", "arguments": { "b": 22, "a": 20 } }
  ],
  "usage": {
    "total_tokens": 190,
    "total_input_tokens": 67,
    "total_output_tokens": 18,
    "total_cached_tokens": 0,
    "total_tool_use_tokens": 0,
    "total_thought_tokens": 105,
    "raw_prompt_token": 107,
    "input_tokens_by_modality": [ { "modality": "text", "tokens": 67 } ]
  }
}
```

Output arrives in `steps`, not `output` or `candidates`. Text lives inside a `model_output` step's `content` array.

### Status values

| Value | Neutral `ResponseStatus` |
|---|---|
| `completed` | `Completed` |
| `incomplete` | `Incomplete` |
| `budget_exceeded` | `Incomplete` |
| `requires_action` | `RequiresAction` |
| `failed` | `Failed` |
| `cancelled` | `Failed` |
| anything else | `Other(String)` |

### Usage

Gemini reports more detail than the neutral `Usage` models. Freya maps `total_input_tokens`, `total_output_tokens`, and `total_tokens`, and the rest, including `total_thought_tokens` and the per-modality breakdown, stays available through `response.provider_metadata`.

Note that thinking tokens are billed. The 190 total above includes 105 thought tokens for a one-line arithmetic question.

## Errors

```json
{ "error": { "message": "Unknown parameter 'id' at 'input[2].content[0]'.", "code": "invalid_request" } }
```

The messages are precise and include a JSON path, which makes them the fastest way to debug a mapping problem. Freya preserves the whole body in `ProviderError::Api`, so nothing is lost.

The exception is `Request contains an invalid argument`, a generic protobuf-level rejection with no path. In practice that one usually means a missing or malformed thought signature.

## What Freya does not send

`reasoning_effort` and `tool_choice` are refused with `UnsupportedCapability` before the request is built, because no portable mapping onto this API has been established. See [Gemini](gemini.md).
