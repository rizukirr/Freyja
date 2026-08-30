//! Errors returned while preparing, sending, or decoding a generation request.

use crate::endpoint::is_secret_name;
use std::borrow::Cow;
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

/// Replaces `url` inside `message` with a copy whose credential-shaped query
/// values are withheld.
///
/// `reqwest` puts the whole URL in its `Display`, and it already strips
/// userinfo, so the query is what is left. An endpoint taking its key as
/// `?key=...` is a real shape, and [`crate::EndpointConfig::query`] is where
/// that key goes, so without this the value `Debug` withholds is printed in
/// full by every transport failure. Errors reach a log far more often than a
/// config does.
///
/// Unconditional rather than gated on the build profile: a redaction that is
/// absent in development and present in production is one nobody ever sees
/// working.
///
/// The same name heuristic as [`crate::EndpointConfig`]'s `Debug`, and the same
/// caveat: it cannot know that `?passport=` is a credential.
///
/// The placeholder is bare rather than the `<redacted>` used elsewhere: a query
/// value is percent-encoded on the way back in, and `%3Credacted%3E` in an
/// error message is harder to read than the thing it stands for.
fn redact_url_in(message: String, url: Option<&reqwest::Url>) -> String {
    const REDACTED: &str = "REDACTED";

    let Some(url) = url else {
        return message;
    };
    if !url.query_pairs().any(|(name, _)| is_secret_name(&name)) {
        return message;
    }

    let mut safe = url.clone();
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(name, value)| {
            let value = if is_secret_name(&name) {
                REDACTED.to_string()
            } else {
                value.into_owned()
            };
            (name.into_owned(), value)
        })
        .collect();
    safe.query_pairs_mut().clear().extend_pairs(pairs);

    message.replace(url.as_str(), safe.as_str())
}

/// Everything that can go wrong on the way to a generation response.
///
/// # Printing one
///
/// Both `Display` and `Debug` trim the endpoint's response body to
/// [`BODY_IN_MESSAGE`] bytes and say how much they left out. A rejected request
/// is usually logged rather than read, and a provider that quotes the whole
/// offending request back turns one 400 into a log line thousands of characters
/// wide.
///
/// Nothing is lost: the fields are public, and [`Error::body`] reaches the
/// untrimmed body without matching on the variant.
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

/// How much of a response body a printed [`Error`] carries.
///
/// Two kilobytes holds every provider error message seen in practice with room
/// to spare. What it excludes is the case that made this necessary: a body that
/// quotes the whole rejected request back, which is normal for a 400 and which
/// no log line should have to hold.
pub const BODY_IN_MESSAGE: usize = 2048;

