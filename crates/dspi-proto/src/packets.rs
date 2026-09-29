//! Typed codecs for every structured payload the firmware exchanges.
//!
//! One struct per wire record, each with a `SIZE`, an `encode` and a
//! length-checked `decode` that can never panic. Offsets are pinned by test
//! against the vendored headers, because a wrong offset here misconfigures real
//! hardware silently: a GPIO read as a clock mode still looks like a number.
//!
//! Everything is little endian, floats are IEEE 754 single precision, and every
//! reserved byte is written as zero.
//!
//! Fields carrying an index into a device-reported table (a noun, an action, a
//! component type) are kept as their raw byte. The device tells the host how
//! many of each it has, so the host addresses them by number and uses
//! [`crate::enums`] only to put a label on one. Fields that are genuinely a
//! closed protocol enum use the open enums from that module, which preserve an
//! unrecognised value rather than clamping it.

use crate::enums::{InputSource, SpdifRxState};
use crate::value::EqParamPacket;
use crate::{FilterType, Platform};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PacketError {
    #[error("{what} needs {want} bytes, got {got}")]
    TooShort {
        what: &'static str,
        want: usize,
        got: usize,
    },

    #[error("`{name}` is not a field of {what}; valid fields are {valid}")]
    UnknownField {
        what: &'static str,
        name: String,
        valid: String,
    },

    #[error("`{value}` is not valid for {field}; expected {expected}")]
    BadValue {
        field: &'static str,
        value: String,
        expected: String,
    },

    #[error("{what} is protocol version {got}; this build reads version {want}")]
    Version {
        what: &'static str,
        got: u8,
        want: u8,
    },

    /// A record read in pieces changed between the pieces: its head and tail
    /// sequence numbers disagree. Read it again.
    #[error("the frame changed while it was read (head {head}, tail {tail})")]
    Torn { head: u8, tail: u8 },
}

// ---------------------------------------------------------------- primitives

fn need(b: &[u8], want: usize, what: &'static str) -> Result<(), PacketError> {
    if b.len() < want {
        return Err(PacketError::TooShort {
            what,
            want,
            got: b.len(),
        });
    }
    Ok(())
}

fn u16at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn i16at(b: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([b[o], b[o + 1]])
}

fn u32at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn f32at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn put_u16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}

fn put_i16(b: &mut [u8], o: usize, v: i16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}

fn put_u32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

fn put_f32(b: &mut [u8], o: usize, v: f32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

/// Read one of the fixed-width NUL-terminated name fields shared by presets,
/// channels, control-surface slots, groups and macros.
fn name_at(b: &[u8], o: usize, len: usize) -> String {
    let field = &b[o..o + len];
    let end = field.iter().position(|c| *c == 0).unwrap_or(len);
    String::from_utf8_lossy(&field[..end]).to_string()
}

/// Write a name into a fixed-width NUL-padded field, truncated so the
/// terminator always survives, exactly as the firmware does on its side.
fn put_name(b: &mut [u8], o: usize, len: usize, name: &str) {
    let bytes = name.as_bytes();
    let n = bytes.len().min(len - 1);
    b[o..o + n].copy_from_slice(&bytes[..n]);
}

// ------------------------------------------------------------ control surfaces

/// A GPIO slot a component does not use (`CS_GPIO_UNUSED`,
/// control_surfaces.h:273).
pub const GPIO_UNUSED: u8 = 0xFF;

/// One binding, 24 bytes, identical on the wire and in flash
/// (control_surfaces.h:445-471).
///
/// The header calls byte 0 `type`, which is a Rust keyword; it is `component`
/// here, the word the app already shows in its own column header.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CsBinding {
    /// `CsType`; 0 (`CS_TYPE_NONE`) is a cleared slot.
    pub component: u8,
    pub noun: u8,
    pub action: u8,
    pub flags: u8,
    /// `gpio[1]` is `CS_GPIO_UNUSED` on a configured single-pin component and
    /// 0 on a cleared slot, so an empty record stays all-zero.
    pub gpio: [u8; 2],
    pub event: u8,
    pub target: u8,
    pub index: u8,
    /// LED_PWM brightness ceiling, percent 1-100; 0 = unset = full. Caps v12
    /// claimed this from the binding's last spare byte, so every other type
    /// must write 0 (control_surfaces.h:380-382).
    pub base_bright: u8,
    pub value: i16,
    pub step: i16,
    pub range_min: i16,
    pub range_max: i16,
    /// Indicator condition timing, 0.1 s units, 0 = immediate.
    pub on_delay: u16,
    pub off_delay: u16,
    /// Type-specific flags, claimed from the reserved pair at caps v18
    /// (control_surfaces.h:468-470): the `aux_extras` bits on an auxiliary
    /// output, and 0 on every other type. Byte 23 stays reserved and zero.
    pub extras: u8,
}

impl CsBinding {
    pub const SIZE: usize = 24;

    pub fn is_empty(&self) -> bool {
        self.component == 0
    }

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.component;
        b[1] = self.noun;
        b[2] = self.action;
        b[3] = self.flags;
        b[4] = self.gpio[0];
        b[5] = self.gpio[1];
        b[6] = self.event;
        b[7] = self.target;
        b[8] = self.index;
        b[9] = self.base_bright;
        put_i16(&mut b, 10, self.value);
        put_i16(&mut b, 12, self.step);
        put_i16(&mut b, 14, self.range_min);
        put_i16(&mut b, 16, self.range_max);
        put_u16(&mut b, 18, self.on_delay);
        put_u16(&mut b, 20, self.off_delay);
        b[22] = self.extras;
        // 23 reserved2, written as zero.
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "CsBinding")?;
        Ok(Self {
            component: b[0],
            noun: b[1],
            action: b[2],
            flags: b[3],
            gpio: [b[4], b[5]],
            event: b[6],
            target: b[7],
            index: b[8],
            base_bright: b[9],
            value: i16at(b, 10),
            step: i16at(b, 12),
            range_min: i16at(b, 14),
            range_max: i16at(b, 16),
            on_delay: u16at(b, 18),
            off_delay: u16at(b, 20),
            extras: b[22],
        })
    }

    /// The GPIOs this binding actually claims, second pin included only when it
    /// is in use.
    pub fn pins(&self) -> Vec<u8> {
        let mut v = vec![self.gpio[0]];
        if self.gpio[1] != GPIO_UNUSED && self.gpio[1] != self.gpio[0] {
            v.push(self.gpio[1]);
        }
        v
    }
}

/// One learned remote button, 16 bytes (control_surfaces.h:403-414).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IrCommand {
    pub noun: u8,
    pub action: u8,
    pub flags: u8,
    pub target: u8,
    pub index: u8,
    /// `CS_IR_PROTO_*`; `NONE` (0) marks the sub-slot empty.
    pub protocol: u8,
    pub value: i16,
    pub step: i16,
    pub code: u32,
}

impl IrCommand {
    pub const SIZE: usize = 16;

    pub fn is_empty(&self) -> bool {
        self.protocol == 0
    }

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.noun;
        b[1] = self.action;
        b[2] = self.flags;
        b[3] = self.target;
        b[4] = self.index;
        b[5] = self.protocol;
        put_i16(&mut b, 6, self.value);
        put_i16(&mut b, 8, self.step);
        // 10-11 reserved.
        put_u32(&mut b, 12, self.code);
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "IrCommand")?;
        Ok(Self {
            noun: b[0],
            action: b[1],
            flags: b[2],
            target: b[3],
            index: b[4],
            protocol: b[5],
            value: i16at(b, 6),
            step: i16at(b, 8),
            code: u32at(b, 12),
        })
    }
}

/// The `REQ_CS_IR_LEARN` result read, `wValue = 2`: 8 bytes
/// `{state, protocol, 0, 0, code_le32}` (control_surfaces.h:739-741).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IrLearnResult {
    pub state: u8,
    pub protocol: u8,
    pub code: u32,
}

impl IrLearnResult {
    pub const SIZE: usize = 8;

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.state;
        b[1] = self.protocol;
        put_u32(&mut b, 4, self.code);
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "IrLearnResult")?;
        Ok(Self {
            state: b[0],
            protocol: b[1],
            code: u32at(b, 4),
        })
    }
}

/// One target group, 40 bytes (control_surfaces.h:440-445).
///
/// `member_mask` is 32 bits because the RP2350 DSP-channel space runs past 16.
/// An empty slot is the strict all-zero record.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CsGroup {
    /// `CS_TARGET_INPUT_CH` / `_OUTPUT_CH` / `_DSP_CH`; 0 = empty.
    pub target_kind: u8,
    pub member_mask: u32,
    pub name: String,
}

impl CsGroup {
    pub const SIZE: usize = 40;
    pub const NAME_LEN: usize = 32;

    /// A group is usable only with a kind and at least one member.
    pub fn is_configured(&self) -> bool {
        self.target_kind != 0 && self.member_mask != 0
    }

    /// Member channel indices, ascending. The lowest is the anchor the firmware
    /// computes bool and enum actions against.
    pub fn members(&self) -> Vec<u8> {
        (0..32u8)
            .filter(|i| self.member_mask & (1u32 << i) != 0)
            .collect()
    }

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.target_kind;
        // 1-3 reserved.
        put_u32(&mut b, 4, self.member_mask);
        put_name(&mut b, 8, Self::NAME_LEN, &self.name);
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "CsGroup")?;
        Ok(Self {
            target_kind: b[0],
            member_mask: u32at(b, 4),
            name: name_at(b, 8, Self::NAME_LEN),
        })
    }
}

/// One macro step, 12 bytes (control_surfaces.h:460-470). All-zero is an empty
/// step, which the sequencer skips.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CsMacroStep {
    pub noun: u8,
    pub action: u8,
    pub flags: u8,
    /// Channel index, or a group index when `CS_FLAG_GROUP` is set.
    pub target: u8,
    pub index: u8,
    pub value: i16,
    pub step: i16,
    /// Delay before this step runs, in 10 ms units. Ten times finer than the
    /// binding's indicator delays, which are 0.1 s.
    pub pre_delay: u16,
}

impl CsMacroStep {
    pub const SIZE: usize = 12;

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.noun;
        b[1] = self.action;
        b[2] = self.flags;
        b[3] = self.target;
        b[4] = self.index;
        // 5 reserved.
        put_i16(&mut b, 6, self.value);
        put_i16(&mut b, 8, self.step);
        put_u16(&mut b, 10, self.pre_delay);
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "CsMacroStep")?;
        Ok(Self {
            noun: b[0],
            action: b[1],
            flags: b[2],
            target: b[3],
            index: b[4],
            value: i16at(b, 6),
            step: i16at(b, 8),
            pre_delay: u16at(b, 10),
        })
    }
}

/// The `REQ_SET_CS_MACRO` payload, 36 bytes: name and step count only
/// (control_surfaces.h:492-496). The steps go one at a time, because the whole
/// 132-byte macro exceeds the 64-byte vendor SET buffer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CsMacroHeaderWire {
    pub name: String,
    pub step_count: u8,
}

impl CsMacroHeaderWire {
    pub const SIZE: usize = 36;
    pub const NAME_LEN: usize = 32;

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        put_name(&mut b, 0, Self::NAME_LEN, &self.name);
        b[32] = self.step_count;
        // 33-35 reserved.
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "CsMacroHeaderWire")?;
        Ok(Self {
            name: name_at(b, 0, Self::NAME_LEN),
            step_count: b[32],
        })
    }
}

/// A whole macro as `REQ_GET_CS_MACRO` answers it, 132 bytes
/// (control_surfaces.h:472-477). Read only: the host writes the header and the
/// steps separately.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CsMacro {
    pub name: String,
    /// Steps executed are `0..step_count`, clamped to the steps that exist.
    pub step_count: u8,
    pub steps: Vec<CsMacroStep>,
}

impl CsMacro {
    pub const SIZE: usize = 132;
    /// Where the step array starts; `CsMacroHeaderWire` is its prefix.
    pub const STEPS_AT: usize = 36;

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "CsMacro")?;
        let count = (b.len() - Self::STEPS_AT) / CsMacroStep::SIZE;
        let steps = (0..count)
            .map(|i| {
                let o = Self::STEPS_AT + i * CsMacroStep::SIZE;
                CsMacroStep::decode(&b[o..o + CsMacroStep::SIZE])
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            name: name_at(b, 0, CsMacroHeaderWire::NAME_LEN),
            // Clamped: the UI iterates `0..step_count` over the decoded steps,
            // so a corrupt record must not describe steps that are not there.
            step_count: b[32].min(steps.len() as u8),
            steps,
        })
    }

    pub fn step(&self, i: usize) -> Option<&CsMacroStep> {
        self.steps.get(i)
    }

    /// The steps that actually execute, in order.
    pub fn active_steps(&self) -> &[CsMacroStep] {
        &self.steps[..(self.step_count as usize).min(self.steps.len())]
    }

    pub fn header(&self) -> CsMacroHeaderWire {
        CsMacroHeaderWire {
            name: self.name.clone(),
            step_count: self.step_count,
        }
    }
}

/// Display configuration, 12 bytes (control_surfaces.h:500-509). Timings are in
/// 0.1 s units, like the indicator delays.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CsDisplayCfg {
    /// `CS_DMODE_FIXED` / `_CYCLE_SELECTED` / `_CYCLE_ALL`.
    pub mode: u8,
    pub home_page: u8,
    /// Cycle period, 0.1 s units; minimum 10 in the cycle modes.
    pub dwell: u16,
    /// Event pop-up hold, 0.1 s units; 0 turns the overlay off.
    pub overlay_hold: u16,
    pub brightness: u8,
    pub flags: u8,
    /// Edit-mode auto-disarm, 0.1 s units; 0 = manual only.
    pub edit_timeout: u16,
}

/// `CsDisplayCfg.flags` bits (control_surfaces.h:304-310).
pub mod display_flags {
    pub const OVERLAY_ANY: u8 = 0x01;
    pub const EDIT_GATED: u8 = 0x02;
    pub const LABEL_ALIGN: u8 = 0x0C;
    pub const VALUE_ALIGN: u8 = 0x30;
    pub const LABEL_ALIGN_SHIFT: u8 = 2;
    pub const VALUE_ALIGN_SHIFT: u8 = 4;
}

impl CsDisplayCfg {
    pub const SIZE: usize = 12;

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.mode;
        b[1] = self.home_page;
        put_u16(&mut b, 2, self.dwell);
        put_u16(&mut b, 4, self.overlay_hold);
        b[6] = self.brightness;
        b[7] = self.flags;
        put_u16(&mut b, 8, self.edit_timeout);
        // 10-11 reserved.
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "CsDisplayCfg")?;
        Ok(Self {
            mode: b[0],
            home_page: b[1],
            dwell: u16at(b, 2),
            overlay_hold: u16at(b, 4),
            brightness: b[6],
            flags: b[7],
            edit_timeout: u16at(b, 8),
        })
    }

    /// Alignment of the label line, one of the two 2-bit fields inside `flags`.
    pub fn label_align(&self) -> u8 {
        (self.flags & display_flags::LABEL_ALIGN) >> display_flags::LABEL_ALIGN_SHIFT
    }

    pub fn set_label_align(&mut self, align: u8) {
        self.flags = (self.flags & !display_flags::LABEL_ALIGN)
            | ((align << display_flags::LABEL_ALIGN_SHIFT) & display_flags::LABEL_ALIGN);
    }

    pub fn value_align(&self) -> u8 {
        (self.flags & display_flags::VALUE_ALIGN) >> display_flags::VALUE_ALIGN_SHIFT
    }

    pub fn set_value_align(&mut self, align: u8) {
        self.flags = (self.flags & !display_flags::VALUE_ALIGN)
            | ((align << display_flags::VALUE_ALIGN_SHIFT) & display_flags::VALUE_ALIGN);
    }
}

/// The `REQ_GET_CS_DISPLAY_CFG` response, 16 bytes: the device's limits then
/// the same 12-byte record the SET takes (config.h:151-153).
///
/// Reading the config from offset 0 would take `max_pages` as the mode.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CsDisplayCfgReply {
    pub max_pages: u8,
    pub model_count: u8,
    pub cfg: CsDisplayCfg,
}

impl CsDisplayCfgReply {
    pub const SIZE: usize = 16;
    pub const CFG_AT: usize = 4;

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.max_pages;
        b[1] = self.model_count;
        b[Self::CFG_AT..].copy_from_slice(&self.cfg.encode());
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "CsDisplayCfgReply")?;
        Ok(Self {
            max_pages: b[0],
            model_count: b[1],
            cfg: CsDisplayCfg::decode(&b[Self::CFG_AT..])?,
        })
    }
}

/// One display page, 4 bytes (control_surfaces.h:513-518). All-zero is an empty
/// slot.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CsDisplayPage {
    pub noun: u8,
    /// Channel, or a group index when `CS_DPAGE_GROUP` is set.
    pub target: u8,
    pub index: u8,
    pub flags: u8,
}

/// `CsDisplayPage.flags` bits (control_surfaces.h:319-324).
pub mod page_flags {
    pub const ACTIVE: u8 = 0x01;
    pub const GROUP: u8 = 0x02;
    pub const LARGE: u8 = 0x04;
    pub const BAR: u8 = 0x08;
}

impl CsDisplayPage {
    pub const SIZE: usize = 4;

    pub fn is_active(&self) -> bool {
        self.flags & page_flags::ACTIVE != 0
    }

    pub fn encode(&self) -> [u8; Self::SIZE] {
        [self.noun, self.target, self.index, self.flags]
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "CsDisplayPage")?;
        Ok(Self {
            noun: b[0],
            target: b[1],
            index: b[2],
            flags: b[3],
        })
    }
}

/// `REQ_GET_CS_DISPLAY_STATUS`, 8 bytes (control_surfaces.h:531-538).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CsDisplayStatus {
    /// 0 down, 1 initialising, 2 live, 3 error or backoff.
    pub init_state: u8,
    /// Shown page slot; 0xFF = none or a synthesised overlay.
    pub current_page: u8,
    /// Bit 0 overlay showing, bit 1 edit armed.
    pub flags: u8,
    pub model: u8,
    /// Cumulative I2C aborts, saturating. How a miswired panel announces itself.
    pub nak_count: u16,
}

impl CsDisplayStatus {
    pub const SIZE: usize = 8;
    pub const FLAG_OVERLAY: u8 = 0x01;
    pub const FLAG_EDIT: u8 = 0x02;

    pub fn is_live(&self) -> bool {
        self.init_state == 2
    }

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.init_state;
        b[1] = self.current_page;
        b[2] = self.flags;
        b[3] = self.model;
        put_u16(&mut b, 4, self.nak_count);
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "CsDisplayStatus")?;
        Ok(Self {
            init_state: b[0],
            current_page: b[1],
            flags: b[2],
            model: b[3],
            nak_count: u16at(b, 4),
        })
    }
}

/// `REQ_GET_CS_EXT_STATUS`, 24 bytes (control_surfaces.h:542-551).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CsExtStatusPacket {
    pub max_groups: u8,
    pub max_macros: u8,
    pub max_macro_steps: u8,
    /// Running macro index; 0xFF when the sequencer is idle.
    pub macro_running: u8,
    pub macro_step: u8,
    pub group_status: [u8; 8],
    pub macro_status: [u8; 8],
}

impl CsExtStatusPacket {
    pub const SIZE: usize = 24;
    /// The idle sentinel, deliberately past every macro index so an
    /// `IND_EQUALS` LED stays dark while nothing runs.
    pub const MACRO_NONE: u8 = 0xFF;

    pub fn is_running(&self) -> bool {
        self.macro_running != Self::MACRO_NONE
    }

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.max_groups;
        b[1] = self.max_macros;
        b[2] = self.max_macro_steps;
        b[3] = self.macro_running;
        b[4] = self.macro_step;
        b[8..16].copy_from_slice(&self.group_status);
        b[16..24].copy_from_slice(&self.macro_status);
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "CsExtStatusPacket")?;
        let mut group_status = [0u8; 8];
        let mut macro_status = [0u8; 8];
        group_status.copy_from_slice(&b[8..16]);
        macro_status.copy_from_slice(&b[16..24]);
        Ok(Self {
            max_groups: b[0],
            max_macros: b[1],
            max_macro_steps: b[2],
            macro_running: b[3],
            macro_step: b[4],
            group_status,
            macro_status,
        })
    }
}

/// `REQ_GET_CS_STATUS`, 41 bytes at caps v6 and later
/// (control_surfaces.h:594-605).
///
/// The two tables are sized by the caps header, never by a constant: caps v6
/// doubled `CS_MAX_IR_COMMANDS` from 8 to 16, which widened `ir_active_mask`
/// from one byte to two and took the packet from 22 bytes to 41.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CsStatusPacket {
    pub last_status: u8,
    pub last_slot: u8,
    pub max_bindings: u8,
    /// The live config differs from flash: an unsaved preview.
    pub dirty: bool,
    pub active_mask: u16,
    pub slot_status: Vec<u8>,
    pub ir_active_mask: u16,
    pub ir_learn_state: u8,
    pub ir_cmd_status: Vec<u8>,
}

impl CsStatusPacket {
    /// The v6 size, with 16 binding slots and 16 IR sub-slots.
    pub const SIZE: usize = 41;

    /// Bytes on the wire for a device reporting these two maxima.
    pub fn wire_len(max_bindings: u8, max_ir: u8) -> usize {
        6 + max_bindings as usize + 3 + max_ir as usize
    }

    pub fn is_slot_active(&self, slot: u8) -> bool {
        slot < 16 && self.active_mask & (1u16 << slot) != 0
    }

    pub fn slot_health(&self, slot: u8) -> u8 {
        self.slot_status.get(slot as usize).copied().unwrap_or(0)
    }

    pub fn is_ir_active(&self, sub: u8) -> bool {
        sub < 16 && self.ir_active_mask & (1u16 << sub) != 0
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut b = vec![
            self.last_status,
            self.last_slot,
            self.max_bindings,
            self.dirty as u8,
        ];
        b.extend(self.active_mask.to_le_bytes());
        b.extend(&self.slot_status);
        b.extend(self.ir_active_mask.to_le_bytes());
        b.push(self.ir_learn_state);
        b.extend(&self.ir_cmd_status);
        b
    }

    /// Decode the v6 layout: 16 binding slots, 16 IR sub-slots.
    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        Self::decode_sized(b, 16, 16)
    }

    /// Decode with the table sizes the caps header reported.
    pub fn decode_sized(b: &[u8], max_bindings: u8, max_ir: u8) -> Result<Self, PacketError> {
        let want = Self::wire_len(max_bindings, max_ir);
        need(b, want, "CsStatusPacket")?;
        let slots = 6 + max_bindings as usize;
        Ok(Self {
            last_status: b[0],
            last_slot: b[1],
            max_bindings: b[2],
            dirty: b[3] != 0,
            active_mask: u16at(b, 4),
            slot_status: b[6..slots].to_vec(),
            ir_active_mask: u16at(b, slots),
            ir_learn_state: b[slots + 2],
            ir_cmd_status: b[slots + 3..want].to_vec(),
        })
    }
}

/// One entry of the capability type table, 4 bytes
/// (control_surfaces.h:631-635).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CsTypeDesc {
    /// `CS_ACT_BIT` mask of the actions this component can drive.
    pub actions: u16,
    pub pin_count: u8,
    pub pin_class: u8,
}

impl CsTypeDesc {
    pub const SIZE: usize = 4;
    /// `CS_PINCLASS_ADC`: GPIO 26-28 on both platforms.
    pub const PINCLASS_ADC: u8 = 1;

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        put_u16(&mut b, 0, self.actions);
        b[2] = self.pin_count;
        b[3] = self.pin_class;
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "CsTypeDesc")?;
        Ok(Self {
            actions: u16at(b, 0),
            pin_count: b[2],
            pin_class: b[3],
        })
    }
}

/// `REQ_GET_CS_CAPS` with `wValue = 0xFFFF` (control_surfaces.h:637-652).
///
/// 44 bytes at caps v10 to v17 and 52 from v18, when the two auxiliary output
/// types took `type_count` to 11 (control_surfaces.h:141-145, 652). The size
/// is not a constant: the four
/// maxima sit at `4 + 4*type_count`, after a table that is meant to grow. A
/// host that hardcodes the offset reads the display type's descriptor as
/// `max_ir_commands` and gets a plausible small number.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CsCapsHeader {
    pub caps_version: u8,
    pub max_bindings: u8,
    pub type_count: u8,
    pub noun_count: u8,
    pub types: Vec<CsTypeDesc>,
    pub max_ir_commands: u8,
    /// Caps v9 and later; a pre-v9 device reads zero, which is exactly "no
    /// groups, no macros" and needs no separate version test.
    pub max_groups: u8,
    pub max_macros: u8,
    pub max_macro_steps: u8,
}

impl CsCapsHeader {
    /// Enough to read `type_count` and size the rest of the read.
    pub const MIN_SIZE: usize = 4;
    /// What v1.1.6 answers: 4 + 4*9 types + 4 maxima.
    pub const SIZE_AT_V13: usize = 44;
    /// What beta4 answers: 4 + 4*11 types + 4 maxima (control_surfaces.h:652).
    pub const SIZE_AT_V20: usize = 52;

    pub fn wire_len(type_count: u8) -> usize {
        Self::MIN_SIZE + CsTypeDesc::SIZE * type_count as usize + 4
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut b = vec![
            self.caps_version,
            self.max_bindings,
            self.type_count,
            self.noun_count,
        ];
        for t in &self.types {
            b.extend(t.encode());
        }
        b.extend([
            self.max_ir_commands,
            self.max_groups,
            self.max_macros,
            self.max_macro_steps,
        ]);
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::MIN_SIZE, "CsCapsHeader")?;
        let type_count = b[2];
        let table_end = Self::MIN_SIZE + CsTypeDesc::SIZE * type_count as usize;
        need(b, table_end, "CsCapsHeader type table")?;

        let types = (0..type_count as usize)
            .map(|i| {
                let o = Self::MIN_SIZE + i * CsTypeDesc::SIZE;
                CsTypeDesc::decode(&b[o..o + CsTypeDesc::SIZE])
            })
            .collect::<Result<Vec<_>, _>>()?;

        // The maxima follow the variable-length table. A shorter answer is an
        // older caps version, whose zeros mean "absent", not "broken".
        let tail = |i: usize| b.get(table_end + i).copied().unwrap_or(0);

        Ok(Self {
            caps_version: b[0],
            max_bindings: b[1],
            type_count,
            noun_count: b[3],
            types,
            max_ir_commands: tail(0),
            max_groups: tail(1),
            max_macros: tail(2),
            max_macro_steps: tail(3),
        })
    }
}

/// One noun descriptor, 12 bytes (control_surfaces.h:654-665).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CsNounDesc {
    /// `CS_KIND_CONTINUOUS` / `_BOOL` / `_ENUM`.
    pub kind: u8,
    pub enum_count: u8,
    /// Action mask this noun accepts. Zero means the noun is unavailable on
    /// this platform, which is how ADAT reports itself on an RP2040.
    pub actions: u16,
    pub min_q: i16,
    pub max_q: i16,
    pub unit: u8,
    pub target_kind: u8,
    pub target_count: u8,
    pub dflags: u8,
}

impl CsNounDesc {
    pub const SIZE: usize = 12;

    pub fn is_available(&self) -> bool {
        self.actions != 0
    }

    /// True when the noun addresses a channel, so a target picker is needed.
    pub fn is_targeted(&self) -> bool {
        self.target_kind != 0 && self.target_count > 0
    }

