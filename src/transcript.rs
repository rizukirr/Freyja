//! Deciding what part of a transcript reaches the model.

use crate::{InputContent, Message};
use std::collections::{HashMap, HashSet};

/// Drops a tool result whose call is absent or does not precede it, and a tool
/// call whose result is absent or does not follow it.
///
/// Both directions are rejected on the wire. A result answering nothing fails
/// everywhere, and Anthropic refuses a `tool_use` block with no answering
/// `tool_result`. A backend trimming to the last few messages produces the
/// second constantly, since cutting right after a call turn is the ordinary
/// case.
///
/// A result is kept only when the call it answers appears strictly earlier, so
/// a transcript that arrives with a result ahead of its call loses both
/// messages. The call goes with it because a call left unanswered is rejected
/// anyway. Where an id appears more than once, the first occurrence in each
/// direction is the one compared.
///
/// The one call site is [`crate::Conversation::send`], applied to what
/// [`crate::Storage::load`] returned, and nothing in this crate can produce a
/// result ahead of its call. The order comes from a backend, which is why this
/// is checked here at all: `Storage` is a boundary this crate does not review.
///
/// A call and the results answering it must not be separated either. Anything
/// that is not a tool result, arriving between a call and its last open
/// result, drops both halves of the pair, though the intervening turn itself
/// is never dropped. This is the strictest of three measured rules, taken
/// because this runs before a dialect is known:
///
/// - **OpenAI Chat** rejects any turn between them, including a pinned one.
/// - **Anthropic** rejects everything except a pinned turn, which never
///   arrives between them anyway because the dialect hoists it into a field of
///   its own.
/// - **Gemini** accepts a turn between them. Measured against the live
///   endpoint with a `user_input` sitting between `function_call` and
///   `function_result` in the request body, and answered normally.
///
/// So this rule is stricter than Gemini needs and exactly as strict as OpenAI
/// Chat needs. Being stricter costs a dropped pair that one endpoint would
/// have accepted; being looser costs a request every other endpoint rejects
/// with an error mentioning nothing about trimming.
///
/// A message left with no content after this is removed, so an assistant turn
/// carrying text beside a dropped call keeps its text.
///
/// This checks one ordering property, that a result follows its call. It does
/// not validate the rest of the order, so a backend that returns messages in
/// an arbitrary sequence can still build a transcript a provider rejects.
/// [`crate::Storage::load`] documents its contract as "oldest first", and a
/// backend breaking that is beyond what a repair pass can cover.
pub(crate) fn repair(messages: &mut Vec<Message>) {
    // Each id mapped to the index of the first message carrying it in that
    // direction. The index is what makes the check an ordering check: a set
    // can only answer whether the partner exists, not whether it came first.
    let mut calls: HashMap<String, usize> = HashMap::new();
    let mut results: HashMap<String, usize> = HashMap::new();

    // A call and the results answering it must not be separated. Measured, the
    // OpenAI Chat dialect rejects any turn between them, including a pinned
    // one, and the Anthropic dialect rejects everything except a pinned turn,
    // which it hoists into a field of its own. This runs before a dialect is
    // known, so it takes the stricter rule and treats anything that is not a
    // tool result as breaking the pair.
    let mut open: HashSet<String> = HashSet::new();
    let mut broken: HashSet<String> = HashSet::new();

    for (index, message) in messages.iter().enumerate() {
        let only_results = !message.content.is_empty()
            && message
                .content
                .iter()
                .all(|content| matches!(content, InputContent::ToolResult { .. }));

        if !open.is_empty() && !only_results {
            broken.extend(open.drain());
        }

        for content in &message.content {
            match content {
                InputContent::ToolCall { id, .. } => {
                    calls.entry(id.clone()).or_insert(index);
                    open.insert(id.clone());
                }
                InputContent::ToolResult { call_id, .. } => {
                    results.entry(call_id.clone()).or_insert(index);
                    open.remove(call_id);
                }
                _ => {}
            }
        }
    }

    for (index, message) in messages.iter_mut().enumerate() {
        message.content.retain(|content| match content {
            InputContent::ToolResult { call_id, .. } => {
                !broken.contains(call_id) && calls.get(call_id).is_some_and(|call| *call < index)
            }
            InputContent::ToolCall { id, .. } => {
                !broken.contains(id) && results.get(id).is_some_and(|result| *result > index)
            }
            _ => true,
        });
    }
    messages.retain(|message| !message.content.is_empty());
}

