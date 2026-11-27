//! The contract a conversation's memory implements.

use crate::Message;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

/// What a storage backend failed with.
///
/// A backend has no endpoint, so it does not produce a [`crate::Error`]: every
/// variant of that type carries one. [`crate::Conversation`] wraps this
/// instead.
pub type StorageError = Box<dyn std::error::Error + Send + Sync>;

/// The future returned by every [`Storage`] method.
pub type StorageFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, StorageError>> + Send + 'a>>;

/// Holds one conversation between turns.
///
/// One value is one conversation. A backend that must tell conversations apart
/// takes its key when it is built, not on every call, so the common case of one
/// conversation per value pays nothing for a parameter it would ignore.
///
/// Every method takes `&mut self` because a [`crate::Conversation`] owns its
/// backend outright. That is what removes the need for interior mutability: a
/// backend holding a plain `Vec` needs no lock, and `Vec<Message>` can
/// implement this trait directly.
///
/// Boxed rather than `async fn` in the trait, for the reason
/// [`crate::ToolFuture`] gives: `async fn` in traits is stable but not
/// `dyn`-compatible, and a backend may be erased behind `Box<dyn Storage>`.
pub trait Storage: Send {
    /// The conversation so far, oldest first.
    ///
    /// Order is this backend's contract. A backend returning a tool result
    /// ahead of the call it answers loses both messages, because
    /// [`crate::Conversation::send`] repairs the transcript before sending it
    /// and a call with no usable answer is rejected on the wire anyway. Order
    /// by a strictly monotonic column, never a second-granularity timestamp.
    fn load(&mut self) -> StorageFuture<'_, Vec<Message>>;

    /// Adds new turns to the end of the conversation.
    fn append(&mut self, messages: Vec<Message>) -> StorageFuture<'_, ()>;

    /// Empties the conversation.
    ///
    /// Must succeed on a conversation that is already empty. A caller whose
    /// clear failed has no way to ask what the backend deleted, so the only
    /// recourse offered is to call this again, and a backend that errors on
    /// an empty conversation turns that recourse into a second failure.
    ///
    /// Freyja does not promise this on a backend's behalf. It is stated here
    /// because [`crate::Conversation::clear`] tells callers a retry is safe,
    /// and that sentence is only true of backends that honour this.
    ///
    /// It must also remove everything stored with [`Storage::set_state`].
    fn clear(&mut self) -> StorageFuture<'_, ()>;

    /// The value stored under `key` in this conversation's state slot, if
    /// this backend keeps state.
    ///
    /// State is what a `Memory` keeps between sends, such as a cached
    /// summary, so a backend that persists it saves a model call after a
    /// restart. The default keeps nothing and returns `None`, which is always
    /// correct: a memory that finds no state rebuilds it.
    fn state<'a>(&'a mut self, key: &'a str) -> StorageFuture<'a, Option<Value>> {
        let _ = key;
        Box::pin(async { Ok(None) })
    }

    /// Stores `value` under `key`, replacing what was there.
    ///
    /// The default discards it. A backend that keeps state returns the
    /// latest value from [`Storage::state`] and removes all of it on
    /// [`Storage::clear`].
    fn set_state<'a>(&'a mut self, key: &'a str, value: Value) -> StorageFuture<'a, ()> {
        let _ = (key, value);
        Box::pin(async { Ok(()) })
    }
}

/// Forwards through a borrow, so a caller can keep their own transcript and
/// still run a conversation over it.
impl<T: Storage + ?Sized> Storage for &mut T {
    fn load(&mut self) -> StorageFuture<'_, Vec<Message>> {
        (**self).load()
    }
    fn append(&mut self, messages: Vec<Message>) -> StorageFuture<'_, ()> {
        (**self).append(messages)
    }
    fn clear(&mut self) -> StorageFuture<'_, ()> {
        (**self).clear()
    }
    fn state<'a>(&'a mut self, key: &'a str) -> StorageFuture<'a, Option<Value>> {
        (**self).state(key)
    }
    fn set_state<'a>(&'a mut self, key: &'a str, value: Value) -> StorageFuture<'a, ()> {
        (**self).set_state(key, value)
    }
}

/// Forwards through a box, so the backend can be chosen at run time.
impl<T: Storage + ?Sized> Storage for Box<T> {
    fn load(&mut self) -> StorageFuture<'_, Vec<Message>> {
        (**self).load()
    }
    fn append(&mut self, messages: Vec<Message>) -> StorageFuture<'_, ()> {
        (**self).append(messages)
    }
    fn clear(&mut self) -> StorageFuture<'_, ()> {
        (**self).clear()
    }
    fn state<'a>(&'a mut self, key: &'a str) -> StorageFuture<'a, Option<Value>> {
        (**self).state(key)
    }
    fn set_state<'a>(&'a mut self, key: &'a str, value: Value) -> StorageFuture<'a, ()> {
        (**self).set_state(key, value)
    }
}
