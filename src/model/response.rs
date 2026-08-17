use super::{InputContent, Message, Role};
use serde_json::Value;

/// A provider-neutral generation response.
#[derive(Debug, Clone, PartialEq)]
pub struct GenerateResponse {
    /// Provider-assigned response id.
    pub id: String,
    /// Model that served the request.
    pub model: String,
    /// Why the response ended.
    pub status: ResponseStatus,
    /// Parts the model produced.
    pub content: Vec<OutputContent>,
    /// Token accounting when reported.
    pub usage: Option<Usage>,
    /// Unmodelled provider fields, preserved verbatim.
    pub provider_metadata: Option<Value>,
}

impl GenerateResponse {
    /// Concatenates every text part.
    pub fn output_text(&self) -> String {
        self.content
            .iter()
            .filter_map(|item| match item {
                OutputContent::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }
    /// Iterates requested tool calls.
    pub fn tool_calls(&self) -> impl Iterator<Item = (&str, &str, &str)> {
        self.content.iter().filter_map(|item| match item {
            OutputContent::ToolCall {
                id,
                name,
                arguments,
            } => Some((id.as_str(), name.as_str(), arguments.as_str())),
            _ => None,
        })
    }
    /// Reports whether the model requested a tool call.
    pub fn has_tool_calls(&self) -> bool {
        self.tool_calls().next().is_some()
    }
    /// Converts this response into the assistant turn for a follow-up request.
    pub fn to_message(&self) -> Message {
        let content = self
            .content
            .iter()
            .map(|item| match item {
                OutputContent::Text(text) | OutputContent::Refusal(text) => {
                    InputContent::Text(text.clone())
                }
                OutputContent::ToolCall {
                    id,
                    name,
                    arguments,
                } => InputContent::ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                },
                OutputContent::Reasoning { data } => InputContent::Reasoning { data: data.clone() },
            })
            .collect();
        Message {
            role: Role::Assistant,
            content,
        }
    }
}

/// Why a response ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseStatus {
    /// Finished normally.
    Completed,
    /// Cut short.
    Incomplete,
    /// Waiting for tool results.
    RequiresAction,
    /// The provider failed.
    Failed,
    /// An unmodelled status.
    Other(String),
}

/// One part of what the model produced.
#[derive(Debug, Clone, PartialEq)]
pub enum OutputContent {
    /// Generated text.
    Text(String),
    /// A refusal.
    Refusal(String),
    /// A requested tool call.
    ToolCall {
        /// Correlation id.
        id: String,
        /// Tool name.
        name: String,
        /// Raw JSON arguments.
        arguments: String,
    },
    /// Opaque provider state that must be replayed unchanged.
    Reasoning {
        /// Provider representation.
        data: Value,
    },
}

/// Token accounting for one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    /// Prompt tokens.
    pub input_tokens: u64,
    /// Generated tokens.
    pub output_tokens: u64,
    /// Total billed tokens.
    pub total_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn response_converts_into_an_assistant_turn() {
        let response = GenerateResponse {
            id: "resp_1".into(),
            model: "test".into(),
            status: ResponseStatus::RequiresAction,
            content: vec![
                OutputContent::Text("thinking".into()),
                OutputContent::ToolCall {
                    id: "call_1".into(),
                    name: "add".into(),
                    arguments: "{\"a\":1,\"b\":2}".into(),
                },
            ],
            usage: None,
            provider_metadata: None,
        };
        assert!(response.has_tool_calls());
        assert_eq!(
            response.tool_calls().collect::<Vec<_>>(),
            vec![("call_1", "add", "{\"a\":1,\"b\":2}")]
        );
        assert_eq!(response.to_message().role, Role::Assistant);
    }
}
