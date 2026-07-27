//! A scripted device, for testing without hardware.
//!
//! This exists mainly to make the *awkward* paths testable: a flash blackout
//! that stalls for a few transfers and then recovers, a device that reports a
//! wire version we do not implement, a short read, a mid-session disconnect.
//! Those are exactly the behaviours that are hard to provoke on demand with real
//! hardware and expensive to get wrong in the field.

use std::collections::HashMap;

use crate::{DeviceDescriptor, Result, Transport, TransportError};

/// What the mock should do for a given opcode.
#[derive(Debug, Clone)]
pub enum Reply {
    /// Answer with these bytes.
    Data(Vec<u8>),
    /// A window into a larger buffer, sliced at the byte offset carried in
    /// `wValue`. Models the chunked bulk opcodes (`0xA2` / `0xA3`), where the
    /// host walks a 5944-byte packet in transfer-sized pieces. Without this the
    /// mock would hand back the same prefix every time and a chunking bug would
    /// sail through the tests.
    Window(Vec<u8>),
    /// Refuse, as the firmware does for an unknown opcode or a bad index.
    Stall,
    /// Stall `n` more times, then fall through to the next reply. Models the
    /// flash blackout: the device is alive but its control IRQ is disabled.
    BusyThen(usize, Box<Reply>),
    /// The device went away.
    Disconnect,
}

/// One recorded exchange, for asserting what the code under test actually sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exchange {
    pub direction: Direction,
    pub opcode: u8,
    pub value: u16,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    In,
    Out,
}

#[derive(Default)]
pub struct MockTransport {
    replies: HashMap<u8, Reply>,
    pub log: Vec<Exchange>,
    descriptor: Option<DeviceDescriptor>,
    max_transfer: Option<usize>,
}

impl MockTransport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reply(mut self, opcode: u8, reply: Reply) -> Self {
        self.replies.insert(opcode, reply);
        self
    }

    /// Answer an opcode with fixed bytes.
    pub fn data(self, opcode: u8, bytes: impl Into<Vec<u8>>) -> Self {
        self.reply(opcode, Reply::Data(bytes.into()))
    }

    /// Answer an opcode by slicing a larger buffer at the offset in `wValue`,
    /// the way the chunked bulk opcodes behave.
    pub fn window(self, opcode: u8, bytes: impl Into<Vec<u8>>) -> Self {
        self.reply(opcode, Reply::Window(bytes.into()))
    }

    /// Constrain the transfer size, to exercise the chunked path that Windows
    /// forces on us without needing Windows.
    pub fn with_max_transfer(mut self, max: usize) -> Self {
        self.max_transfer = Some(max);
        self
    }

    pub fn with_descriptor(mut self, d: DeviceDescriptor) -> Self {
        self.descriptor = Some(d);
        self
    }

    /// Every opcode the code under test touched, in order.
    pub fn opcodes_seen(&self) -> Vec<u8> {
        self.log.iter().map(|e| e.opcode).collect()
    }

    fn resolve(&mut self, opcode: u8, value: u16) -> Result<Vec<u8>> {
        match self.replies.get_mut(&opcode) {
            None => Err(TransportError::Stalled { opcode }),
            Some(Reply::Stall) => Err(TransportError::Stalled { opcode }),
            Some(Reply::Disconnect) => Err(TransportError::Disconnected),
            Some(Reply::Data(d)) => Ok(d.clone()),
            Some(Reply::Window(all)) => {
                let start = (value as usize).min(all.len());
                Ok(all[start..].to_vec())
            }
            Some(Reply::BusyThen(remaining, inner)) => {
                if *remaining > 0 {
                    *remaining -= 1;
                    Err(TransportError::Stalled { opcode })
                } else {
                    let inner = (**inner).clone();
                    self.replies.insert(opcode, inner);
                    self.resolve(opcode, value)
                }
            }
        }
    }
}

impl Transport for MockTransport {
    fn control_in(&mut self, opcode: u8, value: u16, len: u16) -> Result<Vec<u8>> {
        self.log.push(Exchange {
            direction: Direction::In,
            opcode,
            value,
            payload: Vec::new(),
        });

        let mut data = self.resolve(opcode, value)?;
        // Mirror the real backend: answering fewer bytes than asked is an error,
        // not something to pad over.
        if data.len() < len as usize {
            return Err(TransportError::ShortRead {
                opcode,
                wanted: len as usize,
                got: data.len(),
            });
        }
        data.truncate(len as usize);
        Ok(data)
    }

    fn control_out(&mut self, opcode: u8, value: u16, data: &[u8]) -> Result<()> {
        self.log.push(Exchange {
            direction: Direction::Out,
            opcode,
            value,
            payload: data.to_vec(),
        });
        match self.replies.get(&opcode) {
            Some(Reply::Stall) => Err(TransportError::Stalled { opcode }),
            Some(Reply::Disconnect) => Err(TransportError::Disconnected),
            _ => Ok(()),
        }
    }

    fn descriptor(&self) -> &DeviceDescriptor {
        static FALLBACK: std::sync::OnceLock<DeviceDescriptor> = std::sync::OnceLock::new();
        self.descriptor.as_ref().unwrap_or_else(|| {
            FALLBACK.get_or_init(|| DeviceDescriptor {
                serial: "MOCK000000000000".into(),
                bus_id: "mock".into(),
                address: 0,
            })
        })
    }

    fn max_transfer(&self) -> usize {
        self.max_transfer.unwrap_or(4096)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::with_busy_retry;

    #[test]
    fn answers_scripted_data() {
        let mut t = MockTransport::new().data(0x7F, vec![1, 1, 5, 9]);
        assert_eq!(t.control_in(0x7F, 0, 4).unwrap(), vec![1, 1, 5, 9]);
    }

    #[test]
    fn an_unscripted_opcode_stalls_like_the_firmware() {
        let mut t = MockTransport::new();
        assert!(matches!(
            t.control_in(0xFF, 0, 1),
            Err(TransportError::Stalled { opcode: 0xFF })
        ));
    }

    /// The flash blackout, which is the behaviour most likely to be got wrong.
    #[test]
    fn a_busy_window_recovers_after_backoff() {
        let mut t =
            MockTransport::new().reply(0x90, Reply::BusyThen(2, Box::new(Reply::Data(vec![0x00]))));

        let r = with_busy_retry(|| t.control_in(0x90, 3, 1), 0x90);
        assert_eq!(r.unwrap(), vec![0x00]);
        // Two refusals plus the successful third.
        assert_eq!(t.opcodes_seen().len(), 3);
    }

    #[test]
    fn records_what_was_actually_sent() {
        let mut t = MockTransport::new();
        let _ = t.control_out(0x42, 0, &[1, 2, 3]);
        assert_eq!(
            t.log[0],
            Exchange {
                direction: Direction::Out,
                opcode: 0x42,
                value: 0,
                payload: vec![1, 2, 3],
            }
        );
    }

    #[test]
    fn a_short_answer_is_an_error_not_silent_padding() {
        let mut t = MockTransport::new().data(0x50, vec![1, 2]);
        assert!(matches!(
            t.control_in(0x50, 9, 26),
            Err(TransportError::ShortRead {
                wanted: 26,
                got: 2,
                ..
            })
        ));
    }
}
