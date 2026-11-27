# tracing-hook: Implementation Plan

**Spec:** docs/specs/2026-11-27-tracing-hook-design.md
**Goal:** Ship `freyja::Tracing`, a hook behind the optional `tracing` feature that emits one event per model call and per tool call.
**Architecture:** One module, `src/tracing_hook.rs`, compiled only with the feature, re-exported from `lib.rs`. It implements `after_model` and `after_tool` with `tracing::info!` and returns `Flow::Continue`. Tests record events with a hand-written subscriber.

## Global constraints
- Rust 2024, MSRV 1.88.
- `tracing = { version = "0.1.44", optional = true, default-features = false, features = ["std"] }`. No other new dependency, normal or dev.
- No live LLM calls.
- Use `cargo --offline` for every cargo command.

### Task 1: The Tracing hook and its tests → verify: `cargo --offline test --features tracing --test tracing` exits 0, `cargo --offline test --features tracing --test tracing -- --list 2>/dev/null | grep -c ': test$'` prints a number at least 3, and `cargo --offline tree -e normal --depth 1 | grep -c ' tracing v'` prints a number at most 0

**Blocked by:** none

**Files:**
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Create: `src/tracing_hook.rs`
- Modify: `src/lib.rs`
- Create: `tests/tracing.rs`

- [ ] Step 1: Invoke `lazy` before writing any code.
- [ ] Step 2: In `Cargo.toml`, under `[dependencies]` after the `serde_json` line, add:

```toml
# Only with the `tracing` feature. It is already in the tree through hyper, so
# enabling it adds no crate.
tracing = { version = "0.1.44", optional = true, default-features = false, features = ["std"] }
```

and after the `[dev-dependencies]` block, add:

```toml
[features]
# `freyja::Tracing`, a hook that reports every model call and tool call as a
# `tracing` event.
tracing = ["dep:tracing"]
```

- [ ] Step 3: Create `src/tracing_hook.rs`:

```rust
//! A ready-made hook that reports every call as a `tracing` event.

use crate::{Context, Flow, GenerateResponse, Hook};
use std::time::Duration;

/// A [`Hook`] that emits one `tracing` event per model call and per tool
/// call, at `INFO` with target `freyja`.
///
/// A model call reports `model`, `status`, `elapsed_ms`, and `input_tokens`,
/// `output_tokens` and `total_tokens` when the response carries usage. A tool
/// call reports `tool`, `elapsed_ms` and `output_bytes`, including a call the
/// guard refused. No message text, tool arguments or tool output is logged
/// unless [`Tracing::with_content`] is set, because they can hold user data
/// and secrets. It never stops a run.
///
/// Freyja installs no subscriber: events go wherever the application's
/// `tracing` subscriber sends them. Requires the `tracing` feature.
///
/// ```
/// # fn run(client: freyja::Client) {
/// let agent = freyja::Agent::new(client).hook(freyja::Tracing::new());
/// # }
/// ```
///
/// The source is a short example of a complete hook.
#[derive(Debug, Default, Clone, Copy)]
pub struct Tracing {
    content: bool,
}

impl Tracing {
    /// Reports calls without their content.
    pub fn new() -> Self {
        Self::default()
    }

    /// Also logs the model's text as `text`, and each tool call's
    /// `arguments` and `output`.
    pub fn with_content(mut self) -> Self {
        self.content = true;
        self
    }
}

impl Hook for Tracing {
    fn after_model(&self, response: &GenerateResponse, elapsed: Duration, _cx: &Context) -> Flow {
        let usage = response.usage.as_ref();
        let input_tokens = usage.map(|usage| usage.input_tokens);
        let output_tokens = usage.map(|usage| usage.output_tokens);
        let total_tokens = usage.map(|usage| usage.total_tokens);
        let elapsed_ms = elapsed.as_millis() as u64;
        if self.content {
            tracing::info!(
                target: "freyja",
                model = %response.model,
                status = ?response.status,
                elapsed_ms,
                input_tokens,
                output_tokens,
                total_tokens,
                text = %response.output_text(),
                "model call"
            );
        } else {
            tracing::info!(
                target: "freyja",
                model = %response.model,
                status = ?response.status,
                elapsed_ms,
                input_tokens,
                output_tokens,
                total_tokens,
                "model call"
            );
        }
        Flow::Continue
    }