    /// True when the noun addresses a filter band, so an index picker is needed
    /// as well (`CS_TARGET_DSP_BAND`).
    pub fn has_band(&self) -> bool {
        self.target_kind == 4
    }

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.kind;
        b[1] = self.enum_count;
        put_u16(&mut b, 2, self.actions);
        put_i16(&mut b, 4, self.min_q);
        put_i16(&mut b, 6, self.max_q);
        b[8] = self.unit;
        b[9] = self.target_kind;
        b[10] = self.target_count;
        b[11] = self.dflags;
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "CsNounDesc")?;
        Ok(Self {
            kind: b[0],
            enum_count: b[1],
            actions: u16at(b, 2),
            min_q: i16at(b, 4),
            max_q: i16at(b, 6),
            unit: b[8],
            target_kind: b[9],
            target_count: b[10],
            dflags: b[11],
        })
    }
}

// -------------------------------------------------------------------- matrix

/// `REQ_SET/GET_MATRIX_ROUTE` payload, 8 bytes (config.h:845-851).
#[derive(Debug, Clone, PartialEq)]
pub struct MatrixRoutePacket {
    pub input: u8,
    pub output: u8,
    pub enabled: bool,
    pub phase_invert: bool,
    pub gain_db: f32,
}

impl Default for MatrixRoutePacket {
    fn default() -> Self {
        Self {
            input: 0,
            output: 0,
            enabled: false,
            phase_invert: false,
            gain_db: 0.0,
        }
    }
}

impl MatrixRoutePacket {
    pub const SIZE: usize = 8;

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.input;
        b[1] = self.output;
        b[2] = self.enabled as u8;
        b[3] = self.phase_invert as u8;
        put_f32(&mut b, 4, self.gain_db);
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "MatrixRoutePacket")?;
        Ok(Self {
            input: b[0],
            output: b[1],
            enabled: b[2] != 0,
            phase_invert: b[3] != 0,
            gain_db: f32at(b, 4),
        })
    }
}

/// Decode the 16-byte `EqParamPacket` (config.h:969-977), with the 18-byte
/// Linkwitz form's `qp` sidecar when it is present.
///
/// The encoder lives in [`crate::value::EqParamPacket`], which already knows
/// that a 16-byte write preserves the device's stored `qp` and that only a
/// Linkwitz band may send the extra two bytes.
pub fn decode_eq_param(b: &[u8]) -> Result<EqParamPacket, PacketError> {
    need(b, 16, "EqParamPacket")?;
    let filter_type = FilterType::from_raw(b[2]);
    Ok(EqParamPacket {
        channel: b[0],
        band: b[1],
        filter_type,
        // Exactly 1 means bypassed; any other value means active.
        bypass: b[3] == 1,
        freq: f32at(b, 4),
        q: f32at(b, 8),
        gain_db: f32at(b, 12),
        qp: (b.len() >= 18 && filter_type.is_linkwitz())
            .then(|| crate::value::decode_qp(u16at(b, 16))),
    })
}

// ------------------------------------------------------------------- upmixer

/// `REQ_UPMIX_SET/GET_CONFIG`, 44 bytes (upmix.h:167-183).
#[derive(Debug, Clone, PartialEq)]
pub struct UpmixConfigPacket {
    pub enabled: bool,
    /// `UPMIX_CENTER_PASSIVE` 0, `_ADAPTIVE` 1, `_OFF` 2. Off is appended
    /// rather than renumbered to 0, which would mirror the surround enum and
    /// silently remap every saved preset (upmix.h:77-85).
    pub center_mode: u8,
    pub surround_mode: u8,
    /// Centre presence bell, dB. Carried as `presence_q1`, an int8 in 0.5 dB
    /// steps, which claimed a former reserved byte at V26.
    pub presence_db: f32,
    pub strength_pct: f32,
    pub center_width_pct: f32,
    pub corr_threshold_pct: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub detector_hpf_hz: f32,
    pub surround_delay_ms: f32,
    pub surround_hpf_hz: f32,
    pub surround_lpf_hz: f32,
    pub decorr_pct: f32,
}

impl Default for UpmixConfigPacket {
    fn default() -> Self {
        Self {
            enabled: false,
            center_mode: 0,
            surround_mode: 0,
            presence_db: 0.0,
            strength_pct: 0.0,
            center_width_pct: 0.0,
            corr_threshold_pct: 0.0,
            attack_ms: 0.0,
            release_ms: 0.0,
            detector_hpf_hz: 0.0,
            surround_delay_ms: 0.0,
            surround_hpf_hz: 0.0,
            surround_lpf_hz: 0.0,
            decorr_pct: 0.0,
        }
    }
}

impl UpmixConfigPacket {
    pub const SIZE: usize = 44;

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.enabled as u8;
        b[1] = self.center_mode;
        b[2] = self.surround_mode;
        b[3] = self
            .presence_db
            .clamp(-12.0, 12.0)
            .mul_add(2.0, 0.0)
            .round() as i8 as u8;
        for (i, v) in [
            self.strength_pct,
            self.center_width_pct,
            self.corr_threshold_pct,
            self.attack_ms,
            self.release_ms,
            self.detector_hpf_hz,
            self.surround_delay_ms,
            self.surround_hpf_hz,
            self.surround_lpf_hz,
            self.decorr_pct,
        ]
        .into_iter()
        .enumerate()
        {
            put_f32(&mut b, 4 + i * 4, v);
        }
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "UpmixConfigPacket")?;
        let f = |i: usize| f32at(b, 4 + i * 4);
        Ok(Self {
            enabled: b[0] != 0,
            center_mode: b[1],
            surround_mode: b[2],
            presence_db: 0.5 * (b[3] as i8) as f32,
            strength_pct: f(0),
            center_width_pct: f(1),
            corr_threshold_pct: f(2),
            attack_ms: f(3),
            release_ms: f(4),
            detector_hpf_hz: f(5),
            surround_delay_ms: f(6),
            surround_hpf_hz: f(7),
            surround_lpf_hz: f(8),
            decorr_pct: f(9),
        })
    }
}

/// `REQ_UPMIX_GET_STATUS`, 16 bytes (upmix.h:216-227).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpmixStatus {
    pub active: bool,
    /// 0 active, 1 disabled, 2 input not stereo, 3 sample rate above 48 kHz.
    pub parked_reason: u8,
    pub corr_q14: i16,
    pub balance_q14: i16,
    pub center_gain_q15: u16,
    pub ls_gain_q15: u16,
    pub rs_gain_q15: u16,
}

impl UpmixStatus {
    pub const SIZE: usize = 16;

    /// Smoothed L/R correlation, -1 to +1.
    pub fn correlation(&self) -> f32 {
        self.corr_q14 as f32 / 16384.0
    }

    /// Smoothed L/R balance, 0 to 1.
    pub fn balance(&self) -> f32 {
        self.balance_q14 as f32 / 16384.0
    }

    /// The three steering gains, 0 to 1: centre, Ls, Rs.
    pub fn gains(&self) -> (f32, f32, f32) {
        let q15 = |v: u16| v as f32 / 32767.0;
        (
            q15(self.center_gain_q15),
            q15(self.ls_gain_q15),
            q15(self.rs_gain_q15),
        )
    }

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.active as u8;
        b[1] = self.parked_reason;
        put_i16(&mut b, 2, self.corr_q14);
        put_i16(&mut b, 4, self.balance_q14);
        put_u16(&mut b, 6, self.center_gain_q15);
        put_u16(&mut b, 8, self.ls_gain_q15);
        put_u16(&mut b, 10, self.rs_gain_q15);
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "UpmixStatus")?;
        Ok(Self {
            active: b[0] != 0,
            parked_reason: b[1],
            corr_q14: i16at(b, 2),
            balance_q14: i16at(b, 4),
            center_gain_q15: u16at(b, 6),
            ls_gain_q15: u16at(b, 8),
            rs_gain_q15: u16at(b, 10),
        })
    }
}

// ----------------------------------------------------------- signal generator

/// `REQ_SIGGEN_SET/GET_CONFIG`, 36 bytes (siggen.h:86-98).
#[derive(Debug, Clone, PartialEq)]
pub struct SiggenConfig {
    /// `SIGGEN_CFG_VERSION`, 1.
    pub version: u8,
    pub signal_type: u8,
    /// Output-channel select, bit i = output i.
    pub channel_mask: u16,
    /// Polarity-inverted subset of `channel_mask`.
    pub invert_mask: u16,
    pub flags: u8,
    /// Peak level dBFS, -120 to 0.
    pub level_db: f32,
    pub duration_ms: u32,
    pub repeat_count: u16,
    pub gap_ms: u16,
    pub p1: f32,
    pub p2: f32,
    pub p3: f32,
    pub p4: f32,
}

impl Default for SiggenConfig {
    fn default() -> Self {
        Self {
            version: 1,
            signal_type: 0,
            channel_mask: 0,
            invert_mask: 0,
            flags: 0,
            level_db: 0.0,
            duration_ms: 0,
            repeat_count: 0,
            gap_ms: 0,
            p1: 0.0,
            p2: 0.0,
            p3: 0.0,
            p4: 0.0,
        }
    }
}

impl SiggenConfig {
    pub const SIZE: usize = 36;
    pub const FLAG_RAW: u8 = 0x01;
    pub const FLAG_DECORR: u8 = 0x02;
    pub const FLAG_WALK: u8 = 0x04;

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.version;
        b[1] = self.signal_type;
        put_u16(&mut b, 2, self.channel_mask);
        put_u16(&mut b, 4, self.invert_mask);
        b[6] = self.flags;
        // 7 reserved0.
        put_f32(&mut b, 8, self.level_db);
        put_u32(&mut b, 12, self.duration_ms);
        put_u16(&mut b, 16, self.repeat_count);
        put_u16(&mut b, 18, self.gap_ms);
        put_f32(&mut b, 20, self.p1);
        put_f32(&mut b, 24, self.p2);
        put_f32(&mut b, 28, self.p3);
        put_f32(&mut b, 32, self.p4);
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "SiggenConfig")?;
        Ok(Self {
            version: b[0],
            signal_type: b[1],
            channel_mask: u16at(b, 2),
            invert_mask: u16at(b, 4),
            flags: b[6],
            level_db: f32at(b, 8),
            duration_ms: u32at(b, 12),
            repeat_count: u16at(b, 16),
            gap_ms: u16at(b, 18),
            p1: f32at(b, 20),
            p2: f32at(b, 24),
            p3: f32at(b, 28),
            p4: f32at(b, 32),
        })
    }
}

/// `REQ_SIGGEN_GET_STATUS`, 16 bytes (siggen.h:110-120).
#[derive(Debug, Clone, PartialEq)]
pub struct SiggenStatus {
    pub version: u8,
    /// `SIGGEN_STATE_*`: idle, fade in, run, gap, fade out.
    pub state: u8,
    pub signal_type: u8,
    /// The channel a WALK run is on; 0xFF when not walking.
    pub active_channel: u8,
    pub elapsed_ms: u32,
    pub cycles_done: u16,
    pub stop_reason: u8,
    /// Instantaneous sweep frequency, 0 when not sweeping.
    pub current_freq: f32,
}

impl Default for SiggenStatus {
    fn default() -> Self {
        Self {
            version: 1,
            state: 0,
            signal_type: 0,
            active_channel: 0xFF,
            elapsed_ms: 0,
            cycles_done: 0,
            stop_reason: 0,
            current_freq: 0.0,
        }
    }
}

impl SiggenStatus {
    pub const SIZE: usize = 16;

    pub fn is_running(&self) -> bool {
        self.state != 0
    }

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.version;
        b[1] = self.state;
        b[2] = self.signal_type;
        b[3] = self.active_channel;
        put_u32(&mut b, 4, self.elapsed_ms);
        put_u16(&mut b, 8, self.cycles_done);
        b[10] = self.stop_reason;
        // 11 reserved0.
        put_f32(&mut b, 12, self.current_freq);
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "SiggenStatus")?;
        Ok(Self {
            version: b[0],
            state: b[1],
            signal_type: b[2],
            active_channel: b[3],
            elapsed_ms: u32at(b, 4),
            cycles_done: u16at(b, 8),
            stop_reason: b[10],
            current_freq: f32at(b, 12),
        })
    }
}

/// `REQ_SIGGEN_GET_CAPS` with `wValue = 0xFFFF`, 8 bytes (siggen.h:136-143).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SiggenCapsHeader {
    pub version: u8,
    pub type_count: u8,
    pub output_channels: u8,
    /// `SIGGEN_MULTITONE_MAX`: 8 on RP2040, 16 on RP2350. Platform-conditional
    /// in the header, so it can only come from here.
    pub multitone_max: u8,
    pub valid_channel_mask: u16,
}

impl SiggenCapsHeader {
    pub const SIZE: usize = 8;

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.version;
        b[1] = self.type_count;
        b[2] = self.output_channels;
        b[3] = self.multitone_max;
        put_u16(&mut b, 4, self.valid_channel_mask);
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "SiggenCapsHeader")?;
        Ok(Self {
            version: b[0],
            type_count: b[1],
            output_channels: b[2],
            multitone_max: b[3],
            valid_channel_mask: u16at(b, 4),
        })
    }
}

/// One of the four per-parameter descriptors inside a [`SiggenTypeDesc`],
/// 13 bytes (siggen.h:154-157). The floats are unaligned on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SiggenParamDesc {
    /// `SIGGEN_PARAM_*`; 0 means the slot is unused.
    pub semantic: u8,
    pub min: f32,
    pub max: f32,
    pub default: f32,
}

impl SiggenParamDesc {
    pub const SIZE: usize = 13;

    pub fn is_used(&self) -> bool {
        self.semantic != 0
    }

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.semantic;
        put_f32(&mut b, 1, self.min);
        put_f32(&mut b, 5, self.max);
        put_f32(&mut b, 9, self.default);
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "SiggenParamDesc")?;
        Ok(Self {
            semantic: b[0],
            min: f32at(b, 1),
            max: f32at(b, 5),
            default: f32at(b, 9),
        })
    }
}

/// `REQ_SIGGEN_GET_CAPS` with `wValue` = a type index, 62 bytes
/// (siggen.h:164-169). The authoritative source for parameter ranges: it
/// already reflects the running platform's multitone budget.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SiggenTypeDesc {
    pub id: u8,
    /// NUL-padded 8-byte short name.
    pub name: String,
    /// `SIGGEN_TIMING_CONTINUOUS` / `_SWEEP` / `_PATTERN`.
    pub timing_model: u8,
    pub params: [SiggenParamDesc; 4],
}

impl SiggenTypeDesc {
    pub const SIZE: usize = 62;
    pub const NAME_LEN: usize = 8;
    pub const PARAMS_AT: usize = 10;

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.id;
        let name = self.name.as_bytes();
        let n = name.len().min(Self::NAME_LEN);
        b[1..1 + n].copy_from_slice(&name[..n]);
        b[9] = self.timing_model;
        for (i, p) in self.params.iter().enumerate() {
            let o = Self::PARAMS_AT + i * SiggenParamDesc::SIZE;
            b[o..o + SiggenParamDesc::SIZE].copy_from_slice(&p.encode());
        }
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "SiggenTypeDesc")?;
        let mut params = [SiggenParamDesc::default(); 4];
        for (i, p) in params.iter_mut().enumerate() {
            let o = Self::PARAMS_AT + i * SiggenParamDesc::SIZE;
            *p = SiggenParamDesc::decode(&b[o..o + SiggenParamDesc::SIZE])?;
        }
        Ok(Self {
            id: b[0],
            // The name field is NUL-padded rather than NUL-terminated: a name
            // filling all eight bytes has no terminator.
            name: name_at(b, 1, Self::NAME_LEN),
            timing_model: b[9],
            params,
        })
    }
}

// -------------------------------------------------------- board-level config

/// `REQ_SET/GET_DAC_HW_MUTE_CONFIG`, 16 bytes (config.h:440-441; the same
/// layout as the bulk packet's section 18, bulk_params.h:286-294).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DacHwMuteConfig {
    pub enabled: bool,
    /// 1 = assert LOW to mute. The default.
    pub active_low: bool,
    /// GPIO; 0xFF = no pin.
    pub pin: u8,
    /// Mute-attack hold before the clocks stop, ms.
    pub hold_ms: u16,
    /// Post-clock-restart hold before unmute, ms.
    pub release_ms: u16,
}

impl Default for DacHwMuteConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            active_low: true,
            pin: Self::PIN_NONE,
            hold_ms: 0,
            release_ms: 0,
        }
    }
}

impl DacHwMuteConfig {
    pub const SIZE: usize = 16;
    pub const PIN_NONE: u8 = 0xFF;

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.enabled as u8;
        b[1] = self.active_low as u8;
        b[2] = self.pin;
        // 3 reserved0, alignment for hold_ms.
        put_u16(&mut b, 4, self.hold_ms);
        put_u16(&mut b, 6, self.release_ms);
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "DacHwMuteConfig")?;
        Ok(Self {
            enabled: b[0] != 0,
            active_low: b[1] != 0,
            pin: b[2],
            hold_ms: u16at(b, 4),
            release_ms: u16at(b, 6),
        })
    }
}

/// `REQ_SET/GET_UART_CONFIG`, 8 bytes (config.h:525-531).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UartCtrlConfig {
    pub enabled: bool,
    /// GPIO with a UARTx TX mux, so `pin % 4 == 0`.
    pub tx_pin: u8,
    /// GPIO with the same instance's RX mux, so `pin % 4 == 1`.
    pub rx_pin: u8,
    /// Push asynchronous notification frames.
    pub notify_enable: bool,
    /// 9600 to 1000000; 115200 by default.
    pub baud: u32,
}

impl Default for UartCtrlConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            tx_pin: 16,
            rx_pin: 17,
            notify_enable: false,
            baud: 115_200,
        }
    }
}

impl UartCtrlConfig {
    pub const SIZE: usize = 8;

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.enabled as u8;
        b[1] = self.tx_pin;
        b[2] = self.rx_pin;
        b[3] = self.notify_enable as u8;
        put_u32(&mut b, 4, self.baud);
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "UartCtrlConfig")?;
        Ok(Self {
            enabled: b[0] != 0,
            tx_pin: b[1],
            rx_pin: b[2],
            notify_enable: b[3] != 0,
            baud: u32at(b, 4),
        })
    }
}

/// `REQ_SET/GET_I2C_CONFIG`, 8 bytes (config.h:535-541).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct I2cCtrlConfig {
    pub enabled: bool,
    /// GPIO with an I2Cx SDA mux, so an even pin.
    pub sda_pin: u8,
    /// The same instance's SCL, so the odd pin above it.
    pub scl_pin: u8,
    /// 7-bit target address, 0x08 to 0x77; 0x42 by default.
    pub address: u8,
}

impl Default for I2cCtrlConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            sda_pin: 18,
            scl_pin: 19,
            address: 0x42,
        }
    }
}

impl I2cCtrlConfig {
    pub const SIZE: usize = 8;

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.enabled as u8;
        b[1] = self.sda_pin;
        b[2] = self.scl_pin;
        b[3] = self.address;
        // 4-7 reserved.
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "I2cCtrlConfig")?;
        Ok(Self {
            enabled: b[0] != 0,
            sda_pin: b[1],
            scl_pin: b[2],
            address: b[3],
        })
    }
}

/// `REQ_GET_CTRL_IFACE_STATUS`, 8 bytes (config.h:547-554).
///
/// `*_live` is whether the interface is actually up, which differs from the
/// config's `enabled` after a boot-time pin collision kept it down.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CtrlIfaceStatus {
    pub uart_last_status: u8,
    pub uart_live: bool,
    pub i2c_last_status: u8,
    pub i2c_live: bool,
    pub proto_version: u8,
}

impl CtrlIfaceStatus {
    pub const SIZE: usize = 8;

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.uart_last_status;
        b[1] = self.uart_live as u8;
        b[2] = self.i2c_last_status;
        b[3] = self.i2c_live as u8;
        b[4] = self.proto_version;
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "CtrlIfaceStatus")?;
        Ok(Self {
            uart_last_status: b[0],
            uart_live: b[1] != 0,
            i2c_last_status: b[2],
            i2c_live: b[3] != 0,
            proto_version: b[4],
        })
    }
}

// --------------------------------------------------------------- diagnostics

/// Per-instance S/PDIF buffer fill, 8 bytes (config.h:1018-1026).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SpdifBufferStats {
    pub consumer_free: u8,
    pub consumer_prepared: u8,
    pub consumer_playing: u8,
    pub consumer_fill_pct: u8,
    pub consumer_min_fill_pct: u8,
    pub consumer_max_fill_pct: u8,
}

impl SpdifBufferStats {
    pub const SIZE: usize = 8;

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.consumer_free;
        b[1] = self.consumer_prepared;
        b[2] = self.consumer_playing;
        b[3] = self.consumer_fill_pct;
        b[4] = self.consumer_min_fill_pct;
        b[5] = self.consumer_max_fill_pct;
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "SpdifBufferStats")?;
        Ok(Self {
            consumer_free: b[0],
            consumer_prepared: b[1],
            consumer_playing: b[2],
            consumer_fill_pct: b[3],
            consumer_min_fill_pct: b[4],
            consumer_max_fill_pct: b[5],
        })
    }
}

/// PDM buffer fill, 8 bytes (config.h:1028-1036).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PdmBufferStats {
    pub dma_fill_pct: u8,
    pub dma_min_fill_pct: u8,
    pub dma_max_fill_pct: u8,
    pub ring_fill_pct: u8,
    pub ring_min_fill_pct: u8,
    pub ring_max_fill_pct: u8,
}

impl PdmBufferStats {
    pub const SIZE: usize = 8;

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.dma_fill_pct;
        b[1] = self.dma_min_fill_pct;
        b[2] = self.dma_max_fill_pct;
        b[3] = self.ring_fill_pct;
        b[4] = self.ring_min_fill_pct;
        b[5] = self.ring_max_fill_pct;
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "PdmBufferStats")?;
        Ok(Self {
            dma_fill_pct: b[0],
            dma_min_fill_pct: b[1],
            dma_max_fill_pct: b[2],
            ring_fill_pct: b[3],
            ring_min_fill_pct: b[4],
            ring_max_fill_pct: b[5],
        })
    }
}

/// `REQ_GET_BUFFER_STATS`, 44 bytes: 4 + 4x8 + 8 (config.h:1038-1044).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BufferStatsPacket {
    /// `NUM_SPDIF_INSTANCES`, 2 or 4. Entries past it are zeroed.
    pub num_spdif: u8,
    pub flags: u8,
    /// Monotonic counter, wraps at 65535.
    pub sequence: u16,
    pub spdif: [SpdifBufferStats; 4],
    pub pdm: PdmBufferStats,
}

impl BufferStatsPacket {
    pub const SIZE: usize = 44;
    pub const SPDIF_AT: usize = 4;
    pub const PDM_AT: usize = 36;
    pub const FLAG_PDM_ACTIVE: u8 = 0x01;
    pub const FLAG_STREAMING: u8 = 0x02;

    pub fn pdm_active(&self) -> bool {
        self.flags & Self::FLAG_PDM_ACTIVE != 0
    }

    pub fn streaming(&self) -> bool {
        self.flags & Self::FLAG_STREAMING != 0
    }

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.num_spdif;
        b[1] = self.flags;
        put_u16(&mut b, 2, self.sequence);
        for (i, s) in self.spdif.iter().enumerate() {
            let o = Self::SPDIF_AT + i * SpdifBufferStats::SIZE;
            b[o..o + SpdifBufferStats::SIZE].copy_from_slice(&s.encode());
        }
        b[Self::PDM_AT..Self::PDM_AT + PdmBufferStats::SIZE].copy_from_slice(&self.pdm.encode());
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "BufferStatsPacket")?;
        let mut spdif = [SpdifBufferStats::default(); 4];
        for (i, s) in spdif.iter_mut().enumerate() {
            let o = Self::SPDIF_AT + i * SpdifBufferStats::SIZE;
            *s = SpdifBufferStats::decode(&b[o..o + SpdifBufferStats::SIZE])?;
        }
        Ok(Self {
            num_spdif: b[0],
            flags: b[1],
            sequence: u16at(b, 2),
            spdif,
            pdm: PdmBufferStats::decode(&b[Self::PDM_AT..])?,
        })
    }
}

/// `REQ_GET_SPDIF_RX_STATUS`, 16 bytes (config.h:447; survey 6.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpdifRxStatusPacket {
    pub state: SpdifRxState,
    pub input_source: InputSource,
    pub lock_count: u8,
    pub loss_count: u8,
    pub sample_rate: u32,
    pub parity_errors: u32,
    pub fifo_fill_pct: u16,
    /// The two diagnostic bytes the Console shows as the receiver library's
    /// state and its callback count; the survey calls them reserved.
    pub lib_state: u8,
    pub callback_counts: u8,
}

impl Default for SpdifRxStatusPacket {
    fn default() -> Self {
        Self {
            state: SpdifRxState::Inactive,
            input_source: InputSource::Usb,
            lock_count: 0,
            loss_count: 0,
            sample_rate: 0,
            parity_errors: 0,
            fifo_fill_pct: 0,
            lib_state: 0,
            callback_counts: 0,
        }
    }
}

impl SpdifRxStatusPacket {
    pub const SIZE: usize = 16;

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.state.to_raw();
        b[1] = self.input_source.to_raw();
        b[2] = self.lock_count;
        b[3] = self.loss_count;
        put_u32(&mut b, 4, self.sample_rate);
        put_u32(&mut b, 8, self.parity_errors);
        put_u16(&mut b, 12, self.fifo_fill_pct);
        b[14] = self.lib_state;
        b[15] = self.callback_counts;
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "SpdifRxStatusPacket")?;
        Ok(Self {
            state: SpdifRxState::from_raw(b[0]),
            input_source: InputSource::from_raw(b[1]),
            lock_count: b[2],
            loss_count: b[3],
            sample_rate: u32at(b, 4),
            parity_errors: u32at(b, 8),
            fifo_fill_pct: u16at(b, 12),
            lib_state: b[14],
            callback_counts: b[15],
        })
    }
}

/// `REQ_GET_SPDIF_INPUT_CONFIG`, 6 bytes at v1.1.6 (config.h:456-457).
///
/// The mask is `(spdif_rx_enabled_ext << 1) | 1`, so **bit 0 is input 1 and is
/// always set**, and bits 1..3 are the optional inputs 2..4. The bulk packet's
/// `spdif_rx_enabled_ext_p1` is the other mask, shifted down one bit; the two
/// are easy to confuse and one bit apart.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpdifInputConfig {
    pub count: u8,
    pub enable_mask: u8,
    pub gpio: [u8; 4],
}

impl SpdifInputConfig {
    pub const SIZE: usize = 6;

    /// Whether S/PDIF input `index` (0-based, so 0 is input 1) is enabled.
    pub fn enabled(&self, index: u8) -> bool {
        index < 8 && self.enable_mask & (1u8 << index) != 0
    }

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.count;
        b[1] = self.enable_mask;
        b[2..6].copy_from_slice(&self.gpio);
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "SpdifInputConfig")?;
        let mut gpio = [0u8; 4];
        gpio.copy_from_slice(&b[2..6]);
        Ok(Self {
            count: b[0],
            enable_mask: b[1],
            gpio,
        })
    }
}

