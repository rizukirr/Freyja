//! Reaching an endpoint Freya ships no preset for.
//!
//! Freya implements four wire dialects but ships presets only for the three
//! first-party vendors it tests against. Everything else, and that is most of
//! the hosted inference market, is one `Client::custom` call away.
//!
//! ```sh
//! DEEPSEEK_API_KEY=... cargo run --example custom_endpoint
//! ```

use freya::{Auth, Client, GenerateRequest, Message, ProviderConfig, ProviderDialect, Role};

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
        ProviderDialect::OpenAiChat,
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
    let _configured = ProviderConfig::new(
        ProviderDialect::Anthropic,
        "my-gateway",
        "https://gateway.internal/anthropic/v1",
    )
    .default_model("claude-opus-5")
    .api_key_env("GATEWAY_API_KEY")
    .header("x-trace-id", "abc123")
    .auth(Auth::Bearer);

    // A local runtime needs no credentials at all.
    let _local = Client::without_key(ProviderConfig::new(
        ProviderDialect::OpenAiChat,
        "ollama",
        "http://localhost:11434/v1",
    ));
}
