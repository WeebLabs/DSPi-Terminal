//! Control Surfaces: user-wired knobs, buttons, switches, LEDs and IR remotes.
//!
//! Everything here is driven by the device's own capability tables. The
//! firmware reports which component types exist, which parameters they may
//! drive, what units and ranges those take, and which are unavailable on this
//! platform. Hardcoding any of it would mean a firmware that gains a noun
//! tomorrow needs an app release; reading it means the picker simply grows.
//!
//! The macOS Console's Settings tab is the behavioural reference, including the
//! apply-live-then-save preview model and the three-valued IR learn loop.

use dspi_proto::generated::opcodes as op;
use dspi_transport::{Result as TResult, Transport};

/// One binding, 24 bytes, identical on the wire and in flash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Binding {
    pub component: u8,
    pub noun: u8,
    pub action: u8,
    pub flags: u8,
    pub gpio: [u8; 2],
    pub event: u8,
    pub target: u8,
    pub index: u8,
    pub value: i16,
    pub step: i16,
    pub range_min: i16,
    pub range_max: i16,
}

/// A GPIO slot a component does not use.
pub const GPIO_UNUSED: u8 = 0xFF;

impl Binding {
    pub const WIRE_LEN: usize = 24;

    pub fn is_empty(&self) -> bool {
        self.component == 0
    }

    pub fn decode(b: &[u8]) -> Option<Self> {
        if b.len() < Self::WIRE_LEN {
            return None;
        }
        Some(Self {
            component: b[0],
            noun: b[1],
            action: b[2],
            flags: b[3],
            gpio: [b[4], b[5]],
            event: b[6],
            target: b[7],
            index: b[8],
            value: i16::from_le_bytes([b[10], b[11]]),
            step: i16::from_le_bytes([b[12], b[13]]),
            range_min: i16::from_le_bytes([b[14], b[15]]),
            range_max: i16::from_le_bytes([b[16], b[17]]),
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut b = vec![0u8; Self::WIRE_LEN];
        b[0] = self.component;
        b[1] = self.noun;
        b[2] = self.action;
        b[3] = self.flags;
        b[4] = self.gpio[0];
        b[5] = self.gpio[1];
        b[6] = self.event;
        b[7] = self.target;
        b[8] = self.index;
        b[10..12].copy_from_slice(&self.value.to_le_bytes());
        b[12..14].copy_from_slice(&self.step.to_le_bytes());
        b[14..16].copy_from_slice(&self.range_min.to_le_bytes());
        b[16..18].copy_from_slice(&self.range_max.to_le_bytes());
        b
    }
}

/// A learned remote button, 16 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IrCommand {
    pub noun: u8,
    pub action: u8,
    pub flags: u8,
    pub target: u8,
    pub index: u8,
    pub protocol: u8,
    pub value: i16,
    pub step: i16,
    pub code: u32,
}

impl IrCommand {
    pub const WIRE_LEN: usize = 16;

    /// `protocol == NONE` marks the sub-slot empty.
    pub fn is_empty(&self) -> bool {
        self.protocol == 0
    }

    pub fn decode(b: &[u8]) -> Option<Self> {
        if b.len() < Self::WIRE_LEN {
            return None;
        }
        Some(Self {
            noun: b[0],
            action: b[1],
            flags: b[2],
            target: b[3],
            index: b[4],
            protocol: b[5],
            value: i16::from_le_bytes([b[6], b[7]]),
            step: i16::from_le_bytes([b[8], b[9]]),
            code: u32::from_le_bytes([b[12], b[13], b[14], b[15]]),
        })
    }
}

/// What a component type can do, from the device's capability table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeCaps {
    /// Bit mask of the actions this component can drive.
    pub actions: u16,
    pub pin_count: u8,
    pub pin_class: u8,
}

/// What a parameter accepts, from the device's noun table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NounCaps {
    pub kind: u8,
    pub enum_count: u8,
    /// Zero means the noun is unavailable on this platform.
    pub actions: u16,
    pub min_q: i16,
    pub max_q: i16,
    pub unit: u8,
    pub target_kind: u8,
    pub target_count: u8,
    pub flags: u8,
}

impl NounCaps {
    pub const WIRE_LEN: usize = 12;

    /// A noun with no actions is not available here, which is how the firmware
    /// reports a platform difference such as ADAT on an RP2040.
    pub fn is_available(&self) -> bool {
        self.actions != 0
    }

