//! Parameter values, and how they turn into wire bytes.
//!
//! One `Value` type covers every parameter in the registry. Encoding lives here
//! rather than at call sites so that a parameter's wire representation is stated
//! exactly once.

use crate::enums::FilterType;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ValueError {
    #[error("expected {expected}, got {got}")]
    WrongType {
        expected: &'static str,
        got: &'static str,
    },

    #[error("{value} is outside the allowed range {min} to {max}{}", unit_suffix(*unit))]
    OutOfRange {
        value: f64,
        min: f64,
        max: f64,
        unit: Unit,
    },

    #[error("`{0}` is not one of the allowed values")]
    NotAVariant(String),

    #[error("text is {got} bytes, the field holds at most {max}")]
    TextTooLong { got: usize, max: usize },
}

fn unit_suffix(u: Unit) -> &'static str {
    match u {
        Unit::None => "",
        Unit::Db => " dB",
        Unit::Hz => " Hz",
        Unit::Q => "",
        Unit::Percent => " %",
        Unit::Ms => " ms",
        Unit::Spl => " dB SPL",
        Unit::Gpio => " (GPIO)",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum Unit {
    None,
    Db,
    Hz,
    Q,
    Percent,
    Ms,
    Spl,
    Gpio,
}

impl Unit {
    pub fn suffix(self) -> &'static str {
        unit_suffix(self)
    }

    /// Whether this quantity is perceived logarithmically, which decides how a
    /// knob or an arrow key steps through it. Stepping frequency linearly makes
    /// the bass end unusable and the treble end uselessly fine.
    pub fn is_logarithmic(self) -> bool {
        matches!(self, Unit::Hz | Unit::Q)
    }
}

/// A parameter value, independent of how it is carried on the wire.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(untagged))]
pub enum Value {
    Bool(bool),
    Float(f32),
    Int(i64),
    /// A choice, carried as its wire number. The label is resolved through the
    /// parameter's descriptor, not stored here.
    Choice(u8),
    Mask(u32),
    Text(String),
    /// A structured payload this layer does not model field by field.
    Bytes(Vec<u8>),
    /// An action with no value.
    Trigger,
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Bool(_) => "a yes/no value",
            Value::Float(_) => "a number",
            Value::Int(_) => "a whole number",
            Value::Choice(_) => "a choice",
            Value::Mask(_) => "a bit mask",
            Value::Text(_) => "text",
            Value::Bytes(_) => "raw bytes",
            Value::Trigger => "an action",
        }
    }

    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Int(i) => Some(*i as f32),
            Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Value::Choice(c) => Some(*c as f32),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            Value::Int(i) => Some(*i != 0),
            Value::Choice(c) => Some(*c != 0),
            _ => None,
        }
    }

    pub fn as_u8(&self) -> Option<u8> {
        match self {
            Value::Choice(c) => Some(*c),
            Value::Int(i) if (0..=255).contains(i) => Some(*i as u8),
            Value::Bool(b) => Some(*b as u8),
            _ => None,
        }
    }

    /// Render for display, with the parameter's unit.
    pub fn display(&self, unit: Unit) -> String {
        match self {
            Value::Bool(b) => (if *b { "on" } else { "off" }).into(),
            Value::Float(f) => match unit {
                // Gain reads better signed: "+3.0 dB" versus "3.0 dB".
                Unit::Db | Unit::Spl => format!("{f:+.1}{}", unit.suffix()),
                Unit::Hz => {
                    if *f >= 1000.0 {
                        format!("{:.2} kHz", f / 1000.0)
                    } else {
                        format!("{f:.0} Hz")
                    }
                }
                Unit::Q => format!("{f:.2}"),
                _ => format!("{f:.1}{}", unit.suffix()),
            },
            Value::Int(i) => format!("{i}{}", unit.suffix()),
            Value::Choice(c) => format!("{c}"),
            Value::Mask(m) => format!("0x{m:04X}"),
            Value::Text(t) => t.clone(),
            Value::Bytes(b) => format!("{} bytes", b.len()),
            Value::Trigger => "trigger".into(),
        }
    }
}

/// How a value is laid out in a control transfer's data stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repr {
    /// Single byte, 0 or 1.
    Bool8,
    /// IEEE 754 single precision, little endian.
    F32,
    U8,
    U16Le,
    U32Le,
    /// Unsigned 8.8 fixed point, little endian: 1.0 is 256. The auxiliary
    /// output level is a percentage carried this way (config.h:141-145).
    U16Q8,
    /// NUL-padded text of a fixed width.
    Text(usize),
    /// Opaque, already encoded by a dedicated codec.
    Raw,
    /// No data stage at all.
    None,
}

