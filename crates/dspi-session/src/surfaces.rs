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
//!
//! # Every SET here is a deferred preview
//!
//! A control-surface write returns before the firmware has validated it: the
//! main loop applies it a tick later and reports the outcome through
//! `REQ_GET_CS_STATUS`, tagged with the slot it was for. Nothing reaches flash
//! until [`Surfaces::save`], and [`Surfaces::revert`] puts back what is stored.
//! So a write is not done when the transfer returns; it is done when
//! [`Surfaces::wait_applied`] sees the pending flag clear.

use std::time::Duration;

use dspi_proto::generated::opcodes as op;
use dspi_transport::{Result as TResult, Transport, TransportError};

use crate::probe::ControlSurfaceCaps;

/// The wire vocabulary, re-exported so a caller of this module does not have
/// to reach into `dspi-proto` for the records it hands back. The codecs
/// themselves live there, so there is exactly one decoder per record.
pub use dspi_proto::packets::{
    CsBinding, CsDisplayCfg, CsDisplayCfgReply, CsDisplayPage, CsDisplayStatus, CsExtStatusPacket,
    CsGroup, CsMacro, CsMacroHeaderWire, CsMacroStep, CsNounDesc, CsStatusPacket, GPIO_UNUSED,
    IrCommand, IrLearnResult, PacketError,
};

/// The names this app has always used for three of them.
pub type Binding = CsBinding;
pub type NounCaps = CsNounDesc;
pub type Status = CsStatusPacket;
pub type LearnResult = IrLearnResult;

/// Status codes a deferred apply reports (control_surfaces.h:607-637).
pub mod status {
    pub const SUCCESS: u8 = 0x00;
    /// Accepted, the apply has not run yet. Poll again.
    pub const PENDING: u8 = 0x16;
    /// A previous SET is still queued. Back off and retry.
    pub const BUSY: u8 = 0x1B;
}

/// What `last_slot` in the status packet is tagged with
/// (control_surfaces.h:592-596, :767-782).
///
/// One poll has to say which kind of SET just landed, so each kind claims its
/// own high bits. The display's config and its page 0 deliberately share
/// `0x50`, which is only unambiguous because a host never has two display SETs
/// in flight; this module writes them one at a time for that reason.
pub mod tag {
    pub const IR: u8 = 0x80;
    pub const GROUP: u8 = 0x40;
    pub const MACRO: u8 = 0x60;
    pub const DISPLAY: u8 = 0x50;
    pub const SAVE: u8 = 0xFF;
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

#[derive(Debug, thiserror::Error)]
pub enum SurfaceError {
    #[error(transparent)]
    Transport(#[from] TransportError),

    #[error("could not read the device's answer: {0}")]
    Packet(#[from] PacketError),

    #[error("the device is still applying {tag} after {polls} checks")]
    Timeout { tag: String, polls: usize },
}

pub type Result<T> = std::result::Result<T, SurfaceError>;

/// A status code, as a sentence rather than a number to look up.
pub fn explain_status(code: u8) -> String {
    match code {
        0x00 => "applied".into(),
        0x01 => "that GPIO is not allowed".into(),
        0x02 => "that GPIO is already in use".into(),
        0x03 => "that output or slot does not exist".into(),
        0x04 => "disable the output first".into(),
        // `PIN_CONFIG_INVALID_PARAM`, config.h:612: a non-pin field out
        // of range. A display record with a bad I2C address or
        // brightness reaches it (control_surfaces.h:607).
        0x05 => "that setting is out of range".into(),
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
        // Caps v9 and v10 additions, control_surfaces.h:629-637.
        0x1F => "that group is empty, out of range, or the wrong kind for this parameter".into(),
        0x20 => "no such macro, or its step count is wrong".into(),
        0x21 => "that macro step is not a valid action".into(),
        0x22 => "another slot already holds the display".into(),
        0x23 => "those two pins are not a valid I2C pair".into(),
        0x24 => "the I2C control interface already has that instance".into(),
        0x25 => "that display page or setting is not valid".into(),
        other => format!("refused with status 0x{other:02X}"),
    }
}

/// Which write a `last_slot` tag refers to, in words.
///
/// Without this a status packet reports "slot 0x86" for what is really IR
/// sub-slot 6, and the four namespaces are indistinguishable.
pub fn explain_slot_tag(slot: u8) -> String {
    match slot {
        tag::SAVE => "the save".into(),
        0x80..=0x8F => format!("IR command {}", slot & 0x0F),
        0x60..=0x67 => format!("macro {}", slot & 0x07),
        // The config and page 0 share this byte; the config is the likelier
        // reading, and this module never has both in flight at once.
        tag::DISPLAY => "the display settings".into(),
        0x51..=0x5F => format!("display page {}", slot & 0x0F),
        0x40..=0x47 => format!("group {}", slot & 0x07),
        0x00..=0x0F => format!("binding slot {slot}"),
        other => format!("slot 0x{other:02X}"),
    }
}

/// What this device says it has, so nothing here is sized by a constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_bindings: u8,
    pub max_ir_commands: u8,
    pub max_groups: u8,
    pub max_macros: u8,
    pub max_macro_steps: u8,
    pub noun_count: u8,
    /// Not in the caps header: it arrives in the display config reply, so it
    /// starts at the header's `CS_MAX_DISPLAY_PAGES` and is refreshed by
    /// [`Surfaces::read_display_cfg`].
    pub max_display_pages: u8,
}

impl Default for Limits {
    /// The v1.1.6 constants, for a device that has not been asked yet
    /// (control_surfaces.h:272-283, :329).
    fn default() -> Self {
        Self {
            max_bindings: 16,
            max_ir_commands: 16,
            max_groups: 8,
            max_macros: 8,
            max_macro_steps: 8,
            noun_count: 57,
            max_display_pages: 16,
        }
    }
}

impl From<&ControlSurfaceCaps> for Limits {
    fn from(c: &ControlSurfaceCaps) -> Self {
        Self {
            max_bindings: c.max_bindings,
            max_ir_commands: c.max_ir_commands,
            max_groups: c.max_groups,
            max_macros: c.max_macros,
            max_macro_steps: c.max_macro_steps,
            noun_count: c.noun_count,
            ..Self::default()
        }
    }
}

/// The control-surface write path, bound to one transport.
///
/// Holds the tag of the write in flight so [`wait_applied`](Self::wait_applied)
/// can tell this write's outcome from someone else's: a knob turned on the
/// device also lands in `last_status`.
pub struct Surfaces<'t> {
    t: &'t mut dyn Transport,
    limits: Limits,
    /// The `last_slot` tag the write in flight will report under.
    pending: Option<u8>,
    poll_interval: Duration,
    busy_backoff: Duration,
    max_polls: usize,
}

impl<'t> Surfaces<'t> {
    /// The Console's budget: 25 polls at 20 ms.
    const DEFAULT_POLLS: usize = 25;

