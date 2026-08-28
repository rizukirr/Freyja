//! Driving the tool-calling loop.

use crate::{
    Client, Context, Conversation, Dialect, Error, GenerateRequest, Message, OutputContent,
    ReasoningEffort, ResponseStatus, Storage, Tool, ToolChoice, ToolDefinition, Usage,
};
use serde_json::Value;
use std::sync::Arc;

/// The closure an [`Agent`] consults before running a tool.
type GuardFn = dyn Fn(&str, &str, &Context) -> Decision + Send + Sync;

/// Runs the tool-calling loop against a set of tools.
///
/// `Agent` holds configuration only. The transcript belongs to the caller, so
/// one `Agent` can serve any number of conversations at once.
///
/// `previous_response_id` is deliberately unreachable from here: it means the
/// vendor holds the conversation, and an `Agent` run holds it in the caller's
/// vector, so setting both would give one run two disagreeing transcripts.
/// `metadata` and `response_format` are simply not exposed yet, and adding
/// either later breaks nothing.
#[derive(Clone)]
pub struct Agent {
    client: Client,
    tools: Vec<Arc<dyn Tool>>,
    definitions: Vec<ToolDefinition>,
    template: GenerateRequest,
    max_turns: usize,
    guard: Option<Arc<GuardFn>>,
    system: Option<String>,
}

/// What one call to [`Agent::messages`] produced.
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

/// Why [`Agent::messages`] stopped.
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
            system: None,
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

    /// Sets the model every turn asks for.
    ///
    /// Leaving it unset uses the endpoint's default model.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.template = core::mem::take(&mut self.template).model(model);
        self
    }

    /// Caps the tokens the model may generate on each turn.
    pub fn max_tokens(mut self, value: u32) -> Self {
        self.template = core::mem::take(&mut self.template).max_tokens(value);
        self
    }

    /// Sets the sampling temperature for every turn.
    pub fn temperature(mut self, value: f32) -> Self {
        self.template = core::mem::take(&mut self.template).temperature(value);
        self
    }

    /// Sets nucleus sampling for every turn.
    pub fn top_p(mut self, value: f32) -> Self {
        self.template = core::mem::take(&mut self.template).top_p(value);
        self
    }

    /// Sets how much internal reasoning the model spends before answering.
    pub fn reasoning_effort(mut self, effort: ReasoningEffort) -> Self {
        self.template = core::mem::take(&mut self.template).reasoning_effort(effort);
        self
    }

    /// Constrains which tools the model may call.
    ///
    /// [`ToolChoice::Required`] is sent on the first turn only and downgraded
    /// to [`ToolChoice::Auto`] afterwards. Left in place it would force a tool
    /// call every round and the model could never produce a final answer.
    pub fn tool_choice(mut self, choice: ToolChoice) -> Self {
        self.template = core::mem::take(&mut self.template).tool_choice(choice);
        self
    }

    /// Adds dialect-scoped fields deep-merged into the wire body of every turn.
    ///
    /// Reaching a vendor-only field without forking, for an agent as much as
    /// for a single request.
    ///
    /// # Panics
    ///
    /// Panics when `fields` is not a JSON object, matching
    /// [`GenerateRequest::extra_for`].
    pub fn extra_for(mut self, dialect: Dialect, fields: Value) -> Self {
        self.template = core::mem::take(&mut self.template).extra_for(dialect, fields);
        self
    }

    /// Sets a system instruction sent ahead of the transcript on every turn.
    ///
    /// It is not part of the caller's transcript, so it is never returned by
    /// [`Agent::messages`] and never counted in the caller's vector. It is
    /// always prepended just before the request is sent, so nothing else here
    /// can drop it or see it first.
    pub fn system(mut self, instruction: impl Into<String>) -> Self {
        self.system = Some(instruction.into());
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

    /// Runs the tool loop over `messages`, extending it in place.
    ///
    /// Crate-private: the public way in is [`crate::Conversation::send`],
    /// which owns the transcript rather than borrowing the caller's. The
    /// caller's vector is moved in and moved back out, so no copy is made. On
    /// error it is restored to its original length, so a failed call never
    /// leaves a dangling turn behind.
    ///
    /// `ToolChoice::Required` is sent on the first turn only and downgraded to
    /// `ToolChoice::Auto` afterwards. Left in place it would force a tool call
    /// every round, and the model could never produce a final answer.
    ///
    /// `cx` is handed to every tool call and is never sent to the model.
    pub(crate) async fn run_loop(
        &self,
        messages: &mut Vec<Message>,
        cx: &Context,
    ) -> Result<Run, Error> {
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
            // Swaps the full transcript out for a copy with the system
            // instruction prepended, rather than mutating `request.messages`
            // directly. The full transcript is restored immediately after the
            // request returns, whether it succeeded or failed, so an error
            // path never hands the caller a vector with the instruction in it.
            let mut saved = None;
            if let Some(system) = &self.system {
                let mut chosen = request.messages.clone();
                chosen.insert(0, Message::text(crate::Role::System, system.clone()));
                saved = Some(core::mem::replace(&mut request.messages, chosen));
            }

            let response = self.client.generate(&request).await;

            if let Some(saved) = saved {
                request.messages = saved;
            }

            let response = match response {
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

    /// Starts a conversation held in `storage`.
    ///
    /// Pass [`crate::InMemoryStorage`] for one held in this process, `&mut
    /// history` to run over a transcript you already hold, which is extended
    /// in place, or a backend of your own.
    ///
    /// ```no_run
    /// # async fn run(agent: freyja::Agent) -> Result<(), freyja::Error> {
    /// use freyja::InMemoryStorage;
    ///
    /// let mut chat = agent.conversation(InMemoryStorage::new());
    /// println!("{}", chat.send("hello").await?.answer);
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// Takes `&self`, so one agent hands out as many conversations as you
    /// like. The agent is configuration, and configuration is shareable. The
    /// storage is taken by value, so the conversation owns it, which is what
    /// makes the exclusive borrow on [`crate::Conversation::send`] a true
    /// statement rather than a convention.
    pub fn conversation<S: Storage>(&self, storage: S) -> Conversation<S> {
        Conversation::new(self.clone(), storage)
    }

    /// Wraps a backend failure, which has no endpoint of its own.
    pub(crate) fn storage_error(&self, message: String) -> Error {
        Error::InvalidRequest {
            endpoint: self.client.config().name.clone(),
            message,
        }
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
