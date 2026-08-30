//! A storage backend written outside the crate, trimming with no group
//! awareness at all.
//!
//! This is the test that the repair pass inside `Conversation::send` earns
//! its keep: a naive backend has no way to know that a tool call and the
//! result answering it must survive together, so trimming to the last few
//! messages routinely cuts between them. What reaches the transport must
//! still be a request the provider accepts.

mod common;
use common::{ok_response, serve};
use freyja::{
    Agent, Client, Dialect, EndpointConfig, InputContent, Message, Role, Storage, StorageFuture,
};

/// Keeps only the last `keep` messages, oldest first, with no idea that a
/// tool call and its result belong together. This is what a backend outside
/// the crate looks like if it does the simplest thing that could work: no
/// import of `split` or `window_by_groups`, neither of which is reachable
/// from here, just a suffix of the stored vector.
struct Naive {
    messages: Vec<Message>,
    keep: usize,
}

impl Storage for Naive {
    fn load(&mut self) -> StorageFuture<'_, Vec<Message>> {
        Box::pin(async move {
            let start = self.messages.len().saturating_sub(self.keep);
            Ok(self.messages[start..].to_vec())
        })
    }
    fn append(&mut self, messages: Vec<Message>) -> StorageFuture<'_, ()> {
        Box::pin(async move {
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

/// A stored transcript where the last two messages are a tool result and the
/// plain text that followed it, and the call the result answers is one
/// message further back. A backend that keeps only the last two messages
/// cuts exactly between the call and its result.
fn transcript_with_a_call_the_window_will_split_from_its_result() -> Vec<Message> {
    vec![
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
        Message::tool_result("call_1", "SENTINEL-RAINING"),
        Message::text(Role::Assistant, "it is raining"),
    ]
}

#[tokio::test]
async fn a_naive_backend_cutting_between_a_call_and_its_result_still_sends_a_valid_request() {
    let (base, requests) = serve(&[ok_response()]);
    let config =
        EndpointConfig::new(Dialect::OpenAiChat, "local", base).default_model("test-model");
    let storage = Naive {
        messages: transcript_with_a_call_the_window_will_split_from_its_result(),
        keep: 2,
    };
    let agent = Agent::new(Client::new(config, "sk-test"));

    agent
        .conversation(storage)
        .send("SENTINEL-NEW")
        .await
        .expect("run");

    let sent = requests.recv().expect("request");
    let split_at = sent.rfind("\r\n").expect("a header line") + 2;
    let body: serde_json::Value = serde_json::from_str(&sent[split_at..]).expect("json body");
    let messages = body["messages"].as_array().expect("messages array");

    // The naive load handed back a tool result with no call in front of it,
    // since `keep: 2` dropped the call and kept the result. Without the
    // repair pass this reaches the transport as a "tool" message answering
    // nothing, which every provider rejects. With it, the orphaned result is
    // gone and its content never reaches the wire.
    assert!(!sent.contains("SENTINEL-RAINING"), "{sent}");
    assert!(sent.contains("SENTINEL-NEW"), "{sent}");

    // What did reach the transport must be internally consistent: no tool
    // message answers a call id that no assistant message called.
    let called: std::collections::HashSet<&str> = messages
        .iter()
        .filter(|m| m["role"] == "assistant")
        .filter_map(|m| m["tool_calls"].as_array())
        .flatten()
        .filter_map(|call| call["id"].as_str())
        .collect();
    for message in messages {
        if message["role"] == "tool" {
            let answers = message["tool_call_id"].as_str().expect("tool_call_id");
            assert!(
                called.contains(answers),
                "a tool message answered {answers}, which no call requested: {sent}"
            );
        }
    }
}