/// `REQ_GET_ADAT_INPUT_STATUS`, 20 bytes (config.h:354 and the survey's 1.7).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AdatInputStatusPacket {
    /// 0 inactive, 1 acquiring, 2 syncing, 3 locked, 4 relocking.
    pub state: u8,
    /// Live mode: 0 master, 1 slave.
    pub clock_mode: u8,
    pub enabled: bool,
    /// Configured RX GPIO; 0xFF = unset.
    pub pin: u8,
    /// False while parked because the device rate is above 48 kHz.
    pub rate_ok: bool,
    pub lock_count: u8,
    pub loss_count: u8,
    pub slip_count: u8,
    pub header_err: u16,
    pub detected_rate: u32,
    pub measured_hz: u32,
}

impl AdatInputStatusPacket {
    pub const SIZE: usize = 20;
    pub const PIN_UNSET: u8 = 0xFF;

    pub fn is_locked(&self) -> bool {
        self.state == 3
    }

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.state;
        b[1] = self.clock_mode;
        b[2] = self.enabled as u8;
        b[3] = self.pin;
        b[4] = self.rate_ok as u8;
        b[5] = self.lock_count;
        b[6] = self.loss_count;
        b[7] = self.slip_count;
        put_u16(&mut b, 8, self.header_err);
        // 10-11 reserved.
        put_u32(&mut b, 12, self.detected_rate);
        put_u32(&mut b, 16, self.measured_hz);
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "AdatInputStatusPacket")?;
        Ok(Self {
            state: b[0],
            clock_mode: b[1],
            enabled: b[2] != 0,
            pin: b[3],
            rate_ok: b[4] != 0,
            lock_count: b[5],
            loss_count: b[6],
            slip_count: b[7],
            header_err: u16at(b, 8),
            detected_rate: u32at(b, 12),
            measured_hz: u32at(b, 16),
        })
    }
}

/// `REQ_GET_ADAT_STATUS`, 8 bytes (config.h:354). RP2040 answers all zeros.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AdatStatus {
    /// Configured and persisted intent.
    pub enabled: bool,
    /// The stream is running right now.
    pub active: bool,
    pub pin: u8,
    /// The current sample rate is 44.1 or 48 kHz; ADAT auto-suspends above.
    pub rate_ok: bool,
    pub resync_count: u16,
    /// Emergency local resyncs. Should stay 0.
    pub slip_count: u16,
}

impl AdatStatus {
    pub const SIZE: usize = 8;

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.enabled as u8;
        b[1] = self.active as u8;
        b[2] = self.pin;
        b[3] = self.rate_ok as u8;
        put_u16(&mut b, 4, self.resync_count);
        put_u16(&mut b, 6, self.slip_count);
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "AdatStatus")?;
        Ok(Self {
            enabled: b[0] != 0,
            active: b[1] != 0,
            pin: b[2],
            rate_ok: b[3] != 0,
            resync_count: u16at(b, 4),
            slip_count: u16at(b, 6),
        })
    }
}

/// `REQ_GET_I2S_SLAVE_STATUS`, 16 bytes (config.h:480).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct I2sSlaveStatusPacket {
    /// 0 inactive, 1 acquiring, 2 relocking, 3 locked.
    pub state: u8,
    /// Live mode: 0 master, 1 slave.
    pub clock_mode: u8,
    pub lock_count: u8,
    pub loss_count: u8,
    /// Snapped Hz, 0 unless locked.
    pub detected_rate: u32,
    /// Raw measured external rate; 0 when there are no clocks.
    pub measured_hz: u32,
}

impl I2sSlaveStatusPacket {
    pub const SIZE: usize = 16;

    pub fn is_locked(&self) -> bool {
        self.state == 3
    }

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.state;
        b[1] = self.clock_mode;
        b[2] = self.lock_count;
        b[3] = self.loss_count;
        put_u32(&mut b, 4, self.detected_rate);
        put_u32(&mut b, 8, self.measured_hz);
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "I2sSlaveStatusPacket")?;
        Ok(Self {
            state: b[0],
            clock_mode: b[1],
            lock_count: b[2],
            loss_count: b[3],
            detected_rate: u32at(b, 4),
            measured_hz: u32at(b, 8),
        })
    }
}

/// `REQ_GET_LG_SOUND_SYNC_STATUS`, 16 bytes (config.h:496; the same layout as
/// bulk section 16, bulk_params.h:252-258).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LgSoundSyncStatus {
    /// The user gate, the only field honoured on a bulk SET.
    pub enabled: bool,
    pub present: bool,
    /// 0 to 100; 0xFF means never decoded since boot.
    pub volume: u8,
    pub muted: bool,
}

impl Default for LgSoundSyncStatus {
    fn default() -> Self {
        Self {
            enabled: false,
            present: false,
            volume: 0xFF,
            muted: false,
        }
    }
}

impl LgSoundSyncStatus {
    pub const SIZE: usize = 16;
    pub const VOLUME_UNKNOWN: u8 = 0xFF;

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = self.enabled as u8;
        b[1] = self.present as u8;
        b[2] = self.volume;
        b[3] = self.muted as u8;
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "LgSoundSyncStatus")?;
        Ok(Self {
            enabled: b[0] != 0,
            present: b[1] != 0,
            volume: b[2],
            muted: b[3] != 0,
        })
    }
}

// ------------------------------------------------------------------- presets

/// `REQ_PRESET_GET_DIR`, 7 bytes (config.h:296).
///
/// The old specification said 6; the firmware has always answered 7, the last
/// byte being the master-volume mode.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PresetDirectory {
    /// Bit N = slot N holds a preset.
    pub occupied: u16,
    pub startup_mode: u8,
    pub default_slot: u8,
    pub last_active: u8,
    /// 0 independent, 1 with preset.
    pub output_config_mode: u8,
    /// 0 independent, 1 with preset.
    pub master_volume_mode: u8,
}

impl PresetDirectory {
    pub const SIZE: usize = 7;

    pub fn is_occupied(&self, slot: u8) -> bool {
        slot < 16 && self.occupied & (1u16 << slot) != 0
    }

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        put_u16(&mut b, 0, self.occupied);
        b[2] = self.startup_mode;
        b[3] = self.default_slot;
        b[4] = self.last_active;
        b[5] = self.output_config_mode;
        b[6] = self.master_volume_mode;
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "PresetDirectory")?;
        Ok(Self {
            occupied: u16at(b, 0),
            startup_mode: b[2],
            default_slot: b[3],
            last_active: b[4],
            output_config_mode: b[5],
            master_volume_mode: b[6],
        })
    }
}

/// The startup preset. `REQ_PRESET_SET_STARTUP` takes 2 bytes,
/// `REQ_PRESET_GET_STARTUP` answers 3 (config.h:301-302).
///
/// The asymmetry is the whole reason this is one type: the third byte is the
/// live active slot, which is read-only and cannot be written back.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PresetStartup {
    /// 0 = a specified slot, 1 = the last active slot.
    pub mode: u8,
    pub default_slot: u8,
    /// Read-only: the slot the device is on now.
    pub last_active: u8,
}

impl PresetStartup {
    /// What the GET answers.
    pub const SIZE: usize = 3;
    /// What the SET takes.
    pub const SET_SIZE: usize = 2;

    pub fn encode(&self) -> [u8; Self::SET_SIZE] {
        [self.mode, self.default_slot]
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "PresetStartup")?;
        Ok(Self {
            mode: b[0],
            default_slot: b[1],
            last_active: b[2],
        })
    }

    /// Decode the two-byte SET form, which has no `last_active`.
    pub fn decode_set(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SET_SIZE, "PresetStartup")?;
        Ok(Self {
            mode: b[0],
            default_slot: b[1],
            last_active: 0,
        })
    }
}

// -------------------------------------------------------------------- system

/// `REQ_GET_STATUS` with `wValue = 9`, the canonical meter read
/// (config.h:979-985, vendor_commands.c:1972-1991).
///
/// `peaks[n] u16, cpu0, cpu1, clip_flags u32, active_inputs`, so `n*2 + 7`
/// bytes: 41 on RP2350, 21 on RP2040. The channel count comes from the length,
/// which is why there is no size constant. The released `commands.md` documents
/// `n*2 + 4` with a 16-bit clip mask, two changes out of date: a 16-bit mask
/// cannot even represent channel 17's flag.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SystemStatus {
    pub peaks: Vec<u16>,
    pub cpu0: u8,
    pub cpu1: u8,
    /// Sticky per-channel clip latch, cleared by `REQ_CLEAR_CLIPS`.
    pub clip_flags: u32,
    pub active_inputs: u8,
}

impl SystemStatus {
    /// Everything after the peaks: two loads, the 32-bit latch, the input count.
    pub const TAIL: usize = 7;

    pub fn wire_len(num_channels: u8) -> usize {
        num_channels as usize * 2 + Self::TAIL
    }

    /// Peak of channel `i` as 0..1.
    pub fn peak(&self, channel: usize) -> f32 {
        self.peaks.get(channel).copied().unwrap_or(0) as f32 / 65535.0
    }

    /// Shifted on a u32, not a u16: channel 17 exists and its bit is real.
    pub fn clipped(&self, channel: usize) -> bool {
        channel < 32 && self.clip_flags & (1u32 << channel) != 0
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(Self::wire_len(self.peaks.len() as u8));
        for p in &self.peaks {
            b.extend(p.to_le_bytes());
        }
        b.push(self.cpu0);
        b.push(self.cpu1);
        b.extend(self.clip_flags.to_le_bytes());
        b.push(self.active_inputs);
        b
    }

    /// Decode, taking the channel count from the length.
    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::TAIL, "SystemStatus")?;
        let n = (b.len() - Self::TAIL) / 2;
        Self::decode_sized(b, n as u8)
    }

    pub fn decode_sized(b: &[u8], num_channels: u8) -> Result<Self, PacketError> {
        let want = Self::wire_len(num_channels);
        need(b, want, "SystemStatus")?;
        let n = num_channels as usize;
        Ok(Self {
            peaks: (0..n).map(|i| u16at(b, i * 2)).collect(),
            cpu0: b[n * 2],
            cpu1: b[n * 2 + 1],
            clip_flags: u32at(b, n * 2 + 2),
            active_inputs: b[n * 2 + 6],
        })
    }
}

// ------------------------------------------------------------ identification

/// The pre-release part of a firmware version (config.h:664-667).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum Prerelease {
    /// A final release: the beta byte is 0.
    Final,
    /// Beta N of its patch, 1 to 255.
    Beta(u8),
    /// A beta built before the ordinal existed. It answers GET_PLATFORM short
    /// and so claims to be a final release it cannot be (the Console's
    /// `FirmwareVersion.earlyBeta`). 1.1.6 beta 1 and beta 2 are the ones.
    EarlyBeta,
}

/// A firmware release: three plain numbers and a pre-release ordinal
/// (config.h:655-667, `Documentation/Features/firmware_versioning_spec.md`).
///
/// Ordered by `(major, minor, patch, beta == 0 ? 256 : beta)`, because a beta
/// comes before the final release of its patch although final is encoded as
/// 0 (firmware_versioning_spec.md:111). An early beta sorts below every
/// numbered beta, as the Console sorts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct FirmwareVersion {
    pub major: u8,
    pub minor: u8,
    pub patch: u8,
    pub pre: Prerelease,
}

impl FirmwareVersion {
    /// The first release whose every build reports the beta ordinal; a short
    /// GET_PLATFORM reply claiming this or later is an early beta, not a final
    /// release (the Console's `FirmwareVersion.firstWithOrdinal`).
    pub const FIRST_WITH_ORDINAL: (u8, u8, u8) = (1, 1, 6);

    /// `beta` as the wire carries it: 0 is final.
    pub fn new(major: u8, minor: u8, patch: u8, beta: u8) -> Self {
        Self {
            major,
            minor,
            patch,
            pre: match beta {
                0 => Prerelease::Final,
                n => Prerelease::Beta(n),
            },
        }
    }

    /// The release the vendored headers describe (config.h:659-667): what this
    /// build speaks, and what a refused device is told to update to.
    pub fn expected() -> Self {
        use crate::generated::firmware::*;
        Self::new(
            FW_VERSION_MAJOR as u8,
            FW_VERSION_MINOR as u8,
            FW_VERSION_PATCH as u8,
            FW_VERSION_BETA as u8,
        )
    }

    /// The beta ordinal as the wire carries it; an early beta has none.
    pub fn beta(&self) -> Option<u8> {
        match self.pre {
            Prerelease::Final => Some(0),
            Prerelease::Beta(n) => Some(n),
            Prerelease::EarlyBeta => None,
        }
    }

    fn sort_key(&self) -> (u8, u8, u8, u16) {
        let rank = match self.pre {
            Prerelease::Final => 256,
            Prerelease::Beta(n) => n as u16,
            Prerelease::EarlyBeta => 0,
        };
        (self.major, self.minor, self.patch, rank)
    }
}

impl PartialOrd for FirmwareVersion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FirmwareVersion {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.sort_key().cmp(&other.sort_key())
    }
}

/// "1.1.6", "1.1.6 beta 4" or "1.1.6 early beta", the Console's spelling.
impl std::fmt::Display for FirmwareVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        match self.pre {
            Prerelease::Final => Ok(()),
            Prerelease::Beta(n) => write!(f, " beta {n}"),
            Prerelease::EarlyBeta => write!(f, " early beta"),
        }
    }
}

/// `REQ_GET_PLATFORM` (config.h:325, vendor_commands.c:2638-2655): asked for
/// with a length of 7, answered with as many bytes as the firmware knows.
///
/// `[0]` platform, `[1]` major, `[2]` legacy `(minor << 4) | patch`, `[3]`
/// output count, `[4]` minor, `[5]` patch, `[6]` beta ordinal. Older firmware
/// answers 4 or 6 bytes. The decode follows the host rule of
/// firmware_versioning_spec.md:109: minor and patch from bytes 4 and 5 when
/// at least 6 arrive, else from the nibbles, never mixing the two; beta from
/// byte 6 when 7 arrive. A short reply that claims 1.1.6 or later is an early
/// beta, as the Console reads it, rather than the final release it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformInfo {
    pub platform_id: u8,
    pub num_output_channels: u8,
    pub version: FirmwareVersion,
    /// How many bytes the device answered: 4, 6 or 7.
    pub reply_len: usize,
}

impl PlatformInfo {
    /// What to ask for (firmware_versioning_spec.md:109).
    pub const REQUEST_LEN: usize = 7;
    /// The oldest reply, and the least that identifies a device.
    pub const MIN_LEN: usize = 4;

    pub fn platform(&self) -> Platform {
        Platform::from_id(self.platform_id)
    }

    /// The version as the Console spells it.
    pub fn firmware(&self) -> String {
        self.version.to_string()
    }

    /// The full 7-byte reply this firmware would send.
    pub fn encode(&self) -> [u8; Self::REQUEST_LEN] {
        let v = &self.version;
        [
            self.platform_id,
            v.major,
            (v.minor << 4) | (v.patch & 0x0F),
            self.num_output_channels,
            v.minor,
            v.patch,
            v.beta().unwrap_or(0),
        ]
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::MIN_LEN, "PlatformInfo")?;
        let major = b[1];
        let (minor, patch) = if b.len() >= 6 {
            (b[4], b[5])
        } else {
            (b[2] >> 4, b[2] & 0x0F)
        };
        let version = if b.len() >= 7 {
            FirmwareVersion::new(major, minor, patch, b[6])
        } else if (major, minor, patch) >= FirmwareVersion::FIRST_WITH_ORDINAL {
            FirmwareVersion {
                major,
                minor,
                patch,
                pre: Prerelease::EarlyBeta,
            }
        } else {
            FirmwareVersion::new(major, minor, patch, 0)
        };
        Ok(Self {
            platform_id: b[0],
            num_output_channels: b[3],
            version,
            reply_len: b.len().min(Self::REQUEST_LEN),
        })
    }
}

/// `REQ_GET_BUILD_INFO`, 64 bytes (config.h:326, vendor_commands.c:2657-
/// 2667): `git describe --always --dirty` in bytes 0-47 and the build date
/// `YYYY-MM-DD` in bytes 48-59, both NUL padded, then four zero bytes.
///
/// For people only: nothing may gate on it (config.h:326-327). Firmware
/// before beta4 stalls the request.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct BuildInfo {
    pub describe: String,
    pub date: String,
}

impl BuildInfo {
    pub const SIZE: usize = 64;
    const DESCRIBE_LEN: usize = 48;
    const DATE_AT: usize = 48;
    const DATE_LEN: usize = 12;

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        put_name(&mut b, 0, Self::DESCRIBE_LEN, &self.describe);
        put_name(&mut b, Self::DATE_AT, Self::DATE_LEN, &self.date);
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::DATE_AT + Self::DATE_LEN, "BuildInfo")?;
        Ok(Self {
            describe: name_at(b, 0, Self::DESCRIBE_LEN),
            date: name_at(b, Self::DATE_AT, Self::DATE_LEN),
        })
    }
}

// ----------------------------------------------------------- aux outputs

/// `CsBinding.extras` bits on an auxiliary output slot (control_surfaces.h:
/// 425-437). Every other component type writes 0.
pub mod aux_extras {
    use crate::generated::cs;
    /// Boot with the output on.
    pub const BOOT_ON: u8 = cs::CS_AUX_X_BOOT_ON as u8;
    /// `REQ_CS_SAVE` folds the live state and level into the boot fields.
    pub const BOOT_SAVED: u8 = cs::CS_AUX_X_BOOT_SAVED as u8;
    /// AUX_PWM only: linear duty rather than the perceptual curve.
    pub const LINEAR: u8 = cs::CS_AUX_X_LINEAR as u8;
    pub const MASK: u8 = cs::CS_AUX_X_MASK as u8;
}

/// `REQ_GET_CS_AUX_STATE` with `wValue = 0xFFFF`, 48 bytes (config.h:138-140):
/// the on/off state of every binding slot, then every slot's level as an 8.8
/// percentage. Slots that are not auxiliary outputs read zero.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CsAuxStates {
    pub state: [u8; Self::SLOTS],
    pub level_q8: [u16; Self::SLOTS],
}

impl CsAuxStates {
    /// `CS_MAX_BINDINGS` (control_surfaces.h:333): the block has one entry
    /// per binding slot, whatever the caps say is configured.
    pub const SLOTS: usize = crate::generated::cs::CS_MAX_BINDINGS as usize;
    pub const SIZE: usize = Self::SLOTS * 3;

    pub fn is_on(&self, slot: usize) -> bool {
        self.state.get(slot).is_some_and(|s| *s != 0)
    }

    /// Level in percent; 0 on an on/off output.
    pub fn level_percent(&self, slot: usize) -> f32 {
        self.level_q8.get(slot).copied().unwrap_or(0) as f32 / 256.0
    }

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[..Self::SLOTS].copy_from_slice(&self.state);
        for (i, l) in self.level_q8.iter().enumerate() {
            put_u16(&mut b, Self::SLOTS + 2 * i, *l);
        }
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "CsAuxStates")?;
        let mut s = Self::default();
        s.state.copy_from_slice(&b[..Self::SLOTS]);
        for (i, l) in s.level_q8.iter_mut().enumerate() {
            *l = u16at(b, Self::SLOTS + 2 * i);
        }
        Ok(s)
    }
}

// ------------------------------------------------------ spectrum analyser

/// `RTA_CFG_VERSION` (rta.h:27). Every analyser record carries it, and
/// version 3 is incompatible with 2, so any other value is refused.
pub const RTA_VERSION: u8 = crate::generated::rta::RTA_CFG_VERSION as u8;

fn rta_version(b: &[u8], what: &'static str) -> Result<(), PacketError> {
    if b[0] != RTA_VERSION {
        return Err(PacketError::Version {
            what,
            got: b[0],
            want: RTA_VERSION,
        });
    }
    Ok(())
}

/// A level byte in dBFS: 0.5 dB steps, `level_zero` is 0 dBFS (rta_fft.h:44-45).
/// `level_zero` comes from [`RtaCaps`] rather than a constant.
pub fn rta_level_dbfs(level: u8, level_zero: u8) -> f32 {
    (level as f32 - level_zero as f32) * 0.5
}

/// `RtaConfig`, 12 bytes (rta.h:59-70), written with 0x08 and read with 0x09.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtaConfig {
    /// `RTA_TAP_INPUT` or `RTA_TAP_OUTPUT` (rta.h:30-31).
    pub tap: u8,
    /// Bit i is channel i at that tap; 0 stalls.
    pub channel_mask: u16,
    /// `RTA_ORDER_MIN..=RTA_ORDER_MAX` (rta_fft.h:39-40).
    pub fft_order: u8,
    /// Averaging time constant, ms; 0 turns the extra averaging off.
    pub avg_ms: u16,
    /// Peak decay, dB per second; 0 turns peak hold off.
    pub peak_decay_db_s: u8,
    /// `RTA_FLAG_*` (rta.h:34).
    pub flags: u8,
}

impl RtaConfig {
    pub const SIZE: usize = 12;

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = RTA_VERSION;
        b[1] = self.tap;
        put_u16(&mut b, 2, self.channel_mask);
        b[4] = self.fft_order;
        // 5 reserved0
        put_u16(&mut b, 6, self.avg_ms);
        b[8] = self.peak_decay_db_s;
        b[9] = self.flags;
        // 10-11 reserved
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "RtaConfig")?;
        rta_version(b, "RtaConfig")?;
        Ok(Self {
            tap: b[1],
            channel_mask: u16at(b, 2),
            fft_order: b[4],
            avg_ms: u16at(b, 6),
            peak_decay_db_s: b[8],
            flags: b[9],
        })
    }
}

/// `RtaCaps`, 16 bytes (rta.h:72-87), from 0x0A with `wValue = 0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RtaCaps {
    pub input_channels: u8,
    pub output_channels: u8,
    pub order_min: u8,
    pub order_max: u8,
    pub order_default: u8,
    /// Bands 0.. that come from the continuous bass bank.
    pub bass_bands: u8,
    pub max_bands: u8,
    /// The level byte that means 0 dBFS.
    pub level_zero: u8,
    pub dynamic_range_db: u8,
    pub idle_timeout_ms: u16,
    pub max_bin_frame: u16,
    pub bass_dynamic_range_db: u16,
}

impl RtaCaps {
    pub const SIZE: usize = 16;
    /// Band centres per chunk of 0x0A with `wValue >= 1` (rta.h:140-142).
    pub const CENTRES_PER_CHUNK: usize = 32;

    /// A level byte in dBFS, against this device's zero.
    pub fn level_dbfs(&self, level: u8) -> f32 {
        rta_level_dbfs(level, self.level_zero)
    }

    /// The `wValue`s that fetch the band-centre table, one chunk each.
    pub fn centre_chunks(&self) -> std::ops::RangeInclusive<u16> {
        let n = (self.max_bands as usize).div_ceil(Self::CENTRES_PER_CHUNK);
        1..=n as u16
    }

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = RTA_VERSION;
        b[1] = self.input_channels;
        b[2] = self.output_channels;
        b[3] = self.order_min;
        b[4] = self.order_max;
        b[5] = self.order_default;
        b[6] = self.bass_bands;
        b[7] = self.max_bands;
        b[8] = self.level_zero;
        b[9] = self.dynamic_range_db;
        put_u16(&mut b, 10, self.idle_timeout_ms);
        put_u16(&mut b, 12, self.max_bin_frame);
        put_u16(&mut b, 14, self.bass_dynamic_range_db);
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "RtaCaps")?;
        rta_version(b, "RtaCaps")?;
        Ok(Self {
            input_channels: b[1],
            output_channels: b[2],
            order_min: b[3],
            order_max: b[4],
            order_default: b[5],
            bass_bands: b[6],
            max_bands: b[7],
            level_zero: b[8],
            dynamic_range_db: b[9],
            idle_timeout_ms: u16at(b, 10),
            max_bin_frame: u16at(b, 12),
            bass_dynamic_range_db: u16at(b, 14),
        })
    }
}

/// One chunk of the band-centre table: up to 32 `u16` LE frequencies in Hz
/// (rta.h:140-142, rta.c:393-405). The length comes from the reply.
pub fn decode_rta_centres(b: &[u8]) -> Vec<u16> {
    (0..b.len() / 2).map(|i| u16at(b, 2 * i)).collect()
}

/// `RtaBandFrame`, 82 bytes (rta.h:89-99), from 0x0B for one channel or
/// back to back from 0x0F for every live one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtaBandFrame {
    pub channel: u8,
    /// Per-channel frame counter.
    pub seq: u8,
    /// Bands in use: 34 at 44.1 and 48 kHz, 37 at 96 kHz.
    pub n_bands: u8,
    /// Since the last frame; [`RtaBandFrame::NEVER`] if none yet.
    pub age_ms: u16,
    pub avg: [u8; Self::MAX_BANDS],
    pub peak: [u8; Self::MAX_BANDS],
}

impl RtaBandFrame {
    /// `RTA_MAX_BANDS` (rta_fft.h:42): the arrays are this long whatever
    /// `n_bands` says.
    pub const MAX_BANDS: usize = crate::generated::rta::RTA_MAX_BANDS as usize;
    pub const SIZE: usize = 8 + 2 * Self::MAX_BANDS;
    /// `age_ms` of a channel that has never produced a frame.
    pub const NEVER: u16 = 0xFFFF;

    /// The averaged levels of the bands in use.
    pub fn avg_levels(&self) -> &[u8] {
        &self.avg[..(self.n_bands as usize).min(Self::MAX_BANDS)]
    }

    /// The peak levels of the bands in use.
    pub fn peak_levels(&self) -> &[u8] {
        &self.peak[..(self.n_bands as usize).min(Self::MAX_BANDS)]
    }

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = RTA_VERSION;
        b[1] = self.channel;
        b[2] = self.seq;
        b[3] = self.n_bands;
        put_u16(&mut b, 4, self.age_ms);
        // 6-7 reserved
        b[8..8 + Self::MAX_BANDS].copy_from_slice(&self.avg);
        b[8 + Self::MAX_BANDS..].copy_from_slice(&self.peak);
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "RtaBandFrame")?;
        rta_version(b, "RtaBandFrame")?;
        let mut avg = [0u8; Self::MAX_BANDS];
        let mut peak = [0u8; Self::MAX_BANDS];
        avg.copy_from_slice(&b[8..8 + Self::MAX_BANDS]);
        peak.copy_from_slice(&b[8 + Self::MAX_BANDS..Self::SIZE]);
        Ok(Self {
            channel: b[1],
            seq: b[2],
            n_bands: b[3],
            age_ms: u16at(b, 4),
            avg,
            peak,
        })
    }

    /// The reply to 0x0F: one frame per selected, live channel, ascending,
    /// in 82-byte strides; empty when nothing is live (vendor_commands.c:
    /// 4334-4355). A trailing partial frame is an error, not a frame.
    pub fn decode_all(b: &[u8]) -> Result<Vec<Self>, PacketError> {
        if !b.len().is_multiple_of(Self::SIZE) {
            return Err(PacketError::TooShort {
                what: "RtaBandFrame",
                want: b.len().div_ceil(Self::SIZE) * Self::SIZE,
                got: b.len(),
            });
        }
        b.chunks(Self::SIZE).map(Self::decode).collect()
    }
}

/// `RtaBinFrameHeader`, 16 bytes (rta.h:101-110), at the start of the bin
/// frame 0x0C reads from a byte offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RtaBinHeader {
    pub channel: u8,
    /// Repeated as the frame's last byte.
    pub seq: u8,
    pub fft_order: u8,
    pub sample_rate_hz: u32,
    /// `N / 2`: the frame is `16 + n_bins + 1` bytes.
    pub n_bins: u16,
}

impl RtaBinHeader {
    pub const SIZE: usize = 16;

