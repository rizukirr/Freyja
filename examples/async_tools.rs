//! Two slow tools, dispatched at the same time instead of one after another.
//!
//! `Agent` runs every tool call the model asks for in one turn concurrently, so
//! two calls that each take 300ms finish in about 300ms, not 600ms. The agent
//! reports the whole run's duration, which this example prints so the overlap
//! is visible.
//!
//! ```sh
//! cargo run --example async_tools
//! ```

use freyja::{Agent, Client, EndpointPreset, tool};

/// Two async tools. Each sleeps to stand in for real I/O: an HTTP call, a
/// database query, a file read.
#[tool(description = "looks up a user profile by id", strict = true)]
async fn fetch_user(id: u32) -> String {
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    format!("user {id}: Ada Lovelace, joined 2026-01-14")
}

#[tool(description = "lists the recent orders for a user id", strict = true)]
async fn fetch_orders(id: u32) -> String {
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    format!("user {id}: 3 orders, most recent 2026-08-02")
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
        .tool(fetch_user)
        .tool(fetch_orders)
        .max_turns(5);

    let start = std::time::Instant::now();
    match agent
        .ask("Look up both the profile and the recent orders for user 42.")
        .await
    {
        Ok(run) => {
            println!("assistant: {}", run.answer);
            println!("{} requests, {:?} in all", run.turns, start.elapsed());
            println!("usage: {} tokens", run.usage.total_tokens);
        }
        Err(error) => eprintln!("request failed: {error}"),
    }
}
