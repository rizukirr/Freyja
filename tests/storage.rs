//! A storage backend written outside the crate, using only the public API.
//!
//! Stands in for a third-party persistence crate: if this compiles, so does one.

mod common;
use common::serve;
use freyja::{
    Agent, Client, Dialect, EndpointConfig, InMemoryStorage, InputContent, Message, Role, Storage,
    StorageFuture,
};

/// A backend that records every append, so a test can see what was stored.
///
/// `Storage` takes `&mut self`, so the fields need no lock: a `Conversation`
/// owns its backend outright, and nothing else can reach it while it does.
#[derive(Default)]
struct Recording {
    messages: Vec<Message>,
    appends: usize,
    clears: usize,
}

impl Storage for Recording {
    fn load(&mut self) -> StorageFuture<'_, Vec<Message>> {
        Box::pin(async move { Ok(self.messages.clone()) })
    }

    fn append(&mut self, messages: Vec<Message>) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            self.appends += 1;
            self.messages.extend(messages);
            Ok(())
        })
    }

    fn clear(&mut self) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            self.clears += 1;
            self.messages.clear();
            Ok(())
        })
    }
}

/// A backend whose load fails, which must abort the run rather than continue
/// with no history.
struct Broken;

impl Storage for Broken {
    fn load(&mut self) -> StorageFuture<'_, Vec<Message>> {
        Box::pin(async { Err(std::io::Error::other("backend unreachable").into()) })
    }
    fn append(&mut self, _messages: Vec<Message>) -> StorageFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn clear(&mut self) -> StorageFuture<'_, ()> {
        Box::pin(async { Err(std::io::Error::other("backend unreachable").into()) })
    }
}

/// Built with a derived `Content-Length` and leaked to satisfy `serve`'s
/// `&'static str`, the same way `tests/memory.rs` does it.
fn ok_response() -> &'static str {
    let body = r#"{"id":"x","model":"test-model","choices":[{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}]}"#;
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    Box::leak(head.into_boxed_str())
}

fn agent_for(base: String) -> Agent {
    let config =
        EndpointConfig::new(Dialect::OpenAiChat, "local", base).default_model("test-model");
    Agent::new(Client::new(config, "sk-test"))
}

#[tokio::test]
async fn a_third_party_backend_holds_the_conversation() {
    let (base, _requests) = serve(&[ok_response()]);
    let agent = agent_for(base);
    let mut chat = agent.conversation(Recording::default());

    chat.send("a question").await.expect("run");

    let held = chat.storage();
    assert!(
        held.messages
            .iter()
            .any(|m| format!("{m:?}").contains("a question"))
    );
    assert_eq!(held.appends, 1);
}

#[tokio::test]
async fn a_failing_load_aborts_the_run() {
    let (base, requests) = serve(&[ok_response()]);
    let agent = agent_for(base);
    let mut chat = agent.conversation(Broken);

    assert!(chat.send("a question").await.is_err());
    assert!(requests.try_recv().is_err(), "no request may be sent");
}

#[tokio::test]
async fn a_borrowed_vector_is_extended_in_place() {
    let (base, _requests) = serve(&[ok_response()]);
    let agent = agent_for(base);
    let mut history: Vec<Message> = Vec::new();
    let starting_len = history.len();

    let mut chat = agent.conversation(&mut history);
    chat.send("a question").await.expect("run");
    drop(chat);

    assert!(history.len() > starting_len);
}

#[tokio::test]
async fn window_shapes_what_is_sent_while_the_backend_keeps_everything() {
    let (base, requests) = serve(&[ok_response(), ok_response(), ok_response()]);
    let agent = agent_for(base);
    let mut chat = agent.conversation(InMemoryStorage::new().window(1));

    chat.send("first").await.expect("run");
    chat.send("second").await.expect("run");
    chat.send("third").await.expect("run");

    requests.recv().expect("first request");
    requests.recv().expect("second request");
    let last = requests.recv().expect("third request");
    let split_at = last.rfind("\r\n").expect("a header line") + 2;
    let body = &last[split_at..];
    let sent: serde_json::Value = serde_json::from_str(body).expect("json body");
    let sent_len = sent["messages"].as_array().expect("messages array").len();

    assert!(sent_len < chat.storage().messages().len());
}

#[tokio::test]
async fn send_carries_every_content_block() {
    let (base, requests) = serve(&[ok_response()]);
    let agent = agent_for(base);
    let mut chat = agent.conversation(InMemoryStorage::new());

    let message = Message::new(
        Role::User,
        vec![
            InputContent::Text("first block".to_string()),
            InputContent::Text("second block".to_string()),
        ],
    );
    chat.send(message).await.expect("run");

    let sent = requests.recv().expect("request");
    assert!(sent.contains("first block"));
    assert!(sent.contains("second block"));
}

#[tokio::test]
async fn the_backend_hands_back_what_it_holds_without_cloning() {
    let (base, _requests) = serve(&[ok_response()]);
    let agent = agent_for(base);
    let mut chat = agent.conversation(InMemoryStorage::new());

    chat.send("a question").await.expect("run");

    // `messages` borrows rather than cloning, so the slice it returns is the
    // transcript itself and not a copy of it.
    let held: &[Message] = chat.storage().messages();
    assert!(std::ptr::eq(held, chat.storage().messages()));

    // And it is the transcript, not some other vector: the turn just sent is
    // in it, followed by the answer.
    assert_eq!(held.first().map(|message| message.role), Some(Role::User));
    assert_eq!(
        held.last().map(|message| message.role),
        Some(Role::Assistant)
    );
}

