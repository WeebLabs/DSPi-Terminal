//! Orchestration: connect, discover what the device can do, read state.
//!
//! No rendering happens here, which is what lets the TUI and the one-shot CLI
//! share every behaviour rather than reimplementing it.

pub mod autoeq;
pub mod filterfile;
pub mod notify;
pub mod pins;
pub mod preset_file;
pub mod probe;
pub mod state;
pub mod surfaces;
pub mod undo;
pub mod write;

pub use notify::{Event, Notification, Notifications, Source};
pub use pins::{PinClaim, PinConstraint, PinMap, PinRole};
pub use probe::{Capabilities, Meters, probe, read_bulk, read_meters};
pub use state::{Applied, DeviceState, PresetSnapshot};
pub use undo::Undone;
pub use write::{
    Core1Conflict, Crosspoint, EnableOutcome, JournalEntry, Outcome, OutputStrip, Session,
    WriteError,
};
