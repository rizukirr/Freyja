//! A conversation that survives the process.
//!
//! `agent` and `memory` hold their transcripts in memory, so every run starts
//! from nothing. `JsonlStorage` keeps the conversation in a file instead, and
//! a later run opening the same path continues it. Run this twice and the
//! second run answers a question about the first.
//!
//! ```sh
//! cargo run --example persist -- "My name is Freyja."
//! cargo run --example persist -- "What is my name?"
//! cargo run --example persist -- --reset
//! ```

use freyja::{Agent, Client, EndpointPreset, JsonlStorage};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let provider = EndpointPreset::OpenAi;
    let Some(client) = Client::from_env(provider) else {
        eprintln!("{} is missing or empty", provider.api_key_env());
        return;
    };

    let Some(input) = std::env::args().nth(1) else {
        eprintln!("pass a message, or --reset to start over");
        return;
    };

    // One file is one conversation. An application with several picks a path
    // per conversation, and validates the id first if a client supplied it.
    let path = std::env::temp_dir().join("freyja-persist.jsonl");

    // `open` reads whatever an earlier run left and creates the file if there
    // is none. It fails with a `StorageError`, not a `freyja::Error`, because
    // no endpoint is involved.
    let storage = match JsonlStorage::open(&path) {
        Ok(storage) => storage,
        Err(error) => {
            eprintln!("could not open {}: {error}", path.display());
            return;
        }
    };
    println!(
        "{} messages found in {}",
        storage.messages().len(),
        path.display()
    );

    // The system instruction belongs to the agent and is never written to the
    // file, so changing it between runs changes it for the stored turns too.
    let agent = Agent::new(client).system("Answer in one short sentence.");

    // The same window `InMemoryStorage` has. It bounds what one request
    // carries, and the file keeps every turn regardless.
    let mut chat = agent.conversation(storage.window(20));

    if input == "--reset" {
        // Empties the file as well as the transcript. Dropping the
        // conversation would not: the file outlives it.
        match chat.clear().await {
            Ok(()) => println!("conversation cleared"),
            Err(error) => eprintln!("{error}"),
        }
        return;
    }

    match chat.send(input).await {
        Ok(run) => println!("{}", run.answer),
        Err(error) => {
            // Storage is written last, so a failed run adds nothing to the
            // file and the next run does not see half an exchange.
            eprintln!("{} failed: {error}", error.endpoint());
            return;
        }
    }

    // Already on disk. Each line of the file is one of these, as JSON.
    println!("{} messages held", chat.storage().messages().len());
}
