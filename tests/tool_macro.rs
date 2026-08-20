use freyja::{Tool, ToolError, tool};

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

#[tokio::test]
async fn typed_tools_generate_definitions_and_execute_json() {
    let tools = [add, repeat];
    assert_eq!(tools.map(Tool::name), ["add", "repeat"]);

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
    assert_eq!(add.execute(r#"{"a":20,"b":22}"#).await.unwrap(), "42");
    assert_eq!(
        repeat.execute(r#"{"word":"ha","count":2}"#).await.unwrap(),
        r#""haha""#
    );
}

#[tokio::test]
async fn typed_tools_report_invalid_arguments() {
    assert!(matches!(
        add.execute(r#"{"a":"not a number","b":22}"#).await,
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

    assert_eq!(echo.execute(r#"{"word":"ha"}"#).await.unwrap(), r#""ha""#);
}

#[tokio::test]
async fn async_tools_report_invalid_arguments() {
    assert!(matches!(
        echo.execute(r#"{"word":42}"#).await,
        Err(ToolError::Arguments(_))
    ));
}

#[test]
fn tool_execution_futures_are_send() {
    fn assert_send<T: Send>(_: T) {}
    assert_send(echo.execute(r#"{"word":"ha"}"#));
    assert_send(add.execute(r#"{"a":1,"b":2}"#));
}

#[tokio::test]
async fn async_tools_run_concurrently() {
    let (first, second) = tokio::join!(
        echo.execute(r#"{"word":"ha"}"#),
        echo.execute(r#"{"word":"ho"}"#)
    );
    assert_eq!(first.unwrap(), r#""ha""#);
    assert_eq!(second.unwrap(), r#""ho""#);
}
