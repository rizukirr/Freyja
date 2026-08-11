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
| Reasoning effort | Where the provider supports it, refused where it does not |
| Structured output | JSON schema, and free JSON where the provider offers it |
| Token accounting | Yes, normalized across providers |
| Streaming | Yes, on every dialect, see [Streaming](reference/streaming.md) |

Streaming delivers the same answer incrementally, and a drained stream converts back into the same `GenerateResponse`, so a tool loop written against `generate` keeps working. Tool-call arguments and reasoning blobs are assembled for you and surface only once complete, so nothing stitches partial JSON.

### Building agents

| | |
|---|---|
| Declaring tools | Yes, with JSON Schema parameters |
| Constraining tool choice | Auto, none, required, or a named tool |
| The full tool round trip | **Yes, verified against live APIs**, not just tested offline |
| Multi-turn conversations | Yes, transcripts are plain data you own |
| Reasoning state replay | Handled for you, see [Concepts](concepts.md#opaque-state) |

The round trip is the load-bearing feature. A model asks for a function, you run it, you feed the result back, and it continues. [Building an agent](building-an-agent.md) is the guide.

### Reaching providers

| | |
|---|---|
| Built-in | OpenAI, Google Gemini, Anthropic |
| Wire dialects | Four, including the format most third-party vendors copy |
| Any other endpoint | One `Client::custom` call, no code change |
| Local runtimes | Yes, no credentials required |

Built-in means Freyja ships and tests the URL and default model. It does not mean the others are second class: DeepSeek, Groq, OpenRouter, Ollama, and a long tail of others work through [Custom providers](providers/custom.md) with the same code paths.

### Operational

| | |
|---|---|
| Connection pooling | One HTTP client per `Client`, reused |
| Timeouts | 120 seconds of inactivity by default, or supply your own HTTP client |
| Errors | Classified by cause, each attributed to the endpoint that failed |
| Retry decisions | `is_retryable()` and the endpoint's own `Retry-After` |
| Credential safety | `Debug` redacts the API key |
| Dependencies | Three: `reqwest`, `serde`, `serde_json` |

## What does not exist yet

Be sure none of these is on your critical path before adopting.

| | Status | Workaround |
|---|---|---|
| **Retries** | Out of scope, deliberately | A 429 or 5xx surfaces once. `is_retryable()` and `retry_after()` tell you what to do; the loop is yours, and composes with `backon` or `tower::retry`. See [Errors](reference/errors.md#retries). |
| **Automatic tool dispatch** | Not implemented | You match on the tool name and call your function. There is no registry or `Tool` trait yet. |
| **Schema derivation** | Not implemented | Tool parameter schemas are hand-written JSON. |
| **Capability introspection** | Not implemented | You discover an unsupported capability by getting an error, not by asking first. |
| **Embeddings, memory, RAG** | Not implemented | Freyja is a generation client today. |
| **Agent orchestration** | Not implemented | You write the loop. It is about fifteen lines. |

## Per-provider gaps

Capability coverage is not uniform, and Freyja refuses rather than pretending.

| Capability | OpenAI | Gemini | Anthropic | Chat Completions |
|---|---|---|---|---|
| `tool_choice` | Yes | **No** | Yes | Yes |
| `reasoning_effort` | Yes | **No** | Partly | Yes |
| `response_format` | Yes | Yes | Schema only | Yes |
| `previous_response_id` | Yes | Yes | **No** | **No** |

A **No** means `UnsupportedCapability` before any network call, so you find out immediately rather than getting an answer that ignored you. Each provider page has the full table.

## Verification status

Freyja distinguishes "the tests pass" from "a real vendor accepted it", because the two came apart once already and cost three bugs.

| Endpoint | Live tool round trip |
|---|---|
| OpenAI | Yes |
| Gemini | Yes |
| Anthropic | Yes |
| A Chat Completions endpoint (DeepSeek) | Yes |

Beyond text and tool calling, coverage is offline tests only. Images, structured output, reasoning effort, and streaming have not been exercised against a live API on any provider. Each dialect's streaming frames are taken from the vendor's own documentation and tested against recorded fixtures, including a test per dialect asserting that a drained stream matches what `generate` builds from the same turn.
