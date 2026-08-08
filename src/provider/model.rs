//! Provider-neutral request, response, and error types.
//!
//! Nothing in this module is vendor-specific. Each provider owns its own wire
//! format and converts to and from these types, so application code never has
//! to name an OpenAI or Gemini struct.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::sync::Arc;

/// A provider-neutral generation request.
///
/// Every field is optional except [`messages`](Self::messages). A field left as
/// `None` means "use the provider's own default". Freyja does not invent values,
/// because a value that looks harmless on one provider may be rejected by another.
///
/// Build one with the chainable setters:
///
/// ```
/// use freyja::{GenerateRequest, Message, Role};
///
/// let request = GenerateRequest::new()
///     .message(Message::text(Role::System, "Be concise."))
///     .message(Message::text(Role::User, "Hello"))
///     .max_tokens(256);
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GenerateRequest {
    /// Model identifier. `None` uses the provider's default model.
    pub model: Option<String>,
    /// Conversation turns, in order.
    pub messages: Vec<Message>,
    /// Upper bound on tokens the model may generate.
    pub max_tokens: Option<u32>,
    /// Sampling temperature.
    pub temperature: Option<f32>,
    /// Nucleus sampling cutoff.
    pub top_p: Option<f32>,
    /// How much internal reasoning the model should spend before answering.
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Shape the response must take.
    pub response_format: Option<ResponseFormat>,
    /// Functions the model is allowed to call.
    pub tools: Vec<ToolDefinition>,
    /// Whether the model must call a tool, and which one.
    pub tool_choice: Option<ToolChoice>,
    /// Server-side conversation continuation token from a previous response.
    pub previous_response_id: Option<String>,
    /// Free-form metadata forwarded to the provider (labels, trace ids, …).
    pub metadata: Option<Value>,
}

impl GenerateRequest {
    /// Creates an empty request.
    ///
    /// No fields are populated. In particular this does *not* set a default
    /// `tool_choice` or `reasoning_effort`: those are capabilities not every
    /// provider can express, and forcing them here would make an otherwise
    /// portable request fail on providers that reject them.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the model identifier.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Appends a single message to the conversation.
    pub fn message(mut self, message: Message) -> Self {
        self.messages.push(message);
        self
    }

    /// Replaces the whole conversation.
    pub fn messages(mut self, messages: impl Into<Vec<Message>>) -> Self {
        self.messages = messages.into();
        self
    }

    /// Appends several messages to the conversation.
    pub fn extend_messages(mut self, messages: impl IntoIterator<Item = Message>) -> Self {
        self.messages.extend(messages);
        self
    }

    /// Sets the output token cap.
    pub fn max_tokens(mut self, value: u32) -> Self {
        self.max_tokens = Some(value);
        self
    }

    /// Sets the sampling temperature.
    pub fn temperature(mut self, value: f32) -> Self {
        self.temperature = Some(value);
        self
    }

    /// Sets the nucleus sampling cutoff.
    pub fn top_p(mut self, value: f32) -> Self {
        self.top_p = Some(value);
        self
    }

    /// Declares the tools the model may call.
    pub fn tools(mut self, tools: impl Into<Vec<ToolDefinition>>) -> Self {
        self.tools = tools.into();
        self
    }

    /// Constrains which tool the model may call.
    pub fn tool_choice(mut self, choice: ToolChoice) -> Self {
        self.tool_choice = Some(choice);
        self
    }

    /// Sets how much internal reasoning the model should spend.
    pub fn reasoning_effort(mut self, effort: ReasoningEffort) -> Self {
        self.reasoning_effort = Some(effort);
        self
    }

    /// Constrains the shape of the response.
    pub fn response_format(mut self, format: ResponseFormat) -> Self {
        self.response_format = Some(format);
        self
    }

    /// Continues a server-side conversation from a previous response id.
    pub fn previous_response_id(mut self, id: impl Into<String>) -> Self {
        self.previous_response_id = Some(id.into());
        self
    }

    /// Attaches provider metadata (labels, trace ids, …).
    pub fn metadata(mut self, metadata: Value) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

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

    /// Creates the turn that carries a tool's result back to the model.
    ///
    /// `call_id` must be the id from the [`OutputContent::ToolCall`] being
    /// answered, and `output` is the tool's result, JSON or plain text.
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
    /// Instructions from the application developer, ranked above the user.
    Developer,
    /// The end user.
    User,
    /// The model.
    Assistant,
    /// The result of executing a tool the model asked for.
    Tool,
}