    pub fn new(t: &'t mut dyn Transport, limits: Limits) -> Self {
        Self {
            t,
            limits,
            pending: None,
            poll_interval: Duration::from_millis(20),
            busy_backoff: Duration::from_millis(60),
            max_polls: Self::DEFAULT_POLLS,
        }
    }

    /// Shorten the wait, for tests that have no real main loop to wait for.
    pub fn with_poll(mut self, interval: Duration, max_polls: usize) -> Self {
        self.poll_interval = interval;
        self.busy_backoff = interval;
        self.max_polls = max_polls;
        self
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }

    // ------------------------------------------------------------- the wait

    fn expect(&mut self, slot: u8) {
        self.pending = Some(slot);
    }

    /// Poll `REQ_GET_CS_STATUS` until the deferred apply resolves.
    ///
    /// `PENDING` means the main loop has not run it yet; `BUSY` means an
    /// earlier SET is still queued ahead of it, which clears on its own. Both
    /// are waited out. Giving up names the slot, because "timed out" without
    /// saying which write is not something a user can act on.
    pub fn wait_applied(&mut self) -> Result<CsStatusPacket> {
        for _ in 0..self.max_polls {
            std::thread::sleep(self.poll_interval);
            let st = self.read_status()?;

            let mine = self.pending.is_none_or(|tag| st.last_slot == tag);
            if mine && st.last_status != status::PENDING {
                if st.last_status == status::BUSY {
                    std::thread::sleep(self.busy_backoff);
                    continue;
                }
                self.pending = None;
                return Ok(st);
            }
        }
        Err(SurfaceError::Timeout {
            tag: explain_slot_tag(self.pending.take().unwrap_or(0)),
            polls: self.max_polls,
        })
    }

    // ------------------------------------------------------------- bindings

    /// Apply one binding. `CS_TYPE_NONE` clears the slot.
    pub fn write_binding(&mut self, slot: u8, b: &CsBinding) -> Result<CsStatusPacket> {
        self.t
            .control_out(op::REQ_SET_CS_BINDING, slot as u16, &b.encode())?;
        self.expect(slot);
        self.wait_applied()
    }

