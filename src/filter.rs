//! Deciding what part of a transcript reaches the model.

use crate::{Context, InputContent, Message, Role};
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// What a policy failed with.
///
/// A policy has no endpoint, so it does not produce a [`crate::Error`]: every
/// variant of that type carries one. [`crate::Agent`] wraps this instead.
pub type FilterError = Box<dyn std::error::Error + Send + Sync>;

/// The future returned by [`Filter::select`].
pub type FilterFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<Message>, FilterError>> + Send + 'a>>;

/// Decides what part of a transcript goes on the wire.
///
/// The caller's transcript is never modified. A policy is handed everything
/// and returns the messages to send, so removing the policy restores the whole
/// conversation.
///
/// A policy may drop, reorder, compress or prepend. It may return a transcript
/// in which a tool result no longer has its call: [`crate::Agent`] repairs the
/// output before sending, so a policy cannot produce a request the provider
/// rejects for a broken pairing.
///
/// Boxed rather than `async fn` in the trait, for the reason
/// [`crate::ToolFuture`] gives: `async fn` in traits is stable but not
/// `dyn`-compatible, and `Agent` stores trait objects.
pub trait Filter: Send + Sync {
    /// Returns the messages to send this turn.
    fn select<'a>(&'a self, history: &'a [Message], cx: &'a Context) -> FilterFuture<'a>;
}

/// Any suitable closure is a policy, so an ordinary filter never writes a
/// boxed future by hand.
impl<F> Filter for F
where
    F: Fn(&[Message]) -> Vec<Message> + Send + Sync,
{
    fn select<'a>(&'a self, history: &'a [Message], _cx: &'a Context) -> FilterFuture<'a> {
        let selected = self(history);
        Box::pin(async move { Ok(selected) })
    }
}

/// Splits a transcript into the spans that may be evicted as one unit.
///
/// A tool result may only answer a call that already happened, so an assistant
/// turn and every result answering it are inseparable.
///
/// Pinned turns are not groups: they are returned separately and never aged
/// out, because most vendors hoist them into a field of their own, so dropping
/// one silently changes the model's instructions rather than shortening the
/// conversation.
fn split(history: &[Message]) -> (Vec<&Message>, Vec<&[Message]>) {
    let mut pinned = Vec::new();
    let mut groups: Vec<&[Message]> = Vec::new();
    let mut start: Option<usize> = None;

    for (index, message) in history.iter().enumerate() {
        if matches!(message.role, Role::System | Role::Developer) {
            if let Some(from) = start.take() {
                groups.push(&history[from..index]);
            }
            pinned.push(message);
            continue;
        }
        let continues = start.is_some_and(|from| answers_open_call(&history[from..index], message));
        if !continues && let Some(from) = start.take() {
            groups.push(&history[from..index]);
        }
        start.get_or_insert(index);
    }
    if let Some(from) = start {
        groups.push(&history[from..]);
    }
    (pinned, groups)
}

