//! A storage backend written outside the crate, using only the public API.
//!
//! Stands in for a third-party persistence crate: if this compiles, so does one.

mod common;
use common::serve_once;
use freyja::{Agent, Client, Dialect, EndpointConfig, Message, Storage, StorageFuture};

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