    /// Set a slot's label. A single NUL clears it.
    ///
    /// An empty payload is `INVALID_VALUE`, so clearing means sending one zero
    /// byte rather than nothing (control_surfaces.h:350-357).
    pub fn write_name(&mut self, slot: u8, name: &str) -> Result<CsStatusPacket> {
        // Truncated to 31 bytes plus the implicit terminator, as the firmware
        // does on its side. Truncation is on a character boundary so a
        // multi-byte name never becomes invalid UTF-8 on the device.
        let mut bytes: Vec<u8> = Vec::new();
        for c in name.chars() {
            if bytes.len() + c.len_utf8() > 31 {
                break;
            }
            let mut buf = [0u8; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
        if bytes.is_empty() {
            bytes.push(0);
        }
        self.t
            .control_out(op::REQ_SET_CS_NAME, slot as u16, &bytes)?;
        self.expect(slot);
        self.wait_applied()
    }

    /// Apply one IR sub-slot. An all-zero record clears it.
    pub fn write_ir_command(&mut self, sub_slot: u8, c: &IrCommand) -> Result<CsStatusPacket> {
        self.t
            .control_out(op::REQ_SET_CS_IR_CMD, sub_slot as u16, &c.encode())?;
        self.expect(tag::IR | sub_slot);
        self.wait_applied()
    }

    // --------------------------------------------------------------- groups

    /// Apply one group. An all-zero record clears it.
    ///
    /// This re-validates every binding that references a group, so a binding
    /// whose group just emptied goes down with its reason in `slot_status`
    /// rather than the write being refused (control_surfaces.h:689-694).
    pub fn write_group(&mut self, index: u8, g: &CsGroup) -> Result<CsStatusPacket> {
        self.t
            .control_out(op::REQ_SET_CS_GROUP, index as u16, &g.encode())?;
        self.expect(tag::GROUP | index);
        self.wait_applied()
    }

    // --------------------------------------------------------------- macros

    /// Write a whole macro: every step first, then the header.
    ///
    /// That order is the firmware's own instruction ("hosts should write steps
    /// first and the header last so a concurrent fire never sees `step_count`
    /// exceed the written steps", control_surfaces.h:488-491). The header is
    /// what makes the steps live, so writing it first would leave a window in
    /// which firing the macro runs steps that are still the old ones.
    pub fn write_macro(
        &mut self,
        index: u8,
        header: &CsMacroHeaderWire,
        steps: &[CsMacroStep],
    ) -> Result<CsStatusPacket> {
        for (i, step) in steps.iter().enumerate() {
            self.write_macro_step(index, i as u8, step)?;
        }
        self.t
            .control_out(op::REQ_SET_CS_MACRO, index as u16, &header.encode())?;
        self.expect(tag::MACRO | index);
        self.wait_applied()
    }

    /// Apply one step. `wValue` packs the step above the macro index.
    pub fn write_macro_step(
        &mut self,
        index: u8,
        step: u8,
        value: &CsMacroStep,
    ) -> Result<CsStatusPacket> {
        let wvalue = ((step as u16) << 8) | index as u16;
        self.t
            .control_out(op::REQ_SET_CS_MACRO_STEP, wvalue, &value.encode())?;
        self.expect(tag::MACRO | index);
        self.wait_applied()
    }

    /// Clear one step: an all-zero record is the empty step the sequencer
    /// skips.
    pub fn clear_macro_step(&mut self, index: u8, step: u8) -> Result<CsStatusPacket> {
        self.write_macro_step(index, step, &CsMacroStep::default())
    }

    /// Run a macro now, returning the accept status.
    ///
    /// Not deferred, unlike every other control-surface write: the reply is the
    /// accept or reject, and the sequence then runs on the device. Firing while
    /// another macro runs cancels that one at its next step boundary.
    pub fn fire_macro(&mut self, index: u8) -> Result<u8> {
        let d = self.t.control_in(op::REQ_CS_MACRO_FIRE, index as u16, 1)?;
        Ok(d.first().copied().unwrap_or(status::SUCCESS))
    }

    /// Cancel the running macro. `wValue` 0xFFFF is the cancel selector
    /// (config.h:141).
    pub fn cancel_macro(&mut self) -> Result<u8> {
        let d = self.t.control_in(op::REQ_CS_MACRO_FIRE, 0xFFFF, 1)?;
        Ok(d.first().copied().unwrap_or(status::SUCCESS))
    }

    // -------------------------------------------------------------- display

    pub fn write_display_cfg(&mut self, cfg: &CsDisplayCfg) -> Result<CsStatusPacket> {
        self.t
            .control_out(op::REQ_SET_CS_DISPLAY_CFG, 0, &cfg.encode())?;
        self.expect(tag::DISPLAY);
        self.wait_applied()
    }

    /// Apply one page. An all-zero record clears the slot.
    pub fn write_display_page(&mut self, page: u8, p: &CsDisplayPage) -> Result<CsStatusPacket> {
        self.t
            .control_out(op::REQ_SET_CS_DISPLAY_PAGE, page as u16, &p.encode())?;
        self.expect(tag::DISPLAY | page);
        self.wait_applied()
    }

    // ---------------------------------------------------------------- reads

    pub fn read_status(&mut self) -> Result<CsStatusPacket> {
        Ok(read_status(
            self.t,
            self.limits.max_bindings,
            self.limits.max_ir_commands,
        )?)
    }

    pub fn read_binding(&mut self, slot: u8) -> Result<CsBinding> {
        let d = self
            .t
            .control_in(op::REQ_GET_CS_BINDING, slot as u16, CsBinding::SIZE as u16)?;
        Ok(CsBinding::decode(&d)?)
    }

    pub fn read_name(&mut self, slot: u8) -> Result<String> {
        Ok(read_name(self.t, slot)?)
    }

    pub fn read_ir_command(&mut self, sub_slot: u8) -> Result<IrCommand> {
        let d = self.t.control_in(
            op::REQ_GET_CS_IR_CMD,
            sub_slot as u16,
            IrCommand::SIZE as u16,
        )?;
        Ok(IrCommand::decode(&d)?)
    }

    pub fn read_group(&mut self, index: u8) -> Result<CsGroup> {
        let d = self
            .t
            .control_in(op::REQ_GET_CS_GROUP, index as u16, CsGroup::SIZE as u16)?;
        Ok(CsGroup::decode(&d)?)
    }

    /// Read a whole macro: the 132-byte record carries its steps, which the
    /// SET path can only write one at a time.
    pub fn read_macro(&mut self, index: u8) -> Result<CsMacro> {
        let d = self
            .t
            .control_in(op::REQ_GET_CS_MACRO, index as u16, CsMacro::SIZE as u16)?;
        Ok(CsMacro::decode(&d)?)
    }

    pub fn read_ext_status(&mut self) -> Result<CsExtStatusPacket> {
        let d = self
            .t
            .control_in(op::REQ_GET_CS_EXT_STATUS, 0, CsExtStatusPacket::SIZE as u16)?;
        Ok(CsExtStatusPacket::decode(&d)?)
    }

    /// Read the display config, with the limits the reply prepends.
    ///
    /// `max_pages` is only available here, so this also refreshes the limit the
    /// page loop uses.
    pub fn read_display_cfg(&mut self) -> Result<CsDisplayCfgReply> {
        let d = self.t.control_in(
            op::REQ_GET_CS_DISPLAY_CFG,
            0,
            CsDisplayCfgReply::SIZE as u16,
        )?;
        let reply = CsDisplayCfgReply::decode(&d)?;
        if reply.max_pages > 0 {
            self.limits.max_display_pages = reply.max_pages;
        }
        Ok(reply)
    }

    pub fn read_display_page(&mut self, page: u8) -> Result<CsDisplayPage> {
        let d = self.t.control_in(
            op::REQ_GET_CS_DISPLAY_PAGE,
            page as u16,
            CsDisplayPage::SIZE as u16,
        )?;
        Ok(CsDisplayPage::decode(&d)?)
    }

    pub fn read_display_status(&mut self) -> Result<CsDisplayStatus> {
        let d = self.t.control_in(
            op::REQ_GET_CS_DISPLAY_STATUS,
            0,
            CsDisplayStatus::SIZE as u16,
        )?;
        Ok(CsDisplayStatus::decode(&d)?)
    }

    // ------------------------------------------------------------ whole sets

    pub fn read_all_bindings(&mut self) -> Result<Vec<(CsBinding, String)>> {
        let mut out = Vec::with_capacity(self.limits.max_bindings as usize);
        for slot in 0..self.limits.max_bindings {
            let b = self.read_binding(slot)?;
            let name = self.read_name(slot)?;
            out.push((b, name));
        }
        Ok(out)
    }

    pub fn read_all_ir_commands(&mut self) -> Result<Vec<IrCommand>> {
        (0..self.limits.max_ir_commands)
            .map(|i| self.read_ir_command(i))
            .collect()
    }

    pub fn read_all_groups(&mut self) -> Result<Vec<CsGroup>> {
        (0..self.limits.max_groups)
            .map(|i| self.read_group(i))
            .collect()
    }

    pub fn read_all_macros(&mut self) -> Result<Vec<CsMacro>> {
        (0..self.limits.max_macros)
            .map(|i| self.read_macro(i))
            .collect()
    }

    /// Every page slot, sized by the config reply rather than by a constant.
    pub fn read_all_pages(&mut self) -> Result<Vec<CsDisplayPage>> {
        let pages = self.read_display_cfg()?.max_pages;
        (0..pages).map(|i| self.read_display_page(i)).collect()
    }

    /// One noun's descriptor, so a caller can tolerate a single stall rather
    /// than losing the whole table to it.
    pub fn read_noun(&mut self, noun: u8) -> Result<CsNounDesc> {
        Ok(read_noun_caps(self.t, noun)?)
    }

    /// Every noun descriptor. A noun whose `actions` mask is zero is not
    /// available on this platform, which is how the firmware reports a
    /// difference like ADAT on an RP2040; it must not be offered.
    pub fn read_all_nouns(&mut self) -> Result<Vec<CsNounDesc>> {
        (0..self.limits.noun_count)
            .map(|noun| Ok(read_noun_caps(self.t, noun)?))
            .collect()
    }

    // ---------------------------------------------------------- persistence

    /// Persist the previewed configuration.
    ///
    /// Bindings apply live but do not survive a power cycle until this runs,
    /// which is a real feature: an assignment can be tried and backed out of.
    pub fn save(&mut self) -> Result<CsStatusPacket> {
        self.t.control_in(op::REQ_CS_SAVE, 0, 1)?;
        self.expect(tag::SAVE);
        self.wait_applied()
    }

    /// Discard the preview and reload what is in flash.
    pub fn revert(&mut self) -> Result<CsStatusPacket> {
        self.t.control_in(op::REQ_CS_REVERT, 0, 1)?;
        self.expect(tag::SAVE);
        self.wait_applied()
    }

    // --------------------------------------------------------------- IR learn

    /// Arm the receiver, answering the device's own verdict.
    ///
    /// `control_surfaces.h:798`: the opcode "returns PIN_CONFIG_SUCCESS or
    /// CS_STATUS_NO_IR (arm without a live IR component)". Dropping that byte
    /// leaves a refused arm looking like a live one that never hears a button.
    pub fn arm_learn(&mut self) -> Result<u8> {
        Ok(arm_learn(self.t)?)
    }

    pub fn cancel_learn(&mut self) -> Result<u8> {
        Ok(cancel_learn(self.t)?)
    }

    pub fn read_learn(&mut self) -> Result<IrLearnResult> {
        Ok(read_learn(self.t)?)
    }
}

// ---------------------------------------------------------------------------
// Transport-level reads, for callers that hold a bare transport.
// ---------------------------------------------------------------------------

/// Read one binding slot.
pub fn read_binding(t: &mut dyn Transport, slot: u8) -> TResult<Binding> {
    let d = t.control_in(op::REQ_GET_CS_BINDING, slot as u16, Binding::SIZE as u16)?;
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
        IrCommand::SIZE as u16,
    )?;
    Ok(IrCommand::decode(&d).unwrap_or_default())
}

/// Read one noun's descriptor from the device's capability table.
pub fn read_noun_caps(t: &mut dyn Transport, noun: u8) -> TResult<NounCaps> {
    let d = t.control_in(op::REQ_GET_CS_CAPS, noun as u16, NounCaps::SIZE as u16)?;
    Ok(NounCaps::decode(&d).unwrap_or_default())
}

/// Read `CsStatusPacket`, whose length follows the caps header.
///
/// 41 bytes on v1.1.6 (control_surfaces.h:594-605). It was 22 before caps v6,
/// when `ir_active_mask` was one byte and there were eight IR sub-slots, so
/// both counts must come from the caps header rather than from constants.
pub fn read_status(t: &mut dyn Transport, max_bindings: u8, max_ir: u8) -> TResult<Status> {
    let len = Status::wire_len(max_bindings, max_ir);
    let d = t.control_in(op::REQ_GET_CS_STATUS, 0, len as u16)?;
    Status::decode_sized(&d, max_bindings, max_ir).map_err(|e| TransportError::Usb(e.to_string()))
}

/// Start listening for a remote button, answering `PIN_CONFIG_SUCCESS` or
/// `CS_STATUS_NO_IR` (control_surfaces.h:798).
pub fn arm_learn(t: &mut dyn Transport) -> TResult<u8> {
    Ok(t.control_in(op::REQ_CS_IR_LEARN, learn::ARM, 1)?
        .first()
        .copied()
        .unwrap_or(status::SUCCESS))
}

pub fn cancel_learn(t: &mut dyn Transport) -> TResult<u8> {
    Ok(t.control_in(op::REQ_CS_IR_LEARN, learn::CANCEL, 1)?
        .first()
        .copied()
        .unwrap_or(status::SUCCESS))
}

/// Poll for a learn result.
///
/// The opcode is three-valued, which is easy to miss: arm, cancel, and **read**.
/// The read returns eight bytes carrying the state, the decoded protocol and the
/// code. Poll it until the state leaves `ARMED`.
pub fn read_learn(t: &mut dyn Transport) -> TResult<LearnResult> {
    let d = t.control_in(op::REQ_CS_IR_LEARN, learn::READ, LearnResult::SIZE as u16)?;
    Ok(LearnResult::decode(&d).unwrap_or_default())
}

/// Persist the previewed configuration.
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
    use dspi_transport::mock::{Direction, Reply};

