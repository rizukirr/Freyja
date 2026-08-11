//! One request, every vendor — and the honest limits of that.
//!
//! This is the claim the crate is built on: you describe what you want once,
//! and changing vendor changes one line. The first section proves it. The
//! second shows what happens when a request asks for something a vendor cannot
//! express, which is the more interesting case. The third asks the same
//! question with `Client::check`, which reaches the same verdicts without
//! sending anything.
//!
//! ```sh
//! cargo run --example portable
//! ```
//!
//! Set as many of `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, and `GEMINI_API_KEY`
//! as you have. Endpoints with no key are skipped, so one key is enough to run
//! it — though the point lands better with two.

use freyja::{
    Client, GenerateRequest, Message, ProviderError, ProviderType, ReasoningEffort, Role,
};

const PROVIDERS: [ProviderType; 3] = [
    ProviderType::OpenAi,
    ProviderType::Anthropic,
    ProviderType::Gemini,
];

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    // No model, no sampling controls. Every unset field means "the endpoint
    // decides", which is what keeps one request valid everywhere: a value that
    // looks harmless on one vendor is a rejection on another.
    let portable = GenerateRequest::new()
        .message(Message::text(Role::System, "Answer in exactly four words."))
        .message(Message::text(Role::User, "What is Rust good at?"))
        .max_tokens(64);

    println!("== the same request, on every endpoint with a key ==");
    run_on_all(&portable).await;

    // Now ask for something not every vendor has. `Minimal` is a portable name
    // for a per-vendor idea, and two of the three cannot honour it:
    //
    //   OpenAI     maps it to its own minimal effort
    //   Anthropic  has thinking budgets, but no floor this low
    //   Gemini     has no portable effort scale at all
    //
    // Freyja refuses rather than dropping the field, so you learn this from an
    // error before the network call instead of from an answer that quietly
    // ignored you. `tool_choice` behaves the same way on Gemini.
    let demanding = portable.reasoning_effort(ReasoningEffort::Minimal);

    println!("\n== the same request, plus a capability not everyone has ==");
    run_on_all(&demanding).await;

    // The same fact, learned the other way round.
    survey(&demanding);
}

/// Asks every endpoint whether it could carry a request, without sending it.
///
/// `check` runs the conversion `generate` runs and throws the result away, so
/// the verdicts below are the same ones the section above paid a network round
/// trip to discover. No key is used and nothing is sent.
///
/// Worth doing when you need the answer *before* committing to a vendor —
/// picking one at runtime, or greying out an option in a UI — rather than as a
/// guard in front of every call, where the error from `generate` says the same
/// thing.
fn survey(request: &GenerateRequest) {
    println!("\n== which of them could carry it, asked rather than tried ==");

    for provider in PROVIDERS {
        let Some(client) = Client::from_env(provider) else {
            continue;
        };
        let name = client.config().name.clone();

        match client.check(request) {
            Ok(()) => println!("{name:>9}  yes"),
            // Note this is the same error, verbatim, that `run_on_all` printed
            // above. It is the same code path, so the two cannot disagree.
            Err(error) => println!("{name:>9}  no — {error}"),
        }
    }
}

/// Sends one request to every endpoint that has a key, in turn.
async fn run_on_all(request: &GenerateRequest) {
    let mut ran = 0;

    for provider in PROVIDERS {
        // The one line that changes. Everything below it is vendor-agnostic.
        let Some(client) = Client::from_env(provider) else {
            println!("\n{:>9}  skipped, {} unset", "—", provider.api_key_env());
            continue;
        };
        ran += 1;

        let name = client.config().name.clone();
        match client.generate(request).await {
            Ok(response) => {
                // `model` is what actually served the request, which is not
                // always what was asked for: the preset's default filled in
                // here, and vendors substitute dated snapshots of their own.
                println!("\n{name:>9}  {}", response.output_text().trim());
                println!("{:>9}  via {}", "", response.model);
                if let Some(usage) = response.usage {
                    println!("{:>9}  {} tokens", "", usage.total_tokens);
                }
            }

            // The refusal that makes portability honest. It is raised by the
            // dialect before anything is sent, so it costs nothing and cannot
            // be confused with the vendor rejecting the request itself.
            Err(ProviderError::UnsupportedCapability { capability, .. }) => {
                println!("\n{name:>9}  refused: cannot express {capability}");
                println!("{:>9}  (no request was sent)", "");
            }

            Err(error) => {
                println!("\n{name:>9}  failed: {error}");
                if error.is_retryable() {
                    println!("{:>9}  transient — worth another attempt", "");
                }
            }
        }
    }

    if ran == 0 {
        println!("\nno keys set — nothing to compare");
    }
}
