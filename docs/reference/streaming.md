# Streaming

`Client::stream` returns the same answer as `generate`, delivered as it arrives.

```rust
pub async fn stream(&self, request: &GenerateRequest)
    -> Result<EventStream, Error>
```

It returns once the provider has accepted the request, so a non-success status arrives here as `Error::Api` rather than part-way through iteration. By the time you hold an `EventStream`, the request is past authentication, rate limiting, and model validation.

## Driving the stream

```rust
use freyja::StreamEvent;

let request = GenerateRequest::new().message(Message::text(Role::User, "Hello"));
let mut stream = client.stream(&request).await?;

while let Some(event) = stream.next().await? {
    if let StreamEvent::TextDelta(text) = event {
        print!("{text}");
    }
}
```

```rust
pub async fn next(&mut self) -> Result<Option<StreamEvent>, Error>
```

The `?` sits inside the `while let`, not after it. `next` returns `Result<Option<_>>`: the `Result` is whether the stream is still healthy, the `Option` is whether there is anything left. `None` means the provider closed the body, and it is the loop's only exit.

Frames carrying nothing you can act on, such as keepalives and sentinels, are consumed without producing an event, so every event you receive is one you can do something with.

Text deltas arrive mid-line. If you are printing them, flush after each one or nothing appears until the process exits. `examples/streaming.rs` is the full version of the loop above.

## StreamEvent

```rust
#[non_exhaustive]
pub enum StreamEvent {
    TextDelta(String),
    RefusalDelta(String),
    ToolCall { id: String, name: String, arguments: String },
    ReasoningDelta(String),
    Reasoning { data: Value },
    Done { id: String, model: String, status: ResponseStatus, usage: Option<Usage> },
}
```

| Variant | Meaning | When it fires |
|---|---|---|
| `TextDelta` | A fragment of generated text, in order | Repeatedly, as the model produces text |
| `RefusalDelta` | A fragment of a refusal | Repeatedly, when the model declines. Kept distinct from text for the same reason `OutputContent::Refusal` is |
| `ToolCall` | A complete tool call, arguments fully assembled | Once per call, when that call is finished |
| `ReasoningDelta` | Human-readable reasoning text | Repeatedly, on providers that expose it |
| `Reasoning` | Opaque provider reasoning state, complete and replayable | Once per block, when that block is finished |
| `Done` | Id, model, status, and usage | Exactly once, immediately before the stream ends |

The enum is `#[non_exhaustive]`, so a `match` needs a trailing `_ => {}` arm. A provider gaining an event Freyja can model will not break your build.

```rust
while let Some(event) = stream.next().await? {
    match event {
        StreamEvent::TextDelta(text) => print!("{text}"),
        StreamEvent::ToolCall { id, name, arguments } => {
            println!("{name}({arguments}) as {id}");
        }
        StreamEvent::Done { status, usage, .. } => {
            println!("ended {status:?} after {usage:?}");
        }
        _ => {}
    }
}
```

`Done` is where `status` and `usage` come from. There is no separate call to make afterwards.

## Fragments are never exposed

This is the main departure from other streaming APIs, and it is deliberate.

Providers stream tool-call arguments and reasoning blobs in pieces: a few characters of JSON per frame, split at arbitrary points. Freyja assembles those internally and emits them only once complete, so `StreamEvent::ToolCall` always carries arguments you can hand straight to `serde_json::from_str`, and `StreamEvent::Reasoning` always carries a blob you can replay verbatim. No caller stitches partial JSON, and no caller has to know that a given vendor splits its arguments differently from the last one.

Text and refusals are the exception, because a fragment of text is useful on its own. Those arrive as deltas, which is the entire point of streaming.

## into_response

```rust
pub fn into_response(self) -> Result<GenerateResponse, Error>
```

Consumes the drained stream and hands back the `GenerateResponse` that `generate` would have returned for the same turn.

```rust
let mut stream = client.stream(&request).await?;
while let Some(event) = stream.next().await? {
    // render as it arrives
}

let response = stream.into_response()?;
```

What matches `generate` exactly:

| | |
|---|---|
| `id`, `model`, `status` | Identical |
| `content` | Identical part for part, in order |
| `usage` | Identical, including Anthropic's computed total |
| `to_message()` | Produces the same assistant turn |

What differs is `provider_metadata`, and it differs by shape rather than by accident. `generate` collects the fields Freyja does not model, using serde's flatten, so you get the leftovers. A stream carries the provider's terminal object whole, because that object is what the final frame contains. Both are the provider's own data; read them accordingly, and do not compare the two paths field for field. See [Responses](responses.md#provider_metadata).

