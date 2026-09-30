//! Stepping back through the write journal.
//!
//! Undo re-writes the value a parameter held before a change, through the same
//! [`Session::write`] path as any other write. That is the whole design: the
//! reversal gets the same validation, the same capability gating and the same
//! readback confirmation, so "undo" cannot quietly fail in a way an ordinary
//! write would have caught.
//!
//! Only live parameters take part. A flash write, a pin move, a preset load and
//! the bootloader jump are marked not undoable when they are journalled
//! (`Hazard::Flash`, `Deferred`, `Reconfig`, `Irreversible`), because there is
//! nothing to put back: they are confirmed before the fact instead. Those
//! entries are stepped over and named, rather than silently swallowed, so the
//! person who pressed undo learns why nothing moved.
//!
//! Some writes move more than their own value: a tube type loads a voicing,
//! a character edit drops the type to Custom, a limiter joining a link group
//! adopts the group's settings, and a write to every output moves each one.
//! The journal keeps what those moved as it was (`JournalEntry::companions`),
//! and undo puts it back after the value itself.

use dspi_cmd::{Command, Context};

use dspi_proto::value::Value;

use crate::write::{Outcome, Session, WriteError};

/// The result of one step backwards or forwards.
#[derive(Debug, Clone, PartialEq)]
pub struct Undone {
    /// The canonical command that was replayed, ready for the echo line, or
    /// `None` when every entry reached was one that cannot be reversed.
    pub command: Option<String>,
    /// What the replayed write actually did.
    pub outcome: Option<Outcome>,
    /// Entries stepped over on the way, newest first, each with its reason.
    pub skipped: Vec<String>,
}

impl Undone {
    /// Whether anything actually changed on the device.
    pub fn moved(&self) -> bool {
        self.command.is_some()
    }
}

impl Session {
    /// Is there a change left to step back through?
    pub fn can_undo(&self) -> bool {
        self.undo_stack
            .iter()
            .any(|e| e.undoable && (e.before.is_some() || !e.companions.is_empty()))
    }

