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

#[tokio::test]
async fn the_body_goes_out_as_json_with_one_content_type() {
    let (base, requests) = serve(&[ok_response()]);
    let config =
        EndpointConfig::new(Dialect::OpenAiChat, "local", base).default_model("test-model");

    let sent = send_with(config, requests).await;
    assert_eq!(header_values(&sent, "content-type"), ["application/json"]);
    let body = &sent[sent.find("{\"").expect("a JSON body")..];
    let body: serde_json::Value = serde_json::from_str(body).expect("the body is JSON");
    assert_eq!(body["messages"][0]["content"], "Hi");
}

#[tokio::test]
async fn an_endpoint_content_type_replaces_the_default() {
    let (base, requests) = serve(&[ok_response()]);
    let config = EndpointConfig::new(Dialect::OpenAiChat, "local", base)
        .default_model("test-model")
        .header("Content-Type", "application/json; charset=utf-8");

    let sent = send_with(config, requests).await;
    assert_eq!(
        header_values(&sent, "content-type"),
        ["application/json; charset=utf-8"]
    );
}

#[tokio::test]
async fn an_error_body_is_kept_only_up_to_its_cap() {
    // Far past the cap, so a body kept whole would fail the length check.
    let body = "x".repeat(1024 * 1024);
    let response = format!(
        "HTTP/1.1 500 Internal Server Error\r\nContent-Type: text/plain\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let (base, _requests) = serve(&[Box::leak(response.into_boxed_str())]);
    let config =
        EndpointConfig::new(Dialect::OpenAiChat, "local", base).default_model("test-model");

    let error = Client::new(config, "sk-test")
        .generate(&GenerateRequest::new().message(Message::text(Role::User, "Hi")))
        .await
        .expect_err("a 500 is an error");
    assert_eq!(error.status(), Some(500));
    assert_eq!(error.body().map(str::len), Some(64 * 1024));
}
