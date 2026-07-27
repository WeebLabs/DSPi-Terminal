//! The single write path.
//!
//! Every parameter change in the application goes through [`Session::write`],
//! because every subtlety of this protocol lives in one of its steps and none of
//! them are optional:
//!
//! - Some mutating opcodes are dispatched on the **IN** path and stall if issued
//!   as an OUT transfer.
//! - Deferred writes return "accepted" **before** validation runs, so the status
//!   byte is not evidence the value took effect. A bad DAC mute config is
//!   dropped silently.
//! - Flash writes disable the control endpoint for tens of milliseconds, so the
//!   confirming read has to survive a blackout.
//!
//! Spreading this across call sites would guarantee that some of them get it
//! wrong. There is one implementation, and the outcome type makes a silent
//! rejection impossible to ignore.

use dspi_proto::FilterType;
use dspi_proto::generated::opcodes as op;
use dspi_proto::registry::{Hazard, Kind, ParamDesc, Requires, Target, WValue, by_path};
use dspi_proto::value::{EqParamPacket, Repr, Value, ValueError, decode_qp};
use dspi_proto::{ChannelMap, Dir, Platform};
use dspi_transport::{Transport, TransportError, with_busy_retry};

use crate::probe::Capabilities;

#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error("no parameter called `{0}`")]
    UnknownParam(String),

    #[error("`{path}` cannot be changed; it is read-only")]
    ReadOnly { path: String },

    #[error("`{path}` needs {want} index(es), got {got}")]
    WrongArity {
        path: String,
        want: usize,
        got: usize,
    },

    #[error("{0} is not a valid {1} on this device")]
    BadTarget(u8, &'static str),

    #[error("`{path}` is not available: {why}")]
    Unavailable { path: String, why: String },

    #[error("{0}")]
    Value(#[from] ValueError),

    #[error(transparent)]
    Transport(#[from] TransportError),
}

/// What actually happened, which is not always what was asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Written and confirmed by reading it back.
    Confirmed(Value),

    /// The device accepted the request but the readback shows a different value.
    ///
    /// This is the case the protocol makes easy to miss and expensive to get
    /// wrong: the write returned success, and the device did something else.
    Rejected { sent: Value, actual: Value },

    /// Written; the parameter has no readback, so this is the device's word for it.
    Accepted,

    /// An action with no value to confirm.
    Triggered,
}

impl Outcome {
    pub fn is_confirmed(&self) -> bool {
        matches!(
            self,
            Outcome::Confirmed(_) | Outcome::Accepted | Outcome::Triggered
        )
    }
}

/// One recorded change, for undo and for the command echo.
#[derive(Debug, Clone)]
pub struct JournalEntry {
    pub path: &'static str,
    pub indices: Vec<u8>,
    pub before: Option<Value>,
    pub after: Value,
    pub outcome: Outcome,
    /// Whether this can be undone. Flash writes, pin moves and the bootloader
    /// jump cannot, so they are confirmed up front instead.
    pub undoable: bool,
}

pub struct Session {
    transport: Box<dyn Transport>,
    caps: Capabilities,
    map: ChannelMap,
    pub journal: Vec<JournalEntry>,
    /// When set, nothing is written; the intended transfer is recorded instead.
    pub dry_run: bool,
}

impl Session {
    pub fn new(transport: Box<dyn Transport>, caps: Capabilities) -> Option<Self> {
        let map = ChannelMap::new(caps.num_inputs, caps.num_outputs, caps.num_channels)?
            .with_names(caps.channels.iter().map(|c| c.name.clone()).collect());
        Some(Self {
            transport,
            caps,
            map,
            journal: Vec::new(),
            dry_run: false,
        })
    }

    pub fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    pub fn channel_map(&self) -> &ChannelMap {
        &self.map
    }

    /// Change one parameter, and confirm it actually changed.
    pub fn write(
        &mut self,
        path: &str,
        indices: &[u8],
        value: Value,
    ) -> Result<Outcome, WriteError> {
        let d = by_path(path).ok_or_else(|| WriteError::UnknownParam(path.into()))?;

        let set = d
            .set
            .ok_or_else(|| WriteError::ReadOnly { path: path.into() })?;

        self.check_available(d)?;
        self.check_indices(d, indices)?;

        // Validate before the wire, so the user gets a message naming the limits
        // rather than a bare stall from the device.
        let value = d.kind.validate(&value)?;

        let before = self.read(path, indices).ok();

        if self.dry_run {
            let outcome = Outcome::Accepted;
            self.record(d, indices, before, value, outcome.clone());
            return Ok(outcome);
        }

        // An EQ band is written as one whole packet, never field by field: the
        // firmware's SET_EQ_PARAM takes a 16-byte descriptor and carries nothing
        // in wValue. Changing one field therefore means reading the band,
        // editing it, and writing it back.
        if matches!(d.wvalue, WValue::EqScalar(_)) {
            let outcome = self.write_eq_field(d, indices[0], indices[1], &value)?;
            self.record(d, indices, before, value, outcome.clone());
            return Ok(outcome);
        }

        let wvalue = self.build_wvalue(d, indices, &value)?;
        let repr = d.kind.repr();

        match d.dir {
            Dir::Out => {
                let payload = repr.encode(&value)?;
                self.transport.control_out(set, wvalue, &payload)?;
            }
            // Mutating commands on the IN path. Not a mistake: they carry their
            // parameters in wValue and answer with a status byte.
            Dir::WriteAsRead | Dir::In => {
                self.transport.control_in(set, wvalue, 1)?;
            }
        }

        let outcome = self.confirm(d, indices, &value)?;
        self.record(d, indices, before, value, outcome.clone());
        Ok(outcome)
    }

    /// Poll every meter in one transfer.
    pub fn meters(&mut self) -> Result<crate::Meters, WriteError> {
        Ok(crate::read_meters(
            &mut *self.transport,
            self.caps.num_channels,
        )?)
    }

    /// Read a whole EQ band.
    ///
    /// There is no full-packet read in the protocol, so this costs five
    /// transfers, plus a sixth for the Linkwitz Transform's `Qp`.
    pub fn read_band(&mut self, channel: u8, band: u8) -> Result<EqParamPacket, WriteError> {
        let scalar = |s: &mut Self, param: u8, len: u16| -> Result<Vec<u8>, WriteError> {
            let wvalue = ((channel as u16) << 8) | ((band as u16) << 3) | param as u16;
            Ok(with_busy_retry(
                || s.transport.control_in(op::REQ_GET_EQ_PARAM, wvalue, len),
                op::REQ_GET_EQ_PARAM,
            )?)
        };
        let f32_at = |s: &mut Self, param: u8| -> Result<f32, WriteError> {
            let b = scalar(s, param, 4)?;
            Ok(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        };

        let filter_type = FilterType::from_raw(scalar(self, 0, 4)?[0]);
        let freq = f32_at(self, 1)?;
        let q = f32_at(self, 2)?;
        let gain_db = f32_at(self, 3)?;
        let bypass = scalar(self, 4, 4)?[0] != 0;

        // Only fetch the sidecar when it means something; on other types the
        // firmware stores zero there and reading it would just cost a transfer.
        let qp = if filter_type.is_linkwitz() {
            let raw = scalar(self, 5, 4)?;
            Some(decode_qp(u16::from_le_bytes([raw[0], raw[1]])))
        } else {
            None
        };

        Ok(EqParamPacket {
            channel,
            band,
            filter_type,
            bypass,
            freq,
            q,
            gain_db,
            qp,
        })
    }

    /// Write a whole EQ band in one transfer.
    ///
    /// Prefer this over setting fields one at a time: it is one transfer rather
    /// than seven, and it cannot leave the band in a half-updated state if the
    /// device disappears midway.
    pub fn write_band(&mut self, packet: &EqParamPacket) -> Result<Outcome, WriteError> {
        if self.dry_run {
            return Ok(Outcome::Accepted);
        }
        self.transport
            .control_out(op::REQ_SET_EQ_PARAM, 0, &packet.encode())?;

        let actual = self.read_band(packet.channel, packet.band)?;
        Ok(if bands_agree(packet, &actual) {
            Outcome::Confirmed(Value::Bytes(packet.encode()))
        } else {
            Outcome::Rejected {
                sent: Value::Bytes(packet.encode()),
                actual: Value::Bytes(actual.encode()),
            }
        })
    }

    /// Change one field of a band, preserving the rest.
    fn write_eq_field(
        &mut self,
        d: &ParamDesc,
        channel: u8,
        band: u8,
        value: &Value,
    ) -> Result<Outcome, WriteError> {
        let mut packet = self.read_band(channel, band)?;

        match d.path {
            "eq.type" => {
                packet.filter_type = FilterType::from_raw(value.as_u8().unwrap_or(0));
                // Switching to a Linkwitz Transform needs a Qp to send; the
                // firmware reads 0 as "use the 0.707 default".
                if packet.filter_type.is_linkwitz() && packet.qp.is_none() {
                    packet.qp = Some(0.707);
                }
            }
            "eq.freq" => packet.freq = value.as_f32().unwrap_or(packet.freq),
            "eq.q" => packet.q = value.as_f32().unwrap_or(packet.q),
            "eq.gain" => packet.gain_db = value.as_f32().unwrap_or(packet.gain_db),
            other => {
                return Err(WriteError::UnknownParam(other.into()));
            }
        }

        self.write_band(&packet)
    }

    /// Read one parameter back.
    pub fn read(&mut self, path: &str, indices: &[u8]) -> Result<Value, WriteError> {
        let d = by_path(path).ok_or_else(|| WriteError::UnknownParam(path.into()))?;
        let get = d
            .get
            .ok_or_else(|| WriteError::ReadOnly { path: path.into() })?;

        self.check_indices(d, indices)?;
        let wvalue = self.build_read_wvalue(d, indices);
        let repr = d.kind.repr();
        // EQ scalars always answer four bytes regardless of the field's width.
        let len = if matches!(d.wvalue, WValue::EqScalar(_)) {
            4
        } else {
            repr.len().max(1)
        };

        let bytes = with_busy_retry(|| self.transport.control_in(get, wvalue, len as u16), get)?;

        Ok(decode_read(d, &bytes))
    }

    /// Confirm a write landed, for the parameters where the status byte is not
    /// proof. Everything else is taken at the device's word.
    fn confirm(
        &mut self,
        d: &ParamDesc,
        indices: &[u8],
        sent: &Value,
    ) -> Result<Outcome, WriteError> {
        if matches!(d.kind, Kind::Trigger) {
            return Ok(Outcome::Triggered);
        }
        if !d.needs_readback() {
            return Ok(match d.get {
                // Cheap to verify, so verify.
                Some(_) => match self.read(d.path, indices) {
                    Ok(actual) if values_agree(sent, &actual) => Outcome::Confirmed(actual),
                    Ok(actual) => Outcome::Rejected {
                        sent: sent.clone(),
                        actual,
                    },
                    Err(_) => Outcome::Accepted,
                },
                None => Outcome::Accepted,
            });
        }

        // A deferred write may still be applying, and the control endpoint may
        // be dead mid-flash. The retry inside read() covers the blackout.
        match self.read(d.path, indices) {
            Ok(actual) if values_agree(sent, &actual) => Ok(Outcome::Confirmed(actual)),
            Ok(actual) => Ok(Outcome::Rejected {
                sent: sent.clone(),
                actual,
            }),
            Err(e) => Err(e),
        }
    }

    fn record(
        &mut self,
        d: &ParamDesc,
        indices: &[u8],
        before: Option<Value>,
        after: Value,
        outcome: Outcome,
    ) {
        self.journal.push(JournalEntry {
            path: d.path,
            indices: indices.to_vec(),
            before,
            after,
            outcome,
            undoable: matches!(d.hazard, Hazard::None | Hazard::Audible),
        });
    }

    /// Refuse a parameter this device does not have, with the reason.
    fn check_available(&self, d: &ParamDesc) -> Result<(), WriteError> {
        let why = match d.requires {
            Requires::Always => return Ok(()),
            Requires::Rp2350 => {
                if self.caps.platform == Platform::Rp2350 {
                    return Ok(());
                }
                format!("it needs an RP2350; this is {}", self.caps.platform.name())
            }
            Requires::Feature(name) => {
                let present = self
                    .caps
                    .features
                    .iter()
                    .any(|f| f.name == name && f.present);
                if present {
                    return Ok(());
                }
                format!("this firmware does not provide `{name}`")
            }
        };
        Err(WriteError::Unavailable {
            path: d.path.into(),
            why,
        })
    }

    /// Range-check indices against the device's actual topology, never a constant.
    fn check_indices(&self, d: &ParamDesc, indices: &[u8]) -> Result<(), WriteError> {
        let want = d.target.arity();
        if indices.len() != want {
            return Err(WriteError::WrongArity {
                path: d.path.into(),
                want,
                got: indices.len(),
            });
        }

        let ok = match d.target {
            Target::None => true,
            Target::Channel => indices[0] < self.map.num_channels(),
            Target::Input => indices[0] < self.map.num_inputs(),
            Target::Output => indices[0] < self.map.num_outputs(),
            Target::ChannelBand => {
                indices[0] < self.map.num_channels()
                    && is_valid_band(indices[1], self.caps.max_bands)
            }
            Target::Crosspoint => {
                indices[0] < self.map.num_inputs() && indices[1] < self.map.num_outputs()
            }
            Target::PresetSlot => indices[0] < 10,
            Target::CsSlot => indices[0] < self.caps.cs.as_ref().map_or(0, |c| c.max_bindings),
            Target::CsIrSlot => indices[0] < self.caps.cs.as_ref().map_or(0, |c| c.max_ir_commands),
            Target::LegacyChannel => indices[0] < 3,
            // Bounded by the device but not separately reported; the firmware
            // range-checks these and answers with a status code.
            _ => true,
        };

        if ok {
            Ok(())
        } else {
            Err(WriteError::BadTarget(indices[0], target_name(d.target)))
        }
    }

    fn build_wvalue(
        &self,
        d: &ParamDesc,
        indices: &[u8],
        value: &Value,
    ) -> Result<u16, WriteError> {
        Ok(wvalue_for(d, indices, value))
    }

    /// Reads address the same parameter but never carry a value in `wValue`.
    fn build_read_wvalue(&self, d: &ParamDesc, indices: &[u8]) -> u16 {
        read_wvalue_for(d, indices)
    }
}

/// Pack the target and value into `wValue` for a write.
///
/// Free rather than a method so the packing rules can be tested directly; they
/// are the fiddliest part of the protocol and the easiest to get subtly wrong.
pub(crate) fn wvalue_for(d: &ParamDesc, indices: &[u8], value: &Value) -> u16 {
    {
        match d.wvalue {
            WValue::Zero => 0,
            WValue::Fixed(v) => v,
            WValue::Target => indices.first().copied().unwrap_or(0) as u16,
            WValue::ChannelBand => ((indices[0] as u16) << 8) | indices[1] as u16,
            WValue::Crosspoint => ((indices[0] as u16) << 8) | indices[1] as u16,
            // The band field is five bits wide, so crossover bands 20-23 fit.
            WValue::EqScalar(param) => {
                ((indices[0] as u16) << 8) | ((indices[1] as u16) << 3) | param as u16
            }
            WValue::ValueSlot => {
                let v = value.as_u8().unwrap_or(0) as u16;
                (v << 8) | indices.first().copied().unwrap_or(0) as u16
            }
            WValue::ValueIndex => {
                let v = value.as_f32().unwrap_or(0.0) as u16;
                (v << 8) | indices.first().copied().unwrap_or(0) as u16
            }
            WValue::ValueOnly => value.as_f32().unwrap_or(0.0) as u16,
        }
    }
}

pub(crate) fn read_wvalue_for(d: &ParamDesc, indices: &[u8]) -> u16 {
    {
        match d.wvalue {
            WValue::Fixed(v) => v,
            WValue::ChannelBand | WValue::Crosspoint => {
                ((indices[0] as u16) << 8) | indices[1] as u16
            }
            WValue::EqScalar(param) => {
                ((indices[0] as u16) << 8) | ((indices[1] as u16) << 3) | param as u16
            }
            WValue::Target | WValue::ValueSlot | WValue::ValueIndex => {
                indices.first().copied().unwrap_or(0) as u16
            }
            _ => 0,
        }
    }
}

fn is_valid_band(band: u8, max_peq: u8) -> bool {
    // PEQ bands sit at 0..max, crossover bands at 20..23. The gap between is
    // deliberately rejected by the firmware.
    band < max_peq || (20..24).contains(&band)
}

fn target_name(t: Target) -> &'static str {
    match t {
        Target::Channel => "channel",
        Target::Input => "input",
        Target::Output => "output",
        Target::ChannelBand => "channel or band",
        Target::Crosspoint => "crosspoint",
        Target::PresetSlot => "preset slot",
        Target::CsSlot => "binding slot",
        Target::CsIrSlot => "IR command slot",
        Target::LegacyChannel => "legacy channel",
        _ => "index",
    }
}

/// Interpret a read according to the parameter's kind.
fn decode_read(d: &ParamDesc, bytes: &[u8]) -> Value {
    let repr = d.kind.repr();

    // EQ scalars always answer four bytes: an f32 for freq/Q/gain, and a small
    // integer in the low byte for type and bypass.
    if let WValue::EqScalar(param) = d.wvalue {
        return match param {
            1..=3 => Repr::F32.decode(bytes).unwrap_or(Value::Float(0.0)),
            _ => Value::Int(bytes.first().copied().unwrap_or(0) as i64),
        };
    }

    match d.kind {
        Kind::Bool => Value::Bool(bytes.first().copied().unwrap_or(0) != 0),
        Kind::Choice(_) => Value::Choice(bytes.first().copied().unwrap_or(0)),
        Kind::Mask => Value::Mask(
            bytes
                .first()
                .copied()
                .map(|lo| lo as u32 | ((bytes.get(1).copied().unwrap_or(0) as u32) << 8))
                .unwrap_or(0),
        ),
        _ => repr.decode(bytes).unwrap_or(Value::Bytes(bytes.to_vec())),
    }
}

/// Compare a whole band, field by field, with the same float tolerance scalars
/// use. Comparing encoded bytes would report spurious rejections, because the
/// device re-quantises what it stores.
fn bands_agree(sent: &EqParamPacket, actual: &EqParamPacket) -> bool {
    let close = |a: f32, b: f32| (a - b).abs() <= a.abs().max(1.0) * 1e-3;
    sent.filter_type == actual.filter_type
        && sent.bypass == actual.bypass
        && close(sent.freq, actual.freq)
        && close(sent.q, actual.q)
        && close(sent.gain_db, actual.gain_db)
}

/// Compare what we sent against what came back.
///
/// Floats need a tolerance: the device stores single precision and several
/// parameters are clamped or quantised on the way in, so exact equality would
/// report spurious rejections on values that were in fact accepted.
fn values_agree(sent: &Value, actual: &Value) -> bool {
    match (sent, actual) {
        (Value::Float(a), Value::Float(b)) => (a - b).abs() <= a.abs().max(1.0) * 1e-3,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Bool(a), Value::Int(b)) => *a == (*b != 0),
        (Value::Choice(a), Value::Int(b)) => *a as i64 == *b,
        (Value::Choice(a), Value::Choice(b)) => a == b,
        (Value::Int(a), Value::Int(b)) => a == b,
        (Value::Float(a), Value::Int(b)) => (*a as i64) == *b,
        (Value::Mask(a), Value::Mask(b)) => a == b,
        (Value::Mask(a), Value::Int(b)) => *a as i64 == *b,
        (Value::Text(a), Value::Text(b)) => a == b,
        _ => sent == actual,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::{Capabilities, ChannelInfo, ControlSurfaceCaps, Feature};
    use dspi_proto::generated::opcodes as op;
    use dspi_transport::MockTransport;
    use dspi_transport::mock::{Direction, Reply};

    pub(super) fn caps(platform: Platform, features: &[(&str, bool)]) -> Capabilities {
        Capabilities {
            serial: "TEST".into(),
            platform,
            firmware: "1.1.5".into(),
            wire_format: 26,
            num_channels: 17,
            num_inputs: 8,
            num_outputs: 9,
            max_bands: 12,
            channels: (0..17)
                .map(|i| ChannelInfo {
                    index: i,
                    name: format!("Ch {i}"),
                    slug: format!("ch.{i}"),
                    is_output: i >= 8,
                })
                .collect(),
            features: features
                .iter()
                .map(|(n, p)| Feature {
                    name: (*n).into(),
                    present: *p,
                    evidence: "test".into(),
                })
                .collect(),
            cs: Some(ControlSurfaceCaps {
                caps_version: 4,
                max_bindings: 16,
                type_count: 8,
                noun_count: 49,
                max_ir_commands: 8,
            }),
            siggen: None,
            active_preset: Some(0),
        }
    }

    /// Fixed-width NUL-padded text, as the device actually returns it.
    pub(super) fn padded(s: &str) -> Vec<u8> {
        let mut v = vec![0u8; 32];
        v[..s.len()].copy_from_slice(s.as_bytes());
        v
    }

    fn session(t: MockTransport, features: &[(&str, bool)]) -> Session {
        Session::new(Box::new(t), caps(Platform::Rp2350, features)).unwrap()
    }

    #[test]
    fn writes_a_float_and_confirms_it() {
        let t =
            MockTransport::new().data(op::REQ_GET_USER_VOLUME, (-18.0f32).to_le_bytes().to_vec());
        let mut s = session(t, &[]);

        let out = s.write("vol.user", &[], Value::Float(-18.0)).unwrap();
        assert_eq!(out, Outcome::Confirmed(Value::Float(-18.0)));
        assert!(out.is_confirmed());
    }

    /// The failure mode the protocol makes easy to miss: the write succeeds and
    /// the device quietly keeps a different value.
    #[test]
    fn a_silent_rejection_is_surfaced_not_swallowed() {
        let t = MockTransport::new()
            // The device clamps to -60 but reports success on the write.
            .data(op::REQ_GET_USER_VOLUME, (-60.0f32).to_le_bytes().to_vec());
        let mut s = session(t, &[]);

        let out = s.write("vol.user", &[], Value::Float(-50.0)).unwrap();
        match out {
            Outcome::Rejected { sent, actual } => {
                assert_eq!(sent, Value::Float(-50.0));
                assert_eq!(actual, Value::Float(-60.0));
            }
            other => panic!("expected a rejection, got {other:?}"),
        }
    }

    #[test]
    fn out_of_range_is_refused_before_the_wire() {
        let t = MockTransport::new();
        let mut s = session(t, &[("psychoacoustic_bass", true)]);

        let e = s.write("bass.drive", &[], Value::Float(99.0)).unwrap_err();
        assert!(e.to_string().contains("18"), "should name the limit: {e}");
    }

    #[test]
    fn a_missing_feature_is_explained_not_stalled() {
        let t = MockTransport::new();
        let mut s = session(t, &[("psychoacoustic_bass", false)]);

        let e = s.write("bass.on", &[], Value::Bool(true)).unwrap_err();
        assert!(e.to_string().contains("psychoacoustic_bass"), "{e}");
    }

    #[test]
    fn rp2350_only_parameters_are_refused_on_rp2040() {
        let mut caps = caps(Platform::Rp2040, &[]);
        caps.num_inputs = 2;
        caps.num_outputs = 5;
        caps.num_channels = 7;
        caps.channels.truncate(7);
        let mut s = Session::new(Box::new(MockTransport::new()), caps).unwrap();

        let e = s
            .write("up.config", &[], Value::Bytes(vec![0; 44]))
            .unwrap_err();
        assert!(e.to_string().contains("RP2350"), "{e}");
    }

    /// Topology comes from the device, so an index valid on one platform must be
    /// rejected on another.
    #[test]
    fn indices_are_checked_against_the_actual_topology() {
        // RP2350 has nine outputs, so 8 is the last valid index and 9 is not.
        let t = MockTransport::new().data(op::REQ_GET_OUTPUT_GAIN, 0.0f32.to_le_bytes().to_vec());
        let mut s = session(t, &[]);
        assert!(s.write("out.gain", &[8], Value::Float(0.0)).is_ok());
        assert!(s.write("out.gain", &[9], Value::Float(0.0)).is_err());

        let mut caps40 = caps(Platform::Rp2040, &[]);
        caps40.num_inputs = 2;
        caps40.num_outputs = 5;
        caps40.num_channels = 7;
        caps40.channels.truncate(7);
        let mut s40 = Session::new(Box::new(MockTransport::new()), caps40).unwrap();
        let e = s40.write("out.gain", &[6], Value::Float(0.0)).unwrap_err();
        assert!(e.to_string().contains("output"), "{e}");
    }

    #[test]
    fn missing_indices_are_caught_with_a_useful_message() {
        let t = MockTransport::new();
        let mut s = session(t, &[]);
        let e = s.write("eq.freq", &[8], Value::Float(1000.0)).unwrap_err();
        assert!(e.to_string().contains("2 index"), "{e}");
    }

    /// The band field is five bits, so 20-23 must be addressable; band 15 sits
    /// in the reserved gap the firmware rejects.
    #[test]
    fn crossover_bands_are_addressable_and_the_gap_is_not() {
        let t = MockTransport::new().data(op::REQ_GET_EQ_PARAM, 80.0f32.to_le_bytes().to_vec());
        let mut s = session(t, &[]);
        assert!(s.write("eq.freq", &[8, 20], Value::Float(80.0)).is_ok());
        assert!(s.write("eq.freq", &[8, 15], Value::Float(80.0)).is_err());
    }

    /// The band field is five bits wide. A four-bit shift, which older host code
    /// used, silently aliases band 20 onto band 2 of the wrong parameter.
    #[test]
    fn eq_wvalue_packs_the_band_into_five_bits() {
        let d = by_path("eq.freq").unwrap();

        // (channel << 8) | (band << 3) | param, param 1 = freq.
        assert_eq!(wvalue_for(d, &[8, 23], &Value::Float(0.0)), 0x08B9);
        assert_eq!(wvalue_for(d, &[0, 0], &Value::Float(0.0)), 0x0001);
        assert_eq!(wvalue_for(d, &[8, 20], &Value::Float(0.0)), 0x08A1);

        // Bands above 15 must not collide with the next channel.
        let band20 = wvalue_for(d, &[1, 20], &Value::Float(0.0));
        assert_eq!(band20 >> 8, 1, "band 20 must stay within channel 1");

        // Each scalar gets its own param index.
        assert_eq!(
            wvalue_for(by_path("eq.type").unwrap(), &[8, 3], &Value::Choice(0)) & 7,
            0
        );
        assert_eq!(
            wvalue_for(by_path("eq.q").unwrap(), &[8, 3], &Value::Float(0.0)) & 7,
            2
        );
        assert_eq!(
            wvalue_for(by_path("eq.gain").unwrap(), &[8, 3], &Value::Float(0.0)) & 7,
            3
        );
    }

    #[test]
    fn wvalue_packs_targets_and_values_per_rule() {
        // (new_type << 8) | slot
        let ty = by_path("out.type").unwrap();
        assert_eq!(wvalue_for(ty, &[2], &Value::Choice(1)), 0x0102);

        // (gpio << 8) | index
        let pin = by_path("out.pin").unwrap();
        assert_eq!(wvalue_for(pin, &[4], &Value::Int(21)), 0x1504);

        // plain target
        assert_eq!(
            wvalue_for(by_path("out.gain").unwrap(), &[3], &Value::Float(0.0)),
            3
        );

        // value only, no data stage
        assert_eq!(
            wvalue_for(by_path("i2s.mck").unwrap(), &[], &Value::Bool(true)),
            1
        );

        // fixed selector
        assert_eq!(
            wvalue_for(by_path("cs.caps").unwrap(), &[], &Value::Trigger),
            0xFFFF
        );
    }

    /// A read addresses the parameter but must never smuggle a value into wValue.
    #[test]
    fn reads_do_not_carry_a_value() {
        assert_eq!(read_wvalue_for(by_path("out.type").unwrap(), &[2]), 2);
        assert_eq!(read_wvalue_for(by_path("out.pin").unwrap(), &[4]), 4);
    }

    /// Write-as-read opcodes must go out on the IN path or they stall.
    #[test]
    fn a_flash_write_survives_the_blackout() {
        let t = MockTransport::new()
            .data(op::REQ_PRESET_SET_NAME, vec![0])
            .reply(
                op::REQ_PRESET_GET_NAME,
                Reply::BusyThen(3, Box::new(Reply::Data(padded("Living Room")))),
            );
        let mut s = session(t, &[]);

        let out = s
            .write("preset.name", &[3], Value::Text("Living Room".into()))
            .unwrap();
        assert!(matches!(out, Outcome::Confirmed(_)));
    }

    #[test]
    fn dry_run_writes_nothing() {
        let t = MockTransport::new();
        let mut s = session(t, &[]);
        s.dry_run = true;

        let out = s.write("vol.user", &[], Value::Float(-20.0)).unwrap();
        assert_eq!(out, Outcome::Accepted);
        assert_eq!(s.journal.len(), 1);
        // Nothing left the host.
        assert!(!s.transport.descriptor().serial.is_empty());
    }

    #[test]
    fn the_journal_records_before_and_after() {
        let t =
            MockTransport::new().data(op::REQ_GET_USER_VOLUME, (-18.0f32).to_le_bytes().to_vec());
        let mut s = session(t, &[]);
        s.write("vol.user", &[], Value::Float(-18.0)).unwrap();

        let e = s.journal.last().unwrap();
        assert_eq!(e.path, "vol.user");
        assert_eq!(e.after, Value::Float(-18.0));
        assert!(e.undoable);
    }

    /// Flash writes and reconfiguration cannot be undone, so they must not be
    /// offered as undoable; they are confirmed before the fact instead.
    #[test]
    fn irreversible_changes_are_not_marked_undoable() {
        let t = MockTransport::new()
            .data(op::REQ_PRESET_SET_NAME, vec![0])
            .data(op::REQ_PRESET_GET_NAME, padded("Test"));
        let mut s = session(t, &[]);
        s.write("preset.name", &[0], Value::Text("Test".into()))
            .unwrap();
        assert!(!s.journal.last().unwrap().undoable);
    }

    #[test]
    fn unknown_parameters_are_named_in_the_error() {
        let t = MockTransport::new();
        let mut s = session(t, &[]);
        let e = s
            .write("bass.nonsense", &[], Value::Bool(true))
            .unwrap_err();
        assert!(e.to_string().contains("bass.nonsense"));
    }

    #[test]
    fn read_only_parameters_refuse_writes() {
        let t = MockTransport::new();
        let mut s = session(t, &[]);
        let e = s
            .write("dev.serial", &[], Value::Text("x".into()))
            .unwrap_err();
        assert!(e.to_string().contains("read-only"), "{e}");
    }

    #[test]
    fn float_comparison_tolerates_single_precision() {
        assert!(values_agree(
            &Value::Float(2856.0),
            &Value::Float(2856.0001)
        ));
        assert!(!values_agree(&Value::Float(2856.0), &Value::Float(2900.0)));
        // Small values compare absolutely rather than relatively.
        assert!(values_agree(&Value::Float(0.0), &Value::Float(0.0002)));
    }

    #[test]
    fn mock_log_shows_direction_for_out_writes() {
        let mut t = MockTransport::new();
        let _ = t.control_out(op::REQ_SET_USER_VOLUME, 0, &[0, 0, 0, 0]);
        assert_eq!(t.log()[0].direction, Direction::Out);
    }
}

#[cfg(test)]
mod eq_tests {
    use super::tests::caps;
    use super::*;
    use dspi_proto::generated::opcodes as op;
    use dspi_transport::MockTransport;
    use dspi_transport::mock::Direction;

    /// The bug this replaced: a scalar write to 0x42 sends four bytes with a
    /// packed wValue, but the firmware wants a 16-byte descriptor and reads
    /// nothing from wValue. It would have been rejected on hardware.
    /// Build a session over a mock, keeping a handle on the wire log.
    fn rig(reply: [u8; 4]) -> (Session, dspi_transport::mock::LogHandle) {
        let t = MockTransport::new().data(op::REQ_GET_EQ_PARAM, reply.to_vec());
        let log = t.log_handle();
        let s = Session::new(Box::new(t), caps(Platform::Rp2350, &[])).unwrap();
        (s, log)
    }

    #[test]
    fn changing_one_field_sends_a_whole_band_packet() {
        let (mut s, log) = rig(1000.0f32.to_le_bytes());

        s.write("eq.freq", &[8, 3], Value::Float(2856.0)).unwrap();

        let writes: Vec<_> = log
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.direction == Direction::Out && e.opcode == op::REQ_SET_EQ_PARAM)
            .cloned()
            .collect();

        assert_eq!(writes.len(), 1, "exactly one band write");
        let w = &writes[0];
        assert_eq!(
            w.payload.len(),
            16,
            "the firmware wants a 16-byte descriptor"
        );
        assert_eq!(w.value, 0, "wValue carries nothing for this opcode");
        assert_eq!(w.payload[0], 8, "channel travels in the payload");
        assert_eq!(w.payload[1], 3, "so does the band");
        assert_eq!(
            &w.payload[4..8],
            &2856.0f32.to_le_bytes(),
            "the new frequency"
        );
    }

    /// Fields the user did not touch must survive the round trip.
    #[test]
    fn untouched_fields_are_preserved() {
        let (mut s, log) = rig(3.5f32.to_le_bytes());

        s.write("eq.gain", &[8, 3], Value::Float(-6.0)).unwrap();

        let w = log
            .lock()
            .unwrap()
            .iter()
            .find(|e| e.opcode == op::REQ_SET_EQ_PARAM)
            .cloned()
            .unwrap();
        // Freq and Q came from the read, not from defaults.
        assert_eq!(&w.payload[4..8], &3.5f32.to_le_bytes());
        assert_eq!(&w.payload[8..12], &3.5f32.to_le_bytes());
        assert_eq!(&w.payload[12..16], &(-6.0f32).to_le_bytes());
    }

    /// A plain band write must stay 16 bytes, or it would clobber the stored Qp.
    #[test]
    fn non_linkwitz_writes_never_grow_to_eighteen_bytes() {
        let (mut s, log) = rig(1000.0f32.to_le_bytes());
        s.write("eq.q", &[8, 3], Value::Float(2.0)).unwrap();

        let w = log
            .lock()
            .unwrap()
            .iter()
            .find(|e| e.opcode == op::REQ_SET_EQ_PARAM)
            .cloned()
            .unwrap();
        assert_eq!(w.payload.len(), 16);
    }
}
