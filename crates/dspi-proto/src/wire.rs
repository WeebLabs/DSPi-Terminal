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
use crate::enums::FilterType;
use crate::generated::{self, wire::WIRE_FORMAT_VERSION};
use crate::value::{EqParamPacket, decode_qp};

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

    /// Section 15, decoded with its `+1` sentinels resolved.
    pub fn input_config(&self) -> Option<InputConfig> {
        InputConfig::decode(self.section("input_config")?)
    }

    /// One EQ band, decoded from the snapshot.
    ///
    /// The `eq` section is the same table `REQ_GET_EQ_PARAM` answers from, so
    /// this is the whole-table read the protocol otherwise lacks: one bulk
    /// transfer instead of five scalar ones per band. `None` for an index the
    /// wire format has no room for.
    pub fn band(&self, channel: u8, band: u8) -> Option<EqParamPacket> {
        self.band_in("eq", generated::wire::WIRE_MAX_BANDS, channel, band)
    }

    /// One crossover band. The `crossovers` section mirrors `eq` exactly, but
    /// four columns wide rather than twelve.
    pub fn xover_band(&self, channel: u8, band: u8) -> Option<EqParamPacket> {
        self.band_in(
            "crossovers",
            generated::wire::WIRE_MAX_XOVER_BANDS,
            channel,
            band,
        )
    }

    fn band_in(&self, section: &str, stride: u16, channel: u8, band: u8) -> Option<EqParamPacket> {
        if channel as u16 >= generated::wire::WIRE_MAX_CHANNELS || band as u16 >= stride {
            return None;
        }
        let table = self.section(section)?;
        let at = (channel as usize * stride as usize + band as usize) * WIRE_BAND_SIZE;
        let b: &[u8; WIRE_BAND_SIZE] = table.get(at..at + WIRE_BAND_SIZE)?.try_into().ok()?;

        let filter_type = FilterType::from_raw(b[0]);
        let f32_at = |o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        Some(EqParamPacket {
            channel,
            band,
            filter_type,
            // The firmware's rule is "1 means bypassed", not "non-zero".
            bypass: b[1] == 1,
            freq: f32_at(4),
            q: f32_at(8),
            gain_db: f32_at(12),
            // The reserved pair carries the Linkwitz target Q and nothing else,
            // so reading it on another type would report a Q that is not there.
            qp: filter_type
                .is_linkwitz()
                .then(|| decode_qp(u16::from_le_bytes([b[2], b[3]]))),
        })
    }
}

/// `WireBandParams`: type, bypass, two reserved bytes, then three floats.
const WIRE_BAND_SIZE: usize = 16;

/// Byte offsets inside section 15, `WireInputConfig`, at wire **V28**.
///
/// This section is the one place where the packet changed shape at V28 without
/// changing size: `spdif_rx_pin_ext` grew from two entries to three, so every
/// field below it moved down one byte and the section's last reserved byte was
/// consumed (bulk_params.h:203-233). `BULK_SIZE` is unchanged at 5944, so no
/// size check can catch this: only these offsets can. A host that version-gates
/// on packet size alone reads `i2s_clock_mode` where `spdif_rx_enabled_ext_p1`
/// now lives and silently disables the extra S/PDIF inputs.
///
/// `(name, offset from the section start, length)`.
pub const INPUT_CONFIG_FIELDS: [(&str, usize, usize); 12] = [
    ("input_source", 0, 1),
    ("spdif_rx_pin", 1, 1),
    ("i2s_rx_pin", 2, 1),
    ("i2s_input_rate", 3, 1),
    ("i2s_input_channels", 4, 1),
    ("i2s_rx_pin_ext", 5, 3),
    // V28: was [2] at offset 8..10.
    ("spdif_rx_pin_ext", 8, 3),
    ("spdif_rx_enabled_ext_p1", 11, 1),
    ("i2s_clock_mode", 12, 1),
    ("adat_input_pin", 13, 1),
    ("adat_input_enabled_p1", 14, 1),
    ("adat_clock_mode_p1", 15, 1),
];

