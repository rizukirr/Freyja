//! Freya — a multi-LLM agent orchestration framework written from scratch in Rust.
//!
//! Freya's core is a provider-neutral request/response model. You describe what
//! you want once, and any backend can serve it:
//!
//! ```no_run
//! # async fn run() -> Result<(), freya::ProviderError> {
//! use freya::{Client, GenerateRequest, Message, ProviderType, Role};
//!
//! let client = Client::from_env(ProviderType::OpenAi).expect("OPENAI_API_KEY");
//! let response = client
//!     .generate(&GenerateRequest::new().message(Message::text(Role::User, "Hello")))
//!     .await?;
//!
//! println!("{}", response.output_text());
//! # Ok(())
//! # }
//! ```
//!
//! # Design rules
//!
//! - **The neutral model never bends to a vendor.** Each provider owns its wire
//!   format and converts in both directions.
//! - **No silent degradation.** A capability a provider cannot express becomes
//!   [`ProviderError::UnsupportedCapability`], not a quietly dropped field.
//! - **No invented defaults.** A `None` field means "the provider decides", so a
//!   request built for one backend stays portable to another.
//!
//! # Tool calling
//!
//! A tool round trip is three turns: the user asks, the model answers with a
//! [`OutputContent::ToolCall`], and you feed the result back with
//! [`Message::tool_result`]. [`GenerateResponse::to_message`] converts the
//! model's answer into the assistant turn that must precede the result.
//!
//! ```no_run
//! # async fn run(client: freya::Client, request: freya::GenerateRequest) -> Result<(), freya::ProviderError> {
//! use freya::Message;
//!
//! let mut request = request;
//! let response = client.generate(&request).await?;
//!
//! if response.has_tool_calls() {
//!     request = request.message(response.to_message());
//!     for (id, name, arguments) in response.tool_calls() {
//!         let output = format!("ran {name} with {arguments}");
//!         request = request.message(Message::tool_result(id, output));
//!     }
//!     let final_response = client.generate(&request).await?;
//!     println!("{}", final_response.output_text());
//! }
//! # Ok(())
//! # }
//! ```

#![deny(missing_docs)]

pub mod provider;

pub use provider::{
    Client, GenerateRequest, GenerateResponse, InputContent, Message, OutputContent, Provider,
    ProviderError, ProviderType, ReasoningEffort, ResponseFormat, ResponseStatus, Role, ToolChoice,
    ToolDefinition, Usage,
};
