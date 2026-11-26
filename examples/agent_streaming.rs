//! An agent whose answer is printed as it is written.
//!
//! `agent` waits for the whole run and prints the answer at the end. This
//! example runs the same loop through [`freyja::Conversation::send_streaming`]:
//! text appears chunk by chunk, each tool call is shown as the model makes it,
//! and each tool's result is shown before it goes back to the model.
//!
//! ```sh
//! cargo run --example agent_streaming
//! ```
//!
//! Type a message and press enter. `/reset` starts over, `/exit` quits. Try
//! "what is 20 + 22, and then wait 300ms" to see a tool turn stream.

use freyja::{Agent, AgentEvent, Client, EndpointPreset, InMemoryStorage, StreamEvent, tool};
use std::io::{BufRead, Write};

#[tool(description = "adds two numbers together", strict = true)]
fn add(a: i64, b: i64) -> i64 {
    a + b
}

#[tool(description = "waits for the given number of milliseconds, then confirms")]
async fn wait(milliseconds: u64) -> String {
    tokio::time::sleep(std::time::Duration::from_millis(milliseconds)).await;
    format!("waited {milliseconds}ms")
}

/// Prints one event. Text is flushed per chunk, or the terminal would hold it
/// until a newline and the streaming would be invisible.
fn show(event: AgentEvent) {
    let mut out = std::io::stdout();
    match event {
        AgentEvent::Model(StreamEvent::TextDelta(chunk) | StreamEvent::RefusalDelta(chunk)) => {
            print!("{chunk}");
        }
        AgentEvent::Model(StreamEvent::ToolCall {
            name, arguments, ..
        }) => {
            println!("\n  [calling {name}({arguments})]");
        }
        AgentEvent::ToolResult { name, output, .. } => {
            println!("  [{name} returned: {output}]");
        }
        // Reasoning, replay state and the per-request `Done` are not shown.
        _ => {}
    }
    let _ = out.flush();
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let provider = EndpointPreset::OpenAi;
    let Some(client) = Client::from_env(provider) else {
        eprintln!("{} is missing or empty", provider.api_key_env());
        return;
    };

    let agent = Agent::new(client)
        .tool(add)
        .tool(wait)
        .system(
            "You are a concise assistant with tools. Use them when they help, \
             then answer in two sentences at most.",
        )
        .max_turns(5);

    // The conversation owns its transcript, so it carries across turns.
    let mut chat = agent.conversation(InMemoryStorage::new());

    println!("streaming chat with tools. /reset, /exit");

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
            text => text,
        };

        print!("\nbot> ");
        let _ = std::io::stdout().flush();

        // The chunks go to `show` while the model writes. The `Run` comes back
        // once, at the end, with the totals.
        match chat.send_streaming(text, show).await {
            Ok(run) => {
                println!();
                eprintln!(
                    "({} requests, {} tokens, {:?})",
                    run.turns, run.usage.total_tokens, run.stop
                );
            }
            Err(error) => {
                // Text already printed stays on screen, but a failed run
                // stores nothing, so the next turn starts from before it.
                eprintln!("\n{} failed: {error}", error.endpoint());
                if error.is_retryable() {
                    eprintln!("(transient, try again)");
                }
            }
        }
    }

    println!("bye");
}
