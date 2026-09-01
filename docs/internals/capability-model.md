# The capability model

What Freyja is allowed to decide on a vendor's behalf, and why the answer is "almost nothing".

This is the reasoning behind [`src/dialect/refusal.rs`](../../src/dialect/refusal.rs), the eleven fields on `GenerateRequest`, and the existence of `extra_for`. [Concepts](../concepts.md) covers the same ground for people *using* Freyja; this page is for people changing it.

## The problem

Freyja's promise is that one request runs anywhere. Keeping that promise means sometimes saying **no**: a silently dropped `tool_choice` returns a plausible answer that is wrong in a way the caller cannot see. Refusal is load-bearing, not a fallback.

But a refusal is a **claim**, and for a long time Freyja made claims it had no way to check. Fifteen refusals shipped. Seven should not have, four were false, three were true but not Freyja's to make. That is the entire motivation for everything below.

## Three layers of knowledge

The whole model falls out of noticing that "will this work?" is three different questions, and Freyja can only answer one of them.

```
   ┌──────────────────────────────────────────────────────────────────┐
   │  MODEL         "will gpt-5.6 accept reasoning effort 'max'?"     │
   │                changes weekly · nobody knows in advance          │
   ├──────────────────────────────────────────────────────────────────┤
   │  ENDPOINT      "does this deployment allow `labels`?"            │
   │                changes monthly · the operator knows, and         │
   │                tells Freyja through EndpointConfig               │
   ├──────────────────────────────────────────────────────────────────┤
   │  WIRE FORMAT   "is there a field for this at all?"               │
   │                changes yearly · provable by construction         │
   │                                                                  │
   │                ◀── the only layer Freyja can see ───────────     │
   └──────────────────────────────────────────────────────────────────┘
```

Every wrong refusal was a lower-layer fact asserted at the top layer:

| Refusal | Looked like | Actually was |
|---|---|---|
| Gemini `reasoning_effort` | the format lacks it | we looked at the top level of a request that nests |
| Gemini `tool_choice` | the format lacks it | same mistake, same file |
| Images outside user turns | the format lacks it | true for one dialect, applied to four |
| Gemini `labels` | the format lacks it | an **endpoint** gates a field the format has |
| Gemini effort `Max` | the format lacks it | a **model** rejects a value the field takes |

The first three were false. The last two were accurate and still wrong to make, because they described a layer Freyja cannot see changing.

## The law

> **Freyja refuses only what it can prove structurally impossible. Everything else goes to the wire, and the vendor answers.**

Not a matter of taste: it is the table above. Freyja may only speak about the layer it can see.

```
   a field set on GenerateRequest
              │
              ▼
   ┌─────────────────────────────────────────┐
   │ does this dialect have a wire location  │
   │ for it?                                 │
   └─────────────────────────────────────────┘
        │                          │
       no                         yes
        │                          │
        ▼                          ▼
   UnsupportedCapability      translate and send
   (the only legitimate            │
    refusal, and it is        ┌────┴────┐
    provable once)            ▼         ▼
                          accepted   BadRequest
                                     (the vendor's answer,
                                      in the vendor's words)
```

There is deliberately no third branch for "the field exists but we're fairly sure this value fails". That branch is where every stale claim lived.

### Why the round trips are worth paying

The cost is real: `metadata` on Gemini and `reasoning_effort(Max)` on Gemini now each cost a network call to be told no.

The reason to pay is **asymmetry of failure**.

```
   a wrong refusal            a wrong send
   ───────────────            ────────────
   silent                     loud
   permanent                  stops the day the vendor changes
   no signal to the caller    a BadRequest with the vendor's text
   costs a capability         costs one round trip
```

The vendor's message is also usually better than ours. Compare:

```
ours    Gemini does not support reasoning effort 'max'
theirs  The value 'max' is not supported for 'generation_config.thinking_level'.
        Supported values: 'minimal', 'low', 'medium', 'high'.
```

Ours names the failure. Theirs names the fix. Freyja was substituting a worse error for a better one to save a network call.

## Minimal and complete are not in tension

They conflict only if every vendor feature has to become a neutral field. Split the problem and both hold at once.

```
   a vendor feature
          │
          ▼
   ┌──────────────────────────────────────────────────────┐
   │ do two or more dialects have a wire location for it, │
   │ and does it name an intent rather than a field?      │
   └──────────────────────────────────────────────────────┘
        │                                  │
       yes                                no
        │                                  │
        ▼                                  ▼
   the neutral model                  extra_for(dialect, …)
   11 fields, genuinely portable      reachable, scoped, and
   one name per capability            unportable on purpose
```

Test it against reality: `previous_response_id` has a location on two dialects, so it is admitted. `metadata` on three, admitted. `safety_settings` on one, escape hatch. a `minimal` reasoning level is accepted by exactly one vendor, so it is not a rung on a portable ladder. There is deliberately no `ReasoningEffort::Minimal`: it would be a Gemini string wearing a portable name, and it lives in the hatch instead. See [Requests](../reference/requests.md) for how that variant came and went.

This is why the neutral model stays small **without** the library becoming a subset of what vendors offer.

## The five parts

Each exists to answer a question that would otherwise be answered by taste.