    /// The whole frame's length for this header.
    pub fn frame_len(&self) -> usize {
        Self::SIZE + self.n_bins as usize + 1
    }

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = RTA_VERSION;
        b[1] = self.channel;
        b[2] = self.seq;
        b[3] = self.fft_order;
        put_u32(&mut b, 4, self.sample_rate_hz);
        put_u16(&mut b, 8, self.n_bins);
        // 10-15 reserved[3]
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "RtaBinHeader")?;
        rta_version(b, "RtaBinHeader")?;
        Ok(Self {
            channel: b[1],
            seq: b[2],
            fft_order: b[3],
            sample_rate_hz: u32at(b, 4),
            n_bins: u16at(b, 8),
        })
    }
}

/// A whole bin frame, header, levels and tail.
///
/// 0x0C takes no lock (vendor_commands.c:4297-4311), so a frame read in
/// chunks can straddle an update. The device writes the tail as 0xFF before
/// it rewrites a frame and then copies the new sequence number into it, and
/// sequence numbers skip 0xFF (rta.c:128-131, 157-161, 254-256). A frame is
/// whole only when its tail matches its head and is not 0xFF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtaBinFrame {
    pub header: RtaBinHeader,
    pub levels: Vec<u8>,
}

impl RtaBinFrame {
    pub fn encode(&self) -> Vec<u8> {
        let mut b = self.header.encode().to_vec();
        b.extend(&self.levels);
        b.push(self.header.seq);
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        let header = RtaBinHeader::decode(b)?;
        let len = header.frame_len();
        need(b, len, "RtaBinFrame")?;
        let tail = b[len - 1];
        if tail == 0xFF || tail != header.seq {
            return Err(PacketError::Torn {
                head: header.seq,
                tail,
            });
        }
        Ok(Self {
            header,
            levels: b[RtaBinHeader::SIZE..len - 1].to_vec(),
        })
    }
}

/// `RtaStatus`, 24 bytes (rta.h:112-129), from 0x0D. Reading it does not
/// count as a read: it neither starts the analyser nor keeps it alive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RtaStatus {
    /// `RTA_STATE_*` (rta.h:36-38).
    pub state: u8,
    pub tap: u8,
    /// Being captured or transformed; `RTA_CH_NONE` (0xFF) when idle.
    pub channel: u8,
    pub live_count: u8,
    pub live_mask: u16,
    pub frames_per_s: u16,
    pub busy_us_per_s: u16,
    pub last_frame_us: u16,
    /// Since the last data read; 0xFFFF never.
    pub idle_ms: u16,
    pub sample_rate_hz: u32,
    /// 0 with a bass bank at this rate, 0xFF for an unsupported rate.
    pub first_band: u8,
    pub bass_busy_us_per_s: u16,
}

impl RtaStatus {
    pub const SIZE: usize = 24;

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0] = RTA_VERSION;
        b[1] = self.state;
        b[2] = self.tap;
        b[3] = self.channel;
        // 4 reserved0
        b[5] = self.live_count;
        put_u16(&mut b, 6, self.live_mask);
        put_u16(&mut b, 8, self.frames_per_s);
        put_u16(&mut b, 10, self.busy_us_per_s);
        put_u16(&mut b, 12, self.last_frame_us);
        put_u16(&mut b, 14, self.idle_ms);
        put_u32(&mut b, 16, self.sample_rate_hz);
        b[20] = self.first_band;
        // 21 reserved1
        put_u16(&mut b, 22, self.bass_busy_us_per_s);
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "RtaStatus")?;
        rta_version(b, "RtaStatus")?;
        Ok(Self {
            state: b[1],
            tap: b[2],
            channel: b[3],
            live_count: b[5],
            live_mask: u16at(b, 6),
            frames_per_s: u16at(b, 8),
            busy_us_per_s: u16at(b, 10),
            last_frame_us: u16at(b, 12),
            idle_ms: u16at(b, 14),
            sample_rate_hz: u32at(b, 16),
            first_band: b[20],
            bass_busy_us_per_s: u16at(b, 22),
        })
    }
}

// ------------------------------------------------ subharm and limiter reads

/// Decode `n x u16 LE` with the count taken from the reply. The subharm and
/// limiter meters are sized by `NUM_OUTPUT_CHANNELS`, which differs between
/// platforms (18 bytes on an RP2350, 10 on an RP2040), so no length is
/// assumed here.
fn u16_table(b: &[u8], what: &'static str) -> Result<Vec<u16>, PacketError> {
    if !b.len().is_multiple_of(2) {
        return Err(PacketError::TooShort {
            what,
            want: b.len() + 1,
            got: b.len(),
        });
    }
    Ok((0..b.len() / 2).map(|i| u16at(b, 2 * i)).collect())
}

/// `REQ_GET_SUBHARM_METER` (config.h:200): one `u16` per output, the synthesized
/// sub's peak on the same 0..32767 scale as the status peaks
/// (vendor_commands.c:2206-2216).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SubharmMeter {
    pub peaks: Vec<u16>,
}

impl SubharmMeter {
    /// Full scale (subharm.c:437-448).
    pub const FULL_SCALE: f32 = 32767.0;

    /// Peak of output `k` as 0..1.
    pub fn peak(&self, output: usize) -> f32 {
        self.peaks.get(output).copied().unwrap_or(0) as f32 / Self::FULL_SCALE
    }

    pub fn encode(&self) -> Vec<u8> {
        self.peaks.iter().flat_map(|p| p.to_le_bytes()).collect()
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        Ok(Self {
            peaks: u16_table(b, "SubharmMeter")?,
        })
    }
}

/// `REQ_LIMITER` at index `LIMITER_GET_METER` (limiter.h:19): gain reduction
/// per output in 0.01 dB, 0 for none.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LimiterMeter {
    pub centi_db: Vec<u16>,
}

impl LimiterMeter {
    /// Gain reduction on output `k`, in dB.
    pub fn reduction_db(&self, output: usize) -> f32 {
        self.centi_db.get(output).copied().unwrap_or(0) as f32 / 100.0
    }

    pub fn encode(&self) -> Vec<u8> {
        self.centi_db.iter().flat_map(|p| p.to_le_bytes()).collect()
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        Ok(Self {
            centi_db: u16_table(b, "LimiterMeter")?,
        })
    }
}

/// `REQ_LIMITER` at index `LIMITER_GET_STATUS` (limiter.h:20), 4 bytes
/// (vendor_commands.c:2230-2237). Also the limiter's feature probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LimiterStatus {
    /// The lookahead delay is in the signal path right now.
    pub engaged: bool,
    /// `LIMITER_DELAY`, samples of latency while engaged.
    pub lookahead: u8,
    /// `LIMITER_BLOCK`.
    pub block: u8,
    pub num_outputs: u8,
}

impl LimiterStatus {
    pub const SIZE: usize = 4;

    pub fn encode(&self) -> [u8; Self::SIZE] {
        [
            self.engaged as u8,
            self.lookahead,
            self.block,
            self.num_outputs,
        ]
    }

    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        need(b, Self::SIZE, "LimiterStatus")?;
        Ok(Self {
            engaged: b[0] != 0,
            lookahead: b[1],
            block: b[2],
            num_outputs: b[3],
        })
    }
}

// ============================================================================
// Typed command lines
// ============================================================================
//
// The `:` line can build any of these packets from named fields, which is what
// keeps a structured payload typable at all: `cs.binding 3 type=encoder
// noun=master_volume action=step gpio=10,11 step=256 flags=accel`. Each
// typeable packet declares its fields once, and parsing, formatting and
// completion are all walks of that one declaration.

/// What one named field of a packet holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Bool,
    U8,
    I16,
    U16,
    U32,
    F32,
    /// A named choice, or its raw number.
    Enum(&'static [(&'static str, u8)]),
    /// A set of named bits, comma separated, or a raw mask.
    Flags(&'static [(&'static str, u8)]),
    /// One GPIO, or two separated by a comma. A single pin sets the second to
    /// `CS_GPIO_UNUSED`, which is what the firmware expects of a configured
    /// one-pin component.
    Gpio2,
    Mask32,
    Text(usize),
}

/// One named field of a packet, for parsing, completion and help.
#[derive(Debug, Clone, Copy)]
pub struct FieldSpec {
    pub name: &'static str,
    pub kind: FieldKind,
    pub help: &'static str,
}

const fn f(name: &'static str, kind: FieldKind, help: &'static str) -> FieldSpec {
    FieldSpec { name, kind, help }
}

// --------------------------------------------------------------- value tables

/// Component types (control_surfaces.h:128-146).
pub const CS_TYPES: &[(&str, u8)] = &[
    ("none", 0),
    ("button", 1),
    ("switch", 2),
    ("pot", 3),
    ("encoder", 4),
    ("led", 5),
    ("led_pwm", 6),
    ("ir", 7),
    ("display", 8),
    // Caps v18: containers that own a GPIO rather than drive a parameter.
    ("aux_out", 9),
    ("aux_pwm", 10),
];

/// Nouns (control_surfaces.h:152-252). The device reports how many it has, so
/// this table is for typing and labelling; a raw number always works too.
pub const CS_NOUNS: &[(&str, u8)] = &[
    ("user_volume", 0),
    ("master_volume", 1),
    ("user_mute", 2),
    ("loudness", 3),
    ("crossfeed", 4),
    ("leveller", 5),
    ("preset", 6),
    ("input_source", 7),
    ("clip", 8),
    ("eq_bypass", 9),
    ("lg_sync", 10),
    ("crossfeed_preset", 11),
    ("crossfeed_itd", 12),
    ("leveller_amount", 13),
    ("leveller_speed", 14),
    ("leveller_lookahead", 15),
    ("preamp", 16),
    ("output_gain", 17),
    ("output_mute", 18),
    ("output_enable", 19),
    ("filter_freq", 20),
    ("filter_gain", 21),
    ("filter_q", 22),
    ("filter_type", 23),
    ("filter_bypass", 24),
    ("siggen", 25),
    ("dac_mute_test", 26),
    ("clip_ch", 27),
    ("level", 28),
    ("spdif_lock", 29),
    ("sample_rate", 30),
    ("usb_streaming", 31),
    ("adat_active", 32),
    ("lg_present", 33),
    ("lg_muted", 34),
    ("upmix", 35),
    ("upmix_center_mode", 36),
    ("upmix_surround_mode", 37),
    ("upmix_strength", 38),
    ("upmix_width", 39),
    ("upmix_presence", 40),
    ("psybass", 41),
    ("psybass_cutoff", 42),
    ("psybass_harmonics", 43),
    ("psybass_drive", 44),
    ("psybass_character", 45),
    ("psybass_original", 46),
    ("output_delay", 47),
    ("preset_reload", 48),
    ("loudness_spl", 49),
    ("loudness_intensity", 50),
    ("input_level_max", 51),
    ("macro", 52),
    ("cpu_load", 53),
    ("display_page", 54),
    ("display_edit", 55),
    ("page_value", 56),
    // Caps v14 and v15: the subharmonic synthesizer.
    ("subharm", 57),
    ("subharm_low", 58),
    ("subharm_high", 59),
    ("subharm_boost", 60),
    ("subharm_top", 61),
    ("subharm_select", 62),
    ("subharm_depth", 63),
    ("subharm_hold", 64),
    ("subharm_ceiling", 65),
    ("subharm_link", 66),
    ("subharm_solo", 67),
    // Caps v18: auxiliary outputs, targeting a binding slot.
    ("aux", 68),
    ("aux_level", 69),
    // Caps v19: the tube preamp.
    ("tube", 70),
    ("tube_drive", 71),
    ("tube_type", 72),
    ("tube_mix", 73),
    // Caps v20: the output limiter, targeting an output.
    ("limiter", 74),
    ("limiter_threshold", 75),
    ("limiter_release", 76),
    ("limiter_link", 77),
    ("limiter_gr", 78),
];

/// Actions (control_surfaces.h:227-241).
pub const CS_ACTIONS: &[(&str, u8)] = &[
    ("adjust", 0),
    ("step", 1),
    ("inc", 2),
    ("dec", 3),
    ("toggle", 4),
    ("set", 5),
    ("follow", 6),
    ("trigger", 7),
    ("ind_equals", 8),
    ("momentary", 9),
    ("ind_above", 10),
    ("ind_level", 11),
];

/// Button events (control_surfaces.h:247-252).
pub const CS_EVENTS: &[(&str, u8)] = &[("press", 0), ("long", 1), ("double", 2)];

/// Binding flags (control_surfaces.h:255-266). The byte is full.
pub const CS_BINDING_FLAGS: &[(&str, u8)] = &[
    ("invert", 0x01),
    ("reverse", 0x02),
    ("wrap", 0x04),
    ("accel", 0x08),
    ("repeat", 0x10),
    ("group", 0x20),
    ("link_abs", 0x40),
    ("group_all", 0x80),
];

/// The subset an IR command may carry (control_surfaces.h:399-406).
pub const CS_IR_FLAGS: &[(&str, u8)] = &[("wrap", 0x04), ("repeat", 0x10), ("group", 0x20)];

/// The subset a macro step may carry (control_surfaces.h:463).
pub const CS_STEP_FLAGS: &[(&str, u8)] = &[("wrap", 0x04), ("group", 0x20)];

/// Target kinds (control_surfaces.h:272-278).
pub const CS_TARGET_KINDS: &[(&str, u8)] = &[
    ("none", 0),
    ("input_ch", 1),
    ("output_ch", 2),
    ("dsp_ch", 3),
    ("dsp_band", 4),
    // Caps v18: a binding slot holding an auxiliary output; index must be 0.
    ("aux", 5),
];

/// `CsBinding.extras` on an auxiliary output (control_surfaces.h:425-437).
pub const CS_AUX_EXTRAS: &[(&str, u8)] = &[
    ("boot_on", aux_extras::BOOT_ON),
    ("boot_saved", aux_extras::BOOT_SAVED),
    ("linear", aux_extras::LINEAR),
];

/// `CsNounDesc.unit` (control_surfaces.h:259-269). How a noun's value, range
/// and step are encoded depends on it, and the caps say which one each noun
/// uses.
pub mod cs_unit {
    use crate::generated::cs;
    pub const NONE: u8 = cs::CS_UNIT_NONE as u8;
    /// Signed 8.8 dB, linear steps.
    pub const DB: u8 = cs::CS_UNIT_DB as u8;
    /// Plain integer Hz; the step is 8.8 octaves.
    pub const HZ: u8 = cs::CS_UNIT_HZ as u8;
    /// 8.8 Q; the step is 8.8 octaves.
    pub const Q: u8 = cs::CS_UNIT_Q as u8;
    /// 8.8 percent, linear steps.
    pub const PERCENT: u8 = cs::CS_UNIT_PERCENT as u8;
    /// 8.8 milliseconds, linear steps; tops out at 127 ms.
    pub const MS: u8 = cs::CS_UNIT_MS as u8;
    /// Caps v20: plain integer milliseconds, stepped in 8.8 octaves like Hz,
    /// for spans past what 8.8 can hold (the limiter release, 10 to 1000 ms).
    pub const MS_LOG: u8 = cs::CS_UNIT_MS_LOG as u8;
}

/// True when a unit carries its value, range and `min_q`/`max_q` as signed
/// 8.8 fixed point (control_surfaces.h:262-267). Hz and MS_LOG are plain
/// integers.
pub fn cs_unit_is_fixed_point(unit: u8) -> bool {
    matches!(
        unit,
        cs_unit::DB | cs_unit::Q | cs_unit::PERCENT | cs_unit::MS
    )
}

/// True when a unit steps multiplicatively, so its step operand is 8.8
/// octaves: Hz, Q and MS_LOG (the firmware's `cs_unit_is_log`,
/// control_surfaces.h:925-927).
pub fn cs_unit_is_log(unit: u8) -> bool {
    matches!(unit, cs_unit::HZ | cs_unit::Q | cs_unit::MS_LOG)
}

/// A noun's wire value in its natural unit.
pub fn cs_decode_value(q: i16, unit: u8) -> f64 {
    if cs_unit_is_fixed_point(unit) {
        q as f64 / 256.0
    } else {
        q as f64
    }
}

/// A value in its natural unit as the wire carries it, saturating at the
/// ends of the 16-bit field.
pub fn cs_encode_value(v: f64, unit: u8) -> i16 {
    let raw = if cs_unit_is_fixed_point(unit) {
        v * 256.0
    } else {
        v
    };
    raw.round().clamp(i16::MIN as f64, i16::MAX as f64) as i16
}

/// IR protocols (control_surfaces.h:337-342).
pub const IR_PROTOCOLS: &[(&str, u8)] =
    &[("none", 0), ("nec", 1), ("rc5", 2), ("rc6", 3), ("hash", 4)];

/// Display home-content modes (control_surfaces.h:299-301).
pub const DISPLAY_MODES: &[(&str, u8)] = &[("fixed", 0), ("cycle_selected", 1), ("cycle_all", 2)];

/// The two boolean bits of `CsDisplayCfg.flags`; the alignments have their own
/// fields because they are two-bit values, not flags.
pub const DISPLAY_CFG_FLAGS: &[(&str, u8)] = &[("overlay_any", 0x01), ("edit_gated", 0x02)];

/// Line alignment (control_surfaces.h:314-316). Encoding 3 is reserved.
pub const DISPLAY_ALIGN: &[(&str, u8)] = &[("left", 0), ("centre", 1), ("right", 2)];

/// Page flags (control_surfaces.h:319-324).
pub const DISPLAY_PAGE_FLAGS: &[(&str, u8)] = &[
    ("active", 0x01),
    ("group", 0x02),
    ("large", 0x04),
    ("bar", 0x08),
];

/// Display models (control_surfaces.h:287-296).
pub const DISPLAY_MODELS: &[(&str, u8)] = &[
    ("none", 0),
    ("lcd1602", 1),
    ("lcd2004", 2),
    ("oled_16x2", 3),
    ("oled_20x2", 4),
    ("oled_20x4", 5),
    ("ssd1306_128x64", 6),
    ("ssd1306_128x32", 7),
    ("sh1106_128x64", 8),
];

/// Startup modes (survey 6.1).
pub const STARTUP_MODES: &[(&str, u8)] = &[("specified", 0), ("last_active", 1)];

// ------------------------------------------------------------- field parsing

fn bad(field: &'static str, value: &str, expected: impl Into<String>) -> PacketError {
    PacketError::BadValue {
        field,
        value: value.to_string(),
        expected: expected.into(),
    }
}

/// Parse a whole number, accepting `0x` hex as well as decimal and a sign.
fn parse_int(field: &'static str, v: &str) -> Result<i64, PacketError> {
    let t = v.trim();
    let parsed = match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Some(hex) => i64::from_str_radix(hex, 16).ok(),
        None => t.parse::<i64>().ok(),
    };
    parsed.ok_or_else(|| bad(field, v, "a whole number"))
}

fn parse_bool_field(field: &'static str, v: &str) -> Result<bool, PacketError> {
    match v.trim().to_ascii_lowercase().as_str() {
        // An empty value is the bare-word form: `invert` alone means on.
        "" | "on" | "true" | "yes" | "1" | "enabled" => Ok(true),
        "off" | "false" | "no" | "0" | "disabled" => Ok(false),
        _ => Err(bad(field, v, "on or off")),
    }
}

fn parse_choice(
    field: &'static str,
    table: &'static [(&'static str, u8)],
    v: &str,
) -> Result<u8, PacketError> {
    let lower = v.trim().to_ascii_lowercase();
    if let Some((_, raw)) = table.iter().find(|(n, _)| *n == lower) {
        return Ok(*raw);
    }
    // A number stays available: the device's table may be longer than ours.
    if let Ok(n) = parse_int(field, v)
        && (0..=255).contains(&n)
    {
        return Ok(n as u8);
    }
    let names: Vec<&str> = table.iter().map(|(n, _)| *n).collect();
    Err(bad(field, v, names.join(", ")))
}

fn parse_flag_set(
    field: &'static str,
    table: &'static [(&'static str, u8)],
    v: &str,
) -> Result<u8, PacketError> {
    let t = v.trim();
    if t.is_empty() {
        return Ok(0);
    }
    // A raw mask stays available, because a firmware may define a bit this
    // build has no name for.
    if t.starts_with("0x") || t.starts_with("0X") || t.chars().all(|c| c.is_ascii_digit()) {
        let n = parse_int(field, t)?;
        if (0..=255).contains(&n) {
            return Ok(n as u8);
        }
    }
    let mut mask = 0u8;
    for part in t.split(['+', ',', '|']) {
        let name = part.trim().to_ascii_lowercase();
        if name.is_empty() {
            continue;
        }
        match table.iter().find(|(n, _)| *n == name) {
            Some((_, bit)) => mask |= bit,
            None => {
                let names: Vec<&str> = table.iter().map(|(n, _)| *n).collect();
                return Err(bad(field, v, names.join(", ")));
            }
        }
    }
    Ok(mask)
}

fn format_flag_set(table: &'static [(&'static str, u8)], mask: u8) -> String {
    let mut named = 0u8;
    let mut out: Vec<String> = Vec::new();
    for (name, bit) in table {
        if mask & bit != 0 {
            out.push((*name).to_string());
            named |= bit;
        }
    }
    // Bits with no name here still have to survive a round trip.
    if mask & !named != 0 {
        out.push(format!("0x{:02X}", mask & !named));
    }
    out.join(",")
}

fn format_choice(table: &'static [(&'static str, u8)], raw: u8) -> String {
    table
        .iter()
        .find(|(_, v)| *v == raw)
        .map(|(n, _)| (*n).to_string())
        .unwrap_or_else(|| raw.to_string())
}

fn parse_u8_field(field: &'static str, v: &str) -> Result<u8, PacketError> {
    let n = parse_int(field, v)?;
    u8::try_from(n).map_err(|_| bad(field, v, "0 to 255"))
}

fn parse_i16_field(field: &'static str, v: &str) -> Result<i16, PacketError> {
    let n = parse_int(field, v)?;
    i16::try_from(n).map_err(|_| bad(field, v, "-32768 to 32767"))
}

fn parse_u16_field(field: &'static str, v: &str) -> Result<u16, PacketError> {
    let n = parse_int(field, v)?;
    u16::try_from(n).map_err(|_| bad(field, v, "0 to 65535"))
}

fn parse_u32_field(field: &'static str, v: &str) -> Result<u32, PacketError> {
    let n = parse_int(field, v)?;
    u32::try_from(n).map_err(|_| bad(field, v, "0 to 4294967295"))
}

fn parse_mask32_field(field: &'static str, v: &str) -> Result<u32, PacketError> {
    let n = parse_int(field, v)?;
    u32::try_from(n).map_err(|_| bad(field, v, "a 32-bit mask"))
}

fn parse_mask16_field(field: &'static str, v: &str) -> Result<u16, PacketError> {
    let n = parse_int(field, v)?;
    u16::try_from(n).map_err(|_| bad(field, v, "a 16-bit mask"))
}

fn parse_f32_field(field: &'static str, v: &str) -> Result<f32, PacketError> {
    v.trim()
        .parse::<f32>()
        .map_err(|_| bad(field, v, "a number"))
}

fn parse_gpio_pair(field: &'static str, v: &str) -> Result<[u8; 2], PacketError> {
    let mut parts = v.split(',');
    let first = parts
        .next()
        .ok_or_else(|| bad(field, v, "one GPIO, or two separated by a comma"))?;
    let a = parse_u8_field(field, first)?;
    let b = match parts.next() {
        Some(second) => parse_u8_field(field, second)?,
        // A configured single-pin component writes 0xFF explicitly.
        None => GPIO_UNUSED,
    };
    if parts.next().is_some() {
        return Err(bad(field, v, "at most two GPIOs"));
    }
    Ok([a, b])
}

fn format_gpio_pair(gpio: [u8; 2]) -> String {
    if gpio[1] == GPIO_UNUSED {
        gpio[0].to_string()
    } else {
        format!("{},{}", gpio[0], gpio[1])
    }
}

/// Reject a field this packet does not have, naming the ones it does.
fn unknown_field(what: &'static str, name: &str, fields: &'static [FieldSpec]) -> PacketError {
    PacketError::UnknownField {
        what,
        name: name.to_string(),
        valid: fields.iter().map(|f| f.name).collect::<Vec<_>>().join(", "),
    }
}

/// Render a float the way someone would type it back.
fn format_float(v: f32) -> String {
    let rounded = (v * 100.0).round() / 100.0;
    if rounded.fract() == 0.0 {
        format!("{rounded:.0}")
    } else {
        format!("{rounded}")
    }
}

fn on_off(v: bool) -> String {
    (if v { "on" } else { "off" }).to_string()
}

// --------------------------------------------------- per-packet field tables

impl CsBinding {
    pub const FIELDS: &'static [FieldSpec] = &[
        f(
            "type",
            FieldKind::Enum(CS_TYPES),
            "which physical component this is",
        ),
        f(
            "noun",
            FieldKind::Enum(CS_NOUNS),
            "the parameter it drives or indicates",
        ),
        f("action", FieldKind::Enum(CS_ACTIONS), "what it does to it"),
        f("flags", FieldKind::Flags(CS_BINDING_FLAGS), "binding flags"),
        f(
            "gpio",
            FieldKind::Gpio2,
            "one GPIO, or two for an encoder or a display",
        ),
        f("event", FieldKind::Enum(CS_EVENTS), "button gesture"),
        f(
            "target",
            FieldKind::U8,
            "channel index, or group index with the group flag",
        ),
        f(
            "index",
            FieldKind::U8,
            "filter band, or display model on a display slot",
        ),
        f(
            "base_bright",
            FieldKind::U8,
            "PWM LED brightness ceiling, percent 1-100; 0 = full",
        ),
        f(
            "value",
            FieldKind::I16,
            "SET target or indicator comparand, in the noun's wire units",
        ),
        f(
            "step",
            FieldKind::I16,
            "step size in wire units; dB, Q and percent are 8.8 fixed point",
        ),
        f("range_min", FieldKind::I16, "pot span low end"),
        f("range_max", FieldKind::I16, "pot span high end"),
        f(
            "on_delay",
            FieldKind::U16,
            "indicator on delay, 0.1 s units",
        ),
        f(
            "off_delay",
            FieldKind::U16,
            "indicator off delay, 0.1 s units",
        ),
        f(
            "extras",
            FieldKind::Flags(CS_AUX_EXTRAS),
            "auxiliary output boot and response flags; 0 on other types",
        ),
    ];

    pub fn fields() -> &'static [FieldSpec] {
        Self::FIELDS
    }

    pub fn from_fields(given: &[(&str, &str)]) -> Result<Self, PacketError> {
        let mut b = Self::default();
        for (key, value) in given {
            match *key {
                "type" => b.component = parse_choice("type", CS_TYPES, value)?,
                "noun" => b.noun = parse_choice("noun", CS_NOUNS, value)?,
                "action" => b.action = parse_choice("action", CS_ACTIONS, value)?,
                "flags" => b.flags = parse_flag_set("flags", CS_BINDING_FLAGS, value)?,
                "gpio" => b.gpio = parse_gpio_pair("gpio", value)?,
                "event" => b.event = parse_choice("event", CS_EVENTS, value)?,
                "target" => b.target = parse_u8_field("target", value)?,
                "index" => b.index = parse_u8_field("index", value)?,
                "base_bright" => b.base_bright = parse_u8_field("base_bright", value)?,
                "value" => b.value = parse_i16_field("value", value)?,
                "step" => b.step = parse_i16_field("step", value)?,
                "range_min" => b.range_min = parse_i16_field("range_min", value)?,
                "range_max" => b.range_max = parse_i16_field("range_max", value)?,
                "on_delay" => b.on_delay = parse_u16_field("on_delay", value)?,
                "off_delay" => b.off_delay = parse_u16_field("off_delay", value)?,
                "extras" => b.extras = parse_flag_set("extras", CS_AUX_EXTRAS, value)?,
                other => return Err(unknown_field("cs.binding", other, Self::FIELDS)),
            }
        }
        Ok(b)
    }

    pub fn to_fields(&self) -> Vec<(&'static str, String)> {
        let mut out = vec![("type", format_choice(CS_TYPES, self.component))];
        if self.noun != 0 || self.action != 0 {
            out.push(("noun", format_choice(CS_NOUNS, self.noun)));
            out.push(("action", format_choice(CS_ACTIONS, self.action)));
        }
        if self.gpio != [0, 0] {
            out.push(("gpio", format_gpio_pair(self.gpio)));
        }
        if self.flags != 0 {
            out.push(("flags", format_flag_set(CS_BINDING_FLAGS, self.flags)));
        }
        for (name, v) in [
            ("event", self.event as i64),
            ("target", self.target as i64),
            ("index", self.index as i64),
            ("base_bright", self.base_bright as i64),
            ("value", self.value as i64),
            ("step", self.step as i64),
            ("range_min", self.range_min as i64),
            ("range_max", self.range_max as i64),
            ("on_delay", self.on_delay as i64),
            ("off_delay", self.off_delay as i64),
        ] {
            if v != 0 {
                out.push((name, v.to_string()));
            }
        }
        if self.extras != 0 {
            out.push(("extras", format_flag_set(CS_AUX_EXTRAS, self.extras)));
        }
        out
    }
}

