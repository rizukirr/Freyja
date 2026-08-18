//! Prints a model's answer as it arrives.
//!
//! Run with: `cargo run --example streaming`

use freyja::{Client, EndpointPreset, GenerateRequest, Message, Role, StreamEvent};
use std::io::Write;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    let client = Client::from_env(EndpointPreset::OpenAi).ok_or("OPENAI_API_KEY is unset")?;
    let request = GenerateRequest::new()
        .message(Message::text(Role::User, "Name three primary colors."))
        .max_tokens(128);

    let mut stream = client.stream(&request).await?;
    while let Some(event) = stream.next().await? {
        match event {
            StreamEvent::TextDelta(text) => {
                print!("{text}");
                // Deltas arrive mid-line, so nothing appears without a flush.
                std::io::stdout().flush()?;
            }
            StreamEvent::ToolCall {
                name, arguments, ..
            } => println!("\n[tool] {name}({arguments})"),
            StreamEvent::Done {
                usage: Some(usage), ..
            } => println!("\n\n[{} tokens]", usage.total_tokens),
            _ => {}
        }
    }

    Ok(())
}
