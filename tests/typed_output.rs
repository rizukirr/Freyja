//! End-to-end tests for `Client::generate_as`.
//!
//! Over a real socket rather than a recorded body, because the point of the
//! method is what it does with a whole response: the text, and the status that
//! says whether the text is finished.

use freyja::{Client, Dialect, EndpointConfig, Error, GenerateRequest, Message, Role};
use serde::Deserialize;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

#[derive(Debug, Deserialize)]
struct Recommendation {
    name: String,
    purpose: String,
}

/// Serves one Chat Completions response carrying `content` as the answer.
fn serve_once(content: &str, finish_reason: &str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));

    let body = serde_json::json!({
        "id": "chatcmpl-1",
        "model": "test-model",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": content},
            "finish_reason": finish_reason,
        }],
    })
    .to_string();

    std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("accept");
        let mut reader = BufReader::new(socket.try_clone().expect("clone"));

        let mut length = 0usize;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).expect("read") == 0 || line == "\r\n" {
                break;
            }
            if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = value.trim().parse().unwrap_or(0);
            }
        }
        if length > 0 {
            std::io::Read::read_exact(&mut reader, &mut vec![0u8; length]).expect("body");
        }

        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        socket.write_all(response.as_bytes()).expect("write");
        socket.flush().expect("flush");
    });

    base
}

fn client(base: String) -> Client {
    Client::new(
        EndpointConfig::new(Dialect::OpenAiChat, "local", base).default_model("test-model"),
        "sk-test",
    )
}

fn ask() -> GenerateRequest {
    GenerateRequest::new().message(Message::text(Role::User, "Recommend a crate."))
}

#[tokio::test]
async fn deserializes_a_well_shaped_answer() {
    let base = serve_once(r#"{"name":"serde_json","purpose":"Parsing JSON"}"#, "stop");

    let recommendation: Recommendation = client(base)
        .generate_as(&ask())
        .await
        .expect("the answer matches the type");

    assert_eq!(recommendation.name, "serde_json");
    assert_eq!(recommendation.purpose, "Parsing JSON");
}

#[tokio::test]
async fn a_wrong_shape_keeps_the_answer_it_could_not_use() {
    // The model obeyed the format and not the schema. The text is the only
    // record of what it actually said, so the error has to carry it.
    let base = serve_once(r#"{"crate":"serde_json"}"#, "stop");

    let error = client(base)
        .generate_as::<Recommendation>(&ask())
        .await
        .expect_err("the shape is wrong");

    match &error {
        Error::OutputMismatch {
            endpoint,
            text,
            truncated,
            ..
        } => {
            assert_eq!(&**endpoint, "local");
            assert_eq!(text, r#"{"crate":"serde_json"}"#);
            assert!(
                !truncated,
                "the answer was complete, just not the right shape"
            );
        }
        other => panic!("expected OutputMismatch, got {other:?}"),
    }

    // Not a transport problem and not the vendor's fault, so nothing here
    // suggests trying again: the schema or the prompt has to change.
    assert!(!error.is_retryable());
    assert_eq!(error.status(), None);
}

#[tokio::test]
async fn a_truncated_answer_says_so_instead_of_blaming_the_schema() {
    // Cut off by the token cap. Valid text, invalid JSON, and the fix is a
    // bigger cap rather than a different schema -- which a bare serde error
    // ("EOF while parsing an object") would send you looking for in the wrong
    // place.
    let base = serve_once(r#"{"name":"serde_json","purp"#, "length");

    let error = client(base)
        .generate_as::<Recommendation>(&ask())
        .await
        .expect_err("half a JSON object is not JSON");

    match &error {
        Error::OutputMismatch { truncated, .. } => {
            assert!(truncated, "finish_reason 'length' means it was cut short");
        }
        other => panic!("expected OutputMismatch, got {other:?}"),
    }

    assert!(
        error.to_string().contains("cut short"),
        "the message must name the cause: {error}"
    );
}

#[tokio::test]
async fn transport_and_api_failures_pass_straight_through() {
    // `generate_as` adds one failure mode and shadows none. An unreachable
    // endpoint is still a transport error, not a deserialization one.
    let client = client("http://127.0.0.1:1/v1".into());

    let error = client
        .generate_as::<Recommendation>(&ask())
        .await
        .expect_err("nothing is listening");

    assert!(matches!(error, Error::Http { .. }), "got {error:?}");
}
