# OpenAI

Implemented against the Responses API.

| | |
|---|---|
| Endpoint | `POST https://api.openai.com/v1/responses` |
| Auth | `Authorization: Bearer <key>` |
| Key variable | `OPENAI_API_KEY` |
| Default model | `gpt-5.6-sol` |
| Source | `src/provider/openai/` |

```rust
let client = Client::from_env(ProviderType::OpenAi).expect("OPENAI_API_KEY");
```

This is the more complete of the two backends and the one exercised end to end.

## Capability support

| Capability | Supported | Notes |
|---|---|---|
| Text generation | yes | |
| Images in user turns | yes | URL or data URI, as `input_image` |
| System and developer turns | yes | Hoisted into `instructions` |
| `max_tokens` | yes | Sent as `max_output_tokens` |
| `temperature`, `top_p` | yes | Forwarded unchanged |
| `reasoning_effort` | yes | Sent as `reasoning.effort` |
| `response_format` | yes | Text, JSON object, and strict JSON schema |
| Tool declarations | yes | |
| `tool_choice` | yes | All four variants |
| Tool round trip | yes | Verified by tests |
| `previous_response_id` | yes | Sent as `previous_response_id` |
| `metadata` | yes | Sent as `metadata` |
| Usage reporting | yes | |
| Refusals | yes | Parsed as `OutputContent::Refusal` |
| Streaming | no | Not implemented in Freya |

Nothing in the neutral model currently returns `UnsupportedCapability` on OpenAI except misplaced content, covered below.

## Field mapping

### Outbound

| Neutral | Wire |
|---|---|
| `model` | `model`, defaulting to `gpt-5.6-sol` |
| system and developer turns | `instructions`, joined with a blank line |
| other turns | `input` items |
| `max_tokens` | `max_output_tokens` |
| `temperature` | `temperature` |
| `top_p` | `top_p` |
| `reasoning_effort` | `reasoning.effort` |
| `response_format` | `text.format` |
| `tools` | `tools`, each with `"type": "function"` |
| `tool_choice` | `tool_choice` |
| `previous_response_id` | `previous_response_id` |
| `metadata` | `metadata` |

Unset fields are omitted from the body rather than sent as null, and empty `tools` is omitted too.

`tool_choice` serializes as the string `auto`, `none`, or `required`, or as `{"type": "function", "name": "..."}` for `Named`.

### Inbound

| Wire | Neutral |
|---|---|
| `id` | `id` |
| `model` | `model` |
| `status` | `status` |
| `output[].message.content[].output_text` | `OutputContent::Text` |
| `output[].message.content[].refusal` | `OutputContent::Refusal` |
| `output[].function_call` | `OutputContent::ToolCall` |
| `usage.input_tokens` and friends | `Usage` |
| everything else | `provider_metadata` |

Unknown output item types and unknown content block types are skipped rather than failing the parse.

Note that the wire field is `call_id`, and Freya exposes it as `id` on `OutputContent::ToolCall`. Quote it back unchanged in `Message::tool_result`.

## Input items are flat

The Responses API does not nest tool calls inside messages. Messages, tool calls, and tool results are all siblings in one `input` array:

```json
{
  "model": "gpt-5.6-sol",
  "instructions": "Be concise",
  "input": [
    {"type": "message", "role": "user",
     "content": [{"type": "input_text", "text": "What is 20 + 22?"}]},
    {"type": "message", "role": "assistant",
     "content": [{"type": "output_text", "text": "Let me add those."}]},
    {"type": "function_call", "call_id": "call_1", "name": "add",
     "arguments": "{\"a\":20,\"b\":22}"},
    {"type": "function_call_output", "call_id": "call_1", "output": "42"}
  ]
}
```

A neutral `Message` can hold text and a tool call together, so the converter accumulates text and image parts and flushes them as a message item before emitting each tool item. Order is preserved, and one neutral message can become several wire items.

## Text block types differ by role

User turns use `input_text`, assistant turns replayed as input use `output_text`. Freya picks the right one from the role, so you do not have to think about it.

Images use `input_image` with an `image_url` field, and are only accepted on user turns.

## Rejected before the network

| Condition | Error |
|---|---|
| Non text content in a system or developer turn | `UnsupportedCapability` |
| An image on a non user turn | `UnsupportedCapability` |
| Text content on a `Role::Tool` turn | `InvalidRequest` |

## Structured output

```rust
let request = GenerateRequest::new()
    .message(Message::text(Role::User, "Extract the name and age."))
    .response_format(ResponseFormat::JsonSchema {
        name: "person".into(),
        schema: serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "age": {"type": "integer"}
            },
            "required": ["name", "age"],
            "additionalProperties": false
        }),
        strict: true,
    });
```

Becomes `text.format` on the wire. With `strict: true`, OpenAI requires `additionalProperties: false` and every property listed in `required`. A schema that violates that comes back as an `Api` error with status 400.

Read the result with `response.output_text()` and parse it yourself.

## Default model

`gpt-5.6-sol` is used when `model` is unset. It is a constant in `src/provider/openai/types.rs`, and it will drift as OpenAI ships new models. Set `model` explicitly for anything you need to stay stable.

## Errors

`Api` errors carry the status and the raw body:

```
OpenAI returned HTTP 429: {"error":{"message":"Rate limit reached",...}}
```

Freya does not parse the error body into typed variants and does not retry. Both are Phase 1 work. See [Errors](../errors.md).
