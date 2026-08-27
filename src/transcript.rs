//! Deciding what part of a transcript reaches the model.

use crate::{InputContent, Message, Role};
use std::collections::HashSet;

/// Splits a transcript into the spans that must be evicted as one unit.
///
/// A tool result may only answer a call that already happened, so an assistant
/// turn and every result answering it are inseparable. Cutting between them
/// produces a transcript every provider rejects, with an error mentioning
/// nothing about trimming.
///
/// Pinned turns, meaning `Role::System` and `Role::Developer`, are returned
/// separately and never aged out. Everything else is returned as groups, in
/// order.
///
/// Pinned turns are returned first and separately, so a caller that
/// reassembles them ahead of the groups changes their position: a `Developer`
/// message written mid-conversation reaches the model at the front. That
/// matches how most vendors treat system instructions, which they hoist into a
/// field of their own, and it means an instruction meant to apply from one
/// point onward will instead frame the whole conversation.
///
/// Public so a storage backend written elsewhere can trim on safe boundaries
/// without reimplementing this.
pub fn split(history: &[Message]) -> (Vec<&Message>, Vec<&[Message]>) {
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

/// Pinned turns plus the most recent `keep` groups.
///
/// Cuts only on group boundaries, so its output never needs repairing.
///
/// A group is a message, except that an assistant turn requesting tools and
/// the results answering it are one group. So an exchange costs two groups
/// without tools and three with them: `keep` of 20 retains roughly seven
/// exchanges, not twenty.
///
/// Pinned turns are emitted first, so this reorders a `System` or `Developer`
/// message written mid-conversation to the front. See [`split`].
pub fn window_by_groups(history: &[Message], keep: usize) -> Vec<Message> {
    let (pinned, groups) = split(history);
    let from = groups.len().saturating_sub(keep);
    pinned
        .into_iter()
        .cloned()
        .chain(
            groups[from..]
                .iter()
                .flat_map(|group| group.iter().cloned()),
        )
        .collect()
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
/// transcript. A backend that reorders messages can produce a result ahead of
/// its call, which the Gemini builder rejects in
/// `rejects_a_result_that_answers_a_later_call`. Upgrade path: track each
/// call's index while scanning and drop a result whose index is not after it,
/// alongside the existing absent-call check.
pub(crate) fn repair(messages: &mut Vec<Message>) {
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

#[cfg(test)]
mod tests {
    use super::{repair, window_by_groups};
    use crate::{InputContent, Message, Role};

    fn transcript() -> Vec<Message> {
        vec![
            Message::text(Role::System, "pinned"),
            Message::text(Role::User, "first"),
            Message::text(Role::User, "second"),
        ]
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

    #[test]
    fn every_message_lands_in_a_group_or_is_pinned() {
        for history in [tool_conversation(), parallel_conversation()] {
            let (pinned, groups) = super::split(&history);
            let grouped: usize = groups.iter().map(|group| group.len()).sum();
            assert_eq!(pinned.len() + grouped, history.len());
            assert!(groups.iter().all(|group| !group.is_empty()));
        }
    }

    #[test]
    fn a_window_output_needs_no_repair() {
        for history in [tool_conversation(), parallel_conversation()] {
            for keep in 0..=history.len() {
                let mut selected = window_by_groups(&history, keep);
                let before = selected.clone();
                repair(&mut selected);
                assert_eq!(selected, before);
            }
        }
    }

    #[test]
    fn a_window_keeps_pinned_turns_however_old() {
        let selected = window_by_groups(&tool_conversation(), 0);
        assert_eq!(selected, vec![Message::text(Role::System, "pinned")]);
    }

    #[test]
    fn a_window_leaves_the_caller_transcript_alone() {
        let history = tool_conversation();
        let _ = window_by_groups(&history, 1);
        assert_eq!(history, tool_conversation());
    }

    #[test]
    fn a_window_wider_than_the_transcript_sends_all_of_it() {
        let history = tool_conversation();
        let selected = window_by_groups(&history, history.len());
        assert_eq!(selected, history);
    }
}
