//! Condensing turns a window drops into a summary the model still sees.

use crate::{Client, Error, GenerateRequest, InputContent, Message, ResponseStatus, Role};

/// The instruction sent when no prompt is given.
///
/// Asks for facts rather than prose. Each item in a list is kept or dropped
/// whole where a paragraph blurs, and names and numbers are what a later turn
/// most often needs back.
const DEFAULT_PROMPT: &str = "You condense the earlier part of a conversation so it can continue \
without it. List the durable facts, decisions, user preferences and open tasks it contains, one \
per line. Keep names, numbers and identifiers exactly as written. Leave out greetings and anything \
a later turn replaced. Reply with the list only.";

/// Turns conversation turns into a summary, with one model call.
///
/// Holds its own [`Client`] and builds its own request, so nothing from the
/// conversation it serves carries over: not the agent's system instruction,
/// not its tools, not its token cap. That also means it can summarize with a
/// cheaper model than the conversation uses.
///
/// ```no_run
/// # async fn run(client: freyja::Client, dropped: Vec<freyja::Message>) -> Result<(), freyja::Error> {
/// use freyja::Summarizer;
///
/// let summarizer = Summarizer::new(client).model("a-cheaper-model");
/// let summary = summarizer.summarize(&dropped).await?;
/// # let _ = summary;
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct Summarizer {
    client: Client,
    model: Option<String>,
    prompt: String,
    max_tokens: Option<u32>,
}

impl Summarizer {
    /// A summarizer calling `client`, with the default prompt and the
    /// endpoint's default model.
    pub fn new(client: Client) -> Self {
        Self {
            client,
            model: None,
            prompt: DEFAULT_PROMPT.into(),
            max_tokens: None,
        }
    }

    /// The model to summarize with. Unset, the endpoint's default is used.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Replaces the instruction the summarizer is given.
    ///
    /// The default asks for a list of facts, decisions, preferences and open
    /// tasks, one per line.
    pub fn prompt(mut self, prompt: impl Into<String>) -> Self {
        self.prompt = prompt.into();
        self
    }

    /// Caps how long the summary may be.
    ///
    /// A summary that reaches the cap comes back unfinished, and
    /// [`Summarizer::summarize`] returns an error instead of half a summary.
    /// Ask for brevity in the prompt, and treat this as a guard.
    pub fn max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// Summarizes `messages` with one request.
    ///
    /// The turns are sent as one block of text rather than replayed as a
    /// conversation. Replayed, the model would answer the last turn instead of
    /// summarizing, and every provider's rules on turn order and tool pairing
    /// would apply to a slice cut from the middle of a conversation. Reasoning
    /// parts are left out, since they may be dropped but never summarized.
    /// Leaving pinned turns out is the caller's job.
    ///
    /// # Errors
    ///
    /// Whatever [`Client::generate`] returns, and [`Error::InvalidResponse`]
    /// when the model stops before finishing or finishes with no text.
    pub async fn summarize(&self, messages: &[Message]) -> Result<String, Error> {
        let mut request = GenerateRequest::new()
            .message(Message::text(Role::System, self.prompt.as_str()))
            .message(Message::text(Role::User, render(messages)));
        if let Some(model) = &self.model {
            request = request.model(model);
        }
        if let Some(limit) = self.max_tokens {
            request = request.max_tokens(limit);
        }

        let response = self.client.generate(&request).await?;
        if response.status != ResponseStatus::Completed {
            return Err(self.unusable(format!("summary unfinished: {:?}", response.status)));
        }

        let summary = response.output_text();
        if summary.trim().is_empty() {
            return Err(self.unusable("summary was empty".into()));
        }
        Ok(summary)
    }

    /// The error for a reply that arrived but cannot serve as a summary.
    fn unusable(&self, message: String) -> Error {
        Error::InvalidResponse {
            endpoint: self.client.config().name.clone(),
            message,
        }
    }
}

/// One line per part, so the summarizer reads a transcript instead of taking
/// part in one.
fn render(messages: &[Message]) -> String {
    let mut lines = Vec::new();
    for message in messages {
        let speaker = match message.role {
            Role::System | Role::Developer => "Instruction",
            Role::User => "User",
            Role::Assistant => "Assistant",
            Role::Tool => "Tool",
        };
        for part in &message.content {
            lines.push(match part {
                InputContent::Text(text) => format!("{speaker}: {text}"),
                InputContent::ImageUrl(_) => format!("{speaker}: [image]"),
                InputContent::ToolCall {
                    name, arguments, ..
                } => format!("{speaker} called {name} with {arguments}"),
                InputContent::ToolResult { output, .. } => format!("{speaker} result: {output}"),
                // Opaque provider state: it may be dropped but never summarized.
                InputContent::Reasoning { .. } => continue,
            });
        }
    }
    lines.join("\n")
}
