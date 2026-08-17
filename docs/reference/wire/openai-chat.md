# OpenAI Chat Completions wire format

The native JSON of the Chat Completions API, as Freyja speaks it. This page exists so you do not have to read vendor documentation to understand what is going over the wire, or to debug a `Error::Api` body.

This is the format most third party vendors implement. The shapes below were confirmed against DeepSeek; other endpoints implement the same format with varying completeness.

## Endpoint

```http
POST <base_url>/chat/completions
Authorization: Bearer <key>
Content-Type: application/json
```

There is no version header. The base URL is whatever the vendor documents, `https://api.groq.com/openai/v1` or `http://localhost:11434/v1`, and Freyja appends `/chat/completions`.

## Request body

```json
{
  "model": "deepseek-chat",
  "messages": [],
  "max_tokens": 512,
  "temperature": 0.2,
  "top_p": 0.9,
  "reasoning_effort": "medium",
  "response_format": { "type": "json_object" },
  "tools": [],
  "tool_choice": "auto",
  "metadata": {}
}
```

Only `model` and `messages` are required. Freyja omits every unset field rather than sending null.

`max_tokens` is the default spelling and what the compatible ecosystem implements. OpenAI's own newer models reject it and require `max_completion_tokens` instead; `EndpointConfig::token_limit_field` chooses which one is sent, and exactly one ever is. See [Chat Completions](../../providers/openai-chat.md#the-token-cap-has-two-spellings).

