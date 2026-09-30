//! What `JsonlStorage` leaves on disk, and what a later process reads back.

mod common;
use common::serve;
use freyja::{
    Client, Dialect, EndpointConfig, InputContent, JsonlStorage, Message, Role, Storage, Summarizer,
};
use std::io::Write;
use std::path::PathBuf;

/// A path no other test and no other run of this binary is using.
fn path(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("freyja-{name}-{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

/// A Chat Completions reply carrying `content`.
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

fn summarizer_for(base: String) -> Summarizer {
    let config =
        EndpointConfig::new(Dialect::OpenAiChat, "local", base).default_model("test-model");
    Summarizer::new(Client::new(config, "sk-test"))
}

#[tokio::test]
async fn a_reopened_file_holds_what_was_appended() {
    let path = path("reopen");
    // A newline inside a turn must stay inside its line, and every variant
    // must come back as the variant it went in as.
    let turns = vec![
        Message::text(Role::User, "line one\nline two"),
        Message::new(
            Role::Assistant,
            vec![
                InputContent::Reasoning {
                    data: serde_json::json!({ "signature": "opaque" }),
                },
                InputContent::ToolCall {
                    id: "c1".into(),
                    name: "get_weather".into(),
                    arguments: r#"{"city":"Bergen"}"#.into(),
                },
            ],
        ),
        Message::tool_result("c1", "rain all week"),
    ];

    let mut storage = JsonlStorage::open(&path).expect("open");
    storage.append(turns[..1].to_vec()).await.expect("append");
    storage.append(turns[1..].to_vec()).await.expect("append");
    drop(storage);

    let mut storage = JsonlStorage::open(&path).expect("reopen");
    assert_eq!(storage.load().await.expect("load"), turns);
}

#[tokio::test]
async fn a_torn_tail_is_cut_off_and_the_next_append_is_whole() {
    let path = path("torn");
    let mut storage = JsonlStorage::open(&path).expect("open");
    storage
        .append(vec![Message::text(Role::User, "kept")])
        .await
        .expect("append");
    drop(storage);

    // What a process killed inside a write leaves behind.
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("file");
    file.write_all(br#"{"message":{"role":"user","con"#)
        .expect("fragment");
    drop(file);

    let mut storage = JsonlStorage::open(&path).expect("reopen over a torn tail");
    storage
        .append(vec![Message::text(Role::Assistant, "after")])
        .await
        .expect("append");
    drop(storage);

    // A fragment left in place would have had this append glued onto it,
    // and this open would fail on the line the two made together.
    let storage = JsonlStorage::open(&path).expect("reopen");
    assert_eq!(
        storage.messages(),
        [
            Message::text(Role::User, "kept"),
            Message::text(Role::Assistant, "after"),
        ]
    );
}

#[tokio::test]
async fn clear_empties_the_file_and_succeeds_twice() {
    let path = path("clear");
    let mut storage = JsonlStorage::open(&path).expect("open");
    storage
        .append(vec![Message::text(Role::User, "gone")])
        .await
        .expect("append");
    storage.clear().await.expect("clear");
    storage
        .clear()
        .await
        .expect("clear on an empty conversation");
    storage
        .append(vec![Message::text(Role::User, "new")])
        .await
        .expect("append after clear");
    drop(storage);

    let storage = JsonlStorage::open(&path).expect("reopen");
    assert_eq!(storage.messages(), [Message::text(Role::User, "new")]);
}

#[tokio::test]
async fn a_reopened_conversation_reuses_the_stored_summary() {
    let path = path("summary");
    // One reply only. A second summarizing call would find no server, fail,
    // and send the plain window, which the assertion below would catch.
    let (base, _requests) = serve(&[reply("- SUMMARY")]);

    let mut storage = JsonlStorage::open(&path)
        .expect("open")
        .window(1)
        .summarize(summarizer_for(base.clone()));
    storage
        .append(vec![
            Message::text(Role::User, "one"),
            Message::text(Role::Assistant, "two"),
        ])
        .await
        .expect("append");
    storage.load().await.expect("load");
    assert_eq!(storage.summary(), Some("- SUMMARY"));
    drop(storage);

    let mut storage = JsonlStorage::open(&path)
        .expect("reopen")
        .window(1)
        .summarize(summarizer_for(base));
    assert_eq!(storage.summary(), Some("- SUMMARY"));

    let sent = storage.load().await.expect("load");
    assert_eq!(sent.len(), 2);
    assert!(matches!(
        &sent[0].content[0],
        InputContent::Text(text) if text.contains("- SUMMARY")
    ));

    // A cleared conversation must not open its next one with the old summary.
    storage.clear().await.expect("clear");
    drop(storage);
    assert_eq!(JsonlStorage::open(&path).expect("reopen").summary(), None);
}

#[test]
fn a_summary_covering_more_than_the_file_holds_is_dropped() {
    let path = path("stale");
    std::fs::write(
        &path,
        "{\"header\":{\"version\":1}}\n{\"summary\":{\"covers\":9,\"text\":\"stale\"}}\n",
    )
    .expect("write");

    assert_eq!(JsonlStorage::open(&path).expect("open").summary(), None);
}

#[test]
fn a_file_it_cannot_read_is_refused() {
    let newer = path("newer");
    std::fs::write(&newer, "{\"header\":{\"version\":2}}\n").expect("write");
    assert!(JsonlStorage::open(&newer).is_err());

    // A bad line with a newline after it is not a torn tail, so it is
    // corruption rather than something to cut off quietly.
    let corrupt = path("corrupt");
    std::fs::write(&corrupt, "{\"header\":{\"version\":1}}\nnot json\n").expect("write");
    assert!(JsonlStorage::open(&corrupt).is_err());
}
