//! DSPi wire protocol: opcodes, the bulk-params codec, DSP math, and the
//! parameter registry.
//!
//! This crate performs no I/O. It turns bytes into meaning and back.
//!
//! # Source of truth
//!
//! Every protocol constant is generated at build time from the firmware headers
//! vendored in `firmware/`, never transcribed. See `build.rs` and
//! `docs/wire-format.md`. The released `commands.md` is twelve wire versions
//! behind the headers and must not be used for layout or counts.

pub mod generated {
    include!(concat!(env!("OUT_DIR"), "/generated.rs"));
}

pub mod channel;
pub mod dsp;
pub mod enums;
pub mod packets;
pub mod registry;
pub mod value;
pub mod wire;
pub mod xover;

pub use channel::ChannelMap;
pub use enums::FilterType;

/// USB identity of a DSPi device.
pub const USB_VID: u16 = 0x2E8B;
pub const USB_PID: u16 = 0xFEAA;

/// The vendor interface. `wIndex` is always this value for every application
/// command; it never carries a parameter.
pub const VENDOR_INTERFACE: u16 = 2;

/// `bmRequestType` for reads: device to host, vendor, interface.
pub const REQ_TYPE_IN: u8 = 0xC1;
/// `bmRequestType` for writes: host to device, vendor, interface.
pub const REQ_TYPE_OUT: u8 = 0x41;

/// Which control-transfer direction an opcode is dispatched on.
///
/// A group of mutating commands are dispatched on the **IN** path: they carry
/// their parameters packed into `wValue` and return a status byte. Issuing one
/// as an OUT transfer stalls. See REDESIGN_SPEC.md 4.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    /// OUT transfer with a payload.
    Out,
    /// IN transfer that reads.
    In,
    /// IN transfer that mutates state. Not a mistake.
    WriteAsRead,
}

/// The firmware platform, which determines channel counts and feature availability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum Platform {
    Rp2040,
    Rp2350,
    /// A platform id this build does not know. Preserved rather than rejected.
    Unknown(u8),
}

impl Platform {
    pub fn from_id(id: u8) -> Self {
        match id {
            0 => Self::Rp2040,
            1 => Self::Rp2350,
            other => Self::Unknown(other),
        }
    }

