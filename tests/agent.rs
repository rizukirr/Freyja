//! End-to-end tests for `Agent`, driven against a scripted local endpoint.
//!
//! `tests/streaming_transport.rs` serves a single request. The agent loop makes
//! several, so this serves a scripted sequence and hands back every request body
//! it captured. Most of what `Agent` promises is about what it sends on the next
//! turn, which only the captured bodies can show.

mod common;
use common::serve_many;
use freyja::{
    Agent, Client, Context, Decision, Dialect, EndpointConfig, GenerateRequest, Message,
    ReasoningEffort, Role, StopReason, Storage, StorageFuture, Tool, ToolChoice, ToolDefinition,
    ToolError, ToolFuture, tool,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[tool(description = "adds two numbers together", strict = true)]
fn add(a: i64, b: i64) -> i64 {
    a + b
}

#[tool(description = "waits, then echoes a word back")]
async fn echo(word: String) -> String {
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    word
}

/// Per-run state a tool reads out of the [`Context`].
struct UserId(String);

#[tool(description = "names the user this run belongs to")]
fn whoami(cx: &Context) -> Result<String, ToolError> {
    Ok(cx.require::<UserId>()?.0.clone())
}

#[tool(description = "always fails")]
fn sealed() -> Result<String, String> {
    Err("the vault is sealed".to_string())
}

/// A tool that keeps state between calls, which a plain function cannot.
struct Counter {
    calls: Arc<AtomicUsize>,
}

impl Tool for Counter {
    fn name(&self) -> &str {
        "counter"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("counter", "counts the calls it has served")
    }

    fn call<'a>(&'a self, _arguments: &'a str, _cx: &'a Context) -> ToolFuture<'a> {
        let served = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        Box::pin(async move { Ok(served.to_string()) })
    }
}

/// A tool whose name is not known until it is built.
struct Runtime {
    name: String,
}

impl Tool for Runtime {
    fn name(&self) -> &str {
        &self.name
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name.clone(), "defined at runtime")
    }

    fn call<'a>(&'a self, _arguments: &'a str, _cx: &'a Context) -> ToolFuture<'a> {
        Box::pin(async move { Ok("ran the runtime tool".to_string()) })
    }
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

    let mut messages = Vec::new();
    let run = agent
        .conversation_in(&mut messages)
        .send("Hi")
        .await
        .unwrap();

    assert_eq!(run.stop, StopReason::Answered);
    assert_eq!(run.answer, "the answer is 42");
    assert_eq!(run.turns, 1);
    assert_eq!(messages.len(), 2);
}

#[tokio::test]
async fn completes_a_full_tool_round_trip() {
    let (base, requests) = serve_many(vec![canned(CALLS_ADD), canned(ANSWER)]);
    let agent = Agent::new(client(base)).tool(add);

    let mut messages = Vec::new();
    let run = agent
        .conversation_in(&mut messages)
        .send("What is 20 + 22?")
        .await
        .unwrap();

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

    let mut messages = Vec::new();
    let run = agent
        .conversation_in(&mut messages)
        .send("loop")
        .await
        .unwrap();

    assert_eq!(run.stop, StopReason::MaxTurns);
    assert_eq!(run.turns, 2);
    assert!(run.answer.is_empty());
}

#[tokio::test]
async fn sums_usage_across_turns() {
    let (base, _requests) = serve_many(vec![canned(CALLS_ADD), canned(ANSWER)]);
    let agent = Agent::new(client(base)).tool(add);

    let mut messages = Vec::new();
    let run = agent
        .conversation_in(&mut messages)
        .send("What is 20 + 22?")
        .await
        .unwrap();

    assert_eq!(run.usage.total_tokens, 13);
}

const UNKNOWN: &str = r#"{"id":"chatcmpl-1","model":"test-model","choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_9","type":"function","function":{"name":"nope","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#;

const BAD_ARGS: &str = r#"{"id":"chatcmpl-1","model":"test-model","choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_2","type":"function","function":{"name":"add","arguments":"{\"a\":\"twenty\",\"b\":22}"}}]},"finish_reason":"tool_calls"}]}"#;

