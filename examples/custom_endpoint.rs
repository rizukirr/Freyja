//! Reaching an endpoint Freyja ships no preset for.
//!
//! Freyja implements four wire dialects but ships presets only for the three
//! first-party vendors it tests against. Everything else, and that is most of
//! the hosted inference market, is one `Client::custom` call away.
//!
//! ```sh
//! DEEPSEEK_API_KEY=... cargo run --example custom_endpoint
//! ```

use freyja::{Auth, Client, Dialect, EndpointConfig, GenerateRequest, Message, Role};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let Ok(api_key) = std::env::var("DEEPSEEK_API_KEY") else {
        eprintln!("DEEPSEEK_API_KEY is missing or empty");
        return;
    };

    // Four things: which wire format, a name for error messages, the root URL,
    // and the key. Auth follows the dialect, so this sends a bearer token
    // without being told to.
    let client = Client::custom(
        Dialect::OpenAiChat,
        "DeepSeek",
        "https://api.deepseek.com/v1",
        api_key,
    );

    // The endpoint has no default model here, so the request must name one.
    // Leaving both unset fails locally rather than at the vendor.
    let request = GenerateRequest::new()
        .model("deepseek-chat")
        .message(Message::text(Role::User, "Say hello in five words."));

    match client.generate(&request).await {
        Ok(response) => println!("{}", response.output_text()),
        Err(error) => eprintln!("request failed: {error}"),
    }

    // The builder covers what `custom` does not: a default model so requests
    // need not repeat it, a key variable for `Client::from_env`, extra headers,
    // and an auth style that differs from the dialect's convention.
    let _configured = EndpointConfig::new(
        Dialect::Anthropic,
        "my-gateway",
        "https://gateway.internal/anthropic/v1",
    )
    .default_model("claude-opus-5")
    .api_key_env("GATEWAY_API_KEY")
    .header("x-trace-id", "abc123")
    .auth(Auth::Bearer);

    // An endpoint whose URL follows neither the dialect nor the vendor. `path`
    // replaces what the dialect would append, so this one owns the whole path,
    // and `query` pins a parameter on every request. Freyja does the joining
    // and the escaping, so the URL never grows a second `?`.
    let _deployment_scoped = EndpointConfig::new(
        Dialect::OpenAiChat,
        "Azure",
        "https://acme.openai.azure.com",
    )
    .path("/openai/deployments/gpt4/chat/completions")
    .query("api-version", "2024-02-01")
    .api_key_env("AZURE_OPENAI_API_KEY");

    // A gateway wanting a second credential beside the key. `secret_header` and
    // `secret_query` send exactly what `header` and `query` send. The
    // difference is what Freyja prints: a classified value is withheld from
    // `Debug`, from error messages and from `url()`, while `x-acme-tenant` and `api-version`
    // stay readable, which is what you want when a gateway is rejecting you.
    let guarded = EndpointConfig::new(Dialect::OpenAiChat, "acme-gw", "https://gw.acme.test/v1")
        .api_key_env("ACME_API_KEY")
        .header("x-acme-tenant", "engineering")
        .secret_header("x-acme-passport", "a-second-credential")
        .query("api-version", "2024-02-01")
        .secret_query("sig", "a-signature");

    // Neither name contains a word the redaction heuristic knows, so without
    // the two `secret_` builders both would have printed in full.
    println!("{guarded:?}");

    // And the same for the method built to be printed. `sig` reads as
    // REDACTED here, while `api-version` stays visible, which is the parameter
    // you need when the gateway is the thing rejecting you.
    println!("{}", guarded.url());

    // An endpoint that takes the key in the URL rather than in a header. The
    // key still comes from wherever you told Freyja to look, and it is added
    // when the request is sent, so `url()` stays free of credentials.
    let in_the_url = EndpointConfig::new(
        Dialect::Gemini,
        "legacy",
        "https://generativelanguage.googleapis.com/v1beta",
    )
    .api_key_env("GEMINI_API_KEY")
    .auth(Auth::Query("key"));

    println!("{}", in_the_url.url());

    // A local runtime needs no credentials at all.
    let _local = Client::without_key(EndpointConfig::new(
        Dialect::OpenAiChat,
        "ollama",
        "http://localhost:11434/v1",
    ));
}