/// Trims `value` to [`BODY_IN_MESSAGE`] bytes, naming what it dropped.
///
/// Borrows when nothing needs dropping, which is the common case: this runs on
/// every printed error and most bodies are a sentence.
fn capped(value: &str) -> Cow<'_, str> {
    if value.len() <= BODY_IN_MESSAGE {
        return Cow::Borrowed(value);
    }
    // Never split a codepoint: the cut lands mid-character often enough, and
    // slicing there panics.
    let mut end = BODY_IN_MESSAGE;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    Cow::Owned(format!(
        "{}… ({} more bytes)",
        &value[..end],
        value.len() - end
    ))
}

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
            message: redact_url_in(error.to_string(), error.url()),
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
    /// The endpoint's raw response body, untrimmed, when the error has one.
    ///
    /// What [`Display`](fmt::Display) and [`Debug`](fmt::Debug) shorten. Reach
    /// for this when a provider's rejection runs past [`BODY_IN_MESSAGE`] and
    /// the part that matters is not in the first two kilobytes.
    pub fn body(&self) -> Option<&str> {
        match self {
            Self::BadRequest { body, .. }
            | Self::Unauthorized { body, .. }
            | Self::NotFound { body, .. }
            | Self::RateLimit { body, .. }
            | Self::QuotaExceeded { body, .. }
            | Self::ServerError { body, .. }
            | Self::Api { body, .. } => Some(body),
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
                write!(f, "{endpoint} rejected the request: {}", capped(body))
            }
            Self::Unauthorized {
                endpoint,
                status,
                body,
            } => write!(
                f,
                "{endpoint} refused the credential (HTTP {status}): {}",
                capped(body)
            ),
            Self::NotFound { endpoint, body } => {
                write!(
                    f,
                    "{endpoint} has no such model or endpoint: {}",
                    capped(body)
                )
            }
            Self::RateLimit {
                endpoint,
                retry_after,
                body,
            } => match retry_after {
                Some(wait) => write!(
                    f,
                    "{endpoint} rate limited the request, retry after {}s: {}",
                    wait.as_secs(),
                    capped(body)
                ),
                None => write!(f, "{endpoint} rate limited the request: {}", capped(body)),
            },
            Self::QuotaExceeded {
                endpoint,
                status,
                body,
            } => write!(
                f,
                "{endpoint} quota exhausted (HTTP {status}): {}",
                capped(body)
            ),
            Self::ServerError {
                endpoint,
                status,
                body,
            } => write!(f, "{endpoint} failed with HTTP {status}: {}", capped(body)),
            Self::Api {
                endpoint,
                status,
                body,
            } => write!(f, "{endpoint} returned HTTP {status}: {}", capped(body)),
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
/// Mirrors the derived output, with the body and the model's text trimmed.
///
/// Hand-written for the reason [`Display`](fmt::Display) caps: `{:?}` in a
/// logging macro is at least as common as `{}`, so capping only one of the two
/// leaves the case that made this worth doing wide open.
impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedCapability {
                endpoint,
                capability,
            } => f
                .debug_struct("UnsupportedCapability")
                .field("endpoint", endpoint)
                .field("capability", capability)
                .finish(),
            Self::InvalidRequest { endpoint, message } => f
                .debug_struct("InvalidRequest")
                .field("endpoint", endpoint)
                .field("message", message)
                .finish(),
            Self::Http {
                endpoint,
                kind,
                message,
            } => f
                .debug_struct("Http")
                .field("endpoint", endpoint)
                .field("kind", kind)
                .field("message", message)
                .finish(),
            Self::BadRequest { endpoint, body } => f
                .debug_struct("BadRequest")
                .field("endpoint", endpoint)
                .field("body", &capped(body))
                .finish(),
            Self::Unauthorized {
                endpoint,
                status,
                body,
            } => f
                .debug_struct("Unauthorized")
                .field("endpoint", endpoint)
                .field("status", status)
                .field("body", &capped(body))
                .finish(),
            Self::NotFound { endpoint, body } => f
                .debug_struct("NotFound")
                .field("endpoint", endpoint)
                .field("body", &capped(body))
                .finish(),
            Self::RateLimit {
                endpoint,
                retry_after,
                body,
            } => f
                .debug_struct("RateLimit")
                .field("endpoint", endpoint)
                .field("retry_after", retry_after)
                .field("body", &capped(body))
                .finish(),
            Self::QuotaExceeded {
                endpoint,
                status,
                body,
            } => f
                .debug_struct("QuotaExceeded")
                .field("endpoint", endpoint)
                .field("status", status)
                .field("body", &capped(body))
                .finish(),
            Self::ServerError {
                endpoint,
                status,
                body,
            } => f
                .debug_struct("ServerError")
                .field("endpoint", endpoint)
                .field("status", status)
                .field("body", &capped(body))
                .finish(),
            Self::Api {
                endpoint,
                status,
                body,
            } => f
                .debug_struct("Api")
                .field("endpoint", endpoint)
                .field("status", status)
                .field("body", &capped(body))
                .finish(),
            Self::InvalidResponse { endpoint, message } => f
                .debug_struct("InvalidResponse")
                .field("endpoint", endpoint)
                .field("message", message)
                .finish(),
            // `text` is the model's whole answer, which is exactly as large as
            // a generation is allowed to be, so it is capped for the same
            // reason a body is. `Display` never printed it at all.
            Self::OutputMismatch {
                endpoint,
                message,
                text,
                truncated,
            } => f
                .debug_struct("OutputMismatch")
                .field("endpoint", endpoint)
                .field("message", message)
                .field("text", &capped(text))
                .field("truncated", truncated)
                .finish(),
            Self::Stream { endpoint, message } => f
                .debug_struct("Stream")
                .field("endpoint", endpoint)
                .field("message", message)
                .finish(),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    /// A body of `size` bytes that quotes caller content back, which is what a
    /// real 400 does and the reason any of this is capped.
    fn rejection(size: usize) -> Error {
        let mut body = String::from(r#"{"error":{"message":"Invalid value: "#);
        while body.len() < size {
            body.push_str("what the user typed ");
        }
        body.push_str(r#""}}"#);
        Error::from_status("OpenAI".into(), 400, None, body)
    }

    #[test]
    fn a_credential_shaped_query_value_is_withheld_from_the_message() {
        let url =
            reqwest::Url::parse("https://x.test/v1/messages?api-version=2024-02-01&key=SECRET")
                .expect("a valid url");
        let message = format!("error sending request for url ({url})");

        let redacted = super::redact_url_in(message, Some(&url));

        assert!(!redacted.contains("SECRET"), "{redacted}");
        assert!(redacted.contains("key=REDACTED"), "{redacted}");
        // A parameter that is not credential shaped is what a reader needs.
        assert!(redacted.contains("api-version=2024-02-01"), "{redacted}");
    }

    #[test]
    fn a_message_with_nothing_to_hide_is_left_alone() {
        let url = reqwest::Url::parse("https://x.test/v1/messages?api-version=2024-02-01")
            .expect("a valid url");
        let message = format!("error sending request for url ({url})");

        assert_eq!(super::redact_url_in(message.clone(), Some(&url)), message);
        assert_eq!(super::redact_url_in(message.clone(), None), message);
    }

    #[test]
    fn a_short_body_is_printed_whole() {
        let error = Error::from_status("OpenAI".into(), 400, None, "too many tokens".into());

        assert_eq!(
            error.to_string(),
            "OpenAI rejected the request: too many tokens"
        );
        assert!(format!("{error:?}").contains("too many tokens"));
        assert!(!error.to_string().contains("more bytes"));
    }

    #[test]
    fn a_long_body_is_trimmed_in_both_renderings() {
        let error = rejection(9000);
        let full = error.body().expect("a 400 carries its body");

        for rendered in [error.to_string(), format!("{error:?}")] {
            assert!(
                rendered.len() < full.len() / 2,
                "still {} chars: {}",
                rendered.len(),
                &rendered[..80]
            );
            assert!(rendered.contains("more bytes"), "says what it dropped");
            // The part a reader needs is the front of the body, and it
            // survives. Matched without the quoting, since `Debug` escapes it.
            assert!(rendered.contains("Invalid value:"));
            assert!(rendered.contains("what the user typed"));
        }
    }

    #[test]
    fn the_untrimmed_body_is_still_reachable() {
        let error = rejection(9000);

        assert!(error.body().expect("a body").len() > 9000);
        // And the variants that never had one say so rather than inventing it.
        assert!(
            Error::Stream {
                endpoint: "OpenAI".into(),
                message: "closed".into(),
            }
            .body()
            .is_none()
        );
    }

    #[test]
    fn trimming_never_splits_a_codepoint() {
        // A cut landing mid-character panics on slicing, and the boundary is
        // hit by any body of multi-byte text at the wrong length.
        for pad in 0..8 {
            let mut body = "é".repeat(BODY_IN_MESSAGE);
            body.insert_str(0, &"x".repeat(pad));
            let error = Error::from_status("OpenAI".into(), 400, None, body);

            assert!(error.to_string().contains("more bytes"));
            assert!(format!("{error:?}").contains("more bytes"));
        }
    }

    #[test]
    fn the_model_answer_is_trimmed_too() {
        // `Display` never printed it, but `Debug` did, and it is as large as a
        // generation is allowed to be.
        let error = Error::OutputMismatch {
            endpoint: "OpenAI".into(),
            message: "expected an object".into(),
            text: "n".repeat(9000),
            truncated: false,
        };

        let rendered = format!("{error:?}");
        assert!(rendered.len() < 4000, "still {} chars", rendered.len());
        assert!(rendered.contains("more bytes"));
    }
}
