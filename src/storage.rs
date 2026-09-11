//! Where a conversation lives between turns.

use crate::{Message, Role, Summarizer, TokenCounter};
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
    fn clear(&mut self) -> StorageFuture<'_, ()>;
}

/// The conversation in this process, and nowhere else.
///
/// Lost when the vector is dropped. Persisting is a different backend, and
/// writing one needs nothing from this crate beyond [`Storage`], since
/// [`Message`] already derives `Serialize` and `Deserialize`.
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

/// Which rule [`InMemoryStorage::load`] trims by.
///
/// One field rather than two, so the last window set is the window used. A
/// token budget already subsumes what a group cap protects against, and
/// composing the two would need a documented application order plus a builder
/// pair whose two call orders mean the same thing without looking like it.
enum Window {
    Groups(usize),
    Tokens {
        budget: usize,
        counter: Box<dyn TokenCounter>,
    },
}

/// Hand-written because `Box<dyn TokenCounter>` is not `Debug` and requiring
/// it of every counter would buy nothing: the budget is the part worth
/// printing.
impl std::fmt::Debug for Window {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Groups(groups) => formatter.debug_tuple("Groups").field(groups).finish(),
            Self::Tokens { budget, .. } => formatter
                .debug_struct("Tokens")
                .field("budget", budget)
                .finish_non_exhaustive(),
        }
    }
}

/// The conversation in this process, and nowhere else.
///
/// A plain vector, an optional window and an optional summarizer. There is no
/// lock, because a [`crate::Conversation`] owns its backend outright, so
/// [`Storage`] takes `&mut self` and nothing here needs interior mutability.
///
/// Lost when the value is dropped, which makes it the right choice for a
/// short-lived process or a test and the wrong one for anything that has to
/// survive a restart. Persisting is a different backend, and writing one needs
/// nothing from this crate beyond [`Storage`], since [`Message`] already
/// derives `Serialize` and `Deserialize`.
#[derive(Debug, Default)]
pub struct InMemoryStorage {
    messages: Vec<Message>,
    window: Option<Window>,
    summarizer: Option<Summarizer>,
    summary: Option<Summary>,
}

/// The cached summary, and how many leading turn groups it stands for.
///
/// Turns are only ever appended, so the dropped part is always the oldest
/// groups and the same count means the same turns. That count is the whole
/// cache key.
#[derive(Debug)]
struct Summary {
    covers: usize,
    text: String,
}

/// Heads the summary message, so the model reads it as an account of what came
/// before rather than as something the user just said.
const SUMMARY_PREFIX: &str = "Summary of the earlier conversation:\n\n";

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
    ///
    /// The last window set is the one used.
    pub fn window(mut self, groups: usize) -> Self {
        self.window = Some(Window::Groups(groups));
        self
    }

    /// Send only the most recent turn groups fitting an estimated token
    /// budget, plus pinned turns.
    ///
    /// The rule is [`crate::window_by_tokens`], applied inside
    /// [`load`](crate::Storage::load), so nothing is discarded and
    /// [`InMemoryStorage::messages`] still returns everything.
    ///
    /// Pass [`crate::HeuristicCounter`] for a byte-length estimate with no
    /// dependency, or a closure wrapping a real tokenizer.
    ///
    /// The budget covers this transcript only. The agent's system instruction
    /// and every tool schema are added after this returns, and the model's
    /// reply comes back on top of both, so subtract all three from the model's
    /// context limit when choosing a budget, and leave a margin because
    /// [`crate::HeuristicCounter`] undercounts code and JSON.
    ///
    /// Sets the same field [`InMemoryStorage::window`] does, so the last of
    /// the two you call is the one that applies.
    ///
    /// ```
    /// use freyja::{InMemoryStorage, InputContent, Message};
    ///
    /// // A closure is a counter, so a real tokenizer needs no struct.
    /// let storage = InMemoryStorage::new().window_by_tokens(8_000, |message: &Message| {
    ///     message
    ///         .content
    ///         .iter()
    ///         .map(|part| match part {
    ///             InputContent::Text(text) => text.len() / 3,
    ///             _ => 8,
    ///         })
    ///         .sum()
    /// });
    /// # let _ = storage;
    /// ```
    pub fn window_by_tokens(mut self, budget: usize, counter: impl TokenCounter + 'static) -> Self {
        self.window = Some(Window::Tokens {
            budget,
            counter: Box::new(counter),
        });
        self
    }

    /// Summarize the turns the window drops, instead of losing them.
    ///
    /// Only turns a window drops are summarized, so this needs
    /// [`InMemoryStorage::window`] or [`InMemoryStorage::window_by_tokens`].
    /// Without one nothing is dropped and nothing is summarized.
    ///
    /// Inside [`load`](crate::Storage::load), the dropped turns, minus pinned
    /// ones, go to `summarizer`, and the result is sent as one user message
    /// after the pinned turns and before the turns still in view.
    /// [`InMemoryStorage::messages`] still returns every raw turn, and
    /// [`InMemoryStorage::summary`] returns the text last sent.
    ///
    /// Each summary is one extra model call, so it is made at a deeper cut
    /// than the window needs: half the groups for [`InMemoryStorage::window`],
    /// half the budget for [`InMemoryStorage::window_by_tokens`]. That leaves
    /// room for the next several turns, and the summary is reused until the
    /// window needs to drop more than it covers. A window of only a few groups
    /// has little room to give, and summarizes again on most turns.
    ///
    /// A new summary is built from the raw turns, not from the previous
    /// summary, so detail does not decay, at the price of a longer
    /// summarizing input as the conversation grows. The summary rides on top
    /// of a token budget rather than inside it.
    ///
    /// If the summarizing call fails, `load` sends the plain window and the
    /// conversation carries on.
    pub fn summarize(mut self, summarizer: Summarizer) -> Self {
        self.summarizer = Some(summarizer);
        self
    }

    /// The summary last sent in place of the dropped turns, if there is one.
    pub fn summary(&self) -> Option<&str> {
        self.summary.as_ref().map(|summary| summary.text.as_str())
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

    /// What the window alone sends, with no summary.
    fn windowed(&self) -> Vec<Message> {
        match &self.window {
            Some(Window::Groups(groups)) => {
                crate::transcript::window_by_groups(&self.messages, *groups)
            }
            Some(Window::Tokens { budget, counter }) => {
                crate::transcript::window_by_tokens(&self.messages, *budget, counter.as_ref())
            }
            None => self.messages.clone(),
        }
    }
}

