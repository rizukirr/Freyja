//! Asks real endpoints what they do with a transcript whose tool calls and
//! results are not properly paired.
//!
//! Nothing in this crate can answer that question. Three claims about provider
//! pairing rules were written into freyja as comments and enforced as
//! behaviour, and all three were wrong or incomplete until this probe was run.
//! It is an example rather than a test because it needs keys, a network and
//! money, and it makes real calls.
//!
//! Set any of `OPENAI_API_KEY`, `DEEPSEEK_API_KEY` or `ANTHROPIC_API_KEY` in
//! the environment or in a `.env` file. An endpoint with no key is skipped.
//! DeepSeek serves an OpenAI Chat compatible API and an Anthropic compatible
//! one, which is how two dialects are covered by one key.
//!
//! An Anthropic key scoped to all workspaces also needs
//! `ANTHROPIC_WORKSPACE_ID`, since such a key is identity-linked and every
//! request has to name the workspace it acts in. Measured, the two Anthropic
//! endpoints agree on every case here, so the compatible one is a faithful
//! stand-in when no Anthropic key is to hand.
//!
//! ```text
//! cargo run --example pairing_probe
//! ```

use freyja::{
    Client, Dialect, EndpointConfig, GenerateRequest, InputContent, Message, Role, ToolDefinition,
};

fn calls(ids: &[&str]) -> Message {
    Message::new(
        Role::Assistant,
        ids.iter()
            .map(|id| InputContent::ToolCall {
                id: (*id).into(),
                name: "get_time".into(),
                arguments: "{}".into(),
            })
            .collect::<Vec<_>>(),
    )
}

fn tool() -> ToolDefinition {
    ToolDefinition::new("get_time", "returns the current time").parameters(serde_json::json!({
        "type": "object", "properties": {}, "additionalProperties": false
    }))
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let base = GenerateRequest::new()
        .tools(vec![tool()])
        .message(Message::text(Role::User, "what time is it?"));

    let cases: Vec<(&str, GenerateRequest)> = vec![
        (
            "control, a well formed pair",
            base.clone()
                .message(calls(&["c1"]))
                .message(Message::tool_result("c1", "12:00")),
        ),
        (
            "orphan result, no call anywhere",
            base.clone().message(Message::tool_result("c1", "12:00")),
        ),
        (
            "orphan call, no result anywhere",
            base.clone().message(calls(&["c1"])),
        ),
        (
            "result before its call",
            base.clone()
                .message(Message::tool_result("c1", "12:00"))
                .message(calls(&["c1"])),
        ),
        (
            "user turn between the call and its result",
            base.clone()
                .message(calls(&["c1"]))
                .message(Message::text(Role::User, "wait"))
                .message(Message::tool_result("c1", "12:00")),
        ),
        (
            "developer turn between the call and its result",
            base.clone()
                .message(calls(&["c1"]))
                .message(Message::text(Role::Developer, "note"))
                .message(Message::tool_result("c1", "12:00")),
        ),
        (
            "parallel calls, one result message each",
            base.clone()
                .message(calls(&["c1", "c2"]))
                .message(Message::tool_result("c1", "12:00"))
                .message(Message::tool_result("c2", "13:00")),
        ),
        (
            "parallel calls, both results in one message",
            base.clone()
                .message(calls(&["c1", "c2"]))
                .message(Message::new(
                    Role::Tool,
                    vec![
                        InputContent::ToolResult {
                            call_id: "c1".into(),
                            output: "12:00".into(),
                        },
                        InputContent::ToolResult {
                            call_id: "c2".into(),
                            output: "13:00".into(),
                        },
                    ],
                )),
        ),
    ];

    let endpoints = [
        (
            "OpenAI Responses",
            "OPENAI_API_KEY",
            Dialect::OpenAiResponses,
            "https://api.openai.com/v1",
            "gpt-4o-mini",
        ),
        (
            "OpenAI Chat, via DeepSeek",
            "DEEPSEEK_API_KEY",
            Dialect::OpenAiChat,
            "https://api.deepseek.com/v1",
            "deepseek-chat",
        ),
        (
            "Anthropic, via DeepSeek",
            "DEEPSEEK_API_KEY",
            Dialect::Anthropic,
            "https://api.deepseek.com/anthropic",
            "deepseek-chat",
        ),
        (
            "Anthropic",
            "ANTHROPIC_API_KEY",
            Dialect::Anthropic,
            "https://api.anthropic.com/v1",
            "claude-haiku-4-5-20251001",
        ),
    ];

    for (label, variable, dialect, url, model) in endpoints {
        let Ok(key) = std::env::var(variable) else {
            println!("{label}: no {variable}, skipped");
            continue;
        };
        let mut config = EndpointConfig::new(dialect, label, url).default_model(model);

        // An Anthropic key scoped to all workspaces is identity-linked, and
        // every request from one has to name the workspace it acts in. The
        // console shows a dash rather than an id for such a key, and a literal
        // dash is refused, so the id comes from the environment.
        if url.starts_with("https://api.anthropic.com") {
            match std::env::var("ANTHROPIC_WORKSPACE_ID") {
                Ok(workspace) => config = config.header("anthropic-workspace-id", workspace),
                Err(_) => println!(
                    "{label}: no ANTHROPIC_WORKSPACE_ID, which an identity-linked key requires"
                ),
            }
        }

        let client = Client::new(config, key);

        println!("\n{label}");
        for (name, request) in &cases {
            let verdict = match client.generate(request).await {
                Ok(_) => "accepted".to_string(),
                Err(error) => {
                    let message: String = error
                        .to_string()
                        .replace('\n', " ")
                        .chars()
                        .take(140)
                        .collect();
                    format!("rejected: {message}")
                }
            };
            println!("  {name}: {verdict}");
        }
    }
}