impl IrCommand {
    pub const FIELDS: &'static [FieldSpec] = &[
        f("noun", FieldKind::Enum(CS_NOUNS), "the parameter it drives"),
        f("action", FieldKind::Enum(CS_ACTIONS), "what it does to it"),
        f(
            "flags",
            FieldKind::Flags(CS_IR_FLAGS),
            "wrap, repeat, group",
        ),
        f("target", FieldKind::U8, "channel or group index"),
        f("index", FieldKind::U8, "filter band"),
        f(
            "protocol",
            FieldKind::Enum(IR_PROTOCOLS),
            "remote protocol; none clears the sub-slot",
        ),
        f("value", FieldKind::I16, "SET target, in wire units"),
        f("step", FieldKind::I16, "step size, in wire units"),
        f("code", FieldKind::U32, "the learned code"),
    ];

    pub fn fields() -> &'static [FieldSpec] {
        Self::FIELDS
    }

    pub fn from_fields(given: &[(&str, &str)]) -> Result<Self, PacketError> {
        let mut c = Self::default();
        for (key, value) in given {
            match *key {
                "noun" => c.noun = parse_choice("noun", CS_NOUNS, value)?,
                "action" => c.action = parse_choice("action", CS_ACTIONS, value)?,
                "flags" => c.flags = parse_flag_set("flags", CS_IR_FLAGS, value)?,
                "target" => c.target = parse_u8_field("target", value)?,
                "index" => c.index = parse_u8_field("index", value)?,
                "protocol" => c.protocol = parse_choice("protocol", IR_PROTOCOLS, value)?,
                "value" => c.value = parse_i16_field("value", value)?,
                "step" => c.step = parse_i16_field("step", value)?,
                "code" => c.code = parse_u32_field("code", value)?,
                other => return Err(unknown_field("cs.ir", other, Self::FIELDS)),
            }
        }
        Ok(c)
    }

    pub fn to_fields(&self) -> Vec<(&'static str, String)> {
        let mut out = vec![
            ("noun", format_choice(CS_NOUNS, self.noun)),
            ("action", format_choice(CS_ACTIONS, self.action)),
            ("protocol", format_choice(IR_PROTOCOLS, self.protocol)),
        ];
        if self.flags != 0 {
            out.push(("flags", format_flag_set(CS_IR_FLAGS, self.flags)));
        }
        for (name, v) in [
            ("target", self.target as i64),
            ("index", self.index as i64),
            ("value", self.value as i64),
            ("step", self.step as i64),
        ] {
            if v != 0 {
                out.push((name, v.to_string()));
            }
        }
        if self.code != 0 {
            out.push(("code", format!("0x{:08X}", self.code)));
        }
        out
    }
}

impl CsGroup {
    pub const FIELDS: &'static [FieldSpec] = &[
        f(
            "kind",
            FieldKind::Enum(CS_TARGET_KINDS),
            "which channel space the members are in",
        ),
        f(
            "members",
            FieldKind::Mask32,
            "member mask, bit N = channel N of that space",
        ),
        f("name", FieldKind::Text(32), "what to call the group"),
    ];

    pub fn fields() -> &'static [FieldSpec] {
        Self::FIELDS
    }

    pub fn from_fields(given: &[(&str, &str)]) -> Result<Self, PacketError> {
        let mut g = Self::default();
        for (key, value) in given {
            match *key {
                "kind" => g.target_kind = parse_choice("kind", CS_TARGET_KINDS, value)?,
                "members" => g.member_mask = parse_mask32_field("members", value)?,
                "name" => g.name = (*value).to_string(),
                other => return Err(unknown_field("cs.group", other, Self::FIELDS)),
            }
        }
        Ok(g)
    }

    pub fn to_fields(&self) -> Vec<(&'static str, String)> {
        let mut out = vec![("kind", format_choice(CS_TARGET_KINDS, self.target_kind))];
        if self.member_mask != 0 {
            out.push(("members", format!("0x{:X}", self.member_mask)));
        }
        if !self.name.is_empty() {
            out.push(("name", self.name.clone()));
        }
        out
    }
}

impl CsMacroStep {
    pub const FIELDS: &'static [FieldSpec] = &[
        f("noun", FieldKind::Enum(CS_NOUNS), "the parameter to change"),
        f("action", FieldKind::Enum(CS_ACTIONS), "what to do to it"),
        f("flags", FieldKind::Flags(CS_STEP_FLAGS), "wrap, group"),
        f("target", FieldKind::U8, "channel or group index"),
        f("index", FieldKind::U8, "filter band"),
        f("value", FieldKind::I16, "SET target, in wire units"),
        f("step", FieldKind::I16, "step size, in wire units"),
        f(
            "pre_delay",
            FieldKind::U16,
            "wait before this step runs, 10 ms units",
        ),
    ];

    pub fn fields() -> &'static [FieldSpec] {
        Self::FIELDS
    }

    pub fn from_fields(given: &[(&str, &str)]) -> Result<Self, PacketError> {
        let mut s = Self::default();
        for (key, value) in given {
            match *key {
                "noun" => s.noun = parse_choice("noun", CS_NOUNS, value)?,
                "action" => s.action = parse_choice("action", CS_ACTIONS, value)?,
                "flags" => s.flags = parse_flag_set("flags", CS_STEP_FLAGS, value)?,
                "target" => s.target = parse_u8_field("target", value)?,
                "index" => s.index = parse_u8_field("index", value)?,
                "value" => s.value = parse_i16_field("value", value)?,
                "step" => s.step = parse_i16_field("step", value)?,
                "pre_delay" => s.pre_delay = parse_u16_field("pre_delay", value)?,
                other => return Err(unknown_field("cs.macro.step", other, Self::FIELDS)),
            }
        }
        Ok(s)
    }

    pub fn to_fields(&self) -> Vec<(&'static str, String)> {
        let mut out = vec![
            ("noun", format_choice(CS_NOUNS, self.noun)),
            ("action", format_choice(CS_ACTIONS, self.action)),
        ];
        if self.flags != 0 {
            out.push(("flags", format_flag_set(CS_STEP_FLAGS, self.flags)));
        }
        for (name, v) in [
            ("target", self.target as i64),
            ("index", self.index as i64),
            ("value", self.value as i64),
            ("step", self.step as i64),
            ("pre_delay", self.pre_delay as i64),
        ] {
            if v != 0 {
                out.push((name, v.to_string()));
            }
        }
        out
    }
}

impl CsDisplayCfg {
    pub const FIELDS: &'static [FieldSpec] = &[
        f(
            "mode",
            FieldKind::Enum(DISPLAY_MODES),
            "what the home screen shows",
        ),
        f("home_page", FieldKind::U8, "page shown in fixed mode"),
        f("dwell", FieldKind::U16, "cycle period, 0.1 s units"),
        f(
            "overlay_hold",
            FieldKind::U16,
            "pop-up hold, 0.1 s units; 0 = no pop-up",
        ),
        f("brightness", FieldKind::U8, "0-255; applies on next attach"),
        f(
            "flags",
            FieldKind::Flags(DISPLAY_CFG_FLAGS),
            "overlay_any, edit_gated",
        ),
        f(
            "label_align",
            FieldKind::Enum(DISPLAY_ALIGN),
            "label line alignment",
        ),
        f(
            "value_align",
            FieldKind::Enum(DISPLAY_ALIGN),
            "value line alignment",
        ),
        f(
            "edit_timeout",
            FieldKind::U16,
            "edit auto-disarm, 0.1 s units; 0 = manual",
        ),
    ];

    pub fn fields() -> &'static [FieldSpec] {
        Self::FIELDS
    }

    pub fn from_fields(given: &[(&str, &str)]) -> Result<Self, PacketError> {
        let mut c = Self::default();
        for (key, value) in given {
            match *key {
                "mode" => c.mode = parse_choice("mode", DISPLAY_MODES, value)?,
                "home_page" => c.home_page = parse_u8_field("home_page", value)?,
                "dwell" => c.dwell = parse_u16_field("dwell", value)?,
                "overlay_hold" => c.overlay_hold = parse_u16_field("overlay_hold", value)?,
                "brightness" => c.brightness = parse_u8_field("brightness", value)?,
                "flags" => {
                    // Writing the whole byte would clear the two alignment
                    // fields, which live in the same byte and have their own
                    // keys, so only the boolean bits are merged here.
                    let bits = parse_flag_set("flags", DISPLAY_CFG_FLAGS, value)?;
                    c.flags = (c.flags & (display_flags::LABEL_ALIGN | display_flags::VALUE_ALIGN))
                        | bits;
                }
                "label_align" => {
                    let a = parse_choice("label_align", DISPLAY_ALIGN, value)?;
                    c.set_label_align(a);
                }
                "value_align" => {
                    let a = parse_choice("value_align", DISPLAY_ALIGN, value)?;
                    c.set_value_align(a);
                }
                "edit_timeout" => c.edit_timeout = parse_u16_field("edit_timeout", value)?,
                other => return Err(unknown_field("cs.display", other, Self::FIELDS)),
            }
        }
        Ok(c)
    }

    pub fn to_fields(&self) -> Vec<(&'static str, String)> {
        let mut out = vec![("mode", format_choice(DISPLAY_MODES, self.mode))];
        if self.home_page != 0 {
            out.push(("home_page", self.home_page.to_string()));
        }
        out.push(("dwell", self.dwell.to_string()));
        out.push(("overlay_hold", self.overlay_hold.to_string()));
        if self.brightness != 0 {
            out.push(("brightness", self.brightness.to_string()));
        }
        let bits = self.flags & !(display_flags::LABEL_ALIGN | display_flags::VALUE_ALIGN);
        if bits != 0 {
            out.push(("flags", format_flag_set(DISPLAY_CFG_FLAGS, bits)));
        }
        if self.label_align() != 0 {
            out.push((
                "label_align",
                format_choice(DISPLAY_ALIGN, self.label_align()),
            ));
        }
        if self.value_align() != 0 {
            out.push((
                "value_align",
                format_choice(DISPLAY_ALIGN, self.value_align()),
            ));
        }
        out.push(("edit_timeout", self.edit_timeout.to_string()));
        out
    }
}

impl CsDisplayPage {
    pub const FIELDS: &'static [FieldSpec] = &[
        f("noun", FieldKind::Enum(CS_NOUNS), "what the page shows"),
        f("target", FieldKind::U8, "channel or group index"),
        f("index", FieldKind::U8, "filter band"),
        f(
            "flags",
            FieldKind::Flags(DISPLAY_PAGE_FLAGS),
            "active, group, large, bar",
        ),
    ];

    pub fn fields() -> &'static [FieldSpec] {
        Self::FIELDS
    }

    pub fn from_fields(given: &[(&str, &str)]) -> Result<Self, PacketError> {
        let mut p = Self::default();
        for (key, value) in given {
            match *key {
                "noun" => p.noun = parse_choice("noun", CS_NOUNS, value)?,
                "target" => p.target = parse_u8_field("target", value)?,
                "index" => p.index = parse_u8_field("index", value)?,
                "flags" => p.flags = parse_flag_set("flags", DISPLAY_PAGE_FLAGS, value)?,
                other => return Err(unknown_field("cs.display.page", other, Self::FIELDS)),
            }
        }
        Ok(p)
    }

    pub fn to_fields(&self) -> Vec<(&'static str, String)> {
        let mut out = vec![("noun", format_choice(CS_NOUNS, self.noun))];
        if self.target != 0 {
            out.push(("target", self.target.to_string()));
        }
        if self.index != 0 {
            out.push(("index", self.index.to_string()));
        }
        if self.flags != 0 {
            out.push(("flags", format_flag_set(DISPLAY_PAGE_FLAGS, self.flags)));
        }
        out
    }
}

impl MatrixRoutePacket {
    pub const FIELDS: &'static [FieldSpec] = &[
        f("input", FieldKind::U8, "source channel"),
        f("output", FieldKind::U8, "destination channel"),
        f(
            "enabled",
            FieldKind::Bool,
            "whether the route carries audio",
        ),
        f("gain", FieldKind::F32, "crosspoint gain in dB"),
        f("invert", FieldKind::Bool, "flip the polarity"),
    ];

    pub fn fields() -> &'static [FieldSpec] {
        Self::FIELDS
    }

    pub fn from_fields(given: &[(&str, &str)]) -> Result<Self, PacketError> {
        let mut r = Self::default();
        for (key, value) in given {
            match *key {
                "input" => r.input = parse_u8_field("input", value)?,
                "output" => r.output = parse_u8_field("output", value)?,
                "enabled" => r.enabled = parse_bool_field("enabled", value)?,
                "gain" => r.gain_db = parse_f32_field("gain", value)?,
                "invert" => r.phase_invert = parse_bool_field("invert", value)?,
                other => return Err(unknown_field("mix", other, Self::FIELDS)),
            }
        }
        Ok(r)
    }

    /// The two index fields are left out: the command line carries them as the
    /// crosspoint's own indices, and printing them twice would not re-parse.
    pub fn to_fields(&self) -> Vec<(&'static str, String)> {
        let mut out = vec![("enabled", on_off(self.enabled))];
        if self.gain_db != 0.0 {
            out.push(("gain", format_float(self.gain_db)));
        }
        if self.phase_invert {
            out.push(("invert", on_off(true)));
        }
        out
    }
}

impl DacHwMuteConfig {
    pub const FIELDS: &'static [FieldSpec] = &[
        f("enabled", FieldKind::Bool, "drive an external DAC mute pin"),
        f(
            "active_low",
            FieldKind::Bool,
            "assert low to mute; on by default",
        ),
        f("pin", FieldKind::U8, "GPIO; 255 = none"),
        f(
            "hold_ms",
            FieldKind::U16,
            "mute hold before the clocks stop, 1-500",
        ),
        f(
            "release_ms",
            FieldKind::U16,
            "hold after the clocks restart, 0-500",
        ),
    ];

    pub fn fields() -> &'static [FieldSpec] {
        Self::FIELDS
    }

    pub fn from_fields(given: &[(&str, &str)]) -> Result<Self, PacketError> {
        let mut c = Self::default();
        for (key, value) in given {
            match *key {
                "enabled" => c.enabled = parse_bool_field("enabled", value)?,
                "active_low" => c.active_low = parse_bool_field("active_low", value)?,
                "pin" => c.pin = parse_u8_field("pin", value)?,
                "hold_ms" => c.hold_ms = parse_u16_field("hold_ms", value)?,
                "release_ms" => c.release_ms = parse_u16_field("release_ms", value)?,
                other => return Err(unknown_field("dev.dacmute", other, Self::FIELDS)),
            }
        }
        Ok(c)
    }

    pub fn to_fields(&self) -> Vec<(&'static str, String)> {
        vec![
            ("enabled", on_off(self.enabled)),
            ("active_low", on_off(self.active_low)),
            ("pin", self.pin.to_string()),
            ("hold_ms", self.hold_ms.to_string()),
            ("release_ms", self.release_ms.to_string()),
        ]
    }
}

impl UartCtrlConfig {
    pub const FIELDS: &'static [FieldSpec] = &[
        f(
            "enabled",
            FieldKind::Bool,
            "run the serial control interface",
        ),
        f(
            "tx_pin",
            FieldKind::U8,
            "GPIO with a UART TX mux, pin % 4 == 0",
        ),
        f(
            "rx_pin",
            FieldKind::U8,
            "GPIO with a UART RX mux, pin % 4 == 1",
        ),
        f(
            "notify_enable",
            FieldKind::Bool,
            "push change notifications over the link",
        ),
        f("baud", FieldKind::U32, "9600 to 1000000"),
    ];

    pub fn fields() -> &'static [FieldSpec] {
        Self::FIELDS
    }

    pub fn from_fields(given: &[(&str, &str)]) -> Result<Self, PacketError> {
        let mut c = Self::default();
        for (key, value) in given {
            match *key {
                "enabled" => c.enabled = parse_bool_field("enabled", value)?,
                "tx_pin" => c.tx_pin = parse_u8_field("tx_pin", value)?,
                "rx_pin" => c.rx_pin = parse_u8_field("rx_pin", value)?,
                "notify_enable" => c.notify_enable = parse_bool_field("notify_enable", value)?,
                "baud" => c.baud = parse_u32_field("baud", value)?,
                other => return Err(unknown_field("dev.uart", other, Self::FIELDS)),
            }
        }
        Ok(c)
    }

    pub fn to_fields(&self) -> Vec<(&'static str, String)> {
        vec![
            ("enabled", on_off(self.enabled)),
            ("tx_pin", self.tx_pin.to_string()),
            ("rx_pin", self.rx_pin.to_string()),
            ("notify_enable", on_off(self.notify_enable)),
            ("baud", self.baud.to_string()),
        ]
    }
}

impl I2cCtrlConfig {
    pub const FIELDS: &'static [FieldSpec] = &[
        f("enabled", FieldKind::Bool, "run the I2C control interface"),
        f("sda_pin", FieldKind::U8, "even GPIO with an I2C SDA mux"),
        f("scl_pin", FieldKind::U8, "the odd GPIO above it"),
        f(
            "address",
            FieldKind::U8,
            "7-bit target address, 0x08 to 0x77",
        ),
    ];

    pub fn fields() -> &'static [FieldSpec] {
        Self::FIELDS
    }

    pub fn from_fields(given: &[(&str, &str)]) -> Result<Self, PacketError> {
        let mut c = Self::default();
        for (key, value) in given {
            match *key {
                "enabled" => c.enabled = parse_bool_field("enabled", value)?,
                "sda_pin" => c.sda_pin = parse_u8_field("sda_pin", value)?,
                "scl_pin" => c.scl_pin = parse_u8_field("scl_pin", value)?,
                "address" => c.address = parse_u8_field("address", value)?,
                other => return Err(unknown_field("dev.i2c", other, Self::FIELDS)),
            }
        }
        Ok(c)
    }

    pub fn to_fields(&self) -> Vec<(&'static str, String)> {
        vec![
            ("enabled", on_off(self.enabled)),
            ("sda_pin", self.sda_pin.to_string()),
            ("scl_pin", self.scl_pin.to_string()),
            ("address", format!("0x{:02X}", self.address)),
        ]
    }
}

/// The signal catalogue, `siggen.h:34-53`, as tokens a person would type.
pub const SIGGEN_TYPES: &[(&str, u8)] = &[
    ("sine", 0),
    ("square", 1),
    ("white", 2),
    ("pink", 3),
    ("sweep-log", 4),
    ("sweep-lin", 5),
    ("sweep-step", 6),
    ("impulse", 7),
    ("clicks", 8),
    ("polarity", 9),
    ("burst", 10),
    ("tone-pair", 11),
    ("multitone", 12),
    ("isp", 13),
    ("channel-id", 14),
];

/// `SIGGEN_FLAG_*`, siggen.h:56-58.
pub const SIGGEN_FLAGS: &[(&str, u8)] = &[
    ("raw", SiggenConfig::FLAG_RAW),
    ("decorr", SiggenConfig::FLAG_DECORR),
    ("walk", SiggenConfig::FLAG_WALK),
];

impl SiggenConfig {
    pub const FIELDS: &'static [FieldSpec] = &[
        f(
            "type",
            FieldKind::Enum(SIGGEN_TYPES),
            "which signal the generator produces",
        ),
        f(
            "channels",
            FieldKind::Mask32,
            "output-channel select, bit i = output i",
        ),
        f(
            "invert",
            FieldKind::Mask32,
            "the polarity-inverted subset of channels",
        ),
        f("flags", FieldKind::Flags(SIGGEN_FLAGS), "raw, decorr, walk"),
        f("level", FieldKind::F32, "peak level dBFS, -120 to 0"),
        f(
            "duration",
            FieldKind::U32,
            "sweep length or play time, ms; 0 = until stopped",
        ),
        f(
            "repeat",
            FieldKind::U16,
            "sweeps, pattern periods or passes; 0 = forever",
        ),
        f("gap", FieldKind::U16, "gap between repeats, ms"),
        f("p1", FieldKind::F32, "the type's first parameter"),
        f("p2", FieldKind::F32, "the type's second parameter"),
        f("p3", FieldKind::F32, "the type's third parameter"),
        f("p4", FieldKind::F32, "the type's fourth parameter"),
    ];

    pub fn fields() -> &'static [FieldSpec] {
        Self::FIELDS
    }

    pub fn from_fields(given: &[(&str, &str)]) -> Result<Self, PacketError> {
        let mut c = Self::default();
        for (key, value) in given {
            match *key {
                "type" => c.signal_type = parse_choice("type", SIGGEN_TYPES, value)?,
                "channels" => c.channel_mask = parse_mask16_field("channels", value)?,
                "invert" => c.invert_mask = parse_mask16_field("invert", value)?,
                "flags" => c.flags = parse_flag_set("flags", SIGGEN_FLAGS, value)?,
                "level" => c.level_db = parse_f32_field("level", value)?,
                "duration" => c.duration_ms = parse_u32_field("duration", value)?,
                "repeat" => c.repeat_count = parse_u16_field("repeat", value)?,
                "gap" => c.gap_ms = parse_u16_field("gap", value)?,
                "p1" => c.p1 = parse_f32_field("p1", value)?,
                "p2" => c.p2 = parse_f32_field("p2", value)?,
                "p3" => c.p3 = parse_f32_field("p3", value)?,
                "p4" => c.p4 = parse_f32_field("p4", value)?,
                other => return Err(unknown_field("sig.config", other, Self::FIELDS)),
            }
        }
        Ok(c)
    }

    /// The fields worth echoing: the type, the outputs and the level always,
    /// then whatever else is not at its zero.
    pub fn to_fields(&self) -> Vec<(&'static str, String)> {
        let mut out = vec![
            ("type", format_choice(SIGGEN_TYPES, self.signal_type)),
            ("channels", format!("0x{:X}", self.channel_mask)),
            ("level", format_float(self.level_db)),
        ];
        if self.invert_mask != 0 {
            out.push(("invert", format!("0x{:X}", self.invert_mask)));
        }
        if self.flags != 0 {
            out.push(("flags", format_flag_set(SIGGEN_FLAGS, self.flags)));
        }
        if self.duration_ms != 0 {
            out.push(("duration", self.duration_ms.to_string()));
        }
        if self.repeat_count != 0 {
            out.push(("repeat", self.repeat_count.to_string()));
        }
        if self.gap_ms != 0 {
            out.push(("gap", self.gap_ms.to_string()));
        }
        for (name, v) in [
            ("p1", self.p1),
            ("p2", self.p2),
            ("p3", self.p3),
            ("p4", self.p4),
        ] {
            if v != 0.0 {
                out.push((name, format_float(v)));
            }
        }
        out
    }
}

impl PresetStartup {
    pub const FIELDS: &'static [FieldSpec] = &[
        f(
            "mode",
            FieldKind::Enum(STARTUP_MODES),
            "which preset loads at power on",
        ),
        f(
            "slot",
            FieldKind::U8,
            "the slot, when the mode is specified",
        ),
    ];

    pub fn fields() -> &'static [FieldSpec] {
        Self::FIELDS
    }

    pub fn from_fields(given: &[(&str, &str)]) -> Result<Self, PacketError> {
        let mut s = Self::default();
        for (key, value) in given {
            match *key {
                "mode" => s.mode = parse_choice("mode", STARTUP_MODES, value)?,
                "slot" => s.default_slot = parse_u8_field("slot", value)?,
                other => return Err(unknown_field("preset.startup", other, Self::FIELDS)),
            }
        }
        Ok(s)
    }

    /// `last_active` is not here: it is read-only, and the SET is two bytes.
    pub fn to_fields(&self) -> Vec<(&'static str, String)> {
        vec![
            ("mode", format_choice(STARTUP_MODES, self.mode)),
            ("slot", self.default_slot.to_string()),
        ]
    }
}

// ------------------------------------------------------------- packet specs

/// Build a payload from named fields.
pub type PacketEncoder = fn(&[(&str, &str)]) -> Result<Vec<u8>, PacketError>;

/// Read a payload back as named fields.
pub type PacketDescriber = fn(&[u8]) -> Option<Vec<(&'static str, String)>>;

/// How one registry path's packet is typed, read back and completed.
pub struct PacketSpec {
    /// The registry path this packet is the value of.
    pub path: &'static str,
    /// What to call it in an error message.
    pub what: &'static str,
    pub fields: &'static [FieldSpec],
    /// Fields filled from the command's own indices, in order: `mix 0 4` puts
    /// 0 in `input` and 4 in `output`.
    pub index_fields: &'static [&'static str],
    /// Fields that may be given without a key, in this order.
    pub positional: &'static [&'static str],
    /// Build the payload from named fields.
    pub encode: PacketEncoder,
    /// Read a payload back as named fields, index fields excluded.
    pub describe: PacketDescriber,
}

impl PacketSpec {
    pub fn field(&self, name: &str) -> Option<&'static FieldSpec> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// Every field name, for an error that says what would have worked.
    pub fn field_names(&self) -> Vec<&'static str> {
        self.fields.iter().map(|f| f.name).collect()
    }

    /// Render a payload as the `key=value` tail of a command line.
    pub fn format(&self, bytes: &[u8]) -> Option<String> {
        let pairs = (self.describe)(bytes)?;
        Some(
            pairs
                .into_iter()
                .map(|(k, v)| {
                    if v.contains(' ') {
                        format!("{k}=\"{v}\"")
                    } else {
                        format!("{k}={v}")
                    }
                })
                .collect::<Vec<_>>()
                .join(" "),
        )
    }
}

macro_rules! packet_spec {
    ($path:literal, $ty:ty, index: $idx:expr, positional: $pos:expr) => {
        PacketSpec {
            path: $path,
            what: $path,
            fields: <$ty>::FIELDS,
            index_fields: $idx,
            positional: $pos,
            encode: |given| Ok(<$ty>::from_fields(given)?.encode().to_vec()),
            describe: |bytes| <$ty>::decode(bytes).ok().map(|p| p.to_fields()),
        }
    };
}

