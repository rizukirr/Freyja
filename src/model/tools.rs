use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;

/// The future returned by an async tool.
pub type ToolFuture = Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'static>>;

/// How a [`Tool`] runs: directly, or by awaiting a future.
#[derive(Clone, Copy)]
enum Executor {
    Sync(fn(&str) -> Result<String, ToolError>),
    Async(fn(&str) -> ToolFuture),
}

/// A callable tool descriptor.
#[derive(Clone, Copy)]
pub struct Tool {
    name: &'static str,
    definition: fn() -> ToolDefinition,
    execute: Executor,
}

impl Tool {
    /// Creates a tool descriptor from its name, definition factory, and synchronous executor.
    pub const fn new(
        name: &'static str,
        definition: fn() -> ToolDefinition,
        execute: fn(&str) -> Result<String, ToolError>,
    ) -> Self {
        Self {
            name,
            definition,
            execute: Executor::Sync(execute),
        }
    }

    /// Creates a tool descriptor whose executor returns a future.
    ///
    /// The future must be `Send` so callers can drive several tool calls at
    /// once. A tool body may not hold a non-`Send` value across an `.await`.
    pub const fn new_async(
        name: &'static str,
        definition: fn() -> ToolDefinition,
        execute: fn(&str) -> ToolFuture,
    ) -> Self {
        Self {
            name,
            definition,
            execute: Executor::Async(execute),
        }
    }

    /// Returns the tool name.
    pub const fn name(self) -> &'static str {
        self.name
    }

    /// Builds the provider-facing definition for this tool.
    pub fn definition(self) -> ToolDefinition {
        (self.definition)()
    }

    /// Executes this tool with JSON arguments.
    ///
    /// A synchronous tool completes without yielding; an async tool awaits its
    /// future. Dropping the returned future before it completes cancels the
    /// tool.
    pub async fn execute(self, arguments: &str) -> Result<String, ToolError> {
        match self.execute {
            Executor::Sync(execute) => execute(arguments),
            Executor::Async(execute) => execute(arguments).await,
        }
    }
}

/// An error encountered while running a [`Tool`].
#[derive(Debug)]
pub enum ToolError {
    /// Tool arguments could not be deserialized from JSON.
    Arguments(serde_json::Error),
    /// The tool reported an execution failure.
    Execution(String),
    /// The tool result could not be serialized as JSON.
    Result(serde_json::Error),
}

impl fmt::Display for ToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Arguments(error) => write!(formatter, "invalid arguments: {error}"),
            Self::Execution(message) => write!(formatter, "{message}"),
            Self::Result(error) => write!(formatter, "result could not be serialized: {error}"),
        }
    }
}

impl std::error::Error for ToolError {}

/// Per-run data handed to every tool call.
///
/// Keyed by type, so two values of the same type collide: give distinct values
/// distinct newtypes, as `http::Extensions` requires.
///
/// State known when a tool is *built* — a database handle, an HTTP client, a
/// rate limiter — belongs in the tool's own fields instead. `Context` is for
/// state that is not known until the call arrives: a user id, a request id, a
/// cancellation token, a tracing span.
#[derive(Default)]
pub struct Context {
    map: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl Context {
    /// Creates an empty context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores a value, replacing any previous value of the same type.
    pub fn insert<T: Any + Send + Sync>(&mut self, value: T) -> &mut Self {
        self.map.insert(TypeId::of::<T>(), Box::new(value));
        self
    }

    /// Borrows a stored value, if one of this type was inserted.
    pub fn get<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.map
            .get(&TypeId::of::<T>())
            .and_then(|value| value.downcast_ref::<T>())
    }

    /// Borrows a stored value, or fails with a message naming the missing type.
    pub fn require<T: Any + Send + Sync>(&self) -> Result<&T, ToolError> {
        self.get::<T>().ok_or_else(|| {
            ToolError::Execution(format!(
                "context is missing a value of type {}",
                core::any::type_name::<T>()
            ))
        })
    }
}

