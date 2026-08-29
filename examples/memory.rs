//! Bounding what reaches the model without losing what was said.
//!
//! `agent` keeps the whole conversation and sends all of it, which is fine
//! until it is not: a transcript grows until the provider rejects it, and the
//! error says nothing about length. `Conversation::window` decides what goes
//! on the wire each turn, and the transcript itself is never shortened.
//!
//! ```sh
//! cargo run --example memory
//! ```

use freyja::{Agent, Client, EndpointPreset, InMemoryStorage};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let provider = EndpointPreset::OpenAi;
    let Some(client) = Client::from_env(provider) else {
        eprintln!("{} is missing or empty", provider.api_key_env());
        return;
    };

    let agent = Agent::new(client).system("Answer in one short sentence.");
    let mut chat = agent.conversation(InMemoryStorage::new().window(2));

    for question in [
        "Name a Norwegian city.",
        "What is its population?",
        "What language do they speak there?",
        "What was the first thing I asked you?",
    ] {
        match chat.send(question).await {
            Ok(run) => println!("> {question}\n{}\n", run.answer),
            Err(error) => {
                eprintln!("{error}");
                return;
            }
        }
    }

    // Every turn is still here, held by the conversation's own storage. The
    // last request carried only the most recent groups, which is why the
    // model could not answer the last question.
    println!("{} messages held", chat.storage().messages().len());
}
