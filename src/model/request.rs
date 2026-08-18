use super::{Message, ReasoningEffort, ResponseFormat, ToolChoice, ToolDefinition};
use crate::dialect::Dialect;
use serde_json::{Map, Value};

/// A provider-neutral generation request.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GenerateRequest {
    /// Model identifier. `None` uses the endpoint default.
    pub model: Option<String>,
    /// Conversation turns, in order.
    pub messages: Vec<Message>,
    /// Upper bound on generated tokens.
    pub max_tokens: Option<u32>,
    /// Sampling temperature.
    pub temperature: Option<f32>,
    /// Nucleus sampling cutoff.
    pub top_p: Option<f32>,
    /// Internal reasoning effort.
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Required response shape.
    pub response_format: Option<ResponseFormat>,
    /// Functions the model may call.
    pub tools: Vec<ToolDefinition>,
    /// Tool-call constraint.
    pub tool_choice: Option<ToolChoice>,
    /// Server-side conversation continuation token.
    pub previous_response_id: Option<String>,
    /// Metadata forwarded to the provider.
    pub metadata: Option<Value>,
    /// Dialect-scoped provider-specific fields.
    pub extra: Vec<(Dialect, Map<String, Value>)>,
}

impl GenerateRequest {
    /// Creates an empty request with no capability defaults.
    pub fn new() -> Self {
        Self::default()
    }
    /// Sets the model identifier.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }
    /// Appends a conversation message.
    pub fn message(mut self, message: Message) -> Self {
        self.messages.push(message);
        self
    }
    /// Replaces the conversation.
    pub fn messages(mut self, messages: impl Into<Vec<Message>>) -> Self {
        self.messages = messages.into();
        self
    }
    /// Appends conversation messages.
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
    /// Sets the internal reasoning effort.
    pub fn reasoning_effort(mut self, effort: ReasoningEffort) -> Self {
        self.reasoning_effort = Some(effort);
        self
    }
    /// Constrains the response shape.
    pub fn response_format(mut self, format: ResponseFormat) -> Self {
        self.response_format = Some(format);
        self
    }
    /// Continues a server-side conversation.
    pub fn previous_response_id(mut self, id: impl Into<String>) -> Self {
        self.previous_response_id = Some(id.into());
        self
    }
    /// Attaches provider metadata.
    pub fn metadata(mut self, metadata: Value) -> Self {
        self.metadata = Some(metadata);
        self
    }
    /// Adds dialect-scoped fields that are deep-merged into the wire body.
    ///
    /// # Panics
    ///
    /// Panics when `fields` is not a JSON object.
    pub fn extra_for(mut self, dialect: Dialect, fields: Value) -> Self {
        let Value::Object(fields) = fields else {
            panic!("extra_for expects a JSON object, got {fields}");
        };
        self.extra.push((dialect, fields));
        self
    }
}

/// Deep-merges `overlay` into `base`.
pub(crate) fn merge_into(base: &mut Map<String, Value>, overlay: &Map<String, Value>) {
    for (key, value) in overlay {
        match (base.get_mut(key), value) {
            (Some(Value::Object(nested)), Value::Object(incoming)) => merge_into(nested, incoming),
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::GenerateRequest;
    #[test]
    fn new_request_sets_no_capability_defaults() {
        let request = GenerateRequest::new();
        assert!(request.tool_choice.is_none());
        assert!(request.reasoning_effort.is_none());
        assert!(request.response_format.is_none());
        assert_eq!(request, GenerateRequest::default());
    }
}