    /// The v1.1.6 status packet, answering for one slot tag and code.
    fn status_bytes(last_slot: u8, last_status: u8) -> Vec<u8> {
        let mut d = vec![last_status, last_slot, 16, 1, 0x01, 0x00];
        d.extend([0u8; 16]);
        d.extend(0u16.to_le_bytes());
        d.push(learn::IDLE);
        d.extend([0u8; 16]);
        assert_eq!(d.len(), 41);
        d
    }

    fn surfaces(t: &mut MockTransport) -> Surfaces<'_> {
        Surfaces::new(t, Limits::default()).with_poll(Duration::ZERO, 6)
    }

    #[test]
    fn a_binding_round_trips_through_the_wire() {
        let b = Binding {
            component: 4,
            noun: 0,
            action: 1,
            gpio: [27, 28],
            step: 256,
            ..Default::default()
        };
        let bytes = b.encode();
        assert_eq!(bytes.len(), Binding::SIZE);
        assert_eq!(Binding::decode(&bytes).unwrap(), b);
    }

    #[test]
    fn a_short_read_decodes_to_a_default_rather_than_garbage() {
        let mut t = MockTransport::new().data(op::REQ_GET_CS_BINDING, vec![0u8; 24]);
        assert!(read_binding(&mut t, 0).unwrap().is_empty());
    }

