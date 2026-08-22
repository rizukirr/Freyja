use freyja::{Context, Tool, ToolError, tool};

#[tool(description = "adds two numbers together", strict = true)]
fn add(a: i64, b: i64) -> i64 {
    a + b
}

#[tool(description = "repeats a word")]
fn repeat(word: String, count: usize) -> String {
    word.repeat(count)
}

#[tool(description = "waits, then echoes a word back")]
async fn echo(word: String) -> String {
    tokio::task::yield_now().await;
    word
}

#[tool(description = "divides two numbers")]
fn divide(a: i64, b: i64) -> Result<String, String> {
    match b {
        0 => Err("division by zero".to_string()),
        b => Ok((a / b).to_string()),
    }
}

/// The user id a run carries, read from the context rather than the model.
struct UserId(String);

#[tool(description = "greets the caller by their id")]
fn greet(cx: &Context, greeting: String) -> String {
    match cx.get::<UserId>() {
        Some(id) => format!("{greeting}, {}", id.0),
        None => format!("{greeting}, stranger"),
    }
}

#[tokio::test]
async fn typed_tools_generate_definitions_and_execute_json() {
    assert_eq!(add.name(), "add");
    assert_eq!(repeat.name(), "repeat");

    let definition = add.definition();
    assert_eq!(definition.name, "add");
    assert_eq!(
        definition.description.as_deref(),
        Some("adds two numbers together")
    );
    assert_eq!(definition.strict, Some(true));
    assert_eq!(definition.parameters["type"], "object");
    assert_eq!(definition.parameters["additionalProperties"], false);
    assert!(definition.parameters["properties"].get("a").is_some());
    assert!(definition.parameters["properties"].get("b").is_some());
    assert_eq!(
        definition.parameters["required"],
        serde_json::json!(["a", "b"])
    );

    assert_eq!(repeat.definition().strict, Some(false));
    let cx = Context::new();
    assert_eq!(add.call(r#"{"a":20,"b":22}"#, &cx).await.unwrap(), "42");
    assert_eq!(
        repeat
            .call(r#"{"word":"ha","count":2}"#, &cx)
            .await
            .unwrap(),
        r#""haha""#
    );
}

#[tokio::test]
async fn typed_tools_report_invalid_arguments() {
    assert!(matches!(
        add.call(r#"{"a":"not a number","b":22}"#, &Context::new())
            .await,
        Err(ToolError::Arguments(_))
    ));
}

#[tokio::test]
async fn async_tools_generate_definitions_and_execute_json() {
    assert_eq!(echo.name(), "echo");

    let definition = echo.definition();
    assert_eq!(definition.name, "echo");
    assert_eq!(
        definition.description.as_deref(),
        Some("waits, then echoes a word back")
    );
    assert_eq!(definition.parameters["type"], "object");
    assert!(definition.parameters["properties"].get("word").is_some());

    assert_eq!(
        echo.call(r#"{"word":"ha"}"#, &Context::new())
            .await
            .unwrap(),
        r#""ha""#
    );
}

#[tokio::test]
async fn async_tools_report_invalid_arguments() {
    assert!(matches!(
        echo.call(r#"{"word":42}"#, &Context::new()).await,
        Err(ToolError::Arguments(_))
    ));
}

#[test]
fn tool_execution_futures_are_send() {
    fn assert_send<T: Send>(_: T) {}
    let cx = Context::new();
    assert_send(echo.call(r#"{"word":"ha"}"#, &cx));
    assert_send(add.call(r#"{"a":1,"b":2}"#, &cx));
}

#[tokio::test]
async fn async_tools_run_concurrently() {
    let cx = Context::new();
    let (first, second) = tokio::join!(
        echo.call(r#"{"word":"ha"}"#, &cx),
        echo.call(r#"{"word":"ho"}"#, &cx)
    );
    assert_eq!(first.unwrap(), r#""ha""#);
    assert_eq!(second.unwrap(), r#""ho""#);
}

#[tokio::test]
async fn a_fallible_tool_surfaces_its_error_arm() {
    let cx = Context::new();
    assert_eq!(
        divide.call(r#"{"a":84,"b":2}"#, &cx).await.unwrap(),
        r#""42""#
    );

    let error = divide
        .call(r#"{"a":1,"b":0}"#, &cx)
        .await
        .expect_err("dividing by zero must fail");
    assert!(matches!(error, ToolError::Execution(_)));
    assert_eq!(error.to_string(), "division by zero");
}

#[tokio::test]
async fn a_context_parameter_is_hidden_from_the_model_and_read_at_call_time() {
    let definition = greet.definition();
    assert!(definition.parameters["properties"].get("cx").is_none());
    assert!(
        definition.parameters["properties"]
            .get("greeting")
            .is_some()
    );
    assert_eq!(
        definition.parameters["required"],
        serde_json::json!(["greeting"])
    );

    assert_eq!(
        greet
            .call(r#"{"greeting":"hi"}"#, &Context::new())
            .await
            .unwrap(),
        r#""hi, stranger""#
    );

    let mut cx = Context::new();
    cx.insert(UserId("ada".to_string()));
    assert_eq!(
        greet.call(r#"{"greeting":"hi"}"#, &cx).await.unwrap(),
        r#""hi, ada""#
    );
}
