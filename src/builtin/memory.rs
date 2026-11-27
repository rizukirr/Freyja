//! The conversation held in this process, and nowhere else.

use crate::{Message, Storage, StorageFuture};

/// The conversation in this process, and nowhere else.
///
/// Lost when the vector is dropped. [`crate::JsonlStorage`] is the backend
/// that persists, and writing another needs nothing from this crate beyond
/// [`Storage`], since [`Message`] already derives `Serialize` and
/// `Deserialize`.
///
/// This is what a caller passes when they want to hold the transcript
/// themselves, as `agent.conversation(&mut history)`, and it is extended in
/// place.
impl Storage for Vec<Message> {
    fn load(&mut self) -> StorageFuture<'_, Vec<Message>> {
        Box::pin(async move { Ok(self.clone()) })
    }

    fn append(&mut self, messages: Vec<Message>) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            self.extend(messages);
            Ok(())
        })
    }

    fn clear(&mut self) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            Vec::clear(self);
            Ok(())
        })
    }
}

/// The conversation in this process, and nowhere else.
///
/// A pure store: every turn held in this process, lost when dropped, plus
/// state kept for a memory. There is no lock, because a
/// [`crate::Conversation`] owns its backend outright, so [`Storage`] takes
/// `&mut self` and nothing here needs interior mutability.
///
/// Windows and summaries are not done here, see [`crate::Agent::memory`].
/// [`crate::JsonlStorage`] is the backend that persists, and writing another
/// needs nothing from this crate beyond [`Storage`], since [`Message`] already
/// derives `Serialize` and `Deserialize`.
#[derive(Debug, Default)]
pub struct InMemoryStorage {
    messages: Vec<Message>,
    state: std::collections::HashMap<String, serde_json::Value>,
}

impl InMemoryStorage {
    /// An empty conversation.
    pub fn new() -> Self {
        Self::default()
    }

    /// Everything held.
    ///
    /// Borrowed rather than cloned, which is why there is no `all()`. An
    /// inherent method rather than a `Deref` to `[Message]`: this is a struct
    /// with a vector inside, not a smart pointer, and a `Deref` would commit
    /// the public API to every method a slice has, forever, while a later
    /// inherent method of the same name would shadow one silently.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// A conversation read back from somewhere that outlived the process.
    pub(crate) fn restore(
        messages: Vec<Message>,
        state: std::collections::HashMap<String, serde_json::Value>,
    ) -> Self {
        Self { messages, state }
    }
}

impl Storage for InMemoryStorage {
    fn load(&mut self) -> StorageFuture<'_, Vec<Message>> {
        Box::pin(async move { Ok(self.messages.clone()) })
    }

    fn append(&mut self, messages: Vec<Message>) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            self.messages.extend(messages);
            Ok(())
        })
    }

    fn clear(&mut self) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            self.messages.clear();
            self.state.clear();
            Ok(())
        })
    }

    fn state<'a>(&'a mut self, key: &'a str) -> StorageFuture<'a, Option<serde_json::Value>> {
        Box::pin(async move { Ok(self.state.get(key).cloned()) })
    }

    fn set_state<'a>(
        &'a mut self,
        key: &'a str,
        value: serde_json::Value,
    ) -> StorageFuture<'a, ()> {
        Box::pin(async move {
            self.state.insert(key.to_string(), value);
            Ok(())
        })
    }
}