    pub fn name(&self) -> String {
        match self {
            Self::Rp2040 => "RP2040".into(),
            Self::Rp2350 => "RP2350".into(),
            Self::Unknown(n) => format!("unknown platform {n}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::generated;

    /// The generator computes the packet size from the sizing constants. The
    /// firmware asserts 6136 in its own source (bulk_params.h:477, asserted in
    /// bulk_params.c). If these ever disagree, a section changed size and every
    /// offset after it moved.
    #[test]
    fn bulk_size_matches_firmware() {
        assert_eq!(
            generated::BULK_SIZE,
            6136,
            "WireBulkParams size changed; re-derive docs/wire-format.md and the \
             field offset tables in wire.rs before trusting any offset"
        );
    }

    /// V29 to V32 appended three sections and moved nothing: the header
    /// comments on each (bulk_params.h:378-446) and the total in the struct
    /// comment (bulk_params.h:474-477) say where each begins.
    #[test]
    fn the_v32_sections_are_appended_where_the_header_says() {
        assert_eq!((generated::OFF_SUBHARM, generated::LEN_SUBHARM), (5944, 36));
        assert_eq!((generated::OFF_TUBE, generated::LEN_TUBE), (5980, 48));
        assert_eq!(
            (generated::OFF_LIMITER, generated::LEN_LIMITER),
            (6028, 108)
        );
        assert_eq!(generated::OFF_LIMITER + generated::LEN_LIMITER, 6136);
        let last = generated::SECTIONS[generated::SECTIONS.len() - 1];
        assert_eq!(last, ("limiter", 6028, 108));
    }

    /// No section before the V29 append moved. These are the V28 offsets, so a
    /// failure here means a section grew in the middle rather than at the end,
    /// which the firmware promises never to do within a compatible series.
    #[test]
    fn no_v28_section_moved() {
        let v28: &[(&str, usize, usize)] = &[
            ("header", 0, 16),
            ("global", 16, 16),
            ("crossfeed", 32, 16),
            ("legacy", 48, 16),
            ("delays", 64, 68),
            ("crosspoints", 132, 576),
            ("outputs", 708, 108),
            ("pins", 816, 8),
            ("eq", 824, 3264),
            ("channel_names", 4088, 544),
            ("i2s_config", 4632, 16),
            ("leveller", 4648, 20),
            ("preamp", 4668, 32),
            ("master_volume", 4700, 16),
            ("input_config", 4716, 16),
            ("lg_sound_sync", 4732, 16),
            ("user_volume", 4748, 16),
            ("dac_hw_mute", 4764, 16),
            ("crossovers", 4780, 1088),
            ("adat_config", 5868, 8),
            ("psybass", 5876, 24),
            ("upmix", 5900, 44),
        ];
        assert_eq!(&generated::SECTIONS[..v28.len()], v28);
    }

    #[test]
    fn sections_are_contiguous_and_complete() {
        let mut expect = 0usize;
        for (name, off, len) in generated::SECTIONS {
            assert_eq!(off, expect, "section {name} is not contiguous");
            expect += len;
        }
        assert_eq!(expect, generated::BULK_SIZE);
    }

    #[test]
    fn every_opcode_was_generated() {
        // 244 REQ_* defines in config.h at release/v1.1.6 @ 557bce7: the 202 of
        // 112f35b plus CS aux outputs 0x04-0x07 (config.h:136-145), the
        // spectrum analyser 0x08-0x0F (config.h:146-154), the subharmonic
        // synthesizer 0x10-0x1F, 0x2C-0x2F and 0xA9-0xAE (config.h:181-210),
        // the tube preamp 0x3E-0x3F (config.h:228-231), the limiter 0x81
        // (config.h:233-236) and build info 0x80 (config.h:326).
        assert_eq!(
            generated::ALL_OPCODES.len(),
            244,
            "opcode count changed; run the firmware-bump procedure"
        );
    }

    #[test]
    fn wire_format_version_is_known() {
        // bulk_params.h:34 documents V32: the output limiter section (108
        // bytes) is appended after the tube section.
        assert_eq!(generated::wire::WIRE_FORMAT_VERSION, 32);
    }

    /// The version these headers describe (config.h:659-667), which is what a
    /// refused device is told to update to.
    #[test]
    fn the_firmware_version_is_generated() {
        use generated::firmware::*;
        assert_eq!(
            (
                FW_VERSION_MAJOR,
                FW_VERSION_MINOR,
                FW_VERSION_PATCH,
                FW_VERSION_BETA
            ),
            (1, 1, 6, 4)
        );
    }

    /// Protocol ids that the headers declare as enum members rather than
    /// defines: the tube indices (tube.h:16-32), the limiter indices
    /// (limiter.h:12-18), the source tags (notify.h:92-103) and the CS types
    /// and nouns (control_surfaces.h:128-252).
    #[test]
    fn enum_members_are_lifted_with_their_implicit_values() {
        use generated::{cs, limiter, notify, tube};
        assert_eq!(tube::TUBE_PARAM_ENABLED, 0);
        assert_eq!(tube::TUBE_PARAM_OUTPUT_MASK, 1);
        assert_eq!(tube::TUBE_PARAM_TRIM_DB, 13);
        assert_eq!(tube::TUBE_NUM_PARAMS, 14);
        assert_eq!(limiter::LIMITER_PARAM_ENABLED, 0);
        assert_eq!(limiter::LIMITER_PARAM_LINK_GROUP, 3);
        assert_eq!(limiter::LIMITER_NUM_PARAMS, 4);
        assert_eq!(notify::PARAM_SRC_I2C, 9);
        assert_eq!(cs::CS_TYPE_AUX_PWM, 10);
        assert_eq!(cs::CS_TYPE_COUNT, 11);
        assert_eq!(cs::CS_NOUN_LIMITER_GR, 78);
        assert_eq!(cs::CS_NOUN_COUNT, 79);
    }

    /// Topology must never be compiled in. These are platform-conditional in
    /// `config.h`, so the generator is expected to drop them.
    #[test]
    fn platform_topology_is_not_generated() {
        let generated_names: Vec<&str> = generated::ALL_OPCODES.iter().map(|(n, _)| *n).collect();
        assert!(!generated_names.iter().any(|n| n.contains("NUM_CHANNELS")));
    }
}
