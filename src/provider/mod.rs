//! Provider abstraction: one neutral model, several wire dialects, many endpoints.
//!
//! Freyja separates *how* a request is serialized from *where* it is sent. A
//! [`ProviderDialect`] is a wire format, and a [`ProviderConfig`] is an endpoint
//! that speaks one. That split is what lets a single dialect serve a dozen
//! vendors: most hosted inference APIs are wire-compatible with either OpenAI or
//! Anthropic, and differ only in base URL, credentials, and model names.
//!
//! Reach for [`ProviderType`] first, it names the endpoints Freyja knows about.
//! Drop to [`ProviderConfig`] when yours is not on that list.

pub(crate) mod anthropic;
pub(crate) mod gemini;
pub(crate) mod openai_chat;
pub(crate) mod openai_responses;

mod model;
mod presets;
mod refusal;
pub(crate) mod sse;
pub(crate) mod stream;

pub use model::*;
pub use presets::ProviderType;
pub use stream::{EventStream, StreamEvent};

use serde::Serialize;
use serde_json::Value;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

/// Default inactivity timeout applied by [`Client::new`].
///
/// This bounds the gap between bytes, not the total duration of a request.
/// A total timeout would cap how long a response may take to generate, which
/// is wrong for [`Client::stream`]: a long generation is not a stalled one.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// A wire format.
///
/// This is the shape of the JSON, not the vendor serving it. `OpenAiChat` in
/// particular is spoken by a long list of providers that are not OpenAI, which
/// is the reason dialect and endpoint are separate types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderDialect {
    /// OpenAI's Responses API, a flat item list with `previous_response_id`.
    ///
    /// OpenAI-specific. Compatible vendors implement Chat Completions instead.
    OpenAiResponses,
    /// OpenAI's Chat Completions API, the format the compatible ecosystem
    /// speaks.
    ///
    /// Implemented by Groq, Together, Fireworks, DeepSeek, OpenRouter, Ollama,
    /// vLLM, and others, so most vendors need only a [`ProviderConfig`].
    OpenAiChat,
    /// Google Gemini's Interactions API, a flat step list.
    Gemini,
    /// Anthropic's Messages API, content blocks nested inside messages.
    ///
    /// Also spoken by several vendors offering drop-in Claude endpoints.
    Anthropic,
}

impl ProviderDialect {
    /// The path appended to [`ProviderConfig::base_url`].
    pub fn path(self) -> &'static str {
        match self {
            Self::OpenAiResponses => "/responses",
            Self::OpenAiChat => "/chat/completions",
            Self::Gemini => "/interactions",
            Self::Anthropic => "/messages",
        }
    }

    /// How this format conventionally carries credentials.
    ///
    /// A [`ProviderConfig`] may override it, since compatible endpoints
    /// sometimes authenticate differently from the vendor they imitate.
    pub fn default_auth(self) -> Auth {
        match self {
            Self::OpenAiResponses | Self::OpenAiChat => Auth::Bearer,
            Self::Gemini => Auth::Header("x-goog-api-key"),
            Self::Anthropic => Auth::Header("x-api-key"),
        }
    }

    /// Headers the format requires regardless of which vendor serves it.
    pub fn required_headers(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::OpenAiResponses | Self::OpenAiChat => &[],
            Self::Gemini => &[("Api-Revision", "2026-05-20")],
            Self::Anthropic => &[("anthropic-version", "2023-06-01")],
        }
    }

    /// The query string that selects SSE, for dialects that need one.
    ///
    /// Gemini's Interactions API takes `?alt=sse` in addition to the body's
    /// `stream` field; the others are selected by the body alone.
    ///
    /// Public for the same reason as [`Self::path`], [`Self::default_auth`], and
    /// [`Self::required_headers`]: it describes the dialect, and a caller
    /// building a request by hand needs it.
    pub fn stream_query(self) -> Option<&'static str> {
        match self {
            Self::Gemini => Some("alt=sse"),
            Self::OpenAiResponses | Self::OpenAiChat | Self::Anthropic => None,
        }
    }
}

/// How an endpoint expects credentials to be presented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Auth {
    /// `Authorization: Bearer <key>`.
    Bearer,
    /// A named header carrying the raw key, such as `x-api-key`.
    Header(&'static str),
    /// No credentials at all, for local runtimes like Ollama.
    None,
}

/// Which field carries the output token cap on a Chat Completions endpoint.
///
/// The format has had two. `max_tokens` came first and is what the compatible
/// ecosystem implements; OpenAI later replaced it with `max_completion_tokens`
/// and newer OpenAI models now reject the old name outright rather than
/// ignoring it, so this cannot be papered over by sending both.
///
/// Only the [`ProviderDialect::OpenAiChat`] dialect reads this. The other three
/// name their cap unambiguously: `max_output_tokens` on Responses,
/// `maxOutputTokens` on Gemini, `max_tokens` on Anthropic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TokenLimitField {
    /// `max_tokens`. The original field, and the default.
    MaxTokens,
    /// `max_completion_tokens`. Required by newer OpenAI models.
    MaxCompletionTokens,
}

