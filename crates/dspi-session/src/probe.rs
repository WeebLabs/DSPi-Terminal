//! Capability discovery.
//!
//! Run once at connect, before anything else reads or writes. Everything about
//! the device's shape comes from here: channel counts, feature availability,
//! parameter ranges. Nothing downstream is allowed to assume a topology.
//!
//! Where a capability has no descriptor to read, we **probe and cache**: issue
//! the GET once and treat a stall as "this firmware does not have it". That is
//! more durable than comparing firmware versions, because it keeps working when
//! a feature is backported or a platform lacks it.

use dspi_proto::generated::opcodes as op;
use dspi_proto::wire::{BulkPacket, WireHeader};
use dspi_proto::{Platform, generated};
use dspi_transport::{Result, Transport, TransportError, with_busy_retry};

/// Everything discovered about a connected device.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Capabilities {
    pub serial: String,
    pub platform: Platform,
    pub firmware: String,
    pub wire_format: u8,
    pub num_channels: u8,
    pub num_inputs: u8,
    pub num_outputs: u8,
    pub max_bands: u8,
    pub channels: Vec<ChannelInfo>,
    /// Features answered a probe. Absent means this firmware stalled on it.
    pub features: Vec<Feature>,
    pub cs: Option<ControlSurfaceCaps>,
    pub siggen: Option<SiggenCaps>,
    pub active_preset: Option<u8>,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ChannelInfo {
    pub index: u8,
    pub name: String,
    pub slug: String,
    pub is_output: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Feature {
    pub name: String,
    pub present: bool,
    /// Why we concluded that, so a surprising answer is debuggable.
    pub evidence: String,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ControlSurfaceCaps {
    pub caps_version: u8,
    pub max_bindings: u8,
    pub type_count: u8,
    pub noun_count: u8,
    pub max_ir_commands: u8,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct SiggenCaps {
    pub version: u8,
    pub type_count: u8,
    pub output_channels: u8,
    pub multitone_max: u8,
    pub valid_channel_mask: u16,
}

/// Read the full bulk snapshot, chunked.
///
/// The packet is 5944 bytes, which exceeds WinUSB's 4 KB single-transfer cap, so
/// the chunked opcode is used on every platform rather than only on Windows.
/// One path, exercised everywhere. Offset 0 snapshots under the firmware's bulk
/// lock, so the read must start there and proceed in order.
pub fn read_bulk(t: &mut dyn Transport) -> Result<BulkPacket> {
    let total = generated::BULK_SIZE;
    let chunk = t.max_transfer().min(1024);
    let mut buf = Vec::with_capacity(total);

    while buf.len() < total {
        let offset = buf.len();
        let want = chunk.min(total - offset);
        let part = with_busy_retry(
            || t.control_in(op::REQ_GET_ALL_PARAMS_CHUNK, offset as u16, want as u16),
            op::REQ_GET_ALL_PARAMS_CHUNK,
        )?;
        buf.extend_from_slice(&part[..want.min(part.len())]);
    }

    BulkPacket::decode(buf).map_err(|e| TransportError::Usb(e.to_string()))
}

/// Ask the device what it is and what it can do.
pub fn probe(t: &mut dyn Transport) -> Result<Capabilities> {
    // GET_PLATFORM is the canonical liveness read; do it first so a dead or
    // wrongly-bound device fails here with a clear message rather than midway
    // through a 5944-byte transfer.
    let plat = with_busy_retry(
        || t.control_in(op::REQ_GET_PLATFORM, 0, 4),
        op::REQ_GET_PLATFORM,
    )?;
    let platform = Platform::from_id(plat[0]);
    // byte 1 = major, byte 2 = minor.patch packed BCD-style.
    let firmware = format!("{}.{}.{}", plat[1], plat[2] >> 4, plat[2] & 0x0F);

    let serial_raw = t.control_in(op::REQ_GET_SERIAL, 0, 16).unwrap_or_default();
    let serial = String::from_utf8_lossy(&serial_raw)
        .trim_end_matches('\0')
        .trim()
        .to_string();

    let bulk = read_bulk(t)?;
    let h: &WireHeader = bulk.header();

    let map = bulk
        .channel_map()
        .ok_or_else(|| TransportError::Usb("device reported an impossible channel map".into()))?;
    let map = map.with_names(channel_names(&bulk, h.num_channels));

    let channels = (0..h.num_channels)
        .map(|i| ChannelInfo {
            index: i,
            name: map.label(i),
            slug: map.slug(i),
            is_output: map.is_output(i),
        })
        .collect();

    let features = probe_features(t);
    let cs = probe_cs_caps(t);
    let siggen = probe_siggen_caps(t);
    let active_preset = t
        .control_in(op::REQ_PRESET_GET_ACTIVE, 0, 1)
        .ok()
        .and_then(|d| d.first().copied());

    Ok(Capabilities {
        serial: if serial.is_empty() {
            t.descriptor().serial.clone()
        } else {
            serial
        },
        platform,
        firmware,
        wire_format: h.format_version,
        num_channels: h.num_channels,
        num_inputs: h.num_input_channels,
        num_outputs: h.num_output_channels,
        max_bands: h.max_bands,
        channels,
        features,
        cs,
        siggen,
        active_preset,
    })
}

/// Channel names live in the bulk packet, so they cost nothing extra to read.
fn channel_names(bulk: &BulkPacket, count: u8) -> Vec<String> {
    let Some(section) = bulk.section("channel_names") else {
        return Vec::new();
    };
    (0..count as usize)
        .map(|i| {
            let start = i * 32;
            section
                .get(start..start + 32)
                .map(|b| {
                    String::from_utf8_lossy(b)
                        .trim_end_matches('\0')
                        .trim()
                        .to_string()
                })
                .unwrap_or_default()
        })
        .collect()
}

/// Features with no capability descriptor: probe once, cache the answer.
///
/// A stall here is information, not a failure. The loudness output mask, for
/// instance, only exists from wire V19, and older firmware stalls on it.
fn probe_features(t: &mut dyn Transport) -> Vec<Feature> {
    let probes: &[(&str, u8, u16)] = &[
        ("loudness_output_mask", op::REQ_GET_LOUDNESS_MASK, 2),
        ("crossfeed_output_mask", op::REQ_GET_CROSSFEED_OUTPUTS, 1),
        ("leveller_masks", op::REQ_GET_LEVELLER_MASKS, 2),
        ("psychoacoustic_bass", op::REQ_GET_PSYBASS, 1),
        ("upmixer", op::REQ_UPMIX_GET_STATUS, 16),
        ("adat_output", op::REQ_GET_ADAT_ENABLE, 1),
        ("adat_input", op::REQ_GET_ADAT_INPUT_ENABLE, 1),
        ("i2s_slave_clock", op::REQ_GET_I2S_CLOCK_MODE, 1),
        ("i2s_input_channels", op::REQ_GET_I2S_INPUT_CHANNELS, 1),
        ("spdif_multi_input", op::REQ_GET_SPDIF_INPUT_CONFIG, 5),
        ("lg_sound_sync", op::REQ_GET_LG_SOUND_SYNC_ENABLE, 1),
        ("dac_hardware_mute", op::REQ_GET_DAC_HW_MUTE_CONFIG, 16),
        ("uart_control", op::REQ_GET_UART_CONFIG, 8),
        ("i2c_control", op::REQ_GET_I2C_CONFIG, 8),
        ("test_signals", op::REQ_SIGGEN_GET_STATUS, 16),
    ];

    probes
        .iter()
        .map(|(name, opcode, len)| match t.control_in(*opcode, 0, *len) {
            Ok(_) => Feature {
                name: (*name).into(),
                present: true,
                evidence: format!("0x{opcode:02X} answered"),
            },
            Err(TransportError::Stalled { .. }) => Feature {
                name: (*name).into(),
                present: false,
                evidence: format!("0x{opcode:02X} stalled"),
            },
            Err(e) => Feature {
                name: (*name).into(),
                present: false,
                evidence: format!("0x{opcode:02X}: {e}"),
            },
        })
        .collect()
}

fn probe_cs_caps(t: &mut dyn Transport) -> Option<ControlSurfaceCaps> {
    // wValue 0xFFFF asks for the header rather than a single noun descriptor.
    let d = t.control_in(op::REQ_GET_CS_CAPS, 0xFFFF, 4).ok()?;
    let caps_version = *d.first()?;
    let max_bindings = *d.get(1)?;
    let type_count = *d.get(2)?;
    let noun_count = *d.get(3)?;

    // The v3+ additions sit after the variable-length type table, so the header
    // length depends on type_count. Read again now that we know how long it is.
    let full_len = 4 + 4 * type_count as usize + 4;
    let max_ir_commands = t
        .control_in(op::REQ_GET_CS_CAPS, 0xFFFF, full_len as u16)
        .ok()
        .and_then(|full| full.get(4 + 4 * type_count as usize).copied())
        .unwrap_or(0);

    Some(ControlSurfaceCaps {
        caps_version,
        max_bindings,
        type_count,
        noun_count,
        max_ir_commands,
    })
}

fn probe_siggen_caps(t: &mut dyn Transport) -> Option<SiggenCaps> {
    let d = t.control_in(op::REQ_SIGGEN_GET_CAPS, 0xFFFF, 8).ok()?;
    Some(SiggenCaps {
        version: *d.first()?,
        type_count: *d.get(1)?,
        output_channels: *d.get(2)?,
        multitone_max: *d.get(3)?,
        valid_channel_mask: u16::from_le_bytes([*d.get(4)?, *d.get(5)?]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dspi_transport::MockTransport;
    use dspi_transport::mock::Reply;

    fn bulk_bytes() -> Vec<u8> {
        let mut b = vec![0u8; generated::BULK_SIZE];
        b[0] = 26; // wire V26
        b[1] = 1; // RP2350
        b[2] = 17; // channels
        b[3] = 9; // outputs
        b[4] = 8; // inputs
        b[5] = 12; // max bands
        b[6..8].copy_from_slice(&(generated::BULK_SIZE as u16).to_le_bytes());
        // Name channel 0 so the fallback-vs-device-name path is covered.
        let off = generated::OFF_CHANNEL_NAMES;
        b[off..off + 9].copy_from_slice(b"Turntable");
        b
    }

    fn device() -> MockTransport {
        MockTransport::new()
            .data(op::REQ_GET_PLATFORM, vec![1, 1, 0x15, 9])
            .data(op::REQ_GET_SERIAL, b"4BA1DDB9D1443D6A".to_vec())
            .window(op::REQ_GET_ALL_PARAMS_CHUNK, bulk_bytes())
            .data(op::REQ_PRESET_GET_ACTIVE, vec![3])
            .data(op::REQ_GET_CS_CAPS, {
                let mut v = vec![4, 16, 8, 49];
                v.extend(std::iter::repeat_n(0u8, 4 * 8));
                v.push(8); // max_ir_commands
                v.extend([0, 0, 0]);
                v
            })
            .data(
                op::REQ_SIGGEN_GET_CAPS,
                vec![1, 15, 9, 16, 0xFF, 0x01, 0, 0],
            )
    }

    #[test]
    fn discovers_the_channel_model_without_assuming_it() {
        let mut t = device();
        let caps = probe(&mut t).unwrap();
        assert_eq!(caps.platform, Platform::Rp2350);
        assert_eq!(caps.num_channels, 17);
        assert_eq!(caps.num_inputs, 8);
        assert_eq!(caps.num_outputs, 9);
        assert_eq!(caps.firmware, "1.1.5");
        assert_eq!(caps.wire_format, 26);
        assert_eq!(caps.active_preset, Some(3));
    }

    #[test]
    fn uses_device_supplied_channel_names() {
        let mut t = device();
        let caps = probe(&mut t).unwrap();
        assert_eq!(caps.channels[0].name, "Turntable");
        assert_eq!(caps.channels[0].slug, "turntable");
        assert!(!caps.channels[0].is_output);
        // Output channels begin at num_inputs, not at a constant.
        assert!(caps.channels[8].is_output);
        assert!(!caps.channels[7].is_output);
    }

    #[test]
    fn reads_capability_tables_rather_than_hardcoding_them() {
        let mut t = device();
        let caps = probe(&mut t).unwrap();
        let cs = caps.cs.unwrap();
        assert_eq!(cs.caps_version, 4);
        assert_eq!(cs.max_bindings, 16);
        assert_eq!(cs.noun_count, 49);
        assert_eq!(cs.max_ir_commands, 8);

        let sg = caps.siggen.unwrap();
        assert_eq!(sg.type_count, 15);
        assert_eq!(sg.multitone_max, 16);
    }

    /// A stall means "this firmware lacks the feature", which is information we
    /// record, not an error that aborts discovery.
    #[test]
    fn an_absent_feature_is_reported_not_fatal() {
        let mut t = device();
        let caps = probe(&mut t).unwrap();
        let psybass = caps
            .features
            .iter()
            .find(|f| f.name == "psychoacoustic_bass")
            .unwrap();
        assert!(!psybass.present);
        assert!(psybass.evidence.contains("stalled"));
    }

    #[test]
    fn a_present_feature_is_detected() {
        let mut t = device().data(op::REQ_GET_PSYBASS, vec![1]);
        let caps = probe(&mut t).unwrap();
        assert!(
            caps.features
                .iter()
                .find(|f| f.name == "psychoacoustic_bass")
                .unwrap()
                .present
        );
    }

    /// The bulk packet exceeds the 4 KB WinUSB cap, so it must arrive in pieces.
    #[test]
    fn bulk_read_chunks_within_the_transfer_limit() {
        let mut t = device().with_max_transfer(512);
        let packet = read_bulk(&mut t).unwrap();
        assert_eq!(packet.as_bytes().len(), generated::BULK_SIZE);

        let chunk_reads = t
            .log()
            .iter()
            .filter(|e| e.opcode == op::REQ_GET_ALL_PARAMS_CHUNK)
            .count();
        assert!(
            chunk_reads > 1,
            "5944 bytes cannot arrive in one 512-byte transfer"
        );

        // Offsets must be sequential from 0: the firmware snapshots at offset 0.
        let offsets: Vec<u16> = t
            .log()
            .iter()
            .filter(|e| e.opcode == op::REQ_GET_ALL_PARAMS_CHUNK)
            .map(|e| e.value)
            .collect();
        assert_eq!(offsets[0], 0);
        assert!(offsets.windows(2).all(|w| w[1] > w[0]));
    }

    #[test]
    fn a_dead_device_fails_on_the_liveness_read() {
        let mut t = MockTransport::new().reply(op::REQ_GET_PLATFORM, Reply::Disconnect);
        assert!(matches!(probe(&mut t), Err(TransportError::Disconnected)));
    }
}
