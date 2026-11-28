//! An interactive multi-turn chat, streamed.
//!
//! The other examples each make one call. This one holds a conversation, which
//! is a different problem: every turn re-sends the whole transcript, and a
//! failed call must not leave a dangling turn behind. A [`freyja::Conversation`]
//! keeps the transcript and restores it when a turn fails.
//!
//! ```sh
//! cargo run --example chat
//! ```
//!
//! Type a message and press enter. `/reset` starts over, `/history` shows the
//! transcript, `/exit` quits.

use freyja::{Agent, AgentEvent, Client, EndpointPreset, InMemoryStorage, Message, StreamEvent};
use std::io::{BufRead, Write};

/// Frames the whole conversation. Freyja puts it wherever the provider expects
/// it, a top-level field on some, an ordinary turn on others, so the same
/// line works on every vendor.
const SYSTEM_PROMPT: &str = "You are a concise assistant. Answer in two sentences at most.";

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

    // The endpoint's configured name, the same one that shows up in errors.
    let name = client.config().name.clone();

    // The conversation owns the transcript and grows it as the chat does.
    let agent = Agent::new(client).system(SYSTEM_PROMPT);
    let mut chat = agent.conversation(InMemoryStorage::new());

    println!("chatting with {name}. /reset, /history, /exit");

    let stdin = std::io::stdin();
    loop {
        print!("\nyou> ");
        let _ = std::io::stdout().flush();

        // Blocking reads on the runtime thread, which is fine here because
        // nothing else is scheduled. A server would use `spawn_blocking`.
        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => break, // ctrl-D
            Ok(_) => {}
            Err(error) => {
                eprintln!("could not read input: {error}");
                break;
            }
        }

        let text = match line.trim() {
            "" => continue,
            "/exit" | "/quit" => break,
            "/reset" => {
                if let Err(error) = chat.clear().await {
                    eprintln!("could not clear: {error}");
                } else {
                    println!("transcript cleared");
                }
                continue;
            }
            "/history" => {
                print_history(chat.storage().messages());
                continue;
            }
            text => text,
        };

        print!("\nbot> ");
        let _ = std::io::stdout().flush();

        match chat.send_streaming(text, show).await {
            Ok(run) => println!("\n[{} tokens]", run.usage.total_tokens),
            Err(error) => {
                // The failed turn is not stored, so the next question starts
                // from the transcript as it was before this one.
                eprintln!("\n{} failed: {error}", error.endpoint());
                if error.is_retryable() {
                    eprintln!("(transient, try again)");
                }
            }
        }
    }

    println!("bye");
}

/// Prints text as it arrives. Deltas land mid-line, so each one is flushed.
fn show(event: AgentEvent) {
    if let AgentEvent::Model(StreamEvent::TextDelta(text) | StreamEvent::RefusalDelta(text)) = event
    {
        print!("{text}");
        let _ = std::io::stdout().flush();
    }
}

/// Shows what the next call will send after the system prompt.
///
/// Worth looking at once: the whole transcript goes over the wire every turn,
/// so a long conversation costs more per question than a short one, and the
/// growth is quadratic rather than linear. Trimming or summarizing old turns is
/// the caller's job today, see the roadmap's Phase 3.
fn print_history(messages: &[Message]) {
    println!("{} turns:", messages.len());
    for message in messages {
        let text: String = message
            .content
            .iter()
            .filter_map(|part| match part {
                freyja::InputContent::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();

        let preview: String = text.chars().take(70).collect();
        let ellipsis = if text.chars().count() > 70 { "…" } else { "" };
        // Formatted first: a derived `Debug` writes straight to the formatter
        // and ignores the width, so `{:>9?}` would not align anything.
        let role = format!("{:?}", message.role);
        println!("  {role:>9}  {preview}{ellipsis}");
    }
}