/// Whether `message` answers a call made earlier in `span`.
fn answers_open_call(span: &[Message], message: &Message) -> bool {
    let answered: HashSet<&str> = message
        .content
        .iter()
        .filter_map(|content| match content {
            InputContent::ToolResult { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        })
        .collect();
    if answered.is_empty() {
        return false;
    }
    span.iter()
        .flat_map(|earlier| earlier.content.iter())
        .any(|content| match content {
            InputContent::ToolCall { id, .. } => answered.contains(id.as_str()),
            _ => false,
        })
}

/// Drops every tool result whose originating call is absent.
///
/// A window cut at an arbitrary message boundary is the common case and it
/// looks correct. The result is a transcript every provider rejects, with an
/// error that mentions nothing about context length. Freyja's own Gemini
/// builder rejects it before the network, in
/// `rejects_a_result_with_no_matching_call`.
///
/// vibekit: ordering ceiling. This only drops a result whose call is absent,
/// it does not check that a result still follows its call in the trimmed
/// transcript. A policy that reorders messages can produce a result ahead of
/// its call, which the Gemini builder rejects in
/// `rejects_a_result_that_answers_a_later_call`. Upgrade path: track each
/// call's index while scanning and drop a result whose index is not after it,
/// alongside the existing absent-call check.
fn repair(messages: &mut Vec<Message>) {
    let calls: HashSet<String> = messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|content| match content {
            InputContent::ToolCall { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();

    for message in messages.iter_mut() {
        message.content.retain(|content| match content {
            InputContent::ToolResult { call_id, .. } => calls.contains(call_id),
            _ => true,
        });
    }
    messages.retain(|message| !message.content.is_empty());
}

/// Runs every policy in order, each on the previous one's output, then repairs
/// the result.
///
/// The caller's transcript is copied in, so nothing here can shorten it.
pub(crate) async fn apply(
    policies: &[Arc<dyn Filter>],
    history: &[Message],
    cx: &Context,
) -> Result<Vec<Message>, FilterError> {
    let mut chosen = history.to_vec();
    for policy in policies {
        chosen = policy.select(&chosen, cx).await?;
    }
    repair(&mut chosen);
    Ok(chosen)
}

/// Keeps the pinned turns and the most recent turn groups.
///
/// Counts groups rather than tokens, so it needs no tokenizer. It bounds
/// growth rather than guaranteeing a fit: a token-aware policy needs an
/// estimate calibrated from [`crate::Usage`], which is separate work.
///
/// Cuts only on group boundaries, so its output is already valid before the
/// repair pass sees it.
pub struct Window {
    groups: usize,
}

impl Window {
    /// Keeps the most recent `groups` turn groups, plus every pinned turn.
    pub fn groups(groups: usize) -> Self {
        Self { groups }
    }
}

impl Filter for Window {
    fn select<'a>(&'a self, history: &'a [Message], _cx: &'a Context) -> FilterFuture<'a> {
        Box::pin(async move {
            let (pinned, groups) = split(history);
            let from = groups.len().saturating_sub(self.groups);
            let selected = pinned
                .into_iter()
                .cloned()
                .chain(
                    groups[from..]
                        .iter()
                        .flat_map(|group| group.iter().cloned()),
                )
                .collect();
            Ok(selected)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Filter, Window, apply, repair};
    use crate::{Context, InputContent, Message, Role};
    use std::sync::Arc;

    fn transcript() -> Vec<Message> {
        vec![
            Message::text(Role::System, "pinned"),
            Message::text(Role::User, "first"),
            Message::text(Role::User, "second"),
        ]
    }

    #[tokio::test]
    async fn a_closure_is_a_policy() {
        let policy = |history: &[Message]| history.iter().skip(1).cloned().collect::<Vec<_>>();
        let history = transcript();
        let selected = policy.select(&history, &Context::new()).await.unwrap();
        assert_eq!(selected.len(), history.len() - 1);
        assert_eq!(history, transcript());
    }

    fn call(id: &str) -> Message {
        Message::new(
            Role::Assistant,
            vec![InputContent::ToolCall {
                id: id.into(),
                name: "t".into(),
                arguments: "{}".into(),
            }],
        )
    }

    #[test]
    fn drops_a_result_whose_call_is_gone() {
        let mut messages = vec![Message::tool_result("call_1", "out")];
        repair(&mut messages);
        assert!(messages.is_empty());
    }

    #[test]
    fn keeps_a_result_whose_call_is_present() {
        let mut messages = vec![call("call_1"), Message::tool_result("call_1", "out")];
        let before = messages.clone();
        repair(&mut messages);
        assert_eq!(messages, before);
    }

    #[test]
    fn drops_every_result_of_a_removed_parallel_turn() {
        let mut messages: Vec<Message> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|id| Message::tool_result(*id, "out"))
            .collect();
        repair(&mut messages);
        assert!(messages.is_empty());
    }

    #[test]
    fn leaves_a_transcript_with_no_tools_alone() {
        let mut messages = transcript();
        let before = messages.clone();
        repair(&mut messages);
        assert_eq!(messages, before);
    }

    #[tokio::test]
    async fn a_policy_receives_the_previous_one_s_output() {
        let drop_first: Arc<dyn Filter> =
            Arc::new(|history: &[Message]| history.iter().skip(1).cloned().collect::<Vec<_>>());
        let policies = vec![drop_first.clone(), drop_first];
        let history = transcript();
        let selected = apply(&policies, &history, &Context::new()).await.unwrap();
        assert_eq!(selected.len(), history.len() - 2);
        assert_eq!(selected.first(), history.last());
    }

    #[tokio::test]
    async fn a_naive_policy_cannot_orphan_a_result() {
        let history = vec![
            Message::text(Role::User, "go"),
            call("call_1"),
            Message::tool_result("call_1", "out"),
        ];
        let cut_in_half: Arc<dyn Filter> =
            Arc::new(|history: &[Message]| history.iter().skip(2).cloned().collect::<Vec<_>>());
        let selected = apply(&[cut_in_half], &history, &Context::new())
            .await
            .unwrap();
        assert!(selected.is_empty());
        assert_eq!(history.len(), 3);
    }

    #[tokio::test]
    async fn a_failing_policy_surfaces_its_error() {
        struct Broken;
        impl Filter for Broken {
            fn select<'a>(
                &'a self,
                _h: &'a [Message],
                _cx: &'a Context,
            ) -> super::FilterFuture<'a> {
                Box::pin(async { Err(std::io::Error::other("unreachable").into()) })
            }
        }
        let policies: Vec<Arc<dyn Filter>> = vec![Arc::new(Broken)];
        assert!(
            apply(&policies, &transcript(), &Context::new())
                .await
                .is_err()
        );
    }

    fn tool_conversation() -> Vec<Message> {
        vec![
            Message::text(Role::System, "pinned"),
            Message::text(Role::User, "weather?"),
            call("call_1"),
            Message::tool_result("call_1", "raining"),
            Message::text(Role::Assistant, "it is raining"),
            Message::text(Role::User, "and tomorrow?"),
            call("call_2"),
            Message::tool_result("call_2", "sunny"),
            Message::text(Role::Assistant, "sunny"),
        ]
    }

    fn parallel_conversation() -> Vec<Message> {
        let ids = ["a", "b", "c", "d", "e"];
        let calls = Message::new(
            Role::Assistant,
            ids.iter()
                .map(|id| InputContent::ToolCall {
                    id: (*id).into(),
                    name: "t".into(),
                    arguments: "{}".into(),
                })
                .collect::<Vec<_>>(),
        );
        let mut history = vec![Message::text(Role::User, "go"), calls];
        history.extend(ids.iter().map(|id| Message::tool_result(*id, "out")));
        history.push(Message::text(Role::User, "again"));
        history
    }

    async fn window(keep: usize, history: &[Message]) -> Vec<Message> {
        Window::groups(keep)
            .select(history, &Context::new())
            .await
            .unwrap()
    }

    #[test]
    fn every_message_lands_in_a_group_or_is_pinned() {
        for history in [tool_conversation(), parallel_conversation()] {
            let (pinned, groups) = super::split(&history);
            let grouped: usize = groups.iter().map(|group| group.len()).sum();
            assert_eq!(pinned.len() + grouped, history.len());
            assert!(groups.iter().all(|group| !group.is_empty()));
        }
    }

    #[tokio::test]
    async fn a_window_output_needs_no_repair() {
        for history in [tool_conversation(), parallel_conversation()] {
            for keep in 0..=history.len() {
                let mut selected = window(keep, &history).await;
                let before = selected.clone();
                repair(&mut selected);
                assert_eq!(selected, before);
            }
        }
    }

    #[tokio::test]
    async fn a_window_keeps_pinned_turns_however_old() {
        let selected = window(0, &tool_conversation()).await;
        assert_eq!(selected, vec![Message::text(Role::System, "pinned")]);
    }

    #[tokio::test]
    async fn a_window_leaves_the_caller_transcript_alone() {
        let history = tool_conversation();
        let _ = window(1, &history).await;
        assert_eq!(history, tool_conversation());
    }

    #[tokio::test]
    async fn a_window_wider_than_the_transcript_sends_all_of_it() {
        let history = tool_conversation();
        let selected = window(history.len(), &history).await;
        assert_eq!(selected, history);
    }
}
