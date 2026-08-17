//! The public client API.

use crate::dialect::{Dialect, WireDialect};
use crate::endpoint::{Auth, EndpointConfig};
use crate::error::Error;
use crate::model::*;
use std::fmt;
use std::sync::Arc;
#[cfg(test)]
use std::time::Duration;

/// The entry point: an endpoint plus its credentials and HTTP client.
///
/// Cloning is cheap for the HTTP client and config name but copies the API key;
/// prefer sharing one `Client` behind an `Arc` when you need it in several tasks.
///
/// ```no_run
/// # async fn run() -> Result<(), freyja::Error> {
/// use freyja::{Client, GenerateRequest, Message, EndpointPreset, Role};
///
/// let client = Client::from_env(EndpointPreset::OpenAi).unwrap();
/// let response = client
///     .generate(&GenerateRequest::new().message(Message::text(Role::User, "Hi")))
///     .await?;
/// println!("{}", response.output_text());
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct Client {
    config: EndpointConfig,
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
    /// Accepts anything that converts into a [`EndpointConfig`], including a
    /// [`crate::EndpointPreset`] preset.
    pub fn new(config: impl Into<EndpointConfig>, api_key: impl Into<String>) -> Self {
        Self::build(
            config.into(),
            Some(api_key.into()),
            crate::transport::default_http(),
        )
    }

    /// Creates a client for an endpoint Freyja does not ship, in one call.
    ///
    /// Shorthand for [`EndpointConfig::new`] followed by [`Client::new`]. Reach
    /// for the config builder directly when you also need a default model, a
    /// key variable, extra headers, or a non-conventional auth style.
    ///
    /// ```no_run
    /// use freyja::{Client, Dialect};
    ///
    /// let client = Client::custom(
    ///     Dialect::OpenAiChat,
    ///     "my-gateway",
    ///     "https://gateway.internal/v1",
    ///     std::env::var("GATEWAY_API_KEY").unwrap(),
    /// );
    /// ```
    pub fn custom(
        dialect: Dialect,
        name: impl Into<Arc<str>>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self::new(EndpointConfig::new(dialect, name, base_url), api_key)
    }

    /// Creates a client for an endpoint that needs no credentials, such as a
    /// local Ollama or vLLM server.
    pub fn without_key(config: impl Into<EndpointConfig>) -> Self {
        Self::build(config.into(), None, crate::transport::default_http())
    }

    /// Creates a client over a caller-supplied HTTP client.
    ///
    /// Use this to control timeouts, proxies, TLS, or to share one pool with the
    /// rest of your application.
    pub fn with_http_client(
        config: impl Into<EndpointConfig>,
        api_key: impl Into<String>,
        http: reqwest::Client,
    ) -> Self {
        Self::build(config.into(), Some(api_key.into()), http)
    }

    fn build(config: EndpointConfig, api_key: Option<String>, http: reqwest::Client) -> Self {
        Self {
            config,
            api_key,
            http,
        }
    }

    /// Reads the API key from the endpoint's [`EndpointConfig::api_key_env`].
    ///
    /// Returns `None` when the endpoint names a variable that is unset or empty.
    /// An endpoint with [`Auth::None`] needs no key and always succeeds.
    pub fn from_env(config: impl Into<EndpointConfig>) -> Option<Self> {
        let config = config.into();
        if config.auth == Auth::None {
            return Some(Self::build(config, None, crate::transport::default_http()));
        }
        match std::env::var(config.api_key_env?) {
            Ok(key) if !key.is_empty() => Some(Self::build(
                config,
                Some(key),
                crate::transport::default_http(),
            )),
            _ => None,
        }
    }

    /// The endpoint this client talks to.
    pub fn config(&self) -> &EndpointConfig {
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
    ///     Client, GenerateRequest, Message, Error, EndpointPreset, ReasoningEffort, Role,
    /// };
    ///
    /// let client = Client::custom(
    ///     EndpointPreset::Gemini.dialect(),
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
    ///     Err(Error::UnsupportedCapability { capability, .. }) => {
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
    /// passes here can still come back [`Error::BadRequest`].
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
    /// refusal and the evidence behind it is listed in `src/dialect/refusal.rs`.
    ///
    /// # Choosing a provider
    ///
    /// Because the answer depends on the request rather than on a fixed table,
    /// comparing endpoints means checking the same request against each:
    ///
    /// ```
    /// # use freyja::{Client, GenerateRequest, Message, EndpointPreset, Role, ToolChoice};
    /// # let request = GenerateRequest::new()
    /// #     .model("m")
    /// #     .message(Message::text(Role::User, "Hi"))
    /// #     .tool_choice(ToolChoice::Required);
    /// let usable: Vec<_> = [EndpointPreset::OpenAi, EndpointPreset::Gemini]
    ///     .into_iter()
    ///     .filter_map(|kind| Client::from_env(kind))
    ///     .filter(|client| client.check(&request).is_ok())
    ///     .collect();
    /// ```
    pub fn check(&self, request: &GenerateRequest) -> Result<(), Error> {
        // `stream` builds through the same call before adding its streaming
        // flag, so one check covers both paths.
        crate::dialect::with_dialect!(self.config.dialect, |provider| provider
            .build(request, &self.config)
            .map(drop))
    }

    /// Sends a request and returns the normalized response.
    pub async fn generate(&self, request: &GenerateRequest) -> Result<GenerateResponse, Error> {
        crate::dialect::with_dialect!(self.config.dialect, |provider| self
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
    /// # async fn run(client: freyja::Client) -> Result<(), freyja::Error> {
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
    /// [`Error::OutputMismatch`] when the call succeeded and the answer
    /// was not the shape you asked for. That error keeps the model's text so it
    /// can be logged or shown, and flags the truncation case separately —
    /// a cut-off JSON object is the most common reason this fails, and the fix
    /// is a larger [`GenerateRequest::max_tokens`] rather than a different
    /// schema.
    pub async fn generate_as<T: serde::de::DeserializeOwned>(
        &self,
        request: &GenerateRequest,
    ) -> Result<T, Error> {
        let response = self.generate(request).await?;
        let text = response.output_text();

        serde_json::from_str(&text).map_err(|error| Error::OutputMismatch {
            endpoint: self.config.name.clone(),
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
    /// status arrives here as [`Error::Api`] rather than mid-stream.
    ///
    /// The default HTTP client bounds *inactivity*, not total duration, so a
    /// long generation is not cut short. A client supplied through
    /// [`Client::with_http_client`] keeps whatever it was built with — set
    /// `read_timeout` rather than `timeout` on it, or a long stream will be
    /// killed part-way.
    ///
    /// ```no_run
    /// # async fn run() -> Result<(), freyja::Error> {
    /// use freyja::{Client, GenerateRequest, Message, EndpointPreset, Role, StreamEvent};
    ///
    /// let client = Client::from_env(EndpointPreset::OpenAi).unwrap();
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
    pub async fn stream(
        &self,
        request: &GenerateRequest,
    ) -> Result<crate::stream::EventStream, Error> {
        let wire = crate::dialect::with_dialect!(self.config.dialect, |provider| {
            let body = provider.build(request, &self.config)?.streaming();
            crate::transport::to_value(&body, request, &self.config)?
        });
        let decoder = crate::dialect::decoder_for(self.config.dialect);

        let response = crate::transport::post(
            &self.http,
            &self.config,
            self.api_key.as_deref(),
            self.config.stream_url(),
            &wire,
        )
        .await?;

        let status = response.status();
        if !status.is_success() {
            // Read before `text()`, which consumes the response and takes the
            // headers with it.
            let retry_after = crate::transport::parse_retry_after(response.headers());
            let body = response
                .text()
                .await
                .map_err(|error| Error::transport(self.config.name.clone(), &error))?;
            return Err(Error::from_status(
                self.config.name.clone(),
                status.as_u16(),
                retry_after,
                body,
            ));
        }

        Ok(crate::stream::EventStream::new(
            self.config.name.clone(),
            decoder,
            response,
        ))
    }

    /// Convert, POST, check status, parse. Shared by every dialect, which is
    /// why none of them owns transport code.
    async fn run<P: crate::dialect::WireDialect>(
        &self,
        wire_dialect: P,
        request: &GenerateRequest,
    ) -> Result<GenerateResponse, Error> {
        let wire = wire_dialect.build(request, &self.config)?;
        let body = crate::transport::to_value(&wire, request, &self.config)?;
        let response = crate::transport::post(
            &self.http,
            &self.config,
            self.api_key.as_deref(),
            self.config.url(),
            &body,
        )
        .await?;

        let status = response.status();
        let retry_after = crate::transport::parse_retry_after(response.headers());
        let body = response
            .text()
            .await
            .map_err(|error| Error::transport(self.config.name.clone(), &error))?;

        if !status.is_success() {
            return Err(Error::from_status(
                self.config.name.clone(),
                status.as_u16(),
                retry_after,
                body,
            ));
        }

        wire_dialect.parse(&body, &self.config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::{WireDialect, gemini, openai_chat};
    use crate::endpoint::EndpointPreset;
    use serde_json::Value;

    #[test]
    fn debug_never_prints_the_api_key() {
        let client = Client::new(EndpointPreset::OpenAi, "sk-secret-value");
        let rendered = format!("{client:?}");

        assert!(!rendered.contains("sk-secret-value"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");

        // The distinction between "no key" and "a key I will not show you"
        // stays visible, since it is the usual cause of a 401.
        let local = EndpointConfig::new(Dialect::OpenAiChat, "local", "http://localhost:11434/v1");
        let keyless = Client::without_key(local);
        assert!(format!("{keyless:?}").contains("<none>"));
    }

    #[test]
    fn debug_redacts_a_credential_left_in_an_extra_header() {
        // The client redacts its own key, then printed its config verbatim one
        // field over. A gateway wanting a second credential has nowhere else to
        // put it, so the redaction has to reach here too.
        let config = EndpointConfig::new(Dialect::OpenAiChat, "gw", "https://gw.test/v1")
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
            Dialect::OpenAiChat,
            "my-gateway",
            "https://gateway.internal/v1",
            "key",
        );

        assert_eq!(client.config().dialect, Dialect::OpenAiChat);
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
        let config = EndpointConfig::new(Dialect::Anthropic, "test", "https://x.test/v1");
        assert_eq!(config.url(), "https://x.test/v1/messages");

        let trailing = EndpointConfig::new(Dialect::OpenAiResponses, "test", "https://x.test/v1/");
        assert_eq!(trailing.url(), "https://x.test/v1/responses");
    }

    #[test]
    fn takes_the_dialects_auth_style_by_default() {
        let anthropic = EndpointConfig::new(Dialect::Anthropic, "a", "https://x.test");
        assert_eq!(anthropic.auth, Auth::Header("x-api-key"));

        let overridden = anthropic.auth(Auth::Bearer);
        assert_eq!(overridden.auth, Auth::Bearer);
    }

    #[test]
    fn resolves_the_model_from_request_then_endpoint() {
        let config = EndpointConfig::new(Dialect::Anthropic, "a", "https://x.test")
            .default_model("endpoint-default");

        let empty = GenerateRequest::new();
        assert_eq!(config.model_for(&empty).unwrap(), "endpoint-default");

        let explicit = GenerateRequest::new().model("caller-choice");
        assert_eq!(config.model_for(&explicit).unwrap(), "caller-choice");
    }

    #[test]
    fn refuses_a_request_with_no_model_anywhere() {
        let config = EndpointConfig::new(Dialect::Anthropic, "a", "https://x.test");

        assert!(matches!(
            config.model_for(&GenerateRequest::new()),
            Err(Error::InvalidRequest { .. })
        ));
    }

    #[test]
    fn stream_url_appends_alt_sse_for_gemini() {
        let gemini = EndpointConfig::new(Dialect::Gemini, "g", "https://x.test/v1");
        assert_eq!(gemini.url(), "https://x.test/v1/interactions");
        assert_eq!(
            gemini.stream_url(),
            "https://x.test/v1/interactions?alt=sse",
            "Gemini selects SSE by query parameter, not by body field alone"
        );

        // Every other dialect streams from the same URL it generates from.
        let anthropic = EndpointConfig::new(Dialect::Anthropic, "a", "https://x.test/v1");
        assert_eq!(anthropic.stream_url(), anthropic.url());
    }

    const DIALECTS: [Dialect; 4] = [
        Dialect::OpenAiResponses,
        Dialect::OpenAiChat,
        Dialect::Gemini,
        Dialect::Anthropic,
    ];

    /// A client that could never reach anything, which is the point: `check`
    /// must not need a reachable endpoint or a real key.
    fn offline(dialect: Dialect) -> Client {
        Client::custom(dialect, "test", "http://127.0.0.1:1/v1", "unused")
    }

    fn ask() -> GenerateRequest {
        GenerateRequest::new()
            .model("m")
            .message(Message::text(Role::User, "Hi"))
    }

    /// The body a dialect would post, escape hatches merged in.
    fn body_for(config: &EndpointConfig, request: &GenerateRequest) -> Value {
        let wire = gemini::GeminiProvider.build(request, config).unwrap();
        crate::transport::to_value(&wire, request, config).unwrap()
    }

    fn gemini_config() -> EndpointConfig {
        EndpointConfig::new(Dialect::Gemini, "Gemini", "https://x.test/v1")
    }

    #[test]
    fn extra_fields_merge_into_a_nested_object_without_clearing_it() {
        // The whole reason the merge is deep. `generation_config` already has
        // Freyja's cap in it, and adding a seed must not take the cap with it.
        let request = ask().max_tokens(64).extra_for(
            Dialect::Gemini,
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
            Dialect::Gemini,
            serde_json::json!({"generation_config": {"seed": 42}}),
        );

        let openai = EndpointConfig::new(Dialect::OpenAiChat, "OpenAI", "https://x.test/v1");
        let wire = openai_chat::OpenAiChatProvider
            .build(&request, &openai)
            .unwrap();
        let body = crate::transport::to_value(&wire, &request, &openai).unwrap();

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
            Dialect::Gemini,
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
            .extra_for(Dialect::Gemini, serde_json::json!({"seed": 1}))
            .extra_for(Dialect::Gemini, serde_json::json!({"seed": 2}));

        assert_eq!(body_for(&gemini_config(), &request)["seed"], 2);
    }

    #[test]
    fn check_says_nothing_about_extra_fields() {
        // It reports what the format can carry, and this is by definition
        // outside what Freyja knows about the format. A wrong key is the
        // endpoint's to reject.
        let request = ask().extra_for(
            Dialect::Gemini,
            serde_json::json!({"not_a_real_parameter": true}),
        );

        assert!(offline(Dialect::Gemini).check(&request).is_ok());
    }

    /// The capability a check refused, or a panic naming what happened instead.
    fn refusal(dialect: Dialect, request: &GenerateRequest) -> &'static str {
        match offline(dialect).check(request) {
            Err(Error::UnsupportedCapability { capability, .. }) => capability,
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
            refusal(Dialect::Anthropic, &ask().previous_response_id("resp_1")),
            "server-side conversation continuation"
        );
        assert_eq!(
            refusal(
                Dialect::OpenAiChat,
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
        let anthropic = offline(Dialect::Anthropic);

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
                Dialect::Anthropic,
                &ask().response_format(ResponseFormat::JsonObject)
            ),
            "schema-less JSON response format"
        );

        // And the counter-example, which is the more important half. Gemini
        // rejects `thinking_level: max` and rejects `labels` outright, and
        // `check` passes both: the fields exist, so what the endpoint does with
        // them is the endpoint's answer to give. `check` reports what the
        // format can carry, never what the deployment will accept.
        let gemini = offline(Dialect::Gemini);

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
        for dialect in [Dialect::OpenAiResponses, Dialect::Anthropic] {
            assert_eq!(
                refusal(dialect, &misplaced),
                "images outside user messages",
                "{dialect:?}"
            );
        }

        for dialect in [Dialect::OpenAiChat, Dialect::Gemini] {
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
                    Err(Error::InvalidRequest { .. })
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
        let client = offline(Dialect::Anthropic);

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
            crate::transport::parse_retry_after(&headers_with("30")),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            crate::transport::parse_retry_after(&headers_with(" 5 ")),
            Some(Duration::from_secs(5))
        );
    }

    #[test]
    fn an_unusable_retry_after_leaves_the_caller_in_charge() {
        // The HTTP-date form would need a clock and a date parser. Reporting
        // `None` hands pacing back to the caller's own backoff, which is
        // correct; guessing a duration would not be.
        assert_eq!(
            crate::transport::parse_retry_after(&headers_with("Wed, 21 Oct 2015 07:28:00 GMT")),
            None
        );
        assert_eq!(
            crate::transport::parse_retry_after(&headers_with("soon")),
            None
        );
        assert_eq!(
            crate::transport::parse_retry_after(&headers_with("-1")),
            None
        );
        assert_eq!(
            crate::transport::parse_retry_after(&reqwest::header::HeaderMap::new()),
            None
        );
    }
}
