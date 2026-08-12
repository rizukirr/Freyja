//! A provider-neutral LLM client: one request model, four wire dialects, any
//! compatible endpoint.
//!
//! Built toward agent orchestration, see the roadmap in the repository README.
//! Today it is the client layer and the tool-calling round trip that layer needs.
//!
//! Freyja's core is a provider-neutral request/response model. You describe what
//! you want once, and any backend can serve it:
//!
//! ```no_run
//! # async fn run() -> Result<(), freyja::ProviderError> {
//! use freyja::{Client, GenerateRequest, Message, ProviderType, Role};
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
//! # async fn run(client: freyja::Client, request: freyja::GenerateRequest) -> Result<(), freyja::ProviderError> {
//! use freyja::Message;
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
//!
//! # Streaming
//!
//! [`Client::stream`] returns the same answer incrementally. Fragments are not
//! exposed: tool-call arguments and reasoning blobs are assembled internally and
//! surface only once complete, so no caller ever stitches partial JSON.
//!
//! ```no_run
//! # async fn run(client: freyja::Client, request: freyja::GenerateRequest)
//! #     -> Result<(), freyja::ProviderError> {
//! use freyja::StreamEvent;
//!
//! let mut stream = client.stream(&request).await?;
//! while let Some(event) = stream.next().await? {
//!     match event {
//!         StreamEvent::TextDelta(text) => print!("{text}"),
//!         StreamEvent::ToolCall { name, arguments, .. } => {
//!             println!("\ncalling {name} with {arguments}");
//!         }
//!         _ => {}
//!     }
//! }
//!
//! // Drained streams convert back to the non-streaming response, so a tool
//! // loop can reuse `to_message` unchanged.
//! let response = stream.into_response()?;
//! # Ok(())
//! # }
//! ```

#![deny(missing_docs)]

pub mod provider;

pub use provider::{
    Auth, Client, EventStream, GenerateRequest, GenerateResponse, InputContent, Message,
    OutputContent, Provider, ProviderConfig, ProviderDialect, ProviderError, ProviderType,
    ReasoningEffort, ResponseFormat, ResponseStatus, Role, StreamEvent, TokenLimitField,
    ToolChoice, ToolDefinition, TransportError, Usage, strict_schema,
};
