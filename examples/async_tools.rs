//! Two slow tools, dispatched at the same time instead of one after another.
//!
//! Freyja spawns nothing and depends on no async runtime, so the caller
//! picks one, this example picks Tokio. That means concurrency is the
//! caller's job too: `Tool::call` borrows the tool, its arguments and the run
//! context, so a task that outlives the call must own all of them, a cloned
//! `Arc<dyn Tool>` and owned strings, before it is spawned.
//!
//! ```sh
//! cargo run --example async_tools
//! ```

use freyja::{
    Client, Context, EndpointPreset, GenerateRequest, Message, OutputContent, Role, Tool, tool,
};
use std::sync::Arc;

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
    let Some(client) = Client::from_env(provider) else {
        eprintln!("{} is missing or empty", provider.api_key_env());
        return;
    };

    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(fetch_user), Arc::new(fetch_orders)];
    let definitions = tools
        .iter()
        .map(|tool| tool.definition())
        .collect::<Vec<_>>();

    let mut request = GenerateRequest::new()
        .message(Message::text(
            Role::User,
            "Look up both the profile and the recent orders for user 42.",
        ))
        .tools(definitions);

    // Bounded loop: call, run whatever tools the model asks for, call again.
    for _ in 0..5 {
        let response = match client.generate(&request).await {
            Ok(response) => response,
            Err(error) => {
                eprintln!("request failed: {error}");
                return;
            }
        };

        for content in &response.content {
            match content {
                OutputContent::Text(text) => println!("assistant: {text}"),
                OutputContent::Refusal(text) => eprintln!("refusal: {text}"),
                OutputContent::ToolCall {
                    name, arguments, ..
                } => println!("tool call: {name}({arguments})"),
                // Opaque provider state, carried back by to_message().
                OutputContent::Reasoning { .. } => {}
            }
        }

        if !response.has_tool_calls() {
            if let Some(usage) = response.usage {
                println!("usage: {} tokens", usage.total_tokens);
            }
            return;
        }

        let start = std::time::Instant::now();

        let mut handles = Vec::new();
        for (id, name, arguments) in response.tool_calls() {
            let (id, arguments) = (id.to_owned(), arguments.to_owned());
            let tool = tools
                .iter()
                .find(|tool| tool.name() == name)
                .map(Arc::clone);
            handles.push(tokio::spawn(async move {
                let cx = Context::new();
                let output = match tool {
                    Some(tool) => tool
                        .call(&arguments, &cx)
                        .await
                        .unwrap_or_else(|error| format!("error: {error}")),
                    None => "error: unknown tool".to_string(),
                };
                (id, output)
            }));
        }

        let mut results: Vec<Message> = Vec::new();
        for handle in handles {
            let (id, output) = handle.await.expect("tool task panicked");
            println!("tool result: {output}");
            results.push(Message::tool_result(id, output));
        }

        println!("tool round took {:?}", start.elapsed());

        request = request
            .message(response.to_message())
            .extend_messages(results);
    }

    eprintln!("stopped: reached the maximum number of tool-calling rounds");
}