#[tokio::test]
async fn answers_an_unknown_tool_rather_than_skipping_it() {
    let (base, requests) = serve_many(vec![canned(UNKNOWN), canned(ANSWER)]);
    let agent = Agent::new(client(base)).tool(add);

    let mut messages = Vec::new();
    agent
        .conversation_in(&mut messages)
        .send("go")
        .await
        .unwrap();

    let _first = requests.recv().unwrap();
    let second = requests.recv().unwrap();
    assert!(second.contains("call_9"));
    assert!(second.contains("unknown tool"));
}

#[tokio::test]
async fn feeds_a_tool_error_back_to_the_model() {
    let (base, requests) = serve_many(vec![canned(BAD_ARGS), canned(ANSWER)]);
    let agent = Agent::new(client(base)).tool(add);

    let mut messages = Vec::new();
    let run = agent
        .conversation_in(&mut messages)
        .send("go")
        .await
        .unwrap();

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

    let mut messages = Vec::new();
    let started = std::time::Instant::now();
    agent
        .conversation_in(&mut messages)
        .send("go")
        .await
        .unwrap();

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

    let mut messages = Vec::new();
    let run = agent
        .conversation_in(&mut messages)
        .send("go")
        .await
        .unwrap();

    assert_eq!(run.stop, StopReason::Refused);
}

#[tokio::test]
async fn stops_when_the_generation_was_cut_short() {
    let (base, _requests) = serve_many(vec![canned(CUT_SHORT)]);
    let agent = Agent::new(client(base));

    let mut messages = Vec::new();
    let run = agent
        .conversation_in(&mut messages)
        .send("go")
        .await
        .unwrap();

    assert_eq!(run.stop, StopReason::Incomplete);
}

#[tokio::test]
async fn downgrades_required_after_the_first_turn() {
    let (base, requests) = serve_many(vec![canned(CALLS_ADD), canned(ANSWER)]);
    let agent = Agent::new(client(base))
        .tool(add)
        .tool_choice(ToolChoice::Required);

    let mut messages = Vec::new();
    agent
        .conversation_in(&mut messages)
        .send("go")
        .await
        .unwrap();

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

    let mut messages = Vec::new();
    let before = messages.clone();

    assert!(
        agent
            .conversation_in(&mut messages)
            .send("go")
            .await
            .is_err()
    );
    assert_eq!(messages, before);
}

/// A `thinking` block Freyja does not model, beside a tool call.
const THINKS_THEN_CALLS: &str = r#"{"id":"msg_1","model":"test-model","stop_reason":"tool_use","content":[{"type":"thinking","thinking":"add them","signature":"sig-abc123"},{"type":"tool_use","id":"toolu_1","name":"add","input":{"a":20,"b":22}}]}"#;

const ANTHROPIC_ANSWERS: &str = r#"{"id":"msg_2","model":"test-model","stop_reason":"end_turn","content":[{"type":"text","text":"42"}]}"#;

