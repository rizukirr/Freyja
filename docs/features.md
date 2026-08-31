# Features

What Freyja does today, and what it does not. The second list matters as much as the first, so it is on the same page rather than hidden.

## What works

### Talking to models

| | |
|---|---|
| Text generation | Yes, on every provider |
| Images in a prompt | Yes, by URL or `data:` URI |
| System instructions | Yes, placed correctly per provider automatically |
| Model selection | Yes, or leave it unset and take the endpoint's default |
| Sampling controls | `max_tokens`, `temperature`, `top_p` |
| Reasoning effort | Yes, on every dialect; a level a vendor lacks is that vendor's rejection |
| Asking before sending | `Client::check`, no network call and no key used |
| Structured output | JSON schema, and free JSON where the provider offers it |
| Typed responses | `generate_as::<T>()` deserializes for you |
| Strict-mode schemas | `strict_schema()` rewrites a schema into the subset OpenAI accepts |
| Vendor-only fields | `extra_for()`, scoped to a dialect so the request stays portable |
| Token accounting | Yes, normalized across providers |
| Streaming | Yes, on every dialect, see [Streaming](reference/streaming.md) |

Streaming delivers the same answer incrementally, and a drained stream converts back into the same `GenerateResponse`, so a tool loop written against `generate` keeps working. Tool-call arguments and reasoning blobs are assembled for you and surface only once complete, so nothing stitches partial JSON.

### Building agents