    // ------------------------------------------------------ the deferred wait

    /// The whole point of the write path: a SET returns before the firmware
    /// has run it, so the outcome only exists after the pending flag clears.
    #[test]
    fn a_write_waits_for_pending_to_clear() {
        let mut t = MockTransport::new()
            .data(op::REQ_SET_CS_BINDING, vec![])
            .reply(
                op::REQ_GET_CS_STATUS,
                // Pending once, then the real answer.
                Reply::Sequence(vec![
                    Reply::Data(status_bytes(3, status::PENDING)),
                    Reply::Data(status_bytes(3, status::SUCCESS)),
                ]),
            );

        let st = {
            let mut s = surfaces(&mut t);
            s.write_binding(
                3,
                &Binding {
                    component: 1,
                    gpio: [16, GPIO_UNUSED],
                    ..Default::default()
                },
            )
            .unwrap()
        };
        assert_eq!(st.last_status, status::SUCCESS);
        assert_eq!(st.last_slot, 3);

        // The wire: one OUT with the 24-byte record, then two status polls.
        let log = t.log();
        assert_eq!(log[0].direction, Direction::Out);
        assert_eq!(log[0].opcode, op::REQ_SET_CS_BINDING);
        assert_eq!(log[0].value, 3, "the slot rides in wValue");
        assert_eq!(log[0].payload.len(), 24);
        assert_eq!(log[0].payload[0], 1, "type");
        assert_eq!(log[0].payload[4], 16, "gpio[0]");
        assert_eq!(log[0].payload[5], GPIO_UNUSED, "gpio[1]");
        assert_eq!(log[1].opcode, op::REQ_GET_CS_STATUS);
        assert_eq!(log[2].opcode, op::REQ_GET_CS_STATUS);
        assert_eq!(log.len(), 3);
    }

    /// BUSY means an earlier SET is still queued ahead of this one. It clears
    /// on its own, so it is waited out rather than reported as a failure.
    #[test]
    fn a_busy_device_is_waited_out() {
        let mut t = MockTransport::new()
            .data(op::REQ_SET_CS_GROUP, vec![])
            .reply(
                op::REQ_GET_CS_STATUS,
                Reply::Sequence(vec![
                    Reply::Data(status_bytes(tag::GROUP | 2, status::BUSY)),
                    Reply::Data(status_bytes(tag::GROUP | 2, status::SUCCESS)),
                ]),
            );

        let st = {
            let mut s = surfaces(&mut t);
            s.write_group(
                2,
                &CsGroup {
                    target_kind: 2,
                    member_mask: 0b1010,
                    name: "Front Pair".into(),
                },
            )
            .unwrap()
        };
        assert_eq!(st.last_status, status::SUCCESS);
        assert_eq!(t.log()[0].payload.len(), 40);
        assert_eq!(t.log()[0].payload[0], 2, "target_kind");
        assert_eq!(
            &t.log()[0].payload[4..8],
            &[0x0A, 0, 0, 0],
            "member_mask LE"
        );
    }

    /// A knob turned on the device also lands in `last_status`. Without the
    /// tag check the host would read someone else's result as its own.
    #[test]
    fn another_slots_result_is_not_mistaken_for_this_one() {
        let mut t = MockTransport::new()
            .data(op::REQ_SET_CS_BINDING, vec![])
            .reply(
                op::REQ_GET_CS_STATUS,
                Reply::Sequence(vec![
                    Reply::Data(
                        // Slot 7's outcome, from a write this host did not make.
                        status_bytes(7, status::SUCCESS),
                    ),
                    Reply::Data(status_bytes(3, status::SUCCESS)),
                ]),
            );

        let st = {
            let mut s = surfaces(&mut t);
            s.write_binding(3, &Binding::default()).unwrap()
        };
        assert_eq!(st.last_slot, 3, "waited for slot 3, not slot 7");
    }

