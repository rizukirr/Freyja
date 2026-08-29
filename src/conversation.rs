//! One conversation, and the storage holding it.

use crate::{Agent, Context, Error, InputContent, Message, Run, Storage};

/// One conversation, driven one turn at a time.
///
/// Built by [`Agent::conversation`]. It owns an
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
    /// use freyja::InMemoryStorage;
    ///
    /// let mut first = agent.conversation(InMemoryStorage::new());
    /// let mut second = agent.conversation(InMemoryStorage::new());
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
        // Held rather than pushed and forgotten: the append boundary below
        // cannot be an index into `history`, because `repair` may remove
        // messages before it and shift everything after.
        let turn: Message = message.into();

        let mut history = self
            .storage
            .load()
            .await
            .map_err(|error| self.agent.storage_error(format!("storage load: {error}")))?;

        history.push(turn.clone());

        // Every backend decides its own trimming, including ones nobody here
        // reviewed. This is what stops any of them sending a tool result whose
        // call it dropped, which every provider rejects with an error that
        // mentions nothing about trimming.
        //
        // It runs after the caller's turn is pushed, not before, so it judges
        // the transcript that will actually be sent. Repairing the loaded
        // history alone deletes a trailing unanswered call one step before the
        // caller supplies the answer that would have made it valid, which is
        // exactly the human-in-the-loop tool approval shape.
        crate::transcript::repair(&mut history);

        // `repair` only removes content and never reorders, and the turn was
        // pushed last, so it is still last and still equal unless the repair
        // pass took it or part of it. Checking here rather than after the run
        // is what makes the refusal free: no request is sent and storage is
        // never appended to.
        //
        // Partial removal counts. A turn of text plus an orphaned tool result
        // keeps its text and loses the result, and sending the surviving half
        // under an `Ok` is the same silent loss at a smaller scale.
        if history.last() != Some(&turn) {
            // Only the ids that actually answer nothing, not every result in
            // the turn: a turn may carry one good result and one orphan, and
            // naming the good one would send the reader looking for a bug that
            // is not there.
            let answered: Vec<&str> = history
                .iter()
                .flat_map(|message| message.content.iter())
                .filter_map(|content| match content {
                    InputContent::ToolCall { id, .. } => Some(id.as_str()),
                    _ => None,
                })
                .collect();

            let orphans: Vec<&str> = turn
                .content
                .iter()
                .filter_map(|content| match content {
                    InputContent::ToolResult { call_id, .. }
                        if !answered.contains(&call_id.as_str()) =>
                    {
                        Some(call_id.as_str())
                    }
                    _ => None,
                })
                .collect();

            // The ids are opaque and provider-generated, so naming them costs
            // no conversation content and turns a puzzling error into an
            // obvious one. The tool's output and the caller's text stay out.
            let detail = if orphans.is_empty() {
                "the repair pass removed it".to_string()
            } else {
                format!(
                    "a tool result for {} answers no tool call in this conversation",
                    orphans.join(", ")
                )
            };

            return Err(self
                .agent
                .storage_error(format!("this turn cannot be sent: {detail}")));
        }

        let before = history.len();

        let run = self.agent.run_loop(&mut history, cx).await?;

        // The caller's turn is recorded whether or not `repair` kept it in the
        // request. `repair` never writes back, so content it drops from a
        // loaded transcript stays in storage and is dropped again next time,
        // and the caller's turn follows the same rule. Discarding it would
        // return `Ok` while the message existed nowhere.
        let mut new = vec![turn];
        new.extend(history.split_off(before));

        self.storage
            .append(new)
            .await
            .map_err(|error| self.agent.storage_error(format!("storage append: {error}")))?;

        Ok(run)
    }
}