| | |
|---|---|
| Declaring tools | Yes, with JSON Schema parameters |
| Typed tool functions | `#[tool]` derives the schema and JSON executor from a sync or async function |
| Constraining tool choice | Auto, none, required, or a named tool |
| The full tool round trip | **Yes, verified against live APIs**, not just tested offline |
| Multi-turn conversations | Yes, transcripts are plain data you own |
| Reasoning state replay | Handled for you, see [Concepts](concepts.md#opaque-state) |
| Automatic loop orchestration | `Agent` runs the tool-calling loop for you and dispatches parallel tool calls concurrently |
| Refusing a tool call | `Agent::guard` vets every requested call, and a refusal reaches the model as text it can act on |
| Tools that hold state | Implement `Tool` on a struct; its fields are per-agent state |
| Per-run data in a tool | `Context` is handed to every call and never sent to the model |
| Tools defined at runtime | `name` and `definition` are values, so an MCP-shaped tool needs no compile-time type |
| Tools that fail | A `Result` return reaches the model as error text it can recover from |
| Bounding a transcript | `InMemoryStorage::window` keeps pinned turns and the most recent turn groups, applied inside `load` |
| Storage written elsewhere | `Storage` is three methods over public types, so a third-party crate implements it with no change here, and may cut anywhere it likes, because the repair pass drops both halves of a pair the cut separated |

The round trip is the load-bearing feature. A model asks for a function, you run it, you feed the result back, and it continues. [Building an agent](building-an-agent.md) is the guide.

### Reaching providers

| | |
|---|---|
| Built-in | OpenAI, Google Gemini, Anthropic |
| Wire dialects | Four, including the format most third-party vendors copy |
| Any other endpoint | One `Client::custom` call, no code change |
| Non-conventional URLs | `path` replaces the dialect's path, `query` pins parameters on every request |
| Credentials in the URL | `Auth::Query` for an endpoint that takes its key as `?key=`, kept out of `url()`, as is any `secret_query` value |
| Local runtimes | Yes, no credentials required |

Built-in means Freyja ships and tests the URL and default model. It does not mean the others are second class: DeepSeek, Groq, OpenRouter, Ollama, and a long tail of others work through [Custom providers](providers/custom.md) with the same code paths.

### Operational

| | |
|---|---|
| Connection pooling | One HTTP client per `Client`, reused |
| Timeouts | 120 seconds of inactivity by default, or supply your own HTTP client |
| Errors | Classified by cause, each attributed to the endpoint that failed |
| Retry decisions | `is_retryable()` and the endpoint's own `Retry-After` |
| Credential safety | `Debug` redacts the API key, `secret_header` and `secret_query` extend that to a second credential in `Debug`, error messages and `url()`, and every credential header goes out marked sensitive |
| Dependencies | `reqwest`, `serde`, `serde_json`, `schemars`, and `freyja-macros` |

## What does not exist yet

Be sure none of these is on your critical path before adopting.

| | Status | Workaround |
|---|---|---|
| **Retries** | Out of scope, deliberately | A 429 or 5xx surfaces once. `is_retryable()` and `retry_after()` tell you what to do; the loop is yours, and composes with `backon` or `tower::retry`. See [Errors](reference/errors.md#retries). |
| **Per-tool timeouts** | Out of scope, deliberately | Racing a call against a clock needs a timer, and Freyja depends on no runtime. A wrapper tool that holds the inner one and applies your runtime's timeout gets there in a dozen lines, for a tool you did not write as much as one you did. The [`Tool`](https://docs.rs/freyja/latest/freyja/trait.Tool.html) documentation has the whole implementation. |
| **Structured-output schema derivation** | Not implemented | `#[tool]` derives argument schemas, but `ResponseFormat::JsonSchema` still takes an explicit schema. Generate one with `schemars` and pass it through `strict_schema()`. |
| **Capability tables** | Not planned | `Client::check` answers the same question by running the conversion, so there is nothing to keep in sync. It needs a request in hand, which a table would not. |
| **Token-aware windows, summarization, and persistent storage** | Not implemented | `InMemoryStorage::window` bounds a transcript by turn group, which needs no tokenizer. Counting tokens needs an estimate calibrated from `Usage`, and summarizing needs a model call. Both are things a `Storage` backend can do inside its own `load`, without any change to the trait. `InMemoryStorage` is the only `Storage` Freyja ships, and a backend that survives a restart needs nothing from this crate beyond the trait, since `Message` already derives `Serialize` and `Deserialize`. |
| **Embeddings and RAG** | Not implemented | An embeddings endpoint is a request shape no dialect covers, so it is a wire format of its own rather than a feature on top of one. |

## Per-provider gaps

Capability coverage is not uniform, and Freyja refuses rather than pretending.

| Capability | OpenAI | Gemini | Anthropic | Chat Completions |
|---|---|---|---|---|
| `tool_choice` | Yes | Yes | Yes | Yes |
| `reasoning_effort` | Yes | Yes | Yes | Yes |
| `response_format` | Yes | Yes | Schema only | Yes |
| `previous_response_id` | Yes | Yes | **No** | **No** |
| `metadata` | Yes | Yes | Yes | Yes |

A **No** means `UnsupportedCapability` before any network call, so you find out immediately rather than getting an answer that ignored you. Each provider page has the full table.

A **Yes** means Freyja can express it, not that every model will accept every value. Only the dialect is known here; the endpoint and the model are not. `reasoning_effort` is the clearest case — OpenAI's Responses API takes `Max` and its Chat Completions API does not, on the same model, and both arrive as the vendor's own `BadRequest` rather than as a refusal from Freyja.

The table has no middle column for "carries it but the endpoint says no", deliberately. Gemini rejects three `thinking_level` values and rejects `labels` outright; both fields exist, so both requests are sent and the endpoint answers. Freyja refuses a field only when the format has nowhere to put it — anything narrower is a claim about a deployment on a given day. See [Gemini](providers/gemini.md#reasoning-effort-is-nested-and-half-of-it-is-rejected).

## Verification status

Freyja distinguishes "the tests pass" from "a real vendor accepted it", because the two came apart once already and cost three bugs.

| Endpoint | Live tool round trip |
|---|---|
| OpenAI | Yes |
| Gemini | Yes |
| Anthropic | Yes |
| A Chat Completions endpoint (DeepSeek) | Yes |

Beyond text and tool calling, coverage is uneven and worth reading closely.

| Capability | Live coverage |
|---|---|
| Reasoning effort | OpenAI, both dialects, every level; Gemini, all six levels |
| Chat Completions token cap | Both spellings |
| Structured output | OpenAI and Gemini: nested struct, enum, `Option`, `Vec`, through `generate_as` |
| Streaming, text | All four dialects — deltas, usage on `Done`, and `into_response` parity |
| Streaming, tool calls | **None.** The assembler joining argument fragments is fixture-only |
| Images | An `image_url` part accepted live on OpenAI Chat Completions, on every role. Not exercised on the other three |
| Reasoning effort on Anthropic | **None** |
| Structured output on Anthropic | **None** |

Two entries deserve their asterisks. The Anthropic dialect's live runs go through a compatible endpoint rather than Anthropic's own service, which covers the wire format and not the vendor. And streamed *tool calls* are the part of streaming most likely to differ between vendors, since that is where fragments are joined — so the one path with no live coverage is also the one that would benefit most.

That unevenness is not incidental. Sending Gemini a `temperature` was rejected outright until it was tried, and the offline tests had asserted the wrong shape for as long as they existed. Each dialect's streaming frames are taken from the vendor's own documentation and tested against recorded fixtures, including a test per dialect asserting that a drained stream matches what `generate` builds from the same turn.
