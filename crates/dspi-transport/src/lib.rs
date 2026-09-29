//! Transport to a DSPi device.
//!
//! The [`Transport`] trait is the seam that keeps a future remote backend from
//! being a rewrite: it is the only place a `bmRequestType` is constructed, and
//! `wIndex` is not a parameter at all, so no call site can get either wrong.
//!
//! Only [`UsbTransport`] ships today. [`MockTransport`] backs the tests,
//! including the awkward cases (stalls, timeouts, a simulated flash blackout)
//! that are impractical to provoke on real hardware.

use std::time::Duration;

pub mod mock;
pub mod usb;

pub use mock::MockTransport;
pub use usb::{UsbTransport, list_devices};

/// How long to wait on a single control transfer before treating it as timed out.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(1000);

/// Back-off between retries while the device is inside a flash or pipeline-reset
/// blackout. A single flash sector costs up to ~45 ms and a preset save touches
/// two, so this is sized to clear a multi-sector write comfortably.
pub const BUSY_BACKOFF: Duration = Duration::from_millis(150);

/// How many times to retry a transfer that failed inside a busy window.
pub const BUSY_RETRIES: usize = 4;

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("no DSPi device found")]
    NotFound,

    #[error("no DSPi with serial {0}")]
    SerialNotFound(String),

    #[error("{}", permission_help())]
    PermissionDenied,

    #[error("the device refused the request (stalled): opcode 0x{opcode:02X}")]
    Stalled { opcode: u8 },

    #[error("timed out after {retries} retries waiting for opcode 0x{opcode:02X}")]
    TimedOut { opcode: u8, retries: usize },

    #[error("device disconnected")]
    Disconnected,

    #[error("short read for opcode 0x{opcode:02X}: wanted {wanted} bytes, got {got}")]
    ShortRead {
        opcode: u8,
        wanted: usize,
        got: usize,
    },

    #[error("usb error: {0}")]
    Usb(String),

    /// The device answered, but speaks a protocol this build does not. The
    /// message names both sides and what to update; it is shown as is.
    #[error("{0}")]
    Incompatible(String),
}

/// Platform-specific advice for a permission failure.
///
/// The cause differs completely per platform, so a single message would be
/// wrong for two of the three. This is the first thing a blocked user reads, so
/// it has to name the actual fix.
fn permission_help() -> String {
    if cfg!(target_os = "linux") {
        "permission denied opening the DSPi. This is almost always a missing \
         udev rule; run `dspi --doctor` for the exact fix."
            .into()
    } else if cfg!(target_os = "macos") {
        "could not claim the DSPi vendor interface. Another application is \
         probably holding it: quit DSPi Console (or any other DSPi tool) and \
         try again."
            .into()
    } else {
        "could not claim the DSPi vendor interface. Another application may be \
         holding it, or the WinUSB driver is not bound; run `dspi --doctor`."
            .into()
    }
}

pub type Result<T> = std::result::Result<T, TransportError>;

/// Identity of a connected device, as shown in the device picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceDescriptor {
    pub serial: String,
    pub bus_id: String,
    pub address: u8,
}

impl DeviceDescriptor {
    /// The short form the Console shows, so the same device reads the same way
    /// in both apps.
    pub fn short_name(&self) -> String {
        let s = &self.serial;
        if s.len() <= 8 {
            s.clone()
        } else {
            s[s.len() - 8..].to_string()
        }
    }
}

/// A channel over which vendor commands reach a device.
///
/// Implementors handle the wire; they do not interpret payloads. In particular
/// they must not retry on their own beyond the busy-window protocol, because
/// blind retries of a mutating command are not safe for every opcode.
pub trait Transport: Send {
    /// IN transfer. Used both for reads and for the "write-as-read" opcodes that
    /// mutate state while being dispatched on the IN path.
    fn control_in(&mut self, opcode: u8, value: u16, len: u16) -> Result<Vec<u8>>;

    /// IN transfer whose reply may legitimately be shorter than asked.
    ///
    /// A few replies are sized by the device rather than the request:
    /// `GET_PLATFORM` answers 4, 6 or 7 bytes depending on the firmware's age,
    /// the subharm and limiter meters one entry per output, and the analyser's
    /// all-channel read one frame per live channel. For those, a short reply is
    /// the answer, not an error; everything else goes through
    /// [`Transport::control_in`], which refuses one.
    ///
    /// The default is for scripted test transports that always answer in full.
    fn control_in_upto(&mut self, opcode: u8, value: u16, max_len: u16) -> Result<Vec<u8>> {
        self.control_in(opcode, value, max_len)
    }

