//! Endpoint configuration and maintained endpoint presets.

mod presets;

pub use presets::EndpointPreset;

use crate::dialect::Dialect;
use crate::error::Error;
use crate::model::GenerateRequest;
use serde_json::Value;
use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

/// How an endpoint expects credentials to be presented.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Auth {
    /// `Authorization: Bearer <key>`.
    Bearer,
    /// A named header carrying the raw key, such as `x-api-key`.
    Header(&'static str),
    /// A named URL query parameter carrying the raw key, such as `?key=<key>`.
    ///
    /// For endpoints that take their credential in the URL rather than in a
    /// header. The key is added when the request is sent, not when the URL is
    /// built, so [`EndpointConfig::url`] stays safe to print: the URL a request
    /// actually goes to differs from what `url()` reports by this one
    /// parameter, and that is the trade deliberately made.
    ///
    /// An [`EndpointConfig::query`] entry of the same name is replaced, so the
    /// credential wins a collision the way header auth does.
    Query(&'static str),
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
    /// Path appended to `base_url`, replacing [`Dialect::path`] when set.
    ///
    /// For an endpoint that does not follow its dialect's convention, such as
    /// a deployment-scoped Azure URL. A caller setting this owns the whole
    /// path: Freyja does not check that it agrees with the dialect, because
    /// the paths that need this option agree with nothing.
    pub path: Option<String>,
    /// Query parameters sent with every request, percent-encoded by Freyja.
    ///
    /// For what a deployment pins on every call — an API version, a tenant, a
    /// region. Pairs rather than a query string so the joining and the
    /// escaping are not the caller's problem.
    ///
    /// Redacted in `Debug` on the same name heuristic as `extra_headers`, and
    /// with the same advice: a credential belongs in [`EndpointConfig::auth`].
    pub query: Vec<(String, String)>,
    /// Header names the caller has classified as credentials, lowercased.
    ///
    /// Lowercased because a header name is case-insensitive, so `X-Acme-Key`
    /// and `x-acme-key` are one name and a set that told them apart would
    /// answer differently about one value depending on how it was spelled.
    /// [`EndpointConfig::extra_headers`] keeps what the caller wrote, since
    /// that is what goes on the wire.
    ///
    /// [`EndpointConfig::secret_query`] is not lowercased, because a query
    /// parameter name is case-sensitive and `Key` and `key` really are two
    /// parameters.
    ///
    /// Populated by [`EndpointConfig::secret_header`]. Kept apart from
    /// [`EndpointConfig::secret_query`] so classifying a name in one place
    /// says nothing about the other: a header and a query parameter that
    /// happen to share a name are different values.
    ///
    /// The classification is what Freyja prints by, not what it sends by: a
    /// classified value goes on the wire exactly as an unclassified one does.
    ///
    /// Membership here is narrower than being withheld, because the name
    /// heuristic withholds some values nobody classified.
    /// [`EndpointConfig::is_secret_header`] is the whole question.
    pub secret_headers: HashSet<String>,
    /// Query parameter names the caller has classified as credentials.
    ///
    /// The companion to [`EndpointConfig::secret_headers`], populated by
    /// [`EndpointConfig::secret_query`]. [`EndpointConfig::is_secret_query`]
    /// is the whole question, and it counts one source this one does not: the
    /// parameter [`EndpointConfig::auth`] uses under [`Auth::Query`].
    pub secret_query: HashSet<String>,
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

/// Whether [`EndpointConfig::build_url`] prints a credential-shaped query
/// value or withholds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reveal {
    /// Build the URL a request goes to.
    Yes,
    /// Build a URL that is safe to print.
    No,
}

/// What a withheld query value reads as in [`EndpointConfig::url`].
///
/// Bare rather than the `<redacted>` used in `Debug`, and for the reason
/// [`crate::error`] gives for the same choice: a query value is percent-encoded
/// on the way in, so `%3Credacted%3E` is harder to read than the thing it
/// stands for, and the result still has to parse as a URL.
const REDACTED_QUERY_VALUE: &str = "REDACTED";

/// Substrings that mark a header or query parameter as carrying a secret.
///
/// Matched case-insensitively against the *name*. A heuristic, and named as
/// one: it cannot know that `x-acme-passport` is a credential.
const SECRET_NAME_MARKERS: [&str; 6] = ["auth", "key", "token", "secret", "cookie", "password"];

/// Whether a header or query parameter's value should be withheld from `Debug`.
pub(crate) fn is_secret_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    SECRET_NAME_MARKERS
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
/// Names are always shown. A value is withheld when the caller classified it
/// with [`EndpointConfig::secret_header`] or [`EndpointConfig::secret_query`],
/// when it is the query parameter this endpoint's [`EndpointConfig::auth`]
/// uses, or when its name contains `auth`, `key`, `token`, `secret`, `cookie`
/// or `password`. The last of those is a heuristic and stated as one: it
/// cannot know that `x-acme-passport` is a credential, which is why the first
/// two exist. Headers and query parameters are asked separately, so
/// classifying one name says nothing about the other. None of them look inside
/// [`EndpointConfig::base_url`], which prints whatever was put there.
impl fmt::Debug for EndpointConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The predicate is passed in rather than chosen inside, because a
        // header and a query parameter sharing a name are different values
        // and only one of them may be a credential.
        let redact =
            |pairs: &[(String, String)], secret: &dyn Fn(&str) -> bool| -> Vec<(String, String)> {
                pairs
                    .iter()
                    .map(|(name, value)| {
                        let value = if secret(name) {
                            "<redacted>".to_string()
                        } else {
                            value.clone()
                        };
                        (name.clone(), value)
                    })
                    .collect()
            };

        f.debug_struct("EndpointConfig")
            .field("dialect", &self.dialect)
            .field("name", &self.name)
            .field("base_url", &self.base_url)
            .field("auth", &self.auth)
            .field("api_key_env", &self.api_key_env)
            .field("default_model", &self.default_model)
            .field("path", &self.path)
            .field(
                "query",
                &redact(&self.query, &|name| self.is_secret_query(name)),
            )
            .field(
                "extra_headers",
                &redact(&self.extra_headers, &|name| self.is_secret_header(name)),
            )
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
            path: None,
            query: Vec::new(),
            secret_headers: HashSet::new(),
            secret_query: HashSet::new(),
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

    /// Adds an extra header whose value is a credential.
    ///
    /// The same as [`EndpointConfig::header`] on the wire. The difference is
    /// what Freyja prints: this value is withheld from `Debug` and from error
    /// messages whatever the name looks like, so a second credential no longer
    /// depends on its name resembling one.
    ///
    /// ```
    /// use freyja::{EndpointConfig, Dialect};
    ///
    /// let config = EndpointConfig::new(Dialect::OpenAiChat, "gw", "https://gw.test/v1")
    ///     .header("x-acme-tenant", "engineering")
    ///     .secret_header("x-acme-passport", "live-value");
    ///
    /// let printed = format!("{config:?}");
    /// assert!(!printed.contains("live-value"));
    /// assert!(printed.contains("engineering"));
    /// ```
    pub fn secret_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        let name = name.into();
        // Lowercased in the set and left alone in the header, because the name
        // is case-insensitive to match on and is the caller's to spell on the
        // wire.
        self.secret_headers.insert(name.to_ascii_lowercase());
        self.header(name, value)
    }

    /// Replaces the path [`Dialect::path`] would supply.
    ///
    /// ```
    /// use freyja::{EndpointConfig, Dialect};
    ///
    /// let config = EndpointConfig::new(
    ///         Dialect::OpenAiChat,
    ///         "Azure",
    ///         "https://acme.openai.azure.com",
    ///     )
    ///     .path("/openai/deployments/gpt4/chat/completions")
    ///     .query("api-version", "2024-02-01");
    /// assert_eq!(
    ///     config.url(),
    ///     "https://acme.openai.azure.com/openai/deployments/gpt4/chat/completions?api-version=2024-02-01"
    /// );
    /// ```
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Adds a query parameter sent with every request.
    ///
    /// Percent-encoded on the way out, so the value is written as it means to
    /// be read. A credential belongs in [`EndpointConfig::auth`].
    pub fn query(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.query.push((name.into(), value.into()));
        self
    }

    /// Adds a query parameter whose value is a credential.
    ///
    /// The companion to [`EndpointConfig::secret_header`], and the same trade:
    /// identical on the wire, withheld from everything Freyja prints.
    ///
    /// The endpoint's own API key belongs in [`EndpointConfig::auth`], which
    /// has [`Auth::Query`] for an endpoint that wants it in the URL. This is
    /// for a second credential beside it.
    pub fn secret_query(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        let name = name.into();
        self.secret_query.insert(name.clone());
        self.query(name, value)
    }

    /// Whether this header's value must be withheld from anything Freyja prints.
    ///
    /// Two sources: what the caller classified with
    /// [`EndpointConfig::secret_header`], and the name heuristic behind it.
    /// The heuristic stays because a caller who classifies nothing is still
    /// covered, and because it errs toward hiding, which is the right
    /// direction when the alternative is a key in a log.
    ///
    /// Public because [`EndpointConfig::secret_headers`] is. That field holds
    /// one of the two sources, so reading it answers a narrower question than
    /// it appears to, and a caller who reimplemented the heuristic would be
    /// keeping a copy of a rule that lives here.
    ///
    /// ```
    /// use freyja::{Dialect, EndpointConfig};
    ///
    /// let config = EndpointConfig::new(Dialect::OpenAiChat, "gw", "https://gw.test/v1")
    ///     .header("x-api-key", "live-key")
    ///     .secret_header("X-Acme-Passport", "live-value");
    ///
    /// // Nobody classified this name by hand.
    /// assert!(!config.secret_headers.contains("x-api-key"));
    /// // It is withheld all the same, and this is how to ask.
    /// assert!(config.is_secret_header("x-api-key"));
    ///
    /// // A header name is case-insensitive, so one classification answers for
    /// // every spelling of it.
    /// assert!(config.is_secret_header("x-acme-passport"));
    /// assert!(config.is_secret_header("X-ACME-PASSPORT"));
    /// ```
    pub fn is_secret_header(&self, name: &str) -> bool {
        self.secret_headers.contains(&name.to_ascii_lowercase()) || is_secret_name(name)
    }

    /// Whether this query parameter's value must be withheld from anything
    /// Freyja prints.
    ///
    /// Three sources, in order of certainty: what the caller classified with
    /// [`EndpointConfig::secret_query`], the parameter this endpoint's
    /// [`EndpointConfig::auth`] uses under [`Auth::Query`], and the name
    /// heuristic behind both.
    ///
    /// The auth source counts here and not in
    /// [`EndpointConfig::is_secret_header`], because a credential in the URL
    /// says nothing about a header that happens to share its name.
    ///
    /// ```
    /// use freyja::{Auth, Dialect, EndpointConfig};
    ///
    /// let config = EndpointConfig::new(Dialect::Gemini, "g", "https://x.test/v1")
    ///     .auth(Auth::Query("key"));
    ///
    /// assert!(config.is_secret_query("key"));
    /// ```
    pub fn is_secret_query(&self, name: &str) -> bool {
        self.secret_query.contains(name)
            || matches!(self.auth, Auth::Query(parameter) if parameter == name)
            || is_secret_name(name)
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

    /// The full URL requests are sent to, with credential-shaped query values
    /// withheld.
    ///
    /// Safe to print, which is what this is for. A value
    /// [`EndpointConfig::is_secret_query`] answers for reads `REDACTED`, so
    /// this agrees with `Debug` and with the URL in a transport error rather
    /// than being the one rendering that shows the value in full.
    ///
    /// It therefore differs from the URL a request goes to, by those values
    /// and by the [`Auth::Query`] credential, which is added at send time and
    /// never appears here at all. That is the same trade, made once.
    ///
    /// ```
    /// use freyja::{Dialect, EndpointConfig};
    ///
    /// let config = EndpointConfig::new(Dialect::OpenAiChat, "gw", "https://gw.test/v1")
    ///     .query("tenant", "engineering")
    ///     .secret_query("passport", "pp-live");
    ///
    /// assert_eq!(
    ///     config.url(),
    ///     "https://gw.test/v1/chat/completions?tenant=engineering&passport=REDACTED"
    /// );
    /// ```
    pub fn url(&self) -> String {
        self.build_url(&[], Reveal::No)
    }

    /// The full URL streaming requests are sent to, withheld the same way
    /// [`EndpointConfig::url`] is.
    pub fn stream_url(&self) -> String {
        self.build_url(&self.stream_extra(), Reveal::No)
    }

    /// The URL a request actually goes to, credentials included.
    ///
    /// Crate-private, and the only caller is [`crate::Client`] on its way to
    /// the socket. The public pair withholds, because printing where requests
    /// go is what a caller reaches for those for.
    pub(crate) fn send_url(&self) -> String {
        self.build_url(&[], Reveal::Yes)
    }

    /// The streaming companion to [`EndpointConfig::send_url`].
    pub(crate) fn send_stream_url(&self) -> String {
        self.build_url(&self.stream_extra(), Reveal::Yes)
    }

    /// The dialect's streaming parameter, as the slice `build_url` takes.
    fn stream_extra(&self) -> Vec<(&'static str, &'static str)> {
        self.dialect.stream_query().into_iter().collect()
    }

    /// Assembles the request URL, appending `extra` after the endpoint's own
    /// query parameters.
    ///
    /// Goes through [`reqwest::Url`] so the path lands before the query rather
    /// than inside it, and so joining and escaping are never done by hand. A
    /// `base_url` that will not parse falls back to plain concatenation, which
    /// keeps a malformed base failing where it always has: at send time, as a
    /// transport error naming the endpoint.
    ///
    /// `reveal` is a parameter rather than a second function, so there is one
    /// place a URL is assembled and no chance of a printed URL and a sent one
    /// drifting into disagreeing about anything but the credential.
    fn build_url(&self, extra: &[(&str, &str)], reveal: Reveal) -> String {
        let path = self.path.as_deref().unwrap_or_else(|| self.dialect.path());

        let Ok(mut url) = reqwest::Url::parse(&self.base_url) else {
            return format!("{}{}", self.base_url.trim_end_matches('/'), path);
        };

        let joined = format!(
            "{}/{}",
            url.path().trim_end_matches('/'),
            path.trim_start_matches('/')
        );
        url.set_path(&joined);

        // Guarded, because `query_pairs_mut` leaves a bare `?` behind when it
        // is handed nothing to append.
        if !self.query.is_empty() || !extra.is_empty() {
            let mut pairs = url.query_pairs_mut();
            for (name, value) in &self.query {
                match reveal {
                    Reveal::No if self.is_secret_query(name) => {
                        pairs.append_pair(name, REDACTED_QUERY_VALUE)
                    }
                    _ => pairs.append_pair(name, value),
                };
            }
            // `extra` is the dialect's own streaming flag, which is neither
            // caller-supplied nor a credential.
            for (name, value) in extra {
                pairs.append_pair(name, value);
            }
        }

        url.into()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn gateway() -> EndpointConfig {
        EndpointConfig::new(Dialect::OpenAiChat, "gw", "https://gw.test/v1")
            .query("tenant", "engineering")
            .secret_query("passport", "pp-live")
    }

    #[test]
    fn url_withholds_a_classified_query_value() {
        let printed = gateway().url();

        assert!(!printed.contains("pp-live"), "{printed}");
        assert!(printed.contains("passport=REDACTED"), "{printed}");
        // A routing parameter is what a reader needs, and redacting everything
        // would make the method useless for the job it exists for.
        assert!(printed.contains("tenant=engineering"), "{printed}");
    }

    /// The half that stops the fix from breaking every request. Redacting both
    /// renderings would pass the test above and send `REDACTED` to the
    /// endpoint, and no other test in the suite would notice.
    #[test]
    fn the_url_a_request_goes_to_still_carries_the_value() {
        let sent = gateway().send_url();

        assert!(sent.contains("passport=pp-live"), "{sent}");
        assert!(sent.contains("tenant=engineering"), "{sent}");
    }

    #[test]
    fn the_streaming_pair_agrees_with_the_plain_one() {
        // Gemini is the only dialect with a streaming parameter, so it is the
        // only one where the two paths could disagree.
        let config = EndpointConfig::new(Dialect::Gemini, "g", "https://g.test/v1")
            .secret_query("passport", "pp-live");

        let printed = config.stream_url();
        assert!(!printed.contains("pp-live"), "{printed}");
        assert!(printed.contains("passport=REDACTED"), "{printed}");
        assert!(printed.contains("alt=sse"), "{printed}");

        let sent = config.send_stream_url();
        assert!(sent.contains("passport=pp-live"), "{sent}");
        assert!(sent.contains("alt=sse"), "{sent}");
    }

    #[test]
    fn a_classified_header_is_matched_whatever_its_case() {
        // A header name is case-insensitive, and the set held one spelling, so
        // a caller asking with another got told a credential was not one.
        let config = EndpointConfig::new(Dialect::OpenAiChat, "gw", "https://gw.test/v1")
            .secret_header("X-Acme-Passport", "live-value");

        for spelling in ["X-Acme-Passport", "x-acme-passport", "X-ACME-PASSPORT"] {
            assert!(config.is_secret_header(spelling), "{spelling}");
        }
        // A different name is still a different name.
        assert!(!config.is_secret_header("x-acme-tenant"));
        // And the header goes out spelled the way the caller wrote it.
        assert_eq!(
            config.extra_headers,
            vec![("X-Acme-Passport".to_string(), "live-value".to_string())]
        );
    }

    #[test]
    fn a_classified_query_parameter_is_matched_exactly() {
        // The opposite rule, and deliberately: a query parameter name is
        // case-sensitive, so `Sig` and `sig` are two parameters and hiding one
        // says nothing about the other.
        let config = EndpointConfig::new(Dialect::OpenAiChat, "gw", "https://gw.test/v1")
            .secret_query("Sig", "live-value");

        assert!(config.is_secret_query("Sig"));
        assert!(!config.is_secret_query("sig"));
    }

    #[test]
    fn an_auth_query_credential_is_in_neither() {
        // It comes from the Client, not the config, so `build_url` never sees
        // it under either mode.
        let config =
            EndpointConfig::new(Dialect::Gemini, "g", "https://g.test/v1").auth(Auth::Query("key"));

        assert!(!config.url().contains("key="), "{}", config.url());
        assert!(!config.send_url().contains("key="), "{}", config.send_url());
    }
}