    fn after_tool(
        &self,
        name: &str,
        arguments: &str,
        output: &str,
        elapsed: Duration,
        _cx: &Context,
    ) -> Flow {
        let elapsed_ms = elapsed.as_millis() as u64;
        if self.content {
            tracing::info!(
                target: "freyja",
                tool = name,
                elapsed_ms,
                output_bytes = output.len(),
                arguments,
                output,
                "tool call"
            );
        } else {
            tracing::info!(
                target: "freyja",
                tool = name,
                elapsed_ms,
                output_bytes = output.len(),
                "tool call"
            );
        }
        Flow::Continue
    }
}
```

If `Option<u64>` fields or `usize` for `output_bytes` do not compile with `tracing` 0.1.44, convert them (for example `output.len() as u64`) without dropping any field.

- [ ] Step 4: In `src/lib.rs`, after the line `mod summarize;` add `#[cfg(feature = "tracing")]` and `mod tracing_hook;` on two lines. After the line `pub use summarize::Summarizer;` add `#[cfg(feature = "tracing")]` and `pub use tracing_hook::Tracing;` on two lines.

- [ ] Step 5: Create `tests/tracing.rs`:

```rust
//! The `Tracing` hook, under a subscriber that records every event.

#![cfg(feature = "tracing")]

use freyja::testing::ScriptedModel;
use freyja::{
    Agent, Decision, GenerateResponse, OutputContent, ResponseStatus, StopReason, Tracing, Usage,
    tool,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

#[tool(description = "adds two numbers together")]
fn add(a: i64, b: i64) -> i64 {
    a + b
}

/// One recorded event: its target, level and every field as text.
#[derive(Debug, Clone)]
struct Seen {
    target: String,
    level: Level,
    fields: BTreeMap<String, String>,
}

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<Seen>>>);

struct Fields<'a>(&'a mut BTreeMap<String, String>);

impl Visit for Fields<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().into(), value.into());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.0.insert(field.name().into(), format!("{value:?}"));
    }
}

impl Subscriber for Recorder {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _span: &Id, _values: &Record<'_>) {}
    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}
    fn event(&self, event: &Event<'_>) {
        let mut fields = BTreeMap::new();
        event.record(&mut Fields(&mut fields));
        self.0.lock().expect("not poisoned").push(Seen {
            target: event.metadata().target().into(),
            level: *event.metadata().level(),
            fields,
        });
    }
    fn enter(&self, _span: &Id) {}
    fn exit(&self, _span: &Id) {}
}

impl Recorder {
    fn named(&self, message: &str) -> Vec<Seen> {
        self.0
            .lock()
            .expect("not poisoned")
            .iter()
            .filter(|seen| seen.fields.get("message").map(String::as_str) == Some(message))
            .cloned()
            .collect()
    }
}

/// A tool call, then an answer that reports usage.
fn script() -> ScriptedModel {
    ScriptedModel::new()
        .tool_call("add", json!({"a": 1, "b": 2}))
        .respond(GenerateResponse {
            id: "r".into(),
            model: "scripted".into(),
            status: ResponseStatus::Completed,
            content: vec![OutputContent::Text("3".into())],
            usage: Some(Usage {
                input_tokens: 10,
                output_tokens: 2,
                total_tokens: 12,
            }),
            provider_metadata: None,
        })
}

#[tokio::test(flavor = "current_thread")]
async fn every_model_call_and_tool_call_is_one_event() {
    let recorder = Recorder::default();
    let _guard = tracing::subscriber::set_default(recorder.clone());

    let run = Agent::new(script())
        .tool(add)
        .hook(Tracing::new())
        .ask("add 1 and 2")
        .await
        .expect("run");

    assert_eq!(run.stop, StopReason::Answered);
    assert_eq!(run.answer, "3");

    let models = recorder.named("model call");
    assert_eq!(models.len(), 2);
    for seen in &models {
        assert_eq!(seen.target, "freyja");
        assert_eq!(seen.level, Level::INFO);
        for field in ["model", "status", "elapsed_ms"] {
            assert!(seen.fields.contains_key(field), "missing {field}");
        }
    }
    assert!(!models[0].fields.contains_key("input_tokens"));
    assert_eq!(models[1].fields["model"], "scripted");
    assert_eq!(models[1].fields["input_tokens"], "10");
    assert_eq!(models[1].fields["output_tokens"], "2");
    assert_eq!(models[1].fields["total_tokens"], "12");

    let tools = recorder.named("tool call");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].target, "freyja");
    assert_eq!(tools[0].level, Level::INFO);
    assert_eq!(tools[0].fields["tool"], "add");
    assert_eq!(tools[0].fields["output_bytes"], "1");
    assert!(tools[0].fields.contains_key("elapsed_ms"));

    for seen in models.iter().chain(&tools) {
        for field in ["text", "arguments", "output"] {
            assert!(!seen.fields.contains_key(field), "{field} logged by default");
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn with_content_logs_text_arguments_and_output() {
    let recorder = Recorder::default();
    let _guard = tracing::subscriber::set_default(recorder.clone());

    Agent::new(script())
        .tool(add)
        .hook(Tracing::new().with_content())
        .ask("add 1 and 2")
        .await
        .expect("run");

    let models = recorder.named("model call");
    assert_eq!(models[1].fields["text"], "3");
    let tools = recorder.named("tool call");
    assert_eq!(tools[0].fields["output"], "3");
    let arguments: serde_json::Value =
        serde_json::from_str(&tools[0].fields["arguments"]).expect("json");
    assert_eq!(arguments, json!({"a": 1, "b": 2}));
}

#[tokio::test(flavor = "current_thread")]
async fn a_refused_tool_call_is_still_an_event() {
    let recorder = Recorder::default();
    let _guard = tracing::subscriber::set_default(recorder.clone());

    let run = Agent::new(script())
        .tool(add)
        .guard(|_name, _arguments, _cx| Decision::Deny("not today".into()))
        .hook(Tracing::new())
        .ask("add 1 and 2")
        .await
        .expect("run");

    assert_eq!(run.answer, "3");
    let tools = recorder.named("tool call");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].fields["tool"], "add");
}
```

