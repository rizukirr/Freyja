//! Where a conversation lives between turns.

use crate::Message;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

/// What a storage backend failed with.
///
/// A backend has no endpoint, so it does not produce a [`crate::Error`]: every
/// variant of that type carries one. [`crate::Agent`] wraps this instead.
pub type StorageError = Box<dyn std::error::Error + Send + Sync>;

/// The future returned by every [`Storage`] method.
pub type StorageFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, StorageError>> + Send + 'a>>;

/// Holds one conversation between turns.
///
/// One value is one conversation. A backend that must tell conversations apart
/// takes its key when it is built, not on every call, so the common case of one
/// agent per conversation pays nothing for a parameter it would ignore.
///
/// Boxed rather than `async fn` in the trait, for the reason
/// [`crate::ToolFuture`] gives: `async fn` in traits is stable but not
/// `dyn`-compatible, and `Agent` stores trait objects.
pub trait Storage: Send + Sync {
    /// The conversation so far, oldest first.
    fn load(&self) -> StorageFuture<'_, Vec<Message>>;

    /// Adds new turns to the end of the conversation.
    fn append(&self, messages: Vec<Message>) -> StorageFuture<'_, ()>;

    /// Empties the conversation.
    fn clear(&self) -> StorageFuture<'_, ()>;
}

/// The conversation in this process, and nowhere else.
///
/// Lost when the value is dropped. Persisting is a different backend, and
/// writing one needs nothing from this crate beyond the [`Storage`] trait,
/// since [`Message`] already derives `Serialize` and `Deserialize`.
#[derive(Default)]
pub struct InMemoryStorage {
    messages: Mutex<Vec<Message>>,
}

impl InMemoryStorage {
    /// An empty conversation.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Storage for InMemoryStorage {
    fn load(&self) -> StorageFuture<'_, Vec<Message>> {
        Box::pin(async move { Ok(self.messages.lock().expect("poisoned").clone()) })
    }

    fn append(&self, messages: Vec<Message>) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            self.messages.lock().expect("poisoned").extend(messages);
            Ok(())
        })
    }

    fn clear(&self) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            self.messages.lock().expect("poisoned").clear();
            Ok(())
        })
    }
}

/// Forwards through a shared handle, so a caller can hold an `Arc<T>` for
/// itself while also installing it with [`crate::Agent::memory`].
impl<T: Storage + ?Sized> Storage for std::sync::Arc<T> {
    fn load(&self) -> StorageFuture<'_, Vec<Message>> {
        (**self).load()
    }
    fn append(&self, messages: Vec<Message>) -> StorageFuture<'_, ()> {
        (**self).append(messages)
    }
    fn clear(&self) -> StorageFuture<'_, ()> {
        (**self).clear()
    }
}

#[cfg(test)]
mod tests {
    use super::{InMemoryStorage, Storage};
    use crate::{Message, Role};

    #[tokio::test]
    async fn starts_empty() {
        assert!(InMemoryStorage::new().load().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn append_then_load_round_trips_in_order() {
        let storage = InMemoryStorage::new();
        storage
            .append(vec![Message::text(Role::User, "first")])
            .await
            .unwrap();
        storage
            .append(vec![Message::text(Role::User, "second")])
            .await
            .unwrap();
        let loaded = storage.load().await.unwrap();
        assert_eq!(loaded.first(), Some(&Message::text(Role::User, "first")));
        assert_eq!(loaded.last(), Some(&Message::text(Role::User, "second")));
    }

    #[tokio::test]
    async fn clear_empties_it() {
        let storage = InMemoryStorage::new();
        storage
            .append(vec![Message::text(Role::User, "first")])
            .await
            .unwrap();
        storage.clear().await.unwrap();
        assert!(storage.load().await.unwrap().is_empty());
    }
}