/// An endpoint: where to send a request, how to authenticate, and what to
/// default when the caller does not say.
///
/// ```
/// use freyja::{Auth, ProviderConfig, ProviderDialect};
///
/// let config = ProviderConfig::new(ProviderDialect::Anthropic, "Z.ai", "https://api.z.ai/api/anthropic/v1")
///     .api_key_env("ZAI_API_KEY")
///     .default_model("glm-4.6");
/// assert_eq!(config.auth, Auth::Header("x-api-key"));
/// ```
#[derive(Clone)]
pub struct ProviderConfig {
    /// Which wire format this endpoint speaks.
    pub dialect: ProviderDialect,
    /// Display name, used for error attribution so a Groq failure does not
    /// report itself as OpenAI.
    pub name: Arc<str>,
    /// Root URL. [`ProviderDialect::path`] is appended to it.
    pub base_url: String,
    /// How to present credentials.
    pub auth: Auth,
    /// Environment variable holding the key, for [`Client::from_env`].
    pub api_key_env: Option<&'static str>,
    /// Model used when [`GenerateRequest::model`] is unset.
    ///
    /// There is no library-wide default, because a model name is only
    /// meaningful on the endpoint serving it.
    pub default_model: Option<String>,
    /// Extra headers, for gateways that want attribution or routing hints.
    ///
    /// Reach for [`ProviderConfig::auth`] to carry a credential, not this: the
    /// API key is redacted everywhere Freyja prints itself, and a header value
    /// is only redacted when its *name* gives it away. See this type's `Debug`.
    pub extra_headers: Vec<(String, String)>,
    /// Extra body fields, for what this endpoint wants on every request.
    ///
    /// The companion to `extra_headers`, one layer down. Deep-merged into the
    /// wire body the way [`GenerateRequest::extra_for`] is, and applied first,
    /// so a request can still override what the endpoint sets here.
    ///
    /// Use this for a property of the deployment — a safety configuration, a
    /// routing hint, a tier — and `extra_for` for anything that varies per
    /// call.
    pub extra_body: serde_json::Map<String, Value>,
    /// Which field carries [`GenerateRequest::max_tokens`] on this endpoint.
    ///
    /// Read only by the [`ProviderDialect::OpenAiChat`] dialect, and defaulted
    /// to [`TokenLimitField::MaxTokens`] because that is what the compatible
    /// ecosystem implements. Point that dialect at OpenAI itself and you want
    /// [`TokenLimitField::MaxCompletionTokens`] instead.
    pub token_limit_field: TokenLimitField,
}

/// Substrings that mark a header as carrying a secret.
///
/// Matched case-insensitively against the header *name*. A heuristic, and named
/// as one: it cannot know that `x-acme-passport` is a credential.
const SECRET_HEADER_MARKERS: [&str; 6] = ["auth", "key", "token", "secret", "cookie", "password"];

/// Whether a header's value should be withheld from `Debug`.
fn is_secret_header(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    SECRET_HEADER_MARKERS
        .iter()
        .any(|marker| name.contains(marker))
}

/// Redacts header values that look like credentials.
///
/// [`Client`] takes care of the API key, but it prints its config, and a
/// gateway needing a second credential has nowhere to put it but
/// [`ProviderConfig::extra_headers`]. A derived `Debug` would print that
/// verbatim and undo the redaction one field over.
///
/// Names are always shown, and a value is withheld only when its name contains
/// `auth`, `key`, `token`, `secret`, `cookie`, or `password`, so a routing hint
/// stays as readable as it was. A heuristic, and stated as one: it cannot know
/// that `x-acme-passport` is a credential. Put credentials in
/// [`ProviderConfig::auth`], which is redacted whatever it is called.
impl fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let headers: Vec<(&str, &str)> = self
            .extra_headers
            .iter()
            .map(|(name, value)| {
                let value = if is_secret_header(name) {
                    "<redacted>"
                } else {
                    value.as_str()
                };
                (name.as_str(), value)
            })
            .collect();

        f.debug_struct("ProviderConfig")
            .field("dialect", &self.dialect)
            .field("name", &self.name)
            .field("base_url", &self.base_url)
            .field("auth", &self.auth)
            .field("api_key_env", &self.api_key_env)
            .field("default_model", &self.default_model)
            .field("extra_headers", &headers)
            .field("extra_body", &self.extra_body)
            .field("token_limit_field", &self.token_limit_field)
            .finish()
    }
}

impl ProviderConfig {
    /// Creates a config with the dialect's conventional auth style.
    pub fn new(
        dialect: ProviderDialect,
        name: impl Into<Arc<str>>,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            dialect,
            name: name.into(),
            base_url: base_url.into(),
            auth: dialect.default_auth(),
            api_key_env: None,
            default_model: None,
            extra_headers: Vec::new(),
            extra_body: serde_json::Map::new(),
            token_limit_field: TokenLimitField::MaxTokens,
        }
    }

    /// Overrides how credentials are presented.
    pub fn auth(mut self, auth: Auth) -> Self {
        self.auth = auth;
        self
    }

    /// Names the environment variable holding the key.
    pub fn api_key_env(mut self, variable: &'static str) -> Self {
        self.api_key_env = Some(variable);
        self
    }

    /// Sets the model used when a request does not name one.
    pub fn default_model(mut self, model: impl Into<String>) -> Self {
        self.default_model = Some(model.into());
        self
    }

    /// Adds an extra header sent with every request.
    ///
    /// For attribution and routing. A credential belongs in
    /// [`ProviderConfig::auth`], which is redacted unconditionally; a value
    /// left here is redacted only if its name says what it is.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_headers.push((name.into(), value.into()));
        self
    }

    /// Adds body fields sent with every request to this endpoint.
    ///
    /// For a property of the deployment rather than of the call — a safety
    /// configuration, a routing hint, a tier. Deep-merged into the wire body,
    /// and a request's own [`GenerateRequest::extra_for`] overrides it.
    ///
    /// ```
    /// use freyja::{ProviderConfig, ProviderDialect};
    /// use serde_json::json;
    ///
    /// let config = ProviderConfig::new(
    ///     ProviderDialect::Gemini,
    ///     "Gemini",
    ///     "https://generativelanguage.googleapis.com/v1beta",
    /// )
    /// .body(json!({"safety_settings": [{"category": "HARM_CATEGORY_HARASSMENT"}]}));
    /// ```
    ///
    /// # Panics
    ///
    /// If `fields` is not a JSON object.
    pub fn body(mut self, fields: Value) -> Self {
        let Value::Object(fields) = fields else {
            panic!("body expects a JSON object, got {fields}");
        };
        model::merge_into(&mut self.extra_body, &fields);
        self
    }

    /// Chooses which field carries the output token cap.
    ///
    /// Needed only when pointing [`ProviderDialect::OpenAiChat`] at an endpoint
    /// that has moved to `max_completion_tokens`, OpenAI itself being the one
    /// that has:
    ///
    /// ```
    /// use freyja::{ProviderConfig, ProviderDialect, TokenLimitField};
    ///
    /// let config = ProviderConfig::new(
    ///         ProviderDialect::OpenAiChat,
    ///         "OpenAI",
    ///         "https://api.openai.com/v1",
    ///     )
    ///     .token_limit_field(TokenLimitField::MaxCompletionTokens);
    /// ```
    pub fn token_limit_field(mut self, field: TokenLimitField) -> Self {
        self.token_limit_field = field;
        self
    }

    /// The full URL requests are sent to.
    pub fn url(&self) -> String {
        format!(
            "{}{}",
            self.base_url.trim_end_matches('/'),
            self.dialect.path()
        )
    }

    /// The full URL streaming requests are sent to.
    pub fn stream_url(&self) -> String {
        match self.dialect.stream_query() {
            Some(query) => format!("{}?{}", self.url(), query),
            None => self.url(),
        }
    }

    /// Resolves the model for a request, preferring the request's own choice.
    pub(crate) fn model_for(&self, request: &GenerateRequest) -> Result<String, ProviderError> {
        request
            .model
            .clone()
            .or_else(|| self.default_model.clone())
            .ok_or_else(|| ProviderError::InvalidRequest {
                provider: self.name.clone(),
                message: "no model set on the request and no default_model on the endpoint".into(),
            })
    }
}

