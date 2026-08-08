//! Provider abstraction: one neutral model, many vendor backends.

pub(crate) mod anthropic;
pub(crate) mod gemini;
pub(crate) mod openai;

mod model;

pub use model::*;

use std::future::Future;
use std::time::Duration;

use anthropic::AnthropicProvider;
use gemini::GeminiProvider;
use openai::OpenAiProvider;

/// Default per-request timeout applied by [`Client::new`].
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// A vendor backend that can serve a [`GenerateRequest`].
///
/// Implementors own their wire format and are responsible for converting in
/// both directions. When a request asks for something the vendor cannot express,
/// return [`ProviderError::UnsupportedCapability`] rather than degrading it.
///
/// The HTTP client is passed in rather than owned so that every request in a
/// process shares one connection pool.
pub trait Provider: Send + Sync {
    /// Sends `request` to the vendor and normalizes the answer.
    fn generate(
        &self,
        http: &reqwest::Client,
        api_key: &str,
        request: &GenerateRequest,
    ) -> impl Future<Output = Result<GenerateResponse, ProviderError>> + Send;
}

/// Which vendor backend to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderType {
    /// OpenAI, via the Responses API.
    OpenAi,
    /// Google Gemini, via the Interactions API.
    Gemini,
    /// Anthropic Claude, via the Messages API.
    Anthropic,
}

impl ProviderType {
    /// The conventional environment variable holding this provider's API key.
    pub fn api_key_env(self) -> &'static str {
        match self {
            Self::OpenAi => "OPENAI_API_KEY",
            Self::Gemini => "GEMINI_API_KEY",
            Self::Anthropic => "ANTHROPIC_API_KEY",
        }
    }
}

/// The entry point: a provider plus its credentials and HTTP client.
///
/// Cloning is cheap for the HTTP client but copies the API key; prefer sharing
/// one `Client` behind an `Arc` when you need it in several tasks.
///
/// ```no_run
/// # async fn run() -> Result<(), freya::ProviderError> {
/// use freya::{Client, GenerateRequest, Message, ProviderType, Role};
///
/// let client = Client::new(ProviderType::OpenAi, std::env::var("OPENAI_API_KEY").unwrap());
/// let response = client
///     .generate(&GenerateRequest::new().message(Message::text(Role::User, "Hi")))
///     .await?;
/// println!("{}", response.output_text());
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct Client {
    provider: ProviderType,
    api_key: String,
    http: reqwest::Client,
}

impl Client {
    /// Creates a client with a pooled HTTP client and a 120 second timeout.
    pub fn new(provider: ProviderType, api_key: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self::with_http_client(provider, api_key, http)
    }

    /// Creates a client over a caller-supplied HTTP client.
    ///
    /// Use this to control timeouts, proxies, TLS, or to share one pool with the
    /// rest of your application.
    pub fn with_http_client(
        provider: ProviderType,
        api_key: impl Into<String>,
        http: reqwest::Client,
    ) -> Self {
        Self {
            provider,
            api_key: api_key.into(),
            http,
        }
    }

    /// Reads this client's API key from [`ProviderType::api_key_env`].
    ///
    /// Returns `None` when the variable is unset or empty.
    pub fn from_env(provider: ProviderType) -> Option<Self> {
        match std::env::var(provider.api_key_env()) {
            Ok(key) if !key.is_empty() => Some(Self::new(provider, key)),
            _ => None,
        }
    }

    /// The provider this client talks to.
    pub fn provider(&self) -> ProviderType {
        self.provider
    }

    /// Sends a request and returns the normalized response.
    pub async fn generate(
        &self,
        request: &GenerateRequest,
    ) -> Result<GenerateResponse, ProviderError> {
        match self.provider {
            ProviderType::OpenAi => {
                OpenAiProvider
                    .generate(&self.http, &self.api_key, request)
                    .await
            }
            ProviderType::Gemini => {
                GeminiProvider
                    .generate(&self.http, &self.api_key, request)
                    .await
            }
            ProviderType::Anthropic => {
                AnthropicProvider
                    .generate(&self.http, &self.api_key, request)
                    .await
            }
        }
    }
}
