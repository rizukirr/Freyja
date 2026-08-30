//! Compile-time checks for the supported public import paths.

use freyja::dialect::Dialect as CategorizedDialect;
use freyja::endpoint::{
    EndpointConfig as CategorizedEndpointConfig, EndpointPreset as CategorizedEndpointPreset,
};
use freyja::error::Error as CategorizedError;
use freyja::model::GenerateRequest as CategorizedGenerateRequest;
use freyja::stream::StreamEvent;
use freyja::{Client, Dialect, EndpointConfig, EndpointPreset, Error, GenerateRequest};

#[test]
fn public_types_are_available_from_flat_and_categorized_paths() {
    let _: Option<Client> = None;
    let _: Option<Error> = None;
    let _: Option<CategorizedError> = None;

    let dialect = Dialect::OpenAiResponses;
    let _: CategorizedDialect = dialect;

    let config = EndpointConfig::new(dialect, "test", "https://example.test")
        .path("/custom")
        .query("api-version", "2024-02-01");
    let _: CategorizedEndpointConfig = config;

    let _: Option<(&'static str, &'static str)> = Dialect::Gemini.stream_query();

    let preset = EndpointPreset::OpenAi;
    let _: CategorizedEndpointPreset = preset;

    let request = GenerateRequest::new();
    let _: CategorizedGenerateRequest = request;

    let _ = StreamEvent::TextDelta(String::new());
}
