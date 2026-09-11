//! What a summarizer sends, what it does with a reply it cannot use, and how
//! `InMemoryStorage` puts a summary in place of the turns its window drops.

mod common;
use common::serve;
use freyja::{
    Agent, Client, Dialect, EndpointConfig, Error, InMemoryStorage, InputContent, Message, Role,
    Storage, Summarizer,
};

/// A Chat Completions reply carrying `content`, ended for `finish_reason`.
fn reply(content: &str, finish_reason: &str) -> &'static str {
    let body = format!(
        r#"{{"id":"x","model":"test-model","choices":[{{"message":{{"role":"assistant","content":"{content}"}},"finish_reason":"{finish_reason}"}}]}}"#
    );
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    Box::leak(head.into_boxed_str())
}

fn summarizer_for(base: String) -> Summarizer {
    let config =
        EndpointConfig::new(Dialect::OpenAiChat, "local", base).default_model("test-model");
    Summarizer::new(Client::new(config, "sk-test"))
}

/// A user turn, a tool call carrying reasoning, and the result answering it.
fn dropped() -> Vec<Message> {
    vec![
        Message::text(Role::User, "Plan three days in Bergen."),
        Message::new(
            Role::Assistant,
            vec![
                InputContent::Reasoning {
                    data: serde_json::json!({ "signature": "REASONING-PAYLOAD" }),
                },
                InputContent::ToolCall {
                    id: "c1".into(),
                    name: "get_weather".into(),
                    arguments: r#"{"city":"Bergen"}"#.into(),
                },
            ],
        ),
        Message::tool_result("c1", "rain all week"),
    ]
}

#[tokio::test]
async fn sends_one_transcript_under_its_own_instruction() {
    let (base, requests) = serve(&[reply("- Three days in Bergen, rain all week", "stop")]);

    let summary = summarizer_for(base)
        .summarize(&dropped())
        .await
        .expect("summary");
    assert_eq!(summary, "- Three days in Bergen, rain all week");

    let raw = requests.recv().expect("one request");
    let body = &raw[raw.rfind("\r\n").expect("a header line") + 2..];
    let sent: serde_json::Value = serde_json::from_str(body).expect("json body");
    let messages = sent["messages"].as_array().expect("messages array");

    // Two messages, whatever the input held: an instruction and one document.
    // Replaying the turns instead would have the model answer the last one.
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[1]["role"], "user");

    assert!(body.contains("Plan three days in Bergen."));
    assert!(body.contains("get_weather"));
    assert!(body.contains("rain all week"));
    assert!(!body.contains("REASONING-PAYLOAD"));
}

#[tokio::test]
async fn a_reply_it_cannot_use_is_an_error_not_a_summary() {
    let (base, _requests) = serve(&[reply("- Three days in", "length"), reply("", "stop")]);
    let summarizer = summarizer_for(base);

    let cut_off = summarizer.summarize(&dropped()).await;
    assert!(
        matches!(cut_off, Err(Error::InvalidResponse { .. })),
        "{cut_off:?}"
    );

    let empty = summarizer.summarize(&dropped()).await;
    assert!(
        matches!(empty, Err(Error::InvalidResponse { .. })),
        "{empty:?}"
    );
}

#[tokio::test]
async fn dropped_turns_reach_the_model_as_a_summary() {
    let (base, requests) = serve(&[
        reply("ok", "stop"),
        reply("- The user is planning three days in Bergen", "stop"),
        reply("ok", "stop"),
    ]);
    let config =
        EndpointConfig::new(Dialect::OpenAiChat, "local", base).default_model("test-model");
    let client = Client::new(config, "sk-test");
    let storage = InMemoryStorage::new()
        .window(1)
        .summarize(Summarizer::new(client.clone()));
    let mut chat = Agent::new(client).conversation(storage);

    chat.send("Plan three days in Bergen.")
        .await
        .expect("first run");
    chat.send("And the weather?").await.expect("second run");

    // The first run had nothing to drop, so no summarizing call came before it.
    requests.recv().expect("first run");
    let summarizing = requests.recv().expect("summarizing call");
    assert!(summarizing.contains("Plan three days in Bergen."));

    let second = requests.recv().expect("second run");
    let body = &second[second.rfind("\r\n").expect("a header line") + 2..];
    assert!(body.contains("Summary of the earlier conversation"));
    assert!(body.contains("- The user is planning three days in Bergen"));
    assert!(!body.contains("Plan three days in Bergen."));

    assert_eq!(
        chat.storage().summary(),
        Some("- The user is planning three days in Bergen")
    );
    assert_eq!(chat.storage().messages().len(), 4);
}

#[tokio::test]
async fn a_summary_is_reused_while_the_cut_stays_and_cleared_with_the_conversation() {
    // One reply only. A second summarizing call would find no server, fail,
    // and send the plain window, which the loop below would catch.
    let (base, _requests) = serve(&[reply("- SUMMARY", "stop")]);
    let mut storage = InMemoryStorage::new()
        .window(1)
        .summarize(summarizer_for(base));
    storage
        .append(vec![
            Message::text(Role::User, "one"),
            Message::text(Role::Assistant, "two"),
        ])
        .await
        .expect("append");

    for _ in 0..2 {
        let sent = storage.load().await.expect("load");
        assert_eq!(sent.len(), 2);
        assert!(matches!(
            &sent[0].content[0],
            InputContent::Text(text) if text.contains("- SUMMARY")
        ));
    }

    storage.clear().await.expect("clear");
    assert_eq!(storage.summary(), None);
}

#[tokio::test]
async fn a_failed_summary_falls_back_to_the_plain_window() {
    let (base, _requests) = serve(&[reply("- cut off", "length")]);
    let mut storage = InMemoryStorage::new()
        .window(1)
        .summarize(summarizer_for(base));
    storage
        .append(vec![
            Message::text(Role::User, "one"),
            Message::text(Role::Assistant, "two"),
        ])
        .await
        .expect("append");

    let sent = storage
        .load()
        .await
        .expect("a failed summary never fails the load");
    assert_eq!(sent, vec![Message::text(Role::Assistant, "two")]);
    assert_eq!(storage.summary(), None);
}

#[tokio::test]
async fn one_summary_serves_several_turns() {
    // One reply only. A second summarizing call would fail and fall back to
    // the plain window, which the second load's length would show.
    let (base, _requests) = serve(&[reply("- SUMMARY", "stop")]);
    let mut storage = InMemoryStorage::new()
        .window(4)
        .summarize(summarizer_for(base));
    let exchange = || {
        vec![
            Message::text(Role::User, "question"),
            Message::text(Role::Assistant, "answer"),
        ]
    };

    // Six groups against a window of four: two must go, and the summary takes
    // four, down to half the window, leaving the summary and two groups.
    for _ in 0..3 {
        storage.append(exchange()).await.expect("append");
    }
    let sent = storage.load().await.expect("load");
    assert_eq!(sent.len(), 3);
    assert_eq!(storage.summary(), Some("- SUMMARY"));

    // Eight groups: the window needs four dropped and the summary already
    // covers four, so it is reused and the window is full again.
    storage.append(exchange()).await.expect("append");
    let sent = storage.load().await.expect("load");
    assert_eq!(sent.len(), 5);
    assert!(matches!(
        &sent[0].content[0],
        InputContent::Text(text) if text.contains("- SUMMARY")
    ));
}