/// One wire dialect: how to build its request body and read its response.
///
/// Transport is not part of this trait. Every dialect is POSTed the same way,
/// so [`Client`] owns that and implementors only convert.
pub trait Provider: Send + Sync {
    /// This dialect's request body type.
    type Request: Serialize + Send;

    /// Converts a neutral request into this dialect's wire format.
    ///
    /// Return [`ProviderError::UnsupportedCapability`] rather than dropping a
    /// field the format cannot express.
    fn build(
        &self,
        request: &GenerateRequest,
        config: &ProviderConfig,
    ) -> Result<Self::Request, ProviderError>;

    /// Parses a successful response body into the neutral model.
    fn parse(&self, body: &str, config: &ProviderConfig)
    -> Result<GenerateResponse, ProviderError>;
}

/// Runs `$body` with `$provider` bound to the [`Provider`] for a dialect.
///
/// The four arms live here and nowhere else. Written out longhand they appeared
/// three times — in [`Client::check`], [`Client::generate`], and
/// [`Client::stream`] — differing only in what each did with the provider, so a
/// fifth dialect meant finding all three. A `Provider` cannot be returned as
/// `dyn`, since `build` hands back an associated type, which is why this is a
/// macro rather than a function.
macro_rules! with_provider {
    ($dialect:expr, |$provider:ident| $body:expr) => {
        match $dialect {
            ProviderDialect::OpenAiResponses => {
                let $provider = openai_responses::OpenAiResponsesProvider;
                $body
            }
            ProviderDialect::OpenAiChat => {
                let $provider = openai_chat::OpenAiChatProvider;
                $body
            }
            ProviderDialect::Gemini => {
                let $provider = gemini::GeminiProvider;
                $body
            }
            ProviderDialect::Anthropic => {
                let $provider = anthropic::AnthropicProvider;
                $body
            }
        }
    };
}

/// The SSE decoder for a dialect.
///
/// Separate from [`with_provider`] because a decoder *is* returnable as `dyn`,
/// and because `stream` is the only caller that needs one.
fn decoder_for(dialect: ProviderDialect) -> Box<dyn stream::StreamDecoder> {
    match dialect {
        ProviderDialect::OpenAiResponses => Box::new(openai_responses::Decoder),
        ProviderDialect::OpenAiChat => Box::new(openai_chat::Decoder),
        ProviderDialect::Gemini => Box::new(gemini::Decoder::default()),
        ProviderDialect::Anthropic => Box::new(anthropic::Decoder::default()),
    }
}

/// The entry point: an endpoint plus its credentials and HTTP client.
///
/// Cloning is cheap for the HTTP client and config name but copies the API key;
/// prefer sharing one `Client` behind an `Arc` when you need it in several tasks.
///
/// ```no_run
/// # async fn run() -> Result<(), freyja::ProviderError> {
/// use freyja::{Client, GenerateRequest, Message, ProviderType, Role};
///
/// let client = Client::from_env(ProviderType::OpenAi).unwrap();
/// let response = client
///     .generate(&GenerateRequest::new().message(Message::text(Role::User, "Hi")))
///     .await?;
/// println!("{}", response.output_text());
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct Client {
    config: ProviderConfig,
    api_key: Option<String>,
    http: reqwest::Client,
}

/// Redacts the API key.
///
/// Deriving `Debug` here would print the key verbatim, so a single
/// `tracing::debug!(?client)` would put a live credential in the logs.
impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("config", &self.config)
            .field(
                "api_key",
                &match &self.api_key {
                    Some(_) => "<redacted>",
                    None => "<none>",
                },
            )
            .field("http", &self.http)
            .finish()
    }
}

impl Client {
    /// Creates a client with a pooled HTTP client and a 120 second inactivity
    /// timeout.
    ///
    /// The timeout bounds silence, not total duration, so a slow generation is
    /// not cut short. Use [`Client::with_http_client`] to impose a total cap.
    ///
    /// Accepts anything that converts into a [`ProviderConfig`], including a
    /// [`ProviderType`] preset.
    pub fn new(config: impl Into<ProviderConfig>, api_key: impl Into<String>) -> Self {
        Self::build(config.into(), Some(api_key.into()), default_http())
    }

