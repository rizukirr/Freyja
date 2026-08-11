//! Provider-neutral request, response, and error types.
//!
//! Nothing in this module is vendor-specific. Each provider owns its own wire
//! format and converts to and from these types, so application code never has
//! to name an OpenAI or Gemini struct.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

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

/// Why a request never reached the endpoint.
///
/// Every variant describes a failure that happened below HTTP semantics: no
/// status code was ever received, so there is nothing for the endpoint to have
/// said. The distinction that matters to a caller is whether trying again could
/// help, which [`ProviderError::is_retryable`] answers from this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TransportError {
    /// No reply arrived within the configured inactivity timeout.
    ///
    /// Transient by nature: the same request may well succeed on a later
    /// attempt.
    Timeout,
    /// The connection could not be established at all.
    ///
    /// Covers DNS failure, a refused connection, and a rejected TLS
    /// certificate. `reqwest` reports the three identically, and they call for
    /// the same response from a caller: fix the configuration rather than try
    /// again.
    Connect,
    /// The connection died while the body was being read.
    ///
    /// The endpoint accepted the request and began answering, so a later
    /// attempt is worth making.
    Body,
    /// A transport failure the classification above does not cover.
    Other,
}

impl TransportError {
    /// Classify a transport failure from what `reqwest` reports about it.
    pub(crate) fn classify(error: &reqwest::Error) -> Self {
        if error.is_timeout() {
            Self::Timeout
        } else if error.is_connect() {
            Self::Connect
        } else if error.is_body() {
            Self::Body
        } else {
            Self::Other
        }
    }

    /// Whether repeating the request could plausibly get further.
    const fn is_retryable(self) -> bool {
        matches!(self, Self::Timeout | Self::Body)
    }

    /// A short description, used by [`ProviderError`]'s `Display`.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timed out",
            Self::Connect => "could not be reached",
            Self::Body => "dropped the connection",
            Self::Other => "transport failed",
        }
    }
}

/// Everything that can go wrong on the way to a [`GenerateResponse`].
///
/// `provider` is the endpoint's configured name rather than its dialect, so a
/// failure against a Claude-compatible gateway reports that gateway and not
/// "Anthropic".
///
/// The variants fall into three groups: refusals raised before anything left
/// the process, transport failures where no answer came back, and answers the
/// endpoint actually sent. That last group is named by meaning rather than by
/// status code, because the same code means different things on different
/// vendors — but each variant still carries the status and the raw body, so
/// nothing the endpoint said is lost.
///
/// Freyja never retries. [`Self::is_retryable`] and [`Self::retry_after`]
/// report what it knows so a caller's own loop does not have to rediscover it
/// per vendor.
#[derive(Debug)]
#[non_exhaustive]
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
    ///
    /// Distinct from [`Self::BadRequest`], which is the endpoint rejecting a
    /// request Freyja was willing to send.
    InvalidRequest {
        /// Endpoint whose mapping rejected the request.
        provider: Arc<str>,
        /// What is wrong with it.
        message: String,
    },
    /// The HTTP request never completed, so no status was ever received.
    ///
    /// Distinct from [`Self::Api`] and its siblings, where the endpoint did
    /// answer and the answer was an error.
    Http {
        /// Endpoint that could not be reached.
        provider: Arc<str>,
        /// Why it could not be reached.
        kind: TransportError,
        /// The underlying transport message, preserved for debugging.
        message: String,
    },
    /// `400` — the endpoint rejected the request body.
    ///
    /// Not retryable: the same bytes will be rejected again.
    BadRequest {
        /// Endpoint that rejected it.
        provider: Arc<str>,
        /// Raw response body, preserved for debugging.
        body: String,
    },
    /// `401` or `403` — the credential is missing, wrong, or not permitted to
    /// use this model.
    ///
    /// Not retryable: the key has to change.
    Unauthorized {
        /// Endpoint that refused.
        provider: Arc<str>,
        /// The status, since `401` and `403` both land here.
        status: u16,
        /// Raw response body, preserved for debugging.
        body: String,
    },
    /// `404` — no such model, or the base URL is wrong.
    ///
    /// Not retryable.
    NotFound {
        /// Endpoint that answered.
        provider: Arc<str>,
        /// Raw response body, preserved for debugging.
        body: String,
    },
    /// `429` — requests are arriving faster than the endpoint will serve them.
    ///
    /// Retryable once `retry_after` has elapsed. That field carries the
    /// endpoint's own `Retry-After` header when it sent one, and is `None`
    /// otherwise, in which case the caller's own backoff applies.
    RateLimit {
        /// Endpoint that throttled the request.
        provider: Arc<str>,
        /// How long the endpoint asked the caller to wait, if it said.
        retry_after: Option<Duration>,
        /// Raw response body, preserved for debugging.
        body: String,
    },
    /// The account is out of credit or past a hard quota.
    ///
    /// Not retryable by waiting: it needs billing action. Several vendors
    /// report this with the same `429` they use for throttling and separate the
    /// two only in the body, so a caller that treats every `429` as a rate
    /// limit will retry a dead account forever.
    ///
    /// Coverage is uneven, because the split is only possible where the body
    /// says so. The `insufficient_quota` marker on OpenAI-shaped bodies covers
    /// both OpenAI dialects and the endpoints that copy them; Gemini reports
    /// both cases as `RESOURCE_EXHAUSTED` and cannot be split, and Anthropic
    /// has no equivalent. On those, an exhausted quota arrives as
    /// [`Self::RateLimit`] and a bounded retry loop is the protection.
    QuotaExceeded {
        /// Endpoint that refused.
        provider: Arc<str>,
        /// The status the endpoint used, since vendors disagree.
        status: u16,
        /// Raw response body, preserved for debugging.
        body: String,
    },
    /// `5xx` — the endpoint failed on its own side.
    ///
    /// Retryable.
    ServerError {
        /// Endpoint that failed.
        provider: Arc<str>,
        /// HTTP status code, including non-standard ones such as Anthropic's
        /// `529`.
        status: u16,
        /// Raw response body, preserved for debugging.
        body: String,
    },
    /// A non-success status Freyja does not classify.
    ///
    /// The fallback arm. A status no named variant claims arrives here intact
    /// rather than being forced into an approximate category.
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
    /// The response streamed successfully up to a point, then failed.
    ///
    /// Covers a provider's own mid-stream error frame, and calling
    /// `EventStream::into_response` before the stream has been drained. A body
    /// that simply ends early is *not* an error: pending tool calls are flushed
    /// and the stream reports [`ResponseStatus::Incomplete`], so a truncated
    /// answer is still readable.
    ///
    /// Distinct from [`Self::Api`], which reports a non-success HTTP status,
    /// and from [`Self::InvalidResponse`], which reports a body that could not
    /// be parsed at all.
    Stream {
        /// Endpoint whose stream failed.
        provider: Arc<str>,
        /// What went wrong.
        message: String,
    },
}

