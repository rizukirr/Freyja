//! Every capability Freyja refuses, and the evidence that the refusal is true.
//!
//! A refusal is a claim: *this wire format has nowhere to put this field*. It
//! is the only thing Freyja is allowed to decide on a provider's behalf, and it
//! costs the caller a capability, so it had better be true.
//!
//! Twice it was not. The Gemini dialect refused `reasoning_effort` and
//! `tool_choice` because neither appears at the top level of a request — and
//! both live under `generation_config`, where nobody had looked. Both refusals
//! shipped, and neither was ever checked against the endpoint, because nothing
//! recorded that they had not been. Both are mappings now.
//!
//! This module is that record. Every refusal in the codebase names a constant
//! declared here and appears in [`REFUSALS`] with its [`Evidence`], so the
//! question "did anyone verify this?" is a table lookup rather than an
//! archaeology exercise. The counts are asserted in tests: verifying a refusal
//! or adding one both fail until this file is updated to match.
//!
//! What a refusal is *not* allowed to be is a guess about a model. If the wire
//! format has a field, the value goes to the vendor and the vendor answers.

#[cfg(test)]
use super::ProviderDialect;
use super::{ProviderConfig, ProviderError};

/// Builds the refusal error for `capability`, naming the endpoint that raised it.
///
/// Dialects call this rather than constructing the variant, so every refusal
/// goes through a constant declared in this module.
pub(crate) fn unsupported(config: &ProviderConfig, capability: &'static str) -> ProviderError {
    ProviderError::UnsupportedCapability {
        provider: config.name.clone(),
        capability,
    }
}

/// Continuing a conversation by id rather than by replaying the transcript.
pub(crate) const CONVERSATION_CONTINUATION: &str = "server-side conversation continuation";
/// Free-form labels or trace ids attached to the request.
pub(crate) const REQUEST_METADATA: &str = "request metadata";
/// Asking for any valid JSON, with no schema to constrain it.
pub(crate) const SCHEMALESS_JSON: &str = "schema-less JSON response format";
/// Anything but text in a system or developer turn.
pub(crate) const NON_TEXT_SYSTEM: &str = "non-text content in system/developer messages";
/// An image attached to a turn that is not the user's.
pub(crate) const IMAGES_OUTSIDE_USER: &str = "images outside user messages";
/// Reasoning effort at a level this dialect's own scale has no word for.
pub(crate) const EFFORT_NONE: &str = "reasoning effort 'none'";
/// See [`EFFORT_NONE`].
pub(crate) const EFFORT_XHIGH: &str = "reasoning effort 'xhigh'";
/// See [`EFFORT_NONE`].
pub(crate) const EFFORT_MAX: &str = "reasoning effort 'max'";

/// How well a refusal is known to be true.
///
/// The table below is compiled under `cfg(test)` only: nothing reads it at
/// runtime, and its job is to be read by people and asserted over by the
/// ratchet at the bottom of this file. Compiling it always would mean an
/// `allow(dead_code)`, which is how a table stops being maintained.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Evidence {
    /// The endpoint was asked and said no. The strongest evidence there is.
    Probed,
    /// No field exists to put it in, and none could: the API is stateless, or
    /// the concept is absent from its data model. Not probed, because there is
    /// nothing to send.
    Structural,
    /// Nobody has checked. The refusal was written from a reading of the wire
    /// format, which is exactly how the two false ones were written.
    Unverified,
    /// The endpoint was asked and said yes. The refusal is wrong and the code
    /// still carries it.
    Refuted,
}

/// One capability, refused by one dialect.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct Refusal {
    /// The dialect that refuses it.
    pub dialect: ProviderDialect,
    /// The constant from this module that names it.
    pub capability: &'static str,
    /// How well the refusal is known to be true.
    pub evidence: Evidence,
    /// What was observed, or why no observation is possible.
    pub note: &'static str,
}

