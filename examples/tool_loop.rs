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

/// The single tool this example exposes to the model.
#[tool(description = "adds two numbers together", strict = true)]
fn add(a: i64, b: i64) -> i64 {
    a + b
}

/// Runs a tool call and returns the output to send back to the model.
fn dispatch(tools: &[Tool], name: &str, arguments: &str) -> String {
    match tools.iter().copied().find(|tool| tool.name() == name) {
        Some(tool) => tool
            .execute(arguments)
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

    let tools = [add];
    let definitions = tools
        .iter()
        .map(|tool| tool.definition())
        .collect::<Vec<_>>();

    let mut request = GenerateRequest::new()
        .message(Message::text(Role::User, "What is 20 + 22?"))
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

        let results: Vec<Message> = response
            .tool_calls()
            .map(|(id, name, arguments)| {
                let output = dispatch(&tools, name, arguments);
                println!("tool result: {output}");
                Message::tool_result(id, output)
            })
            .collect();

        request = request
            .message(response.to_message())
            .extend_messages(results);
    }

    eprintln!("stopped: reached the maximum number of tool-calling rounds");
}
