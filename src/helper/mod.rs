//! Optional rules a backend can apply to a transcript, or ignore.

mod window;

pub(crate) use window::{cut_by_groups, cut_by_tokens, reassemble, split};
pub use window::{window_by_groups, window_by_tokens};