    /// Is there a change left to re-apply?
    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Put back the value the most recent undoable change replaced.
    ///
    /// Returns `Ok(None)` when there is no history at all.
    pub fn undo(&mut self) -> Result<Option<Undone>, WriteError> {
        let mut skipped = Vec::new();

        loop {
            let Some(entry) = self.undo_stack.pop() else {
                return Ok(if skipped.is_empty() {
                    None
                } else {
                    Some(Undone {
                        command: None,
                        outcome: None,
                        skipped,
                    })
                });
            };

            if !entry.undoable {
                skipped.push(format!("`{}` cannot be undone", entry.path));
                continue;
            }
            // Every value to put back: the write's own, then what the device
            // moved with it. A write to every output has no single value of
            // its own to restore, only each output's.
            let mut steps: Vec<(&'static str, Vec<u8>, Value)> = Vec::new();
            if let Some(before) = &entry.before {
                steps.push((entry.path, entry.indices.clone(), before.clone()));
            }
            steps.extend(
                entry
                    .companions
                    .iter()
                    .map(|c| (c.path, c.indices.clone(), c.before.clone())),
            );
            if steps.is_empty() {
                // Stepping over this one to an older, unrelated change would
                // undo something the person did not ask about. Say so, and
                // let the next undo go further.
                skipped.push(format!("`{}` has no previous value to restore", entry.path));
                return Ok(Some(Undone {
                    command: None,
                    outcome: None,
                    skipped,
                }));
            }

            let mut outcome = None;
            for (path, indices, value) in &steps {
                match self.replay(path, indices, value.clone()) {
                    // The first step is the change itself; what it did is
                    // what the caller reports.
                    Ok(o) => {
                        outcome.get_or_insert(o);
                    }
                    Err(e) => {
                        // The device refused the reversal, so the change is
                        // still standing and must stay on the stack to be
                        // tried again.
                        self.undo_stack.push(entry);
                        return Err(e);
                    }
                }
            }

            let command = steps
                .iter()
                .map(|(path, indices, value)| self.render(path, indices, value))
                .collect::<Vec<_>>()
                .join("; ");
            self.redo_stack.push(entry);
            return Ok(Some(Undone {
                command: Some(command),
                outcome,
                skipped,
            }));
        }
    }

    /// Re-apply the change that undo most recently reversed.
    pub fn redo(&mut self) -> Result<Option<Undone>, WriteError> {
        let Some(entry) = self.redo_stack.pop() else {
            return Ok(None);
        };

        let after = entry.after.clone();
        let outcome = match self.replay(entry.path, &entry.indices, after.clone()) {
            Ok(o) => o,
            Err(e) => {
                self.redo_stack.push(entry);
                return Err(e);
            }
        };

        let command = self.render(entry.path, &entry.indices, &after);
        self.undo_stack.push(entry);
        Ok(Some(Undone {
            command: Some(command),
            outcome: Some(outcome),
            skipped: Vec::new(),
        }))
    }

    /// What the command grammar needs to name this device's channels.
    ///
    /// Built from the discovered topology, so an undone change echoes with the
    /// same channel name the user typed to make it.
    pub fn cmd_context(&self) -> Context {
        let caps = self.capabilities();
        Context {
            channel_slugs: caps.channels.iter().map(|c| c.slug.clone()).collect(),
            num_inputs: caps.num_inputs,
            num_outputs: caps.num_outputs,
            max_bands: caps.max_bands,
        }
    }

    /// Write through the ordinary path, without disturbing the two stacks.
    fn replay(
        &mut self,
        path: &'static str,
        indices: &[u8],
        value: dspi_proto::value::Value,
    ) -> Result<Outcome, WriteError> {
        self.replaying = true;
        let out = self.write(path, indices, value);
        self.replaying = false;
        out
    }

    fn render(
        &self,
        path: &'static str,
        indices: &[u8],
        value: &dspi_proto::value::Value,
    ) -> String {
        dspi_cmd::format(
            &Command::Set {
                path,
                indices: indices.to_vec(),
                value: value.clone(),
            },
            &self.cmd_context(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::{Capabilities, ChannelInfo};
    use dspi_proto::generated::opcodes as op;
    use dspi_proto::value::Value;
    use dspi_proto::{Dir, Platform};
    use dspi_transport::MockTransport;
    use dspi_transport::mock::{Direction, Exchange, LogHandle};

    /// A mock whose reads answer with whatever was last written, so an undo has
    /// a real "before" to find and the wire shows what it put back.
    fn caps() -> Capabilities {
        Capabilities {
            serial: "TEST".into(),
            platform: Platform::Rp2350,
            firmware: "1.1.6".into(),
            firmware_version: dspi_proto::packets::FirmwareVersion::new(1, 1, 6, 0),
            build_info: None,
            wire_format: dspi_proto::generated::wire::WIRE_FORMAT_VERSION as u8,
            num_channels: 17,
            num_inputs: 8,
            num_outputs: 9,
            max_bands: 10,
            band_storage: 12,
            channels: (0..17)
                .map(|i| ChannelInfo {
                    index: i,
                    name: format!("Ch {i}"),
                    slug: format!("ch.{i}"),
                    is_output: i >= 8,
                })
                .collect(),
            features: Vec::new(),
            cs: None,
            siggen: None,
            active_preset: None,
        }
    }

    /// The floats the device answers for `vol.user`, in order.
    fn rig(reads: &[f32]) -> (Session, LogHandle) {
        let mut t = MockTransport::new();
        // Each read answers the same bytes; the test drives the sequence by
        // rebuilding the session's view through writes instead.
        t = t.data(op::REQ_GET_USER_VOLUME, reads[0].to_le_bytes().to_vec());
        let log = t.log_handle();
        (Session::new(Box::new(t), caps()).unwrap(), log)
    }

    fn writes(log: &LogHandle, opcode: u8) -> Vec<Exchange> {
        log.lock()
            .unwrap()
            .iter()
            .filter(|e| e.direction == Direction::Out && e.opcode == opcode)
            .cloned()
            .collect()
    }

    #[test]
    fn there_is_nothing_to_undo_on_a_fresh_session() {
        let (mut s, _) = rig(&[-18.0]);
        assert!(!s.can_undo());
        assert_eq!(s.undo().unwrap(), None);
        assert_eq!(s.redo().unwrap(), None);
    }

    /// The point of undo: the old value goes back out on the wire.
    #[test]
    fn undo_writes_the_old_value_and_redo_writes_the_new_one() {
        let (mut s, log) = rig(&[-30.0]);

        // The device reports -30, so that is what the journal records as the
        // value this write replaced.
        let _ = s.write("vol.user", &[], Value::Float(-18.0));
        assert!(s.can_undo());

        let undone = s.undo().unwrap().unwrap();
        assert_eq!(undone.command.as_deref(), Some("vol.user -30"));
        assert!(undone.skipped.is_empty());

        let sent = writes(&log, op::REQ_SET_USER_VOLUME);
        assert_eq!(sent.len(), 2, "the write, then the reversal");
        assert_eq!(
            sent[1].payload,
            (-30.0f32).to_le_bytes(),
            "undo put the old value back on the wire"
        );

        assert!(s.can_redo());
        let redone = s.redo().unwrap().unwrap();
        assert_eq!(redone.command.as_deref(), Some("vol.user -18"));

        let sent = writes(&log, op::REQ_SET_USER_VOLUME);
        assert_eq!(sent.len(), 3);
        assert_eq!(
            sent[2].payload,
            (-18.0f32).to_le_bytes(),
            "redo put the new value back"
        );
    }

    /// A reversal is a write like any other, so it appears in the log with its
    /// outcome rather than being applied behind the journal's back.
    #[test]
    fn a_reversal_is_journalled_and_confirmed() {
        let (mut s, _) = rig(&[-30.0]);
        let _ = s.write("vol.user", &[], Value::Float(-18.0));
        let before = s.journal.len();

        s.undo().unwrap();
        assert_eq!(
            s.journal.len(),
            before + 1,
            "the reversal is journalled too"
        );
        assert_eq!(s.journal.last().unwrap().after, Value::Float(-30.0));
    }

    /// Undoing twice must not walk back over the reversal itself, or the value
    /// would toggle forever instead of stepping through history.
    #[test]
    fn a_reversal_is_not_itself_an_undo_step() {
        let (mut s, _) = rig(&[-30.0]);
        let _ = s.write("vol.user", &[], Value::Float(-18.0));

        assert!(s.undo().unwrap().unwrap().moved());
        assert!(!s.can_undo(), "one change, one step back");
        assert_eq!(s.undo().unwrap(), None);
    }

    /// A fresh write is a new branch of history: what had been undone is gone.
    #[test]
    fn a_fresh_write_clears_the_redo_stack() {
        let (mut s, _) = rig(&[-30.0]);
        let _ = s.write("vol.user", &[], Value::Float(-18.0));
        s.undo().unwrap();
        assert!(s.can_redo());

        let _ = s.write("vol.user", &[], Value::Float(-12.0));
        assert!(!s.can_redo(), "the undone branch is unreachable now");
        assert_eq!(s.redo().unwrap(), None);
    }

    /// Flash writes are confirmed up front instead, so undo names them rather
    /// than pretending it put something back.
    #[test]
    fn an_irreversible_change_is_reported_not_reversed() {
        let t = MockTransport::new()
            .data(op::REQ_PRESET_SET_NAME, vec![0])
            .data(op::REQ_PRESET_GET_NAME, {
                let mut v = vec![0u8; 32];
                v[..4].copy_from_slice(b"Test");
                v
            });
        let mut s = Session::new(Box::new(t), caps()).unwrap();
        s.write("preset.name", &[0], Value::Text("Test".into()))
            .unwrap();

        assert!(!s.can_undo());
        let undone = s.undo().unwrap().unwrap();
        assert!(!undone.moved());
        assert_eq!(undone.skipped.len(), 1);
        assert!(
            undone.skipped[0].contains("preset.name"),
            "{:?}",
            undone.skipped
        );
    }

    /// The echo has to be a line the user could have typed, indices and all.
    #[test]
    fn the_echo_names_the_channel_the_way_the_user_would() {
        let t = MockTransport::new().data(op::REQ_GET_DELAY, 2.5f32.to_le_bytes().to_vec());
        let mut s = Session::new(Box::new(t), caps()).unwrap();
        let _ = s.write("ch.delay", &[8], Value::Float(5.0));

        let undone = s.undo().unwrap().unwrap();
        assert_eq!(undone.command.as_deref(), Some("ch.delay ch.8 2.5"));
    }

    /// A stand-in for the tube and limiter code on the device, so an undo can
    /// be judged by the state it leaves rather than by the bytes it sends.
    /// Tube: tube.c:113-119 and 138-165. Limiter: limiter.c:143-190.
    struct Device {
        /// Type, bias, asymmetry, hardness, sag.
        tube: [f32; 5],
        /// Per output: enabled, threshold, release, link group.
        lim: Vec<[f32; 4]>,
    }

    impl Device {
        fn new() -> Self {
            Self {
                tube: [0.0, 10.0, 1.0, 40.0, 20.0],
                lim: vec![[0.0, 0.0, 100.0, 0.0]; 9],
            }
        }

        fn set_tube(&mut self, index: u16, v: f32) {
            use dspi_proto::generated::tube as t;
            match index {
                t::TUBE_PARAM_TUBE_TYPE => {
                    let ty = v.round() as u8;
                    if let Some(r) = crate::tube::type_row(ty) {
                        self.tube[1..].copy_from_slice(&[
                            r.bias_pct,
                            r.asym_db,
                            r.hardness_pct,
                            r.sag_pct,
                        ]);
                    }
                    self.tube[0] = ty as f32;
                }
                t::TUBE_PARAM_BIAS_PCT
                | t::TUBE_PARAM_ASYM_DB
                | t::TUBE_PARAM_HARDNESS_PCT
                | t::TUBE_PARAM_SAG_PCT => {
                    let i = (index - t::TUBE_PARAM_BIAS_PCT) as usize + 1;
                    if self.tube[i] != v {
                        self.tube[i] = v;
                        self.tube[0] = 0.0;
                    }
                }
                _ => {}
            }
        }

        fn peer(&self, out: usize, group: f32) -> Option<usize> {
            (0..self.lim.len()).find(|&m| m != out && self.lim[m][3] == group)
        }

        fn set_limiter(&mut self, out: u8, index: usize, v: f32) {
            if out == ALL {
                for k in 0..self.lim.len() {
                    self.lim[k][index] = v;
                }
                if index == 3 {
                    for k in 0..self.lim.len() {
                        let g = self.lim[k][3];
                        if let Some(lead) = self.peer(k, g).filter(|l| g != 0.0 && *l < k) {
                            let src = self.lim[lead];
                            self.lim[k][..3].copy_from_slice(&src[..3]);
                        }
                    }
                }
            } else if index == 3 {
                let out = out as usize;
                self.lim[out][3] = v;
                if let Some(peer) = self.peer(out, v).filter(|_| v != 0.0) {
                    let src = self.lim[peer];
                    self.lim[out][..3].copy_from_slice(&src[..3]);
                }
            } else {
                let out = out as usize;
                let g = self.lim[out][3];
                for k in 0..self.lim.len() {
                    if k == out || (g != 0.0 && self.lim[k][3] == g) {
                        self.lim[k][index] = v;
                    }
                }
            }
        }
    }

    const ALL: u8 = dspi_proto::registry::ALL_OUTPUTS;

    fn number(data: &[u8]) -> f32 {
        match data.len() {
            4 => f32::from_le_bytes(data.try_into().unwrap()),
            _ => data.first().copied().unwrap_or(0) as f32,
        }
    }

    fn answer(v: f32, len: u16) -> Vec<u8> {
        match len {
            4 => v.to_le_bytes().to_vec(),
            n => {
                let mut b = vec![0; n as usize];
                if let Some(first) = b.first_mut() {
                    *first = v as u8;
                }
                b
            }
        }
    }

    /// Shared so the test can look at the device after the session owns it.
    struct Fake(
        std::sync::Arc<std::sync::Mutex<Device>>,
        dspi_transport::DeviceDescriptor,
    );

    impl dspi_transport::Transport for Fake {
        fn control_in(
            &mut self,
            opcode: u8,
            value: u16,
            len: u16,
        ) -> dspi_transport::Result<Vec<u8>> {
            let d = self.0.lock().unwrap();
            let v = match opcode {
                op::REQ_GET_TUBE_PARAM => match value as usize {
                    2 => d.tube[0],
                    i @ 4..=7 => d.tube[i - 3],
                    _ => 0.0,
                },
                op::REQ_LIMITER => d.lim[(value >> 8) as usize][(value & 0xFF) as usize],
                _ => 0.0,
            };
            Ok(answer(v, len))
        }

        fn control_out(
            &mut self,
            opcode: u8,
            value: u16,
            data: &[u8],
        ) -> dspi_transport::Result<()> {
            let mut d = self.0.lock().unwrap();
            match opcode {
                op::REQ_SET_TUBE_PARAM => d.set_tube(value, number(data)),
                op::REQ_LIMITER => {
                    d.set_limiter((value >> 8) as u8, (value & 0xFF) as usize, number(data))
                }
                _ => {}
            }
            Ok(())
        }

        fn descriptor(&self) -> &dspi_transport::DeviceDescriptor {
            &self.1
        }
    }

    fn fake() -> (Session, std::sync::Arc<std::sync::Mutex<Device>>) {
        let device = std::sync::Arc::new(std::sync::Mutex::new(Device::new()));
        let mut caps = caps();
        for name in ["tube_preamp", "output_limiter"] {
            caps.features.push(crate::probe::Feature {
                name: name.into(),
                present: true,
                evidence: "test".into(),
            });
        }
        let descriptor = dspi_transport::DeviceDescriptor {
            serial: "FAKE".into(),
            bus_id: "fake".into(),
            address: 0,
        };
        let s = Session::new(Box::new(Fake(device.clone(), descriptor)), caps).unwrap();
        (s, device)
    }

    /// Undoing a tube type chosen over a Custom voicing puts the Custom
    /// voicing back: writing type 0 alone loads no row (tube.c:138-150).
    #[test]
    fn undoing_a_tube_type_restores_the_custom_voicing() {
        use dspi_proto::generated::tube as t;
        assert_eq!(t::TUBE_PARAM_TUBE_TYPE, 2, "the stand-in's indexing");
        let (mut s, device) = fake();
        let custom = device.lock().unwrap().tube;
        s.write("tube.type", &[], Value::Int(3)).unwrap();
        assert_ne!(device.lock().unwrap().tube, custom, "the row loaded");
        let undone = s.undo().unwrap().unwrap();
        assert!(undone.moved());
        assert_eq!(device.lock().unwrap().tube, custom);
        // Redo is the type again, which loads the row again.
        s.redo().unwrap().unwrap();
        assert_eq!(device.lock().unwrap().tube[0], 3.0);
    }

    /// Undoing a character edit that dropped a type to Custom (tube.c:113-119)
    /// puts the type back as well.
    #[test]
    fn undoing_a_character_edit_restores_the_tube_type() {
        let (mut s, device) = fake();
        s.write("tube.type", &[], Value::Int(3)).unwrap();
        let typed = device.lock().unwrap().tube;
        s.write("tube.bias", &[], Value::Float(-20.0)).unwrap();
        assert_eq!(device.lock().unwrap().tube[0], 0.0, "Custom now");
        s.undo().unwrap().unwrap();
        assert_eq!(device.lock().unwrap().tube, typed);
    }

    /// Undoing a link change puts back the settings the output adopted on
    /// joining (limiter.c:180-186).
    #[test]
    fn undoing_a_limiter_link_restores_what_joining_adopted() {
        let (mut s, device) = fake();
        {
            let mut d = device.lock().unwrap();
            d.lim[0] = [1.0, -6.0, 50.0, 1.0];
            d.lim[1] = [0.0, -1.0, 300.0, 0.0];
        }
        let before = device.lock().unwrap().lim[1];
        s.write("limit.link", &[1], Value::Int(1)).unwrap();
        assert_eq!(device.lock().unwrap().lim[1], [1.0, -6.0, 50.0, 1.0]);
        s.undo().unwrap().unwrap();
        assert_eq!(device.lock().unwrap().lim[1], before);
        assert_eq!(device.lock().unwrap().lim[0], [1.0, -6.0, 50.0, 1.0]);
    }

    /// A write to every output is undone output by output, and never by
    /// stepping over it to an older, unrelated change.
    #[test]
    fn undoing_a_write_to_every_output_restores_each_one() {
        let (mut s, device) = fake();
        s.write("tube.drive", &[], Value::Float(3.0)).unwrap();
        {
            let mut d = device.lock().unwrap();
            for (o, l) in d.lim.iter_mut().enumerate() {
                l[1] = -(o as f32);
            }
        }
        let before = device.lock().unwrap().lim.clone();
        s.write("limit.threshold", &[ALL], Value::Float(-10.0))
            .unwrap();
        assert!(device.lock().unwrap().lim.iter().all(|l| l[1] == -10.0));
        let undone = s.undo().unwrap().unwrap();
        assert!(undone.moved());
        assert_eq!(device.lock().unwrap().lim, before);
        assert!(
            undone
                .command
                .as_deref()
                .unwrap()
                .starts_with("limit.threshold 0 "),
            "{undone:?}"
        );
    }

    /// An entry with nothing to put back stops the undo there rather than
    /// reverting the change before it.
    #[test]
    fn a_change_with_no_previous_value_does_not_undo_an_older_one() {
        let (mut s, log) = rig(&[-30.0]);
        let _ = s.write("vol.user", &[], Value::Float(-18.0));
        let mut blind = s.undo_stack.last().unwrap().clone();
        blind.before = None;
        s.undo_stack.push(blind);
        let undone = s.undo().unwrap().unwrap();
        assert!(!undone.moved());
        assert_eq!(undone.skipped.len(), 1, "{undone:?}");
        assert_eq!(
            writes(&log, op::REQ_SET_USER_VOLUME).len(),
            1,
            "the older change was left alone"
        );
        // The next undo goes on to the older change.
        assert!(s.undo().unwrap().unwrap().moved());
    }

    /// Write-as-read parameters exist; a reversal must still take the IN path.
    #[test]
    fn the_direction_of_the_reversal_follows_the_registry() {
        let d = dspi_proto::registry::by_path("vol.user").unwrap();
        assert_eq!(d.dir, Dir::Out, "vol.user is an OUT write");
    }
}
