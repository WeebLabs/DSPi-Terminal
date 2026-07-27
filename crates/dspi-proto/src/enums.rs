//! Protocol enums, all of which are **open**.
//!
//! Every enum here carries an `Unknown(u8)` variant that round-trips its raw
//! byte. Nothing is ever clamped to a default.
//!
//! This is not defensive style for its own sake. A tuning made on newer firmware
//! can contain a filter type this build has never heard of. Clamping it to
//! `Flat` on read and writing that back would silently destroy the user's work,
//! and they would have no way to tell it had happened. Preserving the raw byte
//! means an unrecognised value survives a read-modify-write untouched, and the
//! UI can say "type 13, unrecognised" instead of lying.

macro_rules! open_enum {
    (
        $(#[$meta:meta])*
        $name:ident : $repr:ty {
            $( $variant:ident = $value:expr, $label:expr ; )*
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[cfg_attr(feature = "serde", derive(serde::Serialize))]
        pub enum $name {
            $( $variant, )*
            /// A value this build does not recognise. Preserved verbatim.
            Unknown($repr),
        }

        impl $name {
            pub const fn from_raw(raw: $repr) -> Self {
                match raw {
                    $( $value => Self::$variant, )*
                    other => Self::Unknown(other),
                }
            }

            pub const fn to_raw(self) -> $repr {
                match self {
                    $( Self::$variant => $value, )*
                    Self::Unknown(raw) => raw,
                }
            }

            /// True when this build understands the value well enough to act on it.
            pub const fn is_known(self) -> bool {
                !matches!(self, Self::Unknown(_))
            }

            pub fn label(self) -> String {
                match self {
                    $( Self::$variant => $label.to_string(), )*
                    Self::Unknown(raw) => format!("type {raw}, unrecognised"),
                }
            }
        }
    };
}

open_enum! {
    /// PEQ and crossover filter types.
    ///
    /// Values 32..63 encode crossover families; see [`Crossover`].
    FilterType: u8 {
        Flat            =  0, "Flat";
        Peaking         =  1, "Peaking";
        LowShelf        =  2, "Low shelf";
        HighShelf       =  3, "High shelf";
        LowPass         =  4, "Low pass";
        HighPass        =  5, "High pass";
        Notch           =  6, "Notch";
        AllPass         =  7, "All pass";
        AllPass1        =  8, "All pass, 1st order";
        LowShelf1       =  9, "Low shelf, 1st order";
        HighShelf1      = 10, "High shelf, 1st order";
        LinkwitzTransform = 11, "Linkwitz Transform";
    }
}

impl FilterType {
    /// Crossover types occupy 32..63 and are configured as a family plus order,
    /// never as a raw number in the UI.
    pub const XOVER_FIRST: u8 = 32;
    pub const XOVER_LAST: u8 = 63;

    pub fn is_crossover(self) -> bool {
        let raw = self.to_raw();
        (Self::XOVER_FIRST..=Self::XOVER_LAST).contains(&raw)
    }

    /// Whether this type uses the `q` field. Shelving and pass filters ignore it,
    /// and showing an editable Q for them misleads the user.
    pub fn uses_q(self) -> bool {
        matches!(
            self,
            Self::Peaking | Self::Notch | Self::AllPass | Self::LowPass | Self::HighPass
        )
    }

    pub fn uses_gain(self) -> bool {
        matches!(
            self,
            Self::Peaking
                | Self::LowShelf
                | Self::HighShelf
                | Self::LowShelf1
                | Self::HighShelf1
        )
    }

    /// The Linkwitz Transform reuses the field names for entirely different
    /// quantities: `freq` is `f0`, `q` is `Q0`, `gain_db` carries `fp` in Hz, and
    /// a fourth parameter `Qp` rides in the band's reserved bytes. A UI that does
    /// not relabel these is incomprehensible.
    pub fn is_linkwitz(self) -> bool {
        matches!(self, Self::LinkwitzTransform)
    }

    /// Field labels for this type, as `(freq, q, gain)`.
    pub fn field_labels(self) -> (&'static str, &'static str, &'static str) {
        if self.is_linkwitz() {
            ("f0", "Q0", "fp")
        } else {
            ("Freq", "Q", "Gain")
        }
    }
}

open_enum! {
    /// Where the device takes audio from.
    InputSource: u8 {
        Usb   = 0, "USB";
        Spdif = 1, "S/PDIF";
        I2s   = 2, "I2S";
        Adat  = 3, "ADAT";
    }
}

open_enum! {
    /// S/PDIF receiver lock state, from `SpdifRxStatusPacket`.
    SpdifRxState: u8 {
        Inactive  = 0, "Inactive";
        Acquiring = 1, "Acquiring";
        Locked    = 2, "Locked";
        Relocking = 3, "Relocking";
    }
}

open_enum! {
    /// What Core 1 is currently doing. PDM and the EQ worker are mutually
    /// exclusive, which is why enabling some outputs can be refused.
    Core1Mode: u8 {
        Idle     = 0, "Idle";
        Pdm      = 1, "PDM";
        EqWorker = 2, "EQ worker";
    }
}

open_enum! {
    /// Crossfeed voicing preset.
    CrossfeedPreset: u8 {
        Default = 0, "Default, 700 Hz / 4.5 dB";
        ChuMoy  = 1, "Chu Moy, 700 Hz / 6.0 dB";
        Meier   = 2, "Meier, 650 Hz / 9.5 dB";
        Custom  = 3, "Custom";
    }
}

open_enum! {
    /// Volume leveller response speed.
    LevellerSpeed: u8 {
        Slow   = 0, "Slow";
        Medium = 1, "Medium";
        Fast   = 2, "Fast";
    }
}

open_enum! {
    /// Physical output bus type, per slot.
    OutputType: u8 {
        Spdif = 0, "S/PDIF";
        I2s   = 1, "I2S";
    }
}

open_enum! {
    /// Which preset a device loads at power-on.
    StartupMode: u8 {
        Specified  = 0, "A specified slot";
        LastActive = 1, "The last active slot";
    }
}

open_enum! {
    /// Whether a setting travels with presets or is device-global.
    ///
    /// This distinction is the subtlest part of the protocol: in independent
    /// mode the device-global copy is authoritative and a preset load leaves it
    /// alone; in per-preset mode the value lives inside each preset.
    PersistenceMode: u8 {
        Independent = 0, "Device-global, not affected by preset load";
        WithPreset  = 1, "Travels with the preset";
    }
}

/// Status codes shared by pin, output and config commands.
///
/// A deferred write returns "accepted" before validation runs, so these are also
/// what a readback comparison has to explain to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinConfigStatus(pub u8);

impl PinConfigStatus {
    pub const SUCCESS: u8 = 0x00;

    pub fn is_ok(self) -> bool {
        self.0 == Self::SUCCESS
    }

    /// A sentence a user can act on, not a code to look up.
    pub fn explain(self) -> String {
        match self.0 {
            0x00 => "Applied".into(),
            0x01 => "That GPIO is not allowed, or the value is out of range".into(),
            0x02 => "That GPIO is already claimed by another function".into(),
            0x03 => "That output or slot does not exist, or the feature is off".into(),
            0x04 => "Disable the output before changing this".into(),
            other => format!("Refused with status 0x{other:02X}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The failure this whole module exists to prevent.
    #[test]
    fn unknown_values_survive_a_round_trip() {
        for raw in 0u8..=255 {
            let t = FilterType::from_raw(raw);
            assert_eq!(t.to_raw(), raw, "filter type {raw} did not round-trip");
        }
    }

    #[test]
    fn unknown_is_never_silently_flattened() {
        let future = FilterType::from_raw(13);
        assert_eq!(future, FilterType::Unknown(13));
        assert!(!future.is_known());
        assert_ne!(future, FilterType::Flat);
        assert_eq!(future.to_raw(), 13);
        assert!(future.label().contains("unrecognised"));
    }

    #[test]
    fn known_types_decode() {
        assert_eq!(FilterType::from_raw(1), FilterType::Peaking);
        assert_eq!(FilterType::from_raw(11), FilterType::LinkwitzTransform);
        assert!(FilterType::Peaking.is_known());
    }

    #[test]
    fn crossover_range_is_recognised() {
        assert!(FilterType::from_raw(32).is_crossover()); // LR2 low pass
        assert!(FilterType::from_raw(63).is_crossover()); // BES8 high pass
        assert!(!FilterType::from_raw(11).is_crossover());
    }

    #[test]
    fn linkwitz_relabels_its_fields() {
        assert_eq!(FilterType::LinkwitzTransform.field_labels(), ("f0", "Q0", "fp"));
        assert_eq!(FilterType::Peaking.field_labels(), ("Freq", "Q", "Gain"));
    }

    #[test]
    fn field_applicability_matches_the_filter_maths() {
        assert!(FilterType::Peaking.uses_q() && FilterType::Peaking.uses_gain());
        // A shelf has gain but its Q is not a user parameter here.
        assert!(FilterType::LowShelf.uses_gain() && !FilterType::LowShelf.uses_q());
        // A pass filter has neither gain nor a meaningful gain field.
        assert!(FilterType::HighPass.uses_q() && !FilterType::HighPass.uses_gain());
    }

    #[test]
    fn all_open_enums_round_trip_every_byte() {
        for raw in 0u8..=255 {
            assert_eq!(InputSource::from_raw(raw).to_raw(), raw);
            assert_eq!(SpdifRxState::from_raw(raw).to_raw(), raw);
            assert_eq!(Core1Mode::from_raw(raw).to_raw(), raw);
            assert_eq!(CrossfeedPreset::from_raw(raw).to_raw(), raw);
            assert_eq!(LevellerSpeed::from_raw(raw).to_raw(), raw);
            assert_eq!(OutputType::from_raw(raw).to_raw(), raw);
            assert_eq!(StartupMode::from_raw(raw).to_raw(), raw);
            assert_eq!(PersistenceMode::from_raw(raw).to_raw(), raw);
        }
    }

    /// ADAT arrived as input source 3 after the released docs were written.
    #[test]
    fn newer_input_sources_are_known() {
        assert_eq!(InputSource::from_raw(3), InputSource::Adat);
    }

    #[test]
    fn pin_status_explains_itself() {
        assert!(PinConfigStatus(0).is_ok());
        assert!(PinConfigStatus(2).explain().contains("already claimed"));
        assert!(PinConfigStatus(0x99).explain().contains("0x99"));
    }
}
