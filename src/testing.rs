//! Checks a crate outside Freyja runs against its own implementations.

use crate::{
    Error, GenerateRequest, GenerateResponse, Message, Model, ModelFuture, OutputContent,
    ResponseStatus, Role, Storage,
};
use serde_json::json;
use std::collections::VecDeque;
use std::sync::Mutex;

/// Checks that a [`Storage`] honours the rules the trait documents.
///
/// `make` returns a new, empty backend each time it is called. Run it from
/// the backend crate's own tests, under any async runtime:
///
/// ```
/// # async fn run() {
/// freyja::testing::storage_conformance(freyja::InMemoryStorage::new).await;
/// # }
/// ```
///
/// # Panics
///
/// On the first broken rule, with a message naming it.
pub async fn storage_conformance<S: Storage>(mut make: impl FnMut() -> S) {
    let first = || {
        vec![
            Message::text(Role::User, "one"),
            Message::text(Role::Assistant, "two"),
        ]
    };
    let second = || vec![Message::text(Role::User, "three")];

    let mut storage = make();
    assert!(
        storage
            .load()
            .await
            .expect("load a new conversation")
            .is_empty(),
        "a new backend must load an empty conversation"
    );

    storage.append(first()).await.expect("append");
    storage.append(second()).await.expect("append");
    let mut expected = first();
    expected.extend(second());
    assert_eq!(
        storage.load().await.expect("load"),
        expected,
        "load must return every appended message, oldest first"
    );

    storage
        .clear()
        .await
        .expect("clear a conversation that holds messages");
    assert!(
        storage.load().await.expect("load after clear").is_empty(),
        "clear must empty the conversation"
    );
    storage
        .clear()
        .await
        .expect("clear must succeed on an empty conversation");

    let mut storage = make();
    storage
        .set_state("conformance", json!(1))
        .await
        .expect("set state");
    if storage
        .state("conformance")
        .await
        .expect("read state")
        .is_some()
    {
        storage
            .set_state("conformance", json!(2))
            .await
            .expect("set state");
        assert_eq!(
            storage.state("conformance").await.expect("read state"),
            Some(json!(2)),
            "state must return the latest value set"
        );
        assert_eq!(
            storage.state("unset").await.expect("read state"),
            None,
            "a key never set must read as None"
        );
        storage.clear().await.expect("clear");
        assert_eq!(
            storage.state("conformance").await.expect("read state"),
            None,
            "clear must remove state"
        );
    }
}

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

    /// Loads a script from a file [`Recorder::save`] wrote, answering its
    /// responses in the order they were recorded.
    ///
    /// Fails with [`std::io::ErrorKind::InvalidData`] when the first line is
    /// not a version 1 recording header or a later line is not a response. The
    /// message names the line.
    pub fn replay(path: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let invalid = |line: usize, why: String| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("line {line}: {why}"),
            )
        };
        let mut lines = text.lines();
        let header: serde_json::Value = serde_json::from_str(lines.next().unwrap_or(""))
            .map_err(|error| invalid(1, error.to_string()))?;
        if header != json!({ "freyja_recording": 1 }) {
            return Err(invalid(1, "not a version 1 freyja recording".into()));
        }
        let mut script = Self::new();
        for (index, line) in lines.enumerate() {
            let response = serde_json::from_str(line)
                .map_err(|error| invalid(index + 2, error.to_string()))?;
            script = script.respond(response);
        }
        Ok(script)
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
