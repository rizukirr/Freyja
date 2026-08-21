//! End-to-end tests for `Agent`, driven against a scripted local endpoint.
//!
//! `tests/streaming_transport.rs` serves a single request. The agent loop makes
//! several, so this serves a scripted sequence and hands back every request body
//! it captured. Most of what `Agent` promises is about what it sends on the next
//! turn, which only the captured bodies can show.

use freyja::{Client, Dialect, EndpointConfig, GenerateRequest, Message, Role};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::mpsc;

/// Serves `responses` in order and returns the base URL plus every request body.
fn serve_many(responses: Vec<&'static str>) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    let (tx, rx) = mpsc::channel();

    std::thread::spawn(move || {
        for response in responses {
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
        }
    });

    (base, rx)
}

/// Wraps a JSON body in the minimal HTTP response the helper needs.
fn ok(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

/// Leaks a response body into a `&'static str` for the serving thread.
fn canned(body: &str) -> &'static str {
    ok(body).leak()
}

fn client(base: String) -> Client {
    Client::new(
        EndpointConfig::new(Dialect::OpenAiChat, "local", base).default_model("test-model"),
        "sk-test",
    )
}

#[tokio::test]
async fn the_scripted_endpoint_serves_a_sequence() {
    let body = r#"{"id":"chatcmpl-1","model":"test-model","choices":[{"index":0,"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}]}"#;
    let (base, requests) = serve_many(vec![canned(body), canned(body)]);
    let client = client(base);

    let request = GenerateRequest::new().message(Message::text(Role::User, "Hi"));
    assert_eq!(client.generate(&request).await.unwrap().output_text(), "hello");
    assert_eq!(client.generate(&request).await.unwrap().output_text(), "hello");

    assert!(requests.recv().unwrap().contains("Hi"));
    assert!(requests.recv().unwrap().contains("Hi"));
}
