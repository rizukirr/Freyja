//! Deciding what part of a transcript reaches the model.

use crate::{InputContent, Message, Role};
use std::collections::{HashMap, HashSet};

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
/// A group stays open while any call in it is unanswered, so a call and the
/// result answering it are always evicted together, whatever sits between
/// them. Interleaved calls therefore share one group rather than fragmenting.
///
/// A pinned turn arriving while a call is open therefore joins that group
/// rather than the pinned list. It is not lost: [`window_by_groups`] rescues a
/// pinned turn out of any group it drops and moves it to the front.
pub(crate) fn split(history: &[Message]) -> (Vec<&Message>, Vec<&[Message]>) {
    let mut pinned = Vec::new();
    let mut groups: Vec<&[Message]> = Vec::new();
    let mut start: Option<usize> = None;
    // Calls opened in the group currently building and not yet answered. A
    // group stays open while this is non-empty, which is the whole rule: a
    // call and the result answering it must be evicted together, so nothing
    // may close between them.
    let mut open: HashSet<&str> = HashSet::new();

    for (index, message) in history.iter().enumerate() {
        let answers_open = message.content.iter().any(|content| match content {
            InputContent::ToolResult { call_id, .. } => open.contains(call_id.as_str()),
            _ => false,
        });

        if open.is_empty() && !answers_open {
            if let Some(from) = start.take() {
                groups.push(&history[from..index]);
            }
            if matches!(message.role, Role::System | Role::Developer) {
                pinned.push(message);
                continue;
            }
        }

        for content in &message.content {
            match content {
                InputContent::ToolResult { call_id, .. } => {
                    open.remove(call_id.as_str());
                }
                InputContent::ToolCall { id, .. } => {
                    open.insert(id.as_str());
                }
                _ => {}
            }
        }
        start.get_or_insert(index);
    }
    if let Some(from) = start {
        groups.push(&history[from..]);
    }
    (pinned, groups)
}

