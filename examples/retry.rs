//! Retrying properly: what to retry, how long to wait, and when to stop.
//!
//! Freyja never retries. Sleeping needs a timer, and the library exposes
//! `async fn` without spawning so that the caller picks the runtime — this
//! example picks Tokio. What Freyja does instead is classify the failure and
//! read the endpoint's own `Retry-After`, so the loop below is short and the
//! decisions in it are not guesses.
//!
//! ```sh
//! cargo run --example retry
//! ```
//!
//! A working key usually succeeds on the first attempt, which shows none of the
//! interesting paths. To see the others:
//!
//! ```sh
//! # A port with nothing behind it: TransportError::Connect, permanent.
//! RETRY_DEMO_URL=http://127.0.0.1:1/v1 cargo run --example retry
//!
//! # A wrong key: Unauthorized, permanent.
//! OPENAI_API_KEY=sk-wrong cargo run --example retry
//! ```

use freyja::{
    Client, Dialect, EndpointConfig, EndpointPreset, Error, GenerateRequest, GenerateResponse,
    Message, Role, TransportError,
};
use std::time::Duration;
use tokio::time::sleep;

/// Give up after this many attempts.
///
/// Every retry re-sends the whole prompt and is billed again, so an unbounded
/// loop against a sustained outage is a bill, not a recovery strategy.
const MAX_ATTEMPTS: u32 = 4;

/// Never wait longer than this, however long the endpoint asks for.
///
/// A vendor under load can return a `Retry-After` measured in minutes. Honour
/// it up to a point, then fail and let the caller above decide.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let Some(client) = build_client() else {
        return;
    };

    let request =
        GenerateRequest::new().message(Message::text(Role::User, "Name three Rust crates."));

    match generate_with_retry(&client, &request).await {
        Ok(response) => {
            println!("{}", response.output_text());
            if let Some(usage) = response.usage {
                println!("\nusage: {} tokens", usage.total_tokens);
            }
        }
        Err(error) => {
            // Every variant names the endpoint, so a program holding several
            // clients can say which one failed.
            eprintln!("{} failed: {error}", error.endpoint());
            eprintln!("  {}", explain(&error));
        }
    }
}

/// Call, and try again while trying again could plausibly help.
///
/// The whole retry policy is four lines of `match`. That is the point: the
/// classification work happened inside Freyja, once, against the response it
/// alone saw.
async fn generate_with_retry(
    client: &Client,
    request: &GenerateRequest,
) -> Result<GenerateResponse, Error> {
    let mut attempt = 0;

    loop {
        match client.generate(request).await {
            Ok(response) => return Ok(response),

            // `is_retryable` is false for anything that cannot change on its
            // own: a bad key, an unknown model, a malformed request, an
            // exhausted quota. Those return immediately rather than burning
            // the attempt budget on a foregone conclusion.
            Err(error) if !error.is_retryable() => return Err(error),

            Err(error) if attempt + 1 >= MAX_ATTEMPTS => {
                eprintln!("giving up after {MAX_ATTEMPTS} attempts");
                return Err(error);
            }

            Err(error) => {
                // The endpoint's own pacing beats our arithmetic when it sent
                // any: it knows when the limit resets and we are guessing.
                let wait = error.retry_after().unwrap_or_else(|| backoff(attempt));
                let wait = wait.min(MAX_BACKOFF);

                eprintln!("attempt {} failed: {error}", attempt + 1);
                eprintln!("  retrying in {}s ({})", wait.as_secs(), explain(&error));

                sleep(wait).await;
                attempt += 1;
            }
        }
    }
}

/// Exponential backoff: 1s, 2s, 4s, 8s.
///
/// Doubling matters more than the starting value. Retrying immediately sends a
/// burst at a service that is already struggling, which is how one failed
/// request becomes an outage.
///
/// Production code adds jitter here — a small random offset, so that a thousand
/// clients that failed at the same instant do not all wake at the same instant
/// and collide again. That needs a source of randomness, which this crate has
/// no dependency for; `backon` and `tower::retry` both do it for you.
fn backoff(attempt: u32) -> Duration {
    Duration::from_secs(1u64 << attempt)
}

/// Why this error is or is not worth another attempt.
///
/// Nothing here is required to use the library — it exists so that running the
/// example teaches the variants. Real code matches on the ones it cares about
/// and lets `is_retryable` handle the rest.
fn explain(error: &Error) -> &'static str {
    match error {
        Error::RateLimit { .. } => "sending too fast — the limit resets on its own",
        Error::ServerError { .. } => "the endpoint failed on its own side",
        Error::Http { kind, .. } => match kind {
            TransportError::Timeout => "no reply in time — but the endpoint may have been billed",
            TransportError::Body => "the connection died mid-answer",
            TransportError::Connect => "unreachable: check the URL, DNS, and TLS",
            // `TransportError` is `#[non_exhaustive]` too, so this arm is
            // required and covers `Other` along with anything added later.
            _ => "the transport failed",
        },

        // The two that look alike and are not. Both arrive as 429 on
        // OpenAI-shaped endpoints; only the body separates them, and a caller
        // branching on the status alone would retry the second one forever.
        Error::QuotaExceeded { .. } => "out of credit — waiting will never fix this",

        Error::Unauthorized { .. } => "the key is wrong or lacks access to this model",
        Error::NotFound { .. } => "no such model or endpoint",
        Error::BadRequest { .. } => "the endpoint rejected the request body",
        Error::UnsupportedCapability { .. } => "this vendor cannot express that field",
        Error::InvalidRequest { .. } => "Freyja rejected the request before sending it",
        Error::InvalidResponse { .. } => "the body did not parse — report this as a bug",
        Error::Stream { .. } => "the stream failed after it had begun",

        // `#[non_exhaustive]`, so a catch-all is required and future variants
        // will not break this build.
        _ => "unclassified",
    }
}

/// The endpoint under test: OpenAI by default, or whatever `RETRY_DEMO_URL`
/// points at, so the failure paths above can be exercised without a vendor
/// outage to wait for.
fn build_client() -> Option<Client> {
    if let Ok(url) = std::env::var("RETRY_DEMO_URL") {
        eprintln!("using RETRY_DEMO_URL: {url}\n");
        // A default model, so the request below stays identical either way.
        // Without one, a request naming no model fails locally as
        // `InvalidRequest` and never reaches the transport being demonstrated.
        let config =
            EndpointConfig::new(Dialect::OpenAiChat, "demo", url).default_model("demo-model");
        return Some(Client::new(
            config,
            std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| "sk-unused".into()),
        ));
    }

    let provider = EndpointPreset::OpenAi;
    match Client::from_env(provider) {
        Some(client) => Some(client),
        None => {
            eprintln!("{} is missing or empty", provider.api_key_env());
            None
        }
    }
}
