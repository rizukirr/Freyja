//! Asking for JSON and getting a Rust value back.
//!
//! `ResponseFormat::JsonSchema` constrains the model's output to a shape you
//! declare, so the answer can be deserialized instead of parsed out of prose.
//! Freyja hands you the JSON as text; turning it into a type is `serde_json`'s
//! job, and this example does both halves.
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

    match client.generate(&request).await {
        Ok(response) => {
            // The answer is still text on the wire. Freyja does not deserialize
            // it for you, because it has no idea what type you wanted.
            let raw = response.output_text();
            println!("raw JSON:\n{raw}\n");

            match serde_json::from_str::<Recommendation>(&raw) {
                Ok(recommendation) => {
                    println!("name:     {}", recommendation.name);
                    println!("purpose:  {}", recommendation.purpose);
                    println!(
                        "maturity: {}",
                        recommendation.maturity.as_deref().unwrap_or("(not given)")
                    );
                }
                // Worth handling rather than unwrapping: a schema is a
                // constraint on the model, not a guarantee from the transport.
                // A truncated answer is still valid text and invalid JSON.
                Err(error) => eprintln!("the answer did not match the struct: {error}"),
            }
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
