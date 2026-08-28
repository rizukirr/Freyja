//! One conversation, and the storage holding it.

use crate::{Agent, Context, Error, Message, Run, Storage};

/// One conversation, driven one turn at a time.
///
/// Built by [`Agent::conversation`] or [`Agent::conversation_in`]. It owns an
/// [`Agent`] clone and the backend holding the transcript, which is what makes
/// the exclusive borrow on [`Conversation::send`] sit on the thing that is
/// genuinely exclusive rather than on the agent.
///
/// Deliberately not `Clone`. Two handles to one conversation would both load
/// before either appended, so the second would answer against a transcript
/// missing the first.
pub struct Conversation<S: Storage> {
    agent: Agent,
    storage: S,
    window: Option<usize>,
}

impl<S: Storage> Conversation<S> {
    pub(crate) fn new(agent: Agent, storage: S) -> Self {
        Self {
            agent,
            storage,
            window: None,
        }
    }

    /// The backend holding this conversation.
    ///
    /// Everything it holds, which is not always everything that was sent: a
    /// window shapes what goes on the wire and leaves the backend untouched.
    pub fn storage(&self) -> &S {
        &self.storage
    }

    /// Adds one turn and runs the loop.
    ///
    /// Equivalent to [`Conversation::send_with`] with an empty context.
    pub async fn send(&mut self, message: impl Into<Message>) -> Result<Run, Error> {
        self.send_with(message, &Context::new()).await
    }

    /// Adds one turn and runs the loop with per-run context.
    ///
    /// Loads from storage, repairs the transcript, applies any window, appends
    /// the turn, runs the loop, then writes the new turns back. Storage is
    /// written last, so a failed run records nothing.
    pub async fn send_with(
        &mut self,
        message: impl Into<Message>,
        cx: &Context,
    ) -> Result<Run, Error> {
        let mut history = self
            .storage
            .load()
            .await
            .map_err(|error| self.agent.storage_error(format!("storage load: {error}")))?;

        // Every backend decides its own trimming, including ones nobody here
        // reviewed. This is what stops any of them sending a tool result whose
        // call it dropped, which every provider rejects with an error that
        // mentions nothing about trimming.
        crate::transcript::repair(&mut history);

        if let Some(groups) = self.window {
            history = crate::transcript::window_by_groups(&history, groups);
        }

        let before = history.len();
        history.push(message.into());

        let run = self.agent.run_loop(&mut history, cx).await?;

        self.storage
            .append(history.split_off(before))
            .await
            .map_err(|error| self.agent.storage_error(format!("storage append: {error}")))?;

        Ok(run)
    }
}

impl Conversation<Vec<Message>> {
    /// Send only the most recent `groups` turn groups, plus pinned turns.
    ///
    /// Everything is still held, and [`Conversation::storage`] returns it.
    ///
    /// A group is a message, except that an assistant turn requesting tools
    /// and the results answering it are one group. So an exchange costs two
    /// groups without tools and three with them: `window(20)` keeps roughly
    /// seven exchanges, not twenty.
    ///
    /// Only on a conversation held in this process. A backend of your own
    /// trims inside its own `load`, where it can push the limit into the query
    /// rather than fetching everything first.
    pub fn window(mut self, groups: usize) -> Self {
        self.window = Some(groups);
        self
    }
}
