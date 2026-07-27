//! The `WireBulkParams` codec.
//!
//! # The compatibility rule
//!
//! `bulk_params.h` broke backward compatibility deliberately at V16 and states
//! that `bulk_params_apply()` **rejects any payload whose `format_version` is
//! not current or whose length is not exactly `sizeof(WireBulkParams)`**.
//!
//! So there is no "send a shorter prefix and let the rest default" path, and no
//! forward compatibility on write. A host may only bulk-write a packet that
//! exactly matches the device's version and size. Writing a guessed layout would
//! rewrite every parameter on the device at once, so this module refuses rather
//! than guesses: [`BulkPacket::decode`] rejects an unknown version outright, and
//! callers fall back to individual `SET_*` opcodes, which are version
//! independent.

use crate::ChannelMap;
use crate::generated::{self, wire::WIRE_FORMAT_VERSION};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WireError {
    #[error(
        "bulk packet is {got} bytes, expected {expected}; \
         the device's wire format does not match this build"
    )]
    WrongSize { got: usize, expected: usize },

    #[error(
        "device reports wire format V{got}, this build implements V{expected}; \
         refusing to interpret the payload. Use individual SET_*/GET_* commands, \
         which are version independent."
    )]
    UnsupportedVersion { got: u8, expected: u8 },

    #[error("header is inconsistent: {inputs} inputs + {outputs} outputs != {channels} channels")]
    InconsistentHeader {
        inputs: u8,
        outputs: u8,
        channels: u8,
    },

    #[error("payload_length field says {stated} but the transfer carried {actual} bytes")]
    LengthMismatch { stated: usize, actual: usize },
}

/// The 16-byte packet header. Authoritative for everything that follows: no
/// offset in the packet may be interpreted before this validates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct WireHeader {
    pub format_version: u8,
    pub platform_id: u8,
    pub num_channels: u8,
    pub num_output_channels: u8,
    pub num_input_channels: u8,
    pub max_bands: u8,
    pub payload_length: u16,
    pub fw_version_major: u16,
    pub fw_version_minor: u16,
}

impl WireHeader {
    pub const LEN: usize = 16;

    /// Parse and validate the header alone.
    ///
    /// This is deliberately callable on a short read: a host can fetch the first
    /// 16 bytes, learn the version, and decide whether a full read is safe.
    pub fn decode(buf: &[u8]) -> Result<Self, WireError> {
        if buf.len() < Self::LEN {
            return Err(WireError::WrongSize {
                got: buf.len(),
                expected: Self::LEN,
            });
        }

        let h = Self {
            format_version: buf[0],
            platform_id: buf[1],
            num_channels: buf[2],
            num_output_channels: buf[3],
            num_input_channels: buf[4],
            max_bands: buf[5],
            payload_length: u16::from_le_bytes([buf[6], buf[7]]),
            fw_version_major: u16::from_le_bytes([buf[8], buf[9]]),
            fw_version_minor: u16::from_le_bytes([buf[10], buf[11]]),
        };

        if h.format_version != WIRE_FORMAT_VERSION as u8 {
            return Err(WireError::UnsupportedVersion {
                got: h.format_version,
                expected: WIRE_FORMAT_VERSION as u8,
            });
        }

        if h.num_input_channels as u16 + h.num_output_channels as u16 != h.num_channels as u16 {
            return Err(WireError::InconsistentHeader {
                inputs: h.num_input_channels,
                outputs: h.num_output_channels,
                channels: h.num_channels,
            });
        }

        Ok(h)
    }

    /// Firmware version as `(major, minor, patch)`, decoding the BCD-ish minor
    /// field the device reports.
    pub fn firmware(&self) -> (u16, u16) {
        (self.fw_version_major, self.fw_version_minor)
    }

    pub fn platform(&self) -> crate::Platform {
        crate::Platform::from_id(self.platform_id)
    }

    pub fn channel_map(&self) -> Option<ChannelMap> {
        ChannelMap::new(
            self.num_input_channels,
            self.num_output_channels,
            self.num_channels,
        )
    }
}

/// A validated full bulk snapshot.
///
/// Holds the raw bytes so that a read-modify-write cycle preserves every field
/// this build does not model, including any the firmware added since. Accessors
/// decode on demand from the generated offset table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BulkPacket {
    header: WireHeader,
    raw: Vec<u8>,
}

impl BulkPacket {
    /// Validate and take ownership of a full snapshot.
    pub fn decode(raw: Vec<u8>) -> Result<Self, WireError> {
        let header = WireHeader::decode(&raw)?;

        if raw.len() != generated::BULK_SIZE {
            return Err(WireError::WrongSize {
                got: raw.len(),
                expected: generated::BULK_SIZE,
            });
        }

        // The device states its own length; disagreement means a torn read.
        if header.payload_length as usize != raw.len() {
            return Err(WireError::LengthMismatch {
                stated: header.payload_length as usize,
                actual: raw.len(),
            });
        }

        Ok(Self { header, raw })
    }

    pub fn header(&self) -> &WireHeader {
        &self.header
    }

    /// The bytes exactly as received, for a faithful read-modify-write.
    pub fn as_bytes(&self) -> &[u8] {
        &self.raw
    }

    /// Borrow one section's bytes by its generated offset and length.
    pub fn section(&self, name: &str) -> Option<&[u8]> {
        generated::SECTIONS
            .iter()
            .find(|(n, _, _)| *n == name)
            .map(|(_, off, len)| &self.raw[*off..*off + *len])
    }

    /// Which section a byte offset falls in. This is how a `PARAM_CHANGED`
    /// notification, which carries only an offset, is routed to a field.
    pub fn section_at(offset: usize) -> Option<&'static str> {
        generated::SECTIONS
            .iter()
            .find(|(_, off, len)| offset >= *off && offset < *off + *len)
            .map(|(name, _, _)| *name)
    }

    pub fn channel_map(&self) -> Option<ChannelMap> {
        self.header.channel_map()
    }
}