#[cfg(test)]
mod tests {
    use super::repair;
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
    fn drops_a_result_that_precedes_its_call() {
        // The Gemini builder rejects this order in
        // rejects_a_result_that_answers_a_later_call, a round trip spent to
        // learn what is decidable here. Both messages go: dropping the result
        // leaves the call unanswered, which is rejected on its own.
        let mut messages = vec![Message::tool_result("call_1", "out"), call("call_1")];
        repair(&mut messages);
        assert!(messages.is_empty());
    }

    #[test]
    fn keeps_an_ordered_pair_beside_an_inverted_one() {
        let ordered = vec![call("call_1"), Message::tool_result("call_1", "out")];
        let mut messages = ordered.clone();
        messages.push(Message::tool_result("call_2", "out"));
        messages.push(call("call_2"));
        repair(&mut messages);
        assert_eq!(messages, ordered);
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

    #[test]
    fn drops_a_call_whose_result_is_gone() {
        let mut messages = vec![Message::text(Role::User, "go"), call("c1")];
        repair(&mut messages);
        assert_eq!(messages, vec![Message::text(Role::User, "go")]);
    }

    #[test]
    fn dropping_a_call_keeps_the_text_beside_it() {
        let mut messages = vec![Message::new(
            Role::Assistant,
            vec![
                InputContent::Text("thinking".into()),
                InputContent::ToolCall {
                    id: "c1".into(),
                    name: "t".into(),
                    arguments: "{}".into(),
                },
            ],
        )];
        repair(&mut messages);
        assert_eq!(messages, vec![Message::text(Role::Assistant, "thinking")]);
    }

    #[test]
    fn a_matched_call_and_result_survive() {
        let mut messages = vec![call("c1"), Message::tool_result("c1", "out")];
        let before = messages.clone();
        repair(&mut messages);
        assert_eq!(messages, before);
    }

    #[test]
    fn drops_a_pair_separated_by_a_user_turn() {
        let mut messages = vec![
            call("c1"),
            Message::text(Role::User, "mid"),
            Message::tool_result("c1", "out"),
        ];
        repair(&mut messages);
        assert_eq!(messages, vec![Message::text(Role::User, "mid")]);
    }

    #[test]
    fn drops_a_pair_separated_by_a_developer_turn() {
        let mut messages = vec![
            call("c1"),
            Message::text(Role::Developer, "mid"),
            Message::tool_result("c1", "out"),
        ];
        repair(&mut messages);
        assert_eq!(messages, vec![Message::text(Role::Developer, "mid")]);
    }

    #[test]
    fn drops_a_pair_separated_by_an_assistant_turn() {
        let mut messages = vec![
            call("c1"),
            Message::text(Role::Assistant, "mid"),
            Message::tool_result("c1", "out"),
        ];
        repair(&mut messages);
        assert_eq!(messages, vec![Message::text(Role::Assistant, "mid")]);
    }

    #[test]
    fn a_parallel_exchange_survives_untouched() {
        let calls = Message::new(
            Role::Assistant,
            vec![
                InputContent::ToolCall {
                    id: "a".into(),
                    name: "t".into(),
                    arguments: "{}".into(),
                },
                InputContent::ToolCall {
                    id: "b".into(),
                    name: "t".into(),
                    arguments: "{}".into(),
                },
            ],
        );
        let mut messages = vec![
            calls,
            Message::tool_result("a", "out"),
            Message::tool_result("b", "out"),
        ];
        let before = messages.clone();
        repair(&mut messages);
        assert_eq!(messages, before);
    }
}
