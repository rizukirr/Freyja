# Introduction

Freyja is a Rust library for talking to large language models, and for building agents on top of them.

You write one request. Freyja translates it into whatever wire format the model you picked actually speaks, sends it, and translates the answer back into one response type. Changing model vendor is changing one line.

```rust
let Some(client) = Client::from_env(ProviderType::OpenAi) else {
    // or ProviderType::Anthropic, or ProviderType::Gemini, or your own endpoint.
    // Nothing else in your program changes.
    return Ok(());
};
```

## Why that matters

Every vendor invented a different shape for the same idea.

Asking a model to run a tool and feeding the answer back is one concept. On OpenAI's Responses API it is a flat list of items where tool calls sit beside messages. On Gemini it is a flat list of typed steps with no roles at all. On Anthropic it is blocks nested inside messages. On the Chat Completions format that most other vendors copy, it is a fourth arrangement with a dedicated `tool` role.

Write against one of those directly and you have written a program that only works with one vendor. Write against Freyja and you have written a program that works with all of them, including vendors that did not exist when you wrote it.

## What Freyja is for

**Building agents.** The library's centre of gravity is the tool-calling loop: the model asks for a function, you run it, you feed the result back, it continues. Everything else exists to make that loop work identically on every vendor. If you only want to send a prompt and print a string, Freyja does that too, and a thinner library would also do.

## What Freyja is not

Being clear about this saves you evaluating it for a job it does not do.

- **Not an SDK wrapper.** It does not depend on any vendor's crate. The wire formats are implemented directly, which is why adding a vendor does not mean waiting for someone to publish a binding.
- **Not a framework.** There is no runtime to adopt, no macro DSL, no trait you must implement to get started. It is a client library with an opinionated request type.
- **Not magic.** It never silently changes your request. When you ask for something a vendor cannot express, you get an error before the network call, not a plausible answer that quietly ignored you.
- **Not finished.** Streaming works on every dialect, but retries and automatic tool dispatch do not exist yet. See [Features](features.md) for the honest boundary.

## The three ideas

Everything in Freyja follows from these. [Concepts](concepts.md) covers them properly; here they are in a sentence each.

**One neutral model.** `GenerateRequest` and `GenerateResponse` describe generation in terms that make sense on their own, and never bend toward a vendor. Your code only ever names these.

**Dialects and endpoints are different things.** A *dialect* is a wire format. An *endpoint* is a URL that speaks one. Most hosted inference APIs copy an existing format, so Freyja reaches far more vendors than it has dialects.

**No invented defaults, no silent degradation.** An unset field means "let the vendor decide". A field the vendor cannot honour is an error, never a quiet omission.

## Where to go next

| You want | Read |
|---|---|
| To make a call in five minutes | [Getting started](getting-started.md) |
| To know what works today | [Features](features.md) |
| To understand the design before committing | [Concepts](concepts.md) |
| To build an agent | [Building an agent](building-an-agent.md) |
| To reach a vendor not listed | [Custom providers](providers/custom.md) |
