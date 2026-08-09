//! OpenAI Chat Completions backend. Transport lives in
//! [`crate::provider::Client`]; this module owns only the wire format.

mod types;

use crate::provider::{GenerateRequest, GenerateResponse, Provider, ProviderConfig, ProviderError};

pub(crate) struct OpenAiChatProvider;

impl Provider for OpenAiChatProvider {
    type Request = types::Request;

    fn build(
        &self,
        request: &GenerateRequest,
        config: &ProviderConfig,
    ) -> Result<Self::Request, ProviderError> {
        types::Request::build(request, config)
    }

    fn parse(
        &self,
        body: &str,
        config: &ProviderConfig,
    ) -> Result<GenerateResponse, ProviderError> {
        types::parse(body, config)
    }
}

/// Decodes this dialect's SSE frames. Filled in by its own task.
#[derive(Default)]
pub(crate) struct Decoder;

impl crate::provider::stream::StreamDecoder for Decoder {
    fn decode(
        &mut self,
        _frame: &crate::provider::sse::SseFrame,
        _out: &mut Vec<crate::provider::stream::RawDelta>,
    ) -> Result<(), crate::provider::ProviderError> {
        Ok(())
    }
}
