//! A policy written outside the crate, using only the public API.
//!
//! Stands in for a third-party memory crate: if this compiles, so does one.

use freyja::{
    Agent, Client, Context, Dialect, EndpointConfig, Filter, FilterFuture, Message, Role, Window,
};

mod common;
use common::serve_once;

/// Builds a canned non-streaming OpenAI Chat Completions response with a
/// correct `Content-Length`, leaked so it satisfies `serve_once`'s
/// `&'static str`.
fn ok_response() -> &'static str {
    let body = r#"{"id":"x","model":"test-model","choices":[{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}]}"#;
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    Box::leak(head.into_boxed_str())
}

/// Prepends a message that was never in the caller's transcript, which is how
/// retrieval attaches to this seam without a second trait.
struct Prepend {
    recalled: Message,
}

impl Filter for Prepend {
    fn select<'a>(&'a self, history: &'a [Message], _cx: &'a Context) -> FilterFuture<'a> {
        Box::pin(async move {
            let mut selected = vec![self.recalled.clone()];
            selected.extend(history.iter().cloned());
            Ok(selected)
        })
    }
}

/// Fails with an error of its own, which is why `select` returns a boxed
/// standard error rather than `freyja::Error`: a policy has no endpoint.
struct Broken;

impl Filter for Broken {
    fn select<'a>(&'a self, _history: &'a [Message], _cx: &'a Context) -> FilterFuture<'a> {
        Box::pin(async { Err(std::io::Error::other("backend unreachable").into()) })
    }
}

#[tokio::test]
async fn a_policy_may_add_messages() {
    let recalled = Message::text(Role::User, "remembered from last week");
    let policy = Prepend {
        recalled: recalled.clone(),
    };
    let history = vec![Message::text(Role::User, "hello")];
    let selected = policy.select(&history, &Context::new()).await.unwrap();
    assert_eq!(selected.first(), Some(&recalled));
    assert_eq!(selected.len(), history.len() + 1);
}

#[tokio::test]
async fn a_closure_from_outside_the_crate_is_a_policy() {
    let policy = |history: &[Message]| history.iter().rev().cloned().collect::<Vec<_>>();
    let history = vec![
        Message::text(Role::User, "first"),
        Message::text(Role::User, "second"),
    ];
    let selected = policy.select(&history, &Context::new()).await.unwrap();
    assert_eq!(selected.first(), history.last());
}

#[tokio::test]
async fn a_policy_may_fail_with_its_own_error() {
    assert!(Broken.select(&[], &Context::new()).await.is_err());
}

#[tokio::test]
async fn no_policy_sends_the_whole_transcript() {
    let (base, request) = serve_once(ok_response());
    let config =
        EndpointConfig::new(Dialect::OpenAiChat, "local", base).default_model("test-model");
    let client = Client::new(config, "sk-test");
    let agent = Agent::new(client);

    let mut messages = vec![
        Message::text(Role::User, "turn-one"),
        Message::text(Role::User, "turn-two"),
        Message::text(Role::User, "turn-three"),
    ];

    agent.messages(&mut messages).await.expect("run succeeds");

    let sent = request.recv().expect("captured request");
    assert!(sent.contains("turn-one"), "{sent}");
    assert!(sent.contains("turn-two"), "{sent}");
    assert!(sent.contains("turn-three"), "{sent}");
}

#[tokio::test]
async fn a_window_sends_less_than_the_whole_transcript() {
    let (base, request) = serve_once(ok_response());
    let config =
        EndpointConfig::new(Dialect::OpenAiChat, "local", base).default_model("test-model");
    let client = Client::new(config, "sk-test");
    let agent = Agent::new(client).filter(Window::groups(1));

    let mut messages = vec![
        Message::text(Role::User, "turn-one"),
        Message::text(Role::User, "turn-two"),
        Message::text(Role::User, "turn-three"),
    ];
    let original_len = messages.len();

    agent.messages(&mut messages).await.expect("run succeeds");

    let sent = request.recv().expect("captured request");
    assert!(
        !sent.contains("turn-one"),
        "the oldest turn must be absent: {sent}"
    );
    assert!(sent.contains("turn-three"), "{sent}");
    // Only the newest group reaches the wire, so exactly one user turn is
    // present where the whole transcript would carry three: strictly less.
    assert_eq!(
        sent.matches("\"role\":\"user\"").count(),
        1,
        "strictly less than the whole transcript: {sent}"
    );

    // The run still appends its own turn, on top of everything the caller
    // already had, even though the wire only saw the trimmed window.
    assert_eq!(messages.len(), original_len + 1);
}

#[tokio::test]
async fn a_failed_request_does_not_shorten_the_caller_transcript() {
    let (base, _request) = serve_once(
        "HTTP/1.1 500 Internal Server Error\r\n\
         Content-Type: application/json\r\n\
         Connection: close\r\n\r\n\
         {\"error\":\"boom\"}",
    );
    let config =
        EndpointConfig::new(Dialect::OpenAiChat, "local", base).default_model("test-model");
    let client = Client::new(config, "sk-test");
    let agent = Agent::new(client).filter(Window::groups(1));

    let mut messages = vec![
        Message::text(Role::User, "turn-one"),
        Message::text(Role::User, "turn-two"),
    ];
    let original_len = messages.len();

    let result = agent.messages(&mut messages).await;

    assert!(result.is_err(), "a 500 must surface as Err");
    assert_eq!(
        messages.len(),
        original_len,
        "the caller's transcript must never be shortened"
    );
}

/// Returns the whole transcript with `true` in the context, or only the
/// newest message without it, so `select` has a caller for the `cx`
/// parameter it otherwise ignores.
struct ContextAware;

impl Filter for ContextAware {
    fn select<'a>(&'a self, history: &'a [Message], cx: &'a Context) -> FilterFuture<'a> {
        Box::pin(async move {
            let keep_all = cx.get::<bool>().copied().unwrap_or(false);
            let selected = if keep_all {
                history.to_vec()
            } else {
                history.iter().rev().take(1).cloned().collect()
            };
            Ok(selected)
        })
    }
}

#[tokio::test]
async fn a_policy_reads_the_context() {
    let history = vec![
        Message::text(Role::User, "first"),
        Message::text(Role::User, "second"),
    ];

    let mut without = Context::new();
    without.insert(false);
    let trimmed = ContextAware.select(&history, &without).await.unwrap();
    assert_eq!(trimmed.len(), 1);

    let mut with = Context::new();
    with.insert(true);
    let whole = ContextAware.select(&history, &with).await.unwrap();
    assert_eq!(whole.len(), history.len());
}
