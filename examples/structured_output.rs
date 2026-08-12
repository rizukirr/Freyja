//! Asking for JSON and getting a Rust value back.
//!
//! `ResponseFormat::JsonSchema` constrains the model's output to a shape you
//! declare, and `Client::generate_as` hands back that shape rather than a
//! string. The schema is still written by hand and must be kept in step with
//! the struct: deriving one from the type is not implemented.
//!
//! ```sh
//! cargo run --example structured_output
//! ```

use freyja::{Client, GenerateRequest, Message, ProviderError, ProviderType, ResponseFormat, Role};
use serde::Deserialize;
use serde_json::json;

/// The shape we want back.
///
/// The schema below has to be written by hand to match it. Deriving one from
/// this struct is on the roadmap and does not exist yet, so the two can drift:
/// keep them next to each other, and let the deserialize step catch it when
/// they do.
#[derive(Debug, Deserialize)]
struct Recommendation {
    name: String,
    purpose: String,
    /// `Option`, because a model that omits a field it was told was required is
    /// a thing that happens on endpoints without strict enforcement.
    maturity: Option<String>,
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let provider = ProviderType::OpenAi;
    let Some(client) = Client::from_env(provider) else {
        eprintln!("{} is missing or empty", provider.api_key_env());
        return;
    };

    // Strict mode has requirements beyond ordinary JSON Schema, and they are
    // the usual reason a first attempt is rejected: every property must appear
    // in `required`, and `additionalProperties` must be false. An optional
    // field is spelled as a nullable type, not as an absent requirement.
    let schema = json!({
        "type": "object",
        "properties": {
            "name":     {"type": "string"},
            "purpose":  {"type": "string"},
            "maturity": {"type": ["string", "null"]}
        },
        "required": ["name", "purpose", "maturity"],
        "additionalProperties": false
    });

    let request = GenerateRequest::new()
        .message(Message::text(
            Role::User,
            "Recommend one Rust crate for parsing JSON.",
        ))
        .response_format(ResponseFormat::JsonSchema {
            name: "recommendation".into(),
            schema,
            // Ask the provider to enforce the schema rather than treat it as a
            // hint. Where that is supported it removes a whole class of
            // retry-and-hope code.
            strict: true,
        });

    // `generate_as` sends the request and deserializes for you. The manual
    // route still works -- `generate` then `output_text` then `from_str` -- and
    // is what you want when the raw text matters as much as the value.
    match client.generate_as::<Recommendation>(&request).await {
        Ok(recommendation) => {
            println!("name:     {}", recommendation.name);
            println!("purpose:  {}", recommendation.purpose);
            println!(
                "maturity: {}",
                recommendation.maturity.as_deref().unwrap_or("(not given)")
            );
        }

        // Worth handling rather than unwrapping: a schema constrains the model,
        // it does not guarantee anything. The error keeps the text so you can
        // see what actually came back, and separates the common cause from the
        // rest -- a cut-off answer is still valid text and invalid JSON, and
        // wants a bigger cap rather than a different schema.
        Err(ProviderError::OutputMismatch {
            message,
            text,
            truncated,
            ..
        }) => {
            if truncated {
                eprintln!("the answer was cut short by max_tokens");
            }
            eprintln!("did not match the struct: {message}");
            eprintln!("what came back: {text}");
        }

        Err(error) => eprintln!("{} failed: {error}", error.provider()),
    }

    demonstrate_schema_less().await;
}

/// The looser form, and where it is not available.
///
/// `JsonObject` asks for "any valid JSON" with no schema. OpenAI and Gemini
/// offer it; Anthropic has no equivalent and Freyja refuses locally rather than
/// sending a request whose central instruction it knows will be ignored.
///
/// Run against every endpoint with a key, because the refusal is the point and
/// it only shows on one of them.
async fn demonstrate_schema_less() {
    let request = GenerateRequest::new()
        .message(Message::text(
            Role::User,
            "Give me a JSON object with one key, 'answer', set to 42.",
        ))
        .response_format(ResponseFormat::JsonObject);

    println!("\n== the same call, schema-less ==");

    for provider in [
        ProviderType::OpenAi,
        ProviderType::Gemini,
        ProviderType::Anthropic,
    ] {
        let Some(client) = Client::from_env(provider) else {
            continue;
        };
        let name = client.config().name.clone();

        match client.generate(&request).await {
            Ok(response) => println!("{name:>9}  {}", response.output_text().trim()),
            Err(ProviderError::UnsupportedCapability { capability, .. }) => {
                println!("{name:>9}  refused: cannot express {capability}");
                println!("{:>9}  (no request was sent)", "");
            }
            Err(error) => eprintln!("{name:>9}  failed: {error}"),
        }
    }
}
