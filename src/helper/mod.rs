//! Optional rules a backend can apply to a transcript, or ignore.

mod window;

// `pub(crate)` is the tightest visibility that compiles here. The only caller
// is `crate::builtin::memory`, and `pub(in crate::helper)` would not reach it.
pub(crate) use window::{cut_by_groups, cut_by_tokens, reassemble, split};
pub use window::{window_by_groups, window_by_tokens};
