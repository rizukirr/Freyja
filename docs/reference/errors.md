# Errors

Every fallible call returns `Result<_, ProviderError>`. There is one error type, with six variants covering the six distinct ways a request can fail.

```rust
#[non_exhaustive]
pub enum ProviderError {
    UnsupportedCapability { provider: Arc<str>, capability: &'static str },
    InvalidRequest { provider: Arc<str>, message: String },
    Http(String),
    Api { provider: Arc<str>, status: u16, body: String },
    InvalidResponse { provider: Arc<str>, message: String },
    Stream { provider: Arc<str>, message: String },
}
```

Implements `Debug`, `Display`, and `std::error::Error`, so it works with `?`, `anyhow`, `thiserror`, and anything else expecting a standard error.

The enum is `#[non_exhaustive]`, so a `match` on it needs a catch-all arm. A future variant, such as the typed rate-limit error that is Phase 1 work, will not break your build.

Every variant except `Http` carries the endpoint's configured name, so an error from a multi provider application says which backend produced it. It is the endpoint rather than the dialect, so a failure against a Claude-compatible gateway reports that gateway and not "Anthropic".

## The variants

### UnsupportedCapability

The request asked for something this provider cannot express. Freyja refuses rather than silently dropping the field, because a quietly ignored `tool_choice` produces a plausible looking answer that is wrong in a way you cannot see.

```
Gemini does not support portable reasoning effort levels
```

Raised before any network call. Current cases:

| Provider | Capability |
|---|---|
| Gemini | `portable reasoning effort levels` |
| Gemini | `portable tool choice` |
| Anthropic | `server-side conversation continuation` |
| Anthropic | `reasoning effort 'minimal'` |
| Anthropic | `schema-less JSON response format` |
| OpenAI Chat Completions | `server-side conversation continuation` |
| all | `images outside user messages` |
| all except OpenAI Chat Completions | `non-text content in system/developer messages` |

The last row is uneven because OpenAI Chat Completions keeps system turns as ordinary messages rather than hoisting them into a text-only field, so it has nothing to refuse.

Recovery means removing the field or switching providers. Retrying is pointless.

There is no way to ask in advance whether a capability is supported. A `Provider::capabilities()` method is Phase 1 work.

### InvalidRequest

The request is malformed and was rejected before leaving the process.

```
invalid request for OpenAI: tool messages may only contain tool results
```

This is a bug in your code, not a provider limitation, and switching providers will not help. Current cases:

| Condition | Where |
|---|---|
| Text content on a `Role::Tool` turn | OpenAI Responses, Gemini |
| A tool message answering more than one call | OpenAI Chat Completions |
| Tool arguments that are not a JSON object | Anthropic |
| A malformed image data URI | Anthropic |
| A tool result whose call is absent from the transcript | Gemini |
| No model on the request and none on the endpoint | all |

See [Messages and content](../reference/messages.md).

### Http

The HTTP request never completed: DNS failure, connection refused, TLS failure, or timeout. Carries the underlying `reqwest` message.

```
HTTP request failed: error sending request for url (...)
```

Usually transient and worth retrying with backoff. Note that a timeout is indistinguishable from other transport failures here, so a retry may duplicate a request the provider already accepted.

### Api

The provider answered with a non success status. The raw body is preserved rather than parsed, so nothing is lost.

```
OpenAI returned HTTP 429: {"error":{"message":"Rate limit reached",...}}
```

Branch on `status` to decide what to do:

| Status | Meaning | Action |
|---|---|---|
| 400 | Malformed request | Fix the request, do not retry |
| 401, 403 | Bad or missing credentials | Fix the key, do not retry |
| 404 | Unknown model or endpoint | Fix the model, do not retry |
| 429 | Rate limited | Back off and retry |
| 5xx | Provider side failure | Back off and retry |

Typed variants for rate limits, auth failures, context length, and content filters are Phase 1 work. Today you parse `body` yourself when you need the detail.

### InvalidResponse

The provider answered successfully but the body could not be parsed. The message includes the parse error and the body.

```
invalid OpenAI response: missing field `id`; body: {...}
```

This means the vendor changed something Freyja models as required. Unknown fields, unknown output types, and unknown status strings are all tolerated already, so this only fires on a genuine break. Retrying will not help. Report it as a bug.

### Stream

A stream that the provider accepted then failed part-way through, reported in the provider's own error frame.

```
OpenAI stream failed: rate limit exceeded
```

Only streaming produces it, and it surfaces from `EventStream::next` rather than from `Client::stream`, which returns before the first frame is read. Calling `EventStream::into_response` on a stream you have not drained raises it too, rather than handing back a response that looks complete and is not.

It is distinct from `Api`, which reports a non-success HTTP status, and from `InvalidResponse`, which reports a body that could not be parsed at all. Whatever text arrived before the failure is already yours to keep; the response as a whole is not. Retrying means re-sending the request from the start.

A stream that simply stops, with no error frame, is not this variant. It ends normally, and the `Done` event carries `ResponseStatus::Incomplete` because no terminal frame set anything else. Check `status` on `Done`, or on the response from `into_response()`, before treating a short answer as a complete one. See [Streaming](../reference/streaming.md).

## Handling errors

Propagate with `?` when the caller decides:

```rust
async fn ask(client: &Client, question: &str) -> Result<String, ProviderError> {
    let request = GenerateRequest::new().message(Message::text(Role::User, question));
    Ok(client.generate(&request).await?.output_text())
}
```

Or branch on what is worth retrying:

```rust
match client.generate(&request).await {
    Ok(response) => println!("{}", response.output_text()),

    // Transient, retry with backoff.
    Err(ProviderError::Http(_)) => retry_later(),
    Err(ProviderError::Api { status: 429 | 500..=599, .. }) => retry_later(),

    // Permanent, fix the request.
    Err(error @ ProviderError::UnsupportedCapability { .. }) => {
        eprintln!("not portable: {error}");
    }
    Err(error @ ProviderError::InvalidRequest { .. }) => {
        eprintln!("bug in the request: {error}");
    }

    Err(error) => eprintln!("failed: {error}"),
}
```

## Retries

Freyja does not retry. A 429 or a 5xx surfaces to you exactly once. Automatic backoff honoring `Retry-After` is Phase 1 work.

Until then, retry at the call site, and only on `Http` and on `Api` with a 429 or 5xx status. Retrying the other variants wastes the call, since the outcome cannot change.

## What is not an error

A non `Completed` `ResponseStatus` is not an error. A truncated answer, a refusal, or a response waiting on tool results all come back as `Ok`, because the call succeeded and the response is real. Check `response.status` for those. See [Responses](../reference/responses.md).
