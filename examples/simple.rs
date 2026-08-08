//! The smallest useful call: one question, one answer.
//!
//! ```sh
//! cargo run --example simple
//! ```

use freya::{Client, GenerateRequest, Message, ProviderType, Role};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let provider = ProviderType::OpenAi;
    let Some(client) = Client::from_env(provider) else {
        eprintln!("{} is missing or empty", provider.api_key_env());
        return;
    };

    // No model, no max_tokens, no temperature. Every unset field means "the
    // endpoint decides", so this same request is valid on every provider.
    let request =
        GenerateRequest::new().message(Message::text(Role::User, "Name three Rust crates."));

    match client.generate(&request).await {
        Ok(response) => {
            println!("{}", response.output_text());
            if let Some(usage) = response.usage {
                println!("\nusage: {} tokens", usage.total_tokens);
            }
        }
        Err(error) => eprintln!("request failed: {error}"),
    }
}
