//! Bounding what reaches the model without losing what was said.
//!
//! `agent` keeps the whole conversation and sends all of it, which is fine
//! until it is not: a transcript grows until the provider rejects it, and the
//! error says nothing about length. `InMemoryStorage::window` decides what
//! goes on the wire each turn, and the transcript itself is never shortened.
//!
//! ```sh
//! cargo run --example memory
//! ```

use freyja::{Agent, Client, EndpointPreset, HeuristicCounter, InMemoryStorage};

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

    // Same agent, same four questions, a token budget instead of a group
    // count. 200 is small enough that the four short exchanges do not all
    // fit, which is what makes this window trim anything at all.
    let mut token_chat =
        agent.conversation(InMemoryStorage::new().window_by_tokens(200, HeuristicCounter));

    for question in [
        "Name a Norwegian city.",
        "What is its population?",
        "What language do they speak there?",
        "What was the first thing I asked you?",
    ] {
        match token_chat.send(question).await {
            Ok(run) => println!("> {question}\n{}\n", run.answer),
            Err(error) => {
                eprintln!("{error}");
                return;
            }
        }
    }

    println!("{} messages held", token_chat.storage().messages().len());
}
