//! Bounding what reaches the model without losing what was said.
//!
//! `agent` keeps the whole conversation and sends all of it, which is fine
//! until it is not: a transcript grows until the provider rejects it, and the
//! error says nothing about length. A [`Window`] decides what goes on the wire
//! each turn, and the transcript here is never shortened.
//!
//! ```sh
//! cargo run --example memory
//! ```

use freyja::{Agent, Client, EndpointPreset, InMemoryStorage, Storage, Window};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let provider = EndpointPreset::OpenAi;
    let Some(client) = Client::from_env(provider) else {
        eprintln!("{} is missing or empty", provider.api_key_env());
        return;
    };

    // Kept as an `Arc` so the length can be read back after the loop: `Agent`
    // takes ownership of whatever is installed with `memory`, and `Storage`
    // is implemented for `Arc<T>` for exactly this reason.
    let storage = Arc::new(InMemoryStorage::new());

    let agent = Agent::new(client)
        .system("You are a concise assistant. Answer in one sentence.")
        .filter(Window::groups(2))
        .memory(Arc::clone(&storage));

    for question in [
        "Name a Norwegian city.",
        "What is its population?",
        "What language do they speak there?",
        "What was the first thing I asked you?",
    ] {
        match agent.message(question).await {
            Ok(run) => println!("> {question}\n{}\n", run.answer),
            Err(error) => {
                eprintln!("{error}");
                return;
            }
        }
    }

    // Every turn is still here, held by storage rather than a vector we kept
    // ourselves. The last request carried the system instruction and the most
    // recent groups only, which is why the model could not answer the last
    // question.
    let held = storage.load().await.expect("read back the conversation");
    println!("transcript still holds {} messages", held.len());
}