/// Every refusal the four dialects raise.
///
/// Ordered by dialect. Read the [`Evidence`] column before trusting a row.
#[cfg(test)]
pub(crate) const REFUSALS: &[Refusal] = &[
    Refusal {
        dialect: ProviderDialect::OpenAiResponses,
        capability: NON_TEXT_SYSTEM,
        evidence: Evidence::Probed,
        note: "`Invalid value: 'input_image'. Supported values are: 'input_text'.` \
               for both the system and developer roles.",
    },
    Refusal {
        dialect: ProviderDialect::OpenAiResponses,
        capability: IMAGES_OUTSIDE_USER,
        evidence: Evidence::Probed,
        note: "An assistant turn takes `output_text` and `refusal` and nothing \
               else; `input_image` and `output_image` are both rejected by name.",
    },
    Refusal {
        dialect: ProviderDialect::OpenAiChat,
        capability: CONVERSATION_CONTINUATION,
        evidence: Evidence::Structural,
        note: "Chat Completions is stateless. There is no response id to continue \
               from and no field to carry one.",
    },
    Refusal {
        dialect: ProviderDialect::Gemini,
        capability: REQUEST_METADATA,
        evidence: Evidence::Probed,
        note: "`labels` is rejected outright -- \"not available on the Gemini API but \
               it is available on the Gemini Enterprise Agent Platform\" -- and \
               `metadata`, `generation_config.labels`, and \
               `generation_config.metadata` are all `Unknown parameter`.",
    },
    Refusal {
        dialect: ProviderDialect::Gemini,
        capability: NON_TEXT_SYSTEM,
        evidence: Evidence::Structural,
        note: "`system_instruction` is a bare string and nothing else: an array or \
               object gives `The value is invalid for 'system_instruction'. \
               Expected string`. There is no richer shape to put an image in.",
    },
    Refusal {
        dialect: ProviderDialect::Gemini,
        capability: EFFORT_NONE,
        evidence: Evidence::Probed,
        note: "`'none' is not supported for 'thinking_level'. Supported values: \
               'minimal', 'low', 'medium', 'high'.`",
    },
    Refusal {
        dialect: ProviderDialect::Gemini,
        capability: EFFORT_XHIGH,
        evidence: Evidence::Probed,
        note: "Rejected by name, as EFFORT_NONE.",
    },
    Refusal {
        dialect: ProviderDialect::Gemini,
        capability: EFFORT_MAX,
        evidence: Evidence::Probed,
        note: "Rejected by name, as EFFORT_NONE.",
    },
    Refusal {
        dialect: ProviderDialect::Anthropic,
        capability: CONVERSATION_CONTINUATION,
        evidence: Evidence::Structural,
        note: "The Messages API is stateless. Nothing is stored to continue from.",
    },
    Refusal {
        dialect: ProviderDialect::Anthropic,
        capability: NON_TEXT_SYSTEM,
        evidence: Evidence::Unverified,
        note: "No Anthropic key is available, so this was probed only against DeepSeek's \
               Anthropic-compatible endpoint, where it was accepted. That is not \
               enough: a compatible endpoint reimplements the schema, and Anthropic \
               could restrict roles above deserialization the way OpenAI Responses \
               does. Needs a first-party key.",
    },
    Refusal {
        dialect: ProviderDialect::Anthropic,
        capability: IMAGES_OUTSIDE_USER,
        evidence: Evidence::Unverified,
        note: "No Anthropic key is available, so this was probed only against DeepSeek's \
               Anthropic-compatible endpoint, where it was accepted. That is not \
               enough: a compatible endpoint reimplements the schema, and Anthropic \
               could restrict roles above deserialization the way OpenAI Responses \
               does. Needs a first-party key.",
    },
    Refusal {
        dialect: ProviderDialect::Anthropic,
        capability: SCHEMALESS_JSON,
        evidence: Evidence::Unverified,
        note: "DeepSeek's Anthropic-compatible endpoint accepted `output_config`, \
               and then accepted an invented top-level key too -- it ignores \
               parameters it does not know, so its answer means nothing here. \
               Needs a first-party key.",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The evidence behind every refusal Freyja ships, counted.
    ///
    /// Not a quality bar -- eight unverified refusals is a poor showing, and
    /// saying so in an assertion is the point. It is a ratchet: probing one of
    /// them fails this test until the row is updated, and so does adding a new
    /// refusal without evidence. Neither can happen quietly.
    #[test]
    fn the_evidence_behind_every_refusal_is_accounted_for() {
        let count = |wanted| {
            REFUSALS
                .iter()
                .filter(|refusal| refusal.evidence == wanted)
                .count()
        };

        assert_eq!(count(Evidence::Probed), 6, "endpoint asked, said no");
        assert_eq!(count(Evidence::Structural), 3, "no field could exist");
        assert_eq!(count(Evidence::Unverified), 3, "nobody has checked");
        assert_eq!(count(Evidence::Refuted), 0, "known wrong, still shipping");
        assert_eq!(REFUSALS.len(), 12);
    }

    /// A dialect refusing the same capability twice means two code paths
    /// disagree about why, and one of them is stale.
    #[test]
    fn no_dialect_refuses_the_same_capability_twice() {
        for (index, refusal) in REFUSALS.iter().enumerate() {
            let duplicate = REFUSALS[..index].iter().any(|prior| {
                prior.dialect == refusal.dialect && prior.capability == refusal.capability
            });

            assert!(
                !duplicate,
                "{:?} refuses {} twice",
                refusal.dialect, refusal.capability
            );
        }
    }

    /// Evidence is only worth recording if it says what was seen.
    #[test]
    fn every_refusal_carries_a_note() {
        for refusal in REFUSALS {
            assert!(
                refusal.note.len() > 30,
                "{:?} / {} needs a note saying what was observed",
                refusal.dialect,
                refusal.capability
            );
        }
    }
}
