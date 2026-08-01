use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseRequest {
    pub model: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<ResponseInput>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<TextConfig>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_response_id: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponseInput {
    Text(String),
    Messages(Vec<InputMessage>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputMessage {
    pub role: Role,
    pub content: Vec<InputContent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    Developer,
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum InputContent {
    #[serde(rename = "input_text")]
    Text { text: String },

    #[serde(rename = "input_image")]
    Image { image_url: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reasoning {
    pub effort: ReasoningEffort,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextConfig {
    pub format: TextFormat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TextFormat {
    #[serde(rename = "text")]
    Text,

    #[serde(rename = "json_object")]
    JsonObject,

    #[serde(rename = "json_schema")]
    JsonSchema {
        name: String,
        schema: serde_json::Value,
        strict: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Tool {
    #[serde(rename = "web_search")]
    WebSearch {},

    #[serde(rename = "file_search")]
    FileSearch {},

    #[serde(rename = "function")]
    Function {
        name: String,
        description: Option<String>,
        parameters: serde_json::Value,
        strict: Option<bool>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolChoice {
    Auto,
    None,
    Required,
    Named {
        #[serde(rename = "type")]
        tool_type: String,
        name: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub id: String,

    pub object: String,

    pub created_at: u64,

    pub status: String,

    pub model: String,

    pub output: Vec<ResponseOutput>,

    pub usage: Option<Usage>,

    pub error: Option<ResponseError>,

    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ResponseOutput {
    #[serde(rename = "message")]
    Message {
        id: String,
        role: String,
        content: Vec<OutputContent>,
    },

    #[serde(rename = "function_call")]
    FunctionCall {
        id: String,
        call_id: String,
        name: String,
        arguments: String,
    },

    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OutputContent {
    #[serde(rename = "output_text")]
    Text { text: String },

    #[serde(rename = "refusal")]
    Refusal { refusal: String },

    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseError {
    pub code: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Copy)]
pub struct FunctionCallRef<'a> {
    pub id: &'a str,
    pub call_id: &'a str,
    pub name: &'a str,
    pub arguments: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub enum ResponseType<'a> {
    Text(&'a str),
    Refusal(&'a str),
    FunctionCall(FunctionCallRef<'a>),
}

impl FunctionCallRef<'_> {
    pub fn parse_arguments<T: DeserializeOwned>(&self) -> serde_json::Result<T> {
        serde_json::from_str(self.arguments)
    }
}

pub(crate) const DEFAULT_MODEL: &str = "gpt-5.6-sol";

impl ResponseRequest {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn input(mut self, input: impl Into<ResponseInput>) -> Self {
        self.input = Some(input.into());
        self
    }

    pub fn instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    pub fn max_output_tokens(mut self, max_output_tokens: u32) -> Self {
        self.max_output_tokens = Some(max_output_tokens);
        self
    }

    pub fn reasoning(mut self, effort: ReasoningEffort) -> Self {
        self.reasoning = Some(Reasoning::new(effort));
        self
    }

    pub fn text_format(mut self, format: TextFormat) -> Self {
        self.text = Some(TextConfig::new(format));
        self
    }

    pub fn tools(mut self, tools: impl Into<Vec<Tool>>) -> Self {
        self.tools = Some(tools.into());
        self
    }
}

impl Default for ResponseRequest {
    fn default() -> Self {
        Self {
            model: DEFAULT_MODEL.to_owned(),
            input: None,
            instructions: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            reasoning: None,
            text: None,
            tools: None,
            tool_choice: None,
            previous_response_id: None,
            metadata: None,
            store: None,
            stream: None,
            user: None,
        }
    }
}

impl ResponseInput {
    pub fn text(input: impl Into<String>) -> Self {
        Self::Text(input.into())
    }

    pub fn messages(messages: impl Into<Vec<InputMessage>>) -> Self {
        Self::Messages(messages.into())
    }
}

impl From<&str> for ResponseInput {
    fn from(input: &str) -> Self {
        Self::text(input)
    }
}

impl From<String> for ResponseInput {
    fn from(input: String) -> Self {
        Self::text(input)
    }
}

impl From<Vec<InputMessage>> for ResponseInput {
    fn from(messages: Vec<InputMessage>) -> Self {
        Self::messages(messages)
    }
}

impl InputMessage {
    pub fn new(role: Role, content: impl Into<Vec<InputContent>>) -> Self {
        Self {
            role,
            content: content.into(),
        }
    }

    pub fn text(role: Role, text: impl Into<String>) -> Self {
        Self::new(role, vec![InputContent::text(text)])
    }
}

impl InputContent {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    pub fn image(image_url: impl Into<String>) -> Self {
        Self::Image {
            image_url: image_url.into(),
        }
    }
}

impl Reasoning {
    pub fn new(effort: ReasoningEffort) -> Self {
        Self { effort }
    }
}

impl TextConfig {
    pub fn new(format: TextFormat) -> Self {
        Self { format }
    }
}

impl TextFormat {
    pub fn text() -> Self {
        Self::Text
    }

    pub fn json_object() -> Self {
        Self::JsonObject
    }

    pub fn json_schema(
        name: impl Into<String>,
        schema: impl Into<serde_json::Value>,
        strict: bool,
    ) -> Self {
        Self::JsonSchema {
            name: name.into(),
            schema: schema.into(),
            strict,
        }
    }
}

impl Response {
    pub fn text_outputs(&self) -> impl Iterator<Item = &str> {
        self.output
            .iter()
            .flat_map(|output| match output {
                ResponseOutput::Message { content, .. } => content.as_slice(),
                _ => &[],
            })
            .filter_map(OutputContent::as_text)
    }

    pub fn output_text(&self) -> String {
        self.text_outputs().collect()
    }

    pub fn output_text_opt(&self) -> Option<String> {
        let text = self.output_text();
        (!text.is_empty()).then_some(text)
    }

    pub fn refusals(&self) -> impl Iterator<Item = &str> {
        self.output
            .iter()
            .flat_map(|output| match output {
                ResponseOutput::Message { content, .. } => content.as_slice(),
                _ => &[],
            })
            .filter_map(OutputContent::as_refusal)
    }

    pub fn function_calls(&self) -> impl Iterator<Item = FunctionCallRef<'_>> {
        self.output
            .iter()
            .filter_map(ResponseOutput::as_function_call)
    }

    pub fn items(&self) -> impl Iterator<Item = ResponseType<'_>> {
        self.output.iter().flat_map(ResponseOutput::items)
    }

    pub fn has_text_output(&self) -> bool {
        self.text_outputs().next().is_some()
    }

    pub fn has_refusals(&self) -> bool {
        self.refusals().next().is_some()
    }

    pub fn has_function_calls(&self) -> bool {
        self.function_calls().next().is_some()
    }

    pub fn is_success(&self) -> bool {
        self.error.is_none() && self.status == "completed"
    }
}

impl ResponseOutput {
    pub fn as_function_call(&self) -> Option<FunctionCallRef<'_>> {
        match self {
            Self::FunctionCall {
                id,
                call_id,
                name,
                arguments,
            } => Some(FunctionCallRef {
                id,
                call_id,
                name,
                arguments,
            }),
            _ => None,
        }
    }

    fn items(&self) -> Vec<ResponseType<'_>> {
        match self {
            Self::Message { content, .. } => content
                .iter()
                .filter_map(OutputContent::as_response_item)
                .collect(),
            Self::FunctionCall { .. } => self
                .as_function_call()
                .map(ResponseType::FunctionCall)
                .into_iter()
                .collect(),
            Self::Unknown => Vec::new(),
        }
    }
}

impl OutputContent {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            _ => None,
        }
    }

    pub fn as_refusal(&self) -> Option<&str> {
        match self {
            Self::Refusal { refusal } => Some(refusal),
            _ => None,
        }
    }

    fn as_response_item(&self) -> Option<ResponseType<'_>> {
        match self {
            Self::Text { text } => Some(ResponseType::Text(text)),
            Self::Refusal { refusal } => Some(ResponseType::Refusal(refusal)),
            Self::Unknown => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response_with(output: Vec<ResponseOutput>) -> Response {
        Response {
            id: "response-id".into(),
            object: "response".into(),
            created_at: 0,
            status: "completed".into(),
            model: DEFAULT_MODEL.into(),
            output,
            usage: None,
            error: None,
            metadata: None,
        }
    }

    #[test]
    fn collects_text_and_refusals() {
        let response = response_with(vec![ResponseOutput::Message {
            id: "message-id".into(),
            role: "assistant".into(),
            content: vec![
                OutputContent::Text {
                    text: "Hello, ".into(),
                },
                OutputContent::Text {
                    text: "world!".into(),
                },
                OutputContent::Refusal {
                    refusal: "Cannot do that".into(),
                },
            ],
        }]);

        assert_eq!(response.output_text(), "Hello, world!");
        assert_eq!(response.refusals().collect::<Vec<_>>(), ["Cannot do that"]);
        assert!(response.is_success());
    }

    #[test]
    fn parses_function_call_arguments() {
        #[derive(Debug, Deserialize, PartialEq)]
        struct WeatherArguments {
            city: String,
        }

        let response = response_with(vec![ResponseOutput::FunctionCall {
            id: "item-id".into(),
            call_id: "call-id".into(),
            name: "get_weather".into(),
            arguments: r#"{"city":"Jakarta"}"#.into(),
        }]);

        let call = response.function_calls().next().unwrap();
        let arguments: WeatherArguments = call.parse_arguments().unwrap();

        assert_eq!(call.name, "get_weather");
        assert_eq!(
            arguments,
            WeatherArguments {
                city: "Jakarta".into()
            }
        );
    }

    #[test]
    fn exposes_guard_methods_and_matchable_items() {
        let response = response_with(vec![
            ResponseOutput::Message {
                id: "message-id".into(),
                role: "assistant".into(),
                content: vec![OutputContent::Text {
                    text: "Hello".into(),
                }],
            },
            ResponseOutput::FunctionCall {
                id: "item-id".into(),
                call_id: "call-id".into(),
                name: "do_work".into(),
                arguments: "{}".into(),
            },
        ]);

        assert!(response.has_text_output());
        assert!(!response.has_refusals());
        assert!(response.has_function_calls());
        assert_eq!(response.output_text_opt().as_deref(), Some("Hello"));

        let item_names = response
            .items()
            .map(|item| match item {
                ResponseType::Text(_) => "text",
                ResponseType::Refusal(_) => "refusal",
                ResponseType::FunctionCall(_) => "function_call",
            })
            .collect::<Vec<_>>();

        assert_eq!(item_names, ["text", "function_call"]);
    }
}
