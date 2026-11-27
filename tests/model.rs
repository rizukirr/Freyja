//! A model written outside the crate, using only the public API.
//!
//! Stands in for a third-party provider crate: if this compiles, so does one.

use freyja::{
    Agent, AgentEvent, Error, GenerateRequest, GenerateResponse, InMemoryStorage, Message, Model,
    ModelFuture, OutputContent, ResponseStatus, Role, StopReason, Storage, StorageFuture,
    StreamEvent, Summarizer, tool,
};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

#[tool(description = "adds two numbers together")]
fn add(a: i64, b: i64) -> i64 {
    a + b
}

/// Hands out its responses in order and never touches a network.
struct Scripted {
    responses: Mutex<VecDeque<GenerateResponse>>,
    /// The roles of the messages in each request received, in order.
    seen: Mutex<Vec<Vec<Role>>>,
}

impl Scripted {
    fn new(responses: Vec<GenerateResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            seen: Mutex::new(Vec::new()),
        }
    }
}

impl Model for Scripted {
    fn name(&self) -> &str {
        "scripted"
    }

    fn generate<'a>(&'a self, request: &'a GenerateRequest) -> ModelFuture<'a, GenerateResponse> {
        Box::pin(async move {
            self.seen
                .lock()
                .expect("not poisoned")
                .push(request.messages.iter().map(|m| m.role).collect());
            self.responses
                .lock()
                .expect("not poisoned")
                .pop_front()
                .ok_or_else(|| Error::InvalidResponse {
                    endpoint: "scripted".into(),
                    message: "script ran out".into(),
                })
        })
    }
}

/// Counts every `generate` that reaches the model it wraps.
struct Counting<M> {
    calls: AtomicUsize,
    inner: M,
}

impl<M: Model> Model for Counting<M> {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn generate<'a>(&'a self, request: &'a GenerateRequest) -> ModelFuture<'a, GenerateResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.generate(request)
    }
}

fn response(status: ResponseStatus, content: Vec<OutputContent>) -> GenerateResponse {
    GenerateResponse {
        id: "r".into(),
        model: "scripted".into(),
        status,
        content,
        usage: None,
        provider_metadata: None,
    }
}

fn tool_call_then_answer() -> Vec<GenerateResponse> {
    vec![
        response(
            ResponseStatus::RequiresAction,
            vec![OutputContent::ToolCall {
                id: "c1".into(),
                name: "add".into(),
                arguments: r#"{"a":1,"b":2}"#.into(),
            }],
        ),
        response(
            ResponseStatus::Completed,
            vec![OutputContent::Text("3".into())],
        ),
    ]
}

/// A backend whose load fails.
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

#[tokio::test]
async fn a_model_written_outside_freyja_runs_a_tool_round_trip() {
    let agent = Agent::new(Scripted::new(tool_call_then_answer())).tool(add);
    let mut chat = agent.conversation(InMemoryStorage::new());

    let run = chat.send("add 1 and 2").await.expect("run");

    assert_eq!(run.answer, "3");
    assert_eq!(run.stop, StopReason::Answered);
    let history = chat.storage().messages();
    assert!(history.iter().any(|message| {
        message.role == Role::Tool && format!("{:?}", message.content).contains('3')
    }));
}

#[tokio::test]
async fn streaming_replays_a_model_that_cannot_stream() {
    let agent = Agent::new(Scripted::new(tool_call_then_answer())).tool(add);
    let mut chat = agent.conversation(InMemoryStorage::new());
    let mut events = Vec::new();

    let run = chat
        .send_streaming("add 1 and 2", |event| events.push(event))
        .await
        .expect("run");

    let done = events
        .iter()
        .filter(|event| matches!(event, AgentEvent::Model(StreamEvent::Done { .. })))
        .count();
    assert_eq!(done, 2);
    let results = events
        .iter()
        .filter(|event| matches!(event, AgentEvent::ToolResult { .. }))
        .count();
    assert_eq!(results, 1);
    assert_eq!(run.answer, "3");
}

#[tokio::test]
async fn a_shared_model_counts_every_caller() {
    let mut script = tool_call_then_answer();
    script.push(response(
        ResponseStatus::Completed,
        vec![OutputContent::Text("- summary".into())],
    ));
    let shared = Arc::new(Counting {
        calls: AtomicUsize::new(0),
        inner: Scripted::new(script),
    });
    let agent = Agent::new(shared.clone()).tool(add);
    let summarizer = Summarizer::new(shared.clone());

    let mut chat = agent.conversation(InMemoryStorage::new());
    chat.send("add 1 and 2").await.expect("run");
    summarizer
        .summarize(&[Message::text(Role::User, "hi")])
        .await
        .expect("summary");

    assert!(shared.calls.load(Ordering::SeqCst) >= 3);
}

#[tokio::test]
async fn a_storage_failure_names_the_model() {
    let agent = Agent::new(Scripted::new(Vec::new()));
    let mut chat = agent.conversation(Broken);

    let error = chat.send("hi").await.expect_err("load fails");

    assert!(
        matches!(&error, Error::InvalidRequest { endpoint, .. } if &**endpoint == "scripted"),
        "{error:?}"
    );
}

#[tokio::test]
async fn ask_keeps_no_history() {
    let text = |t: &str| {
        response(
            ResponseStatus::Completed,
            vec![OutputContent::Text(t.into())],
        )
    };
    let model = Arc::new(Scripted::new(vec![text("one"), text("two")]));
    let agent = Agent::new(model.clone());

    let first = agent.ask("a").await.expect("first");
    let second = agent.ask("b").await.expect("second");

    assert_eq!(first.answer, "one");
    assert_eq!(second.answer, "two");
    let seen = model.seen.lock().expect("not poisoned");
    let user_turns = seen[1].iter().filter(|role| **role != Role::System).count();
    assert_eq!(user_turns, 1);
}