    /// Creates a client for an endpoint Freyja does not ship, in one call.
    ///
    /// Shorthand for [`ProviderConfig::new`] followed by [`Client::new`]. Reach
    /// for the config builder directly when you also need a default model, a
    /// key variable, extra headers, or a non-conventional auth style.
    ///
    /// ```no_run
    /// use freyja::{Client, ProviderDialect};
    ///
    /// let client = Client::custom(
    ///     ProviderDialect::OpenAiChat,
    ///     "my-gateway",
    ///     "https://gateway.internal/v1",
    ///     std::env::var("GATEWAY_API_KEY").unwrap(),
    /// );
    /// ```
    pub fn custom(
        dialect: ProviderDialect,
        name: impl Into<Arc<str>>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self::new(ProviderConfig::new(dialect, name, base_url), api_key)
    }

    /// Creates a client for an endpoint that needs no credentials, such as a
    /// local Ollama or vLLM server.
    pub fn without_key(config: impl Into<ProviderConfig>) -> Self {
        Self::build(config.into(), None, default_http())
    }

    /// Creates a client over a caller-supplied HTTP client.
    ///
    /// Use this to control timeouts, proxies, TLS, or to share one pool with the
    /// rest of your application.
    pub fn with_http_client(
        config: impl Into<ProviderConfig>,
        api_key: impl Into<String>,
        http: reqwest::Client,
    ) -> Self {
        Self::build(config.into(), Some(api_key.into()), http)
    }

    fn build(config: ProviderConfig, api_key: Option<String>, http: reqwest::Client) -> Self {
        Self {
            config,
            api_key,
            http,
        }
    }

    /// Reads the API key from the endpoint's [`ProviderConfig::api_key_env`].
    ///
    /// Returns `None` when the endpoint names a variable that is unset or empty.
    /// An endpoint with [`Auth::None`] needs no key and always succeeds.
    pub fn from_env(config: impl Into<ProviderConfig>) -> Option<Self> {
        let config = config.into();
        if config.auth == Auth::None {
            return Some(Self::build(config, None, default_http()));
        }
        match std::env::var(config.api_key_env?) {
            Ok(key) if !key.is_empty() => Some(Self::build(config, Some(key), default_http())),
            _ => None,
        }
    }

    /// The endpoint this client talks to.
    pub fn config(&self) -> &ProviderConfig {
        &self.config
    }

    /// Decides whether this endpoint's dialect can carry a request, without
    /// sending it.
    ///
    /// Answers the question a capability table would answer, by running the
    /// conversion [`generate`](Self::generate) and [`stream`](Self::stream)
    /// run and discarding the result. It is the same code, so it cannot drift
    /// from what those two actually do, and it reports the real error rather
    /// than a bare `false`:
    ///
    /// ```
    /// use freyja::{
    ///     Client, GenerateRequest, Message, ProviderError, ProviderType, ReasoningEffort, Role,
    /// };
    ///
    /// let client = Client::custom(
    ///     ProviderType::Gemini.dialect(),
    ///     "gemini",
    ///     "https://generativelanguage.googleapis.com/v1beta",
    ///     "key",
    /// );
    /// let request = GenerateRequest::new()
    ///     .model("gemini-3.5-flash")
    ///     .message(Message::text(Role::User, "Hi"))
    ///     .reasoning_effort(ReasoningEffort::High);
    ///
    /// match client.check(&request) {
    ///     Ok(()) => { /* generate() will get as far as the network */ }
    ///     Err(ProviderError::UnsupportedCapability { capability, .. }) => {
    ///         assert_eq!(capability, "portable reasoning effort levels");
    ///     }
    ///     Err(error) => panic!("{error}"),
    /// }
    /// ```
    ///
    /// No network call, no credentials used, and cheap enough to run per
    /// request: it builds one wire body and drops it. The body is never even
    /// serialized to JSON, since that happens on the way to the socket.
    ///
    /// # What `Ok` does and does not promise
    ///
    /// `Ok` means the *dialect* can express this request — every field has
    /// somewhere to go in the wire format, and the transcript is shaped the way
    /// the format requires. It is not a promise that the endpoint will accept
    /// it. Freyja knows the wire format; it has never met your gateway, and
    /// will not claim to know what that gateway implements. A request that
    /// passes here can still come back [`ProviderError::BadRequest`].
    ///
    /// The model is invisible to it. `check` never reads the model name — it
    /// is copied into the body and not inspected — so asking a model that does
    /// no reasoning for a `reasoning_effort` passes here and is settled by the
    /// vendor. Wire formats change on the order of years and are documented;
    /// what a given model accepts changes constantly and differs on every
    /// compatible gateway, so a table of it would be confidently wrong within a
    /// month.
    ///
    /// An `Err` here is an error `generate` would have raised too, since the
    /// two run the same conversion. That makes the two agree; it does not make
    /// either correct. A refusal is a claim that this wire format has nowhere
    /// to put a field, and a claim can be wrong: Freyja refused
    /// `reasoning_effort` on Gemini for months, having looked for it at the top
    /// level of a request that keeps it under `generation_config`. Every
    /// refusal and the evidence behind it is listed in `src/provider/refusal.rs`.
    ///
    /// # Choosing a provider
    ///
    /// Because the answer depends on the request rather than on a fixed table,
    /// comparing endpoints means checking the same request against each:
    ///
    /// ```
    /// # use freyja::{Client, GenerateRequest, Message, ProviderType, Role, ToolChoice};
    /// # let request = GenerateRequest::new()
    /// #     .model("m")
    /// #     .message(Message::text(Role::User, "Hi"))
    /// #     .tool_choice(ToolChoice::Required);
    /// let usable: Vec<_> = [ProviderType::OpenAi, ProviderType::Gemini]
    ///     .into_iter()
    ///     .filter_map(|kind| Client::from_env(kind))
    ///     .filter(|client| client.check(&request).is_ok())
    ///     .collect();
    /// ```
    pub fn check(&self, request: &GenerateRequest) -> Result<(), ProviderError> {
        // `stream` builds through the same call before adding its streaming
        // flag, so one check covers both paths.
        with_provider!(self.config.dialect, |provider| provider
            .build(request, &self.config)
            .map(drop))
    }