    /// Giving up has to say which write is stuck; "timed out" alone is not
    /// something a user can act on.
    #[test]
    fn giving_up_names_the_slot() {
        let mut t = MockTransport::new()
            .data(op::REQ_SET_CS_IR_CMD, vec![])
            .data(
                op::REQ_GET_CS_STATUS,
                status_bytes(tag::IR | 6, status::PENDING),
            );

        let e = {
            let mut s = surfaces(&mut t);
            s.write_ir_command(6, &IrCommand::default()).unwrap_err()
        };
        let msg = e.to_string();
        assert!(msg.contains("IR command 6"), "{msg}");
        assert!(matches!(e, SurfaceError::Timeout { .. }));
    }

    #[test]
    fn every_slot_namespace_explains_itself() {
        assert_eq!(explain_slot_tag(0), "binding slot 0");
        assert_eq!(explain_slot_tag(15), "binding slot 15");
        assert_eq!(explain_slot_tag(tag::IR | 6), "IR command 6");
        assert_eq!(explain_slot_tag(tag::GROUP | 3), "group 3");
        assert_eq!(explain_slot_tag(tag::MACRO | 5), "macro 5");
        assert_eq!(explain_slot_tag(tag::DISPLAY), "the display settings");
        assert_eq!(explain_slot_tag(tag::DISPLAY | 4), "display page 4");
        assert_eq!(explain_slot_tag(tag::SAVE), "the save");
    }

    /// The four namespaces have to stay disjoint, or one poll of `last_slot`
    /// cannot say which kind of write just landed.
    #[test]
    fn the_slot_tags_do_not_collide() {
        let mut seen = std::collections::HashSet::new();
        for n in 0..16u8 {
            assert!(seen.insert(n), "binding {n}");
            assert!(seen.insert(tag::IR | n), "ir {n}");
        }
        for n in 0..8u8 {
            assert!(seen.insert(tag::GROUP | n), "group {n}");
            assert!(seen.insert(tag::MACRO | n), "macro {n}");
        }
        // The display's config and its page 0 deliberately share 0x50.
        for n in 1..16u8 {
            assert!(seen.insert(tag::DISPLAY | n), "page {n}");
        }
        assert!(seen.insert(tag::DISPLAY));
        assert!(seen.insert(tag::SAVE));
    }

    // ------------------------------------------------------------- the writes

    /// A single NUL clears a name: an empty payload is INVALID_VALUE.
    #[test]
    fn clearing_a_name_sends_one_nul() {
        let mut t = MockTransport::new()
            .data(op::REQ_SET_CS_NAME, vec![])
            .data(op::REQ_GET_CS_STATUS, status_bytes(2, status::SUCCESS));
        {
            let mut s = surfaces(&mut t);
            s.write_name(2, "").unwrap();
            s.write_name(2, "Sub Level").unwrap();
        }
        let log = t.log();
        assert_eq!(log[0].payload, vec![0u8]);
        assert_eq!(log[2].payload, b"Sub Level".to_vec());
    }

    #[test]
    fn a_long_name_is_truncated_so_the_terminator_survives() {
        let mut t = MockTransport::new()
            .data(op::REQ_SET_CS_NAME, vec![])
            .data(op::REQ_GET_CS_STATUS, status_bytes(0, status::SUCCESS));
        {
            let mut s = surfaces(&mut t);
            s.write_name(0, &"x".repeat(60)).unwrap();
        }
        assert_eq!(t.log()[0].payload.len(), 31);
    }

    /// Steps before the header, because the header is what makes them live: a
    /// macro fired mid-edit must never see a count reaching past the steps
    /// actually written.
    #[test]
    fn a_macro_writes_its_steps_before_its_header() {
        let mut t = MockTransport::new()
            .data(op::REQ_SET_CS_MACRO_STEP, vec![])
            .data(op::REQ_SET_CS_MACRO, vec![])
            .data(
                op::REQ_GET_CS_STATUS,
                status_bytes(tag::MACRO | 1, status::SUCCESS),
            );

        let steps = vec![
            CsMacroStep {
                noun: 7,
                action: 5,
                value: 1,
                ..Default::default()
            },
            CsMacroStep {
                noun: 6,
                action: 5,
                value: 2,
                pre_delay: 50,
                ..Default::default()
            },
        ];
        {
            let mut s = surfaces(&mut t);
            s.write_macro(
                1,
                &CsMacroHeaderWire {
                    name: "Night".into(),
                    step_count: 2,
                },
                &steps,
            )
            .unwrap();
        }

        let writes: Vec<_> = t
            .log()
            .into_iter()
            .filter(|e| e.direction == Direction::Out)
            .collect();
        assert_eq!(writes.len(), 3);
        assert_eq!(writes[0].opcode, op::REQ_SET_CS_MACRO_STEP);
        assert_eq!(writes[0].value, 1, "step 0, macro 1");
        assert_eq!(writes[1].opcode, op::REQ_SET_CS_MACRO_STEP);
        assert_eq!(
            writes[1].value, 0x0101,
            "step 1 packs above the macro index"
        );
        assert_eq!(
            writes[2].opcode,
            op::REQ_SET_CS_MACRO,
            "the header goes last"
        );
        assert_eq!(writes[2].payload[32], 2, "step_count");
    }

    #[test]
    fn clearing_a_step_writes_an_all_zero_record() {
        let mut t = MockTransport::new()
            .data(op::REQ_SET_CS_MACRO_STEP, vec![])
            .data(
                op::REQ_GET_CS_STATUS,
                status_bytes(tag::MACRO, status::SUCCESS),
            );
        {
            let mut s = surfaces(&mut t);
            s.clear_macro_step(0, 3).unwrap();
        }
        assert_eq!(t.log()[0].payload, vec![0u8; 12]);
        assert_eq!(t.log()[0].value, 0x0300);
    }

