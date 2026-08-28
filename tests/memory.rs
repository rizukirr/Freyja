//! Storage-driven memory tests, covering the caller-owned path.
//!
//! `tests/agent.rs` covers the stored path, including windowing, since it
//! already has a scripted sequence helper this file does not.

use freyja::{Agent, Client, Dialect, EndpointConfig, Message, Role};

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
    ];

    agent
        .conversation_in(&mut messages)
        .send("turn-three")
        .await
        .expect("run succeeds");

    let sent = request.recv().expect("captured request");
    assert!(sent.contains("turn-one"), "{sent}");
    assert!(sent.contains("turn-two"), "{sent}");
    assert!(sent.contains("turn-three"), "{sent}");
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
    let agent = Agent::new(client);

    let mut messages = vec![Message::text(Role::User, "turn-one")];
    let original_len = messages.len();

    let result = agent.conversation_in(&mut messages).send("turn-two").await;

    assert!(result.is_err(), "a 500 must surface as Err");
    assert_eq!(
        messages.len(),
        original_len,
        "the caller's transcript must never be shortened"
    );
}
