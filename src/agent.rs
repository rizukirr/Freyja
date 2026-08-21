//! Driving the tool-calling loop.

use crate::{Client, GenerateRequest, Message, Tool, ToolDefinition, Usage};

/// Runs the tool-calling loop against a set of tools.
///
/// `Agent` holds configuration only. The transcript belongs to the caller, so
/// one `Agent` can serve any number of conversations at once.
#[derive(Clone)]
pub struct Agent {
    client: Client,
    tools: Vec<Tool>,
    definitions: Vec<ToolDefinition>,
    template: GenerateRequest,
    max_turns: usize,
}

/// What one call to [`Agent::run`] produced.
///
/// The transcript is not here: `run` extends the caller's vector in place.
#[derive(Debug, Clone)]
pub struct Run {
    /// The final assistant text, empty when the loop ended without one.
    pub answer: String,
    /// Why the loop stopped.
    pub stop: StopReason,
    /// Token usage summed across every turn.
    pub usage: Usage,
    /// How many requests this run made.
    pub turns: usize,
}

/// Why [`Agent::run`] stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// The model answered without requesting more tools.
    Answered,
    /// The turn bound was reached first.
    MaxTurns,
    /// The model refused.
    Refused,
    /// The provider cut the generation short.
    Incomplete,
    /// The provider reported a failed or unmodelled status.
    Failed,
}

impl Agent {
    /// Creates an agent with no tools and a turn bound of five.
    pub fn new(client: Client) -> Self {
        Self {
            client,
            tools: Vec::new(),
            definitions: Vec::new(),
            template: GenerateRequest::new(),
            max_turns: 5,
        }
    }

    /// Sets the tools the model may call.
    ///
    /// Their provider-facing definitions are built here, once, rather than on
    /// every run.
    pub fn tools(mut self, tools: impl Into<Vec<Tool>>) -> Self {
        self.tools = tools.into();
        self.definitions = self.tools.iter().map(|tool| tool.definition()).collect();
        self
    }

    /// Sets the request every turn is built from: model, temperature, tool choice.
    ///
    /// Any messages or tools on the template are replaced by [`Agent::run`].
    pub fn request(mut self, template: GenerateRequest) -> Self {
        self.template = template;
        self
    }

    /// Sets how many requests one run may make before giving up.
    pub fn max_turns(mut self, turns: usize) -> Self {
        self.max_turns = turns;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::Agent;
    use crate::{Client, Dialect, Tool, ToolDefinition, ToolError};

    fn definition() -> ToolDefinition {
        ToolDefinition::new("add", "adds two numbers")
    }

    fn execute(_arguments: &str) -> Result<String, ToolError> {
        Ok("42".to_string())
    }

    fn client() -> Client {
        Client::custom(Dialect::OpenAiChat, "local", "http://127.0.0.1:1", "sk-test")
    }

    #[test]
    fn defaults_to_a_bounded_loop() {
        assert_eq!(Agent::new(client()).max_turns, 5);
    }

    #[test]
    fn tools_builds_definitions_once() {
        let agent = Agent::new(client()).tools([Tool::new("add", definition, execute)]);
        assert_eq!(agent.definitions.len(), agent.tools.len());
        assert_eq!(agent.definitions[0].name, "add");
    }
}
