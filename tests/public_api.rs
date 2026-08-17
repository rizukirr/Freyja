//! Compile-time checks for the supported public import styles.

use freyja::dialect::Dialect as CategorizedDialect;
use freyja::endpoint::{
    EndpointConfig as CategorizedEndpointConfig, EndpointPreset as CategorizedEndpointPreset,
};
use freyja::error::Error as CategorizedError;
use freyja::model::GenerateRequest as CategorizedGenerateRequest;
use freyja::stream::StreamEvent;
use freyja::{Client, Dialect, EndpointConfig, EndpointPreset, Error, GenerateRequest};

fn accepts_error(_: Error) {}
fn accepts_event(_: StreamEvent) {}

#[test]
fn root_and_categorized_public_paths_are_available() {
    let _: Option<Client> = None;
    let dialect: CategorizedDialect = Dialect::OpenAiResponses;
    let _: CategorizedEndpointConfig =
        EndpointConfig::new(dialect, "public-api-test", "https://api.test/v1");
    let _: CategorizedEndpointPreset = EndpointPreset::OpenAi;
    let _: CategorizedGenerateRequest = GenerateRequest::new();
    let _: fn(CategorizedError) = accepts_error;
    let _: fn(StreamEvent) = accepts_event;
}
