//! Endpoint configuration and maintained endpoint presets.

mod presets;

pub use presets::EndpointPreset;

use crate::dialect::Dialect;
use crate::error::Error;
use crate::model::GenerateRequest;
use serde_json::Value;
use std::fmt;
use std::sync::Arc;

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
/// Only the [`Dialect::OpenAiChat`] dialect reads this. The other three
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
/// use freyja::{Auth, EndpointConfig, Dialect};
///
/// let config = EndpointConfig::new(Dialect::Anthropic, "Z.ai", "https://api.z.ai/api/anthropic/v1")
///     .api_key_env("ZAI_API_KEY")
///     .default_model("glm-4.6");
/// assert_eq!(config.auth, Auth::Header("x-api-key"));
/// ```
#[derive(Clone)]
pub struct EndpointConfig {
    /// Which wire format this endpoint speaks.
    pub dialect: Dialect,
    /// Display name, used for error attribution so a Groq failure does not
    /// report itself as OpenAI.
    pub name: Arc<str>,
    /// Root URL. [`Dialect::path`] is appended to it.
    pub base_url: String,
    /// How to present credentials.
    pub auth: Auth,
    /// Environment variable holding the key, for [`crate::Client::from_env`].
    pub api_key_env: Option<&'static str>,
    /// Model used when [`GenerateRequest::model`] is unset.
    ///
    /// There is no library-wide default, because a model name is only
    /// meaningful on the endpoint serving it.
    pub default_model: Option<String>,
    /// Extra headers, for gateways that want attribution or routing hints.
    ///
    /// Reach for [`EndpointConfig::auth`] to carry a credential, not this: the
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
    /// Read only by the [`Dialect::OpenAiChat`] dialect, and defaulted
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
/// [`crate::Client`] takes care of the API key, but it prints its config, and a
/// gateway needing a second credential has nowhere to put it but
/// [`EndpointConfig::extra_headers`]. A derived `Debug` would print that
/// verbatim and undo the redaction one field over.
///
/// Names are always shown, and a value is withheld only when its name contains
/// `auth`, `key`, `token`, `secret`, `cookie`, or `password`, so a routing hint
/// stays as readable as it was. A heuristic, and stated as one: it cannot know
/// that `x-acme-passport` is a credential. Put credentials in
/// [`EndpointConfig::auth`], which is redacted whatever it is called.
impl fmt::Debug for EndpointConfig {
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

        f.debug_struct("EndpointConfig")
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

impl EndpointConfig {
    /// Creates a config with the dialect's conventional auth style.
    pub fn new(dialect: Dialect, name: impl Into<Arc<str>>, base_url: impl Into<String>) -> Self {
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
    /// [`EndpointConfig::auth`], which is redacted unconditionally; a value
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
    /// use freyja::{EndpointConfig, Dialect};
    /// use serde_json::json;
    ///
    /// let config = EndpointConfig::new(
    ///     Dialect::Gemini,
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
        crate::model::merge_into(&mut self.extra_body, &fields);
        self
    }

    /// Chooses which field carries the output token cap.
    ///
    /// Needed only when pointing [`Dialect::OpenAiChat`] at an endpoint
    /// that has moved to `max_completion_tokens`, OpenAI itself being the one
    /// that has:
    ///
    /// ```
    /// use freyja::{EndpointConfig, Dialect, TokenLimitField};
    ///
    /// let config = EndpointConfig::new(
    ///         Dialect::OpenAiChat,
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
    pub(crate) fn model_for(&self, request: &GenerateRequest) -> Result<String, Error> {
        request
            .model
            .clone()
            .or_else(|| self.default_model.clone())
            .ok_or_else(|| Error::InvalidRequest {
                endpoint: self.name.clone(),
                message: "no model set on the request and no default_model on the endpoint".into(),
            })
    }
}
