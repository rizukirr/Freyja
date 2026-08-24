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
///
/// Composition is how behaviour Freyja does not own gets added. A budget, for
/// instance — including for a tool you did not write, since `Arc<dyn Tool>`
/// wraps as readily as a concrete type:
///
/// ```
/// use freyja::{Context, Tool, ToolDefinition, ToolError, ToolFuture};
/// use std::sync::Arc;
/// use std::time::Duration;
///
/// struct Timeout<T: ?Sized> {
///     inner: Arc<T>,
///     budget: Duration,
/// }
///
/// impl<T: Tool + ?Sized> Tool for Timeout<T> {
///     fn name(&self) -> &str {
///         self.inner.name()
///     }
///
///     fn definition(&self) -> ToolDefinition {
///         self.inner.definition()
///     }
///
///     fn call<'a>(&'a self, arguments: &'a str, cx: &'a Context) -> ToolFuture<'a> {
///         Box::pin(async move {
///             tokio::time::timeout(self.budget, self.inner.call(arguments, cx))
///                 .await
///                 .unwrap_or_else(|_| {
///                     Err(ToolError::Execution(format!(
///                         "timed out after {:?}",
///                         self.budget
///                     )))
///                 })
///         })
///     }
/// }
///
/// # struct Slow;
/// # impl Tool for Slow {
/// #     fn name(&self) -> &str { "slow" }
/// #     fn definition(&self) -> ToolDefinition { ToolDefinition::new("slow", "slow") }
/// #     fn call<'a>(&'a self, _a: &'a str, _c: &'a Context) -> ToolFuture<'a> {
/// #         Box::pin(async { Ok("done".to_string()) })
/// #     }
/// # }
/// # fn assert_tool<T: Tool>(_t: &T) {}
/// # let erased: Arc<dyn Tool> = Arc::new(Slow);
/// # assert_tool(&Timeout { inner: erased, budget: Duration::from_secs(5) });
/// # assert_tool(&Timeout { inner: Arc::new(Slow), budget: Duration::from_secs(5) });
/// ```
///
/// A timed-out call reaches the model as `error: timed out after …`, the same
/// channel every other tool failure uses, so it can react rather than the run
/// ending. Dropping the inner future cancels it — but only work that is
/// actually awaiting. A tool that blocks without awaiting starves its siblings
/// under [`crate::Agent`]'s concurrent dispatch and no timer fires.
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
    ///
    /// Every dialect sends this to the wire, and none of them accepts `null`
    /// there, so a tool taking no arguments needs the empty object schema
    /// rather than nothing. [`ToolDefinition::new`] starts you there, and
    /// [`ToolDefinition::schema`] substitutes it for anything that is not a
    /// JSON object, so setting this field by hand cannot produce a body the
    /// provider rejects.
    pub parameters: Value,
    /// Whether the endpoint must enforce `parameters` exactly.
    pub strict: Option<bool>,
}

/// The schema for a tool that takes no arguments.
///
/// Not `Value::Null`: OpenAI answers `expected an object, but got null`, and
/// Anthropic requires `input_schema` to be an object. A tool with no arguments
/// still has a shape, and this is it.
fn empty_schema() -> Value {
    serde_json::json!({"type": "object", "properties": {}})
}

impl ToolDefinition {
    /// Creates a tool definition whose tool takes no arguments.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: Some(description.into()),
            parameters: empty_schema(),
            strict: None,
        }
    }

    /// The schema as it goes to the wire.
    ///
    /// Substitutes the empty object schema for anything that is not a JSON
    /// object, since [`ToolDefinition::parameters`] is public and no dialect
    /// accepts `null` in that position.
    pub(crate) fn schema(&self) -> Value {
        match &self.parameters {
            object @ Value::Object(_) => object.clone(),
            _ => empty_schema(),
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

    #[test]
    fn a_tool_taking_no_arguments_still_has_a_schema() {
        // `null` here reached the wire on all four dialects and every one of
        // them answers 400. A tool with no arguments has a shape, and the
        // no-parameters constructor has to produce it.
        let definition = ToolDefinition::new("now", "the current time");
        assert_eq!(definition.parameters["type"], "object");
        assert!(definition.parameters["properties"].is_object());
    }

    #[test]
    fn a_schema_set_by_hand_is_only_substituted_when_unusable() {
        use serde_json::{Value, json};

        let mine = json!({"type": "object", "properties": {"a": {"type": "integer"}}});
        let kept = ToolDefinition::new("add", "adds").parameters(mine.clone());
        assert_eq!(kept.schema(), mine);

        // The field is public, so it can still be emptied out.
        let mut cleared = ToolDefinition::new("add", "adds");
        cleared.parameters = Value::Null;
        assert_eq!(cleared.schema()["type"], "object");
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
