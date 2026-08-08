//! A one-tool agent loop, the pattern every larger agent is built from.
//!
//! Asks a question the model cannot answer alone, executes the tool it asks
//! for, feeds the result back, and prints the final answer.
//!
//! ```sh
//! cargo run --example tool_loop
//! ```
//!
//! Swap `ProviderType::OpenAi` below for `Gemini` or `Anthropic` to run the
//! same code against a different vendor. Nothing else changes, which is the
//! point of the neutral model.

use freya::{Client, GenerateRequest, Message, OutputContent, ProviderType, Role, ToolDefinition};
use serde_json::Value;

/// The single tool this example exposes to the model.
fn add(a: i64, b: i64) -> i64 {
    a + b
}

/// Runs a tool call and returns the output to send back to the model.
fn dispatch(name: &str, arguments: &str) -> String {
    let parsed: Value = match serde_json::from_str(arguments) {
        Ok(value) => value,
        Err(error) => return format!("error: arguments were not valid JSON: {error}"),
    };

    match name {
        "add" => match (parsed["a"].as_i64(), parsed["b"].as_i64()) {
            (Some(a), Some(b)) => add(a, b).to_string(),
            _ => "error: both 'a' and 'b' must be integers".to_string(),
        },
        other => format!("error: unknown tool '{other}'"),
    }
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let provider = ProviderType::OpenAi;
    let Some(client) = Client::from_env(provider) else {
        eprintln!("{} is missing or empty", provider.api_key_env());
        return;
    };

    let add_tool =
        ToolDefinition::new("add", "adds two numbers together").parameters(serde_json::json!({
            "type": "object",
            "properties": {
                "a": {"type": "integer"},
                "b": {"type": "integer"}
            },
            "required": ["a", "b"]
        }));

    let mut request = GenerateRequest::new()
        .message(Message::text(Role::User, "What is 20 + 22?"))
        .tools([add_tool]);

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
                let output = dispatch(name, arguments);
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
