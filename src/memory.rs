//! What the model sees of a conversation, decided apart from where the
//! conversation is kept.

use crate::helper::{cut_by_groups, cut_by_tokens, reassemble, split};
use crate::{
    Message, Role, Storage, StorageError, Summarizer, TokenCounter, window_by_groups,
    window_by_tokens,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

/// What a [`Memory`] failed with.
pub type MemoryError = Box<dyn std::error::Error + Send + Sync>;

/// The future returned by every [`Memory`] method.
pub type MemoryFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, MemoryError>> + Send + 'a>>;

/// The state key [`Summarize`] keeps its summary under.
pub(crate) const SUMMARY_KEY: &str = "freyja.summary";

/// Heads the summary message, so the model reads it as an account of what
/// came before rather than as something the user just said.
const SUMMARY_PREFIX: &str = "Summary of the earlier conversation:\n\n";

/// Decides what the model sees of a conversation on each send.
///
/// Set on an agent with [`crate::Agent::memory`]. It works with any
/// [`Storage`]: the backend keeps every turn, and a memory chooses which of
/// them, or what in their place, reaches the model. What a memory keeps
/// between sends goes in the conversation's state slot through [`State`],
/// never in the memory value, so one value serves every conversation.
///
/// Boxed rather than `async fn` in the trait, for the reason
/// [`crate::ToolFuture`] gives.
pub trait Memory: Send + Sync {
    /// What to send of `messages`, the whole stored conversation, oldest
    /// first. The caller's new turn is added after this returns.
    fn recall<'a>(
        &'a self,
        messages: &'a [Message],
        state: &'a mut State<'_>,
    ) -> MemoryFuture<'a, Vec<Message>>;

    /// Summarizes the older part of `messages` now, keeping the newest `keep`
    /// turn groups as they are, or this memory's own choice when `keep` is
    /// `None`. The next [`Memory::recall`] sends the summary in their place.
    ///
    /// The default refuses: only a memory that summarizes can compact.
    fn compact<'a>(
        &'a self,
        messages: &'a [Message],
        state: &'a mut State<'_>,
        keep: Option<usize>,
    ) -> MemoryFuture<'a, ()> {
        let _ = (messages, state, keep);
        Box::pin(async {
            Err("this memory does not summarize, so it cannot compact; \
                 build one with Window::summarize"
                .into())
        })
    }
}

/// A conversation's state slot, as a [`Memory`] sees it.
///
/// Reads come from [`Storage::state`] first and fall back to a cache in this
/// process, which holds only what a backend keeping no state could not keep.
/// So one summary serves many sends within one [`crate::Conversation`]
/// either way, a backend that keeps state lets it outlive the process, and a
/// clear made through another handle on a shared backend that keeps state is
/// seen at once.
pub struct State<'a> {
    storage: &'a mut dyn Storage,
    cache: Cache<'a>,
}

enum Cache<'a> {
    Owned(HashMap<String, Value>),
    Borrowed(&'a mut HashMap<String, Value>),
}

impl<'a> State<'a> {
    /// A handle over `storage` with an empty cache of its own.
    ///
    /// What a memory's own tests call. A [`crate::Conversation`] passes one
    /// whose cache lives as long as the conversation does.
    pub fn new(storage: &'a mut dyn Storage) -> Self {
        Self {
            storage,
            cache: Cache::Owned(HashMap::new()),
        }
    }

    pub(crate) fn cached(
        storage: &'a mut dyn Storage,
        cache: &'a mut HashMap<String, Value>,
    ) -> Self {
        Self {
            storage,
            cache: Cache::Borrowed(cache),
        }
    }

    fn map(&mut self) -> &mut HashMap<String, Value> {
        match &mut self.cache {
            Cache::Owned(map) => map,
            Cache::Borrowed(map) => map,
        }
    }

