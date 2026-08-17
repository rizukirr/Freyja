//! Errors returned while preparing, sending, or decoding a generation request.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

/// Why a request never reached the endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TransportError {
    /// No reply arrived before the inactivity timeout.
    Timeout,
    /// The connection could not be established.
    Connect,
    /// The connection ended while the body was read.
    Body,
    /// An unclassified transport failure.
    Other,
}

impl TransportError {
    pub(crate) fn classify(error: &reqwest::Error) -> Self {
        if error.is_timeout() {
            Self::Timeout
        } else if error.is_connect() {
            Self::Connect
        } else if error.is_body() {
            Self::Body
        } else {
            Self::Other
        }
    }
    const fn is_retryable(self) -> bool {
        matches!(self, Self::Timeout | Self::Body)
    }
    const fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timed out",
            Self::Connect => "could not be reached",
            Self::Body => "dropped the connection",
            Self::Other => "transport failed",
        }
    }
}

/// Everything that can go wrong on the way to a generation response.
#[derive(Debug)]
#[non_exhaustive]
#[allow(missing_docs)]
pub enum Error {
    /// The endpoint cannot express the requested capability.
    UnsupportedCapability {
        endpoint: Arc<str>,
        capability: &'static str,
    },
    /// The request was malformed before it left the process.
    InvalidRequest { endpoint: Arc<str>, message: String },
    /// The HTTP request never completed.
    Http {
        endpoint: Arc<str>,
        kind: TransportError,
        message: String,
    },
    /// The endpoint rejected the request body.
    BadRequest { endpoint: Arc<str>, body: String },
    /// The endpoint rejected the credential.
    Unauthorized {
        endpoint: Arc<str>,
        status: u16,
        body: String,
    },
    /// No such model or endpoint exists.
    NotFound { endpoint: Arc<str>, body: String },
    /// The endpoint rate limited the request.
    RateLimit {
        endpoint: Arc<str>,
        retry_after: Option<Duration>,
        body: String,
    },
    /// The account is out of credit or past a hard quota.
    QuotaExceeded {
        endpoint: Arc<str>,
        status: u16,
        body: String,
    },
    /// The endpoint failed with a server-side error.
    ServerError {
        endpoint: Arc<str>,
        status: u16,
        body: String,
    },
    /// An unclassified non-success HTTP status.
    Api {
        endpoint: Arc<str>,
        status: u16,
        body: String,
    },
    /// A successful response body could not be parsed.
    InvalidResponse { endpoint: Arc<str>, message: String },
    /// Model output did not match the requested type.
    OutputMismatch {
        endpoint: Arc<str>,
        message: String,
        text: String,
        truncated: bool,
    },
    /// A stream failed after it began successfully.
    Stream { endpoint: Arc<str>, message: String },
}

const QUOTA_MARKER: &str = "insufficient_quota";

impl Error {
    pub(crate) fn from_status(
        endpoint: Arc<str>,
        status: u16,
        retry_after: Option<Duration>,
        body: String,
    ) -> Self {
        match status {
            400 => Self::BadRequest { endpoint, body },
            401 | 403 => Self::Unauthorized {
                endpoint,
                status,
                body,
            },
            404 => Self::NotFound { endpoint, body },
            429 if body.contains(QUOTA_MARKER) => Self::QuotaExceeded {
                endpoint,
                status,
                body,
            },
            429 => Self::RateLimit {
                endpoint,
                retry_after,
                body,
            },
            500..=599 => Self::ServerError {
                endpoint,
                status,
                body,
            },
            _ => Self::Api {
                endpoint,
                status,
                body,
            },
        }
    }
    pub(crate) fn transport(endpoint: Arc<str>, error: &reqwest::Error) -> Self {
        Self::Http {
            endpoint,
            kind: TransportError::classify(error),
            message: error.to_string(),
        }
    }
    /// Whether repeating the identical request could plausibly succeed.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Http { kind, .. } => kind.is_retryable(),
            Self::RateLimit { .. } | Self::ServerError { .. } => true,
            Self::Api { status, .. } => *status >= 500,
            _ => false,
        }
    }
    /// How long the endpoint asked the caller to wait, if it said.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimit { retry_after, .. } => *retry_after,
            _ => None,
        }
    }
    /// The HTTP status, if the error has one.
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::BadRequest { .. } => Some(400),
            Self::NotFound { .. } => Some(404),
            Self::RateLimit { .. } => Some(429),
            Self::Unauthorized { status, .. }
            | Self::QuotaExceeded { status, .. }
            | Self::ServerError { status, .. }
            | Self::Api { status, .. } => Some(*status),
            _ => None,
        }
    }
    /// The configured endpoint name.
    pub fn endpoint(&self) -> &str {
        match self {
            Self::UnsupportedCapability { endpoint, .. }
            | Self::InvalidRequest { endpoint, .. }
            | Self::Http { endpoint, .. }
            | Self::BadRequest { endpoint, .. }
            | Self::Unauthorized { endpoint, .. }
            | Self::NotFound { endpoint, .. }
            | Self::RateLimit { endpoint, .. }
            | Self::QuotaExceeded { endpoint, .. }
            | Self::ServerError { endpoint, .. }
            | Self::Api { endpoint, .. }
            | Self::InvalidResponse { endpoint, .. }
            | Self::OutputMismatch { endpoint, .. }
            | Self::Stream { endpoint, .. } => endpoint,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedCapability {
                endpoint,
                capability,
            } => write!(f, "{endpoint} does not support {capability}"),
            Self::InvalidRequest { endpoint, message } => {
                write!(f, "invalid request for {endpoint}: {message}")
            }
            Self::Http {
                endpoint,
                kind,
                message,
            } => write!(f, "{endpoint} {}: {message}", kind.as_str()),
            Self::BadRequest { endpoint, body } => {
                write!(f, "{endpoint} rejected the request: {body}")
            }
            Self::Unauthorized {
                endpoint,
                status,
                body,
            } => write!(
                f,
                "{endpoint} refused the credential (HTTP {status}): {body}"
            ),
            Self::NotFound { endpoint, body } => {
                write!(f, "{endpoint} has no such model or endpoint: {body}")
            }
            Self::RateLimit {
                endpoint,
                retry_after,
                body,
            } => match retry_after {
                Some(wait) => write!(
                    f,
                    "{endpoint} rate limited the request, retry after {}s: {body}",
                    wait.as_secs()
                ),
                None => write!(f, "{endpoint} rate limited the request: {body}"),
            },
            Self::QuotaExceeded {
                endpoint,
                status,
                body,
            } => write!(f, "{endpoint} quota exhausted (HTTP {status}): {body}"),
            Self::ServerError {
                endpoint,
                status,
                body,
            } => write!(f, "{endpoint} failed with HTTP {status}: {body}"),
            Self::Api {
                endpoint,
                status,
                body,
            } => write!(f, "{endpoint} returned HTTP {status}: {body}"),
            Self::InvalidResponse { endpoint, message } => {
                write!(f, "invalid {endpoint} response: {message}")
            }
            Self::OutputMismatch {
                endpoint,
                message,
                truncated,
                ..
            } => write!(
                f,
                "{endpoint} output did not match: {message}{}",
                if *truncated {
                    ", and the answer was cut short"
                } else {
                    ""
                }
            ),
            Self::Stream { endpoint, message } => write!(f, "{endpoint} stream failed: {message}"),
        }
    }
}
impl std::error::Error for Error {}
