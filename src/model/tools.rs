use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;

/// The future returned by a tool call.
///
/// Borrows the tool, its arguments and the run context, so a spawned task must
/// own all three — clone the `Arc<dyn Tool>` and the argument string first.
pub type ToolFuture<'a> = Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>>;

/// A function the model may call.
///
/// State known when the tool is built goes in the implementing struct's fields.
/// State that arrives with the run goes in [`Context`].
///
/// Boxed rather than `async fn` in the trait: `async fn` in traits is stable but
/// not `dyn`-compatible, and [`crate::Agent`] stores trait objects. The future is
/// `Send` so several calls can be driven at once.
pub trait Tool: Send + Sync {
    /// The name the model calls this tool by.
    fn name(&self) -> &str;

    /// Builds the provider-facing definition.
    ///
    /// Called once when the tool is registered, never per run, so it may not
    /// depend on per-run data.
    fn definition(&self) -> ToolDefinition;

    /// Runs the tool against the model's JSON arguments.
    fn call<'a>(&'a self, arguments: &'a str, cx: &'a Context) -> ToolFuture<'a>;
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
    pub fn insert<T: Any + Send + Sync>(&mut self, value: T) {
        self.map.insert(TypeId::of::<T>(), Box::new(value));
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

    /// A hand-written tool holding per-agent state, which the old function
    /// pointer could not capture.
    struct Add {
        offset: i64,
    }

    impl Tool for Add {
        fn name(&self) -> &str {
            "add"
        }

        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new("add", "adds two numbers")
        }

        fn call<'a>(&'a self, arguments: &'a str, _cx: &'a Context) -> ToolFuture<'a> {
            let parsed: Result<serde_json::Value, _> = serde_json::from_str(arguments);
            Box::pin(async move {
                let arguments = parsed.map_err(ToolError::Arguments)?;
                let a = arguments["a"].as_i64().unwrap();
                let b = arguments["b"].as_i64().unwrap();
                Ok((a + b + self.offset).to_string())
            })
        }
    }

    #[tokio::test]
    async fn a_tool_reports_its_name_and_definition() {
        let tool = Add { offset: 0 };
        assert_eq!(tool.name(), "add");
        assert_eq!(tool.definition().name, "add");
    }

    #[tokio::test]
    async fn a_tool_reads_its_own_state_on_every_call() {
        let tool = Add { offset: 100 };
        let cx = Context::new();
        assert_eq!(tool.call(r#"{"a":20,"b":22}"#, &cx).await.unwrap(), "142");
        assert_eq!(tool.call(r#"{"a":1,"b":1}"#, &cx).await.unwrap(), "102");
    }

    #[tokio::test]
    async fn a_tool_reports_invalid_arguments() {
        let tool = Add { offset: 0 };
        assert!(matches!(
            tool.call("not json", &Context::new()).await,
            Err(ToolError::Arguments(_))
        ));
    }

    #[test]
    fn a_trait_object_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync + ?Sized>() {}
        assert_send_sync::<dyn Tool>();
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