If a detail of the public API differs (for example `Usage` not exported at the crate root, or `StopReason` comparison), adjust the test to the real API while keeping every assertion's intent.

- [ ] Step 6: Run `cargo --offline test --features tracing --test tracing`, `cargo --offline clippy --all-targets --all-features -- -D warnings`, `cargo --offline clippy --all-targets -- -D warnings` and `cargo --offline tree -e normal --depth 1`.
- [ ] Step 7: Commit the five files as "Add Tracing, a hook that reports every call as a tracing event".

### Task 2: Document Tracing → verify: `RUSTDOCFLAGS='-D warnings' cargo --offline doc --no-deps --all-features` exits 0 and `grep -l 'Tracing' docs/building-an-agent.md CHANGELOG.md README.md | wc -l` prints a number at least 3

**Blocked by:** Task 1

**Files:**
- Modify: `docs/building-an-agent.md`
- Modify: `CHANGELOG.md`
- Modify: `README.md`

- [ ] Step 1: Invoke `lazy` and `plain` before writing.
- [ ] Step 2: In `docs/building-an-agent.md`, after the paragraph that follows the `Budget` example and its `.hook(Timing)` builder block (the paragraph beginning "`run.stop` tells you why the loop ended"), add a short paragraph: Freyja ships one hook, `Tracing`, behind the `tracing` feature (`freyja = { version = "0.5", features = ["tracing"] }`). `.hook(freyja::Tracing::new())` emits one `INFO` event with target `freyja` per model call and per tool call, with durations and token counts, and with no message content unless `.with_content()` is set. Its source, `src/tracing_hook.rs`, is a complete hook to read before writing one. Show the one-line builder call in a code block.
- [ ] Step 3: In `CHANGELOG.md`, under `## Unreleased`, add `### Added` and one bullet: `**`Tracing`, a hook that reports every model call and tool call as a `tracing` event, behind the `tracing` feature.**` followed by one sentence naming the fields, that content is off unless `with_content` is set, and that the feature adds no crate because `tracing` is already in the tree.
- [ ] Step 4: In `README.md`, in the Phase 5 roadmap paragraph, replace "`tracing` instrumentation, cost accounting, and record and replay for deterministic tests are not built." with "`freyja::Tracing`, behind the `tracing` feature, reports every model call and tool call as a `tracing` event. Spans, cost accounting, and record and replay for deterministic tests are not built."
- [ ] Step 5: Run `RUSTDOCFLAGS='-D warnings' cargo --offline doc --no-deps --all-features`.
- [ ] Step 6: Commit the three files as "Document Tracing".
