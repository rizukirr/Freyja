# Errors

Every fallible call returns `Result<_, ProviderError>`. There is one error type, and its variants fall into three groups: refusals raised before anything left the process, transport failures where no answer came back, and answers the endpoint actually sent.

```rust
#[non_exhaustive]
pub enum ProviderError {
    // Refused here, before the request left the process.
    UnsupportedCapability { provider: Arc<str>, capability: &'static str },
    InvalidRequest { provider: Arc<str>, message: String },

    // The request never completed.
    Http { provider: Arc<str>, kind: TransportError, message: String },

    // The endpoint answered, and the answer was an error.
    BadRequest    { provider: Arc<str>, body: String },
    Unauthorized  { provider: Arc<str>, status: u16, body: String },
    NotFound      { provider: Arc<str>, body: String },
    RateLimit     { provider: Arc<str>, retry_after: Option<Duration>, body: String },
    QuotaExceeded { provider: Arc<str>, status: u16, body: String },
    ServerError   { provider: Arc<str>, status: u16, body: String },
    Api           { provider: Arc<str>, status: u16, body: String },

    // The endpoint answered, but the body was unusable.
    InvalidResponse { provider: Arc<str>, message: String },
    Stream          { provider: Arc<str>, message: String },

    // The endpoint answered fine; the content was not what the caller wanted.
    OutputMismatch  { provider: Arc<str>, message: String, text: String, truncated: bool },
}
```

Implements `Debug`, `Display`, and `std::error::Error`, so it works with `?`, `anyhow`, `thiserror`, and anything else expecting a standard error.

The enum is `#[non_exhaustive]`, so a `match` on it needs a catch-all arm. Variants added later will not break your build.

Every variant carries the endpoint's configured name, reachable with `error.provider()`, so an error from a multi-provider application says which backend produced it. It is the endpoint rather than the dialect, so a failure against a Claude-compatible gateway reports that gateway and not "Anthropic".

## Why the answers are named rather than numbered

The endpoint's error responses could have been one variant carrying a status code, and for a while they were. They are named individually because a status code is not portable: the same number means different things on different vendors, and the classification needs information a caller does not have.

