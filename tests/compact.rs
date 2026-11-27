//! Summarizing on demand with `Conversation::compact`.

mod common;
use common::{ok_response, serve};
use freyja::{Agent, Client, Dialect, EndpointConfig, InMemoryStorage, Summarizer, Window};

fn reply(content: &str) -> &'static str {
    let body = format!(
        r#"{{"id":"x","model":"test-model","choices":[{{"message":{{"role":"assistant","content":"{content}"}},"finish_reason":"stop"}}]}}"#
    );
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    Box::leak(head.into_boxed_str())
}

fn client(base: String) -> Client {
    Client::new(
        EndpointConfig::new(Dialect::OpenAiChat, "local", base).default_model("test-model"),
        "sk-test",
    )
}

fn sent_messages(raw: &str) -> Vec<serde_json::Value> {
    let body = &raw[raw.rfind("\r\n").expect("a header line") + 2..];
    let sent: serde_json::Value = serde_json::from_str(body).expect("json body");
    sent["messages"].as_array().expect("messages array").clone()
}

#[tokio::test]
async fn compact_takes_effect_before_the_window_needs_it() {
    let (base, requests) = serve(&[
        ok_response(),
        ok_response(),
        reply("- SUMMARY"),
        ok_response(),
    ]);
    let client = client(base);
    let agent =
        Agent::new(client.clone()).memory(Window::groups(10).summarize(Summarizer::new(client)));
    let mut chat = agent.conversation(InMemoryStorage::new());

    chat.send("first").await.expect("run");
    chat.send("second").await.expect("run");
    chat.compact().keep(2).await.expect("compact");
    chat.send("third").await.expect("run");

    // The server records a request after answering it, so wait for each.
    let raw: Vec<String> = (0..4)
        .map(|_| requests.recv().expect("a request"))
        .collect();
    let last = sent_messages(&raw[3]);
    // The summary, the two kept groups, then the new turn.
    assert_eq!(last.len(), 4, "{last:?}");
    assert!(
        last[0]["content"]
            .as_str()
            .unwrap_or_default()
            .contains("- SUMMARY")
    );
    assert_eq!(last[1]["content"], "second");
    assert_eq!(last[3]["content"], "third");
    assert_eq!(
        chat.summary().await.expect("summary"),
        Some("- SUMMARY".to_string())
    );
}

#[tokio::test]
async fn compact_without_keep_keeps_what_the_automatic_summary_would() {
    // A window of four keeps two groups when it summarizes on its own.
    let (base, requests) = serve(&[
        ok_response(),
        ok_response(),
        reply("- SUMMARY"),
        ok_response(),
    ]);
    let client = client(base);
    let agent =
        Agent::new(client.clone()).memory(Window::groups(4).summarize(Summarizer::new(client)));
    let mut chat = agent.conversation(InMemoryStorage::new());

    chat.send("first").await.expect("run");
    chat.send("second").await.expect("run");
    chat.compact().await.expect("compact");
    chat.send("third").await.expect("run");

    let raw: Vec<String> = (0..4)
        .map(|_| requests.recv().expect("a request"))
        .collect();
    let last = sent_messages(&raw[3]);
    assert_eq!(last.len(), 4, "{last:?}");
    assert_eq!(last[1]["content"], "second");
}

#[tokio::test]
async fn compact_needs_a_memory_that_summarizes() {
    // A spare reply, so a request compact wrongly made would be recorded.
    let (base, requests) = serve(&[ok_response(), ok_response()]);
    let plain = Agent::new(client(base.clone())).memory(Window::groups(4));
    let mut chat = plain.conversation(InMemoryStorage::new());
    chat.send("first").await.expect("run");
    assert!(chat.compact().keep(0).await.is_err());

    let none = Agent::new(client(base));
    let mut chat = none.conversation(InMemoryStorage::new());
    assert!(chat.compact().await.is_err());

    // Only the one send reached the server. Compact returned before any
    // request it could have made, so a short wait is enough to see one.
    requests.recv().expect("the send");
    assert!(
        requests
            .recv_timeout(std::time::Duration::from_millis(300))
            .is_err()
    );
}
