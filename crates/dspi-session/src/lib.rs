//! Orchestration: connect, discover what the device can do, read state.
//!
//! No rendering happens here, which is what lets the TUI and the one-shot CLI
//! share every behaviour rather than reimplementing it.

pub mod autoeq;
pub mod filterfile;
pub mod preset_file;
pub mod probe;
pub mod surfaces;
pub mod write;

pub use probe::{Capabilities, Meters, probe, read_bulk, read_meters};
pub use write::{Crosspoint, JournalEntry, Outcome, OutputStrip, Session, WriteError};