/// Every packet a person would type. The rest are decoded and shown, but
/// nobody hand-writes a 44-byte upmixer config on a command line.
pub static PACKET_SPECS: &[PacketSpec] = &[
    packet_spec!("cs.binding", CsBinding, index: &[], positional: &[]),
    packet_spec!("cs.ir", IrCommand, index: &[], positional: &[]),
    packet_spec!("cs.group", CsGroup, index: &[], positional: &[]),
    packet_spec!("cs.macro.step", CsMacroStep, index: &[], positional: &[]),
    packet_spec!("cs.display", CsDisplayCfg, index: &[], positional: &[]),
    packet_spec!("cs.display.page", CsDisplayPage, index: &[], positional: &[]),
    // The crosspoint's own indices name the route, and `on` may be given
    // without a key because that is how the Console's grid reads.
    packet_spec!(
        "mix",
        MatrixRoutePacket,
        index: &["input", "output"],
        positional: &["enabled"]
    ),
    packet_spec!(
        "dev.dacmute",
        DacHwMuteConfig,
        index: &[],
        positional: &["enabled"]
    ),
    packet_spec!(
        "dev.uart",
        UartCtrlConfig,
        index: &[],
        positional: &["enabled"]
    ),
    packet_spec!(
        "dev.i2c",
        I2cCtrlConfig,
        index: &[],
        positional: &["enabled"]
    ),
    // The signal type may be given without a key, because the type is the
    // first thing anyone says: `sig.config sine level=-6`.
    packet_spec!(
        "sig.config",
        SiggenConfig,
        index: &[],
        positional: &["type"]
    ),
    // Written as two bytes and read back as three, so it cannot use the macro:
    // what a SET carries has no `last_active` to describe.
    PacketSpec {
        path: "preset.startup",
        what: "preset.startup",
        fields: PresetStartup::FIELDS,
        index_fields: &[],
        positional: &["mode", "slot"],
        encode: |given| Ok(PresetStartup::from_fields(given)?.encode().to_vec()),
        describe: |bytes| PresetStartup::decode_set(bytes).ok().map(|p| p.to_fields()),
    },
];

