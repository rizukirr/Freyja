//! Message fixtures shared by unit tests in more than one module.

use crate::{InputContent, Message, Role};

/// An assistant turn requesting one tool call, with `id` as its correlation id.
pub(crate) fn call(id: &str) -> Message {
    Message::new(
        Role::Assistant,
        vec![InputContent::ToolCall {
            id: id.into(),
            name: "t".into(),
            arguments: "{}".into(),
        }],
    )
}
