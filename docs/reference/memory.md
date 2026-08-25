# Memory

`Memory` decides what part of a transcript reaches the model on a given turn. Without one, `Agent` sends the whole conversation, which is what every release before this one did and is still the default with no policy installed.

## What `select` decides

```rust
pub type MemoryError = Box<dyn std::error::Error + Send + Sync>;
pub type MemoryFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<Message>, MemoryError>> + Send + 'a>>;

pub trait Memory: Send + Sync {
    fn select<'a>(&'a self, history: &'a [Message], cx: &'a Context) -> MemoryFuture<'a>;
}
```

`select` is handed the transcript and the run's `Context`, and returns the messages to send this turn. It runs once per turn of the tool-calling loop, immediately before the request goes out, so a policy that shrinks the transcript sees it shrink again on the next turn if the model keeps calling tools. A policy may drop messages, reorder them, compress them, or prepend something the caller never wrote, such as a summary.

Register one with `Agent::memory`:

```rust
let agent = Agent::new(client).memory(Window::groups(2));
```

## The caller's transcript is never touched

`select` borrows `history`, it does not own it, and `Agent` calls it against a copy. Whatever a policy returns is only what goes on the wire for that request. The `Vec<Message>` the caller passed to `Agent::run` keeps growing exactly as it would with no policy installed, so removing `.memory(...)` from an `Agent` restores the full conversation with no other change.

## Any closure is a policy

```rust
impl<F> Memory for F
where
    F: Fn(&[Message]) -> Vec<Message> + Send + Sync,
{ ... }
```

A plain synchronous filter never has to write a boxed future by hand. Anything shaped `Fn(&[Message]) -> Vec<Message> + Send + Sync` already implements `Memory`, so `.memory(|history: &[Message]| history.iter().rev().take(10).rev().cloned().collect())` is a complete policy. Reach for the trait directly only when a policy needs `Context`, needs to be async, or holds state a closure cannot capture cleanly.

## `Window::groups`

```rust
impl Window {
    pub fn groups(groups: usize) -> Self
}
```

`Window::groups(n)` keeps every pinned turn, meaning `Role::System` and `Role::Developer`, plus the most recent `n` groups.

### What a group is

Grouping is by tool call id, and by nothing else. A pinned turn is set aside and belongs to no group. Every other message starts a new group, with one exception: a tool result whose `call_id` matches a `ToolCall` in the group currently open joins that group instead of starting one.

That rule protects exactly one invariant. A tool result may only answer a call that already happened, and sending a result whose call is missing is rejected by every provider. No provider objects to a question with no answer, or an answer with no question, so nothing else is fused.

The consequence is that a group is usually a single message. Only a call and the results answering it are ever grouped together.

### A worked example

This nine-message transcript is six groups:

| # | Message | Group |
|---|---|---|
| 0 | `System` | pinned, no group |
| 1 | `User "weather?"` | 1 |
| 2 | `Assistant` requesting `call_1` | 2 |
| 3 | `Tool` result for `call_1` | 2, it answers the open call |
| 4 | `Assistant "it is raining"` | 3 |
| 5 | `User "and tomorrow?"` | 4 |
| 6 | `Assistant` requesting `call_2` | 5 |
| 7 | `Tool` result for `call_2` | 5 |
| 8 | `Assistant "sunny"` | 6 |

`Window::groups(2)` keeps groups 5 and 6, so the request carries messages 0, 6, 7 and 8. Messages 1 to 5 are not sent, and they are not removed: your `Vec<Message>` still holds all nine.

The window re-decides from the whole transcript on every turn rather than remembering what it dropped, so it slides forward as the conversation grows.

### Choosing `n`

Count what one exchange costs. Without tools an exchange is a question and an answer, so two groups. With tools it is a question, a call with its results, and an answer, so three groups, and more if the model calls tools twice before answering.

So `n` is roughly three times the number of exchanges you want the model to remember, for a tool-using agent. `Window::groups(10)` keeps about three exchanges, not ten.

A small `n` cuts inside an exchange. In the table above, `groups(2)` hands the model a tool result and an answer with no question, because the user turn at index 5 is its own group and falls outside the window. Every provider accepts that request and the repair pass has nothing to fix, but the model is answering with no idea what was asked. Start at six and adjust down only if you check what is actually being sent.

It counts groups, not tokens. That makes it cheap, since it needs no tokenizer and no knowledge of the target model's limit, but it only bounds how fast a transcript grows. It does not guarantee the result fits the provider's context window. A transcript with unusually long messages inside a small number of groups can still overflow. A token-aware policy is a separate feature that does not exist yet, described below.

## The repair pass

After the last policy runs, `Agent` drops any tool result whose originating call is absent from what the policy returned. A window cut at an arbitrary message boundary is the ordinary way to produce a transcript like this, and it looks correct until the provider rejects it with an error that mentions nothing about the cut. Repair runs unconditionally, once, after every policy has had its turn, so a hand-written policy that trims history without knowing about tool pairing cannot produce a request that fails this way. `Window` never triggers the repair pass, because it only ever cuts on group boundaries, but a policy built any other way can, and does not need to guard against it itself.

## `MemoryError`

```rust
pub type MemoryError = Box<dyn std::error::Error + Send + Sync>;
```

A policy fails with a boxed standard error rather than `freyja::Error`. Every variant of `Error` carries an endpoint, because every variant describes something that went wrong talking to a provider, and a policy has no endpoint of its own. `Agent` wraps whatever a policy returns into `Error::InvalidRequest`, so callers still only ever see one error type from `Agent::run`.

## Composing policies

`Agent::memory` can be called more than once. Policies run in the order added, each receiving the previous one's output, so a token budget and a redaction pass compose without either knowing the other exists:

```rust
let agent = Agent::new(client)
    .memory(redact_secrets)
    .memory(Window::groups(5));
```

The repair pass runs once, after the last policy, not after each one, so an earlier policy is free to produce an intermediate transcript with a dangling tool result as long as a later policy or the repair pass cleans it up before the request is sent.

## What is not built

Token-aware windows, summarization, persistent backends, and retrieval with embeddings and a vector store are not implemented. `Window` bounds a transcript by turn group, which is a coarse and cheap first policy, not a substitute for any of these. Writing a policy that calls out to a summarization model, reads from a database, or queries a vector store is possible today against the `Memory` trait as it stands, since `select` is already async and already fallible, but Freyja ships none of that itself.
