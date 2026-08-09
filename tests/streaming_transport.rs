//! End-to-end transport tests for `Client::stream`.
//!
//! The rest of the suite drives decoders over recorded bytes; nothing else
//! exercises the HTTP path. These use a real socket so that request building,
//! headers, auth, the status check, and the byte pump are all covered.

use freyja::{
    Client, GenerateRequest, Message, ProviderConfig, ProviderDialect, Role, StreamEvent,
};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::mpsc;

/// Serves one request and returns what the client sent.
fn serve_once(response: &'static str) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    let (tx, rx) = mpsc::channel();

    std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("accept");
        let mut reader = BufReader::new(socket.try_clone().expect("clone"));

        // Read the head, then the body if the client announced a length.
        let mut head = String::new();
        let mut length = 0usize;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).expect("read") == 0 || line == "\r\n" {
                break;
            }
            if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = value.trim().parse().unwrap_or(0);
            }
            head.push_str(&line);
        }
        let mut body = vec![0u8; length];
        if length > 0 {
            std::io::Read::read_exact(&mut reader, &mut body).expect("body");
        }
        head.push_str(&String::from_utf8_lossy(&body));

        socket.write_all(response.as_bytes()).expect("write");
        socket.flush().expect("flush");
        let _ = tx.send(head);
    });

    (base, rx)
}

#[tokio::test]
async fn streams_text_from_a_live_server() {
    let (base, request) = serve_once(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/event-stream\r\n\
         Connection: close\r\n\r\n\
         data: {\"id\":\"chatcmpl-1\",\"model\":\"gpt-4o\",\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n\
         data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}},{\"finish_reason\":\"stop\"}]}\n\n\
         data: [DONE]\n\n",
    );

    let config =
        ProviderConfig::new(ProviderDialect::OpenAiChat, "local", base).default_model("test-model");
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
    let (base, _request) = serve_once(
        "HTTP/1.1 429 Too Many Requests\r\n\
         Content-Type: application/json\r\n\
         Connection: close\r\n\r\n\
         {\"error\":{\"message\":\"slow down\"}}",
    );

    let config =
        ProviderConfig::new(ProviderDialect::OpenAiChat, "local", base).default_model("test-model");
    let client = Client::new(config, "sk-test");

    // `EventStream` is not `Debug`, so `expect_err` is unavailable here.
    let error = match client
        .stream(&GenerateRequest::new().message(Message::text(Role::User, "Hi")))
        .await
    {
        Ok(_) => panic!("a 429 must not produce a stream"),
        Err(error) => error,
    };

    match error {
        freyja::ProviderError::Api {
            status,
            body,
            provider,
        } => {
            assert_eq!(status, 429);
            assert_eq!(&*provider, "local");
            assert!(body.contains("slow down"), "{body}");
        }
        other => panic!("expected ProviderError::Api, got {other:?}"),
    }
}

#[tokio::test]
async fn gemini_requests_sse_by_query_parameter() {
    let (base, request) = serve_once(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/event-stream\r\n\
         Connection: close\r\n\r\n\
         data: {\"interaction\":{\"id\":\"v1_1\",\"model\":\"gemini-test\",\"status\":\"completed\"},\"event_type\":\"interaction.completed\"}\n\n",
    );

    let config =
        ProviderConfig::new(ProviderDialect::Gemini, "local", base).default_model("test-model");
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