    /// Firing is immediate, not deferred: the reply is the accept status.
    #[test]
    fn firing_a_macro_answers_at_once() {
        let mut t = MockTransport::new().data(op::REQ_CS_MACRO_FIRE, vec![status::SUCCESS]);
        let (fired, cancelled) = {
            let mut s = surfaces(&mut t);
            (s.fire_macro(2).unwrap(), s.cancel_macro().unwrap())
        };
        assert_eq!(fired, status::SUCCESS);
        assert_eq!(cancelled, status::SUCCESS);
        let log = t.log();
        assert_eq!(log[0].value, 2);
        assert_eq!(log[1].value, 0xFFFF, "0xFFFF cancels");
        assert!(log.iter().all(|e| e.direction == Direction::In));
    }

    #[test]
    fn a_display_page_write_is_tagged_with_its_page() {
        let mut t = MockTransport::new()
            .data(op::REQ_SET_CS_DISPLAY_PAGE, vec![])
            .data(
                op::REQ_GET_CS_STATUS,
                status_bytes(tag::DISPLAY | 4, status::SUCCESS),
            );
        {
            let mut s = surfaces(&mut t);
            s.write_display_page(
                4,
                &CsDisplayPage {
                    noun: 17,
                    target: 2,
                    index: 0,
                    flags: 0x05,
                },
            )
            .unwrap();
        }
        assert_eq!(t.log()[0].value, 4);
        assert_eq!(t.log()[0].payload, vec![17, 2, 0, 0x05]);
    }

    #[test]
    fn the_display_config_write_is_twelve_bytes() {
        let mut t = MockTransport::new()
            .data(op::REQ_SET_CS_DISPLAY_CFG, vec![])
            .data(
                op::REQ_GET_CS_STATUS,
                status_bytes(tag::DISPLAY, status::SUCCESS),
            );
        {
            let mut s = surfaces(&mut t);
            s.write_display_cfg(&CsDisplayCfg {
                mode: 1,
                home_page: 2,
                dwell: 50,
                overlay_hold: 20,
                brightness: 128,
                flags: 0x03,
                edit_timeout: 100,
            })
            .unwrap();
        }
        assert_eq!(t.log()[0].value, 0);
        assert_eq!(
            t.log()[0].payload,
            vec![
                0x01, 0x02, 0x32, 0x00, 0x14, 0x00, 0x80, 0x03, 0x64, 0x00, 0x00, 0x00
            ]
        );
    }

    /// Save and revert are write-as-read: they mutate while travelling on the
    /// IN path, and report under the 0xFF tag.
    #[test]
    fn save_and_revert_wait_under_the_save_tag() {
        let mut t = MockTransport::new()
            .data(op::REQ_CS_SAVE, vec![0])
            .data(op::REQ_CS_REVERT, vec![0])
            .data(
                op::REQ_GET_CS_STATUS,
                status_bytes(tag::SAVE, status::SUCCESS),
            );
        {
            let mut s = surfaces(&mut t);
            assert_eq!(s.save().unwrap().last_slot, tag::SAVE);
            assert_eq!(s.revert().unwrap().last_slot, tag::SAVE);
        }
        assert!(t.log().iter().all(|e| e.direction == Direction::In));
    }

    // -------------------------------------------------------------- the reads

    /// Written, then read back: a control-surface write pushes no
    /// notification, so a re-read is the only confirmation there is.
    #[test]
    fn a_binding_reads_back_after_it_is_written() {
        let stored = Binding {
            component: 4,
            noun: 1,
            action: 1,
            flags: 0x08,
            gpio: [27, 28],
            step: 256,
            ..Default::default()
        };
        let mut t = MockTransport::new()
            .data(op::REQ_SET_CS_BINDING, vec![])
            .data(op::REQ_GET_CS_STATUS, status_bytes(0, status::SUCCESS))
            .data(op::REQ_GET_CS_BINDING, stored.encode().to_vec());

        let back = {
            let mut s = surfaces(&mut t);
            s.write_binding(0, &stored).unwrap();
            s.read_binding(0).unwrap()
        };
        assert_eq!(back, stored);
    }

    #[test]
    fn the_page_loop_is_sized_by_the_config_reply() {
        let reply = CsDisplayCfgReply {
            max_pages: 4,
            model_count: 9,
            cfg: CsDisplayCfg {
                mode: 1,
                dwell: 30,
                overlay_hold: 20,
                edit_timeout: 100,
                ..Default::default()
            },
        };
        let mut t = MockTransport::new()
            .data(op::REQ_GET_CS_DISPLAY_CFG, reply.encode().to_vec())
            .data(op::REQ_GET_CS_DISPLAY_PAGE, vec![0u8; 4]);

        let pages = {
            let mut s = surfaces(&mut t);
            s.read_all_pages().unwrap()
        };
        assert_eq!(pages.len(), 4, "not the header's 16");
    }

    #[test]
    fn the_read_loops_are_sized_by_the_caps_header() {
        let caps = ControlSurfaceCaps {
            caps_version: 13,
            max_bindings: 2,
            type_count: 9,
            noun_count: 3,
            max_ir_commands: 2,
            max_groups: 1,
            max_macros: 1,
            max_macro_steps: 8,
            max_pages: 16,
            display_models: 8,
            types: Vec::new(),
        };
        let mut t = MockTransport::new()
            .data(op::REQ_GET_CS_BINDING, vec![0u8; 24])
            .data(op::REQ_GET_CS_NAME, vec![0u8; 32])
            .data(op::REQ_GET_CS_IR_CMD, vec![0u8; 16])
            .data(op::REQ_GET_CS_GROUP, vec![0u8; 40])
            .data(op::REQ_GET_CS_MACRO, vec![0u8; 132])
            .data(op::REQ_GET_CS_CAPS, vec![0u8; 12]);

        let mut s = Surfaces::new(&mut t, Limits::from(&caps));
        assert_eq!(s.read_all_bindings().unwrap().len(), 2);
        assert_eq!(s.read_all_ir_commands().unwrap().len(), 2);
        assert_eq!(s.read_all_groups().unwrap().len(), 1);
        assert_eq!(s.read_all_macros().unwrap().len(), 1);
        assert_eq!(s.read_all_nouns().unwrap().len(), 3);
    }

