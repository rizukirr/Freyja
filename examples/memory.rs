//! Bounding what reaches the model without losing what was said.
//!
//! `agent` keeps the whole conversation and sends all of it, which is fine
//! until it is not: a transcript grows until the provider rejects it, and the
//! error says nothing about length. `Agent::memory` with a `Window`
//! decides what goes on the wire each turn, and the transcript itself is never
//! shortened. `Window::summarize` sends a summary of what the window drops, so
//! the model keeps the gist of it.
//!
//! ```sh
//! cargo run --example memory
//! ```

use freyja::{
    Agent, Client, Conversation, EndpointPreset, Error, HeuristicCounter, InMemoryStorage, Storage,
    Summarizer, Window,
};

const QUESTIONS: [&str; 4] = [
    "Name a Norwegian city.",
    "What is its population?",
    "What language do they speak there?",
    "What was the first thing I asked you?",
];

/// Asks the four questions in order, printing each answer.
async fn ask<S: Storage>(chat: &mut Conversation<S>) -> Result<(), Error> {
    for question in QUESTIONS {
        let run = chat.send(question).await?;
        println!("> {question}\n{}\n", run.answer);
    }
    Ok(())
}

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

    let base = Agent::new(client.clone()).system("Answer in one short sentence.");

    let agent = base.clone().memory(Window::last(2));
    let mut chat = agent.conversation(InMemoryStorage::new());
    if let Err(error) = ask(&mut chat).await {
        eprintln!("{error}");
        return;
    }

    // Every turn is still here, held by the conversation's own storage. The
    // last request carried only the most recent groups, which is why the
    // model could not answer the last question.
    println!("{} messages held", chat.storage().messages().len());

    // Same four questions, a token budget instead of a group count. 200 is
    // small enough that the four short exchanges do not all fit, which is what
    // makes this window trim anything at all.
    let token_agent = base.clone().memory(Window::tokens(200, HeuristicCounter));
    let mut token_chat = token_agent.conversation(InMemoryStorage::new());
    if let Err(error) = ask(&mut token_chat).await {
        eprintln!("{error}");
        return;
    }
    println!("{} messages held", token_chat.storage().messages().len());

    // The group window again, with a summarizer. What the window drops comes
    // back as one summary message, so this time the model can answer the last
    // question. Each summary is one extra model call, and a window of two
    // groups has so little room that it summarizes again on most turns.
    let summarized_agent = base.memory(Window::last(2).summarize(Summarizer::new(client)));
    let mut summarized_chat = summarized_agent.conversation(InMemoryStorage::new());
    if let Err(error) = ask(&mut summarized_chat).await {
        eprintln!("{error}");
        return;
    }
    match summarized_chat.summary().await {
        Ok(Some(summary)) => println!("Sent in place of the dropped turns:\n{summary}"),
        Ok(None) => {}
        Err(error) => eprintln!("{error}"),
    }
}
