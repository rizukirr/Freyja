use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A single conversation turn.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    /// Who produced this turn.
    pub role: Role,
    /// The parts making up this turn.
    pub content: Vec<InputContent>,
}

impl Message {
    /// Creates a message from a role and raw parts.
    pub fn new(role: Role, content: impl Into<Vec<InputContent>>) -> Self {
        Self {
            role,
            content: content.into(),
        }
    }
    /// Creates a single-part text message.
    pub fn text(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            content: vec![InputContent::Text(text.into())],
        }
    }
    /// Creates the turn that carries a tool result back to the model.
    pub fn tool_result(call_id: impl Into<String>, output: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: vec![InputContent::ToolResult {
                call_id: call_id.into(),
                output: output.into(),
            }],
        }
    }
}

/// Who produced a conversation turn.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Instructions that frame the whole conversation.
    System,
    /// Application developer instructions.
    Developer,
    /// The end user.
    User,
    /// The model.
    Assistant,
    /// A tool result.
    Tool,
}

/// One part of a conversation turn sent to the model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum InputContent {
    /// Plain text.
    Text(String),
    /// An image URL or data URI.
    ImageUrl(String),
    /// A previously made tool call on an assistant turn.
    ToolCall {
        /// Correlation id.
        id: String,
        /// Tool name.
        name: String,
        /// Raw JSON arguments.
        arguments: String,
    },
    /// The result of running a tool on a tool turn.
    ToolResult {
        /// The answered call id.
        call_id: String,
        /// JSON or text output.
        output: String,
    },
    /// Opaque provider state that must be replayed unchanged.
    Reasoning {
        /// Provider representation.
        data: Value,
    },
}

/// How much internal reasoning the model should spend before answering.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    /// No internal reasoning.
    None,
    /// Low effort.
    Low,
    /// Medium effort.
    Medium,
    /// High effort.
    High,
    /// Above high effort.
    Xhigh,
    /// The highest effort the provider supports.
    Max,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tool_result_builds_a_tool_turn() {
        let message = Message::tool_result("call_1", "3");
        assert_eq!(message.role, Role::Tool);
        assert_eq!(
            message.content,
            vec![InputContent::ToolResult {
                call_id: "call_1".into(),
                output: "3".into()
            }]
        );
    }
}
