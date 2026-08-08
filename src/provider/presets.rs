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

use super::{Auth, ProviderConfig, ProviderDialect};

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
    /// DeepSeek, via its OpenAI-compatible endpoint.
    DeepSeek,
    /// Groq, via its OpenAI-compatible endpoint.
    Groq,
    /// Together AI, via its OpenAI-compatible endpoint.
    Together,
    /// OpenRouter, via its OpenAI-compatible endpoint.
    OpenRouter,
    /// A local Ollama server, via its OpenAI-compatible endpoint.
    ///
    /// Needs no credentials, so [`Client::without_key`] and
    /// [`Client::from_env`] both work.
    ///
    /// [`Client::without_key`]: super::Client::without_key
    /// [`Client::from_env`]: super::Client::from_env
    Ollama,
}

impl ProviderType {
    /// The conventional environment variable holding this endpoint's API key.
    ///
    /// Empty for endpoints that need none.
    pub fn api_key_env(self) -> &'static str {
        match self {
            Self::OpenAi => "OPENAI_API_KEY",
            Self::Gemini => "GEMINI_API_KEY",
            Self::Anthropic => "ANTHROPIC_API_KEY",
            Self::DeepSeek => "DEEPSEEK_API_KEY",
            Self::Groq => "GROQ_API_KEY",
            Self::Together => "TOGETHER_API_KEY",
            Self::OpenRouter => "OPENROUTER_API_KEY",
            Self::Ollama => "",
        }
    }

    /// The wire format this endpoint speaks.
    pub fn dialect(self) -> ProviderDialect {
        match self {
            Self::OpenAi => ProviderDialect::OpenAiResponses,
            Self::Gemini => ProviderDialect::Gemini,
            Self::Anthropic => ProviderDialect::Anthropic,
            Self::DeepSeek | Self::Groq | Self::Together | Self::OpenRouter | Self::Ollama => {
                ProviderDialect::OpenAiChat
            }
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

            // Everything below speaks OpenAiChat. Each is one arm and nothing
            // else, no module and no change to the neutral model, which is the
            // point of separating dialect from endpoint.
            //
            // Model catalogues on these endpoints change often, so only
            // DeepSeek carries a default. Elsewhere, set `model` on the request
            // and get a clear local error if you forget.
            Self::DeepSeek => ProviderConfig::new(
                ProviderDialect::OpenAiChat,
                "DeepSeek",
                "https://api.deepseek.com/v1",
            )
            .default_model("deepseek-chat"),
            Self::Groq => ProviderConfig::new(
                ProviderDialect::OpenAiChat,
                "Groq",
                "https://api.groq.com/openai/v1",
            ),
            Self::Together => ProviderConfig::new(
                ProviderDialect::OpenAiChat,
                "Together",
                "https://api.together.xyz/v1",
            ),
            Self::OpenRouter => ProviderConfig::new(
                ProviderDialect::OpenAiChat,
                "OpenRouter",
                "https://openrouter.ai/api/v1",
            ),
            Self::Ollama => ProviderConfig::new(
                ProviderDialect::OpenAiChat,
                "Ollama",
                "http://localhost:11434/v1",
            )
            .auth(Auth::None),
        }
        .api_key_env_opt(self.api_key_env())
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

    const ALL: [ProviderType; 8] = [
        ProviderType::OpenAi,
        ProviderType::Gemini,
        ProviderType::Anthropic,
        ProviderType::DeepSeek,
        ProviderType::Groq,
        ProviderType::Together,
        ProviderType::OpenRouter,
        ProviderType::Ollama,
    ];

    #[test]
    fn every_preset_is_coherent() {
        for preset in ALL {
            let config = preset.config();
            assert!(config.base_url.starts_with("http"), "{preset:?}");
            assert_eq!(config.dialect, preset.dialect(), "{preset:?}");

            // A keyless endpoint names no variable and needs no key.
            if preset.api_key_env().is_empty() {
                assert_eq!(config.auth, Auth::None, "{preset:?}");
                assert_eq!(config.api_key_env, None, "{preset:?}");
            } else {
                assert_eq!(config.api_key_env, Some(preset.api_key_env()), "{preset:?}");
            }
        }
    }

    #[test]
    fn most_presets_reuse_a_dialect_rather_than_adding_one() {
        let chat = ALL
            .iter()
            .filter(|p| p.dialect() == ProviderDialect::OpenAiChat)
            .count();

        // The whole point of the split: one mapping, many endpoints.
        assert!(chat >= 5, "expected OpenAiChat to serve several endpoints");
    }

    #[test]
    fn keyless_endpoints_build_a_client_from_env() {
        assert!(super::super::Client::from_env(ProviderType::Ollama).is_some());
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