/// Marker OpenAI-shaped error bodies use to distinguish an exhausted quota from
/// ordinary throttling. Both arrive as `429`.
const QUOTA_MARKER: &str = "insufficient_quota";

impl ProviderError {
    /// Classify a non-success response into the narrowest variant that fits.
    ///
    /// A status no named variant claims becomes [`Self::Api`] rather than being
    /// forced into an approximate category, so an unfamiliar endpoint never
    /// reports something Freyja cannot actually tell.
    ///
    /// The `429` split into [`Self::RateLimit`] and [`Self::QuotaExceeded`]
    /// keys on the `insufficient_quota` marker that OpenAI-shaped bodies carry,
    /// which covers the OpenAI dialects and the many third-party endpoints that
    /// copy them. Gemini reports both cases as `RESOURCE_EXHAUSTED` and cannot
    /// be split; Anthropic has no equivalent. On those, an exhausted quota
    /// arrives as [`Self::RateLimit`].
    pub(crate) fn from_status(
        provider: Arc<str>,
        status: u16,
        retry_after: Option<Duration>,
        body: String,
    ) -> Self {
        match status {
            400 => Self::BadRequest { provider, body },
            401 | 403 => Self::Unauthorized {
                provider,
                status,
                body,
            },
            404 => Self::NotFound { provider, body },
            429 if body.contains(QUOTA_MARKER) => Self::QuotaExceeded {
                provider,
                status,
                body,
            },
            429 => Self::RateLimit {
                provider,
                retry_after,
                body,
            },
            500..=599 => Self::ServerError {
                provider,
                status,
                body,
            },
            _ => Self::Api {
                provider,
                status,
                body,
            },
        }
    }

    /// Build a [`Self::Http`] from a transport failure, classifying it.
    pub(crate) fn transport(provider: Arc<str>, error: &reqwest::Error) -> Self {
        Self::Http {
            provider,
            kind: TransportError::classify(error),
            message: error.to_string(),
        }
    }

