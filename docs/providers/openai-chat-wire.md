# OpenAI Chat Completions wire format

The native JSON of the Chat Completions API, as Freya speaks it. This page exists so you do not have to read vendor documentation to understand what is going over the wire, or to debug a `ProviderError::Api` body.

This is the format most third party vendors implement. The shapes below were confirmed against DeepSeek; other endpoints implement the same format with varying completeness.

## Endpoint

```http
POST <base_url>/chat/completions
Authorization: Bearer <key>
Content-Type: application/json
```

There is no version header. The base URL is whatever the vendor documents, `https://api.groq.com/openai/v1` or `http://localhost:11434/v1`, and Freya appends `/chat/completions`.

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

Only `model` and `messages` are required. Freya omits every unset field rather than sending null.

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

Freya sends the **string form whenever it can**, because the simpler compatible endpoints accept only that, and switches to the array form only when an image is present. A data URI goes in the same `url` field, unlike Anthropic which needs it split out.

`"content": null` is correct and expected on an assistant turn that carries only tool calls. Freya sends the key explicitly rather than omitting it.

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

`arguments` is a **JSON string**, not an object. Freya keeps it as a string, so parse it yourself. Some endpoints return an empty string when a tool takes no arguments; Freya normalizes that to `{}`.

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

Output arrives in `choices`, an array, rather than `output` on the Responses API, `steps` on Gemini, or `content` on Anthropic. Freya reads `choices[0]` only, since the neutral request has no way to ask for more than one.

Everything Freya does not model, including `object`, `created`, and `logprobs`, stays reachable through `response.provider_metadata`.

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

Some endpoints add extra fields. DeepSeek reports `prompt_cache_hit_tokens` and `prompt_cache_miss_tokens`, and reasoning endpoints often add `completion_tokens_details`. Those are not summed into anything and stay in `provider_metadata`.

## Non-standard fields you may see

This is where "compatible" starts to fray. None of these are read by Freya, and all remain reachable through `provider_metadata`:

| Field | Where | What it is |
|---|---|---|
| `message.reasoning_content` | DeepSeek | Chain of thought. **Do not send it back**, the endpoint rejects it |
| `usage.prompt_cache_hit_tokens` | DeepSeek | Cache accounting |
| `x_groq` | Groq | Timing and request metadata |
| `provider` | OpenRouter | Which upstream actually served the request |

The absence of a standard reasoning field is why this dialect drops `InputContent::Reasoning` rather than replaying it, see [OpenAI Chat Completions](openai-chat.md).

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

Freya preserves the whole body in `ProviderError::Api` alongside the HTTP status, attributed to the endpoint's configured name rather than to the dialect, so a Groq failure reports Groq.

Status codes mostly follow the OpenAI convention, 400 for a bad request, 401 for a bad key, 429 for rate limiting, 5xx for the endpoint's own trouble. Compatible vendors vary here more than anywhere else, and some return 200 with an error body. Read the body when the status alone is not enough.
