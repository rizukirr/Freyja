//! A storage backend written outside the crate, using only the public API.
//!
//! Stands in for a third-party persistence crate: if this compiles, so does one.

mod common;
use common::serve_once;
use freyja::{
    Agent, Client, Dialect, EndpointConfig, InputContent, Message, Role, Storage, StorageFuture,
};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::mpsc;

/// A backend that records every append, so a test can see what was stored.
///
/// `Storage` takes `&mut self`, so the fields need no lock: a `Conversation`
/// owns its backend outright, and nothing else can reach it while it does.
#[derive(Default)]
struct Recording {
    messages: Vec<Message>,
    appends: usize,
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
        Box::pin(async { Ok(()) })
    }
}

/// Built with a derived `Content-Length` and leaked to satisfy `serve_once`'s
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

/// Serves `responses` in order over separate connections, the same way
/// `tests/agent.rs` does it, since `window` needs several turns to show a
/// difference and `common::serve_once` only ever accepts one.
fn serve_many(responses: Vec<&'static str>) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    let (tx, rx) = mpsc::channel();

    std::thread::spawn(move || {
        for response in responses {
            let (mut socket, _) = listener.accept().expect("accept");
            let mut reader = BufReader::new(socket.try_clone().expect("clone"));

            let mut head = String::new();
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).expect("read") == 0 || line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
                head.push_str(&line);
            }
            let mut body = vec![0u8; length];
            if length > 0 {
                std::io::Read::read_exact(&mut reader, &mut body).expect("body");
            }
            head.push_str(&String::from_utf8_lossy(&body));

            socket.write_all(response.as_bytes()).expect("write");
            socket.flush().expect("flush");
            let _ = tx.send(head);
        }
    });

    (base, rx)
}

#[tokio::test]
async fn a_third_party_backend_holds_the_conversation() {
    let (base, _requests) = serve_once(ok_response());
    let agent = agent_for(base);
    let mut chat = agent.conversation_in(Recording::default());

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
    let (base, requests) = serve_once(ok_response());
    let agent = agent_for(base);
    let mut chat = agent.conversation_in(Broken);

    assert!(chat.send("a question").await.is_err());
    assert!(requests.try_recv().is_err(), "no request may be sent");
}

#[tokio::test]
async fn a_borrowed_vector_is_extended_in_place() {
    let (base, _requests) = serve_once(ok_response());
    let agent = agent_for(base);
    let mut history: Vec<Message> = Vec::new();
    let starting_len = history.len();

    let mut chat = agent.conversation_in(&mut history);
    chat.send("a question").await.expect("run");
    drop(chat);

    assert!(history.len() > starting_len);
}

#[tokio::test]
async fn window_shapes_what_is_sent_while_the_backend_keeps_everything() {
    let (base, requests) = serve_many(vec![ok_response(), ok_response(), ok_response()]);
    let agent = agent_for(base);
    let mut chat = agent.conversation().window(1);

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

    assert!(sent_len < chat.storage().len());
}

#[tokio::test]
async fn send_carries_every_content_block() {
    let (base, requests) = serve_once(ok_response());
    let agent = agent_for(base);
    let mut chat = agent.conversation();

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