    /// The value under `key`, from the backend, or else from this
    /// conversation's cache when the backend keeps no state.
    pub async fn get(&mut self, key: &str) -> Result<Option<Value>, StorageError> {
        if let Some(value) = self.storage.state(key).await? {
            return Ok(Some(value));
        }
        Ok(self.map().get(key).cloned())
    }

    /// Stores `value` under `key` in the backend. If reading it back finds
    /// nothing, the backend keeps no state, and the value goes in this
    /// conversation's cache instead.
    pub async fn set(&mut self, key: &str, value: Value) -> Result<(), StorageError> {
        self.storage.set_state(key, value.clone()).await?;
        if self.storage.state(key).await?.is_some() {
            self.map().remove(key);
        } else {
            self.map().insert(key.to_string(), value);
        }
        Ok(())
    }
}

/// Sends only the most recent part of a conversation, plus pinned turns.
///
/// A group is a message, except that an assistant turn requesting tools and
/// the results answering it are one group, so an exchange costs two groups
/// without tools and three with them. Pinned turns (`System` and
/// `Developer`) are always sent.
pub struct Window {
    rule: Rule,
}

enum Rule {
    Groups(usize),
    Tokens {
        budget: usize,
        counter: Box<dyn TokenCounter + Sync>,
    },
}

/// Hand-written because a boxed counter is not `Debug`. The budget is the
/// part worth printing.
impl std::fmt::Debug for Window {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.rule {
            Rule::Groups(groups) => formatter
                .debug_tuple("Window::groups")
                .field(groups)
                .finish(),
            Rule::Tokens { budget, .. } => formatter
                .debug_struct("Window::tokens")
                .field("budget", budget)
                .finish_non_exhaustive(),
        }
    }
}

impl Window {
    /// The most recent `keep` turn groups. The rule is
    /// [`crate::window_by_groups`].
    pub fn groups(keep: usize) -> Self {
        Self {
            rule: Rule::Groups(keep),
        }
    }

    /// The most recent turn groups fitting an estimated token `budget`. The
    /// rule is [`crate::window_by_tokens`].
    ///
    /// The budget covers the stored conversation only. The agent's system
    /// instruction and every tool schema are added after it, and the reply
    /// comes on top, so subtract all three from the model's context limit and
    /// leave a margin, since [`crate::HeuristicCounter`] undercounts code and
    /// JSON.
    pub fn tokens(budget: usize, counter: impl TokenCounter + Sync + 'static) -> Self {
        Self {
            rule: Rule::Tokens {
                budget,
                counter: Box::new(counter),
            },
        }
    }

    /// Summarizes what this window drops instead of losing it.
    pub fn summarize(self, summarizer: Summarizer) -> Summarize {
        Summarize {
            window: self,
            summarizer,
        }
    }

    fn apply(&self, messages: &[Message]) -> Vec<Message> {
        match &self.rule {
            Rule::Groups(keep) => window_by_groups(messages, *keep),
            Rule::Tokens { budget, counter } => {
                window_by_tokens(messages, *budget, counter.as_ref())
            }
        }
    }

    /// How many leading groups the window must drop, and the deeper cut at
    /// half the window a new summary is made at, so one summary serves
    /// several sends.
    fn cuts(&self, pinned: &[&Message], groups: &[&[Message]]) -> (usize, usize) {
        match &self.rule {
            Rule::Groups(keep) => (
                cut_by_groups(groups, *keep),
                cut_by_groups(groups, keep.div_ceil(2)),
            ),
            Rule::Tokens { budget, counter } => (
                cut_by_tokens(pinned, groups, *budget, counter.as_ref()),
                cut_by_tokens(pinned, groups, budget / 2, counter.as_ref()),
            ),
        }
    }
}

impl Memory for Window {
    fn recall<'a>(
        &'a self,
        messages: &'a [Message],
        _state: &'a mut State<'_>,
    ) -> MemoryFuture<'a, Vec<Message>> {
        Box::pin(async move { Ok(self.apply(messages)) })
    }
}