/// Pinned turns plus the most recent `keep` turn groups.
///
/// The trimming rule [`crate::InMemoryStorage::window`] uses, published so a
/// backend of your own can apply the same one inside its
/// [`load`](crate::Storage::load) rather than reimplementing it. Everything it
/// reads is public, so writing your own was always possible; this is here to
/// save you the forty lines and to keep one rule in one place.
///
/// **A group is a message, except that an assistant turn requesting tools and
/// the results answering it are one group.** So an exchange costs two groups
/// without tools and three with them, and `keep` of 20 retains roughly seven
/// exchanges rather than twenty.
///
/// ```
/// use freyja::{InputContent, Message, Role, window_by_groups};
///
/// let history = vec![
///     Message::text(Role::System, "be brief"),
///     Message::text(Role::User, "one"),
///     Message::new(Role::Assistant, vec![InputContent::ToolCall {
///         id: "c1".into(), name: "clock".into(), arguments: "{}".into(),
///     }]),
///     Message::tool_result("c1", "noon"),
///     Message::text(Role::User, "two"),
/// ];
///
/// // Two groups here: the call and its answer are one, and "two" is the other.
/// let kept = window_by_groups(&history, 1);
///
/// // The pinned turn survives, and the tool exchange went whole.
/// assert_eq!(kept.len(), 2);
/// assert_eq!(kept[0].role, Role::System);
/// assert_eq!(kept[1].role, Role::User);
/// ```
///
/// # What it guarantees
///
/// It cuts only on group boundaries, so a call is never separated from the
/// result answering it and the output never needs repairing. You are not
/// obliged to use it: [`crate::Conversation::send`] repairs whatever `load`
/// returns, so a backend may trim by count, by age, by token budget, or not at
/// all, and a cut landing mid-pair is cleaned up before the request is built.
///
/// # What it does not
///
/// Pinned turns, meaning [`Role::System`] and [`Role::Developer`], are never
/// dropped, and they are emitted first. A `Developer` message written
/// mid-conversation therefore reaches the model at the front, framing the whole
/// conversation rather than applying from the point it was written. One written
/// inside a tool exchange stays with that exchange while it survives and moves
/// to the front once it ages out, so its position depends on the window size.
///
/// It takes the whole history, so a backend has to have loaded the whole
/// history to call it. For a store holding thousands of turns, pushing a bound
/// into the query first and calling this on the result is cheaper than fetching
/// everything, and is safe for the reason above: the repair pass covers the
/// boundary the query cut on.
pub fn window_by_groups(history: &[Message], keep: usize) -> Vec<Message> {
    let (pinned, groups) = split(history);
    let from = groups.len().saturating_sub(keep);

    // A pinned turn inside a group that ages out would otherwise leave the
    // request entirely, so an instruction meant to persist would silently stop
    // applying. Rescued turns join the pinned list in the order they appeared,
    // which means a pinned turn moves to the front once its group is dropped.
    let rescued = groups[..from]
        .iter()
        .flat_map(|group| group.iter())
        .filter(|message| matches!(message.role, Role::System | Role::Developer));

    pinned
        .into_iter()
        .chain(rescued)
        .cloned()
        .chain(
            groups[from..]
                .iter()
                .flat_map(|group| group.iter().cloned()),
        )
        .collect()
}

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
/// is never dropped. This is the stricter of two measured rules: the OpenAI
/// Chat dialect rejects any turn between them, including a pinned one, while
/// the Anthropic dialect rejects everything except a pinned turn, which it
/// hoists into a field of its own. The stricter rule is taken here because
/// this runs before a dialect is known.
///
/// A message left with no content after this is removed, so an assistant turn
/// carrying text beside a dropped call keeps its text.
///
/// vibekit: ordering ceiling. This checks one ordering property, that a result
/// follows its call. It does not validate the rest of the order, so a backend
/// that returns messages in an arbitrary sequence can still build a transcript
/// a provider rejects. No upgrade path is planned: `Storage::load` documents
/// its contract as "oldest first", and a backend breaking that is unreliable
/// in ways no repair pass can cover.
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

    /// A pinned turn written between a call and the result answering it. This
    /// is the case that made `a_window_output_needs_no_repair` pass while the
    /// property it asserts was false.
    fn pinned_inside_exchange() -> Vec<Message> {
        vec![
            Message::text(Role::User, "go"),
            call("c1"),
            Message::text(Role::Developer, "mid"),
            Message::tool_result("c1", "out"),
            Message::text(Role::Assistant, "done"),
        ]
    }

    /// A pinned turn outside any exchange, so it survives the pre-repair in
    /// `a_window_output_needs_no_repair` with its tool pair intact.
    ///
    /// `pinned_inside_exchange` cannot: its `Developer` turn sits between a
    /// call and its result, which the adjacency rule breaks, leaving three
    /// plain messages that assert nothing about pairing at any window size.
    fn pinned_outside_exchange() -> Vec<Message> {
        vec![
            Message::text(Role::User, "go"),
            Message::text(Role::Developer, "mid"),
            call("c1"),
            Message::tool_result("c1", "out"),
            Message::text(Role::Assistant, "done"),
        ]
    }

    /// Two calls open before either is answered, and the results arrive in the
    /// reverse order. Fragments into four groups before this change.
    fn interleaved() -> Vec<Message> {
        vec![
            Message::text(Role::User, "go"),
            call("c1"),
            call("c2"),
            Message::tool_result("c2", "b"),
            Message::tool_result("c1", "a"),
        ]
    }

    /// The same, with a pinned turn between the two calls.
    fn interleaved_with_pinned() -> Vec<Message> {
        vec![
            call("c1"),
            Message::text(Role::Developer, "mid"),
            call("c2"),
            Message::tool_result("c1", "a"),
            Message::tool_result("c2", "b"),
        ]
    }

    /// One message that answers `c1` and opens `c2` in the same content
    /// vector. `split` removes answered ids before inserting newly opened
    /// ones, and this is the shape that ordering exists for.
    fn answers_and_opens_in_one_message() -> Vec<Message> {
        vec![
            Message::text(Role::User, "go"),
            call("c1"),
            Message::new(
                Role::Assistant,
                vec![
                    InputContent::ToolResult {
                        call_id: "c1".into(),
                        output: "a".into(),
                    },
                    InputContent::ToolCall {
                        id: "c2".into(),
                        name: "t".into(),
                        arguments: "{}".into(),
                    },
                ],
            ),
            Message::tool_result("c2", "b"),
            Message::text(Role::Assistant, "done"),
        ]
    }

    #[test]
    fn every_message_lands_in_a_group_or_is_pinned() {
        for history in [
            tool_conversation(),
            parallel_conversation(),
            answers_and_opens_in_one_message(),
        ] {
            let (pinned, groups) = super::split(&history);
            let grouped: usize = groups.iter().map(|group| group.len()).sum();
            assert_eq!(pinned.len() + grouped, history.len());
            assert!(groups.iter().all(|group| !group.is_empty()));
        }
    }

    #[test]
    fn a_window_output_needs_no_repair() {
        // Windowing preserves whatever it is handed, so a window output needs
        // no repair only when its input needed none. Repairing the fixture
        // first is what makes the property true rather than merely narrow: a
        // transcript that is already valid on the wire stays valid at every
        // window size.
        for history in [
            tool_conversation(),
            parallel_conversation(),
            pinned_outside_exchange(),
            interleaved(),
            interleaved_with_pinned(),
            answers_and_opens_in_one_message(),
        ] {
            let mut history = history;
            repair(&mut history);

            // A fixture that loses all its tool content to the pre-repair
            // asserts nothing at any window size. `pinned_inside_exchange`
            // does exactly that under the adjacency rule, and it was added
            // because it was the case that made this test pass while the
            // property it asserts was false. Fail loudly rather than pass
            // vacuously the next time a rule empties one.
            let calls = history.iter().any(|message| {
                message
                    .content
                    .iter()
                    .any(|content| matches!(content, InputContent::ToolCall { .. }))
            });
            let results = history.iter().any(|message| {
                message
                    .content
                    .iter()
                    .any(|content| matches!(content, InputContent::ToolResult { .. }))
            });
            assert!(
                calls && results,
                "a fixture lost all its tool content to the pre-repair, so it \
                 checks nothing: {history:?}"
            );

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

    #[test]
    fn a_pinned_turn_travels_with_its_group_until_that_group_is_dropped() {
        let history = pinned_inside_exchange();

        // Not pinned by `split`: it sits inside an open exchange, so it lands
        // in that group rather than the pinned list.
        let (pinned, _groups) = super::split(&history);
        assert!(pinned.is_empty());

        // Wide enough to keep every group: it stays in place, behind the
        // messages that precede it.
        let kept = window_by_groups(&history, history.len());
        assert_ne!(kept.first().map(|m| m.role), Some(Role::Developer));
        assert!(kept.iter().any(|m| m.role == Role::Developer));

        // Narrow enough to drop its group: it is rescued to the front rather
        // than lost.
        let trimmed = window_by_groups(&history, 1);
        assert_eq!(trimmed.first().map(|m| m.role), Some(Role::Developer));
    }

    #[test]
    fn interleaved_calls_share_one_group() {
        let history = interleaved();
        let (pinned, groups) = super::split(&history);
        assert!(pinned.is_empty());
        // [User] and then everything from the first call to the last result.
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[1].len(), history.len() - 1);
    }

    #[test]
    fn a_pinned_turn_outside_an_exchange_is_still_hoisted() {
        let history = vec![
            Message::text(Role::User, "first"),
            Message::text(Role::Developer, "mid"),
            Message::text(Role::User, "second"),
        ];
        let (pinned, _groups) = super::split(&history);
        assert_eq!(pinned.len(), 1);
        assert_eq!(pinned[0].role, Role::Developer);

        let selected = window_by_groups(&history, 1);
        assert_eq!(selected.first().map(|m| m.role), Some(Role::Developer));
    }

    /// The shape that made the old implementation quadratic: one call nobody
    /// answers, then many pinned turns inside that open exchange. The old code
    /// rescanned the whole open span for each of them.
    fn one_open_call_then_pinned(n: usize) -> Vec<Message> {
        let mut history = vec![call("c1")];
        history.extend((0..n).map(|_| Message::text(Role::Developer, "d")));
        history.push(Message::tool_result("c1", "out"));
        history
    }

    /// Every call opened before any is answered, so the whole transcript is one
    /// group holding every call open at once.
    ///
    /// This is where an implementation that rescans the open span would go
    /// quadratic, and interleaved calls fragmenting into separate groups was a
    /// real defect once. Measured, this shape doubles at 2.01 against
    /// `one_open_call_then_pinned`'s 1.22, so a slide toward quadratic shows
    /// here first: that fixture is diluted by fixed costs, this one is not.
    fn many_open_calls_then_results(n: usize) -> Vec<Message> {
        let mut history: Vec<Message> = (0..n).map(|i| call(&format!("c{i}"))).collect();
        history.extend(
            (0..n)
                .rev()
                .map(|i| Message::tool_result(format!("c{i}"), "out")),
        );
        history
    }

    /// The cost at four thousand messages divided by the cost at two thousand.
    ///
    /// `split` is deterministic, so its true cost is the floor and everything
    /// above it is interference from the rest of the machine. Taking the
    /// minimum of several samples measures the function. Taking one sample
    /// measures the machine, which is how this test came to fail once on a
    /// loaded runner while the code under it was linear the whole time.
    /// Measured on an idle machine, twenty-five single-sample ratios gave a
    /// floor of 1.85 and a maximum of 2.57 against a threshold of 3.0.
    fn doubling_ratio(shape: fn(usize) -> Vec<Message>) -> f64 {
        use std::time::Instant;

        fn timed(history: &[Message]) -> std::time::Duration {
            (0..5)
                .map(|_| {
                    let start = Instant::now();
                    for _ in 0..5 {
                        let _ = super::split(history);
                    }
                    start.elapsed()
                })
                .min()
                .expect("at least one sample")
        }

        let small = shape(2_000);
        let large = shape(4_000);

        // Warm up, so the first allocation does not land inside a measurement.
        let _ = timed(&small);

        timed(&large).as_secs_f64() / timed(&small).as_secs_f64()
    }

    /// Fails only when every attempt exceeds the threshold.
    ///
    /// The two things that push this ratio up behave differently under
    /// repetition, and that is the whole point. A quadratic implementation
    /// exceeds the threshold in every attempt, so retrying never rescues it.
    /// Interference from the rest of the machine exceeds it in some, so one
    /// clean measurement is enough to know the code is linear.
    ///
    /// `timed` already takes the minimum of several samples, which defeats a
    /// stall. It does not defeat sustained saturation, which is what a machine
    /// compiling something else produces, and this is the layer that does.
    ///
    /// Doubling the input doubles linear work and quadruples quadratic work.
    /// Measured on the old implementation, doubling gave 4.10. Three sits
    /// between the two curves with room on both sides.
    fn assert_linear(shape: fn(usize) -> Vec<Message>) {
        let mut seen = Vec::new();

        for _ in 0..5 {
            let ratio = doubling_ratio(shape);
            if ratio < 3.0 {
                return;
            }
            seen.push(ratio);
        }

        // Every ratio, not just the last: five values near four read as a
        // curve, where one value tells the reader nothing about which of the
        // two causes they are looking at.
        panic!("doubling the input scaled by {seen:?} in every attempt");
    }

    #[test]
    fn split_is_linear() {
        assert_linear(one_open_call_then_pinned);
    }

    #[test]
    fn split_is_linear_with_interleaved_calls() {
        // The shape most likely to go wrong. One fixture guards the property
        // on one shape only, and the shape this test was first written for is
        // the least sensitive of the ones measured.
        assert_linear(many_open_calls_then_results);
    }
}
