//! A `#[tool]` function in a crate that depends on nothing but freyja.
//!
//! The tool sits in the test module because `#[tool]` makes everything it
//! generates private, so outside tests it would be dead code.

#[cfg(test)]
mod tests {
    use freyja::{Tool, tool};

    #[tool(description = "adds two numbers together")]
    fn add(a: i64, b: i64) -> i64 {
        a + b
    }

    #[test]
    fn the_tool_builds_with_freyja_as_the_only_dependency() {
        assert_eq!(add.name(), "add");
    }
}
