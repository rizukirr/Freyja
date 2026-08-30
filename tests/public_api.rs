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
        .query("api-version", "2024-02-01")
        .secret_query("sig", "signature")
        .secret_header("x-acme-passport", "passport");
    let _: usize = config.secrets.len();
    let _: CategorizedEndpointConfig = config;

    let _: Option<(&'static str, &'static str)> = Dialect::Gemini.stream_query();

    // This file is a separate crate, so it sees `Auth` the way a downstream
    // user does. Every variant is listed *and* a wildcard follows, which
    // compiles only while `Auth` is non_exhaustive: drop the attribute and the
    // wildcard becomes unreachable, which the deny below turns into a build
    // failure. So this asserts the attribute rather than restating it.
    let auth = freyja::Auth::Query("key");
    let _: freyja::endpoint::Auth = auth.clone();
    #[deny(unreachable_patterns)]
    let named = match auth {
        freyja::Auth::Bearer => "bearer",
        freyja::Auth::Header(name) => name,
        freyja::Auth::Query(name) => name,
        freyja::Auth::None => "none",
        _ => "a variant added since this was written",
    };
    assert_eq!(named, "key");

    let preset = EndpointPreset::OpenAi;
    let _: CategorizedEndpointPreset = preset;

    let request = GenerateRequest::new();
    let _: CategorizedGenerateRequest = request;

    let _ = StreamEvent::TextDelta(String::new());
}