    /// OUT transfer carrying a payload.
    fn control_out(&mut self, opcode: u8, value: u16, data: &[u8]) -> Result<()>;

    fn descriptor(&self) -> &DeviceDescriptor;

    /// Largest single control transfer this backend can perform.
    ///
    /// WinUSB caps transfers at 4 KB, and the bulk params packet is 6136 bytes,
    /// so on Windows the chunked opcodes are mandatory rather than an
    /// optimisation. Callers consult this instead of testing the platform.
    fn max_transfer(&self) -> usize {
        4096
    }

    /// A reader for the device's notification endpoint, if this backend has
    /// one. It is independent of the control path so it can live on its own
    /// thread and block without holding the transport.
    fn notifications(&self) -> Option<Box<dyn NotificationSource>> {
        None
    }
}

/// The firmware's notification stream: bulk IN endpoint `0x83`, 64-byte
/// packets. Events are sent at once. When the device has nothing to say the
/// endpoint NAKs, and a one-byte idle packet goes out only after 100 ms of
/// quiet (`NOTIFY_IDLE_KEEPALIVE_US`, usb_audio.c:1064-1080 at 557bce7), so a
/// read may block for about 100 ms before anything arrives.
pub trait NotificationSource: Send {
    /// One packet, sized by what actually arrived. A timeout is `Ok(vec![])`
    /// rather than an error: silence is normal.
    fn read(&mut self, timeout: Duration) -> Result<Vec<u8>>;
}

/// Run a transfer that may land inside a deferred write's busy window.
///
/// Flash writes and pipeline resets briefly disable the USB control IRQ, so a
/// transfer issued during that window legitimately times out or stalls. Backing
/// off and retrying is correct behaviour, not papering over a fault.
///
/// Only use this for **idempotent** operations: reads, and confirming a write
/// landed. Retrying a mutating command blindly could apply it twice.
pub fn with_busy_retry<T>(mut attempt: impl FnMut() -> Result<T>, opcode: u8) -> Result<T> {
    let mut last_err = None;
    for i in 0..=BUSY_RETRIES {
        match attempt() {
            Ok(v) => return Ok(v),
            Err(e @ (TransportError::TimedOut { .. } | TransportError::Stalled { .. })) => {
                last_err = Some(e);
                if i < BUSY_RETRIES {
                    std::thread::sleep(BUSY_BACKOFF);
                }
            }
            // A disconnect is not a busy window; re-acquisition is the caller's job.
            Err(e) => return Err(e),
        }
    }
    Err(last_err.unwrap_or(TransportError::TimedOut {
        opcode,
        retries: BUSY_RETRIES,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_name_matches_the_console_convention() {
        let d = DeviceDescriptor {
            serial: "E6614C311B8B4E3A".into(),
            bus_id: "001".into(),
            address: 7,
        };
        assert_eq!(d.short_name(), "1B8B4E3A");
    }

    #[test]
    fn short_name_handles_a_short_serial() {
        let d = DeviceDescriptor {
            serial: "ABC".into(),
            bus_id: "001".into(),
            address: 1,
        };
        assert_eq!(d.short_name(), "ABC");
    }

    #[test]
    fn busy_retry_succeeds_once_the_blackout_ends() {
        let mut calls = 0;
        let r = with_busy_retry(
            || {
                calls += 1;
                if calls < 3 {
                    Err(TransportError::Stalled { opcode: 0x90 })
                } else {
                    Ok(calls)
                }
            },
            0x90,
        );
        assert_eq!(r.unwrap(), 3);
    }

    #[test]
    fn busy_retry_gives_up_rather_than_looping_forever() {
        let mut calls = 0;
        let r: Result<()> = with_busy_retry(
            || {
                calls += 1;
                Err(TransportError::Stalled { opcode: 0x90 })
            },
            0x90,
        );
        assert!(r.is_err());
        assert_eq!(calls, BUSY_RETRIES + 1);
    }

    /// A disconnect must surface immediately: retrying wastes the user's time
    /// and hides the real cause.
    #[test]
    fn busy_retry_does_not_mask_a_disconnect() {
        let mut calls = 0;
        let r: Result<()> = with_busy_retry(
            || {
                calls += 1;
                Err(TransportError::Disconnected)
            },
            0x90,
        );
        assert!(matches!(r, Err(TransportError::Disconnected)));
        assert_eq!(calls, 1, "a disconnect must not be retried");
    }
}