/// Section 15 decoded, with every `+1` sentinel resolved.
///
/// `None` on an optional field means "absent, keep the device's live value", and
/// is not the same as the value zero: writing a plain `0` into one of the `_p1`
/// fields reads back as "absent" rather than as "disabled" or "master". See
/// `docs/wire-format.md` section 5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct InputConfig {
    pub input_source: u8,
    pub spdif_rx_pin: u8,
    pub i2s_rx_pin: u8,
    pub i2s_input_rate: u8,
    /// Active I2S input channels: 2, 4, 6 or 8. `None` when absent.
    pub i2s_input_channels: Option<u8>,
    /// Data GPIO for I2S stereo pairs 1..3. `None` where unset.
    pub i2s_rx_pin_ext: [Option<u8>; 3],
    /// GPIO for S/PDIF inputs 2..4. Three entries from V28; two before it.
    pub spdif_rx_pin_ext: [Option<u8>; 3],
    /// Enable mask for S/PDIF inputs 2..4, bit 0 = input 2.
    pub spdif_rx_enabled_ext: Option<u8>,
    /// 0 = master, 1 = slave. A plain byte, not a `+1` field: a pre-V21 reader
    /// sees zero here, which is the correct legacy default.
    pub i2s_clock_mode: u8,
    pub adat_input_pin: Option<u8>,
    pub adat_input_enabled: Option<bool>,
    /// 0 = master, 1 = slave.
    pub adat_clock_mode: Option<u8>,
}

impl InputConfig {
    pub const LEN: usize = 16;

    /// Decode the section's 16 bytes. `None` if the slice is short.
    pub fn decode(b: &[u8]) -> Option<Self> {
        let b: &[u8; Self::LEN] = b.get(..Self::LEN)?.try_into().ok()?;
        Some(Self {
            input_source: b[0],
            spdif_rx_pin: b[1],
            i2s_rx_pin: b[2],
            i2s_input_rate: b[3],
            i2s_input_channels: decode_absent_zero(b[4]),
            i2s_rx_pin_ext: [
                decode_absent_zero(b[5]),
                decode_absent_zero(b[6]),
                decode_absent_zero(b[7]),
            ],
            spdif_rx_pin_ext: [
                decode_absent_zero(b[8]),
                decode_absent_zero(b[9]),
                decode_absent_zero(b[10]),
            ],
            spdif_rx_enabled_ext: decode_p1(b[11]),
            i2s_clock_mode: b[12],
            adat_input_pin: decode_absent_zero(b[13]),
            adat_input_enabled: decode_p1(b[14]).map(|v| v != 0),
            adat_clock_mode: decode_p1(b[15]),
        })
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
        header_bytes(28, 8, 9, 17, generated::BULK_SIZE as u16)
    }

    /// One `WireBandParams`: type, bypass, the reserved pair, then the floats.
    fn band_bytes(ty: u8, bypass: u8, freq: f32, q: f32, gain: f32, reserved: u16) -> [u8; 16] {
        let mut b = [0u8; 16];
        b[0] = ty;
        b[1] = bypass;
        b[2..4].copy_from_slice(&reserved.to_le_bytes());
        b[4..8].copy_from_slice(&freq.to_le_bytes());
        b[8..12].copy_from_slice(&q.to_le_bytes());
        b[12..16].copy_from_slice(&gain.to_le_bytes());
        b
    }

    /// Place a band by the row-major rule the header states: channel 0 bands
    /// 0..n-1, then channel 1, and so on.
    fn put_band(raw: &mut [u8], section: &str, stride: usize, at: (usize, usize), b: [u8; 16]) {
        let (_, off, _) = generated::SECTIONS
            .iter()
            .find(|(n, _, _)| *n == section)
            .unwrap();
        let start = off + (at.0 * stride + at.1) * WIRE_BAND_SIZE;
        raw[start..start + WIRE_BAND_SIZE].copy_from_slice(&b);
    }

