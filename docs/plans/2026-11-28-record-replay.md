# record-replay: Implementation Plan

**Spec:** docs/specs/2026-11-28-record-replay-design.md
**Goal:** `freyja::testing::Recorder` saves a real run's responses to a file, and `ScriptedModel::replay` plays them back with no network.
**Architecture:** The response types in `src/model/response.rs` gain serde derives. `src/testing.rs` gains `Recorder<M>`, a `Model` wrapper that keeps each successful response and writes them as JSON Lines with `save`, and `ScriptedModel::replay`, which reads that file into a script. `tests/record_replay.rs` covers it.

## Global constraints
- Rust 2024, MSRV 1.88.
- No new dependency, normal or dev.
- No live LLM calls. Use `cargo --offline`.

### Task 1: Recorder, replay and their tests → verify: `cargo --offline test --all-features` exits 0 and `cargo --offline clippy --all-features --all-targets -- -D warnings` exits 0

**Blocked by:** none

**Files:**
- Modify: `src/model/response.rs`
- Modify: `src/testing.rs`
- Create: `tests/record_replay.rs`

- [ ] Step 1: In `src/model/response.rs`, add `use serde::{Deserialize, Serialize};` to the imports, and add `Serialize, Deserialize` to the `#[derive(...)]` of `GenerateResponse`, `ResponseStatus`, `OutputContent` and `Usage`. Add no `#[serde(...)]` attributes.

- [ ] Step 2: In `src/testing.rs`, add this method inside `impl ScriptedModel`, after `respond`:

```rust
    /// Loads a script from a file [`Recorder::save`] wrote, answering its
    /// responses in the order they were recorded.
    ///
    /// Fails with [`std::io::ErrorKind::InvalidData`] when the first line is
    /// not a version 1 recording header or a later line is not a response. The
    /// message names the line.
    pub fn replay(path: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let invalid = |line: usize, why: String| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, format!("line {line}: {why}"))
        };
        let mut lines = text.lines();
        let header: serde_json::Value = serde_json::from_str(lines.next().unwrap_or(""))
            .map_err(|error| invalid(1, error.to_string()))?;
        if header != json!({ "freyja_recording": 1 }) {
            return Err(invalid(1, "not a version 1 freyja recording".into()));
        }
        let mut script = Self::new();
        for (index, line) in lines.enumerate() {
            let response =
                serde_json::from_str(line).map_err(|error| invalid(index + 2, error.to_string()))?;
            script = script.respond(response);
        }
        Ok(script)
    }
```

- [ ] Step 3: In `src/testing.rs`, after the `impl Model for ScriptedModel` block, add:

```rust
/// Wraps a model and keeps a copy of every response it returns, so a run
/// against a real provider can be saved once and replayed in tests with
/// [`ScriptedModel::replay`].
///
/// It answers exactly as the wrapped model does. A call that fails is passed
/// through and not recorded. A streaming caller is answered all at once, after
/// the whole response has arrived, because recording needs the whole
/// response. Requests are not recorded.
///
/// ```
/// use freyja::testing::{Recorder, ScriptedModel};
/// use freyja::Agent;
/// use std::sync::Arc;
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let file = std::env::temp_dir().join(format!("freyja-doc-{}.jsonl", std::process::id()));
///
/// let model = Arc::new(Recorder::new(ScriptedModel::new().text("hi")));
/// Agent::new(model.clone()).ask("hi").await?;
/// model.save(&file)?;
///
/// let run = Agent::new(ScriptedModel::replay(&file)?).ask("hi").await?;
/// assert_eq!(run.answer, "hi");
/// # std::fs::remove_file(&file)?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct Recorder<M> {
    model: M,
    responses: Mutex<Vec<GenerateResponse>>,
}

impl<M: Model> Recorder<M> {
    /// Wraps `model`, with nothing recorded yet.
    pub fn new(model: M) -> Self {
        Self {
            model,
            responses: Mutex::new(Vec::new()),
        }
    }