    /// Sends a request and returns the normalized response.
    pub async fn generate(
        &self,
        request: &GenerateRequest,
    ) -> Result<GenerateResponse, ProviderError> {
        with_provider!(self.config.dialect, |provider| self
            .run(provider, request)
            .await)
    }

    /// Sends a request and deserializes the answer into `T`.
    ///
    /// The companion to [`ResponseFormat::JsonSchema`]: you constrain the
    /// model's output to a shape, and this hands back that shape instead of a
    /// string you parse yourself.
    ///
    /// ```no_run
    /// # async fn run(client: freyja::Client) -> Result<(), freyja::ProviderError> {
    /// use freyja::{GenerateRequest, Message, ResponseFormat, Role};
    /// use serde::Deserialize;
    ///
    /// #[derive(Deserialize)]
    /// struct Recommendation {
    ///     name: String,
    ///     purpose: String,
    /// }
    ///
    /// let request = GenerateRequest::new()
    ///     .message(Message::text(Role::User, "Recommend one Rust crate for JSON."))
    ///     .response_format(ResponseFormat::JsonSchema {
    ///         name: "recommendation".into(),
    ///         schema: serde_json::json!({
    ///             "type": "object",
    ///             "properties": {
    ///                 "name": {"type": "string"},
    ///                 "purpose": {"type": "string"}
    ///             },
    ///             "required": ["name", "purpose"],
    ///             "additionalProperties": false
    ///         }),
    ///         strict: true,
    ///     });
    ///
    /// let recommendation: Recommendation = client.generate_as(&request).await?;
    /// println!("{}", recommendation.name);
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// The schema is still written by hand, and must be kept in step with `T`
    /// yourself. Deriving one from the type is not implemented.
    ///
    /// # Errors
    ///
    /// Everything [`generate`](Self::generate) can raise, plus
    /// [`ProviderError::OutputMismatch`] when the call succeeded and the answer
    /// was not the shape you asked for. That error keeps the model's text so it
    /// can be logged or shown, and flags the truncation case separately —
    /// a cut-off JSON object is the most common reason this fails, and the fix
    /// is a larger [`GenerateRequest::max_tokens`] rather than a different
    /// schema.
    pub async fn generate_as<T: serde::de::DeserializeOwned>(
        &self,
        request: &GenerateRequest,
    ) -> Result<T, ProviderError> {
        let response = self.generate(request).await?;
        let text = response.output_text();

        serde_json::from_str(&text).map_err(|error| ProviderError::OutputMismatch {
            provider: self.config.name.clone(),
            message: error.to_string(),
            // Checked rather than inferred from the parse error: a truncated
            // answer and a wrong-shaped one both fail here, and only the
            // response itself can tell them apart.
            truncated: response.status == ResponseStatus::Incomplete,
            text,
        })
    }

    /// Opens a streaming generation.
    ///
    /// Returns once the provider has accepted the request, so a non-success
    /// status arrives here as [`ProviderError::Api`] rather than mid-stream.
    ///
    /// The default HTTP client bounds *inactivity*, not total duration, so a
    /// long generation is not cut short. A client supplied through
    /// [`Client::with_http_client`] keeps whatever it was built with — set
    /// `read_timeout` rather than `timeout` on it, or a long stream will be
    /// killed part-way.
    ///
    /// ```no_run
    /// # async fn run() -> Result<(), freyja::ProviderError> {
    /// use freyja::{Client, GenerateRequest, Message, ProviderType, Role, StreamEvent};
    ///
    /// let client = Client::from_env(ProviderType::OpenAi).unwrap();
    /// let request = GenerateRequest::new().message(Message::text(Role::User, "Hi"));
    ///
    /// let mut stream = client.stream(&request).await?;
    /// while let Some(event) = stream.next().await? {
    ///     if let StreamEvent::TextDelta(text) = event {
    ///         print!("{text}");
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn stream(&self, request: &GenerateRequest) -> Result<EventStream, ProviderError> {
        let wire = with_provider!(self.config.dialect, |provider| {
            let body = provider.build(request, &self.config)?.streaming();
            to_value(&body, request, &self.config)?
        });
        let decoder = decoder_for(self.config.dialect);

        let response = self.post(self.config.stream_url(), &wire).await?;

        let status = response.status();
        if !status.is_success() {
            // Read before `text()`, which consumes the response and takes the
            // headers with it.
            let retry_after = parse_retry_after(response.headers());
            let body = response
                .text()
                .await
                .map_err(|error| ProviderError::transport(self.config.name.clone(), &error))?;
            return Err(ProviderError::from_status(
                self.config.name.clone(),
                status.as_u16(),
                retry_after,
                body,
            ));
        }

        Ok(EventStream::new(
            self.config.name.clone(),
            decoder,
            response,
        ))
    }

    /// Convert, POST, check status, parse. Shared by every dialect, which is
    /// why none of them owns transport code.
    async fn run<P: Provider>(
        &self,
        provider: P,
        request: &GenerateRequest,
    ) -> Result<GenerateResponse, ProviderError> {
        let wire = provider.build(request, &self.config)?;
        let body = to_value(&wire, request, &self.config)?;
        let response = self.post(self.config.url(), &body).await?;

        let status = response.status();
        let retry_after = parse_retry_after(response.headers());
        let body = response
            .text()
            .await
            .map_err(|error| ProviderError::transport(self.config.name.clone(), &error))?;

        if !status.is_success() {
            return Err(ProviderError::from_status(
                self.config.name.clone(),
                status.as_u16(),
                retry_after,
                body,
            ));
        }

        provider.parse(&body, &self.config)
    }