#[tokio::test]
async fn replays_opaque_reasoning_state_on_the_next_turn() {
    let (base, requests) = serve_many(vec![canned(THINKS_THEN_CALLS), canned(ANTHROPIC_ANSWERS)]);
    let agent = Agent::new(anthropic_client(base)).tool(add);

    let mut messages = Vec::new();
    let run = agent
        .conversation_in(&mut messages)
        .send("What is 20 + 22?")
        .await
        .unwrap();
    assert_eq!(run.stop, StopReason::Answered);

    // The signature is what Anthropic validates on the next request. Dropping
    // the block, or rebuilding the assistant turn from tool_calls() alone,
    // loses it and the real API rejects the request.
    let _first = requests.recv().unwrap();
    let second = requests.recv().unwrap();
    assert!(second.contains("sig-abc123"));
    assert!(second.contains(r#""type":"thinking""#));
}

const CALLS_COUNTER: &str = r#"{"id":"chatcmpl-1","model":"test-model","choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_c1","type":"function","function":{"name":"counter","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#;

const CALLS_WHOAMI: &str = r#"{"id":"chatcmpl-1","model":"test-model","choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_w1","type":"function","function":{"name":"whoami","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#;

const CALLS_SEALED: &str = r#"{"id":"chatcmpl-1","model":"test-model","choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_s1","type":"function","function":{"name":"sealed","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#;

const CALLS_RUNTIME: &str = r#"{"id":"chatcmpl-1","model":"test-model","choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_r1","type":"function","function":{"name":"lookup_v2","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#;

const CALLS_GHOST: &str = r#"{"id":"chatcmpl-1","model":"test-model","choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_g1","type":"function","function":{"name":"ghost","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#;

/// Same tool as [`CALLS_ADD`], differing only in its arguments.
const CALLS_ADD_99: &str = r#"{"id":"chatcmpl-1","model":"test-model","choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_a99","type":"function","function":{"name":"add","arguments":"{\"a\":99,\"b\":1}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":5,"completion_tokens":5,"total_tokens":10}}"#;

#[tokio::test]
async fn a_stateful_tool_keeps_its_state_across_the_run() {
    let (base, _requests) = serve_many(vec![canned(CALLS_COUNTER), canned(ANSWER)]);
    let calls = Arc::new(AtomicUsize::new(0));
    let agent = Agent::new(client(base)).tool(Counter {
        calls: Arc::clone(&calls),
    });

    let before = calls.load(Ordering::SeqCst);
    let mut messages = Vec::new();
    let run = agent
        .conversation_in(&mut messages)
        .send("go")
        .await
        .unwrap();

    assert_eq!(run.stop, StopReason::Answered);
    assert!(calls.load(Ordering::SeqCst) > before);
}

#[tokio::test]
async fn a_tool_reads_per_run_state_out_of_the_context() {
    let (base, requests) = serve_many(vec![canned(CALLS_WHOAMI), canned(ANSWER)]);
    let agent = Agent::new(client(base)).tool(whoami);

    let mut context = Context::new();
    context.insert(UserId("u-42".to_string()));

    let mut messages = Vec::new();
    let run = agent
        .conversation_in(&mut messages)
        .send_with("who am I?", &context)
        .await
        .unwrap();

    assert_eq!(run.stop, StopReason::Answered);
    let _first = requests.recv().unwrap();
    let second = requests.recv().unwrap();
    assert!(second.contains("call_w1"));
    // The macro serialises with serde, so a `String` result is JSON-quoted and
    // those quotes are escaped again inside the request body.
    assert!(second.contains(r#"\"u-42\""#));
}

#[tokio::test]
async fn a_missing_context_value_reaches_the_model_as_text() {
    let (base, requests) = serve_many(vec![canned(CALLS_WHOAMI), canned(ANSWER)]);
    let agent = Agent::new(client(base)).tool(whoami);

    let mut messages = Vec::new();
    // `run` supplies an empty context, so the tool fails rather than panicking.
    let run = agent
        .conversation_in(&mut messages)
        .send("who am I?")
        .await
        .unwrap();

    assert_eq!(run.stop, StopReason::Answered);
    let _first = requests.recv().unwrap();
    let second = requests.recv().unwrap();
    assert!(second.contains("call_w1"));
    assert!(second.contains("context is missing a value of type"));
    assert!(second.contains("UserId"));
    assert!(!second.contains(r#"\"u-42\""#));
}

#[tokio::test]
async fn a_fallible_tool_reports_its_error_as_a_tool_result() {
    let (base, requests) = serve_many(vec![canned(CALLS_SEALED), canned(ANSWER)]);
    let agent = Agent::new(client(base)).tool(sealed);

    let mut messages = Vec::new();
    let run = agent
        .conversation_in(&mut messages)
        .send("open it")
        .await
        .unwrap();

    assert_eq!(run.stop, StopReason::Answered);
    let _first = requests.recv().unwrap();
    let second = requests.recv().unwrap();
    assert!(second.contains("call_s1"));
    // `Display`, not `Debug`: no `Execution(..)` wrapper around the message.
    assert!(second.contains("error: the vault is sealed"));
}

#[tokio::test]
async fn a_runtime_named_tool_is_dispatched_by_its_name() {
    let (base, requests) = serve_many(vec![canned(CALLS_RUNTIME), canned(ANSWER)]);
    let name = format!("lookup_v{}", 2);
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(Runtime { name })];
    let agent = Agent::new(client(base)).tools(tools);

    let mut messages = Vec::new();
    let run = agent
        .conversation_in(&mut messages)
        .send("go")
        .await
        .unwrap();

    assert_eq!(run.stop, StopReason::Answered);
    let first = requests.recv().unwrap();
    assert!(first.contains("lookup_v2"));
    let second = requests.recv().unwrap();
    // A hand-written tool returns its own string, so no serde quoting here.
    assert!(second.contains("ran the runtime tool"));
    assert!(!second.contains("unknown tool"));
}

/// Allows a call only when the run carries a [`UserId`].
fn needs_a_user(_name: &str, _arguments: &str, cx: &Context) -> Decision {
    match cx.get::<UserId>() {
        Some(_) => Decision::Allow,
        None => Decision::Deny("no user on this run".to_string()),
    }
}

#[tokio::test]
async fn a_denial_reaches_the_model_as_a_tool_result() {
    let (base, requests) = serve_many(vec![canned(CALLS_ADD), canned(ANSWER)]);
    let agent =
        Agent::new(client(base))
            .tool(add)
            .guard(|name: &str, _arguments: &str, _cx: &Context| match name {
                "add" => Decision::Deny("arithmetic is off limits".to_string()),
                _ => Decision::Allow,
            });

    let mut messages = Vec::new();
    let run = agent
        .conversation_in(&mut messages)
        .send("What is 20 + 22?")
        .await
        .expect("run");

    assert_eq!(run.stop, StopReason::Answered);
    let _first = requests.recv().unwrap();
    let second = requests.recv().unwrap();
    assert!(second.contains("call_1"));
    assert!(second.contains("arithmetic is off limits"));
}

#[tokio::test]
async fn a_denied_tool_never_runs() {
    let (base, _requests) = serve_many(vec![canned(CALLS_COUNTER), canned(ANSWER)]);
    let calls = Arc::new(AtomicUsize::new(0));
    let agent = Agent::new(client(base))
        .tool(Counter {
            calls: Arc::clone(&calls),
        })
        .guard(|_name: &str, _arguments: &str, _cx: &Context| {
            Decision::Deny("nothing counts today".to_string())
        });

    let mut messages = Vec::new();
    let run = agent
        .conversation_in(&mut messages)
        .send("go")
        .await
        .expect("run");

    assert_eq!(run.stop, StopReason::Answered);
    // The load-bearing assertion: denial text in the transcript would still
    // appear if the guard had returned it *and* let the tool run.
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn the_guard_reads_the_context() {
    let (denied_base, denied_requests) = serve_many(vec![canned(CALLS_WHOAMI), canned(ANSWER)]);
    let denied = Agent::new(client(denied_base))
        .tool(whoami)
        .guard(needs_a_user);

    let mut messages = Vec::new();
    denied
        .conversation_in(&mut messages)
        .send("who am I?")
        .await
        .expect("run");

    let _first = denied_requests.recv().unwrap();
    let second = denied_requests.recv().unwrap();
    assert!(second.contains("no user on this run"));
    assert!(!second.contains(r#"\"u-42\""#));

    let (allowed_base, allowed_requests) = serve_many(vec![canned(CALLS_WHOAMI), canned(ANSWER)]);
    let allowed = Agent::new(client(allowed_base))
        .tool(whoami)
        .guard(needs_a_user);

    let mut context = Context::new();
    context.insert(UserId("u-42".to_string()));

    let mut messages = Vec::new();
    allowed
        .conversation_in(&mut messages)
        .send_with("who am I?", &context)
        .await
        .expect("run_with");

    let _first = allowed_requests.recv().unwrap();
    let second = allowed_requests.recv().unwrap();
    assert!(second.contains(r#"\"u-42\""#));
    assert!(!second.contains("no user on this run"));
}

/// Refuses a call on its argument content alone, ignoring name and context.
fn refuses_ninety_nine(_name: &str, arguments: &str, _cx: &Context) -> Decision {
    if arguments.contains(r#""a":99"#) {
        Decision::Deny("99 is reserved".to_string())
    } else {
        Decision::Allow
    }
}

#[tokio::test]
async fn the_guard_reads_the_raw_arguments() {
    // Same tool, same guard: only the arguments decide.
    let (denied_base, denied_requests) = serve_many(vec![canned(CALLS_ADD_99), canned(ANSWER)]);
    let denied = Agent::new(client(denied_base))
        .tool(add)
        .guard(refuses_ninety_nine);

    let mut messages = Vec::new();
    denied
        .conversation_in(&mut messages)
        .send("What is 99 + 1?")
        .await
        .expect("run");

    let _first = denied_requests.recv().unwrap();
    let second = denied_requests.recv().unwrap();
    assert!(second.contains("call_a99"));
    assert!(second.contains("denied: 99 is reserved"));
    assert!(!second.contains(r#""content":"100""#));

    let (allowed_base, allowed_requests) = serve_many(vec![canned(CALLS_ADD), canned(ANSWER)]);
    let allowed = Agent::new(client(allowed_base))
        .tool(add)
        .guard(refuses_ninety_nine);

    let mut messages = Vec::new();
    let run = allowed
        .conversation_in(&mut messages)
        .send("What is 20 + 22?")
        .await
        .expect("run");

    assert_eq!(run.stop, StopReason::Answered);
    let _first = allowed_requests.recv().unwrap();
    let second = allowed_requests.recv().unwrap();
    assert!(second.contains(r#""content":"42""#));
    assert!(!second.contains("99 is reserved"));
    assert!(!second.contains("denied:"));
}

#[tokio::test]
async fn the_guard_sees_a_name_no_tool_answers_to() {
    let (base, requests) = serve_many(vec![canned(CALLS_GHOST), canned(ANSWER)]);
    let agent =
        Agent::new(client(base))
            .tool(add)
            .guard(|name: &str, _arguments: &str, _cx: &Context| match name {
                "ghost" => Decision::Deny("no such thing".to_string()),
                _ => Decision::Allow,
            });

    let mut messages = Vec::new();
    let run = agent
        .conversation_in(&mut messages)
        .send("go")
        .await
        .expect("run");

    assert_eq!(run.stop, StopReason::Answered);
    let _first = requests.recv().unwrap();
    let second = requests.recv().unwrap();
    assert!(second.contains("call_g1"));
    // The guard runs before the lookup, so its reason replaces the miss.
    assert!(second.contains("no such thing"));
    assert!(!second.contains("unknown tool"));
}

#[tokio::test]
async fn the_system_instruction_is_sent_and_stays_out_of_the_transcript() {
    let (base, requests) = serve_many(vec![canned(ANSWER), canned(ANSWER)]);
    let agent = Agent::new(client(base)).system("SENTINEL-SYSTEM");

    let mut messages = Vec::new();
    agent
        .conversation_in(&mut messages)
        .send("first")
        .await
        .expect("first run");
    let original_len = messages.len();
    agent
        .conversation_in(&mut messages)
        .send("second")
        .await
        .expect("second run");

    // Sent on every turn, not just the first.
    let first = requests.recv().expect("first request");
    let second = requests.recv().expect("second request");
    assert!(first.contains("SENTINEL-SYSTEM"), "{first}");
    assert!(second.contains("SENTINEL-SYSTEM"), "{second}");
    assert!(first.contains("\"role\":\"system\""), "{first}");

    // Never enters the caller's transcript: the second run added one user turn
    // and one assistant turn on top of what the first run left behind.
    assert_eq!(messages.len(), original_len + 2);
    assert!(messages.iter().all(|m| m.role != Role::System));
}

/// A backend that returns a tool result whose call it dropped, which is what
/// `Agent` must repair before sending.
struct Orphaning;

impl Storage for Orphaning {
    fn load(&mut self) -> StorageFuture<'_, Vec<Message>> {
        Box::pin(async {
            Ok(vec![
                Message::text(Role::User, "earlier"),
                Message::tool_result("call_gone", "SENTINEL-ORPHAN"),
            ])
        })
    }
    fn append(&mut self, _messages: Vec<Message>) -> StorageFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn clear(&mut self) -> StorageFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn an_orphaned_tool_result_from_a_backend_is_repaired() {
    let (base, requests) = serve_many(vec![canned(ANSWER)]);
    let agent = Agent::new(client(base));

    agent
        .conversation_in(Orphaning)
        .send("a question")
        .await
        .expect("run");

    let sent = requests.recv().expect("request");
    assert!(!sent.contains("SENTINEL-ORPHAN"), "{sent}");
    assert!(sent.contains("a question"), "{sent}");
}

#[tokio::test]
async fn the_settings_reach_the_wire() {
    let (base, requests) = serve_many(vec![canned(ANSWER)]);
    let agent = Agent::new(client(base))
        .model("sentinel-model")
        .max_tokens(123)
        .temperature(0.25)
        .top_p(0.5)
        .reasoning_effort(ReasoningEffort::High);

    let mut messages = Vec::new();
    agent
        .conversation_in(&mut messages)
        .send("go")
        .await
        .expect("run");

    // Asserted as key and value together. The captured string is the whole
    // request including its headers, so a bare value can match something the
    // test did not set, such as a content length.
    let sent = requests.recv().expect("captured request");
    assert!(sent.contains(r#""model":"sentinel-model""#), "{sent}");
    // The client in this file leaves `EndpointConfig::token_limit_field` at
    // its default, `TokenLimitField::MaxTokens`, so the wire key is
    // `max_tokens` rather than `max_completion_tokens`.
    assert!(sent.contains(r#""max_tokens":123"#), "{sent}");
    assert!(sent.contains(r#""temperature":0.25"#), "{sent}");
    assert!(sent.contains(r#""top_p":0.5"#), "{sent}");
    assert!(sent.contains(r#""reasoning_effort":"high""#), "{sent}");
}

#[tokio::test]
async fn extra_for_reaches_the_wire() {
    let (base, requests) = serve_many(vec![canned(ANSWER)]);
    let agent = Agent::new(client(base)).extra_for(
        Dialect::OpenAiChat,
        serde_json::json!({"sentinel_field": "sentinel-value"}),
    );

    let mut messages = Vec::new();
    agent
        .conversation_in(&mut messages)
        .send("go")
        .await
        .expect("run");

    let sent = requests.recv().expect("captured request");
    assert!(
        sent.contains(r#""sentinel_field":"sentinel-value""#),
        "{sent}"
    );
}

#[tokio::test]
async fn a_stored_conversation_carries_across_calls() {
    let (base, requests) = serve_many(vec![canned(ANSWER), canned(ANSWER)]);
    let agent = Agent::new(client(base));
    let mut chat = agent.conversation();

    chat.send("first question").await.expect("first");
    chat.send("second question").await.expect("second");

    let _first = requests.recv().expect("first request");
    let second = requests.recv().expect("second request");
    assert!(second.contains("first question"), "{second}");
    assert!(second.contains("second question"), "{second}");
}

/// A second conversation handed out by the same agent starts from empty
/// storage, not from whatever an earlier conversation already holds.
#[tokio::test]
async fn separate_conversations_do_not_share_storage() {
    let (base, _requests) = serve_many(vec![canned(ANSWER)]);
    let agent = Agent::new(client(base));

    let mut messages = Vec::new();
    agent
        .conversation_in(&mut messages)
        .send("held by the caller")
        .await
        .expect("run");

    let other = agent.conversation();
    assert!(other.storage().is_empty());
    assert!(messages.len() > 1);
}

#[tokio::test]
async fn a_window_sends_less_than_the_whole_conversation() {
    let (base, requests) = serve_many(vec![canned(ANSWER), canned(ANSWER)]);
    let agent = Agent::new(client(base));
    let mut chat = agent.conversation().window(1);

    chat.send("turn-one").await.expect("first");
    chat.send("turn-two").await.expect("second");

    let _first = requests.recv().expect("first request");
    let second = requests.recv().expect("second request");
    assert!(!second.contains("turn-one"), "{second}");
    assert!(second.contains("turn-two"), "{second}");
    assert_eq!(chat.storage().len(), 4);
}

#[tokio::test]
async fn clearing_storage_forgets_the_conversation() {
    let (base, requests) = serve_many(vec![canned(ANSWER), canned(ANSWER)]);
    let agent = Agent::new(client(base));

    let mut messages = Vec::new();
    agent
        .conversation_in(&mut messages)
        .send("first question")
        .await
        .expect("first");
    messages.clear();
    agent
        .conversation_in(&mut messages)
        .send("second question")
        .await
        .expect("second");

    let _first = requests.recv().expect("first request");
    let second = requests.recv().expect("second request");
    assert!(!second.contains("first question"), "{second}");
}
