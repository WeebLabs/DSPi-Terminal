//! Orchestration: connect, discover what the device can do, read state.
//!
//! No rendering happens here, which is what lets the TUI and the one-shot CLI
//! share every behaviour rather than reimplementing it.

pub mod probe;
pub mod write;

pub use probe::{Capabilities, Meters, probe, read_bulk, read_meters};
pub use write::{JournalEntry, Outcome, Session, WriteError};