The request also carries `stream` and `stream_options`, both of which `generate()` leaves unset and therefore off the wire. Every body on this page is byte-accurate for a `generate()` call. See [Streaming](#streaming).

Note what is absent: no `system` field, no `instructions`, and no continuation token. System instructions are a message role, and the API is fully stateless, so the whole transcript goes on every request.

## Messages carry everything

```json
{
  "messages": [
    { "role": "system", "content": "Be concise" },
    { "role": "user",   "content": "What is 20 + 22?" },
    { "role": "assistant", "content": null,
      "tool_calls": [
        { "id": "call_0_9f3", "type": "function",
          "function": { "name": "add", "arguments": "{\"a\":20,\"b\":22}" } }
      ] },
    { "role": "tool", "tool_call_id": "call_0_9f3", "content": "42" }
  ]
}
```

Four roles exist: `system`, `user`, `assistant`, `tool`. That is more than Anthropic's two and fewer than nothing, and it is the only dialect where a tool result gets its own role rather than riding on a user turn.

Structurally this sits between the other formats. Tool calls nest inside the assistant message, as on Anthropic, but `arguments` is a JSON string, as on the Responses API.

### Content is a string or an array

```json
"content": "plain text"

"content": [
  { "type": "text", "text": "What is this?" },
  { "type": "image_url", "image_url": { "url": "https://example.com/cat.png" } }
]
```

Freyja sends the **string form whenever it can**, because the simpler compatible endpoints accept only that, and switches to the array form only when an image is present. A data URI goes in the same `url` field, unlike Anthropic which needs it split out.

`"content": null` is correct and expected on an assistant turn that carries only tool calls. Freyja sends the key explicitly rather than omitting it.

## Tool calling

### Declaring tools

```json
"tools": [
  {
    "type": "function",
    "function": {
      "name": "add",
      "description": "adds two numbers together",
      "parameters": {
        "type": "object",
        "properties": { "a": { "type": "integer" }, "b": { "type": "integer" } },
        "required": ["a", "b"]
      }
    }
  }
]
```

The schema is `parameters`, nested under a `function` key. Compare with the Responses API, where the same fields sit flat on the tool object, and Anthropic, where the field is called `input_schema`. Three formats, three spellings.

Optional `"strict": true` goes inside `function`. Support is uneven across compatible endpoints, and many accept the field and ignore it.

### tool_choice

```json
"tool_choice": "auto"
"tool_choice": "none"
"tool_choice": "required"
"tool_choice": { "type": "function", "function": { "name": "add" } }
```

Bare strings for the first three, matching the Responses API, unlike Anthropic where all four are objects. The named form nests twice.

### The call, as returned

```json
{
  "id": "call_0_9f3a1c",
  "type": "function",
  "function": { "name": "add", "arguments": "{\"a\":20,\"b\":22}" }
}
```

`arguments` is a **JSON string**, not an object. Freyja keeps it as a string, so parse it yourself. Some endpoints return an empty string when a tool takes no arguments; Freyja normalizes that to `{}`.

### The result, as sent back

```json
{ "role": "tool", "tool_call_id": "call_0_9f3a1c", "content": "42" }
```

`content` is a plain string, with no type restriction, so `"42"` and a serialized JSON object both work.

**One result per message.** An assistant turn may contain several `tool_calls`, and each needs its own `tool` message. This differs from Anthropic, where several `tool_result` blocks share one user turn.

## Response body

```json
{
  "id": "chatcmpl-1f0a...",
  "object": "chat.completion",
  "created": 1786121202,
  "model": "deepseek-chat",
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "assistant",
        "content": "20 + 22 = 42.",
        "refusal": null,
        "tool_calls": []
      },
      "finish_reason": "stop",
      "logprobs": null
    }
  ],
  "usage": {
    "prompt_tokens": 42,
    "completion_tokens": 15,
    "total_tokens": 57
  }
}
```

Output arrives in `choices`, an array, rather than `output` on the Responses API, `steps` on Gemini, or `content` on Anthropic. Freyja reads `choices[0]` only, since the neutral request has no way to ask for more than one.

`provider_metadata` holds the **top-level** fields Freyja does not model, `object` and `created` among them. It is built by flattening the body's unknown top-level keys, so nothing nested survives: `logprobs` sits inside `choices[]` and is discarded, as is everything else in a choice that Freyja does not map. `usage` subfields go the same way, see [Usage](#usage).

One key in `provider_metadata` is Freyja's own rather than the provider's: if the response has no `choices` at all, Freyja inserts `"freyja_note": "no choices returned"` so an empty answer is distinguishable from a parse failure.

### finish_reason

| Value | Meaning | Neutral `ResponseStatus` |
|---|---|---|
| `stop` | finished naturally | `Completed` |
| `length` | hit `max_tokens` | `Incomplete` |
| `tool_calls` | wants tools executed | `RequiresAction` |
| `function_call` | same, pre-2023 spelling | `RequiresAction` |
| `content_filter` | output withheld | `Other("content_filter")` |

Like Anthropic and unlike the Responses API, this field is accurate about pending tool calls. `has_tool_calls()` is still the portable loop condition.

### Usage

```json
"usage": { "prompt_tokens": 42, "completion_tokens": 15, "total_tokens": 57 }
```

Three fields, a total included, and no cache accounting to fold in, which makes this the simplest usage mapping of the four dialects. Only the names differ from the neutral `Usage`.

Some endpoints add extra fields. DeepSeek reports `prompt_cache_hit_tokens` and `prompt_cache_miss_tokens`, and reasoning endpoints often add `completion_tokens_details`. **None of those reach you.** `usage` is a named field deserialized into a struct holding exactly `prompt_tokens`, `completion_tokens`, and `total_tokens`, with no catch-all, so every other subfield is dropped. They are not in `provider_metadata` either, which only ever holds unknown *top-level* keys. Read the raw body if you need cache accounting on this dialect.

## Non-standard fields you may see

This is where "compatible" starts to fray. None of these are read by Freyja, and **only the top-level ones survive into `provider_metadata`**:

| Field | Where | What it is | Reaches you |
|---|---|---|---|
| `message.reasoning_content` | DeepSeek | Chain of thought. **Do not send it back**, the endpoint rejects it | No, it is nested in `choices[]` |
| `usage.prompt_cache_hit_tokens` | DeepSeek | Cache accounting | No, it is nested in `usage` |
| `x_groq` | Groq | Timing and request metadata | Yes, `provider_metadata["x_groq"]` |
| `provider` | OpenRouter | Which upstream actually served the request | Yes, `provider_metadata["provider"]` |

The rule is mechanical: top-level keys survive, nested ones do not.

The absence of a standard reasoning field is why this dialect drops `InputContent::Reasoning` rather than replaying it, see [OpenAI Chat Completions](../../providers/openai-chat.md).

## Streaming

`Client::stream()` sends the same body to the same URL with two fields added:

```json
{
  "stream": true,
  "stream_options": { "include_usage": true }
}
```

`stream_options` is not optional in practice. **Without `include_usage`, this dialect reports no token counts at all when streaming**, and `StreamEvent::Done` would carry no `Usage` on the most widely-spoken dialect of the four. Freyja always sets it.

The response is a sequence of `data:` frames. Unlike the other three dialects there are no event names: every frame is the same chunk shape, and Freyja reads it positionally.

| Where | What the decoder does with it |
|---|---|
| `choices[0].delta.content` | A fragment of text. Empty strings are skipped |
| `choices[0].delta.refusal` | A fragment of a refusal, kept separate from text |
| `choices[0].delta.tool_calls[]` | Each entry carries an `index`. An entry with an `id` starts that call, with `function.name`; `function.arguments` fragments accumulate into it |
| `choices[0].finish_reason` | The terminal status, mapped by the table above |
| `id`, `model`, `usage` | Read off any frame that has them |
| `error` | Fails the stream as `Error::Stream`, attributed to the endpoint's name |

The final frame is usually **usage-only**: no choices, just `usage`. That is what `include_usage` buys, and it is where the token counts come from.

The stream ends with `data: [DONE]`, which is **not JSON**. A decoder that parses every frame as JSON fails on it. Freyja recognizes the sentinel and consumes it silently, so it never surfaces as an event or an error.

The tool-call `index` counts **tool calls only**, unlike Anthropic's block index, so the first call is always index 0 no matter how much prose precedes it.

`usage` fields are read leniently, defaulting to zero, matching the non-streaming parser: a partial `usage` object yields a `Usage` of zeros rather than no usage.

See [Streaming](../streaming.md).

## Errors

```json
{
  "error": {
    "message": "Model Not Exist",
    "type": "invalid_request_error",
    "param": null,
    "code": "invalid_request_error"
  }
}
```

Freyja classifies the status into a named variant and preserves the whole body alongside it, attributed to the endpoint's configured name rather than to the dialect, so a Groq failure reports Groq.

Status codes mostly follow the OpenAI convention, 400 for a bad request, 401 for a bad key, 429 for rate limiting, 5xx for the endpoint's own trouble. Compatible vendors vary here more than anywhere else, and some return 200 with an error body. Read the body when the status alone is not enough.

The same body can also arrive **mid-stream**, once the connection is already open and the status has long since been 200. This dialect has no event names, so it comes as an ordinary `data:` frame with an `error` object where a chunk would be. Freyja fails the stream with `Error::Stream` carrying the message. An explicit `"error": null`, which several compatible endpoints send on every frame, is not a failure and is ignored.