impl Repr {
    pub fn len(self) -> usize {
        match self {
            Repr::Bool8 | Repr::U8 => 1,
            Repr::U16Le | Repr::U16Q8 => 2,
            Repr::F32 | Repr::U32Le => 4,
            Repr::Text(n) => n,
            Repr::Raw | Repr::None => 0,
        }
    }

    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    pub fn encode(self, v: &Value) -> Result<Vec<u8>, ValueError> {
        Ok(match (self, v) {
            (Repr::None, _) => Vec::new(),
            (Repr::Bool8, _) => vec![v.as_bool().ok_or(mismatch("a yes/no value", v))? as u8],
            (Repr::F32, Value::Mask(m)) => (*m as f32).to_le_bytes().to_vec(),
            (Repr::F32, _) => v
                .as_f32()
                .ok_or(mismatch("a number", v))?
                .to_le_bytes()
                .to_vec(),
            (Repr::U16Q8, _) => {
                let f = v.as_f32().ok_or(mismatch("a number", v))?;
                ((f * 256.0).round().clamp(0.0, u16::MAX as f32) as u16)
                    .to_le_bytes()
                    .to_vec()
            }
            (Repr::U8, Value::Mask(m)) => {
                vec![u8::try_from(*m).map_err(|_| mismatch("a mask 0x0-0xFF", v))?]
            }
            (Repr::U8, _) => vec![v.as_u8().ok_or(mismatch("a whole number 0-255", v))?],
            (Repr::U16Le, Value::Mask(m)) => (*m as u16).to_le_bytes().to_vec(),
            (Repr::U16Le, _) => (v.as_f32().ok_or(mismatch("a whole number", v))? as u16)
                .to_le_bytes()
                .to_vec(),
            (Repr::U32Le, Value::Mask(m)) => m.to_le_bytes().to_vec(),
            (Repr::U32Le, _) => (v.as_f32().ok_or(mismatch("a whole number", v))? as u32)
                .to_le_bytes()
                .to_vec(),
            (Repr::Text(max), Value::Text(t)) => {
                let bytes = t.as_bytes();
                if bytes.len() > max {
                    return Err(ValueError::TextTooLong {
                        got: bytes.len(),
                        max,
                    });
                }
                let mut buf = vec![0u8; max];
                buf[..bytes.len()].copy_from_slice(bytes);
                buf
            }
            (Repr::Text(_), _) => return Err(mismatch("text", v)),
            (Repr::Raw, Value::Bytes(b)) => b.clone(),
            (Repr::Raw, _) => return Err(mismatch("raw bytes", v)),
        })
    }

    pub fn decode(self, bytes: &[u8]) -> Option<Value> {
        Some(match self {
            Repr::None => Value::Trigger,
            Repr::Bool8 => Value::Bool(*bytes.first()? != 0),
            Repr::U8 => Value::Int(*bytes.first()? as i64),
            Repr::U16Le => Value::Int(u16::from_le_bytes([*bytes.first()?, *bytes.get(1)?]) as i64),
            Repr::U16Q8 => {
                Value::Float(u16::from_le_bytes([*bytes.first()?, *bytes.get(1)?]) as f32 / 256.0)
            }
            Repr::U32Le => Value::Int(u32::from_le_bytes([
                *bytes.first()?,
                *bytes.get(1)?,
                *bytes.get(2)?,
                *bytes.get(3)?,
            ]) as i64),
            Repr::F32 => Value::Float(f32::from_le_bytes([
                *bytes.first()?,
                *bytes.get(1)?,
                *bytes.get(2)?,
                *bytes.get(3)?,
            ])),
            Repr::Text(n) => Value::Text(
                String::from_utf8_lossy(bytes.get(..n.min(bytes.len()))?)
                    .trim_end_matches('\0')
                    .trim()
                    .to_string(),
            ),
            Repr::Raw => Value::Bytes(bytes.to_vec()),
        })
    }
}

fn mismatch(expected: &'static str, got: &Value) -> ValueError {
    ValueError::WrongType {
        expected,
        got: got.type_name(),
    }
}

/// The 16-byte `EqParamPacket` written by `SET_EQ_PARAM` (0x42).
///
/// An 18-byte variant carries the Linkwitz Transform's fourth parameter `Qp` as
/// `Q * 512`. A plain 16-byte write **preserves the stored `qp`**, so non-LT
/// writes never need the extra bytes and must not send them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EqParamPacket {
    pub channel: u8,
    pub band: u8,
    pub filter_type: FilterType,
    pub bypass: bool,
    pub freq: f32,
    pub q: f32,
    pub gain_db: f32,
    /// Only meaningful for the Linkwitz Transform.
    pub qp: Option<f32>,
}

impl EqParamPacket {
    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(18);
        b.push(self.channel);
        b.push(self.band);
        b.push(self.filter_type.to_raw());
        // Exactly 1 means bypassed; any other value means active.
        b.push(self.bypass as u8);
        b.extend_from_slice(&self.freq.to_le_bytes());
        b.extend_from_slice(&self.q.to_le_bytes());
        b.extend_from_slice(&self.gain_db.to_le_bytes());

        // Only extend to 18 bytes for a Linkwitz Transform band. Sending the
        // sidecar for any other type would overwrite the stored qp with zero.
        if self.filter_type.is_linkwitz()
            && let Some(qp) = self.qp
        {
            let encoded = (qp * 512.0).round().clamp(0.0, u16::MAX as f32) as u16;
            b.extend_from_slice(&encoded.to_le_bytes());
        }
        b
    }
}

