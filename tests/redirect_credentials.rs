//! A redirect must not carry the credential to another origin.
//!
//! `reqwest` strips `Authorization` when a redirect crosses an origin and
//! cannot strip what it cannot recognize, so the `x-api-key` and
//! `x-goog-api-key` that `Auth::Header` uses were forwarded. Measured before
//! the fix, against exactly the servers below, the Anthropic key arrived at the
//! second host and the call returned `Ok` to the caller.
//!
//! Written against raw sockets rather than a test-server crate: the whole
//! property is about what one host receives after another host redirects, and
//! that is three lines of HTTP.

use freyja::{Client, Dialect, EndpointConfig, GenerateRequest, Message, Role};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{Receiver, Sender, channel};

const BODY: &str = r#"{"id":"m","type":"message","role":"assistant","model":"m",
    "content":[{"type":"text","text":"hi"}],"stop_reason":"end_turn",
    "usage":{"input_tokens":1,"output_tokens":1}}"#;

/// Reads one request and returns its headers, lowercased.
fn read_request(stream: &mut TcpStream) -> Vec<(String, String)> {
    let mut reader = BufReader::new(stream.try_clone().expect("the stream clones"));
    let mut headers = Vec::new();
    let mut length = 0usize;

    let mut request_line = String::new();
    reader.read_line(&mut request_line).expect("a request line");
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).expect("a header line") == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let (name, value) = (name.trim().to_ascii_lowercase(), value.trim().to_string());
            if name == "content-length" {
                length = value.parse().unwrap_or(0);
            }
            headers.push((name, value));
        }
    }
    // Drained, so the peer is never written to while it is still sending.
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).expect("the declared body");
    headers
}

fn respond(stream: &mut TcpStream, head: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {head}\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).expect("a response");
    stream.flush().expect("a flush");
}

/// A server that answers the first request with `307` and every later one with
/// a usable body. Each request's headers go to the returned channel.
///
/// `location` is handed the server's own address, so a test can redirect a
/// server to itself and get the same-origin case.
fn serve(
    location: impl FnOnce(&str) -> Option<String>,
) -> (String, Receiver<Vec<(String, String)>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free port");
    let address = format!("http://{}", listener.local_addr().expect("an address"));
    let target = location(&address);
    let (seen, received): (Sender<_>, Receiver<_>) = channel();

    std::thread::spawn(move || {
        let mut redirected = false;
        for stream in listener.incoming() {
            let mut stream = stream.expect("a connection");
            seen.send(read_request(&mut stream)).ok();
            match &target {
                Some(target) if !redirected => {
                    redirected = true;
                    respond(
                        &mut stream,
                        &format!("307 Temporary Redirect\r\nLocation: {target}"),
                        "",
                    );
                }
                _ => respond(&mut stream, "200 OK", BODY),
            }
        }
    });

    (address, received)
}

fn anthropic_client(base: &str, key: &str) -> Client {
    Client::new(
        EndpointConfig::new(Dialect::Anthropic, "gw", format!("{base}/v1")).default_model("m"),
        key,
    )
}

fn request() -> GenerateRequest {
    GenerateRequest::new().message(Message::text(Role::User, "hi"))
}

fn carries_key(headers: &[(String, String)], key: &str) -> bool {
    headers
        .iter()
        .any(|(name, value)| name == "x-api-key" && value == key)
}

#[tokio::test]
async fn a_cross_origin_redirect_never_reaches_the_second_host() {
    let (target, target_seen) = serve(|_| None);
    let (first, first_seen) = serve(|_| Some(format!("{target}/v1/messages")));

    let error = anthropic_client(&first, "sk-ant-LEAKED-KEY")
        .generate(&request())
        .await
        .expect_err("the redirect is refused, so the 307 surfaces");

    // The first host saw the key, which is correct: the caller named it.
    let sent = first_seen.recv().expect("the first host was reached");
    assert!(carries_key(&sent, "sk-ant-LEAKED-KEY"), "{sent:?}");

    // The host it redirected to saw nothing at all.
    assert!(
        target_seen.try_recv().is_err(),
        "the credential reached a host the caller never named"
    );

    // And the caller is told, rather than being handed an `Ok` built by
    // whoever answered the redirect.
    assert_eq!(error.status(), Some(307), "{error}");
}

#[tokio::test]
async fn a_same_origin_redirect_is_still_followed() {
    // The half that stops the fix from being "refuse every redirect". A
    // gateway moving a path is ordinary and must keep working.
    let (address, seen) = serve(|address| Some(format!("{address}/v1/moved")));

    let response = anthropic_client(&address, "sk-ant-KEY")
        .generate(&request())
        .await
        .expect("a same-origin redirect is followed");

    assert_eq!(response.output_text(), "hi");
    // Twice, so the redirect really was followed rather than answered first
    // time, and the credential travelled on both hops because neither left the
    // origin the caller named.
    for hop in 0..2 {
        let sent = seen.recv().expect("both hops reached the server");
        assert!(carries_key(&sent, "sk-ant-KEY"), "hop {hop}: {sent:?}");
    }
}