    /// Sends one POST with this endpoint's headers and credentials.
    async fn post<T: Serialize>(
        &self,
        url: String,
        wire: &T,
    ) -> Result<reqwest::Response, ProviderError> {
        let mut post = self.http.post(url);
        for (name, value) in self.config.dialect.required_headers() {
            post = post.header(*name, *value);
        }
        for (name, value) in &self.config.extra_headers {
            post = post.header(name, value);
        }
        if let Some(key) = &self.api_key {
            post = match self.config.auth {
                Auth::Bearer => post.bearer_auth(key),
                Auth::Header(name) => post.header(name, key),
                Auth::None => post,
            };
        }

        post.json(wire)
            .send()
            .await
            .map_err(|error| ProviderError::transport(self.config.name.clone(), &error))
    }
}

/// Read `Retry-After` as a delay, so a caller can honour the endpoint's own
/// pacing instead of guessing.
///
/// The header has two forms. Only delay-seconds is parsed: the HTTP-date form
/// would need a clock and a date parser, neither of which Freyja has a
/// dependency for, and no major vendor sends it. An unreadable or absent header
/// is `None`, which leaves the caller's own backoff in charge.
fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

/// The pooled client [`Client::new`] and its siblings build.
///
/// Panics rather than falling back: the only way `build` fails here is a TLS
/// backend that will not initialize, and `reqwest::Client::default()` panics on
/// exactly that. Falling back would trade the panic for a client with no
/// `read_timeout`, quietly breaking the bound [`Client::new`] documents.
/// [`Client::with_http_client`] is the way past a failure worth surviving.
fn default_http() -> reqwest::Client {
    reqwest::Client::builder()
        .read_timeout(DEFAULT_TIMEOUT)
        .build()
        .expect("the TLS backend could not be initialized")
}

