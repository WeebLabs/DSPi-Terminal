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

use dspi_cmd::{Command, Context};

use crate::write::{JournalEntry, Outcome, Session, WriteError};

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
        self.undo_stack.iter().any(|e| reversible(e).is_ok())
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

            let before = match reversible(&entry) {
                Ok(v) => v.clone(),
                Err(why) => {
                    skipped.push(why);
                    continue;
                }
            };

            let outcome = match self.replay(entry.path, &entry.indices, before.clone()) {
                Ok(o) => o,
                Err(e) => {
                    // The device refused the reversal, so the change is still
                    // standing and must stay on the stack to be tried again.
                    self.undo_stack.push(entry);
                    return Err(e);
                }
            };

            let command = self.render(entry.path, &entry.indices, &before);
            self.redo_stack.push(entry);
            return Ok(Some(Undone {
                command: Some(command),
                outcome: Some(outcome),
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

/// The value this entry would restore, or why it cannot restore one.
fn reversible(entry: &JournalEntry) -> Result<&dspi_proto::value::Value, String> {
    if !entry.undoable {
        return Err(format!("`{}` cannot be undone", entry.path));
    }
    entry
        .before
        .as_ref()
        .ok_or_else(|| format!("`{}` has no previous value to restore", entry.path))
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
            wire_format: 28,
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

    /// Write-as-read parameters exist; a reversal must still take the IN path.
    #[test]
    fn the_direction_of_the_reversal_follows_the_registry() {
        let d = dspi_proto::registry::by_path("vol.user").unwrap();
        assert_eq!(d.dir, Dir::Out, "vol.user is an OUT write");
    }
}
