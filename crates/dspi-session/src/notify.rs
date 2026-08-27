//! The device's notification stream, protocol v2.
//!
//! The firmware pushes state changes on bulk endpoint `0x83`
//! (`notification_protocol_v2_spec.md`, `notify.h`). Most of them are
//! `PARAM_CHANGED`: a byte offset into `WireBulkParams`, a size, a source tag
//! and the new bytes, which the host copies straight into its bulk shadow. A
//! few are discrete events with their own small layouts. Every packet carries
//! a wrapping sequence number; a gap means the device dropped something and
//! the only safe recovery is a full re-read.
//!
//! [`Notifications`] runs the reader on its own thread and hands decoded
//! events to the application through a channel, so the interface never
//! blocks on the endpoint.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;
use std::time::Duration;

use dspi_transport::NotificationSource;

/// `NOTIFY_PROTOCOL_VERSION` (notify.h).
pub const PROTOCOL_VERSION: u8 = 2;

/// The largest field a `PARAM_CHANGED` can carry (notify.h); anything bigger
/// arrives as `BULK_INVALIDATED` instead.
pub const MAX_NOTIFIED_FIELD: usize = 52;

/// Event identifiers, `NOTIFY_EVT_*` (notify.h:25-79).
pub mod evt {
    pub const IDLE: u8 = 0x00;
    pub const MASTER_VOLUME: u8 = 0x01;
    pub const PARAM_CHANGED: u8 = 0x02;
    pub const BULK_INVALIDATED: u8 = 0x03;
    pub const PRESET_LOADED: u8 = 0x04;
    pub const INPUT_FORMAT: u8 = 0x05;
    pub const SIGGEN_STATE: u8 = 0x07;
    pub const ADAT_STATE: u8 = 0x08;
    pub const I2S_SLAVE_STATE: u8 = 0x09;
    pub const CS_IR_LEARN: u8 = 0x0A;
    pub const ADAT_INPUT_STATE: u8 = 0x0B;
}

/// Who made the change, `ParamSource` (notify.h:85-96). Unknown values are
/// kept rather than mapped, because the range grows with new transports and
/// "someone else changed it" is the right reading for all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Unknown,
    HostSet,
    BulkSet,
    Preset,
    Factory,
    /// A control surface: every GPIO dispatch.
    Gpio,
    Internal,
    /// The operating system's volume slider, over UAC1.
    Uac1,
    Uart,
    I2c,
    Other(u8),
}

impl Source {
    pub fn from_raw(b: u8) -> Self {
        match b {
            0 => Self::Unknown,
            1 => Self::HostSet,
            2 => Self::BulkSet,
            3 => Self::Preset,
            4 => Self::Factory,
            5 => Self::Gpio,
            6 => Self::Internal,
            7 => Self::Uac1,
            8 => Self::Uart,
            9 => Self::I2c,
            other => Self::Other(other),
        }
    }

    /// Our own writes come back as `HostSet`; everything else is news.
    pub fn is_ours(self) -> bool {
        matches!(self, Self::HostSet | Self::BulkSet)
    }

