//! Counting the tokens a message will cost.

use crate::Message;

/// Approximates the tokens one message contributes to a request.
///
/// Counting exactly is provider-specific: a BPE table for OpenAI, a different
/// one for Gemini, a network call for Anthropic. Freyja takes no tokenizer
/// dependency, so the count comes from you. [`crate::HeuristicCounter`] is the
/// default for a caller who does not care.
///
/// Per message rather than per transcript, for two reasons. A trimmer can walk
/// backwards and stop early instead of recounting a shrinking slice. And a
/// counter handed a slice is handed partial transcripts mid-trim, where a tool
/// result has lost the call it answers, which is a shape some provider
/// counting endpoints reject outright.
///
/// `Send` and not `Sync`, matching [`crate::Storage`]: an
/// [`crate::InMemoryStorage`] owns its counter and a [`crate::Conversation`]
/// owns its backend outright, so nothing shares one.
pub trait TokenCounter: Send {
    /// Approximates the tokens `message` contributes.
    ///
    /// Must be cheap. It runs once per message on every
    /// [`load`](crate::Storage::load).
    ///
    /// Infallible on purpose. A trimmer cannot ask anyone what to do about a
    /// failed estimate, and refusing to load a conversation over one is worse
    /// than a bad estimate, so a counter that could fail returns its own guess
    /// instead.
    fn count(&self, message: &Message) -> usize;
}

/// Any closure of the right shape is a counter, so a caller wiring up a real
/// tokenizer writes a closure rather than a struct.
impl<F: Fn(&Message) -> usize + Send> TokenCounter for F {
    fn count(&self, message: &Message) -> usize {
        self(message)
    }
}

#[cfg(test)]
mod tests {
    use super::TokenCounter;
    use crate::{Message, Role};

    #[test]
    fn closure_satisfies_token_counter() {
        let counter: &dyn TokenCounter = &|_: &Message| 7usize;
        assert_eq!(counter.count(&Message::text(Role::User, "hi")), 7);
    }
}
