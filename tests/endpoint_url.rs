//! Wire-level checks that the URL a request is sent to is well formed.
//!
//! `src/client.rs` asserts what `url()` builds; these assert what arrives, so
//! a path or query mangled between the builder and the socket is caught.

use freyja::{Client, Dialect, EndpointConfig, GenerateRequest, Message, Role};

mod common;
use common::serve;

fn ok_response() -> &'static str {
    let body = r#"{"id":"x","model":"test-model","choices":[{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}]}"#;
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    Box::leak(head.into_boxed_str())
}

fn ask() -> GenerateRequest {
    GenerateRequest::new().message(Message::text(Role::User, "Hi"))
}

#[tokio::test]
async fn an_explicit_path_and_query_reach_the_server() {
    let (base, requests) = serve(&[ok_response()]);
    let config = EndpointConfig::new(Dialect::OpenAiChat, "local", base)
        .default_model("test-model")
        .path("/openai/deployments/gpt4/chat/completions")
        .query("api-version", "2024-02-01");

    Client::new(config, "sk-test")
        .generate(&ask())
        .await
        .expect("request succeeds");

    let sent = requests.recv().expect("captured request");
    assert!(
        sent.starts_with("POST /openai/deployments/gpt4/chat/completions?api-version=2024-02-01 "),
        "{sent}"
    );
}

#[tokio::test]
async fn a_query_value_arrives_percent_encoded() {
    let (base, requests) = serve(&[ok_response()]);
    let config = EndpointConfig::new(Dialect::OpenAiChat, "local", base)
        .default_model("test-model")
        .query("filter", "a b");

    Client::new(config, "sk-test")
        .generate(&ask())
        .await
        .expect("request succeeds");

    let sent = requests.recv().expect("captured request");
    let line = sent.lines().next().expect("request line");
    assert!(
        line.contains("filter=a+b") || line.contains("filter=a%20b"),
        "{line}"
    );
    assert!(!line.contains("filter=a b"), "{line}");
}

#[tokio::test]
async fn an_unparseable_base_url_fails_at_send() {
    // The fallback concatenation hands `reqwest` a string it will reject, so
    // the failure stays a transport error naming the endpoint.
    let config =
        EndpointConfig::new(Dialect::OpenAiChat, "local", "not a url").default_model("test-model");

    let error = Client::new(config, "sk-test")
        .generate(&ask())
        .await
        .expect_err("an unparseable base URL cannot reach anything");

    assert!(matches!(error, freyja::Error::Http { .. }), "{error:?}");
}

#[test]
fn a_credential_shaped_query_parameter_is_withheld_from_debug() {
    let config = EndpointConfig::new(Dialect::Gemini, "g", "https://x.test/v1")
        .query("key", "super-secret")
        .query("api-version", "2024-02-01");

    let printed = format!("{config:?}");
    assert!(!printed.contains("super-secret"), "{printed}");
    assert!(printed.contains("2024-02-01"), "{printed}");
}