    pub fn decode(b: &[u8]) -> Option<Self> {
        if b.len() < Self::WIRE_LEN {
            return None;
        }
        Some(Self {
            kind: b[0],
            enum_count: b[1],
            actions: u16::from_le_bytes([b[2], b[3]]),
            min_q: i16::from_le_bytes([b[4], b[5]]),
            max_q: i16::from_le_bytes([b[6], b[7]]),
            unit: b[8],
            target_kind: b[9],
            target_count: b[10],
            flags: b[11],
        })
    }
}

/// Live status of the whole feature.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    pub last_status: u8,
    pub last_slot: u8,
    pub max_bindings: u8,
    /// Live configuration differs from what is in flash.
    pub dirty: bool,
    pub active_mask: u16,
    pub slot_status: Vec<u8>,
    pub ir_active_mask: u8,
    pub ir_learn_state: u8,
    pub ir_status: Vec<u8>,
}

pub mod learn {
    pub const IDLE: u8 = 0;
    pub const ARMED: u8 = 1;
    pub const DONE: u8 = 2;
    pub const TIMEOUT: u8 = 3;

    /// `wValue` selectors. Reading the result is a third value, which the
    /// opcode's own comment only hints at.
    pub const CANCEL: u16 = 0;
    pub const ARM: u16 = 1;
    pub const READ: u16 = 2;
}

/// What a learn attempt produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LearnResult {
    pub state: u8,
    pub protocol: u8,
    pub code: u32,
}

/// A status code, as a sentence rather than a number to look up.
pub fn explain_status(code: u8) -> String {
    match code {
        0x00 => "applied".into(),
        0x01 => "that GPIO is not allowed".into(),
        0x02 => "that GPIO is already in use".into(),
        0x03 => "that output or slot does not exist".into(),
        0x04 => "disable the output first".into(),
        0x10 => "no such binding slot".into(),
        0x11 => "not a component this firmware knows".into(),
        0x12 => "not a parameter this firmware knows".into(),
        0x13 => "that component cannot do that to that parameter".into(),
        0x14 => "the value or step is out of range".into(),
        0x15 => "a potentiometer needs an analogue-capable pin".into(),
        0x16 => "accepted, not applied yet".into(),
        0x17 => "that channel or band does not exist".into(),
        0x18 => "not a gesture a button can carry".into(),
        0x19 => "that LED clashes with another on the same PWM slice".into(),
        0x1A => "another binding already uses that pin and gesture".into(),
        0x1B => "still applying a previous change; try again shortly".into(),
        0x1C => "could not write to flash".into(),
        0x1D => "another slot already holds the IR receiver".into(),
        0x1E => "set up an IR receiver first".into(),
        other => format!("refused with status 0x{other:02X}"),
    }
}

/// Read one binding slot.
pub fn read_binding(t: &mut dyn Transport, slot: u8) -> TResult<Binding> {
    let d = t.control_in(
        op::REQ_GET_CS_BINDING,
        slot as u16,
        Binding::WIRE_LEN as u16,
    )?;
    Ok(Binding::decode(&d).unwrap_or_default())
}

/// Read a slot's user label.
pub fn read_name(t: &mut dyn Transport, slot: u8) -> TResult<String> {
    let d = t.control_in(op::REQ_GET_CS_NAME, slot as u16, 32)?;
    Ok(String::from_utf8_lossy(&d)
        .trim_end_matches('\0')
        .trim()
        .to_string())
}

pub fn read_ir_command(t: &mut dyn Transport, sub_slot: u8) -> TResult<IrCommand> {
    let d = t.control_in(
        op::REQ_GET_CS_IR_CMD,
        sub_slot as u16,
        IrCommand::WIRE_LEN as u16,
    )?;
    Ok(IrCommand::decode(&d).unwrap_or_default())
}

/// Read one noun's descriptor from the device's capability table.
pub fn read_noun_caps(t: &mut dyn Transport, noun: u8) -> TResult<NounCaps> {
    let d = t.control_in(op::REQ_GET_CS_CAPS, noun as u16, NounCaps::WIRE_LEN as u16)?;
    Ok(NounCaps::decode(&d).unwrap_or(NounCaps {
        kind: 0,
        enum_count: 0,
        actions: 0,
        min_q: 0,
        max_q: 0,
        unit: 0,
        target_kind: 0,
        target_count: 0,
        flags: 0,
    }))
}