/// A [`Window`] that sends a summary of what it drops.
///
/// Built by [`Window::summarize`]. Each summary is one extra model call, so
/// it is made at half the window, which leaves room for several sends, and
/// reused until the window needs to drop more than it covers. A new summary
/// is built from the raw turns, never from the previous summary. If the
/// summarizing call fails, the plain window is sent and the conversation
/// carries on.
///
/// The summary goes in as one user message after the pinned turns and before
/// the turns still in view. It is kept in the conversation's state slot, so a
/// backend that persists state reuses it after a restart.
#[derive(Debug)]
pub struct Summarize {
    window: Window,
    summarizer: Summarizer,
}

/// What is kept under [`SUMMARY_KEY`]: the summary, and how many leading
/// groups it stands for. Turns are only appended, so the same count means
/// the same turns.
#[derive(Serialize, Deserialize)]
struct Cached {
    covers: usize,
    text: String,
}

/// The non-pinned messages in the first `covers` groups. Pinned turns are
/// kept, not summarized.
fn older(groups: &[&[Message]], covers: usize) -> Vec<Message> {
    groups[..covers]
        .iter()
        .flat_map(|group| group.iter())
        .filter(|message| !matches!(message.role, Role::System | Role::Developer))
        .cloned()
        .collect()
}

impl Summarize {
    /// The stored summary, if there is one that still fits the conversation.
    /// One claiming more groups than are held came from somewhere else, such
    /// as a stale file, and is ignored.
    async fn stored(&self, state: &mut State<'_>, groups: usize) -> Option<Cached> {
        let value = state.get(SUMMARY_KEY).await.ok().flatten()?;
        serde_json::from_value::<Cached>(value)
            .ok()
            .filter(|cached| cached.covers <= groups)
    }
}

impl Memory for Summarize {
    fn recall<'a>(
        &'a self,
        messages: &'a [Message],
        state: &'a mut State<'_>,
    ) -> MemoryFuture<'a, Vec<Message>> {
        Box::pin(async move {
            let (pinned, groups) = split(messages);
            let (needed, deeper) = self.window.cuts(&pinned, &groups);

            let cached = self
                .stored(state, groups.len())
                .await
                .filter(|cached| cached.covers >= needed);
            let (from, text) = match cached {
                Some(cached) => (cached.covers, cached.text),
                None if needed == 0 => return Ok(self.window.apply(messages)),
                None => match self.summarizer.summarize(&older(&groups, deeper)).await {
                    Ok(text) => {
                        let record = Cached {
                            covers: deeper,
                            text: text.clone(),
                        };
                        // A summary that fails to store is made again on a
                        // later send, which costs a call and loses nothing.
                        if let Ok(value) = serde_json::to_value(&record) {
                            let _ = state.set(SUMMARY_KEY, value).await;
                        }
                        (deeper, text)
                    }
                    // A summary improves on the window and is never a
                    // condition for it.
                    Err(_) => return Ok(self.window.apply(messages)),
                },
            };

            let kept: usize = groups[from..].iter().map(|group| group.len()).sum();
            let mut sent = reassemble(pinned, &groups, from);
            sent.insert(
                sent.len() - kept,
                Message::text(Role::User, format!("{SUMMARY_PREFIX}{text}")),
            );
            Ok(sent)
        })
    }

    fn compact<'a>(
        &'a self,
        messages: &'a [Message],
        state: &'a mut State<'_>,
        keep: Option<usize>,
    ) -> MemoryFuture<'a, ()> {
        Box::pin(async move {
            let (pinned, groups) = split(messages);
            let covers = match keep {
                Some(keep) => groups.len().saturating_sub(keep),
                None => self.window.cuts(&pinned, &groups).1,
            };
            if covers == 0 {
                return Ok(());
            }
            let text = self.summarizer.summarize(&older(&groups, covers)).await?;
            state
                .set(SUMMARY_KEY, serde_json::to_value(Cached { covers, text })?)
                .await?;
            Ok(())
        })
    }
}