/// Decode a `+1`-encoded optional byte.
///
/// Several V21-V24 fields were added by claiming reserved bytes, using
/// `0 = absent, keep the live value`. Where `0` is itself meaningful the field
/// is stored plus one. Reading such a field without subtracting reports the
/// wrong value; writing a plain `0` reads as "absent" rather than as the value
/// zero. See `docs/wire-format.md` section 5.
#[inline]
pub fn decode_p1(raw: u8) -> Option<u8> {
    raw.checked_sub(1)
}

/// Encode an optional value into a `+1` field. `None` means "leave the device's
/// live value alone".
#[inline]
pub fn encode_p1(value: Option<u8>) -> u8 {
    match value {
        Some(v) => v.saturating_add(1),
        None => 0,
    }
}

/// Decode a field using the plain `0 = absent` convention (no `+1`), used where
/// zero is not a legal value, such as a GPIO pin or a channel count.
#[inline]
pub fn decode_absent_zero(raw: u8) -> Option<u8> {
    (raw != 0).then_some(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_bytes(version: u8, inputs: u8, outputs: u8, channels: u8, len: u16) -> Vec<u8> {
        let mut b = vec![0u8; generated::BULK_SIZE];
        b[0] = version;
        b[1] = 1; // RP2350
        b[2] = channels;
        b[3] = outputs;
        b[4] = inputs;
        b[5] = 12;
        b[6..8].copy_from_slice(&len.to_le_bytes());
        b[8..10].copy_from_slice(&1u16.to_le_bytes());
        b[10..12].copy_from_slice(&1u16.to_le_bytes());
        b
    }

    fn valid() -> Vec<u8> {
        header_bytes(26, 8, 9, 17, generated::BULK_SIZE as u16)
    }

    #[test]
    fn decodes_a_valid_packet() {
        let p = BulkPacket::decode(valid()).unwrap();
        assert_eq!(p.header().num_channels, 17);
        let map = p.channel_map().unwrap();
        assert_eq!(map.channel_of_output(0), Some(8));
    }

    /// The firmware rejects a mismatched version outright, so we must never
    /// interpret one. Guessing here would rewrite all device state at once.
    #[test]
    fn refuses_an_unknown_version_rather_than_guessing() {
        let older = header_bytes(14, 2, 5, 7, generated::BULK_SIZE as u16);
        assert_eq!(
            BulkPacket::decode(older),
            Err(WireError::UnsupportedVersion {
                got: 14,
                expected: 26
            })
        );

        let newer = header_bytes(27, 8, 9, 17, generated::BULK_SIZE as u16);
        assert!(matches!(
            BulkPacket::decode(newer),
            Err(WireError::UnsupportedVersion { got: 27, .. })
        ));
    }

    #[test]
    fn rejects_a_short_transfer() {
        let mut short = valid();
        short.truncate(3000);
        assert!(matches!(
            BulkPacket::decode(short),
            Err(WireError::WrongSize { .. })
        ));
    }

    #[test]
    fn rejects_an_inconsistent_header() {
        let bad = header_bytes(26, 8, 9, 11, generated::BULK_SIZE as u16);
        assert_eq!(
            BulkPacket::decode(bad),
            Err(WireError::InconsistentHeader {
                inputs: 8,
                outputs: 9,
                channels: 11
            })
        );
    }

    #[test]
    fn detects_a_torn_read_via_payload_length() {
        let bad = header_bytes(26, 8, 9, 17, 3664);
        assert_eq!(
            BulkPacket::decode(bad),
            Err(WireError::LengthMismatch {
                stated: 3664,
                actual: 5944
            })
        );
    }

    #[test]
    fn round_trips_bytes_untouched() {
        let raw = valid();
        let p = BulkPacket::decode(raw.clone()).unwrap();
        assert_eq!(p.as_bytes(), &raw[..]);
    }

    #[test]
    fn maps_notification_offsets_to_sections() {
        assert_eq!(BulkPacket::section_at(0), Some("header"));
        assert_eq!(BulkPacket::section_at(16), Some("global"));
        assert_eq!(BulkPacket::section_at(824), Some("eq"));
        assert_eq!(BulkPacket::section_at(5900), Some("upmix"));
        assert_eq!(BulkPacket::section_at(generated::BULK_SIZE), None);
    }

    #[test]
    fn sections_are_addressable_by_name() {
        let p = BulkPacket::decode(valid()).unwrap();
        assert_eq!(p.section("eq").unwrap().len(), 3264);
        assert_eq!(p.section("upmix").unwrap().len(), 44);
        assert!(p.section("nonexistent").is_none());
    }

    /// A plain zero means "absent", not the value zero. Getting this backwards
    /// silently disables S/PDIF inputs or flips the I2S clock pin mode.
    #[test]
    fn p1_sentinels_distinguish_absent_from_zero() {
        assert_eq!(decode_p1(0), None); // absent, keep live
        assert_eq!(decode_p1(1), Some(0)); // the value zero
        assert_eq!(decode_p1(2), Some(1));

        assert_eq!(encode_p1(None), 0);
        assert_eq!(encode_p1(Some(0)), 1);
        assert_eq!(encode_p1(Some(1)), 2);

        for v in 0u8..=200 {
            assert_eq!(decode_p1(encode_p1(Some(v))), Some(v));
        }
    }

    #[test]
    fn absent_zero_fields_have_no_offset() {
        assert_eq!(decode_absent_zero(0), None);
        assert_eq!(decode_absent_zero(6), Some(6));
    }
}
