//! OpenAI Chat Completions backend. Transport lives in
//! [`crate::Client`]; this module owns only the wire format.

mod request;
mod response;
mod stream;

pub(crate) use stream::Decoder;

use crate::dialect::WireDialect;
use crate::endpoint::EndpointConfig;
use crate::error::Error;
use crate::model::{GenerateRequest, GenerateResponse};

pub(crate) struct OpenAiChatProvider;

impl WireDialect for OpenAiChatProvider {
    type Request = request::Request;

    fn build(
        &self,
        request: &GenerateRequest,
        config: &EndpointConfig,
    ) -> Result<Self::Request, Error> {
        request::Request::build(request, config)
    }

    fn parse(&self, body: &str, config: &EndpointConfig) -> Result<GenerateResponse, Error> {
        response::parse(body, config)
    }
}