impl fmt::Debug for Context {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Context")
            .field("len", &self.map.len())
            .finish()
    }
}

/// A function the model is allowed to call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDefinition {
    /// Tool name.
    pub name: String,
    /// What the tool does.
    pub description: Option<String>,
    /// JSON Schema for tool arguments.
    pub parameters: Value,
    /// Whether the endpoint must enforce `parameters` exactly.
    pub strict: Option<bool>,
}

impl ToolDefinition {
    /// Creates a tool definition with no parameter schema.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: Some(description.into()),
            parameters: Value::Null,
            strict: None,
        }
    }
    /// Sets the argument JSON Schema.
    pub fn parameters(mut self, parameters: Value) -> Self {
        self.parameters = parameters;
        self
    }
    /// Requests strict schema enforcement.
    pub fn strict(mut self, strict: bool) -> Self {
        self.strict = Some(strict);
        self
    }
}

/// Whether the model must call a tool, and which one.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolChoice {
    /// The model decides.
    Auto,
    /// The model may not call tools.
    None,
    /// The model must call a tool.
    Required,
    /// The model must call the named tool.
    Named(String),
}

#[cfg(test)]
mod tests {
    use super::{Context, Tool, ToolDefinition, ToolError, ToolFuture};

    fn definition() -> ToolDefinition {
        ToolDefinition::new("add", "adds two numbers")
    }

    fn execute(arguments: &str) -> Result<String, ToolError> {
        let arguments: serde_json::Value =
            serde_json::from_str(arguments).map_err(ToolError::Arguments)?;
        let a = arguments["a"].as_i64().unwrap();
        let b = arguments["b"].as_i64().unwrap();
        Ok((a + b).to_string())
    }

    fn execute_async(arguments: &str) -> ToolFuture {
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(arguments);
        Box::pin(async move {
            let arguments = parsed.map_err(ToolError::Arguments)?;
            let a = arguments["a"].as_i64().unwrap();
            let b = arguments["b"].as_i64().unwrap();
            Ok((a + b).to_string())
        })
    }

    #[tokio::test]
    async fn callable_tool_delegates_definition_and_execution() {
        let tool = Tool::new("add", definition, execute);
        assert_eq!(tool.name(), "add");
        assert_eq!(tool.definition().name, "add");
        assert_eq!(tool.execute(r#"{"a":20,"b":22}"#).await.unwrap(), "42");
    }

    #[tokio::test]
    async fn hand_written_async_tool_awaits_its_future() {
        let tool = Tool::new_async("add", definition, execute_async);
        assert_eq!(tool.name(), "add");
        assert_eq!(tool.definition().name, "add");
        assert_eq!(tool.execute(r#"{"a":20,"b":22}"#).await.unwrap(), "42");
    }

    #[tokio::test]
    async fn async_tool_reports_invalid_arguments() {
        let tool = Tool::new_async("add", definition, execute_async);
        assert!(matches!(
            tool.execute("not json").await,
            Err(ToolError::Arguments(_))
        ));
    }

    #[test]
    fn context_round_trips_a_value_by_type() {
        struct UserId(String);
        let mut context = Context::new();
        context.insert(UserId("u-1".to_string()));
        assert_eq!(context.get::<UserId>().map(|id| id.0.as_str()), Some("u-1"));
    }

    #[test]
    fn context_insert_replaces_the_same_type() {
        struct UserId(String);
        let mut context = Context::new();
        context.insert(UserId("first".to_string()));
        context.insert(UserId("second".to_string()));
        assert_eq!(
            context.get::<UserId>().map(|id| id.0.as_str()),
            Some("second")
        );
    }

    #[test]
    fn require_names_the_missing_type() {
        struct Missing;
        let error = Context::new()
            .require::<Missing>()
            .err()
            .expect("an absent type must fail");
        assert!(error.to_string().contains("Missing"));
    }

    #[test]
    fn tool_error_display_is_not_debug_syntax() {
        let error = ToolError::Execution("no such user".to_string());
        assert_eq!(error.to_string(), "no such user");
    }
}
