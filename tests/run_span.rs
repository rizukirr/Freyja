//! The `run` span every agent run executes inside, under a subscriber that
//! records spans and the span each event happened in.

#![cfg(feature = "tracing")]

use freyja::testing::ScriptedModel;
use freyja::{
    Agent, GenerateResponse, InMemoryStorage, OutputContent, ResponseStatus, StopReason, Tracing,
    Usage, tool,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

#[tool(description = "adds two numbers together")]
fn add(a: i64, b: i64) -> i64 {
    a + b
}

#[derive(Debug, Clone)]
struct SpanSeen {
    id: u64,
    name: String,
    target: String,
    level: Level,
    fields: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
struct EventSeen {
    span: Option<u64>,
    message: String,
}

#[derive(Default)]
struct State {
    next: u64,
    stack: Vec<u64>,
    spans: Vec<SpanSeen>,
    events: Vec<EventSeen>,
}

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<State>>);

struct Fields<'a>(&'a mut BTreeMap<String, String>);

impl Visit for Fields<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().into(), value.into());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.0.insert(field.name().into(), format!("{value:?}"));
    }
}

impl Subscriber for Recorder {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, span: &Attributes<'_>) -> Id {
        let mut state = self.0.lock().expect("not poisoned");
        state.next += 1;
        let id = state.next;
        let mut fields = BTreeMap::new();
        span.record(&mut Fields(&mut fields));
        state.spans.push(SpanSeen {
            id,
            name: span.metadata().name().into(),
            target: span.metadata().target().into(),
            level: *span.metadata().level(),
            fields,
        });
        Id::from_u64(id)
    }

    fn record(&self, span: &Id, values: &Record<'_>) {
        let mut state = self.0.lock().expect("not poisoned");
        if let Some(seen) = state
            .spans
            .iter_mut()
            .find(|seen| seen.id == span.into_u64())
        {
            values.record(&mut Fields(&mut seen.fields));
        }
    }

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut fields = BTreeMap::new();
        event.record(&mut Fields(&mut fields));
        let mut state = self.0.lock().expect("not poisoned");
        let span = state.stack.last().copied();
        state.events.push(EventSeen {
            span,
            message: fields.remove("message").unwrap_or_default(),
        });
    }

    fn enter(&self, span: &Id) {
        self.0
            .lock()
            .expect("not poisoned")
            .stack
            .push(span.into_u64());
    }

    fn exit(&self, span: &Id) {
        let mut state = self.0.lock().expect("not poisoned");
        if let Some(at) = state.stack.iter().rposition(|id| *id == span.into_u64()) {
            state.stack.remove(at);
        }
    }
}

impl Recorder {
    fn runs(&self) -> Vec<SpanSeen> {
        self.0
            .lock()
            .expect("not poisoned")
            .spans
            .iter()
            .filter(|seen| seen.name == "run")
            .cloned()
            .collect()
    }

    /// The `Tracing` hook's events that happened inside span `id`, in order.
    fn events_in(&self, id: u64) -> Vec<String> {
        self.0
            .lock()
            .expect("not poisoned")
            .events
            .iter()
            .filter(|seen| seen.span == Some(id))
            .filter(|seen| seen.message == "model call" || seen.message == "tool call")
            .map(|seen| seen.message.clone())
            .collect()
    }
}

/// A tool call, then an answer that reports usage.
fn script() -> ScriptedModel {
    ScriptedModel::new()
        .tool_call("add", json!({"a": 1, "b": 2}))
        .respond(GenerateResponse {
            id: "r".into(),
            model: "scripted".into(),
            status: ResponseStatus::Completed,
            content: vec![OutputContent::Text("3".into())],
            usage: Some(Usage {
                input_tokens: 10,
                output_tokens: 5,
                total_tokens: 15,
            }),
            provider_metadata: None,
        })
}

#[tokio::test(flavor = "current_thread")]
async fn one_run_is_one_span_holding_its_events() {
    let recorder = Recorder::default();
    let _guard = tracing::subscriber::set_default(recorder.clone());

    let run = Agent::new(script())
        .tool(add)
        .hook(Tracing::new())
        .ask("add 1 and 2")
        .await
        .expect("run");
    assert_eq!(run.stop, StopReason::Answered);

    let runs = recorder.runs();
    assert_eq!(runs.len(), 1);
    let span = &runs[0];
    assert_eq!(span.target, "freyja");
    assert_eq!(span.level, Level::INFO);
    assert_eq!(
        recorder.events_in(span.id),
        vec!["model call", "tool call", "model call"]
    );
    assert_eq!(span.fields["stop"], "Answered");
    assert_eq!(span.fields["turns"], "2");
    assert_eq!(span.fields["input_tokens"], "10");
    assert_eq!(span.fields["output_tokens"], "5");
    assert_eq!(span.fields["total_tokens"], "15");
    assert!(!span.fields.contains_key("error"));
}

#[tokio::test(flavor = "current_thread")]
async fn a_failed_run_records_its_error() {
    let recorder = Recorder::default();
    let _guard = tracing::subscriber::set_default(recorder.clone());

    let result = Agent::new(ScriptedModel::new()).ask("hi").await;
    assert!(result.is_err());

    let runs = recorder.runs();
    assert_eq!(runs.len(), 1);
    assert!(runs[0].fields["error"].contains("ran out of responses"));
    assert!(!runs[0].fields.contains_key("stop"));
}

#[tokio::test(flavor = "current_thread")]
async fn two_runs_are_two_spans() {
    let recorder = Recorder::default();
    let _guard = tracing::subscriber::set_default(recorder.clone());

    let agent = Agent::new(ScriptedModel::new().text("one").text("two")).hook(Tracing::new());
    let mut chat = agent.conversation(InMemoryStorage::new());
    chat.send("first").await.expect("first run");
    chat.send("second").await.expect("second run");

    let runs = recorder.runs();
    assert_eq!(runs.len(), 2);
    assert_ne!(runs[0].id, runs[1].id);
    assert_eq!(recorder.events_in(runs[0].id), vec!["model call"]);
    assert_eq!(recorder.events_in(runs[1].id), vec!["model call"]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_streaming_run_is_one_span_too() {
    let recorder = Recorder::default();
    let _guard = tracing::subscriber::set_default(recorder.clone());

    let agent = Agent::new(script()).tool(add).hook(Tracing::new());
    let mut chat = agent.conversation(InMemoryStorage::new());
    chat.send_streaming("add 1 and 2", |_event| {})
        .await
        .expect("run");

    let runs = recorder.runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(
        recorder.events_in(runs[0].id),
        vec!["model call", "tool call", "model call"]
    );
}
