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
        // config.h:924-925 at release/v1.1.6: first-order low and high pass,
        // accepted by REQ_SET_EQ_PARAM and by WireBandParams.type.
        LowPass1        = 12, "Low pass, 1st order";
        HighPass1       = 13, "High pass, 1st order";
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

    /// Whether this type uses the `q` field.
    ///
    /// Every second-order PEQ section does, the two 12 dB/oct shelves included:
    /// `config.h:905-911` says the *first*-order shelves are "monotonic (no Q)"
    /// and prewarp by `A`, which is exactly the distinction, and the second-order
    /// pair prewarps by `sqrt(A)` with the RBJ `alpha = sin(w)/(2Q)`. The Console
    /// draws the same line: `FilterType.usesQ` (`DSPMath.swift:274`) is
    /// `!(isCrossover || isFirstOrderPEQ)` for everything but `.flat` and the
    /// Linkwitz Transform, which edits Q0 and Qp in its own panel. A crossover
    /// derives its Q from the family and order, so it has no user Q either.
    pub fn uses_q(self) -> bool {
        matches!(
            self,
            Self::Peaking
                | Self::Notch
                | Self::AllPass
                | Self::LowPass
                | Self::HighPass
                | Self::LowShelf
                | Self::HighShelf
        )
    }

    pub fn uses_gain(self) -> bool {
        matches!(
            self,
            Self::Peaking | Self::LowShelf | Self::HighShelf | Self::LowShelf1 | Self::HighShelf1
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

open_enum! {
    /// Control-surface component types, `control_surfaces.h:128-146`.
    ///
    /// The device reports how many of these it knows in the caps header's
    /// `type_count`, so this table is for labelling, never for gating: a
    /// firmware that appends a type shows up as `Unknown` rather than as a
    /// missing row.
    CsType: u8 {
        None    = 0, "None";
        Button  = 1, "Button";
        Switch  = 2, "Switch";
        Pot     = 3, "Potentiometer";
        Encoder = 4, "Encoder";
        Led     = 5, "LED";
        LedPwm  = 6, "LED (PWM)";
        Ir      = 7, "IR receiver";
        // Caps v10: a container slot taking two GPIOs, SDA then SCL.
        Display = 8, "I2C display";
        // Caps v18: containers that own one GPIO (control_surfaces.h:141-144).
        AuxOut  = 9, "Auxiliary output";
        AuxPwm  = 10, "Dimmable auxiliary output";
    }
}

open_enum! {
    /// Control-surface nouns, `control_surfaces.h:152-252`. `CS_NOUN_COUNT` is
    /// 79 at caps v20, so the last defined noun is 78.
    ///
    /// Labels here are short identifiers for diagnostics. Screens take their
    /// wording from the registry row or the Console, not from this table.
    CsNoun: u8 {
        UserVolume          =  0, "User volume";
        MasterVolume        =  1, "Master volume";
        UserMute            =  2, "User mute";
        Loudness            =  3, "Loudness";
        Crossfeed           =  4, "Crossfeed";
        Leveller            =  5, "Leveller";
        Preset              =  6, "Preset";
        InputSource         =  7, "Input source";
        Clip                =  8, "Clip latch";
        EqBypass            =  9, "EQ bypass";
        LgSync              = 10, "LG Sound Sync";
        CrossfeedPreset     = 11, "Crossfeed preset";
        CrossfeedItd        = 12, "Crossfeed ITD";
        LevellerAmount      = 13, "Leveller amount";
        LevellerSpeed       = 14, "Leveller speed";
        LevellerLookahead   = 15, "Leveller lookahead";
        Preamp              = 16, "Preamp";
        OutputGain          = 17, "Output gain";
        OutputMute          = 18, "Output mute";
        OutputEnable        = 19, "Output enable";
        FilterFreq          = 20, "Filter frequency";
        FilterGain          = 21, "Filter gain";
        FilterQ             = 22, "Filter Q";
        FilterType          = 23, "Filter type";
        FilterBypass        = 24, "Filter bypass";
        Siggen              = 25, "Test signal";
        DacMuteTest         = 26, "DAC mute test";
        ClipCh              = 27, "Channel clip";
        Level               = 28, "Channel level";
        SpdifLock           = 29, "S/PDIF lock";
        SampleRate          = 30, "Sample rate";
        UsbStreaming        = 31, "USB streaming";
        AdatActive          = 32, "ADAT active";
        LgPresent           = 33, "LG source present";
        LgMuted             = 34, "LG source muted";
        Upmix               = 35, "Upmixer";
        UpmixCenterMode     = 36, "Upmixer centre mode";
        UpmixSurroundMode   = 37, "Upmixer surround mode";
        UpmixStrength       = 38, "Upmixer strength";
        UpmixWidth          = 39, "Upmixer centre width";
        UpmixPresence       = 40, "Upmixer presence";
        Psybass             = 41, "Psychoacoustic bass";
        PsybassCutoff       = 42, "Psycho bass cutoff";
        PsybassHarmonics    = 43, "Psycho bass harmonics";
        PsybassDrive        = 44, "Psycho bass drive";
        PsybassCharacter    = 45, "Psycho bass character";
        PsybassOriginal     = 46, "Psycho bass original";
        OutputDelay         = 47, "Output delay";
        PresetReload        = 48, "Reload preset";
        // Caps v7.
        LoudnessSpl         = 49, "Loudness reference SPL";
        LoudnessIntensity   = 50, "Loudness intensity";
        // Caps v8.
        InputLevelMax       = 51, "Loudest input channel";
        // Caps v9.
        Macro               = 52, "Macro";
        // Caps v10.
        CpuLoad             = 53, "CPU load";
        DisplayPage         = 54, "Display page";
        DisplayEdit         = 55, "Display edit mode";
        PageValue           = 56, "Shown page's value";
        // Caps v14 and v15.
        Subharm             = 57, "Subharmonic synthesizer";
        SubharmLow          = 58, "Subharm 24-36 Hz level";
        SubharmHigh         = 59, "Subharm 36-56 Hz level";
        SubharmBoost        = 60, "Subharm LF boost";
        SubharmTop          = 61, "Subharm 56-80 Hz level";
        SubharmSelect       = 62, "Subharm selectivity";
        SubharmDepth        = 63, "Subharm selectivity depth";
        SubharmHold         = 64, "Subharm selectivity hold";
        SubharmCeiling      = 65, "Subharm sub ceiling";
        SubharmLink         = 66, "Subharm pair link";
        SubharmSolo         = 67, "Subharm solo";
        // Caps v18.
        Aux                 = 68, "Auxiliary output";
        AuxLevel            = 69, "Auxiliary output level";
        // Caps v19.
        Tube                = 70, "Tube preamp";
        TubeDrive           = 71, "Tube drive";
        TubeType            = 72, "Tube type";
        TubeMix             = 73, "Tube mix";
        // Caps v20.
        Limiter             = 74, "Output limiter";
        LimiterThreshold    = 75, "Limiter threshold";
        LimiterRelease      = 76, "Limiter release";
        LimiterLink         = 77, "Limiter link group";
        LimiterGr           = 78, "Limiter gain reduction";
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
        // 14 is the first PEQ slot the firmware has not defined: config.h:884-891
        // lists 0..13 and puts the crossovers at 32. It used to be 13, which
        // v1.1.6 claimed for FILTER_HIGHPASS1.
        let future = FilterType::from_raw(14);
        assert_eq!(future, FilterType::Unknown(14));
        assert!(!future.is_known());
        assert_ne!(future, FilterType::Flat);
        assert_eq!(future.to_raw(), 14);
        assert!(future.label().contains("unrecognised"));
    }

    /// The two first-order pass filters arrived in v1.1.6 and must not read as
    /// "unrecognised": a build that does not know them draws them as flat.
    #[test]
    fn the_first_order_pass_filters_are_known() {
        assert_eq!(FilterType::from_raw(12), FilterType::LowPass1);
        assert_eq!(FilterType::from_raw(13), FilterType::HighPass1);
        assert!(FilterType::LowPass1.is_known() && FilterType::HighPass1.is_known());
        // First-order sections derive their own damping; Q is not a user field,
        // and neither carries a gain. Matches the Console's `isFirstOrderPEQ`.
        assert!(!FilterType::LowPass1.uses_q() && !FilterType::LowPass1.uses_gain());
        assert!(!FilterType::HighPass1.uses_q() && !FilterType::HighPass1.uses_gain());
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
        assert_eq!(
            FilterType::LinkwitzTransform.field_labels(),
            ("f0", "Q0", "fp")
        );
        assert_eq!(FilterType::Peaking.field_labels(), ("Freq", "Q", "Gain"));
    }

    #[test]
    fn field_applicability_matches_the_filter_maths() {
        assert!(FilterType::Peaking.uses_q() && FilterType::Peaking.uses_gain());
        // A 12 dB/oct shelf has both: its Q sets the shelf's slope, and the
        // Console's own reference row reads `Low Shelf 12 dB/oct 105 Hz +8.8 dB
        // 0.707`.
        assert!(FilterType::LowShelf.uses_gain() && FilterType::LowShelf.uses_q());
        assert!(FilterType::HighShelf.uses_gain() && FilterType::HighShelf.uses_q());
        // A pass filter has neither gain nor a meaningful gain field.
        assert!(FilterType::HighPass.uses_q() && !FilterType::HighPass.uses_gain());
    }

    /// `FilterType.usesQ` in `DSPMath.swift:274`, and the two lists
    /// `FilterFileTests.testGainAndQApplicability` pins. A drift here blanks a
    /// WIDTH cell in the filter list and drops the Q from an exported file.
    #[test]
    fn uses_q_is_the_consoles_list_exactly() {
        for t in [
            FilterType::Peaking,
            FilterType::LowShelf,
            FilterType::HighShelf,
            FilterType::LowPass,
            FilterType::HighPass,
            FilterType::Notch,
            FilterType::AllPass,
        ] {
            assert!(t.uses_q(), "{t:?} should use Q");
        }
        for t in [
            FilterType::AllPass1,
            FilterType::LowShelf1,
            FilterType::HighShelf1,
            FilterType::LowPass1,
            FilterType::HighPass1,
            FilterType::LinkwitzTransform,
            FilterType::Flat,
        ] {
            assert!(!t.uses_q(), "{t:?} should not use Q");
        }
        // Crossovers derive their Q from the family and the order.
        for raw in FilterType::XOVER_FIRST..=FilterType::XOVER_LAST {
            let t = FilterType::from_raw(raw);
            assert!(!t.uses_q(), "crossover {raw} should not use Q");
        }
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
            assert_eq!(CsType::from_raw(raw).to_raw(), raw);
            assert_eq!(CsNoun::from_raw(raw).to_raw(), raw);
        }
    }

    /// `CS_TYPE_COUNT` is 11 at caps v20 (control_surfaces.h:145), which is
    /// what grows the caps header from 44 bytes to 52.
    #[test]
    fn the_aux_component_types_are_known() {
        use crate::generated::cs;
        assert_eq!(CsType::from_raw(8), CsType::Display);
        assert_eq!(CsType::from_raw(9), CsType::AuxOut);
        assert_eq!(CsType::from_raw(10), CsType::AuxPwm);
        assert!(!CsType::from_raw(cs::CS_TYPE_COUNT as u8).is_known());
    }

    /// `CS_NOUN_COUNT` is 79 (control_surfaces.h:251), so 78 is the last one.
    #[test]
    fn the_noun_table_ends_where_the_firmware_says() {
        use crate::generated::cs;
        assert_eq!(CsNoun::from_raw(48), CsNoun::PresetReload);
        assert_eq!(CsNoun::from_raw(56), CsNoun::PageValue);
        assert_eq!(CsNoun::from_raw(57), CsNoun::Subharm);
        assert_eq!(CsNoun::from_raw(68), CsNoun::Aux);
        assert_eq!(CsNoun::from_raw(72), CsNoun::TubeType);
        assert_eq!(CsNoun::from_raw(78), CsNoun::LimiterGr);
        assert!(!CsNoun::from_raw(cs::CS_NOUN_COUNT as u8).is_known());
        for n in 0..cs::CS_NOUN_COUNT as u8 {
            assert!(CsNoun::from_raw(n).is_known(), "noun {n} has no variant");
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
