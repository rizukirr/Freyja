//! The model an agent talks to, as a contract any crate can implement.

use crate::{Client, Error, EventStream, GenerateRequest, GenerateResponse};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// The future returned by every [`Model`] method.
pub type ModelFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'a>>;

/// Something that answers a [`GenerateRequest`].
///
/// [`Client`] is one, for every provider Freyja has a dialect for. A crate
/// can implement it for a provider Freyja does not speak, for a cache or a
/// router wrapped around another model, or for a fake that answers from a
/// script so an agent is tested without a network. [`crate::Agent::new`] and
/// [`crate::Summarizer::new`] take any of them.
///
/// Boxed rather than `async fn` in the trait, for the reason
/// [`crate::ToolFuture`] gives.
pub trait Model: Send + Sync {
    /// A name for this model in errors, such as the endpoint's configured
    /// name.
    fn name(&self) -> &str;

    /// Answers `request` in one response.
    fn generate<'a>(&'a self, request: &'a GenerateRequest) -> ModelFuture<'a, GenerateResponse>;

    /// Answers `request` as a stream of events.
    ///
    /// The default awaits [`Model::generate`] and replays the response with
    /// [`EventStream::from_response`], so a model that cannot stream still
    /// serves a streaming caller, all at once.
    fn stream<'a>(&'a self, request: &'a GenerateRequest) -> ModelFuture<'a, EventStream> {
        Box::pin(async move { Ok(EventStream::from_response(self.generate(request).await?)) })
    }
}

impl Model for Client {
    fn name(&self) -> &str {
        &self.config().name
    }

    fn generate<'a>(&'a self, request: &'a GenerateRequest) -> ModelFuture<'a, GenerateResponse> {
        Box::pin(Client::generate(self, request))
    }

    fn stream<'a>(&'a self, request: &'a GenerateRequest) -> ModelFuture<'a, EventStream> {
        Box::pin(Client::stream(self, request))
    }
}

impl<T: Model + ?Sized> Model for Arc<T> {
    fn name(&self) -> &str {
        (**self).name()
    }
    fn generate<'a>(&'a self, request: &'a GenerateRequest) -> ModelFuture<'a, GenerateResponse> {
        (**self).generate(request)
    }
    fn stream<'a>(&'a self, request: &'a GenerateRequest) -> ModelFuture<'a, EventStream> {
        (**self).stream(request)
    }
}

impl<T: Model + ?Sized> Model for Box<T> {
    fn name(&self) -> &str {
        (**self).name()
    }
    fn generate<'a>(&'a self, request: &'a GenerateRequest) -> ModelFuture<'a, GenerateResponse> {
        (**self).generate(request)
    }
    fn stream<'a>(&'a self, request: &'a GenerateRequest) -> ModelFuture<'a, EventStream> {
        (**self).stream(request)
    }
}
