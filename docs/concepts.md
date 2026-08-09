# Concepts

Five ideas. Once these land, the rest of Freyja is detail you can look up.

## 1. One request, many wire formats

You build a `GenerateRequest`. Freyja converts it to whatever the endpoint expects.

```
                        ┌─→ OpenAI Responses   → flat item list
GenerateRequest ─────────┼─→ Gemini             → flat step list
   (what you write)      ├─→ Anthropic          → nested blocks
                        └─→ Chat Completions   → nested, dedicated tool role
```

The same happens in reverse: four differently shaped responses become one `GenerateResponse`. Streaming follows the same rule rather than escaping it: four differently shaped event formats become one `StreamEvent` sequence, and `EventStream::into_response` collapses that sequence into the same `GenerateResponse` a non-streaming call would have returned.

This is worth more than it sounds. Those four formats disagree about almost everything. Where a tool call lives, whether its arguments are a string or an object, whether system instructions are a message or a separate field, what the correlation id is called, how usage is reported. Your code sees none of it.

**The rule that keeps this honest:** the neutral model never bends toward a vendor. If a field only makes sense on one provider, it does not go in `GenerateRequest`. It surfaces through `provider_metadata` on the way back instead.

## 2. Dialect and endpoint are different things

This is the idea most likely to be new.

A **dialect** is a wire format. A **provider** or **endpoint** is a URL that speaks one.

Most hosted inference APIs do not invent a format. They copy OpenAI's Chat Completions so existing client libraries work unchanged. So one dialect reaches many vendors:

| Dialect | Spoken by |
|---|---|
| `OpenAiChat` | DeepSeek, Groq, Together, OpenRouter, Ollama, vLLM, xAI, Mistral, and more |
| `Anthropic` | Anthropic, plus several drop-in Claude gateways |
| `OpenAiResponses` | OpenAI only |
| `Gemini` | Google only |

Which is why reaching a new vendor is usually not a code change:

```rust
let client = Client::custom(
    ProviderDialect::OpenAiChat,      // which format
    "DeepSeek",                        // name, for error messages
    "https://api.deepseek.com/v1",     // where
    api_key,
);
```

Freyja ships **presets** for OpenAI, Gemini, and Anthropic only. That is deliberate: a preset is a promise that a URL and default model are current, and third-party endpoints change faster than this library could verify. A preset is nothing but a `ProviderConfig` with the fields filled in, so nothing is gated behind having one. See [Custom providers](providers/custom.md).

## 3. Unset means "the vendor decides"

`GenerateRequest::new()` sets nothing. Not a model, not a temperature, not a tool choice.

```rust
let request = GenerateRequest::new()
    .message(Message::text(Role::User, "Hello"));
// Valid on every provider. Nothing here can be rejected.
```

Every optional field left as `None` is omitted from the wire request entirely, so the vendor applies its own default.

This was learned painfully. An earlier version defaulted `tool_choice` to `Auto` and `reasoning_effort` to `Medium`, both fields Gemini rejects outright, so every default-constructed request failed against Gemini before it reached the network. A value that looks harmless on one provider is a 400 on another.

**The consequence for you:** set only what you actually care about. Every field you set is one more thing that can be refused somewhere.

## 4. Refusal, never silent degradation

When you ask for something the endpoint cannot express, Freyja returns an error before sending anything.

```rust
GenerateRequest::new().tool_choice(ToolChoice::Required);
// Against Gemini: Gemini does not support portable tool choice
```

The alternative would be to drop the field and send the request anyway. That is worse. You would get a plausible, well-formed answer, and nothing anywhere would tell you the constraint you asked for had been ignored. A refusal is loud, and loud is debuggable.

The one deliberate exception is documented in [OpenAI Chat Completions](providers/openai-chat.md#reasoning-blocks-are-dropped-not-replayed), and it exists so a conversation can move between providers at all.

## 5. Opaque state

Reasoning models emit signed internal state: Gemini calls them thought signatures, Anthropic thinking blocks, OpenAI reasoning items. They look like noise and they are not optional.

When you send the next request in a conversation, these have to come back **unchanged and in position**. Editing one, rebuilding an equivalent by hand, or dropping it gets the request rejected. The signature is what the vendor validates.

Freyja handles this for you, but only if you build the next turn the intended way:

```rust
request = request
    .message(response.to_message())   // carries the opaque state
    .extend_messages(tool_results);
```

`to_message()` preserves everything the response contained, including blocks Freyja does not model. **Assembling that assistant turn by hand from the tool calls alone will drop the state and your next request will fail.**

That is the whole rule. You do not need to know what is inside.

---

## How the pieces fit

```rust
// 1. Pick an endpoint. A preset, or your own.
//    `from_env` returns None when the key variable is unset.
let Some(client) = Client::from_env(ProviderType::Anthropic) else {
    return Ok(());
};

// 2. Describe what you want, in neutral terms.
let request = GenerateRequest::new()
    .message(Message::text(Role::User, "What is 20 + 22?"))
    .tools([add_tool]);

// 3. Send it. Freyja converts, posts, and converts back.
let response = client.generate(&request).await?;

// 4. Read a neutral response, whatever vendor produced it.
if response.has_tool_calls() { /* ... */ }
println!("{}", response.output_text());
```

Next: [Building an agent](building-an-agent.md) turns step 4 into a loop.
