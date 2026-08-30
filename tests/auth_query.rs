//! Wire-level checks for `Auth::Query`, the credential that travels in the URL.
//!
//! `src/client.rs` asserts that `url()` omits it. These assert that the server
//! receives it, which is the other half of the same claim.

use freyja::{Auth, Client, Dialect, EndpointConfig, GenerateRequest, Message, Role};

mod common;
use common::{ok_response, serve};

fn ask() -> GenerateRequest {
    GenerateRequest::new().message(Message::text(Role::User, "Hi"))
}

fn request_line(head: &str) -> String {
    head.lines().next().expect("a request line").to_string()
}

#[tokio::test]
async fn the_key_travels_as_the_named_query_parameter() {
    let (base, requests) = serve(&[ok_response()]);
    let config = EndpointConfig::new(Dialect::OpenAiChat, "local", base)
        .default_model("test-model")
        .auth(Auth::Query("key"));

    Client::new(config, "sk-test")
        .generate(&ask())
        .await
        .expect("request succeeds");

    let line = request_line(&requests.recv().expect("captured request"));
    assert!(line.contains("key=sk-test"), "{line}");
    assert_eq!(line.matches('?').count(), 1, "{line}");
}

#[tokio::test]
async fn auth_replaces_a_caller_parameter_of_the_same_name() {
    let (base, requests) = serve(&[ok_response()]);
    let config = EndpointConfig::new(Dialect::OpenAiChat, "local", base)
        .default_model("test-model")
        .auth(Auth::Query("key"))
        .query("key", "not-the-credential")
        .query("api-version", "2024-02-01");

    Client::new(config, "sk-test")
        .generate(&ask())
        .await
        .expect("request succeeds");

    let line = request_line(&requests.recv().expect("captured request"));
    assert!(!line.contains("not-the-credential"), "{line}");
    assert_eq!(line.matches("key=").count(), 1, "{line}");
    // A parameter that is not the credential is left alone.
    assert!(line.contains("api-version=2024-02-01"), "{line}");
}

#[tokio::test]
async fn a_streaming_url_carries_the_credential_and_alt_sse() {
    let (base, requests) = serve(&["HTTP/1.1 200 OK\r\n\
         Content-Type: text/event-stream\r\n\
         Connection: close\r\n\r\n\
         data: [DONE]\n\n"]);
    let config = EndpointConfig::new(Dialect::Gemini, "local", base)
        .default_model("test-model")
        .auth(Auth::Query("key"));

    let _stream = Client::new(config, "sk-test")
        .stream(&ask())
        .await
        .expect("the stream opens");

    let line = request_line(&requests.recv().expect("captured request"));
    assert!(line.contains("alt=sse"), "{line}");
    assert!(line.contains("key=sk-test"), "{line}");
    assert_eq!(line.matches('?').count(), 1, "{line}");
}

#[tokio::test]
async fn an_endpoint_needing_no_key_sends_no_parameter() {
    let (base, requests) = serve(&[ok_response()]);
    let config = EndpointConfig::new(Dialect::OpenAiChat, "local", base)
        .default_model("test-model")
        .auth(Auth::None);

    Client::new(config, "")
        .generate(&ask())
        .await
        .expect("request succeeds");

    let line = request_line(&requests.recv().expect("captured request"));
    assert!(!line.contains('?'), "{line}");
}

#[tokio::test]
async fn a_query_credential_is_withheld_from_a_transport_error() {
    // Named so the heuristic added in #40 would miss it: the point is that the
    // exact name is known, not guessed. Port 1 refuses, so the failure carries
    // the URL it tried.
    let config = EndpointConfig::new(Dialect::OpenAiChat, "local", "http://127.0.0.1:1")
        .default_model("test-model")
        .auth(Auth::Query("passport"))
        .query("api-version", "2024-02-01");

    let error = Client::new(config, "sk-test")
        .generate(&ask())
        .await
        .expect_err("nothing listens on port 1");

    for rendered in [error.to_string(), format!("{error:?}")] {
        assert!(!rendered.contains("sk-test"), "{rendered}");
        assert!(rendered.contains("api-version=2024-02-01"), "{rendered}");
    }
}
