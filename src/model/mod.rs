//! Provider-neutral request and response types.

mod message;
mod request;
mod response;
mod schema;
mod tools;

pub use message::{InputContent, Message, ReasoningEffort, Role};
pub use request::GenerateRequest;
pub use response::{GenerateResponse, OutputContent, ResponseStatus, Usage};
pub use schema::{ResponseFormat, strict_schema};
pub use tools::{Tool, ToolChoice, ToolDefinition, ToolError};

pub(crate) use request::merge_into;
