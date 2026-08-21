//! Driving the tool-calling loop.

use crate::{
    Client, Error, GenerateRequest, Message, OutputContent, ResponseStatus, Tool, ToolChoice,
    ToolDefinition, Usage,
};

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

    /// Runs the tool loop, extending `messages` in place.
    ///
    /// The caller's vector is moved in and moved back out, so no copy of the
    /// transcript is made. On error it is restored to its original length, so a
    /// failed call never leaves a dangling turn behind.
    ///
    /// `ToolChoice::Required` is sent on the first turn only and downgraded to
    /// `ToolChoice::Auto` afterwards. Left in place it would force a tool call
    /// every round, and the model could never produce a final answer.
    pub async fn run(&self, messages: &mut Vec<Message>) -> Result<Run, Error> {
        let original_len = messages.len();
        let mut request = self.template.clone();
        request.messages = core::mem::take(messages);
        request.tools = self.definitions.clone();

        let mut usage = Usage {
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
        };
        let mut answer = String::new();
        let mut stop = StopReason::MaxTurns;
        let mut turns = 0usize;

        for turn in 0..self.max_turns {
            let response = match self.client.generate(&request).await {
                Ok(response) => response,
                Err(error) => {
                    *messages = core::mem::take(&mut request.messages);
                    messages.truncate(original_len);
                    return Err(error);
                }
            };

            turns += 1;
            if let Some(turn_usage) = response.usage {
                usage.input_tokens += turn_usage.input_tokens;
                usage.output_tokens += turn_usage.output_tokens;
                usage.total_tokens += turn_usage.total_tokens;
            }

            let refusal = response.content.iter().find_map(|content| match content {
                OutputContent::Refusal(text) => Some(text.clone()),
                _ => None,
            });

            request.messages.push(response.to_message());

            if let Some(text) = refusal {
                answer = text;
                stop = StopReason::Refused;
                break;
            }

            match response.status {
                ResponseStatus::Completed | ResponseStatus::RequiresAction => {}
                ResponseStatus::Incomplete => {
                    answer = response.output_text();
                    stop = StopReason::Incomplete;
                    break;
                }
                ResponseStatus::Failed | ResponseStatus::Other(_) => {
                    stop = StopReason::Failed;
                    break;
                }
            }

            if !response.has_tool_calls() {
                answer = response.output_text();
                stop = StopReason::Answered;
                break;
            }

            if turn == 0 && request.tool_choice == Some(ToolChoice::Required) {
                request.tool_choice = Some(ToolChoice::Auto);
            }

            // Owned, so the futures below do not borrow `response`.
            let calls: Vec<(String, String, String)> = response
                .tool_calls()
                .map(|(id, name, arguments)| {
                    (id.to_string(), name.to_string(), arguments.to_string())
                })
                .collect();

            let outputs = futures_util::future::join_all(
                calls
                    .iter()
                    .map(|(_, name, arguments)| self.dispatch(name, arguments)),
            )
            .await;

            request.messages.reserve(outputs.len());
            for ((id, _, _), output) in calls.iter().zip(outputs) {
                request
                    .messages
                    .push(Message::tool_result(id.clone(), output));
            }
        }

        *messages = core::mem::take(&mut request.messages);

        Ok(Run {
            answer,
            stop,
            usage,
            turns,
        })
    }

    /// Runs one requested call, turning every failure into text for the model.
    async fn dispatch(&self, name: &str, arguments: &str) -> String {
        match self.tools.iter().copied().find(|tool| tool.name() == name) {
            Some(tool) => tool
                .execute(arguments)
                .await
                .unwrap_or_else(|error| format!("error: {error:?}")),
            None => format!("error: unknown tool '{name}'"),
        }
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
        Client::custom(
            Dialect::OpenAiChat,
            "local",
            "http://127.0.0.1:1",
            "sk-test",
        )
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