#[tokio::test]
async fn a_pending_tool_call_is_repaired_after_the_answer_arrives() {
    let (base, requests) = serve(&[ok_response()]);
    let agent = agent_for(base);
    let backend = Recording {
        messages: vec![
            Message::text(Role::User, "call a tool"),
            Message::new(
                Role::Assistant,
                vec![InputContent::ToolCall {
                    id: "call_1".to_string(),
                    name: "t".to_string(),
                    arguments: "{}".to_string(),
                }],
            ),
        ],
        appends: 0,
        clears: 0,
    };
    let mut chat = agent.conversation(backend);

    chat.send(Message::tool_result("call_1", "42"))
        .await
        .expect("run");

    let sent = requests.recv().expect("request");
    assert!(
        sent.contains(r#""id":"call_1""#),
        "tool call missing from the request: {sent}"
    );
    assert!(
        sent.contains(r#""tool_call_id":"call_1""#),
        "tool result missing from the request: {sent}"
    );
}

#[tokio::test]
async fn an_unmatched_tool_result_is_refused_rather_than_recorded() {
    let (base, _requests) = serve(&[ok_response()]);
    let agent = agent_for(base);
    let mut chat = agent.conversation(Recording::default());

    chat.send(Message::tool_result("call_missing", "42"))
        .await
        .expect_err("a tool result answering no open call must be refused");

    let held = chat.storage();
    assert!(
        held.messages.is_empty(),
        "a refused turn must record nothing"
    );
}

#[tokio::test]
async fn a_turn_answering_no_open_call_is_refused() {
    let (base, requests) = serve(&[ok_response()]);
    let agent = agent_for(base);
    let backend = Recording {
        messages: vec![Message::text(Role::User, "hello")],
        appends: 0,
        clears: 0,
    };
    let mut chat = agent.conversation(backend);
    let before = chat.storage().messages.clone();

    let error = chat
        .send(Message::tool_result("call_missing", "42"))
        .await
        .expect_err("a tool result answering no open call must be refused");

    assert!(
        error.to_string().contains("call_missing"),
        "error should name the orphaned call: {error}"
    );
    assert!(requests.try_recv().is_err(), "no request may be sent");
    assert_eq!(chat.storage().messages, before);
}

#[tokio::test]
async fn a_cleared_conversation_stays_usable() {
    let (base, _requests) = serve(&[ok_response(), ok_response()]);
    let agent = agent_for(base);
    let mut chat = agent.conversation(Recording::default());

    chat.send("first question").await.expect("run");
    assert!(
        !chat.storage().messages.is_empty(),
        "the backend should hold the first turn"
    );

    chat.clear().await.expect("clear");
    assert!(
        chat.storage().messages.is_empty(),
        "clear should empty the backend"
    );

    chat.send("second question").await.expect("run");
    assert!(
        chat.storage()
            .messages
            .iter()
            .any(|m| format!("{m:?}").contains("second question")),
        "the conversation should keep working after clear"
    );
}

#[tokio::test]
async fn a_boxed_backend_is_forwarded_to() {
    let (base, _requests) = serve(&[ok_response()]);
    let agent = agent_for(base);
    let backend = Box::new(Vec::<Message>::new()) as Box<dyn Storage>;
    let mut chat = agent.conversation(backend);

    let run = chat.send("a question").await.expect("run");

    assert_eq!(run.answer, "ok");
}

#[tokio::test]
async fn a_window_survives_a_clear() {
    let (base, requests) = serve(&[ok_response(), ok_response(), ok_response(), ok_response()]);
    let agent = agent_for(base);
    let mut chat = agent.conversation(InMemoryStorage::new().window(1));

    chat.send("first").await.expect("run");
    chat.send("second").await.expect("run");
    chat.clear().await.expect("clear");
    chat.send("third").await.expect("run");
    chat.send("fourth").await.expect("run");

    let sent_len = |raw: String| -> usize {
        let split_at = raw.rfind("\r\n").expect("a header line") + 2;
        let body = &raw[split_at..];
        let sent: serde_json::Value = serde_json::from_str(body).expect("json body");
        sent["messages"].as_array().expect("messages array").len()
    };

    let first_len = sent_len(requests.recv().expect("first request"));
    let second_len = sent_len(requests.recv().expect("second request"));
    let third_len = sent_len(requests.recv().expect("third request"));
    let fourth_len = sent_len(requests.recv().expect("fourth request"));

    assert!(fourth_len < chat.storage().messages().len());
    assert_eq!(third_len, first_len);
    assert_eq!(fourth_len, second_len);
}

#[tokio::test]
async fn a_failing_clear_surfaces_as_an_error() {
    let (base, _requests) = serve(&[ok_response()]);
    let agent = agent_for(base);
    let mut chat = agent.conversation(Broken);

    assert!(chat.clear().await.is_err());
}

#[tokio::test]
async fn clear_invokes_the_backends_clear() {
    let (base, _requests) = serve(&[ok_response()]);
    let agent = agent_for(base);
    let mut chat = agent.conversation(Recording::default());

    chat.send("a question").await.expect("run");
    assert_eq!(chat.storage().clears, 0);

    chat.clear().await.expect("clear");
    assert_eq!(chat.storage().clears, 1);
}
