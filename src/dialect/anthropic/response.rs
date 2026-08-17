//! Inbound wire structures for the Anthropic Messages API.

use crate::endpoint::EndpointConfig;
use crate::error::Error;
use crate::model::{GenerateResponse, OutputContent, ResponseStatus, Usage};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
pub(crate) struct Response {
    id: String,
    model: String,
    #[serde(default)]
    stop_reason: Option<String>,
    /// Content blocks stay as raw values so unrecognized ones, `thinking` above
    /// all, can be replayed verbatim on the next request.
    #[serde(default)]
    content: Vec<Value>,
    usage: Option<UsageWire>,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

/// Anthropic reports no total, and `input_tokens` counts only the *uncached*
/// prompt: cached tokens are billed separately and reported in their own
/// fields. The neutral `Usage` therefore sums all three for the true prompt
/// size. The unsummed fields stay available through `provider_metadata`.
#[derive(Deserialize)]
struct UsageWire {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
}

impl From<Response> for GenerateResponse {
    fn from(value: Response) -> Self {
        let content = value.content.into_iter().flat_map(convert_block).collect();

        let usage = value.usage.map(|u| {
            let input_tokens = u.input_tokens.unwrap_or(0)
                + u.cache_creation_input_tokens.unwrap_or(0)
                + u.cache_read_input_tokens.unwrap_or(0);
            let output_tokens = u.output_tokens.unwrap_or(0);
            Usage {
                input_tokens,
                output_tokens,
                total_tokens: input_tokens + output_tokens,
            }
        });

        Self {
            id: value.id,
            model: value.model,
            status: parse_status(value.stop_reason),
            content,
            usage,
            provider_metadata: Some(Value::Object(value.extra)),
        }
    }
}

/// Converts one content block into neutral output parts.
///
/// Anything Freyja does not model becomes [`OutputContent::Reasoning`] rather
/// than being dropped. `thinking` blocks carry a signature the API validates on
/// the next request, and a transcript that omits or rebuilds one is rejected.
fn convert_block(block: Value) -> Vec<OutputContent> {
    match block.get("type").and_then(Value::as_str) {
        Some("text") => block
            .get("text")
            .and_then(Value::as_str)
            .map(|text| vec![OutputContent::Text(text.to_string())])
            .unwrap_or_default(),
        Some("tool_use") => vec![OutputContent::ToolCall {
            id: block
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            name: block
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            // Stringified so tool arguments read the same on every provider.
            arguments: block
                .get("input")
                .map(|input| input.to_string())
                .unwrap_or_else(|| "{}".to_string()),
        }],
        _ => vec![OutputContent::Reasoning { data: block }],
    }
}

/// Maps `stop_reason` onto the neutral status.
///
/// `refusal` and `pause_turn` are preserved rather than flattened: a refusal is
/// a deliberate non-answer rather than a failure, and a paused turn is resumed
/// by re-sending the transcript, not by supplying a tool result.
fn parse_status(stop_reason: Option<String>) -> ResponseStatus {
    match stop_reason.as_deref() {
        Some("end_turn" | "stop_sequence") | None => ResponseStatus::Completed,
        Some("max_tokens") => ResponseStatus::Incomplete,
        Some("tool_use") => ResponseStatus::RequiresAction,
        Some(other) => ResponseStatus::Other(other.to_string()),
    }
}

/// Parses a successful response body, attributing failures to the endpoint.
pub(crate) fn parse(body: &str, config: &EndpointConfig) -> Result<GenerateResponse, Error> {
    let wire: Response = serde_json::from_str(body).map_err(|error| Error::InvalidResponse {
        endpoint: config.name.clone(),
        message: format!("{error}; body: {body}"),
    })?;
    Ok(wire.into())
}