/// One part of a conversation turn being sent *to* the model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum InputContent {
    /// Plain text.
    Text(String),
    /// An image, referenced by URL or data URI.
    ImageUrl(String),
    /// A tool call the model previously made, echoed back so the model can see
    /// its own action in the transcript. Belongs on a [`Role::Assistant`] turn.
    ToolCall {
        /// Correlation id, matched by [`InputContent::ToolResult::call_id`].
        id: String,
        /// Name of the tool the model wants to call.
        name: String,
        /// Arguments, as a raw JSON string.
        arguments: String,
    },
    /// The result of running a tool. Belongs on a [`Role::Tool`] turn.
    ToolResult {
        /// The id of the tool call being answered.
        call_id: String,
        /// The tool's output, JSON or plain text.
        output: String,
    },
    /// Opaque provider state that must be replayed verbatim.
    ///
    /// Reasoning models emit signed internal state (Gemini thought signatures,
    /// Anthropic thinking blocks, OpenAI reasoning items) and reject a follow-up
    /// request that drops it or rebuilds an equivalent by hand. Freyja cannot
    /// model the contents, so it carries the blob through untouched.
    ///
    /// You never construct these. They arrive as [`OutputContent::Reasoning`]
    /// and reach the next request through [`GenerateResponse::to_message`].
    /// Preserve their position within `content`, since providers care about the
    /// order relative to the tool calls they precede.
    Reasoning {
        /// The provider's own representation, replayed as received.
        data: Value,
    },
}

/// How much internal reasoning the model should spend before answering.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    /// No internal reasoning.
    None,
    /// The smallest amount the provider supports.
    Minimal,
    /// Low.
    Low,
    /// Medium.
    Medium,
    /// High.
    High,
    /// Above high.
    Xhigh,
    /// The largest amount the provider supports.
    Max,
}

/// The shape the model's response must take.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ResponseFormat {
    /// Free-form text.
    Text,
    /// Any valid JSON object.
    JsonObject,
    /// JSON conforming to a specific schema.
    JsonSchema {
        /// Schema name reported to the provider.
        name: String,
        /// The JSON Schema itself.
        schema: Value,
        /// Whether the provider must enforce the schema exactly.
        strict: bool,
    },
}

/// A function the model is allowed to call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDefinition {
    /// Tool name, as the model will refer to it.
    pub name: String,
    /// What the tool does. The model uses this to decide when to call it.
    pub description: Option<String>,
    /// JSON Schema for the tool's arguments.
    pub parameters: Value,
    /// Whether the provider must enforce `parameters` exactly.
    pub strict: Option<bool>,
}

impl ToolDefinition {
    /// Creates a tool definition with no parameter schema.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: Some(description.into()),
            parameters: Value::Null,
            strict: None,
        }
    }

    /// Sets the JSON Schema describing the tool's arguments.
    pub fn parameters(mut self, parameters: Value) -> Self {
        self.parameters = parameters;
        self
    }

    /// Requests strict schema enforcement.
    pub fn strict(mut self, strict: bool) -> Self {
        self.strict = Some(strict);
        self
    }
}

/// Whether the model must call a tool, and which one.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolChoice {
    /// The model decides.
    Auto,
    /// The model may not call any tool.
    None,
    /// The model must call some tool.
    Required,
    /// The model must call this specific tool.
    Named(String),
}

/// A provider-neutral generation response.
#[derive(Debug, Clone, PartialEq)]
pub struct GenerateResponse {
    /// Provider-assigned response id. Usable as
    /// [`GenerateRequest::previous_response_id`].
    pub id: String,
    /// The model that actually served the request.
    pub model: String,
    /// Why the response ended.
    pub status: ResponseStatus,
    /// The parts the model produced.
    pub content: Vec<OutputContent>,
    /// Token accounting, when the provider reports it.
    pub usage: Option<Usage>,
    /// Provider fields Freyja does not model, preserved verbatim.
    pub provider_metadata: Option<Value>,
}