impl Storage for InMemoryStorage {
    fn load(&mut self) -> StorageFuture<'_, Vec<Message>> {
        Box::pin(async move {
            let Some(summarizer) = &self.summarizer else {
                return Ok(self.windowed());
            };

            let (pinned, groups) = crate::transcript::split(&self.messages);
            // The cut the window needs, and the deeper one a new summary is
            // made at, half the window, so one summary serves several turns.
            let (needed, deeper) = match &self.window {
                Some(Window::Groups(keep)) => (
                    crate::transcript::cut_by_groups(&groups, *keep),
                    crate::transcript::cut_by_groups(&groups, keep.div_ceil(2)),
                ),
                Some(Window::Tokens { budget, counter }) => (
                    crate::transcript::cut_by_tokens(&pinned, &groups, *budget, counter.as_ref()),
                    crate::transcript::cut_by_tokens(
                        &pinned,
                        &groups,
                        budget / 2,
                        counter.as_ref(),
                    ),
                ),
                None => (0, 0),
            };
            if needed == 0 {
                return Ok(self.windowed());
            }

            // A summary covering at least what the window needs dropped still
            // serves. It may cover more than the window would drop, and that
            // difference is the room it made.
            let cached = self
                .summary
                .as_ref()
                .filter(|summary| summary.covers >= needed)
                .map(|summary| (summary.covers, summary.text.clone()));
            let (from, text) = match cached {
                Some(hit) => hit,
                None => {
                    // Pinned turns are kept, not summarized: the window rescues
                    // them out of the groups it drops.
                    let dropped: Vec<Message> = groups[..deeper]
                        .iter()
                        .flat_map(|group| group.iter())
                        .filter(|message| !matches!(message.role, Role::System | Role::Developer))
                        .cloned()
                        .collect();
                    match summarizer.summarize(&dropped).await {
                        Ok(text) => {
                            self.summary = Some(Summary {
                                covers: deeper,
                                text: text.clone(),
                            });
                            (deeper, text)
                        }
                        // A summary improves on the window and is never a
                        // condition for it, so the conversation goes on
                        // without one.
                        Err(_) => return Ok(self.windowed()),
                    }
                }
            };

            // After the pinned and rescued turns and before the groups still
            // in view, so it reads as what came before them.
            let kept: usize = groups[from..].iter().map(|group| group.len()).sum();
            let mut messages = crate::transcript::reassemble(pinned, &groups, from);
            messages.insert(
                messages.len() - kept,
                Message::text(Role::User, format!("{SUMMARY_PREFIX}{text}")),
            );
            Ok(messages)
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
            // A summary of a conversation that no longer exists would
            // otherwise open the next one.
            self.summary = None;
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
