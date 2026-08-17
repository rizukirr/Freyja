use serde_json::{Map, Value};

/// Keywords OpenAI strict mode rejects that only constrain a value.
const STRICT_DROPS: [&str; 5] = [
    "uniqueItems",
    "minProperties",
    "maxProperties",
    "dependentRequired",
    "dependentSchemas",
];
/// Keywords whose value is a schema or a list of schemas.
const SCHEMA_KEYS: [&str; 11] = [
    "items",
    "additionalItems",
    "prefixItems",
    "contains",
    "propertyNames",
    "not",
    "if",
    "then",
    "else",
    "anyOf",
    "allOf",
];
/// Keywords whose values map names to schemas.
const SCHEMA_MAP_KEYS: [&str; 4] = ["properties", "patternProperties", "$defs", "definitions"];

/// Rewrites a JSON Schema into the subset OpenAI strict mode accepts.
pub fn strict_schema(schema: Value) -> Value {
    let mut schema = schema;
    strictify(&mut schema);
    schema
}

fn strictify(value: &mut Value) {
    let Some(map) = value.as_object_mut() else {
        return;
    };
    for key in STRICT_DROPS {
        map.remove(key);
    }
    if let Some(variants) = map.remove("oneOf") {
        map.insert("anyOf".into(), variants);
    }
    if describes_an_object(map) {
        map.insert("additionalProperties".into(), Value::Bool(false));
        require_every_property(map);
    }
    for key in SCHEMA_KEYS {
        if let Some(nested) = map.get_mut(key) {
            strictify_each(nested);
        }
    }
    for key in SCHEMA_MAP_KEYS {
        if let Some(Value::Object(members)) = map.get_mut(key) {
            members.values_mut().for_each(strictify);
        }
    }
}

fn strictify_each(value: &mut Value) {
    match value {
        Value::Array(schemas) => schemas.iter_mut().for_each(strictify),
        other => strictify(other),
    }
}

fn describes_an_object(map: &Map<String, Value>) -> bool {
    match map.get("type") {
        Some(Value::String(name)) => name == "object",
        Some(Value::Array(names)) => names.iter().any(|name| name.as_str() == Some("object")),
        _ => false,
    }
}

fn require_every_property(map: &mut Map<String, Value>) {
    let Some(properties) = map.get("properties").and_then(Value::as_object) else {
        return;
    };
    let names: Vec<String> = properties.keys().cloned().collect();
    let already: Vec<&str> = map
        .get("required")
        .and_then(Value::as_array)
        .map(|required| required.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let newly: Vec<String> = names
        .iter()
        .filter(|name| !already.contains(&name.as_str()))
        .cloned()
        .collect();
    map.insert(
        "required".into(),
        Value::Array(names.into_iter().map(Value::String).collect()),
    );
    let Some(properties) = map.get_mut("properties").and_then(Value::as_object_mut) else {
        return;
    };
    for name in newly {
        if let Some(property) = properties.get_mut(&name) {
            make_nullable(property);
        }
    }
}

fn make_nullable(property: &mut Value) {
    let Some(map) = property.as_object_mut() else {
        return;
    };
    match map.get_mut("type") {
        Some(Value::String(name)) => {
            let single = std::mem::take(name);
            map.insert(
                "type".into(),
                Value::Array(vec![Value::String(single), Value::String("null".into())]),
            );
        }
        Some(Value::Array(names)) if !names.iter().any(|name| name.as_str() == Some("null")) => {
            names.push(Value::String("null".into()))
        }
        _ => {}
    }
}

/// The shape a model response must take.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum ResponseFormat {
    /// Free-form text.
    Text,
    /// Any valid JSON object.
    JsonObject,
    /// JSON that conforms to a specific schema.
    JsonSchema {
        /// Schema name reported to the provider.
        name: String,
        /// The JSON Schema.
        schema: Value,
        /// Whether the provider must enforce the schema exactly.
        strict: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::strict_schema;
    #[test]
    fn strict_schema_supplies_what_the_endpoint_demands() {
        let schema = strict_schema(
            serde_json::json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}),
        );
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["required"], serde_json::json!(["name"]));
    }
    #[test]
    fn an_optional_nested_object_is_still_strictified() {
        let schema = strict_schema(
            serde_json::json!({"type":"object","properties":{"inner":{"type":"object","properties":{"a":{"type":"string"}}}},"required":[]}),
        );
        assert_eq!(
            schema["properties"]["inner"]["type"],
            serde_json::json!(["object", "null"])
        );
        assert_eq!(schema["properties"]["inner"]["additionalProperties"], false);
    }
}