    /// The EQ table is row-major, so an off-by-one in the stride reads a
    /// neighbouring channel's band and looks entirely plausible.
    #[test]
    fn bands_decode_from_their_own_row() {
        let mut raw = valid();
        put_band(
            &mut raw,
            "eq",
            12,
            (0, 0),
            band_bytes(1, 0, 105.0, 0.707, 8.8, 0),
        );
        put_band(
            &mut raw,
            "eq",
            12,
            (3, 7),
            band_bytes(2, 1, 2856.0, 3.58, -8.6, 0),
        );
        let p = BulkPacket::decode(raw).unwrap();

        let first = p.band(0, 0).unwrap();
        assert_eq!((first.channel, first.band), (0, 0));
        assert_eq!(first.freq, 105.0);
        assert!(!first.bypass);

        let other = p.band(3, 7).unwrap();
        assert_eq!(other.freq, 2856.0);
        assert_eq!(other.gain_db, -8.6);
        assert!(other.bypass, "the bypass byte was 1");

        // A slot nobody wrote reads as zeroes, not as its neighbour.
        assert_eq!(p.band(3, 8).unwrap().freq, 0.0);
        assert_eq!(p.band(4, 0).unwrap().freq, 0.0);
    }

    /// `Qp` rides in the reserved pair and means nothing on other types, so
    /// reporting it everywhere would invent a Q that is not there.
    #[test]
    fn the_linkwitz_sidecar_is_read_only_for_linkwitz() {
        let mut raw = valid();
        let lt = FilterType::LinkwitzTransform.to_raw();
        put_band(
            &mut raw,
            "eq",
            12,
            (1, 0),
            band_bytes(lt, 0, 30.0, 0.707, 0.0, 512),
        );
        // Same reserved bytes, ordinary type.
        put_band(
            &mut raw,
            "eq",
            12,
            (1, 1),
            band_bytes(2, 0, 30.0, 0.707, 0.0, 512),
        );
        let p = BulkPacket::decode(raw).unwrap();

        assert_eq!(p.band(1, 0).unwrap().qp, Some(1.0));
        assert_eq!(p.band(1, 1).unwrap().qp, None);
    }

    /// Crossover rows are four wide where EQ rows are twelve. Sharing the
    /// stride would put every channel but the first in the wrong place.
    #[test]
    fn crossover_bands_use_their_own_stride() {
        let mut raw = valid();
        put_band(
            &mut raw,
            "crossovers",
            4,
            (2, 1),
            band_bytes(5, 0, 80.0, 0.707, 0.0, 0),
        );
        let p = BulkPacket::decode(raw).unwrap();

        assert_eq!(p.xover_band(2, 1).unwrap().freq, 80.0);
        assert_eq!(p.xover_band(2, 0).unwrap().freq, 0.0);
        // Four columns, so there is no band 4 to ask for.
        assert_eq!(p.xover_band(2, 4), None);
    }