pub fn read_status(t: &mut dyn Transport, max_bindings: u8, max_ir: u8) -> TResult<Status> {
    let len = 6 + max_bindings as usize + 2 + max_ir as usize;
    let d = t.control_in(op::REQ_GET_CS_STATUS, 0, len as u16)?;

    let slots = 6 + max_bindings as usize;
    Ok(Status {
        last_status: d[0],
        last_slot: d[1],
        max_bindings: d[2],
        dirty: d[3] != 0,
        active_mask: u16::from_le_bytes([d[4], d[5]]),
        slot_status: d[6..slots.min(d.len())].to_vec(),
        ir_active_mask: d.get(slots).copied().unwrap_or(0),
        ir_learn_state: d.get(slots + 1).copied().unwrap_or(0),
        ir_status: d.get(slots + 2..).unwrap_or(&[]).to_vec(),
    })
}

/// Start listening for a remote button.
pub fn arm_learn(t: &mut dyn Transport) -> TResult<()> {
    t.control_in(op::REQ_CS_IR_LEARN, learn::ARM, 1)?;
    Ok(())
}

pub fn cancel_learn(t: &mut dyn Transport) -> TResult<()> {
    t.control_in(op::REQ_CS_IR_LEARN, learn::CANCEL, 1)?;
    Ok(())
}

/// Poll for a learn result.
///
/// The opcode is three-valued, which is easy to miss: arm, cancel, and **read**.
/// The read returns eight bytes carrying the state, the decoded protocol and the
/// code. Poll it until the state leaves `ARMED`.
pub fn read_learn(t: &mut dyn Transport) -> TResult<LearnResult> {
    let d = t.control_in(op::REQ_CS_IR_LEARN, learn::READ, 8)?;
    Ok(LearnResult {
        state: d[0],
        protocol: d[1],
        code: u32::from_le_bytes([d[4], d[5], d[6], d[7]]),
    })
}

/// Persist the previewed configuration.
///
/// Bindings apply live but do not survive a power cycle until this runs, which
/// is a real feature: an assignment can be tried and backed out of.
pub fn save(t: &mut dyn Transport) -> TResult<u8> {
    Ok(t.control_in(op::REQ_CS_SAVE, 0, 1)?[0])
}