    /// The Console's wording for the echo line.
    pub fn describe(self) -> &'static str {
        match self {
            Self::Unknown | Self::Other(_) => "changed elsewhere",
            Self::HostSet | Self::BulkSet => "changed by this host",
            Self::Preset => "changed by a preset load",
            Self::Factory => "reset to factory defaults",
            Self::Gpio => "changed by a control surface",
            Self::Internal => "changed by the device",
            Self::Uac1 => "changed by the system volume",
            Self::Uart => "changed over UART",
            Self::I2c => "changed over I2C",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The keep-alive; never delivered to the application.
    Idle,
    /// The v1 legacy volume event, emitted alongside a `PARAM_CHANGED`.
    MasterVolume(f32),
    ParamChanged {
        /// `offsetof(WireBulkParams, field)`, plus `index * size` for arrays.
        offset: u16,
        source: Source,
        bytes: Vec<u8>,
    },
    BulkInvalidated {
        source: Source,
    },
    PresetLoaded {
        slot: u8,
    },
    /// Active input channels changed: 2, 4, 6 or 8.
    InputFormat {
        channels: u8,
    },
    SiggenState {
        state: u8,
        reason: u8,
        signal_type: u8,
        /// `0xFF` when not walking outputs.
        channel: u8,
    },
    AdatState {
        enabled: bool,
        active: bool,
        pin: u8,
    },
    I2sSlaveState {
        state: u8,
        /// Zero until locked.
        rate_hz: u32,
    },
    IrLearn {
        state: u8,
        protocol: u8,
        code: u32,
    },
    AdatInputState {
        state: u8,
        rate_hz: u32,
        clock_mode: u8,
    },
    /// An event id this build does not know; kept so it can be shown in the
    /// monitor rather than silently dropped.
    Unknown {
        id: u8,
        bytes: Vec<u8>,
    },
}

/// One decoded packet.
#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    pub seq: u8,
    pub event: Event,
    /// The sequence number skipped: something was dropped before this one,
    /// and the bulk shadow can no longer be trusted.
    pub lost: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DecodeError {
    #[error("notification protocol v{0}; this build speaks v2")]
    Version(u8),
    #[error("event 0x{id:02X} is {got} bytes, needs {need}")]
    Short { id: u8, got: usize, need: usize },
}

/// Decode one packet. A packet shorter than the common header is the idle
/// keep-alive (the firmware sends exactly one byte), never an error.
pub fn decode(p: &[u8]) -> Result<(u8, Event), DecodeError> {
    if p.len() < 4 {
        return Ok((0, Event::Idle));
    }
    // The v1 legacy volume packet is `[0x01, 0, 0, 0, f32]` with no version
    // byte at all; it is recognised by shape before the version check.
    if p[0] == evt::MASTER_VOLUME && p[1] == 0 && p[2] == 0 && p[3] == 0 && p.len() >= 8 {
        return Ok((0, Event::MasterVolume(f32_at(p, 4))));
    }
    if p[0] != PROTOCOL_VERSION {
        return Err(DecodeError::Version(p[0]));
    }
    let id = p[1];
    let seq = p[3];
    let need = |n: usize| {
        if p.len() < n {
            Err(DecodeError::Short {
                id,
                got: p.len(),
                need: n,
            })
        } else {
            Ok(())
        }
    };
    let event = match id {
        evt::IDLE => Event::Idle,
        evt::PARAM_CHANGED => {
            need(12)?;
            let offset = u16::from_le_bytes([p[4], p[5]]);
            let size = u16::from_le_bytes([p[6], p[7]]) as usize;
            let source = Source::from_raw(p[8]);
            need(12 + size)?;
            Event::ParamChanged {
                offset,
                source,
                bytes: p[12..12 + size].to_vec(),
            }
        }
        evt::BULK_INVALIDATED => {
            need(5)?;
            Event::BulkInvalidated {
                source: Source::from_raw(p[4]),
            }
        }
        evt::PRESET_LOADED => {
            need(5)?;
            Event::PresetLoaded { slot: p[4] }
        }
        evt::INPUT_FORMAT => {
            need(5)?;
            Event::InputFormat { channels: p[4] }
        }
        evt::SIGGEN_STATE => {
            need(8)?;
            Event::SiggenState {
                state: p[4],
                reason: p[5],
                signal_type: p[6],
                channel: p[7],
            }
        }
        evt::ADAT_STATE => {
            need(7)?;
            Event::AdatState {
                enabled: p[4] != 0,
                active: p[5] != 0,
                pin: p[6],
            }
        }
        evt::I2S_SLAVE_STATE => {
            need(9)?;
            Event::I2sSlaveState {
                state: p[4],
                rate_hz: u32::from_le_bytes([p[5], p[6], p[7], p[8]]),
            }
        }
        evt::CS_IR_LEARN => {
            need(12)?;
            Event::IrLearn {
                state: p[4],
                protocol: p[5],
                code: u32::from_le_bytes([p[8], p[9], p[10], p[11]]),
            }
        }
        evt::ADAT_INPUT_STATE => {
            need(10)?;
            Event::AdatInputState {
                state: p[4],
                rate_hz: u32::from_le_bytes([p[5], p[6], p[7], p[8]]),
                clock_mode: p[9],
            }
        }
        other => Event::Unknown {
            id: other,
            bytes: p[4..].to_vec(),
        },
    };
    Ok((seq, event))
}

fn f32_at(p: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([p[o], p[o + 1], p[o + 2], p[o + 3]])
}

/// Tracks the wrapping sequence counter and reports gaps.
#[derive(Debug, Default, Clone)]
pub struct SeqTracker {
    last: Option<u8>,
}

impl SeqTracker {
    /// Record `seq`; returns true when at least one packet was skipped.
    pub fn observe(&mut self, seq: u8) -> bool {
        let lost = match self.last {
            Some(prev) => seq != prev.wrapping_add(1),
            None => false,
        };
        self.last = Some(seq);
        lost
    }

