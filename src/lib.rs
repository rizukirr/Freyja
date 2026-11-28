//! A provider-neutral LLM client: one request model, four wire dialects, any
//! compatible endpoint.
//!
//! Built toward agent orchestration, see the roadmap in the repository README.
//! Today it is the client layer and the tool-calling round trip that layer needs.
//!
//! Freyja's core is a provider-neutral request/response model. You describe what
//! you want once, and an [`Agent`] runs it against any backend:
//!
//! ```no_run
//! # async fn run() -> Result<(), freyja::Error> {
//! use freyja::{Agent, Client, EndpointPreset};
//!
//! let client = Client::from_env(EndpointPreset::OpenAi)?;
//! let run = Agent::new(client).ask("Hello").await?;
//!
//! println!("{}", run.answer);
//! # Ok(())
//! # }
//! ```
//!
//! [`Client::generate`] and [`Client::stream`] are the low-level layer under `Agent`, for when you want one request and one response with no loop. See <https://rizukirr.github.io/freyja-web/docs/calling-the-model-directly>.
//!
//! # Design rules
//!
//! - **The neutral model never bends to a vendor.** Each provider owns its wire
//!   format and converts in both directions.
//! - **No silent degradation.** A capability a provider cannot express becomes
//!   [`Error::UnsupportedCapability`], not a quietly dropped field.
//! - **No invented defaults.** A `None` field means "the provider decides", so a
//!   request built for one backend stays portable to another.
//!
//! # Tool calling
//!
//! [`tool`] turns a typed function into a type implementing [`Tool`]. Its
//! parameters generate the JSON Schema sent to the model, and [`Tool::call`]
//! deserializes a requested call and serializes the result:
//!
//! ```
//! use freyja::{Context, Tool, tool};
//!
//! #[tool(description = "adds two numbers together", strict = true)]
//! fn add(a: i64, b: i64) -> i64 {
//!     a + b
//! }
//!
//! # tokio::runtime::Runtime::new().unwrap().block_on(async {
//! assert_eq!(add.definition().name, "add");
//! assert_eq!(add.call(r#"{"a":20,"b":22}"#, &Context::new()).await.unwrap(), "42");
//! # });
//! ```
//!
//! An [`Agent`] runs the tool round trip for you. When the model asks for a
//! tool, Freyja calls it, sends the result back, and repeats until the model
//! answers:
//!
//! ```no_run
//! # async fn run(client: freyja::Client) -> Result<(), freyja::Error> {
//! use freyja::{Agent, tool};
//!
//! #[tool(description = "adds two numbers together")]
//! fn add(a: i64, b: i64) -> i64 {
//!     a + b
//! }
//!
//! let agent = Agent::new(client).tool(add);
//! let run = agent.ask("What is 20 + 22?").await?;
//! println!("{}", run.answer);
//! # Ok(())
//! # }
//! ```
//!
//! # Streaming
//!
//! [`Conversation::send_streaming`] delivers the same answer as it arrives.
//! Fragments are not exposed: tool-call arguments and reasoning blobs are
//! assembled internally and surface only once complete, so no caller ever
//! stitches partial JSON.
//!
//! ```no_run
//! # async fn run(client: freyja::Client) -> Result<(), freyja::Error> {
//! use freyja::{Agent, AgentEvent, InMemoryStorage, StreamEvent};
//!
//! let agent = Agent::new(client);
//! let mut chat = agent.conversation(InMemoryStorage::new());
//! let run = chat
//!     .send_streaming("Hello", |event| {
//!         if let AgentEvent::Model(StreamEvent::TextDelta(text)) = event {
//!             print!("{text}");
//!         }
//!     })
//!     .await?;
//! println!("\n{} turns", run.turns);
//! # Ok(())
//! # }
//! ```

#![deny(missing_docs)]

mod agent;
mod builtin;
mod client;
mod conversation;
mod counter;
pub mod dialect;
pub mod endpoint;
pub mod error;
#[cfg(test)]
mod fixtures;
mod helper;
mod hook;
mod memory;
pub mod model;
mod provider;
mod repair;
mod storage;
/// Types and utilities for consuming streaming provider responses.
pub mod stream;
mod summarize;
pub mod testing;
#[cfg(feature = "tracing")]
mod tracing_hook;
mod transport;

pub use agent::{Agent, AgentEvent, Decision, Run, StopReason};
pub use builtin::{HeuristicCounter, InMemoryStorage, JsonlStorage};
pub use client::Client;
pub use conversation::{Compact, Conversation};
pub use counter::TokenCounter;
pub use dialect::Dialect;
pub use endpoint::{Auth, EndpointConfig, EndpointPreset, TokenLimitField};
pub use error::{BODY_IN_MESSAGE, Error, MAX_RETRY_AFTER, TransportError};
pub use freyja_macros::tool;
pub use helper::{window_by_groups, window_by_tokens};
pub use hook::{Flow, Hook};
pub use memory::{Memory, MemoryError, MemoryFuture, State, Summarize, Window};
pub use model::{
    Context, GenerateRequest, GenerateResponse, InputContent, Message, OutputContent,
    ReasoningEffort, ResponseFormat, ResponseStatus, Role, Tool, ToolChoice, ToolDefinition,
    ToolError, ToolFuture, Usage, strict_schema,
};
pub use provider::{Model, ModelFuture};
pub use storage::{Storage, StorageError, StorageFuture};
pub use stream::{EventStream, StreamEvent};
pub use summarize::Summarizer;
#[cfg(feature = "tracing")]
pub use tracing_hook::Tracing;

/// Private dependencies used by generated code.
#[doc(hidden)]
pub mod __private {
    pub use schemars;
    pub use serde;
    pub use serde_json;
}
