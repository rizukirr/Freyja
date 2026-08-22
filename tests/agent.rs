//! End-to-end tests for `Agent`, driven against a scripted local endpoint.
//!
//! `tests/streaming_transport.rs` serves a single request. The agent loop makes
//! several, so this serves a scripted sequence and hands back every request body
//! it captured. Most of what `Agent` promises is about what it sends on the next
//! turn, which only the captured bodies can show.

use freyja::{
    Agent, Client, Dialect, EndpointConfig, GenerateRequest, Message, Role, StopReason, ToolChoice,
    tool,
};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::mpsc;

#[tool(description = "adds two numbers together", strict = true)]
fn add(a: i64, b: i64) -> i64 {
    a + b
}

#[tool(description = "waits, then echoes a word back")]
async fn echo(word: String) -> String {
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    word
}

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

/// The OpenAiChat dialect never emits `OutputContent::Reasoning`, so anything
/// testing opaque state speaks Anthropic instead.
fn anthropic_client(base: String) -> Client {
    Client::new(
        EndpointConfig::new(Dialect::Anthropic, "local", base).default_model("test-model"),
        "sk-test",
    )
}

#[tokio::test]
async fn the_scripted_endpoint_serves_a_sequence() {
    let body = r#"{"id":"chatcmpl-1","model":"test-model","choices":[{"index":0,"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}]}"#;
    let (base, requests) = serve_many(vec![canned(body), canned(body)]);
    let client = client(base);

    let request = GenerateRequest::new().message(Message::text(Role::User, "Hi"));
    assert_eq!(
        client.generate(&request).await.unwrap().output_text(),
        "hello"
    );
    assert_eq!(
        client.generate(&request).await.unwrap().output_text(),
        "hello"
    );

    assert!(requests.recv().unwrap().contains("Hi"));
    assert!(requests.recv().unwrap().contains("Hi"));
}

const ANSWER: &str = r#"{"id":"chatcmpl-1","model":"test-model","choices":[{"index":0,"message":{"role":"assistant","content":"the answer is 42"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}"#;

const CALLS_ADD: &str = r#"{"id":"chatcmpl-1","model":"test-model","choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"add","arguments":"{\"a\":20,\"b\":22}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":5,"completion_tokens":5,"total_tokens":10}}"#;

#[tokio::test]
async fn answers_without_tool_calls() {
    let (base, _requests) = serve_many(vec![canned(ANSWER)]);
    let agent = Agent::new(client(base));

    let mut messages = vec![Message::text(Role::User, "Hi")];
    let run = agent.run(&mut messages).await.unwrap();

    assert_eq!(run.stop, StopReason::Answered);
    assert_eq!(run.answer, "the answer is 42");
    assert_eq!(run.turns, 1);
    assert_eq!(messages.len(), 2);
}

#[tokio::test]
async fn completes_a_full_tool_round_trip() {
    let (base, requests) = serve_many(vec![canned(CALLS_ADD), canned(ANSWER)]);
    let agent = Agent::new(client(base)).tool(add);

    let mut messages = vec![Message::text(Role::User, "What is 20 + 22?")];
    let run = agent.run(&mut messages).await.unwrap();

    assert_eq!(run.stop, StopReason::Answered);
    assert_eq!(run.turns, 2);
    assert_eq!(messages.len(), 4);

    let _first = requests.recv().unwrap();
    let second = requests.recv().unwrap();
    assert!(second.contains("call_1"));
    assert!(second.contains("42"));
}

#[tokio::test]
async fn stops_at_the_turn_bound() {
    let (base, _requests) = serve_many(vec![canned(CALLS_ADD), canned(CALLS_ADD)]);
    let agent = Agent::new(client(base)).tool(add).max_turns(2);

    let mut messages = vec![Message::text(Role::User, "loop")];
    let run = agent.run(&mut messages).await.unwrap();

    assert_eq!(run.stop, StopReason::MaxTurns);
    assert_eq!(run.turns, 2);
    assert!(run.answer.is_empty());
}

