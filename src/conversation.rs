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
}

impl<S: Storage> Conversation<S> {
    pub(crate) fn new(agent: Agent, storage: S) -> Self {
        Self { agent, storage }
    }

    /// The backend holding this conversation.
    ///
    /// Everything it holds, which is not always everything that was sent: a
    /// window shapes what goes on the wire and leaves the backend untouched.
    pub fn storage(&self) -> &S {
        &self.storage
    }

    /// Two overlapping sends on one conversation do not compile. Both would
    /// load before either appended, so the second would answer against a
    /// transcript missing the first, and the stored order would be the order
    /// the network replied in rather than the order you sent:
    ///
    /// ```compile_fail,E0499
    /// # async fn run(mut chat: freyja::Conversation<Vec<freyja::Message>>) {
    /// let first = chat.send("one");
    /// let second = chat.send("two");
    /// # let _ = (first, second);
    /// # }
    /// ```
    ///
    /// Sequential sends are the supported shape, and this one must compile. It
    /// is what catches a later change that makes the case above fail for some
    /// new reason while still reporting E0499:
    ///
    /// ```no_run
    /// # async fn run(mut chat: freyja::Conversation<Vec<freyja::Message>>)
    /// #     -> Result<(), freyja::Error> {
    /// chat.send("one").await?;
    /// chat.send("two").await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// One agent hands out as many conversations as you want, which is the
    /// shape a server writes: the agent in application state, a conversation
    /// per request. This must compile:
    ///
    /// ```no_run
    /// # fn run(agent: freyja::Agent) {
    /// let mut first = agent.conversation();
    /// let mut second = agent.conversation();
    /// # let _ = (&mut first, &mut second);
    /// # }
    /// ```
    ///
    /// Adds one turn and runs the loop.
    ///
    /// Equivalent to [`Conversation::send_with`] with an empty context.
    pub async fn send(&mut self, message: impl Into<Message>) -> Result<Run, Error> {
        self.send_with(message, &Context::new()).await
    }

    /// Adds one turn and runs the loop with per-run context.
    ///
    /// Loads from storage, repairs the transcript, appends the turn, runs the
    /// loop, then writes the new turns back. Storage is written last, so a
    /// failed run records nothing. Trimming is the backend's business and has
    /// already happened inside `load`.
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