/// The packet a registry path carries, if it is one a person can type.
pub fn spec_for_path(path: &str) -> Option<&'static PacketSpec> {
    PACKET_SPECS.iter().find(|s| s.path == path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Render bytes the way the Console's own wire tests print them, so an
    /// expectation can be copied across and compared by eye.
    fn hex(b: &[u8]) -> String {
        b.iter()
            .map(|x| format!("{x:02x}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    // ------------------------------------------------------------- bindings

    #[test]
    fn cs_binding_is_twenty_four_bytes() {
        // control_surfaces.h:370-395, `CsBinding`.
        assert_eq!(CsBinding::SIZE, 24);
        assert_eq!(CsBinding::default().encode().len(), 24);
    }

    /// Every field, pinned to the byte the header puts it on. A wrong offset
    /// here silently misconfigures real hardware.
    #[test]
    fn cs_binding_field_offsets_match_the_header() {
        let b = CsBinding {
            component: 0x11,    // control_surfaces.h:371 `type`
            noun: 0x22,         // :372
            action: 0x33,       // :373
            flags: 0x44,        // :374
            gpio: [0x55, 0x66], // :375 `gpio[2]`
            event: 0x77,        // :376
            target: 0x88,       // :377
            index: 0x99,        // :378
            base_bright: 0xAA,  // :382 `base_bright`, caps v12
            value: 0x0BBB,      // :383, bytes 10-11
            step: 0x0CCC,       // :384, bytes 12-13
            range_min: 0x0DDD,  // :385, bytes 14-15
            range_max: 0x0EEE,  // :386, bytes 16-17
            on_delay: 0xF00F,   // :390, bytes 18-19
            off_delay: 0x1234,  // :391, bytes 20-21
            extras: 0x05,       // caps v18, control_surfaces.h:468, byte 22
        };
        let w = b.encode();
        assert_eq!(w[0], 0x11, "type");
        assert_eq!(w[1], 0x22, "noun");
        assert_eq!(w[2], 0x33, "action");
        assert_eq!(w[3], 0x44, "flags");
        assert_eq!(w[4], 0x55, "gpio[0]");
        assert_eq!(w[5], 0x66, "gpio[1]");
        assert_eq!(w[6], 0x77, "event");
        assert_eq!(w[7], 0x88, "target");
        assert_eq!(w[8], 0x99, "index");
        assert_eq!(w[9], 0xAA, "base_bright");
        assert_eq!(&w[10..12], &0x0BBBi16.to_le_bytes(), "value");
        assert_eq!(&w[12..14], &0x0CCCi16.to_le_bytes(), "step");
        assert_eq!(&w[14..16], &0x0DDDi16.to_le_bytes(), "range_min");
        assert_eq!(&w[16..18], &0x0EEEi16.to_le_bytes(), "range_max");
        assert_eq!(&w[18..20], &0xF00Fu16.to_le_bytes(), "on_delay");
        assert_eq!(&w[20..22], &0x1234u16.to_le_bytes(), "off_delay");
        assert_eq!(w[22], 0x05, "extras");
        assert_eq!(w[23], 0, "reserved2 is written as zero");
        assert_eq!(CsBinding::decode(&w).unwrap(), b);
    }

    /// The spec's own worked example: a rotary encoder on GPIO 27/28 driving
    /// master volume, 1 dB per detent (256 in 8.8), accelerated.
    #[test]
    fn cs_binding_matches_the_specs_encoder_example() {
        let b = CsBinding {
            component: 4, // CS_TYPE_ENCODER
            noun: 1,      // CS_NOUN_MASTER_VOLUME
            action: 1,    // CS_ACT_STEP
            flags: 0x08,  // CS_FLAG_ACCEL
            gpio: [27, 28],
            step: 256,
            ..Default::default()
        };
        assert_eq!(
            hex(&b.encode()),
            "04 01 01 08 1b 1c 00 00 00 00 00 00 00 01 00 00 00 00 00 00 00 00 00 00"
        );
    }

    /// A display slot is a container: pins, model in `index`, address in
    /// `value`, everything else zero.
    #[test]
    fn cs_binding_matches_the_specs_display_example() {
        let b = CsBinding {
            component: 8, // CS_TYPE_DISPLAY
            gpio: [2, 3], // SDA even, SCL odd
            index: 6,     // CS_DISP_MODEL_SSD1306_128X64
            ..Default::default()
        };
        assert_eq!(
            hex(&b.encode()),
            "08 00 00 00 02 03 00 00 06 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00"
        );
    }

    /// A cleared slot is the all-zero blob, `gpio[1]` included: only a
    /// configured single-pin component writes 0xFF there.
    #[test]
    fn a_cleared_binding_is_all_zero() {
        let b = CsBinding::default();
        assert!(b.is_empty());
        assert_eq!(b.encode(), [0u8; 24]);
        assert_eq!(b.gpio[1], 0);
    }

    #[test]
    fn a_short_read_is_an_error_rather_than_a_panic() {
        assert!(matches!(
            CsBinding::decode(&[0; 10]),
            Err(PacketError::TooShort {
                want: 24,
                got: 10,
                ..
            })
        ));
        assert!(IrCommand::decode(&[0; 4]).is_err());
        assert!(CsGroup::decode(&[0; 8]).is_err());
        assert!(CsMacro::decode(&[0; 40]).is_err());
        assert!(CsCapsHeader::decode(&[0; 2]).is_err());
        assert!(SystemStatus::decode(&[0; 3]).is_err());
        assert!(BufferStatsPacket::decode(&[0; 43]).is_err());
    }

    // ------------------------------------------------------------ IR command

    #[test]
    fn ir_command_offsets_match_the_header() {
        // control_surfaces.h:403-414.
        assert_eq!(IrCommand::SIZE, 16);
        let c = IrCommand {
            noun: 0x11,       // :404
            action: 0x22,     // :405
            flags: 0x33,      // :406
            target: 0x44,     // :407
            index: 0x55,      // :408
            protocol: 0x01,   // :409
            value: 0x0666,    // :410, bytes 6-7
            step: 0x0777,     // :411, bytes 8-9
            code: 0x20DF40BF, // :413, bytes 12-15
        };
        let w = c.encode();
        assert_eq!(w[0], 0x11);
        assert_eq!(w[1], 0x22);
        assert_eq!(w[2], 0x33);
        assert_eq!(w[3], 0x44);
        assert_eq!(w[4], 0x55);
        assert_eq!(w[5], 0x01);
        assert_eq!(&w[6..8], &0x0666i16.to_le_bytes());
        assert_eq!(&w[8..10], &0x0777i16.to_le_bytes());
        assert_eq!(&w[10..12], &[0, 0], "reserved");
        assert_eq!(&w[12..16], &0x20DF40BFu32.to_le_bytes());
        assert_eq!(IrCommand::decode(&w).unwrap(), c);
    }

    /// The Console's grouped-IR example, byte for byte.
    #[test]
    fn a_grouped_ir_command_matches_the_console() {
        let c = IrCommand {
            noun: 17,    // CS_NOUN_OUTPUT_GAIN
            action: 2,   // CS_ACT_INC
            flags: 0x30, // GROUP | REPEAT
            target: 1,
            protocol: 1, // NEC
            step: 256,
            code: 0xE718FF00,
            ..Default::default()
        };
        assert_eq!(
            hex(&c.encode()),
            "11 02 30 01 00 01 00 00 00 01 00 00 00 ff 18 e7"
        );
    }

    #[test]
    fn an_ir_command_with_no_protocol_is_empty() {
        assert!(IrCommand::default().is_empty());
        assert!(
            !IrCommand {
                protocol: 1,
                ..Default::default()
            }
            .is_empty()
        );
    }

    #[test]
    fn the_learn_reply_carries_state_protocol_and_code() {
        // control_surfaces.h:739-741: {state, protocol, 0, 0, code_le32}.
        let r = IrLearnResult {
            state: 2,
            protocol: 1,
            code: 0x20DF40BF,
        };
        let w = r.encode();
        assert_eq!(IrLearnResult::SIZE, 8);
        assert_eq!(w[0], 2);
        assert_eq!(w[1], 1);
        assert_eq!(&w[2..4], &[0, 0]);
        assert_eq!(&w[4..8], &0x20DF40BFu32.to_le_bytes());
        assert_eq!(IrLearnResult::decode(&w).unwrap(), r);
    }

    // ---------------------------------------------------------------- groups

    #[test]
    fn cs_group_offsets_match_the_header() {
        // control_surfaces.h:440-445.
        assert_eq!(CsGroup::SIZE, 40);
        let g = CsGroup {
            target_kind: 2,            // :441
            member_mask: 0x8001_0001,  // :443, bytes 4-7
            name: "Front Pair".into(), // :444, bytes 8-39
        };
        let w = g.encode();
        assert_eq!(w[0], 2, "target_kind");
        assert_eq!(&w[1..4], &[0, 0, 0], "reserved");
        assert_eq!(&w[4..8], &[0x01, 0x00, 0x01, 0x80], "member_mask, LE");
        assert_eq!(&w[8..18], b"Front Pair");
        assert_eq!(w[18], 0, "the name is NUL terminated");
        assert_eq!(CsGroup::decode(&w).unwrap(), g);
        assert_eq!(g.members(), vec![0, 16, 31], "the mask is 32 bits wide");
    }

    #[test]
    fn an_empty_group_is_all_zero() {
        assert_eq!(CsGroup::default().encode(), [0u8; 40]);
        assert!(!CsGroup::default().is_configured());
        // A kind with no members is not a usable group either.
        assert!(
            !CsGroup {
                target_kind: 3,
                ..Default::default()
            }
            .is_configured()
        );
    }

    #[test]
    fn a_long_group_name_keeps_its_terminator() {
        let g = CsGroup {
            target_kind: 2,
            member_mask: 1,
            name: "x".repeat(60),
        };
        let w = g.encode();
        assert_eq!(w[38], b'x');
        assert_eq!(w[39], 0, "the last name byte stays NUL");
        assert_eq!(CsGroup::decode(&w).unwrap().name.len(), 31);
    }

    // ---------------------------------------------------------------- macros

    #[test]
    fn macro_step_offsets_match_the_header() {
        // control_surfaces.h:460-470.
        assert_eq!(CsMacroStep::SIZE, 12);
        let s = CsMacroStep {
            noun: 18,    // CS_NOUN_OUTPUT_MUTE
            action: 5,   // CS_ACT_SET
            flags: 0x20, // CS_FLAG_GROUP
            target: 2,
            value: 1,
            pre_delay: 150, // 1.5 s in 10 ms units
            ..Default::default()
        };
        assert_eq!(hex(&s.encode()), "12 05 20 02 00 00 01 00 00 00 96 00");
        assert_eq!(CsMacroStep::decode(&s.encode()).unwrap(), s);
    }

    #[test]
    fn an_empty_macro_step_is_all_zero() {
        assert_eq!(CsMacroStep::default().encode(), [0u8; 12]);
        assert!(CsMacroStep::default().is_empty());
    }

    #[test]
    fn the_macro_header_payload_is_name_then_count() {
        // control_surfaces.h:492-496.
        assert_eq!(CsMacroHeaderWire::SIZE, 36);
        let h = CsMacroHeaderWire {
            name: "Movie".into(),
            step_count: 3,
        };
        let w = h.encode();
        assert_eq!(&w[..5], b"Movie");
        assert_eq!(w[5], 0, "NUL inside the name field");
        assert_eq!(w[32], 3, "step_count");
        assert_eq!(&w[33..36], &[0, 0, 0], "reserved");
        assert_eq!(CsMacroHeaderWire::decode(&w).unwrap(), h);
    }

    /// The 132-byte read decodes name, count and all eight steps at their
    /// 12-byte stride. A wrong stride shuffles the sequence.
    #[test]
    fn a_macro_decodes_its_steps_at_the_right_stride() {
        // control_surfaces.h:472-477.
        assert_eq!(CsMacro::SIZE, 132);
        let mut w = vec![0u8; 132];
        w[..5].copy_from_slice(b"Night");
        w[32] = 2;
        let s0 = CsMacroStep {
            noun: 7, // input source
            action: 5,
            value: 1,
            ..Default::default()
        };
        let s1 = CsMacroStep {
            noun: 6, // preset
            action: 5,
            value: 2,
            pre_delay: 50,
            ..Default::default()
        };
        w[36..48].copy_from_slice(&s0.encode());
        w[48..60].copy_from_slice(&s1.encode());

        let m = CsMacro::decode(&w).unwrap();
        assert_eq!(m.name, "Night");
        assert_eq!(m.step_count, 2);
        assert_eq!(m.steps.len(), 8);
        assert_eq!(m.step(0), Some(&s0));
        assert_eq!(m.step(1), Some(&s1));
        assert_eq!(m.step(2), Some(&CsMacroStep::default()));
        assert_eq!(m.active_steps(), &[s0, s1]);
        assert_eq!(m.header().name, "Night");
    }

    /// A corrupt count must not describe steps that are not there: the UI
    /// iterates it over the decoded array.
    #[test]
    fn a_macros_step_count_is_clamped_to_the_steps_that_exist() {
        let mut w = vec![0u8; 132];
        w[32] = 200;
        assert_eq!(CsMacro::decode(&w).unwrap().step_count, 8);
    }

    // --------------------------------------------------------------- display

    #[test]
    fn display_cfg_offsets_match_the_header() {
        // control_surfaces.h:500-509.
        assert_eq!(CsDisplayCfg::SIZE, 12);
        let c = CsDisplayCfg {
            mode: 1,           // :501
            home_page: 2,      // :502
            dwell: 50,         // :503, bytes 2-3
            overlay_hold: 20,  // :504, bytes 4-5
            brightness: 128,   // :505
            flags: 0x03,       // :506, OVERLAY_ANY | EDIT_GATED
            edit_timeout: 100, // :507, bytes 8-9
        };
        assert_eq!(hex(&c.encode()), "01 02 32 00 14 00 80 03 64 00 00 00");
        assert_eq!(CsDisplayCfg::decode(&c.encode()).unwrap(), c);
    }

    /// Both alignments live in the flags byte: label in bits 3:2, value in
    /// bits 5:4. Each must write only its own two bits, or setting one would
    /// silently undo the other and the two booleans beside them.
    #[test]
    fn the_display_alignments_pack_into_the_flags_byte() {
        let mut c = CsDisplayCfg {
            dwell: 30,
            overlay_hold: 20,
            flags: display_flags::EDIT_GATED,
            edit_timeout: 100,
            ..Default::default()
        };
        c.set_label_align(2); // right, 2 << 2 = 0x08
        c.set_value_align(1); // centre, 1 << 4 = 0x10
        assert_eq!(c.flags, 0x1A);
        assert_eq!(hex(&c.encode()), "00 00 1e 00 14 00 00 1a 64 00 00 00");
        assert_eq!(c.label_align(), 2);
        assert_eq!(c.value_align(), 1);
        assert_eq!(
            c.flags & display_flags::EDIT_GATED,
            display_flags::EDIT_GATED,
            "an alignment must not clear the booleans"
        );
    }

    /// The GET puts a four-byte limits header before the config. Reading from
    /// offset 0 would take `max_pages` as the mode.
    #[test]
    fn the_display_cfg_reply_carries_the_limits_first() {
        assert_eq!(CsDisplayCfgReply::SIZE, 16);
        let r = CsDisplayCfgReply {
            max_pages: 16,
            model_count: 9,
            cfg: CsDisplayCfg {
                mode: 0,
                home_page: 1,
                dwell: 30,
                overlay_hold: 20,
                edit_timeout: 100,
                ..Default::default()
            },
        };
        let w = r.encode();
        assert_eq!(w[0], 16, "max_pages");
        assert_eq!(w[1], 9, "model_count");
        assert_eq!(&w[2..4], &[0, 0], "reserved");
        assert_eq!(CsDisplayCfgReply::decode(&w).unwrap(), r);
        assert_eq!(r.cfg.mode, 0, "the config starts at byte 4");
    }

    #[test]
    fn display_page_offsets_match_the_header() {
        // control_surfaces.h:513-518.
        assert_eq!(CsDisplayPage::SIZE, 4);
        let p = CsDisplayPage {
            noun: 17,
            target: 2,
            index: 0,
            flags: page_flags::ACTIVE | page_flags::LARGE,
        };
        assert_eq!(hex(&p.encode()), "11 02 00 05");
        assert_eq!(CsDisplayPage::decode(&p.encode()).unwrap(), p);
        assert!(p.is_active());
        assert_eq!(CsDisplayPage::default().encode(), [0u8; 4]);
        assert!(!CsDisplayPage::default().is_active());
    }

    /// Caps v13's level bar is one more bit in the same byte.
    #[test]
    fn the_page_bar_flag_sits_clear_of_the_others() {
        let p = CsDisplayPage {
            noun: 17,
            target: 2,
            index: 0,
            flags: page_flags::ACTIVE | page_flags::BAR,
        };
        assert_eq!(hex(&p.encode()), "11 02 00 09");
        assert_eq!(page_flags::BAR, 0x08);
        assert_eq!(
            page_flags::ACTIVE | page_flags::GROUP | page_flags::LARGE | page_flags::BAR,
            0x0F
        );
    }

    #[test]
    fn display_status_offsets_match_the_header() {
        // control_surfaces.h:531-538.
        assert_eq!(CsDisplayStatus::SIZE, 8);
        let s = CsDisplayStatus {
            init_state: 2,
            current_page: 3,
            flags: CsDisplayStatus::FLAG_OVERLAY | CsDisplayStatus::FLAG_EDIT,
            model: 1,
            nak_count: 300,
        };
        let w = s.encode();
        assert_eq!(w[0], 2);
        assert_eq!(w[1], 3);
        assert_eq!(w[2], 0x03);
        assert_eq!(w[3], 1);
        assert_eq!(&w[4..6], &300u16.to_le_bytes(), "nak_count, LE");
        assert!(s.is_live());
        assert_eq!(CsDisplayStatus::decode(&w).unwrap(), s);
    }

    // --------------------------------------------------------------- status

    #[test]
    fn ext_status_offsets_match_the_header() {
        // control_surfaces.h:542-551.
        assert_eq!(CsExtStatusPacket::SIZE, 24);
        let mut w = vec![0u8; 24];
        w[0] = 8;
        w[1] = 8;
        w[2] = 8;
        w[3] = 3; // macro 3 running
        w[4] = 1; // on its second step
        w[8 + 2] = 0x1F; // group_status[2] = INVALID_GROUP
        w[16 + 5] = 0x21; // macro_status[5] = INVALID_STEP

        let s = CsExtStatusPacket::decode(&w).unwrap();
        assert_eq!(s.max_groups, 8);
        assert_eq!(s.max_macro_steps, 8);
        assert_eq!(s.macro_running, 3);
        assert_eq!(s.macro_step, 1);
        assert!(s.is_running());
        assert_eq!(s.group_status[2], 0x1F);
        assert_eq!(s.macro_status[5], 0x21);
        assert_eq!(s.encode().to_vec(), w);
    }

    #[test]
    fn an_idle_sequencer_reports_the_sentinel() {
        let mut w = vec![0u8; 24];
        w[3] = CsExtStatusPacket::MACRO_NONE;
        let s = CsExtStatusPacket::decode(&w).unwrap();
        assert_eq!(s.macro_running, 0xFF);
        assert!(!s.is_running());
    }

    /// The v1.1.6 status packet, byte by byte.
    fn status_bytes() -> Vec<u8> {
        let mut d = vec![0x02, 3, 16, 1, 0x05, 0x00];
        d.extend([0u8; 16]); // slot_status[16], bytes 6-21
        d[6 + 3] = 0x1A; // slot 3: pin and gesture already in use
        d.extend(0x8003u16.to_le_bytes()); // ir_active_mask, bytes 22-23
        d.push(0); // ir_learn_state, byte 24
        let mut ir = [0u8; 16]; // ir_cmd_status[16], bytes 25-40
        ir[15] = 0x1E;
        d.extend(ir);
        d
    }

    #[test]
    fn cs_status_offsets_match_the_header() {
        // control_surfaces.h:594-605.
        let d = status_bytes();
        assert_eq!(d.len(), CsStatusPacket::SIZE);
        assert_eq!(CsStatusPacket::SIZE, 41);
        assert_eq!(CsStatusPacket::wire_len(16, 16), 41);

        let s = CsStatusPacket::decode(&d).unwrap();
        assert_eq!(s.last_status, 0x02);
        assert_eq!(s.last_slot, 3);
        assert_eq!(s.max_bindings, 16);
        assert!(s.dirty);
        assert_eq!(s.active_mask, 0b101);
        assert!(s.is_slot_active(0) && s.is_slot_active(2) && !s.is_slot_active(1));
        assert_eq!(s.slot_health(3), 0x1A);
        assert_eq!(s.ir_active_mask, 0x8003);
        assert_eq!(s.ir_learn_state, 0);
        assert_eq!(s.ir_cmd_status.len(), 16);
        assert_eq!(s.encode(), d);
    }

    /// Caps v6 widened the mask to 16 bits and the table to 16 entries. At the
    /// old offsets the learn state lands a byte early and sub-slot 15's result
    /// is read as sub-slot 14's.
    #[test]
    fn the_sixteenth_ir_sub_slot_is_reachable() {
        let s = CsStatusPacket::decode(&status_bytes()).unwrap();
        assert!(
            s.is_ir_active(15),
            "sub-slot 15 has no bit in an 8-bit mask"
        );
        assert_eq!(s.ir_cmd_status[15], 0x1E);
    }

    /// A device reporting smaller tables is decoded at its own size, not ours.
    #[test]
    fn the_status_tables_are_sized_by_the_caps_header() {
        assert_eq!(CsStatusPacket::wire_len(16, 8), 33);
        let mut d = vec![0u8; 33];
        d[2] = 16;
        d[22] = 0x03; // ir_active_mask low byte at the shorter offset
        let s = CsStatusPacket::decode_sized(&d, 16, 8).unwrap();
        assert_eq!(s.ir_cmd_status.len(), 8);
        assert_eq!(s.ir_active_mask, 3);
    }

    // ----------------------------------------------------------------- caps

    #[test]
    fn caps_header_locates_its_maxima_past_the_type_table() {
        // control_surfaces.h:637-652: the maxima sit at 4 + 4*type_count.
        assert_eq!(CsCapsHeader::wire_len(9), CsCapsHeader::SIZE_AT_V13);
        assert_eq!(CsCapsHeader::SIZE_AT_V13, 44);

        let mut d = vec![13u8, 16, 9, 57];
        for _ in 0..9 {
            d.extend([0xBC, 0x02, 1, 0]);
        }
        d.extend([16, 8, 8, 8]);
        assert_eq!(d.len(), 44);

        let caps = CsCapsHeader::decode(&d).unwrap();
        assert_eq!(caps.caps_version, 13);
        assert_eq!(caps.type_count, 9);
        assert_eq!(caps.noun_count, 57);
        assert_eq!(caps.types.len(), 9);
        assert_eq!(caps.types[0].actions, 0x02BC);
        assert_eq!(caps.max_ir_commands, 16);
        assert_eq!(caps.max_groups, 8);
        assert_eq!(caps.max_macros, 8);
        assert_eq!(caps.max_macro_steps, 8);
        assert_eq!(caps.encode(), d);
    }

    /// The same build has to talk to a v9 device, whose table is one row
    /// shorter and whose header is therefore 40 bytes.
    #[test]
    fn a_shorter_type_table_moves_the_maxima_down() {
        let mut d = vec![9u8, 16, 8, 53];
        for _ in 0..8 {
            d.extend([0xBC, 0x02, 1, 0]);
        }
        d.extend([16, 8, 8, 8]);
        assert_eq!(d.len(), 40);
        let caps = CsCapsHeader::decode(&d).unwrap();
        assert_eq!(caps.max_ir_commands, 16);
        assert_eq!(caps.max_macro_steps, 8);
    }

    /// A pre-v9 device answers without the tail; zeros there mean "no groups,
    /// no macros", which needs no version test of its own.
    #[test]
    fn a_missing_caps_tail_reads_as_absent_not_as_an_error() {
        let mut d = vec![3u8, 16, 8, 35];
        for _ in 0..8 {
            d.extend([0, 0, 1, 0]);
        }
        let caps = CsCapsHeader::decode(&d).unwrap();
        assert_eq!(caps.max_ir_commands, 0);
        assert_eq!(caps.max_groups, 0);
    }

    #[test]
    fn noun_desc_offsets_match_the_header() {
        // control_surfaces.h:578-589.
        assert_eq!(CsNounDesc::SIZE, 12);
        let n = CsNounDesc {
            kind: 0,         // :579
            enum_count: 0,   // :580
            actions: 0x0F0F, // :581, bytes 2-3
            min_q: -5120,    // :583, bytes 4-5
            max_q: 4608,     // :584, bytes 6-7
            unit: 1,         // :585
            target_kind: 2,  // :586
            target_count: 9, // :587
            dflags: 1,       // :588
        };
        let w = n.encode();
        assert_eq!(&w[2..4], &0x0F0Fu16.to_le_bytes());
        assert_eq!(&w[4..6], &(-5120i16).to_le_bytes());
        assert_eq!(&w[6..8], &4608i16.to_le_bytes());
        assert_eq!(w[8], 1);
        assert_eq!(w[9], 2);
        assert_eq!(w[10], 9);
        assert_eq!(w[11], 1);
        assert_eq!(CsNounDesc::decode(&w).unwrap(), n);
        assert!(n.is_available() && n.is_targeted() && !n.has_band());
    }

    /// A noun with no actions is how the firmware reports a platform
    /// difference, such as ADAT on an RP2040. Offering it would produce a
    /// binding the device silently refuses.
    #[test]
    fn a_noun_with_no_actions_is_unavailable() {
        assert!(!CsNounDesc::decode(&[0; 12]).unwrap().is_available());
    }

    #[test]
    fn type_desc_offsets_match_the_header() {
        // control_surfaces.h:555-559.
        assert_eq!(CsTypeDesc::SIZE, 4);
        let t = CsTypeDesc {
            actions: 0x02BC,
            pin_count: 2,
            pin_class: CsTypeDesc::PINCLASS_ADC,
        };
        let w = t.encode();
        assert_eq!(&w[0..2], &0x02BCu16.to_le_bytes());
        assert_eq!(w[2], 2);
        assert_eq!(w[3], 1);
        assert_eq!(CsTypeDesc::decode(&w).unwrap(), t);
    }

    // --------------------------------------------------------------- matrix

    #[test]
    fn matrix_route_offsets_match_the_header() {
        // config.h:845-851.
        assert_eq!(MatrixRoutePacket::SIZE, 8);
        let r = MatrixRoutePacket {
            input: 3,
            output: 5,
            enabled: true,
            phase_invert: true,
            gain_db: -6.0,
        };
        let w = r.encode();
        assert_eq!(w[0], 3, "input");
        assert_eq!(w[1], 5, "output");
        assert_eq!(w[2], 1, "enabled");
        assert_eq!(w[3], 1, "phase_invert");
        assert_eq!(&w[4..8], &(-6.0f32).to_le_bytes(), "gain_db");
        assert_eq!(MatrixRoutePacket::decode(&w).unwrap(), r);
    }

    #[test]
    fn an_eq_packet_decodes_what_the_encoder_wrote() {
        let p = EqParamPacket {
            channel: 8,
            band: 3,
            filter_type: FilterType::Peaking,
            bypass: true,
            freq: 2856.0,
            q: 3.58,
            gain_db: -8.6,
            qp: None,
        };
        let bytes = p.encode();
        assert_eq!(bytes.len(), 16, "a non-Linkwitz write stays 16 bytes");
        assert_eq!(decode_eq_param(&bytes).unwrap(), p);
    }

    /// The Linkwitz form carries a fourth parameter in two extra bytes; only
    /// that type may send them, and only that type reads them back.
    #[test]
    fn the_linkwitz_sidecar_survives_a_round_trip() {
        let p = EqParamPacket {
            channel: 0,
            band: 1,
            filter_type: FilterType::LinkwitzTransform,
            bypass: false,
            freq: 40.0,
            q: 0.7,
            gain_db: 30.0,
            qp: Some(1.2),
        };
        let bytes = p.encode();
        assert_eq!(bytes.len(), 18);
        let back = decode_eq_param(&bytes).unwrap();
        assert!((back.qp.unwrap() - 1.2).abs() < 0.01);
    }

    // -------------------------------------------------------------- upmixer

    #[test]
    fn upmix_config_offsets_match_the_header() {
        // upmix.h:167-183.
        assert_eq!(UpmixConfigPacket::SIZE, 44);
        let c = UpmixConfigPacket {
            enabled: true,
            center_mode: 2, // UPMIX_CENTER_OFF, widened at V27
            surround_mode: 1,
            presence_db: -3.0,
            strength_pct: 100.0,
            center_width_pct: 25.0,
            corr_threshold_pct: 30.0,
            attack_ms: 10.0,
            release_ms: 100.0,
            detector_hpf_hz: 200.0,
            surround_delay_ms: 12.0,
            surround_hpf_hz: 300.0,
            surround_lpf_hz: 7000.0,
            decorr_pct: 90.0,
        };
        let w = c.encode();
        assert_eq!(w[0], 1, "enabled");
        assert_eq!(w[1], 2, "center_mode");
        assert_eq!(w[2], 1, "surround_mode");
        assert_eq!(w[3] as i8, -6, "presence_q1 is dB times two");
        assert_eq!(&w[4..8], &100.0f32.to_le_bytes(), "strength_pct");
        assert_eq!(&w[40..44], &90.0f32.to_le_bytes(), "decorr_pct");
        assert_eq!(UpmixConfigPacket::decode(&w).unwrap(), c);
    }

    #[test]
    fn upmix_status_offsets_match_the_header() {
        // upmix.h:216-227.
        assert_eq!(UpmixStatus::SIZE, 16);
        let s = UpmixStatus {
            active: true,
            parked_reason: 0,
            corr_q14: 8192,
            balance_q14: 16384,
            center_gain_q15: 32767,
            ls_gain_q15: 16384,
            rs_gain_q15: 8192,
        };
        let w = s.encode();
        assert_eq!(&w[2..4], &8192i16.to_le_bytes());
        assert_eq!(&w[4..6], &16384i16.to_le_bytes());
        assert_eq!(&w[6..8], &32767u16.to_le_bytes());
        assert_eq!(&w[8..10], &16384u16.to_le_bytes());
        assert_eq!(&w[10..12], &8192u16.to_le_bytes());
        assert_eq!(UpmixStatus::decode(&w).unwrap(), s);
        assert!((s.correlation() - 0.5).abs() < 1e-6);
        assert!((s.balance() - 1.0).abs() < 1e-6);
        assert!((s.gains().0 - 1.0).abs() < 1e-4);
    }

    // ----------------------------------------------------- signal generator

    #[test]
    fn siggen_config_offsets_match_the_header() {
        // siggen.h:86-98.
        assert_eq!(SiggenConfig::SIZE, 36);
        let c = SiggenConfig {
            version: 1,
            signal_type: 4,
            channel_mask: 0x0003,
            invert_mask: 0x0002,
            flags: SiggenConfig::FLAG_RAW,
            level_db: -20.0,
            duration_ms: 5000,
            repeat_count: 3,
            gap_ms: 250,
            p1: 20.0,
            p2: 20000.0,
            p3: 1.0,
            p4: 2.0,
        };
        let w = c.encode();
        assert_eq!(w[0], 1, "version");
        assert_eq!(w[1], 4, "signal_type");
        assert_eq!(&w[2..4], &0x0003u16.to_le_bytes(), "channel_mask");
        assert_eq!(&w[4..6], &0x0002u16.to_le_bytes(), "invert_mask");
        assert_eq!(w[6], 1, "flags");
        assert_eq!(w[7], 0, "reserved0");
        assert_eq!(&w[8..12], &(-20.0f32).to_le_bytes(), "level_db");
        assert_eq!(&w[12..16], &5000u32.to_le_bytes(), "duration_ms");
        assert_eq!(&w[16..18], &3u16.to_le_bytes(), "repeat");
        assert_eq!(&w[18..20], &250u16.to_le_bytes(), "gap_ms");
        assert_eq!(&w[20..24], &20.0f32.to_le_bytes(), "p1");
        assert_eq!(&w[32..36], &2.0f32.to_le_bytes(), "p4");
        assert_eq!(SiggenConfig::decode(&w).unwrap(), c);
    }

    #[test]
    fn siggen_status_offsets_match_the_header() {
        // siggen.h:110-120.
        assert_eq!(SiggenStatus::SIZE, 16);
        let s = SiggenStatus {
            version: 1,
            state: 2,
            signal_type: 4,
            active_channel: 0xFF,
            elapsed_ms: 1234,
            cycles_done: 7,
            stop_reason: 2,
            current_freq: 997.0,
        };
        let w = s.encode();
        assert_eq!(&w[4..8], &1234u32.to_le_bytes());
        assert_eq!(&w[8..10], &7u16.to_le_bytes());
        assert_eq!(w[10], 2);
        assert_eq!(w[11], 0, "reserved0");
        assert_eq!(&w[12..16], &997.0f32.to_le_bytes());
        assert_eq!(SiggenStatus::decode(&w).unwrap(), s);
        assert!(s.is_running());
    }

    #[test]
    fn siggen_caps_header_offsets_match_the_header() {
        // siggen.h:136-143.
        assert_eq!(SiggenCapsHeader::SIZE, 8);
        let h = SiggenCapsHeader {
            version: 1,
            type_count: 15,
            output_channels: 9,
            multitone_max: 16,
            valid_channel_mask: 0x01FF,
        };
        let w = h.encode();
        assert_eq!(&w[4..6], &0x01FFu16.to_le_bytes());
        assert_eq!(&w[6..8], &[0, 0], "reserved0");
        assert_eq!(SiggenCapsHeader::decode(&w).unwrap(), h);
    }

    /// 62 bytes: id, an eight-byte name, the timing model, then four 13-byte
    /// parameter descriptors whose floats are unaligned on the wire.
    #[test]
    fn siggen_type_desc_offsets_match_the_header() {
        // siggen.h:154-169.
        assert_eq!(SiggenTypeDesc::SIZE, 62);
        assert_eq!(SiggenParamDesc::SIZE, 13);
        assert_eq!(
            SiggenTypeDesc::PARAMS_AT + 4 * SiggenParamDesc::SIZE,
            SiggenTypeDesc::SIZE
        );

        let d = SiggenTypeDesc {
            id: 4,
            name: "SweepLog".into(),
            timing_model: 1,
            params: [
                SiggenParamDesc {
                    semantic: 1,
                    min: 10.0,
                    max: 24000.0,
                    default: 20.0,
                },
                SiggenParamDesc {
                    semantic: 1,
                    min: 10.0,
                    max: 24000.0,
                    default: 20000.0,
                },
                SiggenParamDesc::default(),
                SiggenParamDesc::default(),
            ],
        };
        let w = d.encode();
        assert_eq!(w[0], 4, "id");
        assert_eq!(&w[1..9], b"SweepLog", "name fills all eight bytes");
        assert_eq!(w[9], 1, "timing_model");
        assert_eq!(w[10], 1, "params[0].semantic");
        assert_eq!(&w[11..15], &10.0f32.to_le_bytes(), "params[0].min");
        assert_eq!(&w[23], &1, "params[1].semantic at 10 + 13");
        assert_eq!(SiggenTypeDesc::decode(&w).unwrap(), d);
    }

    // ------------------------------------------------ board-level interfaces

    #[test]
    fn dac_hw_mute_offsets_match_the_header() {
        // bulk_params.h:286-294, the same layout as the vendor command.
        assert_eq!(DacHwMuteConfig::SIZE, 16);
        let c = DacHwMuteConfig {
            enabled: true,
            active_low: true,
            pin: 11,
            hold_ms: 5,
            release_ms: 0,
        };
        let w = c.encode();
        assert_eq!(w[0], 1);
        assert_eq!(w[1], 1);
        assert_eq!(w[2], 11);
        assert_eq!(w[3], 0, "reserved0 aligns hold_ms");
        assert_eq!(&w[4..6], &5u16.to_le_bytes());
        assert_eq!(&w[6..8], &0u16.to_le_bytes());
        assert!(w[8..].iter().all(|b| *b == 0), "reserved");
        assert_eq!(DacHwMuteConfig::decode(&w).unwrap(), c);
    }

    #[test]
    fn uart_config_offsets_match_the_header() {
        // config.h:525-531.
        assert_eq!(UartCtrlConfig::SIZE, 8);
        let c = UartCtrlConfig {
            enabled: true,
            tx_pin: 16,
            rx_pin: 17,
            notify_enable: true,
            baud: 115_200,
        };
        let w = c.encode();
        assert_eq!(w[0], 1);
        assert_eq!(w[1], 16);
        assert_eq!(w[2], 17);
        assert_eq!(w[3], 1);
        assert_eq!(&w[4..8], &115_200u32.to_le_bytes());
        assert_eq!(UartCtrlConfig::decode(&w).unwrap(), c);
    }

    #[test]
    fn i2c_config_offsets_match_the_header() {
        // config.h:535-541.
        assert_eq!(I2cCtrlConfig::SIZE, 8);
        let c = I2cCtrlConfig {
            enabled: true,
            sda_pin: 18,
            scl_pin: 19,
            address: 0x42,
        };
        let w = c.encode();
        assert_eq!(w[..4], [1, 18, 19, 0x42]);
        assert!(w[4..].iter().all(|b| *b == 0), "reserved");
        assert_eq!(I2cCtrlConfig::decode(&w).unwrap(), c);
    }

    #[test]
    fn ctrl_iface_status_offsets_match_the_header() {
        // config.h:547-554.
        assert_eq!(CtrlIfaceStatus::SIZE, 8);
        let s = CtrlIfaceStatus {
            uart_last_status: 0,
            uart_live: true,
            i2c_last_status: 2,
            i2c_live: false,
            proto_version: 1,
        };
        let w = s.encode();
        assert_eq!(w[..5], [0, 1, 2, 0, 1]);
        assert_eq!(CtrlIfaceStatus::decode(&w).unwrap(), s);
    }

    // ---------------------------------------------------------- diagnostics

    #[test]
    fn buffer_stats_offsets_match_the_header() {
        // config.h:1018-1044: 4 + 4x8 + 8.
        assert_eq!(BufferStatsPacket::SIZE, 44);
        assert_eq!(SpdifBufferStats::SIZE, 8);
        assert_eq!(PdmBufferStats::SIZE, 8);
        assert_eq!(BufferStatsPacket::PDM_AT, 36);

        let mut p = BufferStatsPacket {
            num_spdif: 4,
            flags: BufferStatsPacket::FLAG_PDM_ACTIVE | BufferStatsPacket::FLAG_STREAMING,
            sequence: 4321,
            ..Default::default()
        };
        p.spdif[2].consumer_fill_pct = 61;
        p.pdm.ring_max_fill_pct = 99;

        let w = p.encode();
        assert_eq!(w[0], 4);
        assert_eq!(&w[2..4], &4321u16.to_le_bytes());
        assert_eq!(w[4 + 2 * 8 + 3], 61, "spdif[2].consumer_fill_pct");
        assert_eq!(w[36 + 5], 99, "pdm.ring_max_fill_pct");
        assert!(p.pdm_active() && p.streaming());
        assert_eq!(BufferStatsPacket::decode(&w).unwrap(), p);
    }

    #[test]
    fn spdif_rx_status_offsets_match_the_survey() {
        // config.h:447; survey 6.6.
        assert_eq!(SpdifRxStatusPacket::SIZE, 16);
        let s = SpdifRxStatusPacket {
            state: SpdifRxState::Locked,
            input_source: InputSource::Spdif,
            lock_count: 3,
            loss_count: 1,
            sample_rate: 48000,
            parity_errors: 7,
            fifo_fill_pct: 55,
            lib_state: 2,
            callback_counts: 9,
        };
        let w = s.encode();
        assert_eq!(w[0], 2);
        assert_eq!(w[1], 1);
        assert_eq!(&w[4..8], &48000u32.to_le_bytes());
        assert_eq!(&w[8..12], &7u32.to_le_bytes());
        assert_eq!(&w[12..14], &55u16.to_le_bytes());
        assert_eq!(SpdifRxStatusPacket::decode(&w).unwrap(), s);
    }

    /// An unknown state must not be flattened onto a known one: a newer
    /// firmware's fifth lock state would otherwise read as "inactive".
    #[test]
    fn an_unknown_spdif_state_survives_the_decoder() {
        let mut w = [0u8; 16];
        w[0] = 9;
        let s = SpdifRxStatusPacket::decode(&w).unwrap();
        assert_eq!(s.state, SpdifRxState::Unknown(9));
        assert_eq!(s.encode()[0], 9);
    }

    /// Bit 0 of this mask is input 1 and is always set: the firmware answers
    /// `(spdif_rx_enabled_ext << 1) | 1`. The bulk packet's mask is the other
    /// one, shifted down a bit, and the two are easy to confuse.
    #[test]
    fn the_spdif_enable_mask_starts_at_input_one() {
        // config.h:456, vendor_commands.c:3407.
        assert_eq!(SpdifInputConfig::SIZE, 6);
        let c = SpdifInputConfig {
            count: 4,
            // inputs 1 and 3 on: (0b010 << 1) | 1.
            enable_mask: 0b0101,
            gpio: [5, 20, 21, 22],
        };
        assert!(c.enabled(0), "input 1 is always enabled");
        assert!(!c.enabled(1), "input 2 is off");
        assert!(c.enabled(2), "input 3 is on");
        assert!(!c.enabled(3));

        let w = c.encode();
        assert_eq!(w[0], 4, "count");
        assert_eq!(w[1], 0b0101, "enable_mask");
        assert_eq!(&w[2..6], &[5, 20, 21, 22], "gpio[0..3]");
        assert_eq!(SpdifInputConfig::decode(&w).unwrap(), c);
    }

    #[test]
    fn adat_input_status_offsets_match_the_survey() {
        // config.h:354; survey 1.7.
        assert_eq!(AdatInputStatusPacket::SIZE, 20);
        let s = AdatInputStatusPacket {
            state: 3,
            clock_mode: 1,
            enabled: true,
            pin: 12,
            rate_ok: true,
            lock_count: 2,
            loss_count: 1,
            slip_count: 0,
            header_err: 300,
            detected_rate: 48000,
            measured_hz: 48001,
        };
        let w = s.encode();
        assert_eq!(&w[..8], &[3, 1, 1, 12, 1, 2, 1, 0]);
        assert_eq!(&w[8..10], &300u16.to_le_bytes());
        assert_eq!(&w[10..12], &[0, 0], "reserved");
        assert_eq!(&w[12..16], &48000u32.to_le_bytes());
        assert_eq!(&w[16..20], &48001u32.to_le_bytes());
        assert!(s.is_locked());
        assert_eq!(AdatInputStatusPacket::decode(&w).unwrap(), s);
    }

    #[test]
    fn adat_output_status_offsets_match_the_survey() {
        // config.h:354.
        assert_eq!(AdatStatus::SIZE, 8);
        let s = AdatStatus {
            enabled: true,
            active: true,
            pin: 12,
            rate_ok: true,
            resync_count: 4,
            slip_count: 0,
        };
        let w = s.encode();
        assert_eq!(&w[..4], &[1, 1, 12, 1]);
        assert_eq!(&w[4..6], &4u16.to_le_bytes());
        assert_eq!(AdatStatus::decode(&w).unwrap(), s);
    }

    #[test]
    fn i2s_slave_status_offsets_match_the_survey() {
        // config.h:480.
        assert_eq!(I2sSlaveStatusPacket::SIZE, 16);
        let s = I2sSlaveStatusPacket {
            state: 3,
            clock_mode: 1,
            lock_count: 5,
            loss_count: 2,
            detected_rate: 96000,
            measured_hz: 95999,
        };
        let w = s.encode();
        assert_eq!(&w[..4], &[3, 1, 5, 2]);
        assert_eq!(&w[4..8], &96000u32.to_le_bytes());
        assert_eq!(&w[8..12], &95999u32.to_le_bytes());
        assert!(s.is_locked());
        assert_eq!(I2sSlaveStatusPacket::decode(&w).unwrap(), s);
    }

    #[test]
    fn lg_sound_sync_status_offsets_match_the_header() {
        // bulk_params.h:252-258, the layout the vendor command shares.
        assert_eq!(LgSoundSyncStatus::SIZE, 16);
        let s = LgSoundSyncStatus {
            enabled: true,
            present: true,
            volume: 42,
            muted: false,
        };
        let w = s.encode();
        assert_eq!(&w[..4], &[1, 1, 42, 0]);
        assert!(w[4..].iter().all(|b| *b == 0), "reserved");
        assert_eq!(LgSoundSyncStatus::decode(&w).unwrap(), s);
        // The never-decoded sentinel must not read as silence.
        assert_eq!(
            LgSoundSyncStatus::default().volume,
            LgSoundSyncStatus::VOLUME_UNKNOWN
        );
    }

    // -------------------------------------------------------------- presets

    /// Seven bytes, not the six the old specification claimed.
    #[test]
    fn preset_directory_offsets_match_the_firmware() {
        // config.h:296; survey 1.12.
        assert_eq!(PresetDirectory::SIZE, 7);
        let d = PresetDirectory {
            occupied: 0b0000_0011_0000_0101,
            startup_mode: 1,
            default_slot: 2,
            last_active: 3,
            output_config_mode: 1,
            master_volume_mode: 0,
        };
        let w = d.encode();
        assert_eq!(&w[0..2], &d.occupied.to_le_bytes(), "occupied, LE");
        assert_eq!(w[2], 1, "startup_mode");
        assert_eq!(w[3], 2, "default_slot");
        assert_eq!(w[4], 3, "last_active");
        assert_eq!(w[5], 1, "output_config_mode");
        assert_eq!(w[6], 0, "master_volume_mode");
        assert!(d.is_occupied(0) && !d.is_occupied(1) && d.is_occupied(9));
        assert_eq!(PresetDirectory::decode(&w).unwrap(), d);
    }

    /// The SET takes two bytes and the GET answers three: the third is the
    /// live active slot, which is read-only and cannot be written back.
    #[test]
    fn preset_startup_is_asymmetric() {
        // config.h:301-302.
        assert_eq!(PresetStartup::SET_SIZE, 2);
        assert_eq!(PresetStartup::SIZE, 3);
        let s = PresetStartup::decode(&[0, 4, 7]).unwrap();
        assert_eq!(s.mode, 0);
        assert_eq!(s.default_slot, 4);
        assert_eq!(s.last_active, 7);
        assert_eq!(s.encode(), [0, 4], "the SET cannot carry last_active");
    }

    // --------------------------------------------------------------- system

    /// `n*2 + 7` bytes, with a 32-bit clip latch: 41 on RP2350, 21 on RP2040.
    #[test]
    fn system_status_is_sized_by_the_channel_count() {
        assert_eq!(SystemStatus::wire_len(17), 41);
        assert_eq!(SystemStatus::wire_len(7), 21);

        let s = SystemStatus {
            peaks: (0..17).map(|i| (i * 1000) as u16).collect(),
            cpu0: 42,
            cpu1: 91,
            clip_flags: 1 << 16,
            active_inputs: 2,
        };
        let w = s.encode();
        assert_eq!(w.len(), 41);
        assert_eq!(&w[32..34], &16000u16.to_le_bytes(), "peaks[16]");
        assert_eq!(w[34], 42, "cpu0");
        assert_eq!(w[35], 91, "cpu1");
        assert_eq!(&w[36..40], &(1u32 << 16).to_le_bytes(), "clip_flags");
        assert_eq!(w[40], 2, "active_inputs");
        assert_eq!(SystemStatus::decode(&w).unwrap(), s);
        // Shifted on a u32: channel 17 exists and its bit is real.
        assert!(s.clipped(16));
        assert!(!s.clipped(15));
    }

    #[test]
    fn an_rp2040_status_packet_decodes_at_its_own_length() {
        let s = SystemStatus {
            peaks: vec![0; 7],
            cpu0: 10,
            cpu1: 0,
            clip_flags: 0,
            active_inputs: 2,
        };
        let w = s.encode();
        assert_eq!(w.len(), 21);
        assert_eq!(SystemStatus::decode(&w).unwrap().peaks.len(), 7);
    }

    // ------------------------------------------------------ identification

    /// A beta2 device answers 4 bytes whatever it is asked (config.h:325 at
    /// 112f35b): the nibbles, and no ordinal. It claims 1.1.6, which only an
    /// early beta can do without the ordinal (the Console's `earlyBeta`).
    #[test]
    fn a_four_byte_platform_reply_decodes_the_nibbles() {
        let p = PlatformInfo::decode(&[1, 1, 0x16, 9]).unwrap();
        assert_eq!(p.platform(), Platform::Rp2350);
        assert_eq!(p.num_output_channels, 9);
        assert_eq!(p.reply_len, 4);
        assert_eq!(p.version.pre, Prerelease::EarlyBeta);
        assert_eq!(p.firmware(), "1.1.6 early beta");

        // Below 1.1.6 a short reply is a final release, as it always was.
        let old = PlatformInfo::decode(&[0, 1, 0x15, 5]).unwrap();
        assert_eq!(old.version, FirmwareVersion::new(1, 1, 5, 0));
        assert_eq!(old.firmware(), "1.1.5");
        assert_eq!(old.platform(), Platform::Rp2040);

        // An unknown platform id is preserved rather than rejected.
        assert_eq!(
            PlatformInfo::decode(&[7, 1, 0x16, 9]).unwrap().platform(),
            Platform::Unknown(7)
        );
        assert!(PlatformInfo::decode(&[1, 1, 0x16]).is_err());
    }

    /// Six bytes: full-width minor and patch in bytes 4 and 5, which win over
    /// the nibbles (firmware_versioning_spec.md:109), and still no ordinal.
    #[test]
    fn a_six_byte_platform_reply_takes_minor_and_patch_from_their_own_bytes() {
        // Patch 17 does not fit a nibble; byte 2 has wrapped and must be ignored.
        let p = PlatformInfo::decode(&[1, 1, 0x11, 9, 1, 17]).unwrap();
        assert_eq!((p.version.minor, p.version.patch), (1, 17));
        assert_eq!(p.version.pre, Prerelease::EarlyBeta);
        let p = PlatformInfo::decode(&[1, 1, 0x15, 9, 1, 5]).unwrap();
        assert_eq!(p.version, FirmwareVersion::new(1, 1, 5, 0));
    }

    /// Seven bytes: the beta ordinal in byte 6, 0 for a final release
    /// (config.h:664-667, vendor_commands.c:2638-2655).
    #[test]
    fn a_seven_byte_platform_reply_carries_the_beta() {
        let p = PlatformInfo::decode(&[1, 1, 0x16, 9, 1, 6, 4]).unwrap();
        assert_eq!(p.reply_len, 7);
        assert_eq!(p.version, FirmwareVersion::new(1, 1, 6, 4));
        assert_eq!(p.firmware(), "1.1.6 beta 4");
        assert_eq!(p.encode(), [1, 1, 0x16, 9, 1, 6, 4]);
        assert_eq!(PlatformInfo::decode(&p.encode()).unwrap(), p);

        let fin = PlatformInfo::decode(&[0, 1, 0x16, 5, 1, 6, 0]).unwrap();
        assert_eq!(fin.version.pre, Prerelease::Final);
        assert_eq!(fin.firmware(), "1.1.6");
    }

    /// A beta precedes the final release of its patch: order by
    /// `(major, minor, patch, beta == 0 ? 256 : beta)`
    /// (firmware_versioning_spec.md:111). An early beta sorts below beta 1.
    #[test]
    fn firmware_versions_order_betas_before_their_release() {
        let v = FirmwareVersion::new;
        let early = FirmwareVersion {
            pre: Prerelease::EarlyBeta,
            ..v(1, 1, 6, 0)
        };
        let order = [
            v(1, 1, 5, 0),
            early,
            v(1, 1, 6, 1),
            v(1, 1, 6, 3),
            v(1, 1, 6, 4),
            v(1, 1, 6, 0),
            v(1, 1, 7, 1),
            v(1, 2, 0, 0),
        ];
        for w in order.windows(2) {
            assert!(w[0] < w[1], "{} should sort before {}", w[0], w[1]);
        }
        assert_eq!(v(1, 1, 6, 4).beta(), Some(4));
        assert_eq!(v(1, 1, 6, 0).beta(), Some(0));
        assert_eq!(early.beta(), None);
    }

    /// The version this build expects is the headers' (config.h:659-667).
    #[test]
    fn the_expected_firmware_is_the_vendored_one() {
        assert_eq!(
            FirmwareVersion::expected(),
            FirmwareVersion::new(1, 1, 6, 4)
        );
        assert_eq!(FirmwareVersion::expected().to_string(), "1.1.6 beta 4");
    }

    /// 64 bytes: describe in 0-47, date in 48-59, zeros in 60-63
    /// (vendor_commands.c:2657-2667).
    #[test]
    fn build_info_reads_describe_and_date_from_their_offsets() {
        let mut b = [0u8; 64];
        b[..16].copy_from_slice(b"v1.1.6-beta4-2-g");
        b[16..23].copy_from_slice(b"557bce7");
        b[48..58].copy_from_slice(b"2026-09-28");
        let info = BuildInfo::decode(&b).unwrap();
        assert_eq!(info.describe, "v1.1.6-beta4-2-g557bce7");
        assert_eq!(info.date, "2026-09-28");
        assert_eq!(BuildInfo::SIZE, 64);
        assert_eq!(info.encode(), b);
        assert!(BuildInfo::decode(&b[..40]).is_err());
    }

    // -------------------------------------------------- control surfaces v20

    /// Caps v18 grew the type table to 11 rows, so the header is 52 bytes and
    /// the maxima sit at 4 + 4*11 = 48 (control_surfaces.h:637-652).
    #[test]
    fn a_v20_caps_header_has_eleven_types_and_its_maxima_at_48() {
        assert_eq!(CsCapsHeader::wire_len(11), CsCapsHeader::SIZE_AT_V20);
        assert_eq!(CsCapsHeader::SIZE_AT_V20, 52);

        let mut d = vec![20u8, 16, 11, 79];
        for t in 0..11u8 {
            // A distinctive table, so reading a row as a maximum shows.
            d.extend([0xEE, 0xEE, 1, t]);
        }
        d.extend([16, 8, 8, 8]);
        assert_eq!(d.len(), 52);
        let caps = CsCapsHeader::decode(&d).unwrap();
        assert_eq!(
            (caps.caps_version, caps.type_count, caps.noun_count),
            (20, 11, 79)
        );
        assert_eq!(caps.types.len(), 11);
        assert_eq!(caps.types[10].pin_class, 10, "the AUX_PWM row");
        assert_eq!(
            (
                caps.max_ir_commands,
                caps.max_groups,
                caps.max_macros,
                caps.max_macro_steps
            ),
            (16, 8, 8, 8)
        );
        assert_eq!(caps.encode(), d);
    }

    /// An aux container carries its boot flags in byte 22 (control_surfaces.h:
    /// 425-437, 468-470).
    #[test]
    fn an_aux_binding_carries_its_extras() {
        let b = CsBinding::from_fields(&[
            ("type", "aux_pwm"),
            ("gpio", "22"),
            ("extras", "boot_on,linear"),
            ("value", "12800"),
        ])
        .unwrap();
        assert_eq!(b.component, 10);
        assert_eq!(b.extras, aux_extras::BOOT_ON | aux_extras::LINEAR);
        let w = b.encode();
        assert_eq!(w[22], 0x05);
        assert_eq!(w[23], 0);
        let fields = b.to_fields();
        assert!(fields.contains(&("extras", "boot_on,linear".to_string())));
        assert_eq!(
            (
                aux_extras::BOOT_ON,
                aux_extras::BOOT_SAVED,
                aux_extras::LINEAR
            ),
            (0x01, 0x02, 0x04)
        );
    }

    /// `CS_UNIT_MS_LOG` (control_surfaces.h:268-269) is plain integer ms for
    /// the value and range, like Hz, and steps in 8.8 octaves, like Hz: the
    /// limiter release's 10 to 1000 ms would not fit 8.8's 127 ms.
    #[test]
    fn ms_log_values_are_plain_milliseconds_and_step_in_octaves() {
        assert_eq!(cs_unit::MS_LOG, 6);
        assert!(!cs_unit_is_fixed_point(cs_unit::MS_LOG));
        assert!(cs_unit_is_log(cs_unit::MS_LOG));
        assert_eq!(cs_encode_value(1000.0, cs_unit::MS_LOG), 1000);
        assert_eq!(cs_decode_value(10, cs_unit::MS_LOG), 10.0);
        // The old MS unit is 8.8 and linear.
        assert!(cs_unit_is_fixed_point(cs_unit::MS));
        assert!(!cs_unit_is_log(cs_unit::MS));
        assert_eq!(cs_encode_value(1.5, cs_unit::MS), 384);
        // Hz and Q step in octaves, dB and percent linearly.
        assert!(cs_unit_is_log(cs_unit::HZ) && cs_unit_is_log(cs_unit::Q));
        assert!(!cs_unit_is_log(cs_unit::DB) && !cs_unit_is_log(cs_unit::PERCENT));
        // A noun descriptor for the limiter release, as a v20 device reports it.
        let nd = CsNounDesc {
            kind: 0,
            min_q: 10,
            max_q: 1000,
            unit: cs_unit::MS_LOG,
            target_kind: 2,
            target_count: 9,
            ..Default::default()
        };
        assert_eq!(cs_decode_value(nd.max_q, nd.unit), 1000.0);
    }

    /// The v18 target kind addresses a binding slot (control_surfaces.h:277).
    #[test]
    fn the_aux_target_kind_is_known() {
        assert!(CS_TARGET_KINDS.contains(&("aux", 5)));
        assert_eq!(crate::generated::cs::CS_TARGET_AUX, 5);
    }

    /// `REQ_GET_CS_AUX_STATE` with 0xFFFF: 16 states, then 16 levels as 8.8
    /// percent, 48 bytes (config.h:138-140).
    #[test]
    fn the_aux_state_block_is_states_then_levels() {
        assert_eq!(CsAuxStates::SIZE, 48);
        let mut d = vec![0u8; 48];
        d[3] = 1;
        d[16 + 2 * 3..16 + 2 * 3 + 2].copy_from_slice(&(75u16 * 256).to_le_bytes());
        d[15] = 1;
        let s = CsAuxStates::decode(&d).unwrap();
        assert!(s.is_on(3) && s.is_on(15) && !s.is_on(0));
        assert_eq!(s.level_percent(3), 75.0);
        assert_eq!(s.level_percent(0), 0.0);
        assert_eq!(s.encode().to_vec(), d);
        assert!(CsAuxStates::decode(&d[..47]).is_err());
    }

    // ------------------------------------------------------ spectrum analyser

    fn rta_caps() -> RtaCaps {
        RtaCaps {
            input_channels: 8,
            output_channels: 9,
            order_min: 8,
            order_max: 10,
            order_default: 10,
            bass_bands: 14,
            max_bands: 37,
            level_zero: 243,
            dynamic_range_db: 120,
            idle_timeout_ms: 5000,
            max_bin_frame: 529,
            bass_dynamic_range_db: 70,
        }
    }

    /// `RtaConfig`, 12 bytes (rta.h:59-70).
    #[test]
    fn rta_config_fields_are_at_their_offsets() {
        let c = RtaConfig {
            tap: 1,
            channel_mask: 0x01FF,
            fft_order: 10,
            avg_ms: 250,
            peak_decay_db_s: 20,
            flags: 0x01,
        };
        let w = c.encode();
        assert_eq!(w.len(), 12);
        assert_eq!(w[0], 3, "version");
        assert_eq!(w[1], 1, "tap");
        assert_eq!(&w[2..4], &0x01FFu16.to_le_bytes(), "channel_mask");
        assert_eq!(w[4], 10, "fft_order");
        assert_eq!(w[5], 0, "reserved0");
        assert_eq!(&w[6..8], &250u16.to_le_bytes(), "avg_ms");
        assert_eq!(w[8], 20, "peak_decay_db_s");
        assert_eq!(w[9], 1, "flags");
        assert_eq!(&w[10..12], &[0, 0], "reserved");
        assert_eq!(RtaConfig::decode(&w).unwrap(), c);
    }

    /// `RtaCaps`, 16 bytes (rta.h:72-87), and the level byte's meaning
    /// (rta_fft.h:44-45): 0.5 dB steps from `level_zero`.
    #[test]
    fn rta_caps_fields_are_at_their_offsets() {
        let c = rta_caps();
        let w = c.encode();
        assert_eq!(w.len(), 16);
        assert_eq!(
            &w[..10],
            &[3, 8, 9, 8, 10, 10, 14, 37, 243, 120],
            "version to dynamic_range_db"
        );
        assert_eq!(&w[10..12], &5000u16.to_le_bytes(), "idle_timeout_ms");
        assert_eq!(&w[12..14], &529u16.to_le_bytes(), "max_bin_frame");
        assert_eq!(&w[14..16], &70u16.to_le_bytes(), "bass_dynamic_range_db");
        assert_eq!(RtaCaps::decode(&w).unwrap(), c);

        assert_eq!(c.level_dbfs(243), 0.0);
        assert_eq!(c.level_dbfs(255), 6.0);
        assert_eq!(c.level_dbfs(0), -121.5);
        assert_eq!(rta_level_dbfs(233, 243), -5.0);
        // 37 bands arrive in two chunks of up to 32 (rta.c:393-405).
        assert_eq!(c.centre_chunks(), 1..=2);
        let chunk2: Vec<u8> = [16000u16, 20000, 25000, 31500, 40000]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        assert_eq!(
            decode_rta_centres(&chunk2),
            vec![16000, 20000, 25000, 31500, 40000]
        );
    }

    /// Protocol V3 is incompatible with V2, so every analyser record refuses
    /// any other version (rta.h:27).
    #[test]
    fn rta_records_refuse_other_protocol_versions() {
        let mut w = rta_caps().encode();
        w[0] = 2;
        assert_eq!(
            RtaCaps::decode(&w),
            Err(PacketError::Version {
                what: "RtaCaps",
                got: 2,
                want: 3
            })
        );
        let mut s = RtaStatus::default().encode();
        s[0] = 4;
        assert!(matches!(
            RtaStatus::decode(&s),
            Err(PacketError::Version { .. })
        ));
        let mut c = [0u8; 12];
        c[0] = 2;
        assert!(RtaConfig::decode(&c).is_err());
    }

    /// `RtaBandFrame`, 82 bytes (rta.h:89-99), and 0x0F's back-to-back
    /// frames (vendor_commands.c:4334-4355).
    #[test]
    fn rta_band_frames_are_82_bytes_with_both_arrays_at_full_width() {
        assert_eq!(RtaBandFrame::SIZE, 82);
        let mut f = RtaBandFrame {
            channel: 2,
            seq: 9,
            n_bands: 34,
            age_ms: 21,
            avg: [0; 37],
            peak: [0; 37],
        };
        f.avg[0] = 200;
        f.avg[36] = 1;
        f.peak[0] = 230;
        let w = f.encode();
        assert_eq!(&w[..4], &[3, 2, 9, 34]);
        assert_eq!(&w[4..6], &21u16.to_le_bytes(), "age_ms");
        assert_eq!(&w[6..8], &[0, 0], "reserved");
        assert_eq!(w[8], 200, "avg[0]");
        assert_eq!(w[8 + 36], 1, "avg[36]");
        assert_eq!(w[45], 230, "peak[0] follows all 37 averages");
        let back = RtaBandFrame::decode(&w).unwrap();
        assert_eq!(back, f);
        assert_eq!(back.avg_levels().len(), 34, "n_bands in use at 48 kHz");

        let mut two = w.to_vec();
        let mut g = f.clone();
        g.channel = 5;
        two.extend(g.encode());
        let all = RtaBandFrame::decode_all(&two).unwrap();
        assert_eq!(
            all.iter().map(|f| f.channel).collect::<Vec<_>>(),
            vec![2, 5]
        );
        assert!(
            RtaBandFrame::decode_all(&[]).unwrap().is_empty(),
            "nothing live"
        );
        assert!(RtaBandFrame::decode_all(&two[..100]).is_err());
    }

    /// The bin frame is a 16-byte header, `n_bins` levels and a tail that
    /// repeats the sequence number; the tail reads 0xFF mid-write and the
    /// sequence skips 0xFF (rta.h:101-110, rta.c:128-131, 254-256).
    #[test]
    fn a_bin_frame_is_whole_only_when_head_and_tail_agree() {
        let h = RtaBinHeader {
            channel: 1,
            seq: 42,
            fft_order: 10,
            sample_rate_hz: 48_000,
            n_bins: 512,
        };
        let hb = h.encode();
        assert_eq!(hb.len(), 16);
        assert_eq!(&hb[..4], &[3, 1, 42, 10]);
        assert_eq!(&hb[4..8], &48_000u32.to_le_bytes(), "sample_rate_hz");
        assert_eq!(&hb[8..10], &512u16.to_le_bytes(), "n_bins");
        assert_eq!(&hb[10..16], &[0; 6], "reserved[3]");
        assert_eq!(h.frame_len(), 529, "RtaCaps.max_bin_frame at order 10");

        let frame = RtaBinFrame {
            header: h,
            levels: vec![100; 512],
        };
        let mut w = frame.encode();
        assert_eq!(w.len(), 529);
        assert_eq!(*w.last().unwrap(), 42);
        assert_eq!(RtaBinFrame::decode(&w).unwrap(), frame);

        // A newer frame's tail under an older head: torn.
        *w.last_mut().unwrap() = 43;
        assert_eq!(
            RtaBinFrame::decode(&w),
            Err(PacketError::Torn { head: 42, tail: 43 })
        );
        // Mid-write, the tail is 0xFF.
        *w.last_mut().unwrap() = 0xFF;
        assert!(matches!(
            RtaBinFrame::decode(&w),
            Err(PacketError::Torn { .. })
        ));
        // Short of the tail is short, not torn.
        assert!(matches!(
            RtaBinFrame::decode(&w[..400]),
            Err(PacketError::TooShort { .. })
        ));
    }

    /// `RtaStatus`, 24 bytes (rta.h:112-129).
    #[test]
    fn rta_status_fields_are_at_their_offsets() {
        let s = RtaStatus {
            state: 2,
            tap: 1,
            channel: 4,
            live_count: 3,
            live_mask: 0x0013,
            frames_per_s: 47,
            busy_us_per_s: 1234,
            last_frame_us: 900,
            idle_ms: 60,
            sample_rate_hz: 96_000,
            first_band: 0xFF,
            bass_busy_us_per_s: 65535,
        };
        let w = s.encode();
        assert_eq!(w.len(), 24);
        assert_eq!(&w[..6], &[3, 2, 1, 4, 0, 3]);
        assert_eq!(&w[6..8], &0x0013u16.to_le_bytes(), "live_mask");
        assert_eq!(&w[8..10], &47u16.to_le_bytes(), "frames_per_s");
        assert_eq!(&w[10..12], &1234u16.to_le_bytes(), "busy_us_per_s");
        assert_eq!(&w[12..14], &900u16.to_le_bytes(), "last_frame_us");
        assert_eq!(&w[14..16], &60u16.to_le_bytes(), "idle_ms");
        assert_eq!(&w[16..20], &96_000u32.to_le_bytes(), "sample_rate_hz");
        assert_eq!(w[20], 0xFF, "first_band");
        assert_eq!(&w[22..24], &65535u16.to_le_bytes(), "bass_busy_us_per_s");
        assert_eq!(RtaStatus::decode(&w).unwrap(), s);
    }

    // ------------------------------------------------ subharm and limiter

    /// One u16 per output, the count taken from the reply: 18 bytes on an
    /// RP2350, 10 on an RP2040 (config.h:200, vendor_commands.c:2206-2216).
    #[test]
    fn the_subharm_meter_is_sized_by_its_reply() {
        let rp2350: Vec<u8> = (0..9u16).flat_map(|k| (k * 1000).to_le_bytes()).collect();
        let m = SubharmMeter::decode(&rp2350).unwrap();
        assert_eq!(m.peaks.len(), 9);
        assert_eq!(m.peaks[3], 3000);
        assert_eq!(m.encode(), rp2350);
        let rp2040 = SubharmMeter::decode(&[0xFF, 0x7F, 0, 0, 0, 0, 0, 0, 0, 0]).unwrap();
        assert_eq!(rp2040.peaks.len(), 5);
        assert_eq!(rp2040.peak(0), 1.0, "32767 is full scale");
        assert_eq!(rp2040.peak(7), 0.0, "past the outputs reads silent");
        assert!(SubharmMeter::decode(&[1, 2, 3]).is_err());
    }

    /// Gain reduction in 0.01 dB per output (limiter.h:19,
    /// vendor_commands.c:2221-2229), and the 4-byte status (limiter.h:20,
    /// vendor_commands.c:2230-2237).
    #[test]
    fn the_limiter_meter_and_status_decode() {
        let d: Vec<u8> = [0u16, 150, 12000, 0, 0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let m = LimiterMeter::decode(&d).unwrap();
        assert_eq!(m.centi_db.len(), 5);
        assert_eq!(m.reduction_db(1), 1.5);
        assert_eq!(m.reduction_db(2), 120.0);
        assert_eq!(m.encode(), d);

        assert_eq!(LimiterStatus::SIZE, 4);
        let s = LimiterStatus::decode(&[1, 32, 16, 9]).unwrap();
        assert!(s.engaged);
        assert_eq!((s.lookahead, s.block, s.num_outputs), (32, 16, 9));
        assert_eq!(s.encode(), [1, 32, 16, 9]);
        assert!(LimiterStatus::decode(&[1, 32, 16]).is_err());
    }

    // -------------------------------------------------------- typed commands

    #[test]
    fn a_binding_is_built_from_named_fields() {
        let b = CsBinding::from_fields(&[
            ("type", "encoder"),
            ("noun", "user_volume"),
            ("action", "step"),
            ("gpio", "10,11"),
            ("step", "1"),
            ("flags", "accel"),
        ])
        .unwrap();
        assert_eq!(b.component, 4);
        assert_eq!(b.noun, 0);
        assert_eq!(b.action, 1);
        assert_eq!(b.gpio, [10, 11]);
        assert_eq!(b.step, 1);
        assert_eq!(b.flags, 0x08);
    }

    /// One GPIO means the second is explicitly unused, which is what the
    /// firmware expects of a configured single-pin component.
    #[test]
    fn a_single_gpio_marks_the_second_unused() {
        let b = CsBinding::from_fields(&[("type", "button"), ("gpio", "16")]).unwrap();
        assert_eq!(b.gpio, [16, GPIO_UNUSED]);
    }

    #[test]
    fn flags_accept_several_names_and_a_raw_mask() {
        let named = CsBinding::from_fields(&[("flags", "wrap,accel")]).unwrap();
        assert_eq!(named.flags, 0x0C);
        let raw = CsBinding::from_fields(&[("flags", "0x0C")]).unwrap();
        assert_eq!(raw.flags, 0x0C);
        assert_eq!(format_flag_set(CS_BINDING_FLAGS, 0x0C), "wrap,accel");
    }

    /// A bit this table has no name for still has to survive being read and
    /// written back, or a firmware that adds one loses it on every edit.
    #[test]
    fn an_unnamed_flag_bit_round_trips_as_hex() {
        // A macro step may carry only WRAP and GROUP, so 0x01 has no name here.
        assert_eq!(format_flag_set(CS_STEP_FLAGS, 0x24), "wrap,group");
        assert_eq!(format_flag_set(CS_STEP_FLAGS, 0x21), "group,0x01");
        let s = CsMacroStep::from_fields(&[("flags", "0x21")]).unwrap();
        assert_eq!(s.flags, 0x21);
    }

    #[test]
    fn an_unknown_field_names_the_ones_that_would_work() {
        let e = CsBinding::from_fields(&[("pin", "10")]).unwrap_err();
        let msg = e.to_string();
        assert!(msg.contains("`pin`"), "{msg}");
        assert!(msg.contains("gpio"), "the valid names are listed: {msg}");
    }

    #[test]
    fn a_bad_choice_lists_the_alternatives() {
        let e = CsBinding::from_fields(&[("type", "banana")]).unwrap_err();
        let msg = e.to_string();
        assert!(msg.contains("encoder"), "{msg}");
    }

    /// A number always works, because the device's noun table may be longer
    /// than the one this build was written against.
    #[test]
    fn a_noun_may_be_given_as_a_number() {
        let b = CsBinding::from_fields(&[("noun", "56")]).unwrap();
        assert_eq!(b.noun, 56);
    }

    #[test]
    fn a_crosspoint_is_built_from_its_indices_and_fields() {
        let r = MatrixRoutePacket::from_fields(&[
            ("input", "0"),
            ("output", "4"),
            ("enabled", "on"),
            ("gain", "-3"),
            ("invert", ""),
        ])
        .unwrap();
        assert_eq!(r.input, 0);
        assert_eq!(r.output, 4);
        assert!(r.enabled);
        assert_eq!(r.gain_db, -3.0);
        assert!(r.phase_invert);
    }

    /// The alignments and the two booleans share a byte, so writing the flags
    /// key must not clear an alignment set beside it.
    #[test]
    fn setting_display_flags_keeps_the_alignments() {
        let c = CsDisplayCfg::from_fields(&[
            ("label_align", "right"),
            ("flags", "edit_gated"),
            ("value_align", "centre"),
        ])
        .unwrap();
        assert_eq!(c.label_align(), 2);
        assert_eq!(c.value_align(), 1);
        assert_eq!(
            c.flags & display_flags::EDIT_GATED,
            display_flags::EDIT_GATED
        );
    }

    /// Every typeable packet must survive being rendered and typed back, or
    /// the echo line teaches a command that does something else.
    #[test]
    fn every_packet_spec_round_trips_through_its_own_text() {
        for spec in PACKET_SPECS {
            let sample: Vec<(&str, &str)> = match spec.path {
                "cs.binding" => vec![
                    ("type", "encoder"),
                    ("noun", "master_volume"),
                    ("action", "step"),
                    ("gpio", "27,28"),
                    ("flags", "accel"),
                    ("step", "256"),
                ],
                "cs.ir" => vec![
                    ("noun", "user_volume"),
                    ("action", "inc"),
                    ("protocol", "nec"),
                    ("step", "256"),
                    ("code", "0x20DF40BF"),
                    ("flags", "repeat"),
                ],
                "cs.group" => vec![
                    ("kind", "output_ch"),
                    ("members", "0xA"),
                    ("name", "Fronts"),
                ],
                "cs.macro.step" => vec![
                    ("noun", "output_mute"),
                    ("action", "set"),
                    ("flags", "group"),
                    ("target", "2"),
                    ("value", "1"),
                    ("pre_delay", "150"),
                ],
                "cs.display" => vec![
                    ("mode", "cycle_selected"),
                    ("dwell", "50"),
                    ("overlay_hold", "20"),
                    ("flags", "edit_gated"),
                    ("label_align", "right"),
                    ("edit_timeout", "100"),
                ],
                "cs.display.page" => vec![
                    ("noun", "output_gain"),
                    ("target", "2"),
                    ("flags", "active,large"),
                ],
                "mix" => vec![
                    ("input", "0"),
                    ("output", "4"),
                    ("enabled", "on"),
                    ("gain", "-3"),
                    ("invert", "on"),
                ],
                "dev.dacmute" => vec![("enabled", "on"), ("pin", "11"), ("hold_ms", "5")],
                "dev.uart" => vec![("enabled", "on"), ("baud", "115200")],
                "dev.i2c" => vec![("enabled", "on"), ("address", "0x42")],
                "preset.startup" => vec![("mode", "specified"), ("slot", "3")],
                "sig.config" => vec![
                    ("type", "sweep-log"),
                    ("channels", "0x3"),
                    ("invert", "0x2"),
                    ("flags", "raw,walk"),
                    ("level", "-12.5"),
                    ("duration", "5000"),
                    ("repeat", "3"),
                    ("gap", "250"),
                    ("p1", "20"),
                    ("p2", "20000"),
                ],
                other => panic!("no sample for {other}"),
            };

            let bytes = (spec.encode)(&sample).expect(spec.path);
            let text = spec.format(&bytes).expect(spec.path);

            // Re-parse what was printed, adding back the fields the command
            // line carries as indices rather than as text.
            let mut pairs: Vec<(String, String)> = spec
                .index_fields
                .iter()
                .map(|name| {
                    let given = sample
                        .iter()
                        .find(|(k, _)| k == name)
                        .map(|(_, v)| (*v).to_string())
                        .unwrap_or_default();
                    ((*name).to_string(), given)
                })
                .collect();
            for token in text.split_whitespace() {
                let (k, v) = token.split_once('=').unwrap_or((token, ""));
                pairs.push((k.to_string(), v.trim_matches('"').to_string()));
            }
            let refs: Vec<(&str, &str)> = pairs
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();

            let again = (spec.encode)(&refs)
                .unwrap_or_else(|e| panic!("{}: `{text}` did not parse back: {e}", spec.path));
            assert_eq!(again, bytes, "{} did not survive `{text}`", spec.path);
        }
    }

    #[test]
    fn every_typeable_path_is_a_packet_row_in_the_registry() {
        for spec in PACKET_SPECS {
            let d = crate::registry::by_path(spec.path)
                .unwrap_or_else(|| panic!("{} is not in the registry", spec.path));
            assert!(
                matches!(d.kind, crate::registry::Kind::Packet),
                "{} is not a packet row",
                spec.path
            );
        }
        assert!(spec_for_path("vol.user").is_none());
    }

    /// Field help is what the completion menu shows, so every field needs one.
    #[test]
    fn every_field_has_a_name_and_a_help_line() {
        for spec in PACKET_SPECS {
            for f in spec.fields {
                assert!(!f.name.is_empty(), "{} has a nameless field", spec.path);
                assert!(!f.help.is_empty(), "{}.{} has no help", spec.path, f.name);
                assert!(
                    !f.help.contains('-') || !f.help.contains("--"),
                    "{}.{} uses a dash rule the house style forbids",
                    spec.path,
                    f.name
                );
            }
        }
    }

    #[test]
    fn the_noun_table_covers_every_noun_the_firmware_defines() {
        use crate::generated::cs;
        // CS_NOUN_COUNT is 79 at caps v20 (control_surfaces.h:251).
        assert_eq!(CS_NOUNS.len(), cs::CS_NOUN_COUNT as usize);
        assert_eq!(CS_NOUNS.len(), 79);
        for (i, (_, raw)) in CS_NOUNS.iter().enumerate() {
            assert_eq!(*raw as usize, i, "the noun table must stay in order");
        }
        assert_eq!(
            CS_TYPES.len(),
            cs::CS_TYPE_COUNT as usize,
            "CS_TYPE_COUNT is 11"
        );
        assert_eq!(CS_ACTIONS.len(), 12, "CS_ACT_COUNT is 12");
    }

    /// The hand tables are the header's enums, spelled in lower case: every
    /// `CS_NOUN_X = n` and `CS_TYPE_X = n` line of the vendored
    /// control_surfaces.h (:128-252) must appear as `("x", n)`, so a
    /// renumbered or misspelt entry fails here rather than on a device.
    #[test]
    fn the_noun_and_type_tables_match_the_vendored_header() {
        const HEADER: &str = include_str!("../firmware/control_surfaces.h");
        let lines = |prefix: &str| -> Vec<(String, u8)> {
            HEADER
                .lines()
                .filter_map(|l| {
                    let l = l.trim().strip_prefix(prefix)?;
                    let (name, rest) = l.split_once('=')?;
                    let n = rest.trim().split(',').next()?.trim().parse().ok()?;
                    Some((name.trim().to_lowercase(), n))
                })
                .collect()
        };
        let table = |t: &[(&str, u8)]| -> Vec<(String, u8)> {
            t.iter().map(|(n, v)| (n.to_string(), *v)).collect()
        };
        assert_eq!(lines("CS_NOUN_"), table(CS_NOUNS));
        assert_eq!(lines("CS_TYPE_"), table(CS_TYPES));
    }
}