    /// Whether repeating the identical request could plausibly succeed.
    ///
    /// Freyja never retries — this reports what it knows so a caller's own loop
    /// does not have to rediscover it per vendor. `false` means the request
    /// will fail the same way every time until something outside it changes:
    /// the key, the model name, the request body, or the account balance.
    ///
    /// A caller that also wants to respect the endpoint's pacing pairs this
    /// with [`Self::retry_after`].
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Http { kind, .. } => kind.is_retryable(),
            Self::RateLimit { .. } | Self::ServerError { .. } => true,
            Self::Api { status, .. } => *status >= 500,
            _ => false,
        }
    }

    /// How long the endpoint asked the caller to wait, when it said so.
    ///
    /// Only [`Self::RateLimit`] carries this, and only when the response
    /// included a `Retry-After` header.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimit { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    /// The HTTP status, for the errors that have one.
    ///
    /// `None` for failures raised before the request left the process and for
    /// transport failures, neither of which ever saw a status.
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::BadRequest { .. } => Some(400),
            Self::NotFound { .. } => Some(404),
            Self::RateLimit { .. } => Some(429),
            Self::Unauthorized { status, .. }
            | Self::QuotaExceeded { status, .. }
            | Self::ServerError { status, .. }
            | Self::Api { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// The endpoint's configured name.
    ///
    /// Every variant carries it, so a caller holding several clients can always
    /// tell which one failed.
    pub fn provider(&self) -> &str {
        match self {
            Self::UnsupportedCapability { provider, .. }
            | Self::InvalidRequest { provider, .. }
            | Self::Http { provider, .. }
            | Self::BadRequest { provider, .. }
            | Self::Unauthorized { provider, .. }
            | Self::NotFound { provider, .. }
            | Self::RateLimit { provider, .. }
            | Self::QuotaExceeded { provider, .. }
            | Self::ServerError { provider, .. }
            | Self::Api { provider, .. }
            | Self::InvalidResponse { provider, .. }
            | Self::Stream { provider, .. } => provider,
        }
    }
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
            Self::Http {
                provider,
                kind,
                message,
            } => write!(f, "{provider} {}: {message}", kind.as_str()),
            Self::BadRequest { provider, body } => {
                write!(f, "{provider} rejected the request: {body}")
            }
            Self::Unauthorized {
                provider,
                status,
                body,
            } => write!(
                f,
                "{provider} refused the credential (HTTP {status}): {body}"
            ),
            Self::NotFound { provider, body } => {
                write!(f, "{provider} has no such model or endpoint: {body}")
            }
            Self::RateLimit {
                provider,
                retry_after,
                body,
            } => match retry_after {
                Some(wait) => write!(
                    f,
                    "{provider} rate limited the request, retry after {}s: {body}",
                    wait.as_secs()
                ),
                None => write!(f, "{provider} rate limited the request: {body}"),
            },
            Self::QuotaExceeded {
                provider,
                status,
                body,
            } => write!(f, "{provider} quota exhausted (HTTP {status}): {body}"),
            Self::ServerError {
                provider,
                status,
                body,
            } => write!(f, "{provider} failed with HTTP {status}: {body}"),
            Self::Api {
                provider,
                status,
                body,
            } => write!(f, "{provider} returned HTTP {status}: {body}"),
            Self::InvalidResponse { provider, message } => {
                write!(f, "invalid {provider} response: {message}")
            }
            Self::Stream { provider, message } => {
                write!(f, "{provider} stream failed: {message}")
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

    #[test]
    fn error_stream_displays_provider_and_message() {
        let error = ProviderError::Stream {
            provider: "acme".into(),
            message: "boom".into(),
        };

        assert_eq!(error.to_string(), "acme stream failed: boom");
    }

    /// Shorthand for the mapping tests: only the status is under test.
    fn classify(status: u16, body: &str) -> ProviderError {
        ProviderError::from_status("acme".into(), status, None, body.into())
    }

    #[test]
    fn from_status_names_the_statuses_it_recognises() {
        assert!(matches!(
            classify(400, "{}"),
            ProviderError::BadRequest { .. }
        ));
        assert!(matches!(
            classify(401, "{}"),
            ProviderError::Unauthorized { status: 401, .. }
        ));
        assert!(matches!(
            classify(403, "{}"),
            ProviderError::Unauthorized { status: 403, .. }
        ));
        assert!(matches!(
            classify(404, "{}"),
            ProviderError::NotFound { .. }
        ));
        assert!(matches!(
            classify(429, "{}"),
            ProviderError::RateLimit { .. }
        ));
        assert!(matches!(
            classify(503, "{}"),
            ProviderError::ServerError { status: 503, .. }
        ));
    }

    #[test]
    fn from_status_treats_every_5xx_as_a_server_error() {
        // Anthropic's overload signal is non-standard, so a hard-coded list of
        // the familiar 500-503 would drop it into the fallback arm and report
        // it as unretryable.
        let error = classify(529, "overloaded_error");

        assert!(matches!(
            error,
            ProviderError::ServerError { status: 529, .. }
        ));
        assert!(error.is_retryable());
    }

    #[test]
    fn from_status_falls_back_rather_than_guessing() {
        // A status no variant claims must survive intact instead of being
        // forced into an approximate category.
        let error = classify(418, "teapot");

        assert!(matches!(error, ProviderError::Api { status: 418, .. }));
        assert_eq!(error.status(), Some(418));
        assert!(!error.is_retryable());
    }

    #[test]
    fn an_exhausted_quota_is_not_a_rate_limit() {
        // Both arrive as 429 on OpenAI-shaped endpoints and only the body
        // separates them. Reporting the second as a rate limit would tell a
        // caller to retry a request that can never succeed.
        let throttled = classify(429, r#"{"error":{"code":"rate_limit_exceeded"}}"#);
        let exhausted = classify(429, r#"{"error":{"code":"insufficient_quota"}}"#);

        assert!(matches!(throttled, ProviderError::RateLimit { .. }));
        assert!(throttled.is_retryable());

        assert!(matches!(
            exhausted,
            ProviderError::QuotaExceeded { status: 429, .. }
        ));
        assert!(!exhausted.is_retryable());
    }

    #[test]
    fn retry_after_survives_onto_the_rate_limit() {
        let error = ProviderError::from_status(
            "acme".into(),
            429,
            Some(Duration::from_secs(30)),
            "slow down".into(),
        );

        assert_eq!(error.retry_after(), Some(Duration::from_secs(30)));
        assert_eq!(
            error.to_string(),
            "acme rate limited the request, retry after 30s: slow down"
        );
    }

    #[test]
    fn only_a_rate_limit_carries_a_retry_delay() {
        assert_eq!(classify(503, "{}").retry_after(), None);
        assert_eq!(classify(429, "{}").retry_after(), None);
    }

    #[test]
    fn transport_failures_split_on_whether_waiting_helps() {
        let retryable = [TransportError::Timeout, TransportError::Body];
        let permanent = [TransportError::Connect, TransportError::Other];

        for kind in retryable {
            let error = ProviderError::Http {
                provider: "acme".into(),
                kind,
                message: "x".into(),
            };
            assert!(error.is_retryable(), "{kind:?} should be retryable");
            assert_eq!(error.status(), None, "no status was ever received");
        }

        for kind in permanent {
            let error = ProviderError::Http {
                provider: "acme".into(),
                kind,
                message: "x".into(),
            };
            assert!(!error.is_retryable(), "{kind:?} should not be retryable");
        }
    }

    #[test]
    fn refusals_raised_before_the_network_are_never_retryable() {
        let unsupported = ProviderError::UnsupportedCapability {
            provider: "acme".into(),
            capability: "tool_choice",
        };
        let invalid = ProviderError::InvalidRequest {
            provider: "acme".into(),
            message: "no messages".into(),
        };

        assert!(!unsupported.is_retryable());
        assert!(!invalid.is_retryable());
        assert_eq!(unsupported.status(), None);
        assert_eq!(invalid.status(), None);
    }

    #[test]
    fn every_error_names_the_endpoint_that_produced_it() {
        // A caller holding several clients has to be able to tell which one
        // failed, including on the transport failures that once carried only a
        // message.
        let errors = [
            classify(400, "{}"),
            classify(401, "{}"),
            classify(404, "{}"),
            classify(429, "{}"),
            classify(429, "insufficient_quota"),
            classify(500, "{}"),
            classify(418, "{}"),
            ProviderError::Http {
                provider: "acme".into(),
                kind: TransportError::Timeout,
                message: "x".into(),
            },
            ProviderError::UnsupportedCapability {
                provider: "acme".into(),
                capability: "reasoning_effort",
            },
            ProviderError::InvalidRequest {
                provider: "acme".into(),
                message: "x".into(),
            },
            ProviderError::InvalidResponse {
                provider: "acme".into(),
                message: "x".into(),
            },
            ProviderError::Stream {
                provider: "acme".into(),
                message: "x".into(),
            },
        ];

        for error in &errors {
            assert_eq!(error.provider(), "acme", "{error:?}");
        }
    }

    #[test]
    fn named_errors_display_their_cause_and_endpoint() {
        assert_eq!(
            classify(400, "bad json").to_string(),
            "acme rejected the request: bad json"
        );
        assert_eq!(
            classify(401, "no key").to_string(),
            "acme refused the credential (HTTP 401): no key"
        );
        assert_eq!(
            classify(404, "no model").to_string(),
            "acme has no such model or endpoint: no model"
        );
        assert_eq!(
            classify(500, "oops").to_string(),
            "acme failed with HTTP 500: oops"
        );
        assert_eq!(
            ProviderError::Http {
                provider: "acme".into(),
                kind: TransportError::Timeout,
                message: "operation timed out".into(),
            }
            .to_string(),
            "acme timed out: operation timed out"
        );
    }
}