#[tokio::test]
async fn sums_usage_across_turns() {
    let (base, _requests) = serve_many(vec![canned(CALLS_ADD), canned(ANSWER)]);
    let agent = Agent::new(client(base)).tool(add);

    let mut messages = vec![Message::text(Role::User, "What is 20 + 22?")];
    let run = agent.run(&mut messages).await.unwrap();

    assert_eq!(run.usage.total_tokens, 13);
}

const UNKNOWN: &str = r#"{"id":"chatcmpl-1","model":"test-model","choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_9","type":"function","function":{"name":"nope","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#;

const BAD_ARGS: &str = r#"{"id":"chatcmpl-1","model":"test-model","choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_2","type":"function","function":{"name":"add","arguments":"{\"a\":\"twenty\",\"b\":22}"}}]},"finish_reason":"tool_calls"}]}"#;

#[tokio::test]
async fn answers_an_unknown_tool_rather_than_skipping_it() {
    let (base, requests) = serve_many(vec![canned(UNKNOWN), canned(ANSWER)]);
    let agent = Agent::new(client(base)).tool(add);

    let mut messages = vec![Message::text(Role::User, "go")];
    agent.run(&mut messages).await.unwrap();

    let _first = requests.recv().unwrap();
    let second = requests.recv().unwrap();
    assert!(second.contains("call_9"));
    assert!(second.contains("unknown tool"));
}

#[tokio::test]
async fn feeds_a_tool_error_back_to_the_model() {
    let (base, requests) = serve_many(vec![canned(BAD_ARGS), canned(ANSWER)]);
    let agent = Agent::new(client(base)).tool(add);

    let mut messages = vec![Message::text(Role::User, "go")];
    let run = agent.run(&mut messages).await.unwrap();

    assert_eq!(run.stop, StopReason::Answered);
    let _first = requests.recv().unwrap();
    let second = requests.recv().unwrap();
    assert!(second.contains("call_2"));
    assert!(second.contains("Arguments"));
}

const CALLS_ECHO_THRICE: &str = r#"{"id":"chatcmpl-1","model":"test-model","choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_a","type":"function","function":{"name":"echo","arguments":"{\"word\":\"ha\"}"}},{"id":"call_b","type":"function","function":{"name":"echo","arguments":"{\"word\":\"ho\"}"}},{"id":"call_c","type":"function","function":{"name":"echo","arguments":"{\"word\":\"hi\"}"}}]},"finish_reason":"tool_calls"}]}"#;