/// Serializes a dialect's request and merges the escape hatches into it.
///
/// Erasing the request type is also what lets `stream` pick a decoder and a
/// body in one `match` without four near-identical arms after it.
///
/// Precedence runs from general to specific: what the dialect built, then
/// [`ProviderConfig::extra_body`], then [`GenerateRequest::extra_for`] entries
/// for this dialect in the order they were added. Freyja's own fields are the
/// weakest, because a caller reaching for an escape hatch has said what they
/// want more plainly than the neutral model could.
fn to_value<T: Serialize>(
    wire: &T,
    request: &GenerateRequest,
    config: &ProviderConfig,
) -> Result<Value, ProviderError> {
    let invalid = |error: serde_json::Error| ProviderError::InvalidRequest {
        provider: config.name.clone(),
        message: error.to_string(),
    };

    let mut body = match serde_json::to_value(wire).map_err(invalid)? {
        Value::Object(body) => body,
        // Unreachable: every dialect's request type is a struct. Reported
        // rather than unwrapped so a future one that is not says so.
        other => {
            return Err(ProviderError::InvalidRequest {
                provider: config.name.clone(),
                message: format!("request body must be a JSON object, built {other}"),
            });
        }
    };

    model::merge_into(&mut body, &config.extra_body);
    for (dialect, fields) in &request.extra {
        if *dialect == config.dialect {
            model::merge_into(&mut body, fields);
        }
    }

    Ok(Value::Object(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_prints_the_api_key() {
        let client = Client::new(ProviderType::OpenAi, "sk-secret-value");
        let rendered = format!("{client:?}");

        assert!(!rendered.contains("sk-secret-value"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");

        // The distinction between "no key" and "a key I will not show you"
        // stays visible, since it is the usual cause of a 401.
        let local = ProviderConfig::new(
            ProviderDialect::OpenAiChat,
            "local",
            "http://localhost:11434/v1",
        );
        let keyless = Client::without_key(local);
        assert!(format!("{keyless:?}").contains("<none>"));
    }

    #[test]
    fn debug_redacts_a_credential_left_in_an_extra_header() {
        // The client redacts its own key, then printed its config verbatim one
        // field over. A gateway wanting a second credential has nowhere else to
        // put it, so the redaction has to reach here too.
        let config = ProviderConfig::new(ProviderDialect::OpenAiChat, "gw", "https://gw.test/v1")
            .header("x-gateway-token", "tok-secret-value")
            .header("Authorization", "Bearer sk-second-secret")
            .header("x-trace-id", "abc123");

        let rendered = format!("{:?}", Client::new(config, "sk-primary"));

        assert!(!rendered.contains("tok-secret-value"), "{rendered}");
        assert!(!rendered.contains("sk-second-secret"), "{rendered}");
        assert!(!rendered.contains("sk-primary"), "{rendered}");

        // Names always show, and a routing hint stays readable: redacting
        // everything would make the field useless for the job it exists for.
        assert!(rendered.contains("x-gateway-token"), "{rendered}");
        assert!(rendered.contains("abc123"), "{rendered}");
    }

    #[test]
    fn custom_builds_an_endpoint_freyja_does_not_ship() {
        let client = Client::custom(
            ProviderDialect::OpenAiChat,
            "my-gateway",
            "https://gateway.internal/v1",
            "key",
        );

        assert_eq!(client.config().dialect, ProviderDialect::OpenAiChat);
        assert_eq!(&*client.config().name, "my-gateway");
        assert_eq!(
            client.config().url(),
            "https://gateway.internal/v1/chat/completions"
        );
        // Auth follows the dialect unless overridden.
        assert_eq!(client.config().auth, Auth::Bearer);
    }

    #[test]
    fn joins_base_url_and_dialect_path() {
        let config = ProviderConfig::new(ProviderDialect::Anthropic, "test", "https://x.test/v1");
        assert_eq!(config.url(), "https://x.test/v1/messages");

        let trailing = ProviderConfig::new(
            ProviderDialect::OpenAiResponses,
            "test",
            "https://x.test/v1/",
        );
        assert_eq!(trailing.url(), "https://x.test/v1/responses");
    }

    #[test]
    fn takes_the_dialects_auth_style_by_default() {
        let anthropic = ProviderConfig::new(ProviderDialect::Anthropic, "a", "https://x.test");
        assert_eq!(anthropic.auth, Auth::Header("x-api-key"));

        let overridden = anthropic.auth(Auth::Bearer);
        assert_eq!(overridden.auth, Auth::Bearer);
    }

    #[test]
    fn resolves_the_model_from_request_then_endpoint() {
        let config = ProviderConfig::new(ProviderDialect::Anthropic, "a", "https://x.test")
            .default_model("endpoint-default");

        let empty = GenerateRequest::new();
        assert_eq!(config.model_for(&empty).unwrap(), "endpoint-default");

        let explicit = GenerateRequest::new().model("caller-choice");
        assert_eq!(config.model_for(&explicit).unwrap(), "caller-choice");
    }

    #[test]
    fn refuses_a_request_with_no_model_anywhere() {
        let config = ProviderConfig::new(ProviderDialect::Anthropic, "a", "https://x.test");

        assert!(matches!(
            config.model_for(&GenerateRequest::new()),
            Err(ProviderError::InvalidRequest { .. })
        ));
    }

    #[test]
    fn stream_url_appends_alt_sse_for_gemini() {
        let gemini = ProviderConfig::new(ProviderDialect::Gemini, "g", "https://x.test/v1");
        assert_eq!(gemini.url(), "https://x.test/v1/interactions");
        assert_eq!(
            gemini.stream_url(),
            "https://x.test/v1/interactions?alt=sse",
            "Gemini selects SSE by query parameter, not by body field alone"
        );

        // Every other dialect streams from the same URL it generates from.
        let anthropic = ProviderConfig::new(ProviderDialect::Anthropic, "a", "https://x.test/v1");
        assert_eq!(anthropic.stream_url(), anthropic.url());
    }

    const DIALECTS: [ProviderDialect; 4] = [
        ProviderDialect::OpenAiResponses,
        ProviderDialect::OpenAiChat,
        ProviderDialect::Gemini,
        ProviderDialect::Anthropic,
    ];

    /// A client that could never reach anything, which is the point: `check`
    /// must not need a reachable endpoint or a real key.
    fn offline(dialect: ProviderDialect) -> Client {
        Client::custom(dialect, "test", "http://127.0.0.1:1/v1", "unused")
    }

    fn ask() -> GenerateRequest {
        GenerateRequest::new()
            .model("m")
            .message(Message::text(Role::User, "Hi"))
    }

    /// The body a dialect would post, escape hatches merged in.
    fn body_for(config: &ProviderConfig, request: &GenerateRequest) -> Value {
        let wire = gemini::GeminiProvider.build(request, config).unwrap();
        to_value(&wire, request, config).unwrap()
    }

    fn gemini_config() -> ProviderConfig {
        ProviderConfig::new(ProviderDialect::Gemini, "Gemini", "https://x.test/v1")
    }

    #[test]
    fn extra_fields_merge_into_a_nested_object_without_clearing_it() {
        // The whole reason the merge is deep. `generation_config` already has
        // Freyja's cap in it, and adding a seed must not take the cap with it.
        let request = ask().max_tokens(64).extra_for(
            ProviderDialect::Gemini,
            serde_json::json!({"generation_config": {"seed": 42}}),
        );

        let config = &body_for(&gemini_config(), &request)["generation_config"];

        assert_eq!(config["max_output_tokens"], 64);
        assert_eq!(config["seed"], 42);
    }

    #[test]
    fn extra_fields_are_ignored_by_every_other_dialect() {
        // What keeps a request portable. The same one still runs against
        // OpenAI, which never sees a field meant for Gemini.
        let request = ask().extra_for(
            ProviderDialect::Gemini,
            serde_json::json!({"generation_config": {"seed": 42}}),
        );

        let openai =
            ProviderConfig::new(ProviderDialect::OpenAiChat, "OpenAI", "https://x.test/v1");
        let wire = openai_chat::OpenAiChatProvider
            .build(&request, &openai)
            .unwrap();
        let body = to_value(&wire, &request, &openai).unwrap();

        assert!(body.get("generation_config").is_none());
        assert!(
            openai_chat::OpenAiChatProvider
                .build(&request, &openai)
                .is_ok()
        );
    }

    #[test]
    fn a_request_hatch_beats_an_endpoint_one_and_both_beat_the_dialect() {
        // General to specific: what the dialect built, then the endpoint's
        // standing fields, then this call's.
        let config = gemini_config().body(serde_json::json!({
            "generation_config": {"seed": 1, "candidate_count": 2},
            "model": "from-the-endpoint",
        }));
        let request = ask().extra_for(
            ProviderDialect::Gemini,
            serde_json::json!({"generation_config": {"seed": 99}}),
        );

        let body = body_for(&config, &request);

        assert_eq!(body["generation_config"]["seed"], 99, "the call wins");
        assert_eq!(
            body["generation_config"]["candidate_count"], 2,
            "and does not clear what it did not mention"
        );
        assert_eq!(
            body["model"], "from-the-endpoint",
            "a hatch outranks the dialect"
        );
    }

    #[test]
    fn later_extra_calls_win_over_earlier_ones() {
        let request = ask()
            .extra_for(ProviderDialect::Gemini, serde_json::json!({"seed": 1}))
            .extra_for(ProviderDialect::Gemini, serde_json::json!({"seed": 2}));

        assert_eq!(body_for(&gemini_config(), &request)["seed"], 2);
    }

    #[test]
    fn check_says_nothing_about_extra_fields() {
        // It reports what the format can carry, and this is by definition
        // outside what Freyja knows about the format. A wrong key is the
        // endpoint's to reject.
        let request = ask().extra_for(
            ProviderDialect::Gemini,
            serde_json::json!({"not_a_real_parameter": true}),
        );

        assert!(offline(ProviderDialect::Gemini).check(&request).is_ok());
    }

    /// The capability a check refused, or a panic naming what happened instead.
    fn refusal(dialect: ProviderDialect, request: &GenerateRequest) -> &'static str {
        match offline(dialect).check(request) {
            Err(ProviderError::UnsupportedCapability { capability, .. }) => capability,
            other => panic!("{dialect:?}: expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn check_accepts_what_every_dialect_can_carry() {
        for dialect in DIALECTS {
            assert!(
                offline(dialect).check(&ask()).is_ok(),
                "{dialect:?} should carry a plain text request"
            );
        }
    }

    #[test]
    fn check_reports_a_field_the_dialect_cannot_express() {
        assert_eq!(
            refusal(
                ProviderDialect::Anthropic,
                &ask().previous_response_id("resp_1")
            ),
            "server-side conversation continuation"
        );
        assert_eq!(
            refusal(
                ProviderDialect::OpenAiChat,
                &ask().previous_response_id("chatcmpl-1")
            ),
            "server-side conversation continuation"
        );
    }

    #[test]
    fn check_reports_a_value_the_dialect_cannot_express() {
        // The case a struct of booleans gets wrong. Anthropic supports
        // response_format and refuses one value of it, so
        // `response_format: true` would be a true answer to the wrong
        // question.
        let anthropic = offline(ProviderDialect::Anthropic);

        assert!(
            anthropic
                .check(&ask().response_format(ResponseFormat::JsonSchema {
                    name: "s".into(),
                    schema: serde_json::json!({"type": "object"}),
                    strict: true,
                }))
                .is_ok(),
        );
        assert_eq!(
            refusal(
                ProviderDialect::Anthropic,
                &ask().response_format(ResponseFormat::JsonObject)
            ),
            "schema-less JSON response format"
        );

        // And the counter-example, which is the more important half. Gemini
        // rejects `thinking_level: max` and rejects `labels` outright, and
        // `check` passes both: the fields exist, so what the endpoint does with
        // them is the endpoint's answer to give. `check` reports what the
        // format can carry, never what the deployment will accept.
        let gemini = offline(ProviderDialect::Gemini);

        for request in [
            ask().reasoning_effort(ReasoningEffort::Max),
            ask().metadata(serde_json::json!({"trace": "abc"})),
        ] {
            assert!(
                gemini.check(&request).is_ok(),
                "a value the endpoint refuses is not a capability Freyja lacks",
            );
        }
    }

    #[test]
    fn check_reports_a_placement_no_table_could_describe() {
        // Not a property of the vendor at all: the same image is fine one turn
        // earlier. Only something that sees the request can answer it.
        let misplaced = GenerateRequest::new()
            .model("m")
            .message(Message::new(
                Role::Assistant,
                [InputContent::ImageUrl("https://e.test/a.png".into())],
            ))
            .message(Message::text(Role::User, "What is this?"));

        // Two of the four, not all four. The rule was written once and applied
        // everywhere; probing found Chat Completions takes an image on any role
        // and Gemini takes one on a `model_output` step, which leaves the
        // refusal true only where an assistant turn is a closed content set.
        for dialect in [ProviderDialect::OpenAiResponses, ProviderDialect::Anthropic] {
            assert_eq!(
                refusal(dialect, &misplaced),
                "images outside user messages",
                "{dialect:?}"
            );
        }

        for dialect in [ProviderDialect::OpenAiChat, ProviderDialect::Gemini] {
            assert!(
                offline(dialect).check(&misplaced).is_ok(),
                "{dialect:?} carries this",
            );
        }

        let placed = GenerateRequest::new().model("m").message(Message::new(
            Role::User,
            [
                InputContent::ImageUrl("https://e.test/a.png".into()),
                InputContent::Text("What is this?".into()),
            ],
        ));

        for dialect in DIALECTS {
            assert!(offline(dialect).check(&placed).is_ok(), "{dialect:?}");
        }
    }

    #[test]
    fn check_catches_everything_decidable_before_the_network() {
        // Not a capability: the request names no model and the endpoint has no
        // default. `generate` raises this too, so `check` reporting it is the
        // honest behaviour rather than scope creep.
        let no_model = GenerateRequest::new().message(Message::text(Role::User, "Hi"));

        for dialect in DIALECTS {
            assert!(
                matches!(
                    offline(dialect).check(&no_model),
                    Err(ProviderError::InvalidRequest { .. })
                ),
                "{dialect:?}"
            );
        }
    }

    #[tokio::test]
    async fn check_agrees_with_the_path_it_stands_in_for() {
        // The whole argument for building rather than tabulating: `check` runs
        // the same conversion, so it cannot drift. `stream` converts before it
        // opens a socket, which is why this reaches a verdict against an
        // endpoint that does not exist.
        let refused = ask().response_format(ResponseFormat::JsonObject);
        let client = offline(ProviderDialect::Anthropic);

        let from_check = client.check(&refused).expect_err("check refuses");
        let from_stream = match client.stream(&refused).await {
            Ok(_) => panic!("a refused request must not open a stream"),
            Err(error) => error,
        };

        assert_eq!(from_check.to_string(), from_stream.to_string());
    }

    fn headers_with(value: &str) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::RETRY_AFTER,
            reqwest::header::HeaderValue::from_str(value).expect("valid header value"),
        );
        headers
    }

    #[test]
    fn retry_after_reads_the_delay_seconds_form() {
        assert_eq!(
            parse_retry_after(&headers_with("30")),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            parse_retry_after(&headers_with(" 5 ")),
            Some(Duration::from_secs(5))
        );
    }

    #[test]
    fn an_unusable_retry_after_leaves_the_caller_in_charge() {
        // The HTTP-date form would need a clock and a date parser. Reporting
        // `None` hands pacing back to the caller's own backoff, which is
        // correct; guessing a duration would not be.
        assert_eq!(
            parse_retry_after(&headers_with("Wed, 21 Oct 2015 07:28:00 GMT")),
            None
        );
        assert_eq!(parse_retry_after(&headers_with("soon")), None);
        assert_eq!(parse_retry_after(&headers_with("-1")), None);
        assert_eq!(parse_retry_after(&reqwest::header::HeaderMap::new()), None);
    }
}
