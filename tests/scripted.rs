//! `ScriptedModel` from outside the crate, with no network.

use freyja::testing::ScriptedModel;
use freyja::{
    Agent, Error, GenerateRequest, GenerateResponse, InMemoryStorage, InputContent, Model,
    OutputContent, ResponseStatus, Role, tool,
};
use serde_json::json;
use std::sync::Arc;

#[tool(description = "adds two numbers together")]
fn add(a: i64, b: i64) -> i64 {
    a + b
}

fn add_then_answer() -> ScriptedModel {
    ScriptedModel::new()
        .tool_call("add", json!({"a": 1, "b": 2}))
        .text("3")
}

#[tokio::test]
async fn an_agent_runs_a_tool_call_and_answers_from_the_script() {
    let run = Agent::new(add_then_answer())
        .tool(add)
        .ask("add 1 and 2")
        .await
        .expect("run");

    assert_eq!(run.answer, "3");
    assert_eq!(run.turns, 2);
}

#[tokio::test]
async fn every_request_is_recorded_and_the_second_carries_the_tool_result() {
    let model = Arc::new(add_then_answer());
    Agent::new(model.clone())
        .tool(add)
        .ask("add 1 and 2")
        .await
        .expect("run");

    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    let last = requests[1].messages.last().expect("a message");
    assert_eq!(last.role, Role::Tool);
    assert!(matches!(
        &last.content[0],
        InputContent::ToolResult { output, .. } if output == "3"
    ));
}

#[tokio::test]
async fn a_streaming_run_answers_the_same() {
    let agent = Agent::new(add_then_answer()).tool(add);
    let mut chat = agent.conversation(InMemoryStorage::new());

    let run = chat
        .send_streaming("add 1 and 2", |_event| {})
        .await
        .expect("run");

    assert_eq!(run.answer, "3");
}

#[tokio::test]
async fn text_returns_a_completed_answer() {
    let model = ScriptedModel::new().text("hi");
    let response = model
        .generate(&GenerateRequest::default())
        .await
        .expect("a response");

    assert_eq!(response.status, ResponseStatus::Completed);
    assert_eq!(response.output_text(), "hi");
}

#[tokio::test]
async fn tool_call_returns_one_call_with_a_unique_id() {
    let model = ScriptedModel::new()
        .tool_call("add", json!({"a": 1}))
        .tool_call("add", json!({"a": 2}));
    let request = GenerateRequest::default();
    let first = model.generate(&request).await.expect("a response");
    let second = model.generate(&request).await.expect("a response");

    assert_eq!(first.status, ResponseStatus::RequiresAction);
    let calls: Vec<_> = first.tool_calls().collect();
    assert_eq!(calls.len(), 1);
    let (id, name, arguments) = calls[0];
    assert_eq!(name, "add");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(arguments).expect("json"),
        json!({"a": 1})
    );
    let (second_id, _, _) = second.tool_calls().next().expect("a call");
    assert_ne!(id, second_id);
}

#[tokio::test]
async fn respond_returns_the_response_as_given() {
    let given = GenerateResponse {
        id: "mine".into(),
        model: "m".into(),
        status: ResponseStatus::Incomplete,
        content: vec![OutputContent::Text("cut".into())],
        usage: None,
        provider_metadata: None,
    };
    let model = ScriptedModel::new().respond(given.clone());

    let response = model
        .generate(&GenerateRequest::default())
        .await
        .expect("a response");

    assert_eq!(response, given);
}

#[tokio::test]
async fn an_empty_script_returns_invalid_response() {
    let error = ScriptedModel::new()
        .generate(&GenerateRequest::default())
        .await
        .expect_err("the script is empty");

    assert!(matches!(
        error,
        Error::InvalidResponse { ref message, .. } if message.contains("ran out")
    ));
}
