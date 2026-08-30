use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::mpsc;

/// Serves `responses` in order and returns the base URL plus every request
/// body the client sent.
///
/// One entry point for the socket rather than a single-response and a
/// multi-response pair. Each test binary compiles this module whole, so a
/// second variant only some binaries call is dead code in the rest.
pub fn serve(responses: &[&'static str]) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    let (tx, rx) = mpsc::channel();
    let responses: Vec<&'static str> = responses.to_vec();

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

/// A canned non-streaming OpenAI Chat Completions response with a correct
/// `Content-Length`, leaked so it satisfies `serve`'s `&'static str`.
///
/// Five test binaries had byte-identical copies of this. Two of the binaries
/// that compile this module have no use for it, so it is allowed to be dead
/// there. The exemption is written on this item alone rather than on the
/// module, so a helper that stops being used everywhere is still reported.
#[allow(dead_code)]
pub fn ok_response() -> &'static str {
    let body = r#"{"id":"x","model":"test-model","choices":[{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}]}"#;
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    Box::leak(head.into_boxed_str())
}
