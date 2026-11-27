# fake-model: Implementation Plan

**Spec:** docs/specs/2026-11-27-fake-model-design.md
**Goal:** Ship `freyja::testing::ScriptedModel`, a `Model` that answers from a script and records every request.
**Architecture:** One struct in `src/testing.rs` beside `storage_conformance`, holding a queue of responses and a list of received requests behind mutexes. Builders append to the queue. Streaming uses the trait's default replay.

## Global constraints
- Rust 2024, MSRV 1.88. No new dependency.
- Public API additions only. No existing item changes.
- No live LLM calls.
- Use `cargo --offline` for every cargo command.

### Task 1: ScriptedModel and its tests → verify: `cargo --offline test --test scripted` exits 0 and `cargo --offline test --test scripted -- --list 2>/dev/null | grep -c ': test$'` prints a number at least 6

**Blocked by:** none

**Files:**
- Modify: `src/testing.rs`
- Create: `tests/scripted.rs`

- [ ] Step 1: Invoke `lazy` before writing any code.
- [ ] Step 2: In `src/testing.rs`, change the `use` line to `use crate::{Error, GenerateRequest, GenerateResponse, Message, Model, ModelFuture, OutputContent, ResponseStatus, Role, Storage};` and add `use std::collections::VecDeque;` and `use std::sync::Mutex;`. Then append:

```rust
/// A [`Model`] that answers from a script, for testing an agent with no
/// network, key or spend.
///
/// Each call to [`Model::generate`] records the request and returns the next
/// scripted response, oldest first. A call after the script runs out returns
/// [`Error::InvalidResponse`]. Streaming replays each response through the
/// default [`Model::stream`].
///
/// [`crate::Agent::new`] takes the model by value. Wrap it in an
/// [`std::sync::Arc`] to read [`ScriptedModel::requests`] after a run:
///
/// ```
/// # async fn run() -> Result<(), freyja::Error> {
/// use freyja::{Agent, testing::ScriptedModel};
/// use std::sync::Arc;
///
/// let model = Arc::new(ScriptedModel::new().text("hi"));
/// let agent = Agent::new(model.clone());
/// let run = agent.ask("hi").await?;
/// assert_eq!(run.answer, "hi");
/// assert_eq!(model.requests().len(), 1);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Default)]
pub struct ScriptedModel {
    responses: Mutex<VecDeque<GenerateResponse>>,
    requests: Mutex<Vec<GenerateRequest>>,
    calls: usize,
}

impl ScriptedModel {
    /// Starts an empty script.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a completed response holding `text`.
    pub fn text(self, text: impl Into<String>) -> Self {
        self.respond(scripted(
            ResponseStatus::Completed,
            OutputContent::Text(text.into()),
        ))
    }

    /// Adds a response that asks for one call to the tool `name`, with an id
    /// unique within this script.
    pub fn tool_call(mut self, name: impl Into<String>, arguments: serde_json::Value) -> Self {
        self.calls += 1;
        let call = OutputContent::ToolCall {
            id: format!("call_{}", self.calls),
            name: name.into(),
            arguments: arguments.to_string(),
        };
        self.respond(scripted(ResponseStatus::RequiresAction, call))
    }

    /// Adds `response` as given, for any shape the other builders do not
    /// cover, such as parallel tool calls or token usage.
    pub fn respond(self, response: GenerateResponse) -> Self {
        self.responses
            .lock()
            .expect("not poisoned")
            .push_back(response);
        self
    }

    /// A clone of every request received, oldest first.
    pub fn requests(&self) -> Vec<GenerateRequest> {
        self.requests.lock().expect("not poisoned").clone()
    }
}

fn scripted(status: ResponseStatus, content: OutputContent) -> GenerateResponse {
    GenerateResponse {
        id: "scripted".into(),
        model: "scripted".into(),
        status,
        content: vec![content],
        usage: None,
        provider_metadata: None,
    }
}

impl Model for ScriptedModel {
    fn name(&self) -> &str {
        "scripted"
    }

    fn generate<'a>(&'a self, request: &'a GenerateRequest) -> ModelFuture<'a, GenerateResponse> {
        Box::pin(async move {
            self.requests
                .lock()
                .expect("not poisoned")
                .push(request.clone());
            self.responses
                .lock()
                .expect("not poisoned")
                .pop_front()
                .ok_or_else(|| Error::InvalidResponse {
                    endpoint: "scripted".into(),
                    message: "the script ran out of responses".into(),
                })
        })
    }
}
```

If a name in the `use` line is already imported or turns out unused, adjust the line so it compiles with no warning.

- [ ] Step 3: Create `tests/scripted.rs`:

```rust
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
```

If a test does not compile because of a detail of the public API (for example `tool_calls()` yielding a different tuple shape, `GenerateResponse` lacking `PartialEq`, or `InputContent` not exported at the root), adjust the test to the real API while keeping what it asserts.

- [ ] Step 4: Run `cargo --offline test --test scripted` and `cargo --offline clippy --all-targets -- -D warnings`.
- [ ] Step 5: Commit `src/testing.rs` and `tests/scripted.rs` as "Add ScriptedModel, a model that answers from a script".

### Task 2: Document ScriptedModel → verify: `RUSTDOCFLAGS='-D warnings' cargo --offline doc --no-deps` exits 0 and `grep -l ScriptedModel docs/extending.md CHANGELOG.md README.md | wc -l` prints a number at least 3

**Blocked by:** Task 1

**Files:**
- Modify: `docs/extending.md`
- Modify: `CHANGELOG.md`
- Modify: `README.md`

- [ ] Step 1: Invoke `lazy` and `plain` before writing.
- [ ] Step 2: In `docs/extending.md`, add a section `## Testing with a scripted model` after the section `## Checking a storage backend`. It says that `freyja::testing::ScriptedModel` answers from a script with no network, shows a short example that builds `ScriptedModel::new().tool_call("add", json!({"a": 1, "b": 2})).text("3")`, wraps it in `Arc`, runs `Agent::new(model.clone()).tool(add).ask(...)`, and reads `model.requests()`, and says that `respond` adds any other response shape and that a call after the script runs out returns `Error::InvalidResponse`.
- [ ] Step 3: In `CHANGELOG.md`, under `## Unreleased` then `### Added`, add one bullet in the style of its neighbours: `**`ScriptedModel`, a model that answers from a script, in `freyja::testing`.**` followed by one sentence naming `text`, `tool_call`, `respond` and `requests`, and the ran-out error.
- [ ] Step 4: In `README.md`, in the Phase 5 roadmap paragraph, after "a mock provider for testing agents without network access", add a sentence saying `freyja::testing::ScriptedModel` ships, a model that answers from a script.
- [ ] Step 5: Run `RUSTDOCFLAGS='-D warnings' cargo --offline doc --no-deps`.
- [ ] Step 6: Commit the three files as "Document ScriptedModel".
