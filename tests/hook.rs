//! Hooks seen from outside the crate, over a scripted model and no network.

use freyja::{
    Agent, Context, Decision, Error, Flow, GenerateRequest, GenerateResponse, Hook,
    InMemoryStorage, Model, ModelFuture, OutputContent, ResponseStatus, Role, StopReason, Tool,
    ToolDefinition, ToolError, ToolFuture, tool,
};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[tool(description = "adds two numbers together")]
fn add(a: i64, b: i64) -> i64 {
    a + b
}

/// Hands out its responses in order and never touches a network.
struct Scripted {
    responses: Mutex<VecDeque<GenerateResponse>>,
}

impl Scripted {
    fn new(responses: Vec<GenerateResponse>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into()),
        })
    }

    fn remaining(&self) -> usize {
        self.responses.lock().expect("not poisoned").len()
    }
}

impl Model for Scripted {
    fn name(&self) -> &str {
        "scripted"
    }

    fn generate<'a>(&'a self, _request: &'a GenerateRequest) -> ModelFuture<'a, GenerateResponse> {
        Box::pin(async move {
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

type Log = Arc<Mutex<Vec<String>>>;

/// Records every call and can stop at one named point.
struct Recorder {
    log: Log,
    stop_at: Option<(&'static str, &'static str)>,
}

impl Recorder {
    fn new(log: &Log) -> Self {
        Self {
            log: log.clone(),
            stop_at: None,
        }
    }

    fn stopping(log: &Log, point: &'static str, reason: &'static str) -> Self {
        Self {
            log: log.clone(),
            stop_at: Some((point, reason)),
        }
    }

    fn record(&self, point: &'static str, entry: String) -> Flow {
        self.log.lock().expect("not poisoned").push(entry);
        match self.stop_at {
            Some((at, reason)) if at == point => Flow::Stop(reason.into()),
            _ => Flow::Continue,
        }
    }
}

impl Hook for Recorder {
    fn before_model(&self, _request: &GenerateRequest, _cx: &Context) -> Flow {
        self.record("before_model", "before_model".into())
    }

    fn after_model(&self, _response: &GenerateResponse, _elapsed: Duration, _cx: &Context) -> Flow {
        self.record("after_model", "after_model".into())
    }

    fn before_tool(&self, name: &str, arguments: &str, _cx: &Context) -> Flow {
        self.record("before_tool", format!("before_tool {name} {arguments}"))
    }

    fn after_tool(
        &self,
        name: &str,
        _arguments: &str,
        output: &str,
        _elapsed: Duration,
        _cx: &Context,
    ) -> Flow {
        self.record("after_tool", format!("after_tool {name} {output}"))
    }
}

fn entries(log: &Log) -> Vec<String> {
    log.lock().expect("not poisoned").clone()
}

fn full_round_trip() -> Vec<String> {
    vec![
        "before_model".into(),
        "after_model".into(),
        r#"before_tool add {"a":1,"b":2}"#.into(),
        "after_tool add 3".into(),
        "before_model".into(),
        "after_model".into(),
    ]
}

#[tokio::test]
async fn hooks_see_every_call_in_order() {
    let log = Log::default();
    let agent = Agent::new(Scripted::new(tool_call_then_answer()))
        .tool(add)
        .hook(Recorder::new(&log));
    let mut chat = agent.conversation(InMemoryStorage::new());

    let run = chat.send("add 1 and 2").await.expect("run");

    assert_eq!(run.stop, StopReason::Answered);
    assert_eq!(entries(&log), full_round_trip());
}

#[tokio::test]
async fn hooks_see_every_call_when_streaming() {
    let log = Log::default();
    let agent = Agent::new(Scripted::new(tool_call_then_answer()))
        .tool(add)
        .hook(Recorder::new(&log));
    let mut chat = agent.conversation(InMemoryStorage::new());

    let run = chat
        .send_streaming("add 1 and 2", |_event| {})
        .await
        .expect("run");

    assert_eq!(run.stop, StopReason::Answered);
    assert_eq!(entries(&log), full_round_trip());
}

#[tokio::test]
async fn a_stop_after_the_model_ends_the_run() {
    let log = Log::default();
    let model = Scripted::new(tool_call_then_answer());
    let agent =
        Agent::new(model.clone())
            .tool(add)
            .hook(Recorder::stopping(&log, "after_model", "enough"));
    let mut chat = agent.conversation(InMemoryStorage::new());

    let run = chat.send("add 1 and 2").await.expect("run");

    assert_eq!(run.stop, StopReason::Stopped("enough".into()));
    assert_eq!(run.turns, 1);
    assert_eq!(model.remaining(), 1);
}

#[tokio::test]
async fn a_stop_before_the_model_makes_no_call() {
    let log = Log::default();
    let model = Scripted::new(tool_call_then_answer());
    let agent =
        Agent::new(model.clone())
            .tool(add)
            .hook(Recorder::stopping(&log, "before_model", "no"));
    let mut chat = agent.conversation(InMemoryStorage::new());

    let run = chat.send("add 1 and 2").await.expect("run");

    assert_eq!(run.stop, StopReason::Stopped("no".into()));
    assert_eq!(run.turns, 0);
    assert_eq!(model.remaining(), 2);
}

/// Counts how often it runs.
struct Counted(Arc<AtomicUsize>);

impl Tool for Counted {
    fn name(&self) -> &str {
        "add"
    }

    fn definition(&self) -> ToolDefinition {
        add.definition()
    }

    fn call<'a>(&'a self, _arguments: &'a str, _cx: &'a Context) -> ToolFuture<'a> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok::<String, ToolError>("3".into()) })
    }
}

#[tokio::test]
async fn a_stop_before_a_tool_skips_it() {
    let log = Log::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let model = Scripted::new(tool_call_then_answer());
    let agent = Agent::new(model.clone())
        .tool(Counted(calls.clone()))
        .hook(Recorder::stopping(&log, "before_tool", "halt"));
    let mut chat = agent.conversation(InMemoryStorage::new());

    let run = chat.send("add 1 and 2").await.expect("run");

    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(run.stop, StopReason::Stopped("halt".into()));
    assert_eq!(model.remaining(), 1);
    assert!(chat.storage().messages().iter().any(|message| {
        message.role == Role::Tool && format!("{:?}", message.content).contains("stopped:")
    }));
}

#[tokio::test]
async fn every_hook_runs_and_the_first_stop_wins() {
    let log = Log::default();
    let other = Log::default();
    let agent = Agent::new(Scripted::new(tool_call_then_answer()))
        .tool(add)
        .hook(Recorder::stopping(&log, "after_model", "first"))
        .hook(Recorder::stopping(&other, "after_model", "second"));
    let mut chat = agent.conversation(InMemoryStorage::new());

    let run = chat.send("add 1 and 2").await.expect("run");

    assert_eq!(run.stop, StopReason::Stopped("first".into()));
    assert!(entries(&log).contains(&"after_model".to_string()));
    assert!(entries(&other).contains(&"after_model".to_string()));
}

#[tokio::test]
async fn after_tool_sees_a_refused_call() {
    let log = Log::default();
    let agent = Agent::new(Scripted::new(tool_call_then_answer()))
        .tool(add)
        .guard(|_name, _arguments, _cx| Decision::Deny("no".into()))
        .hook(Recorder::new(&log));
    let mut chat = agent.conversation(InMemoryStorage::new());

    chat.send("add 1 and 2").await.expect("run");

    let seen = entries(&log);
    assert!(!seen.iter().any(|entry| entry.starts_with("before_tool")));
    let after: Vec<_> = seen
        .iter()
        .filter(|entry| entry.starts_with("after_tool"))
        .collect();
    assert_eq!(after.len(), 1);
    assert!(after[0].starts_with("after_tool add denied:"));
}
