//! The smallest useful call: one question, one answer.
//!
//! ```sh
//! cargo run --example simple
//! ```

use freyja::{Agent, Client, EndpointPreset};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let provider = EndpointPreset::OpenAi;
    let client = match Client::from_env(provider) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("{error}");
            return;
        }
    };

    // No model, no max_tokens, no temperature. Every unset field means "the
    // endpoint decides", so this same agent is valid on every provider.
    let agent = Agent::new(client);

    match agent.ask("Name three Rust crates.").await {
        Ok(run) => {
            println!("{}", run.answer);
            println!("\nusage: {} tokens", run.usage.total_tokens);
        }
        Err(error) => eprintln!("request failed: {error}"),
    }
}
