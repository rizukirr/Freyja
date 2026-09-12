//! A token count estimated from byte length, with no tokenizer.

use crate::{InputContent, Message, TokenCounter};

/// Bytes per token. English prose sits near four, code and JSON below it, so
/// this undercounts exactly the content a token budget exists to bound. That
/// is why the budget wants a margin and why [`TokenCounter`] is a trait.
const BYTES_PER_TOKEN: usize = 4;

/// The role marker and delimiters every provider wraps a turn in. Without it a
/// transcript of many short turns undercounts badly.
const PER_MESSAGE_OVERHEAD: usize = 4;

/// Approximates a token count from UTF-8 byte length, with no tokenizer, no
/// dependency and no network call.
///
/// An estimate, not a bound. It undercounts code, JSON and CJK, so size a
/// budget below the model's real limit rather than at it. When the margin has
/// to be tight, pass a real tokenizer as a closure instead.
///
/// ```
/// use freyja::{HeuristicCounter, Message, Role, TokenCounter};
///
/// let counter = HeuristicCounter;
/// let short = counter.count(&Message::text(Role::User, "hi"));
/// let long = counter.count(&Message::text(Role::User, "hi".repeat(400)));
///
/// assert!(long > short);
/// ```
pub struct HeuristicCounter;

impl TokenCounter for HeuristicCounter {
    fn count(&self, message: &Message) -> usize {
        // Exhaustive with no wildcard arm, and `InputContent` is not
        // `non_exhaustive`, so a new variant is a compile error here rather
        // than a silent zero.
        let bytes: usize = message
            .content
            .iter()
            .map(|part| match part {
                InputContent::Text(text) => text.len(),
                InputContent::ImageUrl(url) => url.len(),
                InputContent::ToolCall {
                    id,
                    name,
                    arguments,
                } => id.len() + name.len() + arguments.len(),
                InputContent::ToolResult { call_id, output } => call_id.len() + output.len(),
                InputContent::Reasoning { data } => data.to_string().len(),
            })
            .sum();

        bytes / BYTES_PER_TOKEN + PER_MESSAGE_OVERHEAD
    }
}

#[cfg(test)]
mod tests {
    use super::{HeuristicCounter, PER_MESSAGE_OVERHEAD};
    use crate::{InputContent, Message, Role, TokenCounter};

    #[test]
    fn heuristic_counter_counts_every_variant_above_overhead() {
        let messages = vec![
            Message::new(Role::User, vec![InputContent::Text("hello there".into())]),
            Message::new(
                Role::User,
                vec![InputContent::ImageUrl("https://example.com/a.png".into())],
            ),
            Message::new(
                Role::Assistant,
                vec![InputContent::ToolCall {
                    id: "call_1".into(),
                    name: "search".into(),
                    arguments: "{\"q\":\"x\"}".into(),
                }],
            ),
            Message::new(
                Role::User,
                vec![InputContent::ToolResult {
                    call_id: "call_1".into(),
                    output: "result text".into(),
                }],
            ),
            Message::new(
                Role::Assistant,
                vec![InputContent::Reasoning {
                    data: serde_json::json!({"steps": ["a", "b"]}),
                }],
            ),
        ];

        let counter = HeuristicCounter;
        for message in &messages {
            assert!(counter.count(message) > PER_MESSAGE_OVERHEAD);
        }
    }
}
