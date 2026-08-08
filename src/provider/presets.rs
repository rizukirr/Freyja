//! Endpoints Freya ships knowledge of.
//!
//! A preset is nothing but a [`ProviderConfig`] with the URL, auth style, key
//! variable, and default model filled in. Adding a vendor here is a few lines
//! and touches nothing else, which is the point of separating dialect from
//! endpoint.
//!
//! Anything not listed still works, build the config yourself:
//!
//! ```
//! use freya::{ProviderConfig, ProviderDialect};
//!
//! // Any endpoint offering a drop-in Claude API.
//! let config = ProviderConfig::new(
//!         ProviderDialect::Anthropic,
//!         "my-gateway",
//!         "https://gateway.internal/anthropic/v1",
//!     )
//!     .api_key_env("GATEWAY_API_KEY")
//!     .default_model("claude-opus-5");
//! ```

use super::{ProviderConfig, ProviderDialect};

/// A known endpoint.
///
/// Converts into a [`ProviderConfig`], so anywhere a config is accepted a
/// `ProviderType` is too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderType {
    /// OpenAI, via the Responses API.
    OpenAi,
    /// Google Gemini, via the Interactions API.
    Gemini,
    /// Anthropic Claude, via the Messages API.
    Anthropic,
}

impl ProviderType {
    /// The conventional environment variable holding this endpoint's API key.
    pub fn api_key_env(self) -> &'static str {
        match self {
            Self::OpenAi => "OPENAI_API_KEY",
            Self::Gemini => "GEMINI_API_KEY",
            Self::Anthropic => "ANTHROPIC_API_KEY",
        }
    }

    /// The wire format this endpoint speaks.
    pub fn dialect(self) -> ProviderDialect {
        match self {
            Self::OpenAi => ProviderDialect::OpenAiResponses,
            Self::Gemini => ProviderDialect::Gemini,
            Self::Anthropic => ProviderDialect::Anthropic,
        }
    }

    /// The full endpoint description.
    pub fn config(self) -> ProviderConfig {
        match self {
            Self::OpenAi => ProviderConfig::new(
                ProviderDialect::OpenAiResponses,
                "OpenAI",
                "https://api.openai.com/v1",
            )
            .default_model("gpt-5.6-sol"),
            Self::Gemini => ProviderConfig::new(
                ProviderDialect::Gemini,
                "Gemini",
                "https://generativelanguage.googleapis.com/v1beta",
            )
            .default_model("gemini-3.5-flash"),
            Self::Anthropic => ProviderConfig::new(
                ProviderDialect::Anthropic,
                "Anthropic",
                "https://api.anthropic.com/v1",
            )
            .default_model("claude-opus-5"),
        }
        .api_key_env(self.api_key_env())
    }
}

impl From<ProviderType> for ProviderConfig {
    fn from(value: ProviderType) -> Self {
        value.config()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_is_fully_populated() {
        for preset in [
            ProviderType::OpenAi,
            ProviderType::Gemini,
            ProviderType::Anthropic,
        ] {
            let config = preset.config();
            assert!(config.base_url.starts_with("https://"));
            assert!(config.default_model.is_some());
            assert_eq!(config.api_key_env, Some(preset.api_key_env()));
            assert_eq!(config.dialect, preset.dialect());
        }
    }

    #[test]
    fn preset_urls_match_the_endpoints_freya_shipped_with() {
        assert_eq!(
            ProviderType::OpenAi.config().url(),
            "https://api.openai.com/v1/responses"
        );
        assert_eq!(
            ProviderType::Gemini.config().url(),
            "https://generativelanguage.googleapis.com/v1beta/interactions"
        );
        assert_eq!(
            ProviderType::Anthropic.config().url(),
            "https://api.anthropic.com/v1/messages"
        );
    }
}
