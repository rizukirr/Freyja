//! A one-tool agent loop, the pattern every larger agent is built from.
//!
//! Asks a question the model cannot answer alone, executes the tool it asks
//! for, feeds the result back, and prints the final answer.
//!
//! ```sh
//! cargo run --example tool_loop
//! ```
//!
//! Swap `EndpointPreset::OpenAi` below for `Gemini` or `Anthropic` to run the
//! same code against a different vendor. Nothing else changes, which is the
//! point of the neutral model.

use freyja::{Client, EndpointPreset, GenerateRequest, Message, OutputContent, Role, Tool, tool};

/// The tools this example exposes to the model.
#[tool(description = "adds two numbers together", strict = true)]
fn add(a: i64, b: i64) -> i64 {
    a + b
}

/// An async tool. Any `.await` works here: an HTTP call, a database query, or
/// a sleep, as in this case.
#[tool(description = "waits for the given number of milliseconds, then confirms")]
async fn wait(milliseconds: u64) -> String {
    tokio::time::sleep(std::time::Duration::from_millis(milliseconds)).await;
    format!("waited {milliseconds}ms")
}

/// Runs a tool call and returns the output to send back to the model.
async fn dispatch(tools: &[Tool], name: &str, arguments: &str) -> String {
    match tools.iter().copied().find(|tool| tool.name() == name) {
        Some(tool) => tool
            .execute(arguments)
            .await
            .unwrap_or_else(|error| format!("error: {error:?}")),
        None => format!("error: unknown tool '{name}'"),
    }
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let provider = EndpointPreset::OpenAi;
    let Some(client) = Client::from_env(provider) else {
        eprintln!("{} is missing or empty", provider.api_key_env());
        return;
    };

    let tools = [add, wait];
    let definitions = tools
        .iter()
        .map(|tool| tool.definition())
        .collect::<Vec<_>>();

    let mut request = GenerateRequest::new()
        .message(Message::text(
            Role::User,
            "What is 20 + 22, and once you have it, wait 500 milliseconds before confirming?",
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

        let mut results: Vec<Message> = Vec::new();
        for (id, name, arguments) in response.tool_calls() {
            let output = dispatch(&tools, name, arguments).await;
            println!("tool result: {output}");
            results.push(Message::tool_result(id, output));
        }

        request = request
            .message(response.to_message())
            .extend_messages(results);
    }

    eprintln!("stopped: reached the maximum number of tool-calling rounds");
}