impl GenerateResponse {
    /// Concatenates every text part into one string.
    pub fn output_text(&self) -> String {
        self.content
            .iter()
            .filter_map(|item| match item {
                OutputContent::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Iterates the tool calls the model requested.
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

    /// Whether the model asked for at least one tool call.
    pub fn has_tool_calls(&self) -> bool {
        self.tool_calls().next().is_some()
    }

    /// Converts this response into an assistant turn, so it can be appended to
    /// the transcript before sending tool results back.
    ///
    /// Refusals become text, because that is how they read in a transcript.
    pub fn to_message(&self) -> Message {
        let content = self
            .content
            .iter()
            .map(|item| match item {
                OutputContent::Text(text) => InputContent::Text(text.clone()),
                OutputContent::Refusal(text) => InputContent::Text(text.clone()),
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
    /// The model finished normally.
    Completed,
    /// The model was cut short, typically by a token limit.
    Incomplete,
    /// The model is waiting on tool results.
    RequiresAction,
    /// The provider failed to produce a response.
    Failed,
    /// A status Freyja does not model, preserved verbatim.
    Other(String),
}

/// One part of what the model produced.
#[derive(Debug, Clone, PartialEq)]
pub enum OutputContent {
    /// Generated text.
    Text(String),
    /// The model declined to answer.
    Refusal(String),
    /// The model wants a tool executed.
    ToolCall {
        /// Correlation id to quote back in [`Message::tool_result`].
        id: String,
        /// Name of the tool to run.
        name: String,
        /// Arguments, as a raw JSON string.
        arguments: String,
    },
    /// Opaque provider state that must be replayed verbatim on the next request.
    ///
    /// Gemini thought signatures, Anthropic thinking blocks, and OpenAI
    /// reasoning items all land here. Providers reject a follow-up request that
    /// drops these or rebuilds them by hand, so they are preserved as received
    /// and carried back by [`GenerateResponse::to_message`].
    ///
    /// Ignore them unless you are assembling a transcript yourself, in which
    /// case keep them in place and in order.
    Reasoning {
        /// The provider's own representation, preserved as received.
        data: Value,
    },
}

/// Token accounting for one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    /// Tokens consumed by the prompt.
    pub input_tokens: u64,
    /// Tokens produced by the model.
    pub output_tokens: u64,
    /// Total tokens billed.
    pub total_tokens: u64,
}

/// Everything that can go wrong on the way to a [`GenerateResponse`].
///
/// `provider` is the endpoint's configured name rather than its dialect, so a
/// failure against a Claude-compatible gateway reports that gateway and not
/// "Anthropic".
#[derive(Debug)]
pub enum ProviderError {
    /// The request asked for something this provider cannot express. Freyja
    /// refuses rather than silently dropping the capability.
    UnsupportedCapability {
        /// Endpoint that refused.
        provider: Arc<str>,
        /// The capability it cannot express.
        capability: &'static str,
    },
    /// The request is malformed and was rejected before leaving the process.
    InvalidRequest {
        /// Endpoint whose mapping rejected the request.
        provider: Arc<str>,
        /// What is wrong with it.
        message: String,
    },
    /// The HTTP request never completed.
    Http(String),
    /// The provider answered with a non-success status.
    Api {
        /// Endpoint that answered.
        provider: Arc<str>,
        /// HTTP status code.
        status: u16,
        /// Raw response body, preserved for debugging.
        body: String,
    },
    /// The provider answered successfully but the body could not be parsed.
    InvalidResponse {
        /// Endpoint that answered.
        provider: Arc<str>,
        /// Parse failure detail, including the body.
        message: String,
    },
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedCapability {
                provider,
                capability,
            } => write!(f, "{provider} does not support {capability}"),
            Self::InvalidRequest { provider, message } => {
                write!(f, "invalid request for {provider}: {message}")
            }
            Self::Http(message) => write!(f, "HTTP request failed: {message}"),
            Self::Api {
                provider,
                status,
                body,
            } => write!(f, "{provider} returned HTTP {status}: {body}"),
            Self::InvalidResponse { provider, message } => {
                write!(f, "invalid {provider} response: {message}")
            }
        }
    }
}

impl std::error::Error for ProviderError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_request_sets_no_capability_defaults() {
        let request = GenerateRequest::new();

        assert!(request.tool_choice.is_none());
        assert!(request.reasoning_effort.is_none());
        assert!(request.response_format.is_none());
        assert_eq!(request, GenerateRequest::default());
    }

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

        let message = response.to_message();
        assert_eq!(message.role, Role::Assistant);
        assert_eq!(message.content[0], InputContent::Text("thinking".into()));
        assert_eq!(
            message.content[1],
            InputContent::ToolCall {
                id: "call_1".into(),
                name: "add".into(),
                arguments: "{\"a\":1,\"b\":2}".into(),
            }
        );
    }

    #[test]
    fn tool_result_builds_a_tool_turn() {
        let message = Message::tool_result("call_1", "3");

        assert_eq!(message.role, Role::Tool);
        assert_eq!(
            message.content,
            vec![InputContent::ToolResult {
                call_id: "call_1".into(),
                output: "3".into(),
            }]
        );
    }
}