/// Decode the `qp` sidecar: `Q * 512`, where 0 selects the 0.707 default.
pub fn decode_qp(raw: u16) -> f32 {
    if raw == 0 { 0.707 } else { raw as f32 / 512.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_round_trip_little_endian() {
        let v = Value::Float(2856.0);
        let bytes = Repr::F32.encode(&v).unwrap();
        assert_eq!(bytes, 2856.0f32.to_le_bytes());
        assert_eq!(Repr::F32.decode(&bytes), Some(v));
    }

    #[test]
    fn bools_are_single_bytes() {
        assert_eq!(Repr::Bool8.encode(&Value::Bool(true)).unwrap(), vec![1]);
        assert_eq!(Repr::Bool8.encode(&Value::Bool(false)).unwrap(), vec![0]);
        assert_eq!(Repr::Bool8.decode(&[1]), Some(Value::Bool(true)));
        // Any non-zero byte reads as true, matching the firmware's convention.
        assert_eq!(Repr::Bool8.decode(&[2]), Some(Value::Bool(true)));
    }

    #[test]
    fn text_is_nul_padded_to_the_field_width() {
        let b = Repr::Text(32)
            .encode(&Value::Text("Living Room".into()))
            .unwrap();
        assert_eq!(b.len(), 32);
        assert_eq!(&b[..11], b"Living Room");
        assert!(b[11..].iter().all(|&c| c == 0));
        assert_eq!(
            Repr::Text(32).decode(&b),
            Some(Value::Text("Living Room".into()))
        );
    }

    #[test]
    fn text_longer_than_the_field_is_refused_not_truncated() {
        let long = "x".repeat(40);
        assert_eq!(
            Repr::Text(32).encode(&Value::Text(long)),
            Err(ValueError::TextTooLong { got: 40, max: 32 })
        );
    }

    #[test]
    fn a_type_mismatch_says_what_was_expected() {
        let e = Repr::F32.encode(&Value::Text("hello".into())).unwrap_err();
        assert_eq!(
            e,
            ValueError::WrongType {
                expected: "a number",
                got: "text"
            }
        );
    }

    /// A 16-byte write preserves the device's stored qp. Padding every write out
    /// to 18 bytes would silently reset it on every ordinary filter edit.
    #[test]
    fn only_linkwitz_bands_carry_the_qp_sidecar() {
        let peaking = EqParamPacket {
            channel: 0,
            band: 3,
            filter_type: FilterType::Peaking,
            bypass: false,
            freq: 2856.0,
            q: 3.58,
            gain_db: -8.6,
            qp: Some(1.2),
        };
        assert_eq!(peaking.encode().len(), 16, "non-LT must stay 16 bytes");

        let lt = EqParamPacket {
            filter_type: FilterType::LinkwitzTransform,
            ..peaking
        };
        let bytes = lt.encode();
        assert_eq!(bytes.len(), 18);
        assert_eq!(
            u16::from_le_bytes([bytes[16], bytes[17]]),
            (1.2f32 * 512.0).round() as u16
        );
    }

    #[test]
    fn eq_packet_layout_matches_the_wire() {
        let p = EqParamPacket {
            channel: 8,
            band: 3,
            filter_type: FilterType::Peaking,
            bypass: true,
            freq: 1000.0,
            q: 0.707,
            gain_db: 3.0,
            qp: None,
        };
        let b = p.encode();
        assert_eq!(b[0], 8);
        assert_eq!(b[1], 3);
        assert_eq!(b[2], 1); // Peaking
        assert_eq!(b[3], 1); // bypassed
        assert_eq!(&b[4..8], &1000.0f32.to_le_bytes());
        assert_eq!(&b[8..12], &0.707f32.to_le_bytes());
        assert_eq!(&b[12..16], &3.0f32.to_le_bytes());
    }

    #[test]
    fn qp_zero_means_the_default_not_zero() {
        assert_eq!(decode_qp(0), 0.707);
        assert_eq!(decode_qp(512), 1.0);
    }

    #[test]
    fn frequency_and_q_step_logarithmically() {
        assert!(Unit::Hz.is_logarithmic());
        assert!(Unit::Q.is_logarithmic());
        assert!(!Unit::Db.is_logarithmic());
        assert!(!Unit::Ms.is_logarithmic());
    }

    #[test]
    fn display_is_readable_per_unit() {
        assert_eq!(Value::Float(3.0).display(Unit::Db), "+3.0 dB");
        assert_eq!(Value::Float(-8.6).display(Unit::Db), "-8.6 dB");
        assert_eq!(Value::Float(2856.0).display(Unit::Hz), "2.86 kHz");
        assert_eq!(Value::Float(64.0).display(Unit::Hz), "64 Hz");
        assert_eq!(Value::Float(3.58).display(Unit::Q), "3.58");
        assert_eq!(Value::Bool(true).display(Unit::None), "on");
    }
}
