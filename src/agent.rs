//! Driving the tool-calling loop.

use crate::{
    Client, Context, Error, GenerateRequest, Message, OutputContent, ResponseStatus, Tool,
    ToolChoice, ToolDefinition, Usage,
};
use std::sync::Arc;

/// Runs the tool-calling loop against a set of tools.
///
/// `Agent` holds configuration only. The transcript belongs to the caller, so
/// one `Agent` can serve any number of conversations at once.
#[derive(Clone)]
pub struct Agent {
    client: Client,
    tools: Vec<Arc<dyn Tool>>,
    definitions: Vec<ToolDefinition>,
    template: GenerateRequest,
    max_turns: usize,
    #[allow(clippy::type_complexity)]
    guard: Option<Arc<dyn Fn(&str, &str, &Context) -> Decision + Send + Sync>>,
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

/// What a guard decided about one requested tool call.
///
/// A `Deny` reaches the model as tool-result text, so the reason is the
/// model's only route to recovering — apologising, asking for permission, or
/// trying something else. A denial with no usable reason burns turns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Run the tool.
    Allow,
    /// Refuse, and tell the model why.
    Deny(String),
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
            guard: None,
        }
    }

    /// Adds a tool, replacing any already registered under the same name.
    ///
    /// Its provider-facing definition is built here, once, rather than on every
    /// run.
    pub fn tool(mut self, tool: impl Tool + 'static) -> Self {
        self.insert(Arc::new(tool));
        self
    }

    /// Adds tools whose types are already erased, as a runtime source hands them over.
    ///
    /// Each replaces any already registered under the same name.
    pub fn tools(mut self, tools: impl IntoIterator<Item = Arc<dyn Tool>>) -> Self {
        for tool in tools {
            self.insert(tool);
        }
        self
    }

    fn insert(&mut self, tool: Arc<dyn Tool>) {
        let definition = tool.definition();
        match self
            .tools
            .iter()
            .position(|existing| existing.name() == tool.name())
        {
            Some(index) => {
                self.tools[index] = tool;
                self.definitions[index] = definition;
            }
            None => {
                self.tools.push(tool);
                self.definitions.push(definition);
            }
        }
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

    /// Sets a guard consulted before every tool call.
    ///
    /// It receives the tool name, the model's raw JSON arguments, and the run
    /// context. Returning [`Decision::Deny`] stops the call and sends the
    /// reason to the model instead of running anything.
    ///
    /// The guard sees every requested name, including names registered through
    /// [`Agent::tools`] and names matching no tool at all, so a policy written
    /// here cannot be bypassed by a tool someone forgot to wrap. It is
    /// synchronous: there is nothing to await, and keeping it so keeps it out
    /// of the boxed-future machinery.
    pub fn guard(
        mut self,
        guard: impl Fn(&str, &str, &Context) -> Decision + Send + Sync + 'static,
    ) -> Self {
        self.guard = Some(Arc::new(guard));
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
    ///
    /// `cx` is handed to every tool call and is never sent to the model.
    pub async fn run_with(&self, messages: &mut Vec<Message>, cx: &Context) -> Result<Run, Error> {
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
                    .map(|(_, name, arguments)| self.dispatch(name, arguments, cx)),
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

    /// Runs the tool loop, extending `messages` in place.
    ///
    /// Equivalent to [`Agent::run_with`] with an empty context.
    pub async fn run(&self, messages: &mut Vec<Message>) -> Result<Run, Error> {
        self.run_with(messages, &Context::new()).await
    }

    /// Runs one requested call, turning every refusal and failure into text
    /// for the model.
    ///
    /// The guard runs before the lookup, so no name reaches a tool without
    /// passing it.
    async fn dispatch(&self, name: &str, arguments: &str, cx: &Context) -> String {
        if let Some(guard) = &self.guard
            && let Decision::Deny(reason) = guard(name, arguments, cx)
        {
            return format!("denied: {reason}");
        }
        match self.tools.iter().find(|tool| tool.name() == name) {
            Some(tool) => tool
                .call(arguments, cx)
                .await
                .unwrap_or_else(|error| format!("error: {error}")),
            None => format!("error: unknown tool '{name}'"),
        }
    }

    /// Starts a conversation that carries its own transcript.
    pub fn chat(&self) -> Chat {
        Chat {
            agent: self.clone(),
            messages: Vec::new(),
            last: None,
        }
    }
}

/// A conversation that keeps its own transcript.
///
/// Cheaper than it looks: cloning an [`Agent`] copies refcounts and small
/// vectors, so one agent can hand out many chats.
#[derive(Clone)]
pub struct Chat {
    agent: Agent,
    messages: Vec<Message>,
    last: Option<Run>,
}

impl Chat {
    /// Appends a user turn and runs the loop over the whole conversation.
    pub async fn ask(&mut self, prompt: impl Into<String>) -> Result<&Run, Error> {
        self.ask_with(prompt, &Context::new()).await
    }

    /// Appends a user turn and runs the loop with per-run context.
    pub async fn ask_with(
        &mut self,
        prompt: impl Into<String>,
        cx: &Context,
    ) -> Result<&Run, Error> {
        self.messages
            .push(Message::text(crate::Role::User, prompt.into()));
        let run = self.agent.run_with(&mut self.messages, cx).await?;
        Ok(self.last.insert(run))
    }

    /// Borrows the transcript, for persisting or inspecting.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Takes the transcript, consuming the chat.
    pub fn into_messages(self) -> Vec<Message> {
        self.messages
    }
}

#[cfg(test)]
mod tests {
    use super::Agent;
    use crate::{Client, Context, Decision, Dialect, Tool, ToolDefinition, ToolFuture};

    struct Add;

    impl Tool for Add {
        fn name(&self) -> &str {
            "add"
        }

        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new("add", "adds two numbers")
        }

        fn call<'a>(&'a self, _arguments: &'a str, _cx: &'a Context) -> ToolFuture<'a> {
            Box::pin(async { Ok("42".to_string()) })
        }
    }

    /// A second tool under the same name, to prove registration replaces.
    struct AddAgain;

    impl Tool for AddAgain {
        fn name(&self) -> &str {
            "add"
        }

        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new("add", "adds two numbers, again")
        }

        fn call<'a>(&'a self, _arguments: &'a str, _cx: &'a Context) -> ToolFuture<'a> {
            Box::pin(async { Ok("43".to_string()) })
        }
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
        let agent = Agent::new(client()).tool(Add);
        assert_eq!(agent.definitions.len(), agent.tools.len());
        assert_eq!(agent.definitions[0].name, "add");
    }

    #[test]
    fn registering_the_same_name_replaces_rather_than_shadows() {
        let agent = Agent::new(client()).tool(Add).tool(AddAgain);
        assert_eq!(agent.tools.len(), agent.definitions.len());
        assert_eq!(
            agent.definitions[0].description.as_deref(),
            Some("adds two numbers, again")
        );
    }

    #[test]
    fn an_agent_without_a_guard_holds_none() {
        assert!(Agent::new(client()).guard.is_none());
    }

    #[test]
    fn a_guard_is_stored_and_callable() {
        let agent = Agent::new(client()).guard(|name, _arguments, _cx| {
            if name == "wipe" {
                Decision::Deny("no".to_string())
            } else {
                Decision::Allow
            }
        });
        let guard = agent.guard.as_ref().expect("a guard was set");
        let cx = Context::new();
        assert_eq!(guard("wipe", "{}", &cx), Decision::Deny("no".to_string()));
        assert_eq!(guard("add", "{}", &cx), Decision::Allow);
    }
}