    /// Writes every response recorded so far to `path`, replacing the file.
    ///
    /// The file is JSON Lines: a header line `{"freyja_recording":1}`, then
    /// one response per line.
    pub fn save(&self, path: impl AsRef<std::path::Path>) -> std::io::Result<()> {
        let mut out = String::from("{\"freyja_recording\":1}\n");
        for response in self.responses.lock().expect("not poisoned").iter() {
            out.push_str(&serde_json::to_string(response).map_err(std::io::Error::other)?);
            out.push('\n');
        }
        std::fs::write(path, out)
    }
}

impl<M: Model> Model for Recorder<M> {
    fn name(&self) -> &str {
        self.model.name()
    }

    fn generate<'a>(&'a self, request: &'a GenerateRequest) -> ModelFuture<'a, GenerateResponse> {
        Box::pin(async move {
            let response = self.model.generate(request).await?;
            self.responses
                .lock()
                .expect("not poisoned")
                .push(response.clone());
            Ok(response)
        })
    }
}
```

If the doc example fails to compile because `tokio` is not available to doctests, read `Cargo.toml` `[dev-dependencies]` and the existing doc example on `ScriptedModel` in `src/testing.rs`, and use the same harness that example uses.

- [ ] Step 4: Create `tests/record_replay.rs`:

```rust
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
```

- [ ] Step 5: Run `cargo --offline fmt`, then `cargo --offline test --all-features` and `cargo --offline clippy --all-features --all-targets -- -D warnings`.
- [ ] Step 6: Commit `src/model/response.rs`, `src/testing.rs` and `tests/record_replay.rs` as "Recorder saves a run, and ScriptedModel::replay plays it back".

### Task 2: Documentation → verify: `RUSTDOCFLAGS='-D warnings' cargo --offline doc --no-deps --all-features` exits 0 and `grep -l Recorder docs/extending.md CHANGELOG.md README.md` lists all three files

**Blocked by:** Task 1

**Files:**
- Modify: `docs/extending.md`
- Modify: `CHANGELOG.md`
- Modify: `README.md`

- [ ] Step 1: In `docs/extending.md`, in the section `## Testing with a scripted model`, after the paragraph that starts "`respond` adds any other response shape", add a blank line, then this paragraph on one line: "`Recorder` turns a real run into a script. Wrap the model in it, run the agent once against the real provider, and save what the model answered. `ScriptedModel::replay` loads the file, so the test runs on those answers with no network and no key. Only answers are saved: replay hands them out in order whatever the requests are, and a call that failed is not saved. A recorder answers a streaming caller all at once, after the whole response has arrived." Then a blank line and this code block:

```rust
use freyja::testing::{Recorder, ScriptedModel};
use std::sync::Arc;

// Once, against the real provider.
let model = Arc::new(Recorder::new(Client::from_env()?));
Agent::new(model.clone()).tool(add).ask("add 1 and 2").await?;
model.save("tests/fixtures/add.jsonl")?;

// In the test, from then on.
let model = ScriptedModel::replay("tests/fixtures/add.jsonl")?;
let run = Agent::new(model).tool(add).ask("add 1 and 2").await?;
```

- [ ] Step 2: In `CHANGELOG.md`, under `## Unreleased` (which has no subsections yet), add a blank line, the heading `### Added`, a blank line, and this bullet on one line: "- **`freyja::testing::Recorder` saves a real run, and `ScriptedModel::replay` plays it back.** A test runs on answers a real model gave, with no network and no key. The file is JSON Lines with a version header, and `GenerateResponse`, `ResponseStatus`, `OutputContent` and `Usage` now implement `Serialize` and `Deserialize`."
- [ ] Step 3: In `README.md`, in the line starting "**Phase 5, observability and release.**", replace "Money cost accounting and record and replay for deterministic tests are not built." with "`freyja::testing::Recorder` saves a real run, and `ScriptedModel::replay` plays it back in tests. Money cost accounting is not built."
- [ ] Step 4: Run `RUSTDOCFLAGS='-D warnings' cargo --offline doc --no-deps --all-features`.
- [ ] Step 5: Commit the three files as "Document record and replay".