    #[test]
    fn a_band_outside_the_wire_array_is_none() {
        let p = BulkPacket::decode(valid()).unwrap();
        assert_eq!(p.band(17, 0), None, "only 17 channels, indexed 0..16");
        assert_eq!(p.band(0, 12), None, "only 12 slots per channel");
        assert!(p.band(16, 11).is_some(), "the last slot is addressable");
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
                expected: 28
            })
        );

        let newer = header_bytes(29, 8, 9, 17, generated::BULK_SIZE as u16);
        assert!(matches!(
            BulkPacket::decode(newer),
            Err(WireError::UnsupportedVersion { got: 29, .. })
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
        let bad = header_bytes(28, 8, 9, 11, generated::BULK_SIZE as u16);
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
        let bad = header_bytes(28, 8, 9, 17, 3664);
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

    /// V28 is the change no size check can catch: the section is still 16 bytes
    /// and `BULK_SIZE` is still 5944, but `spdif_rx_pin_ext` grew from two
    /// entries to three and pushed the five fields below it down one byte
    /// (bulk_params.h:203-233). Every offset is pinned by name here, because
    /// getting one wrong reads a GPIO as a clock mode and looks plausible.
    #[test]
    fn the_v28_input_config_offsets_are_where_the_header_puts_them() {
        let expect: &[(&str, usize, usize)] = &[
            ("input_source", 0, 1),
            ("spdif_rx_pin", 1, 1),
            ("i2s_rx_pin", 2, 1),
            ("i2s_input_rate", 3, 1),
            ("i2s_input_channels", 4, 1),
            ("i2s_rx_pin_ext", 5, 3),
            ("spdif_rx_pin_ext", 8, 3),
            ("spdif_rx_enabled_ext_p1", 11, 1),
            ("i2s_clock_mode", 12, 1),
            ("adat_input_pin", 13, 1),
            ("adat_input_enabled_p1", 14, 1),
            ("adat_clock_mode_p1", 15, 1),
        ];
        assert_eq!(INPUT_CONFIG_FIELDS.as_slice(), expect);

        // The section is exactly full: no reserved byte is left at V28.
        let (_, last_off, last_len) = INPUT_CONFIG_FIELDS[INPUT_CONFIG_FIELDS.len() - 1];
        assert_eq!(last_off + last_len, InputConfig::LEN);
        assert_eq!(InputConfig::LEN, generated::LEN_INPUT_CONFIG);
    }

    /// Each field is decoded from its own byte. A distinctive value per offset
    /// means a one-byte slip shows up as the wrong field, not as a near miss.
    #[test]
    fn every_input_config_field_decodes_from_its_own_byte() {
        let mut raw = valid();
        let off = generated::OFF_INPUT_CONFIG;
        let bytes: [u8; 16] = [
            2, // input_source = I2S
            5, // spdif_rx_pin
            1, // i2s_rx_pin
            2, // i2s_input_rate = 96 kHz
            8, // i2s_input_channels
            2, 3, 4, // i2s_rx_pin_ext[3]
            20, 21, 22, // spdif_rx_pin_ext[3], three entries from V28
            8,  // spdif_rx_enabled_ext_p1: +1, so the mask is 7
            1,  // i2s_clock_mode = slave, a plain byte with no +1
            9,  // adat_input_pin
            2,  // adat_input_enabled_p1: +1, so enabled
            2,  // adat_clock_mode_p1: +1, so slave
        ];
        raw[off..off + 16].copy_from_slice(&bytes);

        let c = BulkPacket::decode(raw).unwrap().input_config().unwrap();
        assert_eq!(c.input_source, 2);
        assert_eq!(c.spdif_rx_pin, 5);
        assert_eq!(c.i2s_rx_pin, 1);
        assert_eq!(c.i2s_input_rate, 2);
        assert_eq!(c.i2s_input_channels, Some(8));
        assert_eq!(c.i2s_rx_pin_ext, [Some(2), Some(3), Some(4)]);
        assert_eq!(
            c.spdif_rx_pin_ext,
            [Some(20), Some(21), Some(22)],
            "V28 carries a pin for S/PDIF 4"
        );
        assert_eq!(c.spdif_rx_enabled_ext, Some(7));
        assert_eq!(c.i2s_clock_mode, 1);
        assert_eq!(c.adat_input_pin, Some(9));
        assert_eq!(c.adat_input_enabled, Some(true));
        assert_eq!(c.adat_clock_mode, Some(1));
    }

    /// An all-zero section means "absent everywhere", not "disabled and
    /// master": the `+1` fields must not read their sentinel as a value.
    #[test]
    fn an_untouched_input_config_is_absent_not_zero() {
        let c = BulkPacket::decode(valid()).unwrap().input_config().unwrap();
        assert_eq!(c.spdif_rx_enabled_ext, None);
        assert_eq!(c.adat_input_enabled, None);
        assert_eq!(c.adat_clock_mode, None);
        assert_eq!(c.i2s_input_channels, None);
        assert_eq!(c.spdif_rx_pin_ext, [None, None, None]);
        // i2s_clock_mode has no sentinel, and zero is master.
        assert_eq!(c.i2s_clock_mode, 0);
    }

    /// The pre-V28 reading of the same bytes, to show what the shift costs:
    /// byte 11 used to be `i2s_clock_mode`, so a stale decoder reports the
    /// S/PDIF enable mask as a clock mode and vice versa.
    #[test]
    fn the_v28_shift_would_be_invisible_to_a_size_check() {
        assert_eq!(generated::BULK_SIZE, 5944, "unchanged from V26");
        assert_eq!(generated::LEN_INPUT_CONFIG, 16, "unchanged from V26");

        let field = |name: &str| {
            INPUT_CONFIG_FIELDS
                .iter()
                .find(|(n, _, _)| *n == name)
                .unwrap()
        };
        assert_eq!(field("spdif_rx_enabled_ext_p1").1, 11, "was 10 at V26");
        assert_eq!(field("i2s_clock_mode").1, 12, "was 11 at V26");
        assert_eq!(field("adat_clock_mode_p1").1, 15, "was 14 at V26");
    }
}
