//! `Recorder` saves a run's responses, and `ScriptedModel::replay` plays them
//! back.

use freyja::testing::{Recorder, ScriptedModel};
use freyja::{
    Agent, GenerateRequest, GenerateResponse, InMemoryStorage, Model, OutputContent,
    ResponseStatus, Usage, tool,
};
use serde_json::json;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::Arc;

#[tool(description = "adds two numbers together")]
fn add(a: i64, b: i64) -> i64 {
    a + b
}

fn path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "freyja-recording-{name}-{}.jsonl",
        std::process::id()
    ))
}

fn response(status: ResponseStatus, content: Vec<OutputContent>) -> GenerateResponse {
    GenerateResponse {
        id: "r".into(),
        model: "scripted".into(),
        status,
        content,
        usage: Some(Usage {
            input_tokens: 10,
            output_tokens: 5,
            total_tokens: 15,
        }),
        provider_metadata: Some(json!({ "region": "test" })),
    }
}

/// A tool call, then an answer that reports usage.
fn script() -> ScriptedModel {
    ScriptedModel::new()
        .tool_call("add", json!({"a": 1, "b": 2}))
        .respond(response(
            ResponseStatus::Completed,
            vec![OutputContent::Text("3".into())],
        ))
}

#[tokio::test]
async fn a_recorder_answers_as_its_model_does() {
    let recorder = Recorder::new(script());
    let run = Agent::new(recorder)
        .tool(add)
        .ask("add 1 and 2")
        .await
        .unwrap();
    assert_eq!(run.answer, "3");
}

#[tokio::test]
async fn a_replayed_run_matches_the_recorded_one() {
    let file = path("roundtrip");
    let recorder = Arc::new(Recorder::new(script()));
    let recorded = Agent::new(recorder.clone())
        .tool(add)
        .ask("add 1 and 2")
        .await
        .unwrap();
    recorder.save(&file).unwrap();

    let replayed = Agent::new(ScriptedModel::replay(&file).unwrap())
        .tool(add)
        .ask("add 1 and 2")
        .await
        .unwrap();
    std::fs::remove_file(&file).ok();

    assert_eq!(recorded.answer, "3");
    assert_eq!(replayed.answer, recorded.answer);
    assert_eq!(replayed.stop, recorded.stop);
    assert_eq!(replayed.usage, recorded.usage);
    assert_eq!(replayed.turns, recorded.turns);
}

#[tokio::test]
async fn every_response_survives_the_file_unchanged() {
    let file = path("shapes");
    let responses = vec![
        response(
            ResponseStatus::Completed,
            vec![
                OutputContent::Text("hi".into()),
                OutputContent::Reasoning {
                    data: json!({ "opaque": [1, 2] }),
                },
            ],
        ),
        response(
            ResponseStatus::RequiresAction,
            vec![OutputContent::ToolCall {
                id: "call_1".into(),
                name: "add".into(),
                arguments: "{\"a\":1}".into(),
            }],
        ),
        response(
            ResponseStatus::Incomplete,
            vec![OutputContent::Refusal("no".into())],
        ),
        response(ResponseStatus::Failed, vec![]),
        response(ResponseStatus::Other("paused".into()), vec![]),
    ];
    let mut model = ScriptedModel::new();
    for each in &responses {
        model = model.respond(each.clone());
    }
    let recorder = Recorder::new(model);
    let request = GenerateRequest::new();
    for _ in &responses {
        recorder.generate(&request).await.unwrap();
    }
    recorder.save(&file).unwrap();

    let replay = ScriptedModel::replay(&file).unwrap();
    std::fs::remove_file(&file).ok();
    for expected in &responses {
        assert_eq!(&replay.generate(&request).await.unwrap(), expected);
    }
    assert!(replay.generate(&request).await.is_err());
}

#[tokio::test]
async fn a_failed_call_is_not_recorded() {
    let file = path("failed");
    let recorder = Arc::new(Recorder::new(ScriptedModel::new()));
    assert!(Agent::new(recorder.clone()).ask("hi").await.is_err());
    recorder.save(&file).unwrap();

    let replay = ScriptedModel::replay(&file).unwrap();
    std::fs::remove_file(&file).ok();
    assert!(replay.generate(&GenerateRequest::new()).await.is_err());
}

#[tokio::test]
async fn a_streaming_run_is_recorded_too() {
    let file = path("streaming");
    let recorder = Arc::new(Recorder::new(script()));
    let agent = Agent::new(recorder.clone()).tool(add);
    let mut chat = agent.conversation(InMemoryStorage::new());
    let run = chat
        .send_streaming("add 1 and 2", |_event| {})
        .await
        .unwrap();
    recorder.save(&file).unwrap();

    let replayed = Agent::new(ScriptedModel::replay(&file).unwrap())
        .tool(add)
        .ask("add 1 and 2")
        .await
        .unwrap();
    std::fs::remove_file(&file).ok();
    assert_eq!(run.answer, "3");
    assert_eq!(replayed.answer, run.answer);
}

fn rejected(name: &str, contents: &str) -> std::io::Error {
    let file = path(name);
    std::fs::write(&file, contents).unwrap();
    let error = ScriptedModel::replay(&file).unwrap_err();
    std::fs::remove_file(&file).ok();
    error
}

#[test]
fn a_file_without_the_header_is_rejected() {
    let error = rejected("noheader", "{\"id\":\"r\"}\n");
    assert_eq!(error.kind(), ErrorKind::InvalidData);
    assert!(error.to_string().contains("line 1"));
}

#[test]
fn a_newer_version_is_rejected() {
    let error = rejected("newer", "{\"freyja_recording\":2}\n");
    assert_eq!(error.kind(), ErrorKind::InvalidData);
    assert!(error.to_string().contains("line 1"));
}

#[test]
fn a_bad_response_line_is_rejected_by_number() {
    let error = rejected("badline", "{\"freyja_recording\":1}\nnot json\n");
    assert_eq!(error.kind(), ErrorKind::InvalidData);
    assert!(error.to_string().contains("line 2"));
}
