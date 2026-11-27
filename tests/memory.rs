//! Storage-driven memory tests, covering the caller-owned path.
//!
//! `tests/agent.rs` covers the stored path, including windowing, since it
//! already has a scripted sequence helper this file does not.

use freyja::{Agent, Client, Dialect, EndpointConfig, Message, Role};

mod common;
use common::{ok_response, serve};

#[tokio::test]
async fn no_policy_sends_the_whole_transcript() {
    let (base, request) = serve(&[ok_response()]);
    let config =
        EndpointConfig::new(Dialect::OpenAiChat, "local", base).default_model("test-model");
    let client = Client::new(config, "sk-test");
    let agent = Agent::new(client);

    let mut messages = vec![
        Message::text(Role::User, "turn-one"),
        Message::text(Role::User, "turn-two"),
    ];

    agent
        .conversation(&mut messages)
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
    let (base, _request) = serve(&["HTTP/1.1 500 Internal Server Error\r\n\
         Content-Type: application/json\r\n\
         Connection: close\r\n\r\n\
         {\"error\":\"boom\"}"]);
    let config =
        EndpointConfig::new(Dialect::OpenAiChat, "local", base).default_model("test-model");
    let client = Client::new(config, "sk-test");
    let agent = Agent::new(client);

    let mut messages = vec![Message::text(Role::User, "turn-one")];
    let original_len = messages.len();

    let result = agent.conversation(&mut messages).send("turn-two").await;

    assert!(result.is_err(), "a 500 must surface as Err");
    assert_eq!(
        messages.len(),
        original_len,
        "the caller's transcript must never be shortened"
    );
}

/// A backend written outside Freyja, with no window of its own.
#[derive(Default)]
struct Plain(Vec<freyja::Message>);

impl freyja::Storage for Plain {
    fn load(&mut self) -> freyja::StorageFuture<'_, Vec<freyja::Message>> {
        Box::pin(async move { Ok(self.0.clone()) })
    }
    fn append(&mut self, messages: Vec<freyja::Message>) -> freyja::StorageFuture<'_, ()> {
        Box::pin(async move {
            self.0.extend(messages);
            Ok(())
        })
    }
    fn clear(&mut self) -> freyja::StorageFuture<'_, ()> {
        Box::pin(async move {
            self.0.clear();
            Ok(())
        })
    }
}

fn sent_messages(raw: &str) -> Vec<serde_json::Value> {
    let body = &raw[raw.rfind("\r\n").expect("a header line") + 2..];
    let sent: serde_json::Value = serde_json::from_str(body).expect("json body");
    sent["messages"].as_array().expect("messages array").clone()
}

#[tokio::test]
async fn a_memory_bounds_a_backend_written_outside_freyja() {
    let (base, requests) = common::serve(&[
        common::ok_response(),
        common::ok_response(),
        common::ok_response(),
    ]);
    let config = freyja::EndpointConfig::new(freyja::Dialect::OpenAiChat, "local", base)
        .default_model("test-model");
    let agent = freyja::Agent::new(freyja::Client::new(config, "sk-test"))
        .memory(freyja::Window::groups(2));
    let mut chat = agent.conversation(Plain::default());

    chat.send("first").await.expect("run");
    chat.send("second").await.expect("run");
    chat.send("third").await.expect("run");

    requests.recv().expect("first request");
    requests.recv().expect("second request");
    let last = sent_messages(&requests.recv().expect("third request"));

    // Two stored groups survive the window, then the new turn.
    assert_eq!(last.len(), 3, "{last:?}");
    assert_eq!(last[0]["content"], "second");
    assert_eq!(last[2]["content"], "third");
    // The backend still holds every turn.
    assert_eq!(chat.storage().0.len(), 6);
}

#[tokio::test]
async fn one_summary_serves_two_sends_over_a_backend_with_no_state() {
    let summary = {
        let body = r#"{"id":"x","model":"test-model","choices":[{"message":{"role":"assistant","content":"- SUMMARY"},"finish_reason":"stop"}]}"#;
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        Box::leak(head.into_boxed_str()) as &'static str
    };
    // Exactly one summarizing reply. A second summarizing call would take the
    // answer meant for the last send, and that send would then find no
    // server and fail.
    let (base, requests) = common::serve(&[
        common::ok_response(),
        common::ok_response(),
        common::ok_response(),
        summary,
        common::ok_response(),
        common::ok_response(),
    ]);
    let config = freyja::EndpointConfig::new(freyja::Dialect::OpenAiChat, "local", base)
        .default_model("test-model");
    let client = freyja::Client::new(config, "sk-test");
    let agent = freyja::Agent::new(client.clone())
        .memory(freyja::Window::groups(4).summarize(freyja::Summarizer::new(client)));

    let mut messages: Vec<freyja::Message> = Vec::new();
    let mut chat = agent.conversation(&mut messages);
    for turn in ["one", "two", "three", "four", "five"] {
        chat.send(turn).await.expect("run");
    }

    // The server records a request after answering it, so wait for each.
    let raw: Vec<String> = (0..6)
        .map(|_| requests.recv().expect("a request"))
        .collect();
    let last = &raw[5];
    assert!(last.contains("- SUMMARY"), "{last}");
}
