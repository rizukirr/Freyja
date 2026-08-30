//! Wire-level checks that a header named by two layers goes out once.
//!
//! `reqwest` appends rather than replaces, so these count occurrences in the
//! captured request head rather than looking for a value.

use freyja::{Auth, Client, Dialect, EndpointConfig, GenerateRequest, Message, Role};

mod common;
use common::{ok_response, serve};

/// Every occurrence of `name` in the captured head, value only, lowercased
/// name so a server's own casing does not decide the outcome.
fn header_values(head: &str, name: &str) -> Vec<String> {
    head.lines()
        .filter_map(|line| line.split_once(':'))
        .filter(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim().to_string())
        .collect()
}

async fn send_with(config: EndpointConfig, requests: std::sync::mpsc::Receiver<String>) -> String {
    Client::new(config, "sk-test")
        .generate(&GenerateRequest::new().message(Message::text(Role::User, "Hi")))
        .await
        .expect("request succeeds");
    requests.recv().expect("captured request")
}

#[tokio::test]
async fn auth_replaces_a_caller_supplied_authorization_header() {
    let (base, requests) = serve(&[ok_response()]);
    let config = EndpointConfig::new(Dialect::OpenAiChat, "local", base)
        .default_model("test-model")
        .header("Authorization", "Bearer gateway-token");

    let sent = send_with(config, requests).await;
    assert_eq!(header_values(&sent, "authorization"), ["Bearer sk-test"]);
}

#[tokio::test]
async fn auth_replaces_a_caller_supplied_named_key_header() {
    let (base, requests) = serve(&[ok_response()]);
    let config = EndpointConfig::new(Dialect::OpenAiChat, "local", base)
        .default_model("test-model")
        .auth(Auth::Header("x-api-key"))
        .header("X-Api-Key", "gateway-token");

    let sent = send_with(config, requests).await;
    assert_eq!(header_values(&sent, "x-api-key"), ["sk-test"]);
}

#[tokio::test]
async fn an_extra_header_supersedes_a_dialect_required_one() {
    let (base, requests) = serve(&[ok_response()]);
    let config = EndpointConfig::new(Dialect::Anthropic, "local", base)
        .default_model("test-model")
        .header("anthropic-version", "2024-10-22");

    let sent = send_with(config, requests).await;
    assert_eq!(header_values(&sent, "anthropic-version"), ["2024-10-22"]);
}

#[tokio::test]
async fn the_last_header_call_wins() {
    let (base, requests) = serve(&[ok_response()]);
    let config = EndpointConfig::new(Dialect::OpenAiChat, "local", base)
        .default_model("test-model")
        .header("X-Route", "first")
        .header("x-route", "second");

    let sent = send_with(config, requests).await;
    assert_eq!(header_values(&sent, "x-route"), ["second"]);
}

#[tokio::test]
async fn a_caller_header_survives_when_no_key_claims_the_name() {
    let (base, requests) = serve(&[ok_response()]);
    let config = EndpointConfig::new(Dialect::OpenAiChat, "local", base)
        .default_model("test-model")
        .auth(Auth::None)
        .header("Authorization", "Bearer gateway-token");

    let sent = send_with(config, requests).await;
    assert_eq!(
        header_values(&sent, "authorization"),
        ["Bearer gateway-token"]
    );
}
