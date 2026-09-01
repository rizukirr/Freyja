//! An interactive multi-turn chat, streamed.
//!
//! The other examples each make one call. This one holds a conversation, which
//! is a different problem: the transcript is yours to keep, every turn re-sends
//! all of it, and a failed call must not leave a dangling turn behind.
//!
//! ```sh
//! cargo run --example chat
//! ```
//!
//! Type a message and press enter. `/reset` starts over, `/history` shows the
//! transcript, `/exit` quits.

use freyja::{Client, EndpointPreset, GenerateRequest, Message, ResponseStatus, Role, StreamEvent};
use std::io::{BufRead, Write};

/// Frames the whole conversation. Freyja puts it wherever the provider expects
/// it, a top-level field on some, an ordinary turn on others, so the same
/// line works on every vendor.
const SYSTEM_PROMPT: &str = "You are a concise assistant. Answer in two sentences at most.";

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let provider = EndpointPreset::OpenAi;
    let Some(client) = Client::from_env(provider) else {
        eprintln!("{} is missing or empty", provider.api_key_env());
        return;
    };

    // The transcript lives in the request and grows as the conversation does.
    // There is no session object and no hidden state on the client: what you
    // send is exactly what the model sees.
    let mut request = new_conversation();

    // The endpoint's configured name, the same one that shows up in errors.
    println!(
        "chatting with {}. /reset, /history, /exit",
        client.config().name
    );

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

        match line.trim() {
            "" => continue,
            "/exit" | "/quit" => break,
            "/reset" => {
                request = new_conversation();
                println!("transcript cleared");
                continue;
            }
            "/history" => {
                print_history(&request);
                continue;
            }
            text => request = request.message(Message::text(Role::User, text)),
        }

        match say(&client, &request).await {
            // The model's turn has to go back into the transcript, or the next
            // question arrives with no memory of the answer to this one.
            Ok(reply) => request = request.message(reply),

            Err(error) => {
                eprintln!("\n{} failed: {error}", error.endpoint());
                if error.is_retryable() {
                    eprintln!("(transient, try again)");
                }

                // Drop the user turn that was never answered. Leaving it would
                // send a transcript ending in an unanswered question, which
                // some providers reject and all of them bill for.
                request.messages.pop();
            }
        }
    }

    println!("bye");
}

/// Streams one answer, printing it as it arrives, and returns the assistant
/// turn to append to the transcript.
async fn say(client: &Client, request: &GenerateRequest) -> Result<Message, freyja::Error> {
    let mut stream = client.stream(request).await?;

    print!("\nbot> ");
    let _ = std::io::stdout().flush();

    while let Some(event) = stream.next().await? {
        match event {
            StreamEvent::TextDelta(text) | StreamEvent::RefusalDelta(text) => {
                print!("{text}");
                // Deltas arrive mid-line, so nothing appears without a flush.
                let _ = std::io::stdout().flush();
            }
            StreamEvent::Done {
                usage: Some(usage), ..
            } => println!("\n[{} tokens]", usage.total_tokens),
            _ => {}
        }
    }

    // A drained stream converts back into the same response `generate` would
    // have returned, so transcript handling is identical either way.
    let response = stream.into_response()?;

    // A short answer is not always a finished one. Without this check a reply
    // truncated by a token cap reads as a complete thought.
    if response.status == ResponseStatus::Incomplete {
        println!("\n[truncated: the answer was cut short]");
    }

    Ok(response.to_message())
}

/// A fresh transcript, carrying only the system framing.
fn new_conversation() -> GenerateRequest {
    GenerateRequest::new().message(Message::text(Role::System, SYSTEM_PROMPT))
}

/// Shows what the next call will actually send.
///
/// Worth looking at once: the whole transcript goes over the wire every turn,
/// so a long conversation costs more per question than a short one, and the
/// growth is quadratic rather than linear. Trimming or summarizing old turns is
/// the caller's job today, see the roadmap's Phase 3.
fn print_history(request: &GenerateRequest) {
    println!("{} turns:", request.messages.len());
    for message in &request.messages {
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
