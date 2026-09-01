//! An interactive multi-turn agent: [`Agent`] drives the tool-calling loop so
//! the example does not have to.
//!
//! `tool_loop` writes that loop by hand, once, for one question. `chat` holds
//! a plain conversation with no tools at all. This example needs both: every
//! turn may call tools, more than once, before the model settles on an
//! answer, and across turns the conversation itself continues. This example
//! owns that transcript directly, as an explicit `Vec<Message>` it hands to a
//! fresh [`freyja::Conversation`] on every turn.
//!
//! ```sh
//! cargo run --example agent
//! ```
//!
//! Type a message and press enter. `/reset` starts over, `/exit` quits.

use freyja::{Agent, Client, EndpointPreset, Message, StopReason, tool};
use std::io::{BufRead, Write};

/// A sync tool: no `.await` anywhere in its body.
#[tool(description = "adds two numbers together", strict = true)]
fn add(a: i64, b: i64) -> i64 {
    a + b
}

/// An async tool. `Agent` dispatches every tool call the model asks for in one
/// turn concurrently, so several of these in flight together still take as
/// long as the slowest one, not their sum.
#[tool(description = "waits for the given number of milliseconds, then confirms")]
async fn wait(milliseconds: u64) -> String {
    tokio::time::sleep(std::time::Duration::from_millis(milliseconds)).await;
    format!("waited {milliseconds}ms")
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

    // The transcript is ours to keep: `Agent` holds no state across calls, so
    // this vector is the only place the conversation lives.
    let mut messages: Vec<Message> = Vec::new();

    println!("chatting with tools. /reset, /exit");

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
                messages.clear();
                println!("transcript cleared");
                continue;
            }
            text => text,
        };

        // A fresh conversation over the same vector every turn: `send` only
        // appends the user turn and the answer once the run succeeds, so a
        // failed run leaves the transcript exactly as it found it.
        match agent.conversation(&mut messages).send(text).await {
            Ok(run) => {
                println!("\nbot> {}", run.answer);
                if run.stop == StopReason::MaxTurns {
                    eprintln!(
                        "(stopped: reached the maximum number of turns without a final answer)"
                    );
                }
            }
            Err(error) => {
                eprintln!("\n{} failed: {error}", error.endpoint());
                if error.is_retryable() {
                    eprintln!("(transient, try again)");
                }
            }
        }
    }

    println!("bye");
}
