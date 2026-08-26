//! A storage backend written outside the crate, using only the public API.
//!
//! Stands in for a third-party persistence crate: if this compiles, so does one.

mod common;
use common::serve_once;
use freyja::{Agent, Client, Dialect, EndpointConfig, Message, Storage, StorageFuture};
use std::sync::{Arc, Mutex};

/// A backend that records every append, so a test can see what was stored.
#[derive(Default)]
struct Recording {
    messages: Mutex<Vec<Message>>,
    appends: Mutex<usize>,
}

impl Storage for Recording {
    fn load(&self) -> StorageFuture<'_, Vec<Message>> {
        Box::pin(async move { Ok(self.messages.lock().unwrap().clone()) })
    }

    fn append(&self, messages: Vec<Message>) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            *self.appends.lock().unwrap() += 1;
            self.messages.lock().unwrap().extend(messages);
            Ok(())
        })
    }

    fn clear(&self) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            self.messages.lock().unwrap().clear();
            Ok(())
        })
    }
}

/// A backend whose load fails, which must abort the run rather than continue
/// with no history.
struct Broken;

impl Storage for Broken {
    fn load(&self) -> StorageFuture<'_, Vec<Message>> {
        Box::pin(async { Err(std::io::Error::other("backend unreachable").into()) })
    }
    fn append(&self, _messages: Vec<Message>) -> StorageFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn clear(&self) -> StorageFuture<'_, ()> {
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

fn agent_for(base: String, storage: Arc<Recording>) -> Agent {
    let config =
        EndpointConfig::new(Dialect::OpenAiChat, "local", base).default_model("test-model");
    Agent::new(Client::new(config, "sk-test")).memory(storage)
}

#[tokio::test]
async fn a_third_party_backend_holds_the_conversation() {
    let (base, _requests) = serve_once(ok_response());
    let storage = Arc::new(Recording::default());
    let agent = agent_for(base, Arc::clone(&storage));

    agent.message("a question").await.expect("run");

    let held = storage.load().await.unwrap();
    assert!(held.iter().any(|m| format!("{m:?}").contains("a question")));
    assert_eq!(*storage.appends.lock().unwrap(), 1);
}

#[tokio::test]
async fn a_failing_load_aborts_the_run() {
    let (base, requests) = serve_once(ok_response());
    let config =
        EndpointConfig::new(Dialect::OpenAiChat, "local", base).default_model("test-model");
    let agent = Agent::new(Client::new(config, "sk-test")).memory(Broken);

    assert!(agent.message("a question").await.is_err());
    assert!(requests.try_recv().is_err(), "no request may be sent");
}
