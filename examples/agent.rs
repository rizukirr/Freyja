//! An interactive multi-turn agent: [`Agent`] drives the tool-calling loop so
//! the example does not have to.
//!
//! [`Agent`] drives the tool loop: every turn may call tools, more than once, before the model settles on an answer. The conversation keeps the history across turns in [`InMemoryStorage`], so this example holds no transcript of its own.
//!
//! ```sh
//! cargo run --example agent
//! ```
//!
//! Type a message and press enter. `/reset` starts over, `/exit` quits.

use freyja::{Agent, Client, EndpointPreset, InMemoryStorage, StopReason, tool};
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
    let client = match Client::from_env(provider) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("{error}");
            return;
        }
    };

    let agent = Agent::new(client)
        .tool(add)
        .tool(wait)
        .system(
            "You are a concise assistant with tools. Use them when they help, \
             then answer in two sentences at most.",
        )
        .max_turns(5);

    let mut chat = agent.conversation(InMemoryStorage::new());

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
                match chat.clear().await {
                    Ok(()) => println!("transcript cleared"),
                    Err(error) => eprintln!("could not clear the transcript: {error}"),
                }
                continue;
            }
            text => text,
        };

        match chat.send(text).await {
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