The clearest case is `429`. On OpenAI-shaped endpoints it means *either* "you are sending too fast" *or* "your account is out of credit", separated only by a marker in the body. The first is worth retrying; the second will never succeed no matter how long you wait. A caller branching on the number alone cannot tell them apart, and will retry a dead account forever. Freyja reads the body once and reports [`RateLimit`](#ratelimit) or [`QuotaExceeded`](#quotaexceeded) accordingly.

The `Api` variant remains as the fallback, so a status Freyja does not classify arrives intact rather than being forced into an approximate category.

## The helpers

Four methods answer the questions a caller actually has, so the knowledge does not have to be rebuilt per vendor at every call site:

| Method | Returns |
|---|---|
| `is_retryable()` | Whether repeating the identical request could plausibly succeed |
| `retry_after()` | The endpoint's own `Retry-After` delay, when it sent one |
| `status()` | The HTTP status, for the errors that have one |
| `provider()` | The endpoint's configured name, on every variant |

`is_retryable()` returning `false` means the request will fail the same way every time until something outside it changes: the key, the model name, the request body, or the account balance.

None of these retries anything. See [Retries](#retries).

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
| Gemini | `request metadata` |
| Anthropic | `server-side conversation continuation` |
| Anthropic | `schema-less JSON response format` |
| OpenAI Chat Completions | `server-side conversation continuation` |
| all | `images outside user messages` |
| all except OpenAI Chat Completions | `non-text content in system/developer messages` |

The last row is uneven because OpenAI Chat Completions keeps system turns as ordinary messages rather than hoisting them into a text-only field, so it has nothing to refuse.

Recovery means removing the field or switching providers. Retrying is pointless.

To ask in advance, use [`Client::check`](client.md#check), which runs the same conversion without sending anything and hands back this same error. There is no table of booleans to consult, deliberately: support is not always a property of the field, and a table would be a second description of the dialects to keep in sync by hand.

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

Distinct from [`BadRequest`](#badrequest), which is the endpoint rejecting a request Freyja was willing to send. See [Messages and content](../reference/messages.md).

### Http

The request never completed, so no status was ever received. `kind` says why:

| `TransportError` | Cause | Retryable |
|---|---|---|
| `Timeout` | No reply within the inactivity timeout | Yes |
| `Connect` | DNS failure, connection refused, or rejected TLS | No |
| `Body` | The connection died while the body was being read | Yes |
| `Other` | Anything the classification above does not cover | No |

```
OpenAI timed out: error sending request for url (...)
```

`Connect` is one variant covering three causes because `reqwest` reports them identically, and because they call for the same response: fix the configuration rather than try again. A wrong host name and an expired certificate do not become right on the second attempt.

Note that `Timeout` and `Body` are retryable but not idempotent. The endpoint may have accepted and billed a request whose reply never arrived, so a retry can duplicate work you have already paid for.

### BadRequest

`400` — the endpoint rejected the request body.

```
OpenAI rejected the request: {"error":{"message":"Invalid schema for function 'get_weather'",...}}
```

Not retryable: the same bytes will be rejected again. The raw body is preserved rather than parsed, so the vendor's own message is intact. The [wire reference](wire/openai.md) documents the JSON Freyja sends, which is usually the fastest way to interpret one of these.

### Unauthorized

`401` or `403` — the credential is missing, wrong, or not permitted to use this model. Both statuses land here, and `status` says which.

```
OpenAI refused the credential (HTTP 401): {"error":{"message":"Incorrect API key provided",...}}
```

Not retryable: the key has to change.

### NotFound

`404` — no such model, or the base URL is wrong.

```
Groq has no such model or endpoint: {"error":{"message":"The model `gpt-4o` does not exist"}}
```

Not retryable. On a custom endpoint this most often means the base URL already contains a path segment the dialect appends again; see [Custom providers](../providers/custom.md).

### RateLimit

`429` — requests are arriving faster than the endpoint will serve them.

```
OpenAI rate limited the request, retry after 30s: {"error":{"message":"Rate limit reached",...}}
```

Retryable. `retry_after` carries the endpoint's own `Retry-After` header when it sent one, and is `None` otherwise, in which case your own backoff applies.

Only the delay-seconds form of the header is parsed. The HTTP-date form would need a clock and a date parser, neither of which Freyja has a dependency for, and no major vendor sends it. An unreadable header is `None` rather than a guess.

### QuotaExceeded

The account is out of credit or past a hard quota.

```
OpenAI quota exhausted (HTTP 429): {"error":{"code":"insufficient_quota",...}}
```

Not retryable by waiting: it needs billing action. This exists because several vendors report it with the same `429` they use for throttling, so a caller treating every `429` as a rate limit retries forever against an account that cannot serve the request.

Coverage is uneven, and deliberately so. The split keys on the `insufficient_quota` marker that OpenAI-shaped bodies carry, which covers both OpenAI dialects and the many third-party endpoints that copy them. Gemini reports both cases as `RESOURCE_EXHAUSTED` and cannot be split; Anthropic has no equivalent. On those, an exhausted quota arrives as `RateLimit`, and a bounded retry loop is the protection.

### ServerError

`5xx` — the endpoint failed on its own side.

```
Anthropic failed with HTTP 529: {"type":"error","error":{"type":"overloaded_error",...}}
```

Retryable. The whole `500..=599` range maps here rather than a list of familiar codes, which is what catches Anthropic's non-standard `529` overload signal.

### Api

A non-success status Freyja does not classify. The fallback arm.

```
local returned HTTP 418: {"detail":"teapot"}
```

`is_retryable()` reports `true` for a `5xx` here and `false` otherwise, which is the most that can be said without knowing what the status means.

### InvalidResponse

The provider answered successfully but the body could not be parsed. The message includes the parse error and the body.

```
invalid OpenAI response: missing field `id`; body: {...}
```

This means the vendor changed something Freyja models as required. Unknown fields, unknown output types, and unknown status strings are all tolerated already, so this only fires on a genuine break. Retrying will not help. Report it as a bug.

### OutputMismatch

The call succeeded and the model's answer did not match the type [`generate_as`](client.md#generate_as) was asked for. Only that method raises it.

```
OpenAI output did not match: missing field `purpose` at line 1 column 24
```

Distinct from `InvalidResponse`, and the distinction matters. `InvalidResponse` is the *vendor's* body being unreadable, which means Freyja has a bug and you should report it. Here the vendor behaved perfectly: a well-formed response arrived whose content is the wrong shape. That is a problem with your schema, your prompt, or your token cap.

It carries two things a bare `serde_json::Error` does not:

`text` is the model's answer, kept because the parse failure destroys the only record of what actually came back. Log it, show it, or salvage what you can from it.

`truncated` says whether the answer was cut short, checked against `ResponseStatus::Incomplete` rather than guessed from the parse error. This is the most common cause and the one most easily misread — half a JSON object produces `EOF while parsing an object`, which reads like a schema problem and is not:

```rust
Err(ProviderError::OutputMismatch { truncated: true, .. }) => {
    // Raise max_tokens. The schema is fine.
}
```

Not retryable. Model output is nondeterministic, so another attempt might parse by luck, but the request that produced this will keep producing it. Match the variant directly if you want to retry regardless.

### Stream

A stream that the provider accepted then failed part-way through, reported in the provider's own error frame.

```
OpenAI stream failed: rate limit exceeded
```

Only streaming produces it, and it surfaces from `EventStream::next` rather than from `Client::stream`, which returns before the first frame is read. Calling `EventStream::into_response` on a stream you have not drained raises it too, rather than handing back a response that looks complete and is not.

It is distinct from the status-bearing variants, which report an answer that never began, and from `InvalidResponse`, which reports a body that could not be parsed at all. Whatever text arrived before the failure is already yours to keep; the response as a whole is not. Retrying means re-sending the request from the start, and paying for the prompt and the discarded output again. See [Retries](#retries).

`is_retryable()` reports `false` for it, which is a deliberate understatement: the provider's error frame is not parsed, so Freyja cannot tell a transient overload from the `into_response` misuse, and the safe default is not to encourage a retry it cannot justify. Overload and rate limit conditions genuinely do arrive this way once the connection is open, so read the message and decide.

A stream that simply stops, with no error frame, is not this variant. It ends normally, and the `Done` event carries `ResponseStatus::Incomplete` because no terminal frame set anything else. Check `status` on `Done`, or on the response from `into_response()`, before treating a short answer as a complete one. See [Streaming](../reference/streaming.md).

## Handling errors

Propagate with `?` when the caller decides:

```rust
async fn ask(client: &Client, question: &str) -> Result<String, ProviderError> {
    let request = GenerateRequest::new().message(Message::text(Role::User, question));
    Ok(client.generate(&request).await?.output_text())
}
```

Branch on the name when you want to act on the cause:

```rust
match client.generate(&request).await {
    Ok(response) => println!("{}", response.output_text()),

    Err(ProviderError::RateLimit { retry_after, .. }) => {
        back_off(retry_after.unwrap_or(Duration::from_secs(1)));
    }
    Err(ProviderError::QuotaExceeded { .. }) => {
        eprintln!("out of credit — retrying will not help");
    }
    Err(ProviderError::Unauthorized { .. }) => eprintln!("check the API key"),
    Err(error @ ProviderError::UnsupportedCapability { .. }) => {
        eprintln!("not portable: {error}");
    }

    Err(error) => eprintln!("{} failed: {error}", error.provider()),
}
```

Or skip the match entirely when all you need is whether to try again:

```rust
match client.generate(&request).await {
    Ok(response) => println!("{}", response.output_text()),
    Err(error) if error.is_retryable() => {
        back_off(error.retry_after().unwrap_or(Duration::from_secs(1)));
    }
    Err(error) => return Err(error),
}
```

## Retries

**Freyja does not retry, and will not.** A `429` or a `5xx` surfaces to you exactly once.

This is a deliberate boundary rather than missing work. Backing off means sleeping, and there is no runtime-agnostic timer in `std`, so an automatic retry would mean either taking a `tokio` dependency or maintaining a feature matrix over every async runtime. Freyja exposes `async fn` and never spawns, so the caller chooses the runtime; a retry loop would take that choice away to provide something a caller can write in ten lines.

The policy is application-level in any case. Deadlines, budgets, circuit breakers, and whether a particular call is worth paying for twice are decisions the library cannot make. And a stream that fails after emitting tokens cannot be transparently retried at all without either duplicating output or discarding it silently.

What Freyja does instead is make the decision cheap. It reads the body and the headers once, reports what it found, and leaves the loop to you:

```rust
let mut attempt = 0;
loop {
    match client.generate(&request).await {
        Ok(response) => break Ok(response),
        Err(error) if error.is_retryable() && attempt < 3 => {
            let wait = error
                .retry_after()
                .unwrap_or_else(|| Duration::from_secs(1 << attempt));
            sleep(wait).await;
            attempt += 1;
        }
        Err(error) => break Err(error),
    }
}
```

`examples/retry.rs` is the full version of that loop, with an attempt cap, a ceiling on the wait, and a note on the jitter this sketch leaves out. Run it with `cargo run --example retry`; the module docs show how to point it at a dead port or a wrong key to exercise the failure paths.

Established crates do the general case better than a hand-rolled loop: `backon`, `tower::retry`, and `tokio-retry` all compose with `is_retryable()` directly.

Bound the attempts. `Stream` failures in particular are expensive to retry, because there is no resume: a retry re-sends the request from the start and pays for the whole prompt again, plus the output tokens already generated and thrown away. On a long generation, consider surfacing the partial text instead.

## What is not an error

A non `Completed` `ResponseStatus` is not an error. A truncated answer, a refusal, or a response waiting on tool results all come back as `Ok`, because the call succeeded and the response is real. Check `response.status` for those. See [Responses](../reference/responses.md).
