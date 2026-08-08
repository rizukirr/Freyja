//! Provider abstraction: one neutral model, several wire dialects, many endpoints.
//!
//! Freya separates *how* a request is serialized from *where* it is sent. A
//! [`ProviderDialect`] is a wire format, and a [`ProviderConfig`] is an endpoint
//! that speaks one. That split is what lets a single dialect serve a dozen
//! vendors: most hosted inference APIs are wire-compatible with either OpenAI or
//! Anthropic, and differ only in base URL, credentials, and model names.
//!
//! Reach for [`ProviderType`] first, it names the endpoints Freya knows about.
//! Drop to [`ProviderConfig`] when yours is not on that list.

pub(crate) mod anthropic;
pub(crate) mod gemini;
pub(crate) mod openai_chat;
pub(crate) mod openai_responses;

mod model;
mod presets;

pub use model::*;
pub use presets::ProviderType;

use serde::Serialize;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

/// Default per-request timeout applied by [`Client::new`].
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

/// An endpoint: where to send a request, how to authenticate, and what to
/// default when the caller does not say.
///
/// ```
/// use freya::{Auth, ProviderConfig, ProviderDialect};
///
/// let config = ProviderConfig::new(ProviderDialect::Anthropic, "Z.ai", "https://api.z.ai/api/anthropic/v1")
///     .api_key_env("ZAI_API_KEY")
///     .default_model("glm-4.6");
/// assert_eq!(config.auth, Auth::Header("x-api-key"));
/// ```
#[derive(Debug, Clone)]
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
    pub extra_headers: Vec<(String, String)>,
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
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_headers.push((name.into(), value.into()));
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

/// The entry point: an endpoint plus its credentials and HTTP client.
///
/// Cloning is cheap for the HTTP client and config name but copies the API key;
/// prefer sharing one `Client` behind an `Arc` when you need it in several tasks.
///
/// ```no_run
/// # async fn run() -> Result<(), freya::ProviderError> {
/// use freya::{Client, GenerateRequest, Message, ProviderType, Role};
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
    /// Creates a client with a pooled HTTP client and a 120 second timeout.
    ///
    /// Accepts anything that converts into a [`ProviderConfig`], including a
    /// [`ProviderType`] preset.
    pub fn new(config: impl Into<ProviderConfig>, api_key: impl Into<String>) -> Self {
        Self::build(config.into(), Some(api_key.into()), default_http())
    }

    /// Creates a client for an endpoint Freya does not ship, in one call.
    ///
    /// Shorthand for [`ProviderConfig::new`] followed by [`Client::new`]. Reach
    /// for the config builder directly when you also need a default model, a
    /// key variable, extra headers, or a non-conventional auth style.
    ///
    /// ```no_run
    /// use freya::{Client, ProviderDialect};
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

    /// Sends a request and returns the normalized response.
    pub async fn generate(
        &self,
        request: &GenerateRequest,
    ) -> Result<GenerateResponse, ProviderError> {
        match self.config.dialect {
            ProviderDialect::OpenAiResponses => {
                self.run(openai_responses::OpenAiResponsesProvider, request)
                    .await
            }
            ProviderDialect::OpenAiChat => self.run(openai_chat::OpenAiChatProvider, request).await,
            ProviderDialect::Gemini => self.run(gemini::GeminiProvider, request).await,
            ProviderDialect::Anthropic => self.run(anthropic::AnthropicProvider, request).await,
        }
    }

    /// Convert, POST, check status, parse. Shared by every dialect, which is
    /// why none of them owns transport code.
    async fn run<P: Provider>(
        &self,
        provider: P,
        request: &GenerateRequest,
    ) -> Result<GenerateResponse, ProviderError> {
        let wire = provider.build(request, &self.config)?;

        let mut post = self.http.post(self.config.url());
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

        let response = post
            .json(&wire)
            .send()
            .await
            .map_err(|error| ProviderError::Http(error.to_string()))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| ProviderError::Http(error.to_string()))?;

        if !status.is_success() {
            return Err(ProviderError::Api {
                provider: self.config.name.clone(),
                status: status.as_u16(),
                body,
            });
        }

        provider.parse(&body, &self.config)
    }
}

fn default_http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(DEFAULT_TIMEOUT)
        .build()
        .unwrap_or_default()
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
    fn custom_builds_an_endpoint_freya_does_not_ship() {
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
}
