//! The backends and counters Freyja ships ready to use.

mod heuristic;
mod memory;

pub use heuristic::HeuristicCounter;
pub use memory::InMemoryStorage;
