use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::mpsc;

/// Serves one request and returns what the client sent.
pub fn serve_once(response: &'static str) -> (String, mpsc::Receiver<String>) {
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