| | What it is | The question it settles |
|---|---|---|
| **Capability** | a portable intent | should this be a field on `GenerateRequest`? |
| **Neutral model** | one name per capability | what does the caller write? |
| **Dialect** | a wire format with an ecosystem | should this be a new dialect? |
| **Endpoint** | a deployment of a dialect | where does non-capability variance go? |
| **Escape hatch** | `extra_for`, `EndpointConfig::body` | how do I send what is not a capability? |

A wire format earns a dialect slot when **at least two independent vendors speak it**. Under that test only `OpenAiChat` really earns one, Groq, DeepSeek, Together, OpenRouter, Ollama, vLLM. The other three exist because their vendors matter, not because their formats spread. Worth knowing before proposing a fifth.

`Endpoint` is the part most often collapsed into `Dialect`, and the collapse is where bugs come from. `max_tokens` versus `max_completion_tokens` is not a capability difference; neither is `labels` working on Gemini Enterprise and not on the public API. Both are endpoint facts, and both were briefly modelled as dialect facts.

## The rack dispatches; the engine translates

```
                    ┌──────────────────────────────┐
   GenerateRequest ─│  Client  (the rack)          │
                    │  picks a dialect, posts,     │
                    │  classifies the failure      │
                    └───┬──────┬──────┬──────┬─────┘
                        │      │      │      │
                     ┌──▼──┐┌──▼──┐┌──▼──┐┌──▼──┐
                     │ Resp││ Chat││ Gem ││ Anth│   each owns its own
                     └─────┘└─────┘└─────┘└─────┘   conversion, both ways
```

The rack must never translate. A rack that knows all four wire formats grows with every fifth one added, and every dialect change becomes a change to shared code. Translation stays in the engine.

The current rack has **welded slots**: `Dialect` is a closed enum dispatched by `match` at four sites, and `Provider::Request` is an associated type, so the trait is not object-safe and no external crate can supply a dialect. Making the slots real is a genuine architectural change, described in [Architecture](architecture.md). Nothing today is blocked by it.

## Where it happens in the code

```
   GenerateRequest
        │
        ▼
   Provider::build ────────────▶ types::Request::build   ◀── refusals fire here
        │                        (per dialect)               and nowhere else
        ▼
   to_value ───────────────────▶ serialize, then merge   ◀── escape hatches
        │                        config.extra_body,          merge here
        │                        then request.extra
        ▼
   Client::post ───────────────▶ the network
```

`Client::check` is `build` with the result dropped. That is why it cannot be wrong independently of `generate`, and why "fix `check`" turned out to mean "fix `build`", `check` documented its contract correctly from the beginning.

Refusals are not a validation prologue. Some are whole-request gates at the top of `build`; the rest sit **inline, in the same match arm as the mapping they are refusing**:

```rust
InputContent::ImageUrl(url) => {
    if message.role == Role::Tool { return Err(…); }
    pending.push(json!({"type": "image", "uri": url}));
}
```

The `return Err` and the `push` are alternatives. A refusal cannot drift out of sync with the mapping it refuses, because changing one puts you in the other. It is also why `check` takes a whole request rather than exposing a capability matrix: the same image is fine one turn earlier, and no table of booleans can say that.

## Enforcement

The law was already the informal principle when seven refusals violated it. Principles do not enforce themselves.

What made it real is [`refusal.rs`](../../src/dialect/refusal.rs): every refusal names a constant declared in one file and appears in a table with its evidence.

| Evidence | Meaning |
|---|---|
| `Probed` | the endpoint was asked and said no |
| `Structural` | no field could exist: the API is stateless, or the concept is absent |
| `Unverified` | nobody has checked; this is how the false ones were written |
| `Refuted` | the endpoint was asked and said yes, and the code still refuses |

A probe is only worth its category against an endpoint that rejects what it does not recognize. Three Anthropic refusals sat at `Unverified` because they had been probed against a compatible endpoint that accepted an invented top-level key, so its acceptance said nothing about the schema. Anthropic answers `freyja_invented_field: Extra inputs are not permitted`, and that control is what makes everything measured against it mean something. Send the nonsense field first.

A test asserts the count in each category. Probing a refusal fails CI until the row is updated; **adding an unverified refusal fails CI too.** Both directions are deliberate.

That converts "did anyone verify this?" from archaeology into a table lookup, which is the part a principle cannot do.

## Two asymmetries worth internalizing

**Requests reject; responses preserve.** `parse` never refuses. An unrecognized finish reason becomes `ResponseStatus::Other(String)`; an unmodelled step becomes opaque `OutputContent::Reasoning` and replays verbatim. Losing data on the way back is unacceptable, while sending data the format cannot hold is impossible. For a long time responses had escape valves and requests had none; `extra_for` closed that.

**Freyja's own fields are the weakest.** Precedence runs dialect → endpoint → call. A caller reaching for an escape hatch has said what they want more plainly than the neutral model could infer, so the hatch wins.

## What this does not solve

**Model-level variance.** Freyja knows the wire format and never the model. A table of what `gpt-5.6` accepts would be confidently wrong within a month, so there is no such table and there should not be one.

The consequence is real and worth stating plainly: an application that switches models at runtime still has to catch `BadRequest` and decide for itself what to drop. `Error` classifies the failure and `Client::check` rules out the format-level problems for free, but the last mile belongs to the caller. The model makes that boundary explicit rather than pretending to fix it.

**Cross-vendor reasoning state.** Opaque reasoning blobs are replayed faithfully, which is correct within a vendor and fatal across one. Switching mid-conversation means stripping `InputContent::Reasoning` from the transcript, and Freyja offers no helper for that yet.