#[tokio::test]
async fn dispatches_parallel_calls_concurrently() {
    let (base, requests) = serve_many(vec![canned(CALLS_ECHO_THRICE), canned(ANSWER)]);
    let agent = Agent::new(client(base)).tool(echo);

    let mut messages = vec![Message::text(Role::User, "go")];
    let started = std::time::Instant::now();
    agent.run(&mut messages).await.unwrap();

    assert!(started.elapsed() < std::time::Duration::from_millis(350));
    assert_eq!(messages.len(), 6);

    // Each call is answered exactly once. Match the result's correlation field
    // rather than the bare id: the transcript also carries the assistant's own
    // `tool_calls[].id`, so the bare id appears twice in one body by design.
    // See `src/dialect/openai_chat/request.rs:424` and `:435`.
    let _first = requests.recv().unwrap();
    let second = requests.recv().unwrap();
    assert_eq!(second.matches(r#""tool_call_id":"call_a""#).count(), 1);
    assert_eq!(second.matches(r#""tool_call_id":"call_b""#).count(), 1);
    assert_eq!(second.matches(r#""tool_call_id":"call_c""#).count(), 1);
}

const REFUSAL: &str = r#"{"id":"chatcmpl-1","model":"test-model","choices":[{"message":{"role":"assistant","refusal":"I cannot help with that"},"finish_reason":"stop"}]}"#;

const CUT_SHORT: &str = r#"{"id":"chatcmpl-1","model":"test-model","choices":[{"message":{"role":"assistant","content":"partial"},"finish_reason":"length"}]}"#;

#[tokio::test]
async fn stops_on_a_refusal() {
    let (base, _requests) = serve_many(vec![canned(REFUSAL)]);
    let agent = Agent::new(client(base));

    let mut messages = vec![Message::text(Role::User, "go")];
    let run = agent.run(&mut messages).await.unwrap();

    assert_eq!(run.stop, StopReason::Refused);
}

#[tokio::test]
async fn stops_when_the_generation_was_cut_short() {
    let (base, _requests) = serve_many(vec![canned(CUT_SHORT)]);
    let agent = Agent::new(client(base));

    let mut messages = vec![Message::text(Role::User, "go")];
    let run = agent.run(&mut messages).await.unwrap();

    assert_eq!(run.stop, StopReason::Incomplete);
}

#[tokio::test]
async fn downgrades_required_after_the_first_turn() {
    let (base, requests) = serve_many(vec![canned(CALLS_ADD), canned(ANSWER)]);
    let agent = Agent::new(client(base))
        .tool(add)
        .request(GenerateRequest::new().tool_choice(ToolChoice::Required));

    let mut messages = vec![Message::text(Role::User, "go")];
    agent.run(&mut messages).await.unwrap();

    let first = requests.recv().unwrap();
    let second = requests.recv().unwrap();
    assert!(first.contains("required"));
    assert!(second.contains("auto"));
}

#[tokio::test]
async fn a_failed_call_leaves_the_transcript_untouched() {
    let (base, _requests) = serve_many(vec![
        "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    ]);
    let agent = Agent::new(client(base));

    let mut messages = vec![Message::text(Role::User, "go")];
    let before = messages.clone();

    assert!(agent.run(&mut messages).await.is_err());
    assert_eq!(messages, before);
}

/// A `thinking` block Freyja does not model, beside a tool call.
const THINKS_THEN_CALLS: &str = r#"{"id":"msg_1","model":"test-model","stop_reason":"tool_use","content":[{"type":"thinking","thinking":"add them","signature":"sig-abc123"},{"type":"tool_use","id":"toolu_1","name":"add","input":{"a":20,"b":22}}]}"#;

const ANTHROPIC_ANSWERS: &str = r#"{"id":"msg_2","model":"test-model","stop_reason":"end_turn","content":[{"type":"text","text":"42"}]}"#;

#[tokio::test]
async fn replays_opaque_reasoning_state_on_the_next_turn() {
    let (base, requests) = serve_many(vec![canned(THINKS_THEN_CALLS), canned(ANTHROPIC_ANSWERS)]);
    let agent = Agent::new(anthropic_client(base)).tool(add);

    let mut messages = vec![Message::text(Role::User, "What is 20 + 22?")];
    let run = agent.run(&mut messages).await.unwrap();
    assert_eq!(run.stop, StopReason::Answered);

    // The signature is what Anthropic validates on the next request. Dropping
    // the block, or rebuilding the assistant turn from tool_calls() alone,
    // loses it and the real API rejects the request.
    let _first = requests.recv().unwrap();
    let second = requests.recv().unwrap();
    assert!(second.contains("sig-abc123"));
    assert!(second.contains(r#""type":"thinking""#));
}

#[tokio::test]
async fn chat_carries_the_transcript_between_asks() {
    let (base, requests) = serve_many(vec![canned(ANSWER), canned(ANSWER)]);
    let agent = Agent::new(client(base));
    let mut chat = agent.chat();

    assert_eq!(
        chat.ask("first question").await.unwrap().answer,
        "the answer is 42"
    );
    assert_eq!(
        chat.ask("second question").await.unwrap().answer,
        "the answer is 42"
    );

    let _first = requests.recv().unwrap();
    let second = requests.recv().unwrap();
    assert!(second.contains("first question"));
    assert!(second.contains("second question"));
    assert_eq!(chat.messages().len(), 4);
}
