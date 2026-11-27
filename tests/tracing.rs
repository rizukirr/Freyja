//! The `Tracing` hook, under a subscriber that records every event.

#![cfg(feature = "tracing")]

use freyja::testing::ScriptedModel;
use freyja::{
    Agent, Decision, GenerateResponse, OutputContent, ResponseStatus, StopReason, Tracing, Usage,
    tool,
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

/// One recorded event: its target, level and every field as text.
#[derive(Debug, Clone)]
struct Seen {
    target: String,
    level: Level,
    fields: BTreeMap<String, String>,
}

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<Seen>>>);

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
    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _span: &Id, _values: &Record<'_>) {}
    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}
    fn event(&self, event: &Event<'_>) {
        let mut fields = BTreeMap::new();
        event.record(&mut Fields(&mut fields));
        self.0.lock().expect("not poisoned").push(Seen {
            target: event.metadata().target().into(),
            level: *event.metadata().level(),
            fields,
        });
    }
    fn enter(&self, _span: &Id) {}
    fn exit(&self, _span: &Id) {}
}

impl Recorder {
    fn named(&self, message: &str) -> Vec<Seen> {
        self.0
            .lock()
            .expect("not poisoned")
            .iter()
            .filter(|seen| seen.fields.get("message").map(String::as_str) == Some(message))
            .cloned()
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
                output_tokens: 2,
                total_tokens: 12,
            }),
            provider_metadata: None,
        })
}

#[tokio::test(flavor = "current_thread")]
async fn every_model_call_and_tool_call_is_one_event() {
    let recorder = Recorder::default();
    let _guard = tracing::subscriber::set_default(recorder.clone());

    let run = Agent::new(script())
        .tool(add)
        .hook(Tracing::new())
        .ask("add 1 and 2")
        .await
        .expect("run");

    assert_eq!(run.stop, StopReason::Answered);
    assert_eq!(run.answer, "3");

    let models = recorder.named("model call");
    assert_eq!(models.len(), 2);
    for seen in &models {
        assert_eq!(seen.target, "freyja");
        assert_eq!(seen.level, Level::INFO);
        for field in ["model", "status", "elapsed_ms"] {
            assert!(seen.fields.contains_key(field), "missing {field}");
        }
    }
    assert!(!models[0].fields.contains_key("input_tokens"));
    assert_eq!(models[1].fields["model"], "scripted");
    assert_eq!(models[1].fields["input_tokens"], "10");
    assert_eq!(models[1].fields["output_tokens"], "2");
    assert_eq!(models[1].fields["total_tokens"], "12");

    let tools = recorder.named("tool call");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].target, "freyja");
    assert_eq!(tools[0].level, Level::INFO);
    assert_eq!(tools[0].fields["tool"], "add");
    assert_eq!(tools[0].fields["output_bytes"], "1");
    assert!(tools[0].fields.contains_key("elapsed_ms"));

    for seen in models.iter().chain(&tools) {
        for field in ["text", "arguments", "output"] {
            assert!(
                !seen.fields.contains_key(field),
                "{field} logged by default"
            );
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn with_content_logs_text_arguments_and_output() {
    let recorder = Recorder::default();
    let _guard = tracing::subscriber::set_default(recorder.clone());

    Agent::new(script())
        .tool(add)
        .hook(Tracing::new().with_content())
        .ask("add 1 and 2")
        .await
        .expect("run");

    let models = recorder.named("model call");
    assert_eq!(models[1].fields["text"], "3");
    let tools = recorder.named("tool call");
    assert_eq!(tools[0].fields["output"], "3");
    let arguments: serde_json::Value =
        serde_json::from_str(&tools[0].fields["arguments"]).expect("json");
    assert_eq!(arguments, json!({"a": 1, "b": 2}));
}

#[tokio::test(flavor = "current_thread")]
async fn a_refused_tool_call_is_still_an_event() {
    let recorder = Recorder::default();
    let _guard = tracing::subscriber::set_default(recorder.clone());

    let run = Agent::new(script())
        .tool(add)
        .guard(|_name, _arguments, _cx| Decision::Deny("not today".into()))
        .hook(Tracing::new())
        .ask("add 1 and 2")
        .await
        .expect("run");

    assert_eq!(run.answer, "3");
    let tools = recorder.named("tool call");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].fields["tool"], "add");
}
