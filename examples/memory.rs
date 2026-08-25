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

use freyja::{Agent, Client, EndpointPreset, GenerateRequest, Message, Role, Window};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let provider = EndpointPreset::OpenAi;
    let Some(client) = Client::from_env(provider) else {
        eprintln!("{} is missing or empty", provider.api_key_env());
        return;
    };

    let agent = Agent::new(client)
        .request(GenerateRequest::new().message(Message::text(
            Role::System,
            "You are a concise assistant. Answer in one sentence.",
        )))
        .memory(Window::groups(2));

    let mut messages: Vec<Message> = Vec::new();

    for question in [
        "Name a Norwegian city.",
        "What is its population?",
        "What language do they speak there?",
        "What was the first thing I asked you?",
    ] {
        messages.push(Message::text(Role::User, question));
        match agent.run(&mut messages).await {
            Ok(run) => println!("> {question}\n{}\n", run.answer),
            Err(error) => {
                eprintln!("{error}");
                return;
            }
        }
    }

    // Every turn is still here. The last request carried only the pinned system
    // message and the most recent groups, which is why the model could not
    // answer the last question.
    println!("transcript still holds {} messages", messages.len());
}
