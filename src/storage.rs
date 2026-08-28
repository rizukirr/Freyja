//! Where a conversation lives between turns.

use crate::Message;
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
    fn clear(&mut self) -> StorageFuture<'_, ()>;
}

/// The conversation in this process, and nowhere else.
///
/// Lost when the vector is dropped. Persisting is a different backend, and
/// writing one needs nothing from this crate beyond [`Storage`], since
/// [`Message`] already derives `Serialize` and `Deserialize`.
///
/// This is what [`crate::Agent::conversation`] hands you, so the easy path
/// names no storage type at all.
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
/// A plain vector and an optional window. There is no lock, because a
/// [`crate::Conversation`] owns its backend outright, so [`Storage`] takes
/// `&mut self` and nothing here needs interior mutability.
///
/// Lost when the value is dropped, which makes it the right choice for a
/// short-lived process or a test and the wrong one for anything that has to
/// survive a restart. Persisting is a different backend, and writing one needs
/// nothing from this crate beyond [`Storage`], since [`Message`] already
/// derives `Serialize` and `Deserialize`.
#[derive(Debug, Default)]
pub struct InMemoryStorage {
    messages: Vec<Message>,
    window: Option<usize>,
}

impl InMemoryStorage {
    /// An empty conversation, with no window.
    pub fn new() -> Self {
        Self::default()
    }

    /// Send only the most recent `groups` turn groups, plus pinned turns.
    ///
    /// Everything is still held. [`InMemoryStorage::messages`] returns all of it,
    /// so a window shapes what one turn puts on the wire and never discards
    /// anything.
    ///
    /// A group is a message, except that an assistant turn requesting tools
    /// and the results answering it are one group. So an exchange costs two
    /// groups without tools and three with them: `window(20)` keeps roughly
    /// seven exchanges, not twenty.
    ///
    /// Trimming happens inside `load`, which is where every backend does it. A
    /// backend of your own decides its own rule, and may cut anywhere, since
    /// [`crate::Conversation::send`] repairs a cut that separated a tool call
    /// from the result answering it.
    pub fn window(mut self, groups: usize) -> Self {
        self.window = Some(groups);
        self
    }

    /// Everything held, which a window never shrinks.
    ///
    /// Borrowed rather than cloned, which is why there is no `all()`. An
    /// inherent method rather than a `Deref` to `[Message]`: this is a struct
    /// with a vector inside, not a smart pointer, and a `Deref` would commit
    /// the public API to every method a slice has, forever, while a later
    /// inherent method of the same name would shadow one silently.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }
}

impl Storage for InMemoryStorage {
    fn load(&mut self) -> StorageFuture<'_, Vec<Message>> {
        Box::pin(async move {
            Ok(match self.window {
                Some(groups) => crate::transcript::window_by_groups(&self.messages, groups),
                None => self.messages.clone(),
            })
        })
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
            Ok(())
        })
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
}
