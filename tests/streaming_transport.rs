//! End-to-end transport tests for `Client::stream`.
//!
//! The rest of the suite drives decoders over recorded bytes; nothing else
//! exercises the HTTP path. These use a real socket so that request building,
//! headers, auth, the status check, and the byte pump are all covered.

use freyja::{Client, Dialect, EndpointConfig, GenerateRequest, Message, Role, StreamEvent};

mod common;
use common::serve;

#[tokio::test]
async fn streams_text_from_a_live_server() {
    let (base, request) = serve(&["HTTP/1.1 200 OK\r\n\
         Content-Type: text/event-stream\r\n\
         Connection: close\r\n\r\n\
         data: {\"id\":\"chatcmpl-1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n\
         data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}},{\"finish_reason\":\"stop\"}]}\n\n\
         data: [DONE]\n\n"]);

    let config =
        EndpointConfig::new(Dialect::OpenAiChat, "local", base).default_model("test-model");
    let client = Client::new(config, "sk-test");

    let mut stream = client
        .stream(&GenerateRequest::new().message(Message::text(Role::User, "Hi")))
        .await
        .expect("stream opens");

    let mut text = String::new();
    while let Some(event) = stream.next().await.expect("no stream error") {
        if let StreamEvent::TextDelta(delta) = event {
            text.push_str(&delta);
        }
    }
    assert_eq!(text, "Hello");

    let sent = request.recv().expect("captured request");
    assert!(sent.starts_with("POST /chat/completions "), "{sent}");
    assert!(
        sent.contains("authorization: Bearer sk-test")
            || sent.contains("Authorization: Bearer sk-test"),
        "{sent}"
    );
    assert!(
        sent.contains("\"stream\":true"),
        "the body must ask for a stream: {sent}"
    );
    assert!(
        sent.contains("\"include_usage\":true"),
        "usage must be requested: {sent}"
    );

    let response = stream.into_response().expect("drained");
    assert_eq!(response.output_text(), "Hello");
}

#[tokio::test]
async fn surfaces_an_error_status_before_streaming() {
    let (base, _request) = serve(&["HTTP/1.1 429 Too Many Requests\r\n\
         Content-Type: application/json\r\n\
         Retry-After: 30\r\n\
         Connection: close\r\n\r\n\
         {\"error\":{\"message\":\"slow down\"}}"]);

    let config =
        EndpointConfig::new(Dialect::OpenAiChat, "local", base).default_model("test-model");
    let client = Client::new(config, "sk-test");

    // `EventStream` is not `Debug`, so `expect_err` is unavailable here.
    let error = match client
        .stream(&GenerateRequest::new().message(Message::text(Role::User, "Hi")))
        .await
    {
        Ok(_) => panic!("a 429 must not produce a stream"),
        Err(error) => error,
    };

    match &error {
        freyja::Error::RateLimit {
            endpoint,
            retry_after,
            body,
        } => {
            assert_eq!(&**endpoint, "local");
            // The header is read before `text()` consumes the response, so the
            // endpoint's own pacing survives into the error.
            assert_eq!(*retry_after, Some(std::time::Duration::from_secs(30)));
            assert!(body.contains("slow down"), "{body}");
        }
        other => panic!("expected Error::RateLimit, got {other:?}"),
    }

    assert_eq!(error.status(), Some(429));
    assert!(error.is_retryable());
    assert_eq!(
        error.retry_after(),
        Some(std::time::Duration::from_secs(30))
    );
}

#[tokio::test]
async fn gemini_requests_sse_by_query_parameter() {
    let (base, request) = serve(&["HTTP/1.1 200 OK\r\n\
         Content-Type: text/event-stream\r\n\
         Connection: close\r\n\r\n\
         data: {\"interaction\":{\"id\":\"v1_1\",\"model\":\"gemini-test\",\"status\":\"completed\"},\"event_type\":\"interaction.completed\"}\n\n"]);

    let config = EndpointConfig::new(Dialect::Gemini, "local", base).default_model("test-model");
    let client = Client::new(config, "key");

    let mut stream = client
        .stream(&GenerateRequest::new().message(Message::text(Role::User, "Hi")))
        .await
        .expect("stream opens");
    while stream.next().await.expect("no error").is_some() {}

    let sent = request.recv().expect("captured request");
    assert!(
        sent.starts_with("POST /interactions?alt=sse "),
        "Gemini needs ?alt=sse on the URL, not only stream:true in the body: {sent}"
    );
    assert!(
        sent.contains("Api-Revision: 2026-05-20") || sent.contains("api-revision: 2026-05-20"),
        "{sent}"
    );
}

/// `generate()` must NOT ask for SSE.
///
/// This exists because it very nearly shipped broken: a transient edit made
/// during another test's setup swapped `Client::run`'s URL for the streaming
/// one, so every non-streaming Gemini call would have requested SSE and then
/// tried to parse the event stream as JSON. The whole suite passed anyway,
/// because until now nothing exercised `generate()` over a socket either.
#[tokio::test]
async fn generate_does_not_request_sse() {
    let (base, request) = serve(&["HTTP/1.1 200 OK\r\n\
         Content-Type: application/json\r\n\
         Connection: close\r\n\r\n\
         {\"id\":\"int_1\",\"model\":\"gemini-test\",\"status\":\"completed\",\"steps\":[]}"]);

    let config = EndpointConfig::new(Dialect::Gemini, "local", base).default_model("test-model");
    let client = Client::new(config, "key");

    let _ = client
        .generate(&GenerateRequest::new().message(Message::text(Role::User, "Hi")))
        .await;

    let sent = request.recv().expect("captured request");
    assert!(
        sent.starts_with("POST /interactions HTTP/1.1"),
        "the non-streaming path must post to the plain URL, with no ?alt=sse: {sent}"
    );
    assert!(
        !sent.contains("alt=sse"),
        "generate() must never request an event stream: {sent}"
    );
}
