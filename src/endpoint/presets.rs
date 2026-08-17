//! Endpoints Freyja ships knowledge of.
//!
//! Deliberately only the three first-party vendors whose dialects Freyja
//! implements and tests against. A preset is a standing promise that a base URL
//! and a default model are still current, and third-party endpoints change both
//! faster than this crate could verify. Shipping a stale preset is worse than
//! shipping none, because it fails at the vendor with a confusing 404 rather
//! than locally with a clear message.
//!
//! Every other endpoint is reachable, and no less supported for being absent.
//! Most copy one of these three wire formats, so they need a [`EndpointConfig`]
//! and nothing more. See the compatible-endpoint list in
//! `docs/providers/custom.md`.
//!
//! ```
//! use freyja::{EndpointConfig, Dialect};
//!
//! // Any endpoint offering a drop-in Claude API.
//! let config = EndpointConfig::new(
//!         Dialect::Anthropic,
//!         "my-gateway",
//!         "https://gateway.internal/anthropic/v1",
//!     )
//!     .api_key_env("GATEWAY_API_KEY")
//!     .default_model("claude-opus-5");
//! ```

use super::EndpointConfig;
use crate::dialect::Dialect;

/// A known endpoint.
///
/// Converts into a [`EndpointConfig`], so anywhere a config is accepted a
/// `EndpointPreset` is too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointPreset {
    /// OpenAI, via the Responses API.
    OpenAi,
    /// Google Gemini, via the Interactions API.
    Gemini,
    /// Anthropic Claude, via the Messages API.
    Anthropic,
}

impl EndpointPreset {
    /// The conventional environment variable holding this endpoint's API key.
    pub fn api_key_env(self) -> &'static str {
        match self {
            Self::OpenAi => "OPENAI_API_KEY",
            Self::Gemini => "GEMINI_API_KEY",
            Self::Anthropic => "ANTHROPIC_API_KEY",
        }
    }

    /// The wire format this endpoint speaks.
    pub fn dialect(self) -> Dialect {
        match self {
            Self::OpenAi => Dialect::OpenAiResponses,
            Self::Gemini => Dialect::Gemini,
            Self::Anthropic => Dialect::Anthropic,
        }
    }

    /// The full endpoint description.
    pub fn config(self) -> EndpointConfig {
        match self {
            Self::OpenAi => EndpointConfig::new(
                Dialect::OpenAiResponses,
                "OpenAI",
                "https://api.openai.com/v1",
            )
            .default_model("gpt-5.6-sol"),
            Self::Gemini => EndpointConfig::new(
                Dialect::Gemini,
                "Gemini",
                "https://generativelanguage.googleapis.com/v1beta",
            )
            .default_model("gemini-3.5-flash"),
            Self::Anthropic => EndpointConfig::new(
                Dialect::Anthropic,
                "Anthropic",
                "https://api.anthropic.com/v1",
            )
            .default_model("claude-opus-5"),
        }
        .api_key_env(self.api_key_env())
    }
}

impl From<EndpointPreset> for EndpointConfig {
    fn from(value: EndpointPreset) -> Self {
        value.config()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [EndpointPreset; 3] = [
        EndpointPreset::OpenAi,
        EndpointPreset::Gemini,
        EndpointPreset::Anthropic,
    ];

    #[test]
    fn every_preset_is_fully_populated() {
        for preset in ALL {
            let config = preset.config();
            assert!(config.base_url.starts_with("https://"), "{preset:?}");
            assert!(config.default_model.is_some(), "{preset:?}");
            assert_eq!(config.api_key_env, Some(preset.api_key_env()), "{preset:?}");
            assert_eq!(config.dialect, preset.dialect(), "{preset:?}");
        }
    }

    #[test]
    fn presets_cover_only_first_party_vendors() {
        // Third-party endpoints are reached through EndpointConfig, not shipped
        // here, so this list stays short on purpose. Adding to it means taking
        // on responsibility for a URL and a model name Freyja does not test.
        assert_eq!(ALL.len(), 3);
    }

    #[test]
    fn preset_urls_match_the_endpoints_freyja_shipped_with() {
        assert_eq!(
            EndpointPreset::OpenAi.config().url(),
            "https://api.openai.com/v1/responses"
        );
        assert_eq!(
            EndpointPreset::Gemini.config().url(),
            "https://generativelanguage.googleapis.com/v1beta/interactions"
        );
        assert_eq!(
            EndpointPreset::Anthropic.config().url(),
            "https://api.anthropic.com/v1/messages"
        );
    }
}