Calling `into_response` before `next` has returned `None` fails with `Error::Stream`. A response that looks complete but is not, replayed to a provider on the next turn, fails in ways that are hard to trace back here, so Freyja refuses instead. Drain first.

## A streaming tool loop

Because a drained stream converts back into a `GenerateResponse`, the tool loop is the one from [Tool calling](tools.md#a-complete-loop) with the single `generate` call replaced by drain-then-convert. Nothing after that line changes.

```rust
let mut request = GenerateRequest::new()
    .message(Message::text(Role::User, "What is 20 + 22?"))
    .tools([add_tool]);

for _ in 0..5 {
    let mut stream = client.stream(&request).await?;

    while let Some(event) = stream.next().await? {
        if let StreamEvent::TextDelta(text) = event {
            print!("{text}");
        }
    }

    let response = stream.into_response()?;

    if !response.has_tool_calls() {
        break;
    }

    let results: Vec<Message> = response
        .tool_calls()
        .map(|(id, name, arguments)| Message::tool_result(id, dispatch(name, arguments)))
        .collect();

    request = request
        .message(response.to_message())
        .extend_messages(results);
}
```

The bound is still not optional, for the same reason. You can dispatch a tool the moment its `StreamEvent::ToolCall` arrives rather than waiting for the drain, since arguments are complete by then, but you still need the drained response to build the assistant turn: `to_message()` carries the reasoning blocks that Gemini and Anthropic require back verbatim, and hand-assembling that turn from the events fails at the API.

## Timeouts

The default HTTP client bounds *inactivity*, not total duration. It sets `read_timeout`, so a stream is cut off after 120 seconds of silence and not after 120 seconds of streaming. A long generation is safe as long as bytes keep arriving.

A client you supply through `Client::with_http_client` keeps whatever you built it with. Set `read_timeout` there rather than `timeout`, or a healthy long stream is killed part-way:

```rust
use std::time::Duration;

let http = reqwest::Client::builder()
    .read_timeout(Duration::from_secs(120))
    .connect_timeout(Duration::from_secs(5))
    .build()?;

let client = Client::with_http_client(EndpointPreset::OpenAi, api_key, http);
```

`timeout` on a streaming request is a deadline for the whole response body, which for streaming is a deadline on how long the model is allowed to talk.

## Size limits

A timeout bounds silence. It does not bound volume, and an endpoint that keeps sending is never late, so there is a second bound underneath it: one server-sent event may buffer 16 MiB before the stream fails with `Error::Stream`.

```
probe stream failed: a single event grew past 16777216 bytes without ending
```

An event is a JSON object, and the largest any provider sends is a terminal object carrying the whole interaction, so the ceiling is orders of magnitude above anything real. What it catches is an endpoint that never emits a frame separator, which would otherwise be buffered whole until the process runs out of memory. Freyja is built to be pointed at gateways it has never met, so this is not hypothetical.

Non-streaming responses have the same protection at 64 MiB, reported as `Error::InvalidResponse`.

## Errors

Everything that can fail before the first byte surfaces from `stream`, classified by cause: `RateLimit`, `Unauthorized`, `ServerError`, and the rest of the status-bearing variants. After that, failures surface from `next`: `Error::Stream` for the provider's own mid-stream error frame, `Error::Http` with a `Body` kind for the connection dropping underneath.

A stream that simply stops early, with no error frame, is not an error. The `Done` event carries `ResponseStatus::Incomplete`, because nothing set a terminal status, so check `status` rather than assuming a stream that ended is a stream that finished. See [Errors](errors.md#stream).

## Provider differences

All four dialects stream, and the neutral event sequence is the same on each. What differs is underneath:

| Provider | How |
|---|---|
| OpenAI | `stream: true` in the body, semantic SSE events |
| OpenAI Chat Completions | `stream: true`, plus `stream_options.include_usage` so token counts still arrive |
| Gemini | `stream: true` in the body *and* `?alt=sse` on the URL, which is what selects SSE framing |
| Anthropic | `stream: true`, with `message_start` / `content_block_*` / `message_delta` events |

Freyja sets all of that for you. The per-provider pages have the detail.

All four have been exercised against a live endpoint for a **text** turn: deltas arrive, usage lands on `Done`, and `into_response` rebuilds exactly the text the deltas carried. Streamed **tool calls** have not — that is the path where argument fragments are joined, and it remains covered by recorded fixtures only, with a test per dialect asserting that a drained stream matches what `generate` builds from the same turn. See [Features](../features.md#verification-status).