    /// A noun with no actions is unavailable on this platform, which is how
    /// the firmware reports a difference like ADAT on an RP2040.
    #[test]
    fn an_unavailable_noun_is_visible_as_such() {
        let mut t = MockTransport::new().data(op::REQ_GET_CS_CAPS, vec![0u8; 12]);
        let nouns = {
            let mut s = Surfaces::new(
                &mut t,
                Limits {
                    noun_count: 1,
                    ..Limits::default()
                },
            );
            s.read_all_nouns().unwrap()
        };
        assert!(!nouns[0].is_available());
    }

    #[test]
    fn the_extended_status_reports_the_running_macro() {
        let mut ext = vec![0u8; 24];
        ext[0] = 8;
        ext[1] = 8;
        ext[2] = 8;
        ext[3] = 3;
        let mut t = MockTransport::new().data(op::REQ_GET_CS_EXT_STATUS, ext);
        let st = {
            let mut s = surfaces(&mut t);
            s.read_ext_status().unwrap()
        };
        assert!(st.is_running());
        assert_eq!(st.macro_running, 3);
    }

    #[test]
    fn the_display_status_says_whether_the_panel_is_live() {
        let mut d = vec![0u8; 8];
        d[0] = 2; // live
        d[1] = 3; // page 3
        let mut t = MockTransport::new().data(op::REQ_GET_CS_DISPLAY_STATUS, d);
        let st = {
            let mut s = surfaces(&mut t);
            s.read_display_status().unwrap()
        };
        assert!(st.is_live());
        assert_eq!(st.current_page, 3);
    }

    // ------------------------------------------------------------- IR learn

    /// The learn opcode is three-valued: arm, cancel and read. Treating it as
    /// two would leave the result unreadable.
    #[test]
    fn the_learn_loop_arms_polls_and_reads_a_code() {
        let mut t = MockTransport::new().data(op::REQ_CS_IR_LEARN, {
            let mut v = vec![learn::DONE, 1, 0, 0];
            v.extend(0x20DF40BFu32.to_le_bytes());
            v
        });

        let r = {
            let mut s = surfaces(&mut t);
            s.arm_learn().unwrap();
            s.read_learn().unwrap()
        };
        assert_eq!(r.state, learn::DONE);
        assert_eq!(r.protocol, 1);
        assert_eq!(r.code, 0x20DF40BF);

        let values: Vec<u16> = t.log().iter().map(|e| e.value).collect();
        assert_eq!(values, vec![learn::ARM, learn::READ]);
    }

    /// `REQ_CS_IR_LEARN` answers a status byte, and a refused arm is the whole
    /// reason to read it: without it the page waits for a button on a receiver
    /// the device never armed (control_surfaces.h:798).
    #[test]
    fn arming_a_learn_reports_the_devices_verdict() {
        use crate::surfaces::status;
        let mut t = MockTransport::new().data(op::REQ_CS_IR_LEARN, vec![status::SUCCESS]);
        assert_eq!(surfaces(&mut t).arm_learn().unwrap(), status::SUCCESS);

        // CS_STATUS_NO_IR: arming with no live receiver.
        let mut t = MockTransport::new().data(op::REQ_CS_IR_LEARN, vec![0x1E]);
        assert_eq!(surfaces(&mut t).arm_learn().unwrap(), 0x1E);
        assert_eq!(explain_status(0x1E), "set up an IR receiver first");

        let mut t = MockTransport::new().data(op::REQ_CS_IR_LEARN, vec![status::SUCCESS]);
        assert_eq!(surfaces(&mut t).cancel_learn().unwrap(), status::SUCCESS);
    }

    #[test]
    fn a_learn_that_times_out_is_distinguishable_from_one_still_waiting() {
        assert_ne!(learn::TIMEOUT, learn::ARMED);
        assert_ne!(learn::DONE, learn::ARMED);
    }

    // --------------------------------------------------------------- status

    #[test]
    fn status_decodes_the_dirty_flag_and_per_slot_results() {
        let mut d = status_bytes(3, 0x02);
        d[6 + 3] = 0x1A;
        d[22..24].copy_from_slice(&0x8003u16.to_le_bytes());
        d[40] = 0x1E;

        let mut t = MockTransport::new().data(op::REQ_GET_CS_STATUS, d);
        let s = read_status(&mut t, 16, 16).unwrap();

        assert!(s.dirty, "unsaved changes must be visible");
        assert_eq!(s.max_bindings, 16);
        assert_eq!(s.slot_health(3), 0x1A);
        assert_eq!(s.ir_active_mask, 0x8003);
        assert!(
            s.is_ir_active(15),
            "sub-slot 15 has no bit in an 8-bit mask"
        );
        assert_eq!(s.ir_cmd_status[15], 0x1E);
    }

    /// Every status code needs a sentence; a bare number sends the user to a
    /// header file.
    #[test]
    fn every_status_code_explains_itself() {
        for code in [
            0x00u8, 0x02, 0x05, 0x13, 0x15, 0x1A, 0x1D, 0x1E, 0x1F, 0x20, 0x21, 0x22, 0x23, 0x24,
            0x25,
        ] {
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
        assert!(t.log().iter().all(|e| e.direction == Direction::In));
    }
}
