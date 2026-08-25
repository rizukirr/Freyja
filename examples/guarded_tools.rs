//! A refund desk: the four things a tool can do beyond being a plain function.
//!
//! `agent` shows the loop; `async_tools` shows concurrency. Both give the
//! model tools that are pure functions of their arguments. Real tools rarely
//! are. This one keeps a ledger between calls, reads who the run belongs to
//! out of the [`Context`], reports a lookup miss the model can recover from,
//! and refuses to issue a refund the operator is not authorised for — the
//! same agent, run twice, differing only in the context it is handed.
//!
//! ```sh
//! cargo run --example guarded_tools
//! ```

use freyja::{
    Agent, Client, Context, Decision, EndpointPreset, Message, Role, StopReason, Tool,
    ToolDefinition, ToolError, ToolFuture, tool,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

/// Who the run belongs to, and what they are allowed to do. [`Context`] is
/// keyed by type, so this is a newtype rather than a bare `String`: a `String`
/// key would collide with every other string any tool wanted to stash.
struct Operator {
    badge: String,
    may_refund: bool,
}

/// The refunds actually written, in order. A `#[tool]` function cannot hold
/// this — it is called through a function pointer with nowhere to keep it — so
/// this one is hand-written on a struct whose fields outlive the call. The
/// `Arc<Mutex<..>>` is what lets `main` still read the ledger afterwards; the
/// agent only ever sees the `Tool`.
struct Ledger {
    entries: Arc<Mutex<Vec<String>>>,
}

impl Tool for Ledger {
    fn name(&self) -> &str {
        "issue_refund"
    }

    fn definition(&self) -> ToolDefinition {
        // Hand-written means the schema is hand-written too: no macro read
        // this struct's fields, because the arguments are not its fields.
        ToolDefinition::new("issue_refund", "refunds an order, in cents").parameters(json!({
            "type": "object",
            "properties": {
                "order": { "type": "string" },
                "cents": { "type": "integer" }
            },
            "required": ["order", "cents"]
        }))
    }

    fn call<'a>(&'a self, arguments: &'a str, _cx: &'a Context) -> ToolFuture<'a> {
        let parsed: Result<Value, _> = serde_json::from_str(arguments);
        Box::pin(async move {
            let arguments = parsed.map_err(ToolError::Arguments)?;
            let order = arguments["order"].as_str().unwrap_or("unknown");
            let cents = arguments["cents"].as_u64().unwrap_or(0);

            let entry = format!("{order} refunded {cents} cents");
            self.entries
                .lock()
                .expect("ledger poisoned")
                .push(entry.clone());
            Ok(entry)
        })
    }
}

/// The desk's order book, standing in for a database.
const ORDERS: [(&str, u64); 2] = [("A-1002", 4_250), ("A-1177", 990)];

/// Reads the run's own state rather than taking it as an argument. `cx` may
/// only be the first parameter, and the macro leaves it out of the schema the
/// model sees — the model cannot name an operator, it can only ask which one
/// is on duty.
#[tool(description = "names the operator this session belongs to")]
fn on_duty(cx: &Context) -> Result<String, ToolError> {
    Ok(cx.require::<Operator>()?.badge.clone())
}

/// Fallible on purpose: a mistyped order id is the model's mistake to fix, not
/// a reason to end the run. The `Err` arm arrives as `error: no order …`, so
/// the model can read the known ids out of it and try again.
#[tool(description = "returns the total of an order, in cents", strict = true)]
fn order_total(order: String) -> Result<u64, String> {
    ORDERS
        .iter()
        .find(|(id, _)| *id == order)
        .map(|(_, cents)| *cents)
        .ok_or_else(|| format!("no order {order}; the order book holds A-1002 and A-1177"))
}

/// One policy covering both outcomes. The guard runs before every tool call
/// and reads the same [`Context`] the tools do, so authorisation lives on the
/// run rather than in the tool — and `issue_refund` needs no branch of its own.
fn refunds_need_authority(name: &str, _arguments: &str, cx: &Context) -> Decision {
    if name != "issue_refund" {
        return Decision::Allow;
    }
    match cx.get::<Operator>() {
        Some(operator) if operator.may_refund => Decision::Allow,
        Some(operator) => Decision::Deny(format!("{} may not issue refunds", operator.badge)),
        None => Decision::Deny("this session has no operator".to_string()),
    }
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let provider = EndpointPreset::OpenAi;
    let Some(client) = Client::from_env(provider) else {
        eprintln!("{} is missing or empty", provider.api_key_env());
        return;
    };

    // Kept out of the agent so it can be read once both runs are done.
    let entries: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let agent = Agent::new(client)
        .tool(Ledger {
            entries: Arc::clone(&entries),
        })
        .tool(on_duty)
        .tool(order_total)
        .guard(refunds_need_authority)
        .system(
            "You work a refund desk. Look up what an order is worth before \
             refunding it, and say who authorised the refund. If a tool turns \
             you down, report what it said instead of trying again.",
        )
        .max_turns(6);

    // The same agent, the same tools, the same question. Only the context
    // differs, and with it what the guard decides.
    let operators = [
        Operator {
            badge: "op-7".to_string(),
            may_refund: true,
        },
        Operator {
            badge: "op-9".to_string(),
            may_refund: false,
        },
    ];

    for operator in operators {
        println!(
            "\n--- {} (may_refund: {}) ---",
            operator.badge, operator.may_refund
        );

        let mut context = Context::new();
        context.insert(operator);

        let mut messages = vec![Message::text(
            Role::User,
            // A-1003 does not exist: the first lookup fails, and the model
            // recovers from the error text without the run ending.
            "Refund order A-1003 in full. If that is not an order, refund A-1002 instead.",
        )];

        match agent.run_with(&mut messages, &context).await {
            Ok(run) => {
                println!("answer: {}", run.answer);
                println!(
                    "stop: {:?}, turns: {}, tokens: {}",
                    run.stop, run.turns, run.usage.total_tokens
                );
                if run.stop == StopReason::MaxTurns {
                    eprintln!("(stopped: ran out of turns before answering)");
                }
            }
            Err(error) => {
                eprintln!("{} failed: {error}", error.endpoint());
                if error.is_retryable() {
                    eprintln!("(transient — try again)");
                }
            }
        }
    }

    // The ledger is per tool, not per run: what the first run wrote is still
    // here, and the denied run added nothing to it.
    println!("\n--- ledger ---");
    let entries = entries.lock().expect("ledger poisoned");
    if entries.is_empty() {
        println!("(no refunds written)");
    }
    for entry in entries.iter() {
        println!("{entry}");
    }
}