    /// Forget the counter, after a full re-read has restored the shadow.
    pub fn reset(&mut self) {
        self.last = None;
    }
}

/// The reader thread and its channel.
pub struct Notifications {
    rx: Receiver<Notification>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    /// Set by the reader when the device went away.
    disconnected: Arc<AtomicBool>,
}

impl Notifications {
    /// How long one read waits before checking whether to stop. The device
    /// answers idle packets immediately, so this only matters on shutdown.
    pub const READ_TIMEOUT: Duration = Duration::from_millis(250);

    /// Start reading. Idle packets are dropped here; everything else is
    /// delivered with its sequence number and the loss flag.
    pub fn start(mut source: Box<dyn NotificationSource>) -> Self {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let disconnected = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let disc_flag = Arc::clone(&disconnected);
        let thread = std::thread::Builder::new()
            .name("dspi-notify".into())
            .spawn(move || {
                let mut seqs = SeqTracker::default();
                while !stop_flag.load(Ordering::Relaxed) {
                    let packet = match source.read(Self::READ_TIMEOUT) {
                        Ok(p) => p,
                        Err(dspi_transport::TransportError::Disconnected) => {
                            disc_flag.store(true, Ordering::Relaxed);
                            break;
                        }
                        // A stall or a transient error: give the bus a
                        // moment rather than spinning.
                        Err(_) => {
                            std::thread::sleep(Duration::from_millis(50));
                            continue;
                        }
                    };
                    if packet.is_empty() {
                        continue;
                    }
                    match decode(&packet) {
                        Ok((_, Event::Idle)) => {}
                        Ok((seq, event)) => {
                            let lost = seqs.observe(seq);
                            if tx.send(Notification { seq, event, lost }).is_err() {
                                break;
                            }
                        }
                        Err(_) => {
                            // A packet we cannot read is still a packet we
                            // missed; say so through the loss flag.
                            if tx
                                .send(Notification {
                                    seq: 0,
                                    event: Event::Unknown {
                                        id: 0xFF,
                                        bytes: packet,
                                    },
                                    lost: true,
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                }
            })
            .expect("spawn the notification reader");
        Self {
            rx,
            stop,
            thread: Some(thread),
            disconnected,
        }
    }

    /// Everything that has arrived since the last drain, oldest first.
    pub fn drain(&self) -> Vec<Notification> {
        let mut out = Vec::new();
        while let Ok(n) = self.rx.try_recv() {
            out.push(n);
        }
        out
    }

    /// Block for the next notification, up to `timeout`.
    pub fn next(&self, timeout: Duration) -> Option<Notification> {
        self.rx.recv_timeout(timeout).ok()
    }

    pub fn is_disconnected(&self) -> bool {
        self.disconnected.load(Ordering::Relaxed)
    }
}

impl Drop for Notifications {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dspi_transport::MockTransport;
    use dspi_transport::Transport;

    fn param(seq: u8, offset: u16, source: u8, bytes: &[u8]) -> Vec<u8> {
        let mut p = vec![2, evt::PARAM_CHANGED, 0, seq];
        p.extend(offset.to_le_bytes());
        p.extend((bytes.len() as u16).to_le_bytes());
        p.extend([source, 0, 0, 0]);
        p.extend(bytes);
        p
    }

    #[test]
    fn a_param_change_carries_offset_source_and_bytes() {
        // user_volume.user_volume_db at 4748 (bulk_params.h), from UAC1.
        let p = param(7, 4748, 7, &(-12.0f32).to_le_bytes());
        let (seq, e) = decode(&p).unwrap();
        assert_eq!(seq, 7);
        assert_eq!(
            e,
            Event::ParamChanged {
                offset: 4748,
                source: Source::Uac1,
                bytes: (-12.0f32).to_le_bytes().to_vec()
            }
        );
    }

    #[test]
    fn discrete_events_decode_by_id() {
        assert_eq!(
            decode(&[2, 3, 0, 9, 3, 0, 0, 0]).unwrap().1,
            Event::BulkInvalidated {
                source: Source::Preset
            }
        );
        assert_eq!(
            decode(&[2, 4, 0, 10, 5, 0, 0, 0]).unwrap().1,
            Event::PresetLoaded { slot: 5 }
        );
        assert_eq!(
            decode(&[2, 5, 0, 11, 8, 0, 0, 0]).unwrap().1,
            Event::InputFormat { channels: 8 }
        );
        assert_eq!(
            decode(&[2, 7, 0, 12, 1, 0, 4, 0xFF]).unwrap().1,
            Event::SiggenState {
                state: 1,
                reason: 0,
                signal_type: 4,
                channel: 0xFF
            }
        );
        assert_eq!(
            decode(&[2, 8, 0, 13, 1, 1, 12, 0]).unwrap().1,
            Event::AdatState {
                enabled: true,
                active: true,
                pin: 12
            }
        );
        assert_eq!(
            decode(&[2, 9, 0, 14, 2, 0x80, 0xBB, 0, 0]).unwrap().1,
            Event::I2sSlaveState {
                state: 2,
                rate_hz: 48_000
            }
        );
        assert_eq!(
            decode(&[2, 0x0A, 0, 15, 2, 1, 0, 0, 0x78, 0x56, 0x34, 0x12])
                .unwrap()
                .1,
            Event::IrLearn {
                state: 2,
                protocol: 1,
                code: 0x1234_5678
            }
        );
        assert_eq!(
            decode(&[2, 0x0B, 0, 16, 2, 0x44, 0xAC, 0, 0, 1]).unwrap().1,
            Event::AdatInputState {
                state: 2,
                rate_hz: 44_100,
                clock_mode: 1
            }
        );
        assert_eq!(
            decode(&[2, 0x42, 0, 17, 9]).unwrap().1,
            Event::Unknown {
                id: 0x42,
                bytes: vec![9]
            }
        );
    }

    #[test]
    fn idle_and_legacy_packets_are_recognised_by_shape() {
        assert_eq!(decode(&[0]).unwrap().1, Event::Idle);
        assert_eq!(decode(&[]).unwrap().1, Event::Idle);
        let mut legacy = vec![1, 0, 0, 0];
        legacy.extend((-20.0f32).to_le_bytes());
        assert_eq!(decode(&legacy).unwrap().1, Event::MasterVolume(-20.0));
        assert_eq!(decode(&[3, 2, 0, 0]).unwrap_err(), DecodeError::Version(3));
        assert!(matches!(
            decode(&[2, 2, 0, 0, 1, 2]).unwrap_err(),
            DecodeError::Short { .. }
        ));
    }

    #[test]
    fn sequence_gaps_are_reported_and_wrap_is_not_a_gap() {
        let mut t = SeqTracker::default();
        assert!(!t.observe(254));
        assert!(!t.observe(255));
        assert!(!t.observe(0), "255 to 0 is the wrap, not a loss");
        assert!(t.observe(2), "1 was skipped");
        t.reset();
        assert!(!t.observe(9));
    }

    #[test]
    fn the_reader_delivers_events_and_flags_a_loss() {
        let mock = MockTransport::new();
        mock.push_notification(param(1, 4748, 5, &[0, 0, 0, 0]));
        mock.push_notification(vec![0]);
        mock.push_notification(vec![2, 3, 0, 3, 1, 0, 0, 0]);
        let n = Notifications::start(mock.notifications().unwrap());
        let first = n.next(Duration::from_secs(2)).expect("first");
        assert_eq!(first.seq, 1);
        assert!(!first.lost);
        assert!(matches!(
            first.event,
            Event::ParamChanged {
                source: Source::Gpio,
                ..
            }
        ));
        let second = n.next(Duration::from_secs(2)).expect("second");
        assert_eq!(second.seq, 3);
        assert!(second.lost, "seq 2 never arrived");
        assert!(matches!(second.event, Event::BulkInvalidated { .. }));
        assert!(n.next(Duration::from_millis(50)).is_none());
        assert!(!n.is_disconnected());
    }

    #[test]
    fn a_disconnect_stops_the_reader_and_is_visible() {
        let mock = MockTransport::new();
        mock.push_notification(dspi_transport::mock::MockNotifications::DISCONNECT.to_vec());
        let n = Notifications::start(mock.notifications().unwrap());
        std::thread::sleep(Duration::from_millis(50));
        assert!(n.is_disconnected());
    }

    #[test]
    fn sources_describe_themselves_in_the_consoles_words() {
        assert_eq!(Source::from_raw(7), Source::Uac1);
        assert_eq!(Source::from_raw(42), Source::Other(42));
        assert!(Source::HostSet.is_ours());
        assert!(!Source::Gpio.is_ours());
        assert_eq!(Source::Gpio.describe(), "changed by a control surface");
    }
}
