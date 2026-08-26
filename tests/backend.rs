//! A storage backend written outside the crate, trimming with the public
//! grouping function rather than reimplementing turn grouping.

mod common;
use common::serve_once;
use freyja::{
    Agent, Client, Dialect, EndpointConfig, InputContent, Message, Role, Storage, StorageFuture,
    split, window_by_groups,
};
use std::sync::Mutex;

struct Windowed {
    messages: Mutex<Vec<Message>>,
    keep: usize,
}

impl Storage for Windowed {
    fn load(&self) -> StorageFuture<'_, Vec<Message>> {
        Box::pin(async move {
            let all = self.messages.lock().unwrap().clone();
            Ok(window_by_groups(&all, self.keep))
        })
    }
    fn append(&self, messages: Vec<Message>) -> StorageFuture<'_, ()> {
        Box::pin(async move {
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

/// Built with a derived `Content-Length` and leaked to satisfy `serve_once`'s
/// `&'static str`, the same way the other integration tests do it.
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
async fn a_backend_can_trim_with_the_public_grouping_function() {
    let (base, requests) = serve_once(ok_response());
    let config =
        EndpointConfig::new(Dialect::OpenAiChat, "local", base).default_model("test-model");
    let storage = Windowed {
        messages: Mutex::new(vec![Message::text(freyja::Role::User, "SENTINEL-OLD")]),
        keep: 1,
    };
    let agent = Agent::new(Client::new(config, "sk-test")).memory(storage);

    agent.message("SENTINEL-NEW").await.expect("run");

    let sent = requests.recv().expect("request");
    assert!(sent.contains("SENTINEL-NEW"), "{sent}");
}

/// A backend author reaches for `split` when they want to group a transcript
/// themselves instead of calling `window_by_groups`. This checks the shape
/// they get back is usable: every message is accounted for exactly once,
/// no group is empty, and a call stays fused with the result answering it.
#[test]
fn split_groups_a_transcript_for_a_caller_outside_the_crate() {
    let history = vec![
        Message::text(Role::System, "pinned"),
        Message::text(Role::User, "weather?"),
        Message::new(
            Role::Assistant,
            vec![InputContent::ToolCall {
                id: "call_1".into(),
                name: "get_weather".into(),
                arguments: "{}".into(),
            }],
        ),
        Message::tool_result("call_1", "raining"),
        Message::text(Role::Assistant, "it is raining"),
    ];

    let (pinned, groups) = split(&history);

    assert_eq!(pinned.len(), 1);
    assert_eq!(pinned[0].role, Role::System);

    let grouped: usize = groups.iter().map(|group| group.len()).sum();
    assert_eq!(pinned.len() + grouped, history.len());
    assert!(groups.iter().all(|group| !group.is_empty()));

    let call_and_result = groups
        .iter()
        .find(|group| group.len() > 1)
        .expect("the call and its result should share a group");
    assert!(call_and_result.iter().any(|message| {
        message
            .content
            .iter()
            .any(|content| matches!(content, InputContent::ToolCall { id, .. } if id == "call_1"))
    }));
    assert!(call_and_result.iter().any(|message| message
        .content
        .iter()
        .any(|content| matches!(content, InputContent::ToolResult { call_id, .. } if call_id == "call_1"))));
}
