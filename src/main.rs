mod provider;

use crate::provider::openai::create::create;
use crate::provider::openai::model::{ResponseRequest, ResponseType, TextFormat, Tool};
use dotenvy::dotenv;

#[derive(serde::Deserialize)]
struct AddArguments {
    a: i32,
    b: i32,
}

fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[tokio::main]
async fn main() {
    dotenv().ok();

    let mut request = ResponseRequest::new()
        .input("What is 20 + 22? Use the add function.")
        .max_output_tokens(100)
        .tools(vec![Tool::Function {
            name: "add".to_string(),
            description: Some("Adds two numbers together".to_string()),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "a": { "type": "integer" },
                    "b": { "type": "integer" }
                },
                "required": ["a", "b"],
                "additionalProperties": false
            }),
            strict: Some(true),
        }])
        .text_format(TextFormat::text());

    let api_key = match std::env::var("OPENAI_API_KEY") {
        Ok(key) => {
            if key.is_empty() {
                eprintln!("OPENAI_API_KEY is empty. Add it to .env");
                return;
            }
            key
        }
        Err(_) => {
            eprintln!("OPENAI_API_KEY is missing. Add it to .env");
            return;
        }
    };
    match create(&api_key, &mut request).await {
        Ok(response) => {
            if let Some(error) = &response.error {
                eprintln!("Response error: {}", error.message);
                return;
            }

            let mut items = response.items().peekable();

            if items.peek().is_none() {
                println!("Response contains no supported output");
                return;
            }

            for item in items {
                match item {
                    ResponseType::Text(text) => println!("Assistant: {text}"),
                    ResponseType::Refusal(reason) => {
                        eprintln!("Request refused: {reason}");
                    }
                    ResponseType::FunctionCall(call) => match call.name {
                        "add" => match call.parse_arguments::<AddArguments>() {
                            Ok(arguments) => {
                                let result = add(arguments.a, arguments.b);
                                println!("Function {} returned: {result}", call.name);
                            }
                            Err(error) => {
                                eprintln!("Invalid arguments for {}: {error}", call.name);
                            }
                        },
                        name => eprintln!("Unknown function requested: {name}"),
                    },
                }
            }
        }
        Err(error) => eprintln!("Request failed: {error}"),
    }
}
