use freyja::{Tool, ToolError, tool};

#[tool(description = "adds two numbers together", strict = true)]
fn add(a: i64, b: i64) -> i64 {
    a + b
}

#[tool(description = "repeats a word")]
fn repeat(word: String, count: usize) -> String {
    word.repeat(count)
}

#[test]
fn typed_tools_generate_definitions_and_execute_json() {
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
    assert_eq!(add.execute(r#"{"a":20,"b":22}"#).unwrap(), "42");
    assert_eq!(
        repeat.execute(r#"{"word":"ha","count":2}"#).unwrap(),
        r#""haha""#
    );
}

#[test]
fn typed_tools_report_invalid_arguments() {
    assert!(matches!(
        add.execute(r#"{"a":"not a number","b":22}"#),
        Err(ToolError::Arguments(_))
    ));
}
