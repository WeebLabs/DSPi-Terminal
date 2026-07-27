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
pub mod enums;
pub mod wire;

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
    /// firmware asserts 5944 in its own source. If these ever disagree, a
    /// section changed size and every offset after it moved.
    #[test]
    fn bulk_size_matches_firmware() {
        assert_eq!(
            generated::BULK_SIZE,
            5944,
            "WireBulkParams size changed; re-derive docs/wire-format.md and the \
             FieldDesc table before trusting any offset"
        );
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
        // 190 REQ_* defines in config.h at release/v1.1.5 @ 9776c2f.
        assert_eq!(
            generated::ALL_OPCODES.len(),
            190,
            "opcode count changed; run the firmware-bump procedure"
        );
    }

    #[test]
    fn wire_format_version_is_known() {
        assert_eq!(generated::wire::WIRE_FORMAT_VERSION, 26);
    }

    /// Topology must never be compiled in. These are platform-conditional in
    /// `config.h`, so the generator is expected to drop them.
    #[test]
    fn platform_topology_is_not_generated() {
        let generated_names: Vec<&str> = generated::ALL_OPCODES.iter().map(|(n, _)| *n).collect();
        assert!(!generated_names.iter().any(|n| n.contains("NUM_CHANNELS")));
    }
}
