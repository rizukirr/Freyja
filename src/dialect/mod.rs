//! Wire dialect selection and request/response conversion.

pub(crate) mod anthropic;
pub(crate) mod gemini;
pub(crate) mod openai_chat;
pub(crate) mod openai_responses;

mod refusal;

#[cfg(test)]
pub(crate) mod sse {
    pub(crate) use crate::stream::SseFrame;
}

#[cfg(test)]
pub(crate) mod stream {
    pub(crate) use crate::stream::drain_for_test;
    pub(crate) use crate::stream::{RawDelta, StreamDecoder};
}

use crate::endpoint::{Auth, EndpointConfig};
use crate::error::Error;
use crate::model::{GenerateRequest, GenerateResponse};
use serde::Serialize;

/// A wire format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// OpenAI's Responses API.
    OpenAiResponses,
    /// OpenAI's Chat Completions API.
    OpenAiChat,
    /// Google's Gemini Interactions API.
    Gemini,
    /// Anthropic's Messages API.
    Anthropic,
}

impl Dialect {
    /// The path appended to an endpoint base URL.
    pub fn path(self) -> &'static str {
        match self {
            Self::OpenAiResponses => "/responses",
            Self::OpenAiChat => "/chat/completions",
            Self::Gemini => "/interactions",
            Self::Anthropic => "/messages",
        }
    }

    /// The conventional authentication method for this dialect.
    pub fn default_auth(self) -> Auth {
        match self {
            Self::OpenAiResponses | Self::OpenAiChat => Auth::Bearer,
            Self::Gemini => Auth::Header("x-goog-api-key"),
            Self::Anthropic => Auth::Header("x-api-key"),
        }
    }

    /// Headers required by the dialect regardless of endpoint.
    pub fn required_headers(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::OpenAiResponses | Self::OpenAiChat => &[],
            Self::Gemini => &[("Api-Revision", "2026-05-20")],
            Self::Anthropic => &[("anthropic-version", "2023-06-01")],
        }
    }

    /// The query parameter that enables SSE where one is required.
    ///
    /// A pair rather than a preformatted `alt=sse`, so the caller appends it
    /// like any other parameter and nothing in the crate decides between `?`
    /// and `&`.
    pub fn stream_query(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Gemini => Some(("alt", "sse")),
            Self::OpenAiResponses | Self::OpenAiChat | Self::Anthropic => None,
        }
    }
}

/// Internal implementation of a wire dialect.
pub(crate) trait WireDialect: Send + Sync {
    type Request: Serialize + Send;

    fn build(
        &self,
        request: &GenerateRequest,
        config: &EndpointConfig,
    ) -> Result<Self::Request, Error>;

    fn parse(&self, body: &str, config: &EndpointConfig) -> Result<GenerateResponse, Error>;
}

macro_rules! with_dialect {
    ($dialect:expr, |$provider:ident| $body:expr) => {
        match $dialect {
            $crate::dialect::Dialect::OpenAiResponses => {
                let $provider = $crate::dialect::openai_responses::OpenAiResponsesProvider;
                $body
            }
            $crate::dialect::Dialect::OpenAiChat => {
                let $provider = $crate::dialect::openai_chat::OpenAiChatProvider;
                $body
            }
            $crate::dialect::Dialect::Gemini => {
                let $provider = $crate::dialect::gemini::GeminiProvider;
                $body
            }
            $crate::dialect::Dialect::Anthropic => {
                let $provider = $crate::dialect::anthropic::AnthropicProvider;
                $body
            }
        }
    };
}

pub(crate) use with_dialect;

pub(crate) fn decoder_for(dialect: Dialect) -> Box<dyn crate::stream::StreamDecoder> {
    match dialect {
        Dialect::OpenAiResponses => Box::new(openai_responses::Decoder),
        Dialect::OpenAiChat => Box::new(openai_chat::Decoder),
        Dialect::Gemini => Box::new(gemini::Decoder::default()),
        Dialect::Anthropic => Box::new(anthropic::Decoder::default()),
    }
}