/// Discard the preview and reload what is in flash.
pub fn revert(t: &mut dyn Transport) -> TResult<u8> {
    Ok(t.control_in(op::REQ_CS_REVERT, 0, 1)?[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use dspi_transport::MockTransport;

    #[test]
    fn a_binding_round_trips_through_the_wire() {
        let b = Binding {
            component: 4,
            noun: 0,
            action: 1,
            flags: 2,
            gpio: [27, 28],
            event: 0,
            target: 3,
            index: 5,
            value: -120,
            step: 256,
            range_min: -600,
            range_max: 0,
        };
        let bytes = b.encode();
        assert_eq!(
            bytes.len(),
            Binding::WIRE_LEN,
            "the wire size is fixed at 24"
        );
        assert_eq!(Binding::decode(&bytes), Some(b));
    }

    #[test]
    fn reserved_bytes_are_written_as_zero() {
        let bytes = Binding {
            component: 1,
            ..Default::default()
        }
        .encode();
        assert_eq!(bytes[9], 0, "reserved");
        assert!(bytes[18..].iter().all(|b| *b == 0), "reserved2");
    }

    #[test]
    fn an_empty_slot_is_recognisable() {
        assert!(Binding::default().is_empty());
        assert!(
            !Binding {
                component: 1,
                ..Default::default()
            }
            .is_empty()
        );
    }

    #[test]
    fn a_short_read_decodes_to_nothing_rather_than_garbage() {
        assert!(Binding::decode(&[0; 10]).is_none());
        assert!(IrCommand::decode(&[0; 4]).is_none());
        assert!(NounCaps::decode(&[0; 3]).is_none());
    }

    /// A noun with no actions is how the firmware reports a platform
    /// difference, such as ADAT on an RP2040. Offering it would produce a
    /// binding the device silently refuses.
    #[test]
    fn a_noun_with_no_actions_is_unavailable() {
        let mut caps = NounCaps::decode(&[0; 12]).unwrap();
        assert!(!caps.is_available());
        caps.actions = 0b101;
        assert!(caps.is_available());
    }

    #[test]
    fn noun_caps_decode_their_range_and_targets() {
        // drive: continuous, dB, 0..18, untargeted.
        let bytes = [0u8, 0, 0b0000_1111, 0, 0, 0, 0x40, 0x12, 1, 0, 0, 0];
        let c = NounCaps::decode(&bytes).unwrap();
        assert!(c.is_available());
        assert_eq!(c.unit, 1);
        assert_eq!(c.max_q, 0x1240);
    }

    #[test]
    fn an_ir_command_with_no_protocol_is_an_empty_slot() {
        assert!(IrCommand::default().is_empty());
        let learned =
            IrCommand::decode(&[0, 2, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0xBF, 0x40, 0xDF, 0x20])
                .unwrap();
        assert!(!learned.is_empty());
        assert_eq!(learned.protocol, 1);
        assert_eq!(learned.code, 0x20DF40BF);
    }

    /// The learn opcode is three-valued: arm, cancel and read. Treating it as
    /// two would leave the result unreadable.
    #[test]
    fn the_learn_loop_arms_polls_and_reads_a_code() {
        let mut t = MockTransport::new().data(op::REQ_CS_IR_LEARN, {
            let mut v = vec![learn::DONE, 1, 0, 0];
            v.extend(0x20DF40BFu32.to_le_bytes());
            v
        });

        arm_learn(&mut t).unwrap();
        let r = read_learn(&mut t).unwrap();
        assert_eq!(r.state, learn::DONE);
        assert_eq!(r.protocol, 1);
        assert_eq!(r.code, 0x20DF40BF);

        let values: Vec<u16> = t.log().iter().map(|e| e.value).collect();
        assert_eq!(values, vec![learn::ARM, learn::READ]);
    }

    #[test]
    fn a_learn_that_times_out_is_distinguishable_from_one_still_waiting() {
        assert_ne!(learn::TIMEOUT, learn::ARMED);
        assert_ne!(learn::DONE, learn::ARMED);
    }

    #[test]
    fn status_decodes_the_dirty_flag_and_per_slot_results() {
        let mut d = vec![0x02, 3, 16, 1, 0x05, 0x00];
        d.extend([0u8; 16]);
        d[6 + 3] = 0x1A; // slot 3 refused: pin and gesture already in use
        d.push(0b11); // ir active
        d.push(learn::IDLE);
        d.extend([0u8; 8]);

        let mut t = MockTransport::new().data(op::REQ_GET_CS_STATUS, d);
        let s = read_status(&mut t, 16, 8).unwrap();

        assert!(s.dirty, "unsaved changes must be visible");
        assert_eq!(s.max_bindings, 16);
        assert_eq!(s.active_mask, 0b101);
        assert_eq!(s.slot_status[3], 0x1A);
        assert_eq!(s.ir_active_mask, 0b11);
    }

    /// Every status code needs a sentence; a bare number sends the user to a
    /// header file.
    #[test]
    fn every_status_code_explains_itself() {
        for code in [0x00u8, 0x02, 0x13, 0x15, 0x1A, 0x1D, 0x1E] {
            let msg = explain_status(code);
            // The bar is "reads as English", not a length: "applied" is a
            // perfectly good answer for success.
            assert!(!msg.is_empty(), "0x{code:02X} has no explanation");
            assert!(
                !msg.contains("0x"),
                "0x{code:02X} was left as a number to look up"
            );
            assert!(
                msg.chars().next().unwrap().is_lowercase(),
                "0x{code:02X} should read as a clause"
            );
        }
        assert!(
            explain_status(0x77).contains("0x77"),
            "unknown codes stay visible"
        );
    }

    #[test]
    fn save_and_revert_are_dispatched_as_reads() {
        let mut t = MockTransport::new()
            .data(op::REQ_CS_SAVE, vec![0])
            .data(op::REQ_CS_REVERT, vec![0]);
        assert_eq!(save(&mut t).unwrap(), 0);
        assert_eq!(revert(&mut t).unwrap(), 0);
        // Both are write-as-read: they mutate but travel on the IN path.
        assert!(
            t.log()
                .iter()
                .all(|e| e.direction == dspi_transport::mock::Direction::In)
        );
    }
}
