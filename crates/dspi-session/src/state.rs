//! The device, as the interface sees it.
//!
//! One typed model that every screen reads from and every write updates: the
//! bulk packet as a shadow with typed views decoded on demand, the meters, and
//! the things the bulk packet does not carry. Notifications patch the shadow
//! in place; a full re-read replaces it. The preset snapshot and diff mirror
//! the Console's `unsaved_changes_spec.md`, so the `*` marker and the
//! Save / Discard / Cancel prompt behave the same way.
//!
//! Byte layouts cite `bulk_params.h` at v1.1.6; section offsets come from the
//! generated table, never from a literal.

use dspi_proto::enums::FilterType;
use dspi_proto::value::EqParamPacket;
use dspi_proto::wire::{BulkPacket, InputConfig};

use crate::Meters;
use crate::notify::{Event, Notification, Source};
use crate::probe::Capabilities;

fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

/// `WireGlobalParams` (bulk_params.h): preamp, bypass, loudness.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Global {
    pub preamp_db: f32,
    pub bypass: bool,
    pub loudness_enabled: bool,
    pub loudness_output_mask: u16,
    pub loudness_ref_spl: f32,
    pub loudness_intensity_pct: f32,
}

/// `WireCrossfeedParams`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crossfeed {
    pub enabled: bool,
    pub preset: u8,
    pub itd_enabled: bool,
    pub output_pair_mask: u8,
    pub custom_fc: f32,
    pub custom_feed_db: f32,
}

/// One `WireCrosspoint`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Crosspoint {
    pub enabled: bool,
    pub phase_invert: bool,
    pub gain_db: f32,
}

/// One `WireOutputChannel`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Output {
    pub enabled: bool,
    pub mute: bool,
    pub gain_db: f32,
    pub delay_ms: f32,
}

/// `WireI2SConfig`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct I2sConfig {
    pub output_types: [u8; 4],
    pub bck_pin: u8,
    pub mck_pin: u8,
    pub mck_enabled: bool,
    pub mck_multiplier: u8,
    /// `clock_pin_mode_p1` resolved: `None` when the firmware did not say.
    pub clock_pin_mode: Option<u8>,
    pub bck_pin_slave: u8,
}

/// `WireLevellerConfig`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Leveller {
    pub enabled: bool,
    pub speed: u8,
    pub lookahead: bool,
    pub amount_pct: f32,
    pub max_gain_db: f32,
    pub gate_threshold_db: f32,
    pub detector_mask: u8,
    pub apply_mask: u8,
}

/// `WireLgSoundSync`: `enabled` is honoured on a SET; the rest is read-only.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LgSoundSync {
    pub enabled: bool,
    pub present: bool,
    pub volume: u8,
    pub muted: bool,
}

/// `WireDacHwMute`, mirroring `DacHwMuteConfig`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DacHwMute {
    pub enabled: bool,
    pub active_low: bool,
    pub pin: u8,
    pub hold_ms: u16,
    pub release_ms: u16,
}

/// `WirePsybassParams`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Psybass {
    pub enabled: bool,
    pub output_mask: u16,
    pub cutoff_hz: f32,
    pub harmonics_db: f32,
    pub drive_db: f32,
    pub character_pct: f32,
    pub original_db: f32,
}

/// `WireUpmixParams`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Upmix {
    pub enabled: bool,
    pub center_mode: u8,
    pub surround_mode: u8,
    /// `presence_q1` resolved: dB, from the int8 half-dB field.
    pub presence_db: f32,
    pub strength: f32,
    pub center_width: f32,
    pub threshold: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub det_hpf_hz: f32,
    pub sur_delay_ms: f32,
    pub sur_hpf_hz: f32,
    pub sur_lpf_hz: f32,
    pub decorr: f32,
}

/// `WireSubharmParams` (bulk_params.h:378-400). Solo is not here: it is
/// runtime only and lives in [`DeviceState::subharm_solo`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Subharm {
    pub enabled: bool,
    pub output_mask: u16,
    /// The 24-36 Hz sub; `SUBHARM_LEVEL_MIN` (-30) is the band off.
    pub low_db: f32,
    /// The 36-56 Hz sub.
    pub high_db: f32,
    /// The 70 Hz bell.
    pub boost_db: f32,
    /// The 56-80 Hz sub.
    pub top_db: f32,
    pub select_depth_pct: f32,
    pub select_hold_ms: f32,
    /// `SUBHARM_CEILING_MAX` (0 dBFS) is the ceiling off.
    pub ceiling_db: f32,
    /// `SUBHARM_SELECT_*` (subharm.h:71-73).
    pub select_mode: u8,
    pub link_pairs: bool,
}

/// `WireTubeParams` (bulk_params.h:403-427).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tube {
    pub enabled: bool,
    /// 0 is Custom, 1..=`TUBE_TYPE_MAX` a row of [`crate::tube::TYPES`].
    pub tube_type: u8,
    /// `0..=TUBE_RECT_MAX`, a row of [`crate::tube::RECTIFIERS`].
    pub rectifier: u8,
    /// The output stage.
    pub xfmr_enabled: bool,
    pub output_mask: u16,
    pub drive_db: f32,
    pub bias_pct: f32,
    pub asym_db: f32,
    pub hardness_pct: f32,
    pub sag_pct: f32,
    pub xfmr_damping: f32,
    pub xfmr_res_hz: f32,
    pub mix_pct: f32,
    pub trim_db: f32,
}

/// One `WireLimiterOutput` (bulk_params.h:436-442).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LimiterOutput {
    pub enabled: bool,
    /// 0 unlinked, 1..=`LIMITER_LINK_GROUP_MAX`.
    pub link_group: u8,
    pub threshold_db: f32,
    pub release_ms: f32,
}

/// `OUTPUT_CONFIG_MODE_*` (config.h:497-498): whether the output
/// configuration, and from wire V32 the limiters, is stored on its own or with
/// each preset. The header's names are not generated, so they are here.
pub mod output_config_mode {
    pub const INDEPENDENT: u8 = 0;
    /// The firmware's default.
    pub const WITH_PRESET: u8 = 1;
}

/// `MASTER_VOLUME_MODE_*` (config.h:484-485): whether a preset load restores
/// the master volume (`apply_master_volume_from_mode`, flash_storage.c:3743).
pub mod master_volume_mode {
    /// The Console's default until the directory is read
    /// (DSPViewModel.swift:1749).
    pub const INDEPENDENT: u8 = 0;
    pub const WITH_PRESET: u8 = 1;
}

/// What changed since the last snapshot, one line each, categorised as the
/// Console's `PresetDiff` is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub category: &'static str,
    pub text: String,
}

/// The preset-relevant state at a point in time: the bulk packet with the
/// runtime-only fields blanked, so two snapshots compare equal exactly when
/// the device would save the same preset.
///
/// The output-config mode is kept with the bytes, as the Console's snapshot
/// keeps `outputConfigMode`: the limiters belong to the preset only in
/// WITH_PRESET mode (bulk_params.c:1008 applies them from a preset only
/// then), so the comparison and the diff are gated on the live mode. The
/// master-volume mode is kept the same way, as the Console's
/// `masterVolumeMode` (PresetSnapshot.swift:33-37), and the channel counts so
/// the comparison can work out the default channel names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetSnapshot {
    bytes: Vec<u8>,
    output_config_mode: u8,
    master_volume_mode: u8,
    num_inputs: usize,
    num_outputs: usize,
}

impl PresetSnapshot {
    pub fn capture(state: &DeviceState) -> Self {
        let mut bytes = state.bulk.as_bytes().to_vec();
        Self::blank_runtime(&mut bytes);
        Self {
            bytes,
            output_config_mode: state.output_config_mode,
            master_volume_mode: state.master_volume_mode,
            num_inputs: state.caps.num_inputs as usize,
            num_outputs: state.caps.num_outputs as usize,
        }
    }

    /// Zero the fields that change without anyone editing a preset: the
    /// header, and the LG Sound Sync telemetry (`present`, `volume`,
    /// `muted`, bulk_params.h `WireLgSoundSync`).
    fn blank_runtime(bytes: &mut [u8]) {
        let (_, hoff, hlen) = section("header");
        bytes[hoff..hoff + hlen].fill(0);
        let (_, loff, _) = section("lg_sound_sync");
        bytes[loff + 1..loff + 4].fill(0);
    }

    /// Whether a live packet would capture to this snapshot, compared in
    /// place: this runs every frame, so it must not allocate.
    ///
    /// `output_config_mode` is the live mode. In INDEPENDENT mode the output
    /// configuration (see [`Self::output_config`]) is not the preset's, so it
    /// is left out of the comparison, as the Console's diff leaves it out.
    /// `master_volume_mode` is the live master-volume mode, and the master
    /// volume counts only in WITH_PRESET mode, as in the Console's diff
    /// (PresetSnapshot.swift:186-198). The channel names are compared by
    /// [`Self::names_match`].
    pub fn matches(&self, live: &[u8], output_config_mode: u8, master_volume_mode: u8) -> bool {
        if live.len() != self.bytes.len() {
            return false;
        }
        let (_, hoff, hlen) = section("header");
        let (_, loff, _) = section("lg_sound_sync");
        let (_, moff, mlen) = section("master_volume");
        let (_, noff, nlen) = section("channel_names");
        let with_preset = output_config_mode == output_config_mode::WITH_PRESET;
        let with_master = master_volume_mode == master_volume_mode::WITH_PRESET;
        let io = Self::output_config();
        live.iter().zip(&self.bytes).enumerate().all(|(i, (a, b))| {
            let blanked = (i >= hoff && i < hoff + hlen)
                || (i > loff && i < loff + 4)
                || (i >= noff && i < noff + nlen)
                || (!with_master && i >= moff && i < moff + mlen)
                || (!with_preset && io.iter().any(|(o, n)| i >= *o && i < o + n));
            blanked || a == b
        }) && self.names_match(live)
    }

    /// Whether every channel name in `live` is this snapshot's, apart from
    /// a rename from one default name to another. The firmware renames an
    /// output still at its default when the output's type flips
    /// (main.c:672-689), a consequence of the output configuration rather
    /// than an edit, and the Console skips it (PresetSnapshot.swift:536-553).
    fn names_match(&self, live: &[u8]) -> bool {
        let (na, nb) = (
            sec(&self.bytes, "channel_names"),
            sec(live, "channel_names"),
        );
        let (ta, tb) = (sec(&self.bytes, "i2s_config"), sec(live, "i2s_config"));
        (0..na.len() / 32).all(|ch| {
            let (x, y) = (&na[ch * 32..ch * 32 + 32], &nb[ch * 32..ch * 32 + 32]);
            x == y || self.default_rename(ch, x, ta, y, tb)
        })
    }

    /// Whether channel `ch` going from name `x` (with output types `tx`) to
    /// `y` (with `ty`) is a default-to-default rename. Only an output can be
    /// one: the Console's input defaults do not follow the input source
    /// (DSPViewModel.swift:3079-3096), so an input whose old and new names
    /// are both its default has not changed, and the Console never skips an
    /// input rename, including the firmware's own on a source switch
    /// (main.c:3740-3756).
    fn default_rename(&self, ch: usize, x: &[u8], tx: &[u8], y: &[u8], ty: &[u8]) -> bool {
        let (ni, no) = (self.num_inputs, self.num_outputs);
        is_default_output_name(x, ch, ni, no, tx) && is_default_output_name(y, ch, ni, no, ty)
    }

    /// The byte ranges of the output configuration: what a preset load takes
    /// from the slot only in WITH_PRESET mode (`FlashOutputConfig`,
    /// flash_storage.c:225-285, and the limiters, 3383-3441, both chosen by
    /// `apply_output_config_from_mode`, flash_storage.c:3434). That is the
    /// output pins, the I2S outputs and clocks, the ADAT output, the
    /// limiters, and every field of the input block but the source itself:
    /// the S/PDIF, I2S and ADAT input pins and enables, the I2S count, rate
    /// and clock mode (bulk_params.h:203-233). The Console's diff gates the
    /// same set on the mode (PresetSnapshot.swift:355-362, 555-630).
    fn output_config() -> [(usize, usize); 5] {
        let range = |name: &str| {
            let (_, off, len) = section(name);
            (off, len)
        };
        let (ioff, ilen) = range("input_config");
        [
            range("pins"),
            range("i2s_config"),
            range("adat_config"),
            range("limiter"),
            // `input_source` is the block's first byte.
            (ioff + 1, ilen - 1),
        ]
    }

    /// Every difference between two snapshots, described the way the
    /// Console's `PresetSnapshot.diff` describes them.
    pub fn diff(&self, other: &Self, num_inputs: usize, num_outputs: usize) -> Vec<DiffLine> {
        let a = &self.bytes;
        let b = &other.bytes;
        let mut out = Vec::new();
        // Names come from the older snapshot, so a rename reads "old renamed
        // to new" and a channel that never had a name is numbered.
        let name = |ch: usize| -> String {
            let n = sec(a, "channel_names");
            let text = if ch < 17 {
                cstr(&n[ch * 32..ch * 32 + 32])
            } else {
                String::new()
            };
            if text.is_empty() {
                format!("channel {}", ch + 1)
            } else {
                text
            }
        };
        let differs = |sec: &str, a: &[u8], b: &[u8]| -> bool {
            let (_, off, len) = section(sec);
            a[off..off + len] != b[off..off + len]
        };
        let line = |out: &mut Vec<DiffLine>, category: &'static str, text: String| {
            out.push(DiffLine { category, text });
        };

        if differs("global", a, b) {
            let (ga, gb) = (
                decode_global(sec(a, "global")),
                decode_global(sec(b, "global")),
            );
            if ga.preamp_db != gb.preamp_db {
                line(
                    &mut out,
                    "Global",
                    format!("Preamp {:+.1} dB to {:+.1} dB", ga.preamp_db, gb.preamp_db),
                );
            }
            if ga.bypass != gb.bypass {
                line(&mut out, "Global", format!("Bypass {}", on_off(gb.bypass)));
            }
            if ga.loudness_enabled != gb.loudness_enabled {
                line(
                    &mut out,
                    "Loudness",
                    format!("Loudness {}", on_off(gb.loudness_enabled)),
                );
            }
            if ga.loudness_ref_spl != gb.loudness_ref_spl
                || ga.loudness_intensity_pct != gb.loudness_intensity_pct
            {
                line(&mut out, "Loudness", "Loudness parameters".into());
            }
            if ga.loudness_output_mask != gb.loudness_output_mask {
                line(&mut out, "Loudness", "Loudness outputs".into());
            }
        }
        if differs("crossfeed", a, b) {
            let (ca, cb) = (
                decode_crossfeed(sec(a, "crossfeed")),
                decode_crossfeed(sec(b, "crossfeed")),
            );
            if ca.enabled != cb.enabled {
                line(
                    &mut out,
                    "Crossfeed",
                    format!("Crossfeed {}", on_off(cb.enabled)),
                );
            } else {
                line(&mut out, "Crossfeed", "Crossfeed parameters".into());
            }
        }
        if differs("delays", a, b) {
            let (da, db) = (sec(a, "delays"), sec(b, "delays"));
            for ch in 0..(num_inputs + num_outputs).min(17) {
                let (x, y) = (f32_at(da, ch * 4), f32_at(db, ch * 4));
                if x != y {
                    line(
                        &mut out,
                        "Delays",
                        format!("{} delay {:.1} ms to {:.1} ms", name(ch), x, y),
                    );
                }
            }
        }
        if differs("crosspoints", a, b) {
            let (ca, cb) = (sec(a, "crosspoints"), sec(b, "crosspoints"));
            for i in 0..num_inputs.min(8) {
                for o in 0..num_outputs.min(9) {
                    let off = (i * 9 + o) * 8;
                    if ca[off..off + 8] != cb[off..off + 8] {
                        line(
                            &mut out,
                            "Matrix",
                            format!("{} to {} routing", name(i), name(num_inputs + o)),
                        );
                    }
                }
            }
        }
        if differs("outputs", a, b) {
            let (oa, ob) = (sec(a, "outputs"), sec(b, "outputs"));
            for o in 0..num_outputs.min(9) {
                let (x, y) = (
                    decode_output(&oa[o * 12..o * 12 + 12]),
                    decode_output(&ob[o * 12..o * 12 + 12]),
                );
                if x != y {
                    let what = if x.enabled != y.enabled {
                        (if y.enabled { "enabled" } else { "disabled" }).to_string()
                    } else if x.mute != y.mute {
                        (if y.mute { "muted" } else { "unmuted" }).to_string()
                    } else if x.gain_db != y.gain_db {
                        format!("gain {:+.1} dB to {:+.1} dB", x.gain_db, y.gain_db)
                    } else {
                        format!("delay {:.1} ms to {:.1} ms", x.delay_ms, y.delay_ms)
                    };
                    line(
                        &mut out,
                        "Outputs",
                        format!("{} {}", name(num_inputs + o), what),
                    );
                }
            }
        }
        // The output configuration counts only in WITH_PRESET mode, as the
        // limiters below do (see `output_config`).
        let with_preset = other.output_config_mode == output_config_mode::WITH_PRESET;
        if with_preset && differs("pins", a, b) {
            line(&mut out, "Hardware", "Output pins".into());
        }
        for (secname, stride, label) in [("eq", 12usize, "EQ"), ("crossovers", 4usize, "Crossover")]
        {
            if differs(secname, a, b) {
                let (ea, eb) = (sec(a, secname), sec(b, secname));
                for ch in 0..(num_inputs + num_outputs).min(17) {
                    let mut changed = Vec::new();
                    for band in 0..stride {
                        let off = (ch * stride + band) * 16;
                        if ea[off..off + 16] != eb[off..off + 16] {
                            changed.push(band + 1);
                        }
                    }
                    if !changed.is_empty() {
                        let bands: Vec<String> = changed.iter().map(|b| b.to_string()).collect();
                        line(
                            &mut out,
                            "Filters",
                            format!(
                                "{} {} band{} {}",
                                name(ch),
                                label,
                                if changed.len() > 1 { "s" } else { "" },
                                bands.join(", ")
                            ),
                        );
                    }
                }
            }
        }
        if differs("channel_names", a, b) {
            let (na, nb) = (sec(a, "channel_names"), sec(b, "channel_names"));
            let (ta, tb) = (sec(a, "i2s_config"), sec(b, "i2s_config"));
            for ch in 0..(num_inputs + num_outputs).min(17) {
                let (x, y) = (&na[ch * 32..ch * 32 + 32], &nb[ch * 32..ch * 32 + 32]);
                // A default-to-default rename is the firmware's, not an edit
                // (see `names_match`).
                if x != y && !self.default_rename(ch, x, ta, y, tb) {
                    line(
                        &mut out,
                        "Names",
                        format!(
                            "{} renamed to {}",
                            name(ch),
                            cstr(&nb[ch * 32..ch * 32 + 32])
                        ),
                    );
                }
            }
        }
        if with_preset && differs("i2s_config", a, b) {
            line(&mut out, "Hardware", "I2S configuration".into());
        }
        if differs("leveller", a, b) {
            let (la, lb) = (
                decode_leveller(sec(a, "leveller")),
                decode_leveller(sec(b, "leveller")),
            );
            if la.enabled != lb.enabled {
                line(
                    &mut out,
                    "Leveller",
                    format!("Volume Leveller {}", on_off(lb.enabled)),
                );
            } else {
                line(&mut out, "Leveller", "Volume Leveller parameters".into());
            }
        }
        if differs("preamp", a, b) {
            let (pa, pb) = (sec(a, "preamp"), sec(b, "preamp"));
            for ch in 0..num_inputs.min(8) {
                let (x, y) = (f32_at(pa, ch * 4), f32_at(pb, ch * 4));
                if x != y {
                    line(
                        &mut out,
                        "Global",
                        format!("{} preamp {:+.1} dB to {:+.1} dB", name(ch), x, y),
                    );
                }
            }
        }
        // Gated on the live mode, as the Console's is
        // (PresetSnapshot.swift:186-198).
        if other.master_volume_mode == master_volume_mode::WITH_PRESET
            && differs("master_volume", a, b)
        {
            line(&mut out, "Global", "Master volume".into());
        }
        let (inputs_a, inputs_b) = (sec(a, "input_config"), sec(b, "input_config"));
        if inputs_a[0] != inputs_b[0] || (with_preset && inputs_a[1..] != inputs_b[1..]) {
            line(&mut out, "Hardware", "Input configuration".into());
        }
        if differs("lg_sound_sync", a, b) {
            line(&mut out, "Hardware", "LG Sound Sync".into());
        }
        if differs("user_volume", a, b) {
            line(&mut out, "Global", "Volume".into());
        }
        if differs("dac_hw_mute", a, b) {
            line(&mut out, "Hardware", "External mute".into());
        }
        if with_preset && differs("adat_config", a, b) {
            line(&mut out, "Hardware", "ADAT output".into());
        }
        if differs("psybass", a, b) {
            let (x, y) = (
                decode_psybass(sec(a, "psybass")),
                decode_psybass(sec(b, "psybass")),
            );
            if x.enabled != y.enabled {
                line(
                    &mut out,
                    "Psybass",
                    format!("Psychoacoustic Bass {}", on_off(y.enabled)),
                );
            } else {
                line(&mut out, "Psybass", "Psychoacoustic Bass parameters".into());
            }
        }
        if differs("subharm", a, b) {
            diff_subharm(
                &decode_subharm(sec(a, "subharm")),
                &decode_subharm(sec(b, "subharm")),
                &mut out,
            );
        }
        if differs("tube", a, b) {
            diff_tube(
                &decode_tube(sec(a, "tube")),
                &decode_tube(sec(b, "tube")),
                &mut out,
            );
        }
        // The limiters follow the output-config mode like the pins: only in
        // WITH_PRESET mode does a preset restore them, so only there are they
        // a preset change. The gate is the live mode, as the Console's is
        // (PresetSnapshot.swift:355-362).
        if other.output_config_mode == output_config_mode::WITH_PRESET && differs("limiter", a, b) {
            let (la, lb) = (sec(a, "limiter"), sec(b, "limiter"));
            for o in 0..num_outputs.min(9) {
                let (x, y) = (
                    decode_limiter(&la[o * 12..o * 12 + 12]),
                    decode_limiter(&lb[o * 12..o * 12 + 12]),
                );
                if x != y {
                    // The output's channel name, from the older snapshot as
                    // the other lines take it; the Console's fallback is the
                    // zero-based output index.
                    let at = (num_inputs + o) * 32;
                    let n = sec(a, "channel_names")
                        .get(at..at + 32)
                        .map(cstr)
                        .unwrap_or_default();
                    let n = if n.is_empty() {
                        format!("Output {o}")
                    } else {
                        n
                    };
                    diff_limiter(&n, &x, &y, &mut out);
                }
            }
        }
        if differs("upmix", a, b) {
            let (x, y) = (decode_upmix(sec(a, "upmix")), decode_upmix(sec(b, "upmix")));
            if x.enabled != y.enabled {
                line(
                    &mut out,
                    "Upmix",
                    format!("Stereo Upmixer {}", on_off(y.enabled)),
                );
            } else {
                line(&mut out, "Upmix", "Stereo Upmixer parameters".into());
            }
        }
        out
    }

    /// The Console's bulleted summary, capped with "and N more".
    pub fn summary(lines: &[DiffLine], max_lines: usize) -> String {
        let mut s = String::new();
        for l in lines.iter().take(max_lines) {
            s.push_str(&format!("- {}: {}\n", l.category, l.text));
        }
        if lines.len() > max_lines {
            s.push_str(&format!("- and {} more\n", lines.len() - max_lines));
        }
        s.trim_end().to_string()
    }
}

fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

// The beta4 sections' lines are the Console's own, word for word
// (PresetSnapshot.swift:261-378), with its arrow, one line per field.

/// The Console's `formatVal` (PresetSnapshot.swift:665-670): whole numbers
/// without a decimal, the rest to one place.
fn format_val(v: f32) -> String {
    if v == v.round() && v.abs() < 100_000.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    }
}

fn enabled(b: bool) -> &'static str {
    if b { "enabled" } else { "disabled" }
}

fn diff_subharm(o: &Subharm, n: &Subharm, out: &mut Vec<DiffLine>) {
    use dspi_proto::generated::ranges::{SUBHARM_CEILING_MAX, SUBHARM_LEVEL_MIN};
    let mut line = |text: String| {
        out.push(DiffLine {
            category: "Subharm",
            text,
        })
    };
    // The floor is the band off, and a ceiling at full scale is the stage
    // off, so both are named rather than given as levels.
    let level = |db: f32| {
        if db <= SUBHARM_LEVEL_MIN {
            "off".to_string()
        } else {
            format!("{} dB", format_val(db))
        }
    };
    let ceiling = |db: f32| {
        if db >= SUBHARM_CEILING_MAX {
            "off".to_string()
        } else {
            format!("{} dBFS", format_val(db))
        }
    };
    // SUBHARM_SELECT_* (subharm.h:71-73).
    let select = |m: u8| match m as u16 {
        dspi_proto::generated::subharm::SUBHARM_SELECT_PERCUSSIVE => "percussive",
        dspi_proto::generated::subharm::SUBHARM_SELECT_SUSTAINED => "sustained",
        _ => "all material",
    };
    if o.enabled != n.enabled {
        line(format!("Subharmonic Synthesizer: {}", enabled(n.enabled)));
    }
    if o.output_mask != n.output_mask {
        line(format!(
            "Subharm outputs: 0x{:04X} → 0x{:04X}",
            o.output_mask, n.output_mask
        ));
    }
    for (label, a, b) in [
        ("24-36 Hz", o.low_db, n.low_db),
        ("36-56 Hz", o.high_db, n.high_db),
        ("56-80 Hz", o.top_db, n.top_db),
    ] {
        if a != b {
            line(format!("Subharm {label}: {} → {}", level(a), level(b)));
        }
    }
    if o.select_mode != n.select_mode {
        line(format!(
            "Subharm selectivity: {} → {}",
            select(o.select_mode),
            select(n.select_mode)
        ));
    }
    if o.select_depth_pct != n.select_depth_pct {
        line(format!(
            "Subharm selectivity depth: {}% → {}%",
            format_val(o.select_depth_pct),
            format_val(n.select_depth_pct)
        ));
    }
    if o.select_hold_ms != n.select_hold_ms {
        line(format!(
            "Subharm selectivity hold: {} ms → {} ms",
            format_val(o.select_hold_ms),
            format_val(n.select_hold_ms)
        ));
    }
    if o.ceiling_db != n.ceiling_db {
        line(format!(
            "Subharm sub ceiling: {} → {}",
            ceiling(o.ceiling_db),
            ceiling(n.ceiling_db)
        ));
    }
    if o.link_pairs != n.link_pairs {
        line(format!(
            "Subharm pair link: {}",
            if n.link_pairs {
                "linked"
            } else {
                "independent"
            }
        ));
    }
    if o.boost_db != n.boost_db {
        line(format!(
            "Subharm LF boost: {} dB → {} dB",
            format_val(o.boost_db),
            format_val(n.boost_db)
        ));
    }
}

/// A type change moves the four character values too, so their lines come
/// with it; the Console expects that rather than treating it as noise.
fn diff_tube(o: &Tube, n: &Tube, out: &mut Vec<DiffLine>) {
    use crate::tube::{rectifier_name, type_name};
    let mut line = |text: String| {
        out.push(DiffLine {
            category: "Tube",
            text,
        })
    };
    let pair =
        |a: f32, b: f32, unit: &str| format!("{}{unit} → {}{unit}", format_val(a), format_val(b));
    if o.enabled != n.enabled {
        line(format!("Tube Modeller: {}", enabled(n.enabled)));
    }
    if o.output_mask != n.output_mask {
        line(format!(
            "Tube outputs: 0x{:04X} → 0x{:04X}",
            o.output_mask, n.output_mask
        ));
    }
    if o.tube_type != n.tube_type {
        line(format!(
            "Tube type: {} → {}",
            type_name(o.tube_type),
            type_name(n.tube_type)
        ));
    }
    if o.drive_db != n.drive_db {
        line(format!(
            "Tube drive: {}",
            pair(o.drive_db, n.drive_db, " dB")
        ));
    }
    if o.bias_pct != n.bias_pct {
        line(format!("Tube bias: {}", pair(o.bias_pct, n.bias_pct, "%")));
    }
    if o.asym_db != n.asym_db {
        line(format!(
            "Tube asymmetry: {}",
            pair(o.asym_db, n.asym_db, " dB")
        ));
    }
    if o.hardness_pct != n.hardness_pct {
        line(format!(
            "Tube knee hardness: {}",
            pair(o.hardness_pct, n.hardness_pct, "%")
        ));
    }
    if o.sag_pct != n.sag_pct {
        line(format!("Tube sag: {}", pair(o.sag_pct, n.sag_pct, "%")));
    }
    if o.rectifier != n.rectifier {
        line(format!(
            "Tube rectifier: {} → {}",
            rectifier_name(o.rectifier),
            rectifier_name(n.rectifier)
        ));
    }
    if o.xfmr_enabled != n.xfmr_enabled {
        line(format!("Tube output stage: {}", enabled(n.xfmr_enabled)));
    }
    if o.xfmr_damping != n.xfmr_damping {
        line(format!(
            "Tube damping factor: {}",
            pair(o.xfmr_damping, n.xfmr_damping, "")
        ));
    }
    if o.xfmr_res_hz != n.xfmr_res_hz {
        // The Console gives the unit once, at the end.
        line(format!(
            "Tube speaker resonance: {} → {} Hz",
            format_val(o.xfmr_res_hz),
            format_val(n.xfmr_res_hz)
        ));
    }
    if o.mix_pct != n.mix_pct {
        line(format!("Tube mix: {}", pair(o.mix_pct, n.mix_pct, "%")));
    }
    if o.trim_db != n.trim_db {
        line(format!(
            "Tube output trim: {}",
            pair(o.trim_db, n.trim_db, " dB")
        ));
    }
}

/// The Console's `limiterLinkGroupName` (Constants.swift:448-450).
pub fn limiter_link_group_name(group: u8) -> String {
    if group == 0 {
        "Unlinked".into()
    } else {
        format!("Group {group}")
    }
}

fn diff_limiter(name: &str, o: &LimiterOutput, n: &LimiterOutput, out: &mut Vec<DiffLine>) {
    let mut line = |text: String| {
        out.push(DiffLine {
            category: "Limiter",
            text,
        })
    };
    if o.enabled != n.enabled {
        line(format!("{name} limiter: {}", enabled(n.enabled)));
    }
    if o.threshold_db != n.threshold_db {
        line(format!(
            "{name} limiter threshold: {:.1} → {:.1} dBFS",
            o.threshold_db, n.threshold_db
        ));
    }
    if o.release_ms != n.release_ms {
        line(format!(
            "{name} limiter release: {} ms → {} ms",
            format_val(o.release_ms),
            format_val(n.release_ms)
        ));
    }
    if o.link_group != n.link_group {
        line(format!(
            "{name} limiter link: {} → {}",
            limiter_link_group_name(o.link_group),
            limiter_link_group_name(n.link_group)
        ));
    }
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

/// Whether the wire name `name` is the firmware's default for output
/// channel `ch` under the slot types `types` (the I2S block's
/// `output_types`, bulk_params.h:155): `get_default_channel_name`,
/// usb_audio.c:318-330, names the last channel `PDM` and the others
/// `<SPDIF|I2S> <slot> <L|R>`, I2S for a slot of `OUTPUT_TYPE_I2S`
/// (config.h:690). Written into a stack buffer: [`PresetSnapshot::matches`]
/// must not allocate.
fn is_default_output_name(
    name: &[u8],
    ch: usize,
    num_inputs: usize,
    num_outputs: usize,
    types: &[u8],
) -> bool {
    use std::io::Write;
    if ch < num_inputs || ch >= num_inputs + num_outputs {
        return false;
    }
    let name = &name[..name.iter().position(|&c| c == 0).unwrap_or(name.len())];
    if ch + 1 == num_inputs + num_outputs {
        return name == b"PDM";
    }
    let (slot, side) = ((ch - num_inputs) / 2, (ch - num_inputs) % 2);
    let prefix = if types.get(slot) == Some(&1) {
        "I2S"
    } else {
        "SPDIF"
    };
    let mut buf = [0u8; 32];
    let mut w = &mut buf[..];
    let side = if side == 0 { 'L' } else { 'R' };
    if write!(w, "{prefix} {} {side}", slot + 1).is_err() {
        return false;
    }
    let len = 32 - w.len();
    name == &buf[..len]
}

fn section(name: &str) -> (&'static str, usize, usize) {
    *dspi_proto::generated::SECTIONS
        .iter()
        .find(|(n, _, _)| *n == name)
        .expect("a known section name")
}

fn sec<'a>(bytes: &'a [u8], name: &str) -> &'a [u8] {
    let (_, off, len) = section(name);
    &bytes[off..off + len]
}

// Section decoders, by `bulk_params.h` layout.

fn decode_global(b: &[u8]) -> Global {
    Global {
        preamp_db: f32_at(b, 0),
        bypass: b[4] != 0,
        loudness_enabled: b[5] != 0,
        loudness_output_mask: u16_at(b, 6),
        loudness_ref_spl: f32_at(b, 8),
        loudness_intensity_pct: f32_at(b, 12),
    }
}

fn decode_crossfeed(b: &[u8]) -> Crossfeed {
    Crossfeed {
        enabled: b[0] != 0,
        preset: b[1],
        itd_enabled: b[2] != 0,
        output_pair_mask: b[3],
        custom_fc: f32_at(b, 4),
        custom_feed_db: f32_at(b, 8),
    }
}

fn decode_output(b: &[u8]) -> Output {
    Output {
        enabled: b[0] != 0,
        mute: b[1] != 0,
        gain_db: f32_at(b, 4),
        delay_ms: f32_at(b, 8),
    }
}

fn decode_leveller(b: &[u8]) -> Leveller {
    Leveller {
        enabled: b[0] != 0,
        speed: b[1],
        lookahead: b[2] != 0,
        amount_pct: f32_at(b, 4),
        max_gain_db: f32_at(b, 8),
        gate_threshold_db: f32_at(b, 12),
        detector_mask: b[16],
        apply_mask: b[17],
    }
}

fn decode_psybass(b: &[u8]) -> Psybass {
    Psybass {
        enabled: b[0] != 0,
        output_mask: u16_at(b, 2),
        cutoff_hz: f32_at(b, 4),
        harmonics_db: f32_at(b, 8),
        drive_db: f32_at(b, 12),
        character_pct: f32_at(b, 16),
        original_db: f32_at(b, 20),
    }
}

fn decode_upmix(b: &[u8]) -> Upmix {
    Upmix {
        enabled: b[0] != 0,
        center_mode: b[1],
        surround_mode: b[2],
        presence_db: (b[3] as i8) as f32 / 2.0,
        strength: f32_at(b, 4),
        center_width: f32_at(b, 8),
        threshold: f32_at(b, 12),
        attack_ms: f32_at(b, 16),
        release_ms: f32_at(b, 20),
        det_hpf_hz: f32_at(b, 24),
        sur_delay_ms: f32_at(b, 28),
        sur_hpf_hz: f32_at(b, 32),
        sur_lpf_hz: f32_at(b, 36),
        decorr: f32_at(b, 40),
    }
}

/// bulk_params.h:385-400: two bytes, the mask, seven floats, two bytes and
/// two reserved.
fn decode_subharm(b: &[u8]) -> Subharm {
    Subharm {
        enabled: b[0] != 0,
        output_mask: u16_at(b, 2),
        low_db: f32_at(b, 4),
        high_db: f32_at(b, 8),
        boost_db: f32_at(b, 12),
        top_db: f32_at(b, 16),
        select_depth_pct: f32_at(b, 20),
        select_hold_ms: f32_at(b, 24),
        ceiling_db: f32_at(b, 28),
        select_mode: b[32],
        link_pairs: b[33] != 0,
    }
}

/// bulk_params.h:410-427: four bytes, the mask, two reserved, then nine
/// floats and a reserved one.
fn decode_tube(b: &[u8]) -> Tube {
    Tube {
        enabled: b[0] != 0,
        tube_type: b[1],
        rectifier: b[2],
        xfmr_enabled: b[3] != 0,
        output_mask: u16_at(b, 4),
        drive_db: f32_at(b, 8),
        bias_pct: f32_at(b, 12),
        asym_db: f32_at(b, 16),
        hardness_pct: f32_at(b, 20),
        sag_pct: f32_at(b, 24),
        xfmr_damping: f32_at(b, 28),
        xfmr_res_hz: f32_at(b, 32),
        mix_pct: f32_at(b, 36),
        trim_db: f32_at(b, 40),
    }
}

/// bulk_params.h:436-442: one 12-byte record.
fn decode_limiter(b: &[u8]) -> LimiterOutput {
    LimiterOutput {
        enabled: b[0] != 0,
        link_group: b[1],
        threshold_db: f32_at(b, 4),
        release_ms: f32_at(b, 8),
    }
}

/// What a notification did to the state, for the screens to react to.
#[derive(Debug, Clone, PartialEq)]
pub enum Applied {
    /// Bytes landed in this section; the screens showing it should redraw.
    Section {
        name: &'static str,
        source: Source,
    },
    /// The shadow is stale; the caller must do a full re-read.
    NeedsReread {
        source: Source,
    },
    /// A preset finished loading (a re-read follows from the device).
    PresetLoaded {
        slot: u8,
    },
    /// The active input count changed; the channel list should re-lay-out.
    InputFormat {
        channels: u8,
    },
    /// A discrete status event the sub-state kept.
    Status(Event),
    Nothing,
}

/// The device state.
#[derive(Debug, Clone)]
pub struct DeviceState {
    pub caps: Capabilities,
    pub bulk: BulkPacket,
    pub meters: Meters,
    /// Sticky clip flags, OR'd from every meter read until cleared, as the
    /// Console's `clipLatched`.
    pub clip_latched: u32,
    /// The snapshot the `*` marker compares against.
    pub saved: Option<PresetSnapshot>,
    /// The last discrete events, for the status views.
    pub siggen_state: Option<(u8, u8, u8, u8)>,
    pub adat_state: Option<(bool, bool, u8)>,
    pub i2s_slave_state: Option<(u8, u32)>,
    pub adat_input_state: Option<(u8, u32, u8)>,
    pub ir_learn: Option<(u8, u8, u32)>,
    /// The last auxiliary output change, `(slot, state, level_q8)`
    /// (`NOTIFY_EVT_CS_AUX`, notify.h:78-83). The per-slot view is
    /// [`Self::cs_aux_states`]; this keeps the event itself for the status
    /// views.
    pub cs_aux: Option<(u8, u8, u16)>,
    /// Every auxiliary output's state and level (`REQ_GET_CS_AUX_STATE` with
    /// `wValue = 0xFFFF`, config.h:138-140). `None` until something reads it
    /// with [`Self::refresh_cs_aux`]; after that every `NOTIFY_EVT_CS_AUX`
    /// patches its slot, so it stays current without polling.
    pub cs_aux_states: Option<dspi_proto::packets::CsAuxStates>,
    /// The subharmonic synthesizer's solo (`REQ_GET_SUBHARM_SOLO`,
    /// config.h:202). Runtime only, not on the wire and never notified, so a
    /// panel that shows it polls [`Self::refresh_subharm_solo`]. `None` until
    /// read, and on a device without the feature.
    pub subharm_solo: Option<bool>,
    /// The worst-case gain the synthesizer adds, dB (`REQ_GET_SUBHARM_HEADROOM`,
    /// config.h:194); 0 while it is off. Not on the wire either.
    pub subharm_headroom_db: Option<f32>,
    /// The synthesized sub's peak per output (`REQ_GET_SUBHARM_METER`,
    /// config.h:200), polled while a panel shows it.
    pub subharm_meter: Option<dspi_proto::packets::SubharmMeter>,
    /// Gain reduction per output (`REQ_LIMITER` index `LIMITER_GET_METER`,
    /// limiter.h:19), polled while a page shows it.
    pub limiter_meter: Option<dspi_proto::packets::LimiterMeter>,
    /// Whether the lookahead delay is in the path (`LIMITER_GET_STATUS`,
    /// limiter.h:20).
    pub limiter_status: Option<dspi_proto::packets::LimiterStatus>,
    /// `OUTPUT_CONFIG_MODE_*` (config.h:497-498), from the preset directory.
    /// It decides whether a limiter edit is a preset change or an output
    /// configuration change. WITH_PRESET, the firmware's default, until read.
    pub output_config_mode: u8,
    /// `MASTER_VOLUME_MODE_*` (config.h:484-485), from the preset directory.
    /// It decides whether a master-volume change is a preset change.
    /// INDEPENDENT until read, as the Console's `presetMasterVolumeMode`.
    pub master_volume_mode: u8,
    /// The limiter records as they stood before the first edit since the
    /// output configuration was last saved, in INDEPENDENT mode: the
    /// limiter part of the Console's `OutputConfigSnapshot`. See
    /// [`Self::begin_limiter_edit`].
    pub limiter_baseline: Option<Vec<u8>>,
    /// The upmixer's telemetry (`REQ_UPMIX_GET_STATUS`, config.h:187-191).
    ///
    /// The notification endpoint does not carry it, so unlike the sub-states
    /// above this one is polled: the runner refreshes it once a second while
    /// the upmixer panel is showing its gauges, and it stays `None` on a
    /// device without the feature.
    pub upmix_status: Option<dspi_proto::packets::UpmixStatus>,
    /// Set when a notification said the shadow can no longer be trusted.
    pub stale: bool,
    /// False once the transport or the notification reader reports the
    /// device gone. The shell's dot and dimming follow this.
    pub connected: bool,
}

impl DeviceState {
    pub fn new(caps: Capabilities, bulk: BulkPacket) -> Self {
        let n = caps.num_channels as usize;
        let mut s = Self {
            caps,
            bulk,
            meters: Meters {
                peaks: vec![0.0; n],
                clipped: vec![false; n],
                cpu0: 0,
                cpu1: 0,
                active_inputs: 0,
            },
            clip_latched: 0,
            saved: None,
            siggen_state: None,
            adat_state: None,
            i2s_slave_state: None,
            adat_input_state: None,
            ir_learn: None,
            cs_aux: None,
            cs_aux_states: None,
            subharm_solo: None,
            subharm_headroom_db: None,
            subharm_meter: None,
            limiter_meter: None,
            limiter_status: None,
            output_config_mode: output_config_mode::WITH_PRESET,
            master_volume_mode: master_volume_mode::INDEPENDENT,
            limiter_baseline: None,
            upmix_status: None,
            stale: false,
            connected: true,
        };
        s.mark_saved();
        s
    }

    /// Replace the shadow after a full read.
    pub fn replace_bulk(&mut self, bulk: BulkPacket) {
        self.bulk = bulk;
        self.stale = false;
    }

    /// Take the current state as the saved baseline: on connect, after a
    /// preset load, and after a preset save.
    pub fn mark_saved(&mut self) {
        self.saved = Some(PresetSnapshot::capture(self));
    }

    pub fn has_unsaved_changes(&self) -> bool {
        match &self.saved {
            Some(s) => !s.matches(
                self.bulk.as_bytes(),
                self.output_config_mode,
                self.master_volume_mode,
            ),
            None => false,
        }
    }

    /// What has changed since the baseline, in the Console's words.
    pub fn unsaved_diff(&self) -> Vec<DiffLine> {
        let Some(saved) = &self.saved else {
            return Vec::new();
        };
        saved.diff(
            &PresetSnapshot::capture(self),
            self.caps.num_inputs as usize,
            self.caps.num_outputs as usize,
        )
    }

    /// Fold a meter read in, keeping the clip latch.
    pub fn update_meters(&mut self, m: Meters) {
        for (i, c) in m.clipped.iter().enumerate() {
            if *c {
                self.clip_latched |= 1u32.checked_shl(i as u32).unwrap_or(0);
            }
        }
        self.meters = m;
    }

    pub fn clear_clip_latch(&mut self) {
        self.clip_latched = 0;
    }

    pub fn is_clipped(&self, channel: usize) -> bool {
        self.clip_latched & 1u32.checked_shl(channel as u32).unwrap_or(0) != 0
    }

    /// Apply one notification to the shadow.
    pub fn apply(&mut self, n: &Notification) -> Applied {
        if n.lost {
            self.stale = true;
        }
        let applied = self.apply_event(n);
        // A lost packet carried something this shadow will never see; the
        // only recovery is a full re-read, whatever the surviving packet did.
        match applied {
            Applied::NeedsReread { .. } | Applied::PresetLoaded { .. } => applied,
            _ if n.lost => Applied::NeedsReread {
                source: Source::Unknown,
            },
            _ => applied,
        }
    }

    fn apply_event(&mut self, n: &Notification) -> Applied {
        match &n.event {
            Event::ParamChanged {
                offset,
                source,
                bytes,
            } => match self.bulk.patch(*offset as usize, bytes) {
                Some(name) => Applied::Section {
                    name,
                    source: *source,
                },
                None => {
                    self.stale = true;
                    Applied::NeedsReread { source: *source }
                }
            },
            Event::BulkInvalidated { source } => {
                self.stale = true;
                Applied::NeedsReread { source: *source }
            }
            Event::PresetLoaded { slot } => Applied::PresetLoaded { slot: *slot },
            Event::InputFormat { channels } => Applied::InputFormat {
                channels: *channels,
            },
            Event::SiggenState {
                state,
                reason,
                signal_type,
                channel,
            } => {
                self.siggen_state = Some((*state, *reason, *signal_type, *channel));
                Applied::Status(n.event.clone())
            }
            Event::AdatState {
                enabled,
                active,
                pin,
            } => {
                self.adat_state = Some((*enabled, *active, *pin));
                Applied::Status(n.event.clone())
            }
            Event::I2sSlaveState { state, rate_hz } => {
                self.i2s_slave_state = Some((*state, *rate_hz));
                Applied::Status(n.event.clone())
            }
            Event::AdatInputState {
                state,
                rate_hz,
                clock_mode,
            } => {
                self.adat_input_state = Some((*state, *rate_hz, *clock_mode));
                Applied::Status(n.event.clone())
            }
            Event::IrLearn {
                state,
                protocol,
                code,
            } => {
                self.ir_learn = Some((*state, *protocol, *code));
                Applied::Status(n.event.clone())
            }
            Event::CsAux {
                slot,
                state,
                level_q8,
                ..
            } => {
                self.cs_aux = Some((*slot, *state, *level_q8));
                // Patch the slot into the block once it has been read. A slot
                // past the block is ignored rather than trusted.
                if let Some(all) = self.cs_aux_states.as_mut()
                    && let (Some(s), Some(l)) = (
                        all.state.get_mut(*slot as usize),
                        all.level_q8.get_mut(*slot as usize),
                    )
                {
                    *s = *state;
                    *l = *level_q8;
                }
                Applied::Status(n.event.clone())
            }
            Event::MasterVolume(_) | Event::Idle => Applied::Nothing,
            Event::Unknown { .. } => {
                if n.lost {
                    Applied::NeedsReread {
                        source: Source::Unknown,
                    }
                } else {
                    Applied::Nothing
                }
            }
        }
    }

    // Typed views over the shadow.

    pub fn global(&self) -> Global {
        decode_global(sec(self.bulk.as_bytes(), "global"))
    }

    pub fn crossfeed(&self) -> Crossfeed {
        decode_crossfeed(sec(self.bulk.as_bytes(), "crossfeed"))
    }

    /// Per-channel delay in ms, unified channel index.
    pub fn delay_ms(&self, channel: usize) -> f32 {
        let d = sec(self.bulk.as_bytes(), "delays");
        if channel * 4 + 4 <= d.len() {
            f32_at(d, channel * 4)
        } else {
            0.0
        }
    }

    /// `[input][output]`; the wire array is always nine outputs wide.
    pub fn crosspoint(&self, input: usize, output: usize) -> Crosspoint {
        let c = sec(self.bulk.as_bytes(), "crosspoints");
        let off = (input * 9 + output) * 8;
        if input >= 8 || output >= 9 || off + 8 > c.len() {
            return Crosspoint::default();
        }
        Crosspoint {
            enabled: c[off] != 0,
            phase_invert: c[off + 1] != 0,
            gain_db: f32_at(c, off + 4),
        }
    }

    pub fn output(&self, output: usize) -> Output {
        let o = sec(self.bulk.as_bytes(), "outputs");
        if output >= 9 {
            return Output::default();
        }
        decode_output(&o[output * 12..output * 12 + 12])
    }

    /// `WirePinConfig`: `num_pin_outputs`, then the pins.
    pub fn output_pins(&self) -> Vec<u8> {
        let p = sec(self.bulk.as_bytes(), "pins");
        let n = (p[0] as usize).min(5);
        p[1..1 + n].to_vec()
    }

    pub fn band(&self, channel: u8, band: u8) -> Option<EqParamPacket> {
        self.bulk.band(channel, band)
    }

    pub fn xover_band(&self, channel: u8, band: u8) -> Option<EqParamPacket> {
        self.bulk.xover_band(channel, band)
    }

    /// The live PEQ bands of a channel, `max_bands` of them.
    pub fn bands(&self, channel: u8) -> Vec<EqParamPacket> {
        (0..self.caps.max_bands)
            .filter_map(|b| self.band(channel, b))
            .collect()
    }

    pub fn xover_bands(&self, channel: u8) -> Vec<EqParamPacket> {
        (0..4).filter_map(|b| self.xover_band(channel, b)).collect()
    }

    /// Whether any band on the channel is not `Flat`.
    pub fn has_active_bands(&self, channel: u8) -> bool {
        self.bands(channel)
            .iter()
            .any(|b| b.filter_type != FilterType::Flat)
    }

    pub fn channel_name(&self, channel: usize) -> String {
        let n = sec(self.bulk.as_bytes(), "channel_names");
        if channel >= 17 {
            return String::new();
        }
        cstr(&n[channel * 32..channel * 32 + 32])
    }

    pub fn channel_names(&self) -> Vec<String> {
        (0..self.caps.num_channels as usize)
            .map(|c| self.channel_name(c))
            .collect()
    }

    pub fn i2s(&self) -> I2sConfig {
        let b = sec(self.bulk.as_bytes(), "i2s_config");
        I2sConfig {
            output_types: [b[0], b[1], b[2], b[3]],
            bck_pin: b[4],
            mck_pin: b[5],
            mck_enabled: b[6] != 0,
            mck_multiplier: b[7],
            clock_pin_mode: dspi_proto::wire::decode_p1(b[8]),
            bck_pin_slave: b[9],
        }
    }

    pub fn leveller(&self) -> Leveller {
        decode_leveller(sec(self.bulk.as_bytes(), "leveller"))
    }

    pub fn preamp_db(&self, input: usize) -> f32 {
        let p = sec(self.bulk.as_bytes(), "preamp");
        if input < 8 { f32_at(p, input * 4) } else { 0.0 }
    }

    pub fn master_volume_db(&self) -> f32 {
        f32_at(sec(self.bulk.as_bytes(), "master_volume"), 0)
    }

    pub fn input_config(&self) -> Option<InputConfig> {
        self.bulk.input_config()
    }

    pub fn lg_sound_sync(&self) -> LgSoundSync {
        let b = sec(self.bulk.as_bytes(), "lg_sound_sync");
        LgSoundSync {
            enabled: b[0] != 0,
            present: b[1] != 0,
            volume: b[2],
            muted: b[3] != 0,
        }
    }

    /// `(user_volume_db, user_mute)`.
    pub fn user_volume(&self) -> (f32, bool) {
        let b = sec(self.bulk.as_bytes(), "user_volume");
        (f32_at(b, 0), b[4] != 0)
    }

    pub fn dac_hw_mute(&self) -> DacHwMute {
        let b = sec(self.bulk.as_bytes(), "dac_hw_mute");
        DacHwMute {
            enabled: b[0] != 0,
            active_low: b[1] != 0,
            pin: b[2],
            hold_ms: u16_at(b, 4),
            release_ms: u16_at(b, 6),
        }
    }

    /// `(enabled, pin)` for the ADAT output.
    pub fn adat_output(&self) -> (bool, u8) {
        let b = sec(self.bulk.as_bytes(), "adat_config");
        (b[0] != 0, b[1])
    }

    pub fn psybass(&self) -> Psybass {
        decode_psybass(sec(self.bulk.as_bytes(), "psybass"))
    }

    pub fn upmix(&self) -> Upmix {
        decode_upmix(sec(self.bulk.as_bytes(), "upmix"))
    }

    pub fn subharm(&self) -> Subharm {
        decode_subharm(sec(self.bulk.as_bytes(), "subharm"))
    }

    pub fn tube(&self) -> Tube {
        decode_tube(sec(self.bulk.as_bytes(), "tube"))
    }

    /// One output's limiter. The wire always carries nine records; the ones
    /// past this device's outputs are zero on a read and ignored on a write
    /// (bulk_params.h:432-434), so they are `None` here.
    pub fn limiter(&self, output: usize) -> Option<LimiterOutput> {
        if output >= self.caps.num_outputs as usize {
            return None;
        }
        let l = sec(self.bulk.as_bytes(), "limiter");
        l.get(output * 12..output * 12 + 12).map(decode_limiter)
    }

    /// Every output's limiter, in output order.
    pub fn limiters(&self) -> Vec<LimiterOutput> {
        (0..self.caps.num_outputs as usize)
            .filter_map(|o| self.limiter(o))
            .collect()
    }

    // The limiters as output configuration.
    //
    // In INDEPENDENT mode a limiter edit is not a preset change: it lives
    // with the pins and the clocks, is stored by Save Output Configuration
    // (0x52, config.h:259-269) and is put back by Revert, as part of the
    // output-config category of the Console's save bar
    // (DSPi_ConsoleApp.swift:950-1115).
    // Limiter edits are made from the output page, not from Settings, so the
    // baseline lives here, on the state every screen shares, rather than in
    // the Settings screen, which is rebuilt each time it opens.

    fn limiter_bytes(&self) -> Vec<u8> {
        sec(self.bulk.as_bytes(), "limiter").to_vec()
    }

    /// Call before a write that may change a limiter: keeps the records it
    /// is about to replace, the Console's `beginOutputEdit`
    /// (DSPi_ConsoleApp.swift:1109-1115). Only in INDEPENDENT mode, and only
    /// when nothing is unsaved yet, so the baseline stays the saved state
    /// across a run of edits.
    pub fn begin_limiter_edit(&mut self) {
        if self.output_config_mode != output_config_mode::INDEPENDENT || self.limiter_unsaved() {
            return;
        }
        self.limiter_baseline = Some(self.limiter_bytes());
    }

    /// Whether the limiters differ from the saved output configuration.
    ///
    /// Compared rather than flagged, which is the one departure from the
    /// Console: an edit put back by hand, or by Revert, reads as saved again,
    /// the way the preset marker does.
    pub fn limiter_unsaved(&self) -> bool {
        self.output_config_mode == output_config_mode::INDEPENDENT
            && self
                .limiter_baseline
                .as_deref()
                .is_some_and(|b| b != sec(self.bulk.as_bytes(), "limiter"))
    }

    /// The output configuration was saved (0x52): the live limiters are the
    /// saved ones now.
    pub fn limiter_saved(&mut self) {
        self.limiter_baseline = None;
    }

    /// The commands that put the baseline back, in the Console's order
    /// (`applyLimiterSettings`, Commands.swift:1658-1672): unlink the outputs
    /// that will change, so that one write does not move a whole group
    /// (limiter.c:143-194), then threshold, release and enable, then the
    /// groups again in ascending order, each joining output adopting the
    /// settings its lowest member already has.
    pub fn limiter_restore_commands(&self) -> Vec<String> {
        let Some(base) = self.limiter_baseline.as_deref() else {
            return Vec::new();
        };
        let targets: Vec<(usize, LimiterOutput)> = (0..self.caps.num_outputs as usize)
            .filter_map(|o| {
                let want = decode_limiter(base.get(o * 12..o * 12 + 12)?);
                (self.limiter(o)? != want).then_some((o, want))
            })
            .collect();
        let mut out = Vec::new();
        for (o, _) in &targets {
            if self.limiter(*o).is_some_and(|l| l.link_group != 0) {
                out.push(format!("limit.link {o} 0"));
            }
        }
        for (o, want) in &targets {
            out.push(format!("limit.threshold {o} {}", want.threshold_db));
            out.push(format!("limit.release {o} {}", want.release_ms));
            out.push(format!("limit.on {o} {}", on_off(want.enabled)));
        }
        for (o, want) in &targets {
            if want.link_group != 0 {
                out.push(format!("limit.link {o} {}", want.link_group));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::evt;
    use dspi_proto::Platform;
    use dspi_proto::generated::{BULK_SIZE, wire::WIRE_FORMAT_VERSION};

    /// A V28 packet for an RP2350 with a few fields set.
    fn packet() -> Vec<u8> {
        let mut b = vec![0u8; BULK_SIZE];
        b[0] = WIRE_FORMAT_VERSION as u8;
        b[1] = 1; // RP2350
        b[2] = 17;
        b[3] = 9;
        b[4] = 8;
        b[5] = 12;
        b[6..8].copy_from_slice(&(BULK_SIZE as u16).to_le_bytes());
        let (_, g, _) = section("global");
        b[g..g + 4].copy_from_slice(&(-5.3f32).to_le_bytes());
        let (_, u, _) = section("user_volume");
        b[u..u + 4].copy_from_slice(&(-12.0f32).to_le_bytes());
        let (_, n, _) = section("channel_names");
        b[n..n + 2].copy_from_slice(b"FL");
        b[n + 32..n + 34].copy_from_slice(b"FR");
        let (_, c, _) = section("crosspoints");
        b[c] = 1;
        b[c + 4..c + 8].copy_from_slice(&(-3.0f32).to_le_bytes());
        b
    }

    fn caps() -> Capabilities {
        Capabilities {
            serial: "TEST".into(),
            platform: Platform::Rp2350,
            firmware: "1.1.6".into(),
            firmware_version: dspi_proto::packets::FirmwareVersion::new(1, 1, 6, 0),
            build_info: None,
            wire_format: WIRE_FORMAT_VERSION as u8,
            num_channels: 17,
            num_inputs: 8,
            num_outputs: 9,
            max_bands: 10,
            band_storage: 12,
            channels: Vec::new(),
            features: Vec::new(),
            cs: None,
            siggen: None,
            active_preset: Some(3),
        }
    }

    fn state() -> DeviceState {
        DeviceState::new(caps(), BulkPacket::decode(packet()).unwrap())
    }

    /// The clip latch is a 32-bit word: a channel past it, from a confused
    /// device, is never latched rather than overflowing the shift.
    #[test]
    fn a_channel_past_the_clip_latch_is_not_clipped() {
        let mut s = state();
        s.update_meters(Meters {
            clipped: vec![true; 40],
            ..Meters::default()
        });
        assert!(s.is_clipped(31));
        assert!(!s.is_clipped(32) && !s.is_clipped(39));
    }

    #[test]
    fn typed_views_read_the_shadow() {
        let s = state();
        assert_eq!(s.global().preamp_db, -5.3);
        assert_eq!(s.user_volume(), (-12.0, false));
        assert_eq!(s.channel_name(0), "FL");
        assert_eq!(s.channel_name(1), "FR");
        assert_eq!(
            s.crosspoint(0, 0),
            Crosspoint {
                enabled: true,
                phase_invert: false,
                gain_db: -3.0
            }
        );
        assert_eq!(s.crosspoint(7, 8), Crosspoint::default());
        assert_eq!(s.output(8), Output::default());
        assert!(!s.has_active_bands(0));
        assert_eq!(
            s.bands(0).len(),
            10,
            "max_bands live bands, not the wire's twelve"
        );
    }

    #[test]
    fn a_param_change_patches_the_shadow_and_names_its_section() {
        let mut s = state();
        let (_, off, _) = section("user_volume");
        let n = Notification {
            seq: 1,
            event: Event::ParamChanged {
                offset: off as u16,
                source: Source::Uac1,
                bytes: (-20.0f32).to_le_bytes().to_vec(),
            },
            lost: false,
        };
        assert_eq!(
            s.apply(&n),
            Applied::Section {
                name: "user_volume",
                source: Source::Uac1
            }
        );
        assert_eq!(s.user_volume().0, -20.0);
        assert!(!s.stale);
    }

    #[test]
    fn a_change_into_the_header_or_past_the_end_marks_the_shadow_stale() {
        let mut s = state();
        let n = Notification {
            seq: 1,
            event: Event::ParamChanged {
                offset: 2,
                source: Source::Gpio,
                bytes: vec![0],
            },
            lost: false,
        };
        assert!(matches!(s.apply(&n), Applied::NeedsReread { .. }));
        assert!(s.stale);
        let mut s = state();
        let n = Notification {
            seq: 5,
            event: Event::BulkInvalidated {
                source: Source::Preset,
            },
            lost: false,
        };
        assert!(matches!(
            s.apply(&n),
            Applied::NeedsReread {
                source: Source::Preset
            }
        ));
        assert!(s.stale);
        let mut s = state();
        let n = Notification {
            seq: 9,
            event: Event::PresetLoaded { slot: 2 },
            lost: true,
        };
        assert_eq!(s.apply(&n), Applied::PresetLoaded { slot: 2 });
        assert!(s.stale, "a lost packet always means a re-read");
    }

    #[test]
    fn discrete_events_land_in_their_sub_state() {
        let mut s = state();
        let (_, e) = crate::notify::decode(&[2, evt::SIGGEN_STATE, 0, 1, 1, 0, 4, 0xFF]).unwrap();
        s.apply(&Notification {
            seq: 1,
            event: e,
            lost: false,
        });
        assert_eq!(s.siggen_state, Some((1, 0, 4, 0xFF)));
        let (_, e) = crate::notify::decode(&[
            2,
            evt::CS_IR_LEARN,
            0,
            2,
            2,
            1,
            0,
            0,
            0x78,
            0x56,
            0x34,
            0x12,
        ])
        .unwrap();
        s.apply(&Notification {
            seq: 2,
            event: e,
            lost: false,
        });
        assert_eq!(s.ir_learn, Some((2, 1, 0x1234_5678)));
    }

    /// An aux change is kept for the status views, like the other runtime
    /// events, and never touches the bulk shadow.
    #[test]
    fn an_aux_output_change_is_kept_not_patched() {
        let mut s = state();
        let before = s.bulk.as_bytes().to_vec();
        let (_, e) = crate::notify::decode(&[2, evt::CS_AUX, 0, 7, 4, 1, 0x80, 0x0C, 5]).unwrap();
        let applied = s.apply(&Notification {
            seq: 7,
            event: e,
            lost: false,
        });
        assert!(matches!(applied, Applied::Status(Event::CsAux { .. })));
        assert_eq!(s.cs_aux, Some((4, 1, 0x0C80)));
        assert_eq!(s.bulk.as_bytes(), &before[..]);
    }

    #[test]
    fn the_clip_latch_is_sticky_until_cleared() {
        let mut s = state();
        s.update_meters(Meters {
            peaks: vec![0.0; 17],
            clipped: (0..17).map(|i| i == 16).collect(),
            cpu0: 1,
            cpu1: 2,
            active_inputs: 2,
        });
        s.update_meters(Meters {
            peaks: vec![0.0; 17],
            clipped: vec![false; 17],
            cpu0: 1,
            cpu1: 2,
            active_inputs: 2,
        });
        assert!(s.is_clipped(16));
        s.clear_clip_latch();
        assert!(!s.is_clipped(16));
    }

    #[test]
    fn the_snapshot_ignores_runtime_fields_and_describes_real_changes() {
        let mut s = state();
        assert!(!s.has_unsaved_changes());
        // LG telemetry moves on its own; it is not an edit.
        let (_, lg, _) = section("lg_sound_sync");
        s.bulk.patch(lg + 1, &[1, 42, 0]);
        assert!(!s.has_unsaved_changes());
        // A preamp change is.
        let (_, g, _) = section("global");
        s.bulk.patch(g, &(-6.0f32).to_le_bytes());
        assert!(s.has_unsaved_changes());
        // And an EQ band, a crosspoint and a rename.
        let (_, eq, _) = section("eq");
        s.bulk.patch(eq + 2 * 16, &[1]);
        let (_, c, _) = section("crosspoints");
        s.bulk.patch(c + 10 * 8, &[1]);
        let (_, n, _) = section("channel_names");
        s.bulk.patch(n + 32, b"Right\0");
        let diff = s.unsaved_diff();
        let texts: Vec<String> = diff
            .iter()
            .map(|d| format!("{}: {}", d.category, d.text))
            .collect();
        assert_eq!(
            texts,
            vec![
                "Global: Preamp -5.3 dB to -6.0 dB",
                "Matrix: FR to channel 10 routing",
                "Filters: FL EQ band 3",
                "Names: FR renamed to Right",
            ],
            "{texts:?}"
        );
        let summary = PresetSnapshot::summary(&diff, 2);
        assert!(summary.ends_with("- and 2 more"), "{summary}");
        s.mark_saved();
        assert!(!s.has_unsaved_changes());
    }

    // -- the beta4 sections -------------------------------------------------

    fn put_f32(s: &mut DeviceState, off: usize, v: f32) {
        s.bulk.patch(off, &v.to_le_bytes()).unwrap();
    }

    /// The firmware's factory subharm section (subharm.h:85-94), at 5944.
    fn default_subharm(s: &mut DeviceState) {
        let (_, o, _) = section("subharm");
        assert_eq!(o, 5944);
        s.bulk.patch(o, &[0, 0, 0xFF, 0xFF]);
        for (at, v) in [(4, 0.0), (8, 0.0), (12, 0.0), (16, -30.0)] {
            put_f32(s, o + at, v);
        }
        for (at, v) in [(20, 100.0), (24, 150.0), (28, 0.0)] {
            put_f32(s, o + at, v);
        }
        s.bulk.patch(o + 32, &[0, 1]);
    }

    /// The firmware's factory tube section (tube.h:60-73), at 5980.
    fn default_tube(s: &mut DeviceState) {
        let (_, o, _) = section("tube");
        assert_eq!(o, 5980);
        s.bulk.patch(o, &[0, 1, 1, 1, 0xFF, 0xFF]);
        for (i, v) in [-12.0, 10.0, 3.0, 40.0, 15.0, 2.0, 95.0, 100.0, 0.0]
            .iter()
            .enumerate()
        {
            put_f32(s, o + 8 + 4 * i, *v);
        }
    }

    /// Every output's limiter at the firmware's defaults (limiter.h:32-33),
    /// at 6028.
    fn default_limiters(s: &mut DeviceState) {
        let (_, o, _) = section("limiter");
        assert_eq!(o, 6028);
        for k in 0..9 {
            put_f32(s, o + 12 * k + 4, -1.0);
            put_f32(s, o + 12 * k + 8, 100.0);
        }
    }

    fn beta4_state() -> DeviceState {
        let mut s = state();
        default_subharm(&mut s);
        default_tube(&mut s);
        default_limiters(&mut s);
        s.mark_saved();
        s
    }

    fn texts(s: &DeviceState) -> Vec<String> {
        s.unsaved_diff()
            .iter()
            .map(|d| format!("{}: {}", d.category, d.text))
            .collect()
    }

    #[test]
    fn the_subharm_section_decodes_field_by_field() {
        let s = beta4_state();
        assert_eq!(
            s.subharm(),
            Subharm {
                enabled: false,
                output_mask: 0xFFFF,
                low_db: 0.0,
                high_db: 0.0,
                boost_db: 0.0,
                top_db: -30.0,
                select_depth_pct: 100.0,
                select_hold_ms: 150.0,
                ceiling_db: 0.0,
                select_mode: 0,
                link_pairs: true,
            }
        );
    }

    #[test]
    fn the_tube_section_decodes_field_by_field() {
        let s = beta4_state();
        assert_eq!(
            s.tube(),
            Tube {
                enabled: false,
                tube_type: 1,
                rectifier: 1,
                xfmr_enabled: true,
                output_mask: 0xFFFF,
                drive_db: -12.0,
                bias_pct: 10.0,
                asym_db: 3.0,
                hardness_pct: 40.0,
                sag_pct: 15.0,
                xfmr_damping: 2.0,
                xfmr_res_hz: 95.0,
                mix_pct: 100.0,
                trim_db: 0.0,
            }
        );
    }

    /// Nine records on the wire whatever the device; the ones past its outputs
    /// are ignored (bulk_params.h:432-434).
    #[test]
    fn limiter_records_follow_the_output_count() {
        let mut s = beta4_state();
        let (_, o, _) = section("limiter");
        s.bulk.patch(o + 12 * 2, &[1, 3]).unwrap();
        put_f32(&mut s, o + 12 * 2 + 4, -6.5);
        assert_eq!(
            s.limiter(2),
            Some(LimiterOutput {
                enabled: true,
                link_group: 3,
                threshold_db: -6.5,
                release_ms: 100.0
            })
        );
        assert_eq!(s.limiters().len(), 9);
        s.caps.num_outputs = 5;
        assert_eq!(s.limiters().len(), 5, "an RP2040 has five outputs");
        assert_eq!(s.limiter(5), None);
    }

    /// A notification at any of the three new offsets patches the shadow in
    /// place and names its section, like every other one.
    #[test]
    fn param_changes_patch_the_new_sections() {
        let mut s = beta4_state();
        for (offset, bytes, name) in [
            (5944 + 16, (-6.0f32).to_le_bytes().to_vec(), "subharm"),
            (5980 + 1, vec![12], "tube"),
            (
                6028 + 12 * 4 + 4,
                (-3.0f32).to_le_bytes().to_vec(),
                "limiter",
            ),
        ] {
            let applied = s.apply(&Notification {
                seq: 1,
                event: Event::ParamChanged {
                    offset,
                    source: Source::Gpio,
                    bytes,
                },
                lost: false,
            });
            assert_eq!(
                applied,
                Applied::Section {
                    name,
                    source: Source::Gpio
                }
            );
        }
        assert_eq!(s.subharm().top_db, -6.0);
        assert_eq!(s.tube().tube_type, 12);
        assert_eq!(s.limiter(4).unwrap().threshold_db, -3.0);
        assert!(!s.stale);
    }

    /// A change this build cannot place must not be dropped: the shadow is
    /// marked stale and the caller re-reads, as the Console now resyncs on an
    /// offset it does not decode in place (survey-console-beta4 section 3).
    #[test]
    fn a_change_that_cannot_be_patched_asks_for_a_reread() {
        for (offset, len) in [
            // Past the end of the V32 packet.
            (BULK_SIZE as u16, 4usize),
            // The last limiter record, running off the end.
            (BULK_SIZE as u16 - 2, 4),
            // Into the header.
            (6, 2),
        ] {
            let mut s = beta4_state();
            let before = s.bulk.as_bytes().to_vec();
            let applied = s.apply(&Notification {
                seq: 1,
                event: Event::ParamChanged {
                    offset,
                    source: Source::HostSet,
                    bytes: vec![0xAA; len],
                },
                lost: false,
            });
            assert_eq!(
                applied,
                Applied::NeedsReread {
                    source: Source::HostSet
                },
                "offset {offset}"
            );
            assert!(s.stale, "offset {offset}");
            assert_eq!(s.bulk.as_bytes(), &before[..], "nothing half-patched");
        }
    }

    /// PresetSnapshot.swift:261-308, word for word, one line per field.
    #[test]
    fn subharm_changes_read_as_the_console_words_them() {
        let mut s = beta4_state();
        let (_, o, _) = section("subharm");
        s.bulk.patch(o, &[1, 0, 0x00, 0x01]).unwrap();
        put_f32(&mut s, o + 4, -30.0);
        put_f32(&mut s, o + 8, 2.5);
        put_f32(&mut s, o + 12, 3.0);
        put_f32(&mut s, o + 16, 0.0);
        put_f32(&mut s, o + 20, 75.0);
        put_f32(&mut s, o + 24, 200.0);
        put_f32(&mut s, o + 28, -12.0);
        s.bulk.patch(o + 32, &[1, 0]).unwrap();
        assert_eq!(
            texts(&s),
            vec![
                "Subharm: Subharmonic Synthesizer: enabled",
                "Subharm: Subharm outputs: 0xFFFF → 0x0100",
                "Subharm: Subharm 24-36 Hz: 0 dB → off",
                "Subharm: Subharm 36-56 Hz: 0 dB → 2.5 dB",
                "Subharm: Subharm 56-80 Hz: off → 0 dB",
                "Subharm: Subharm selectivity: all material → percussive",
                "Subharm: Subharm selectivity depth: 100% → 75%",
                "Subharm: Subharm selectivity hold: 150 ms → 200 ms",
                "Subharm: Subharm sub ceiling: off → -12 dBFS",
                "Subharm: Subharm pair link: independent",
                "Subharm: Subharm LF boost: 0 dB → 3 dB",
            ]
        );
    }

    /// PresetSnapshot.swift:310-353. A type change carries its four
    /// character values with it, which is what the device does.
    #[test]
    fn tube_changes_read_as_the_console_words_them() {
        let mut s = beta4_state();
        let (_, o, _) = section("tube");
        // Type 3 (12AT7) and its row (tube.c:52), then the rest by hand.
        s.bulk.patch(o, &[1, 3, 0, 0, 0x0F, 0x00]).unwrap();
        for (i, v) in [-6.0, 5.0, 2.0, 55.0, 10.0, 4.5, 80.0, 50.0, -1.5]
            .iter()
            .enumerate()
        {
            put_f32(&mut s, o + 8 + 4 * i, *v);
        }
        assert_eq!(
            texts(&s),
            vec![
                "Tube: Tube Modeller: enabled",
                "Tube: Tube outputs: 0xFFFF → 0x000F",
                "Tube: Tube type: 12AX7 / ECC83 → 12AT7 / ECC81",
                "Tube: Tube drive: -12 dB → -6 dB",
                "Tube: Tube bias: 10% → 5%",
                "Tube: Tube asymmetry: 3 dB → 2 dB",
                "Tube: Tube knee hardness: 40% → 55%",
                "Tube: Tube sag: 15% → 10%",
                "Tube: Tube rectifier: GZ34 / 5AR4 → Solid state",
                "Tube: Tube output stage: disabled",
                "Tube: Tube damping factor: 2 → 4.5",
                "Tube: Tube speaker resonance: 95 → 80 Hz",
                "Tube: Tube mix: 100% → 50%",
                "Tube: Tube output trim: 0 dB → -1.5 dB",
            ]
        );
        // Custom has a name of its own.
        s.mark_saved();
        s.bulk.patch(o + 1, &[0]).unwrap();
        assert_eq!(texts(&s), vec!["Tube: Tube type: 12AT7 / ECC81 → Custom"]);
    }

    fn edit_limiter(s: &mut DeviceState) {
        let (_, o, _) = section("limiter");
        // Output 1 ("OUT R" on the fixture is unnamed here: channel 9).
        s.bulk.patch(o + 12, &[1, 2]).unwrap();
        put_f32(s, o + 12 + 4, -3.5);
        put_f32(s, o + 12 + 8, 250.0);
    }

    /// PresetSnapshot.swift:355-378: in WITH_PRESET mode a limiter is part
    /// of the preset, named after its output channel.
    #[test]
    fn limiter_lines_appear_with_the_preset_mode() {
        let mut s = beta4_state();
        let (_, n, _) = section("channel_names");
        s.bulk.patch(n + 9 * 32, b"Tweeter R\0").unwrap();
        s.mark_saved();
        edit_limiter(&mut s);
        assert!(s.has_unsaved_changes());
        assert_eq!(
            texts(&s),
            vec![
                "Limiter: Tweeter R limiter: enabled",
                "Limiter: Tweeter R limiter threshold: -1.0 → -3.5 dBFS",
                "Limiter: Tweeter R limiter release: 100 ms → 250 ms",
                "Limiter: Tweeter R limiter link: Unlinked → Group 2",
            ]
        );
        assert!(
            !s.limiter_unsaved(),
            "not output configuration in this mode"
        );
        // An unnamed output falls back to the Console's zero-based label.
        s.bulk.patch(n + 9 * 32, &[0; 32]).unwrap();
        s.mark_saved();
        let (_, o, _) = section("limiter");
        s.bulk.patch(o + 12, &[0]).unwrap();
        assert_eq!(texts(&s), vec!["Limiter: Output 1 limiter: disabled"]);
    }

    /// The pins, the I2S and ADAT set-up and the input pins are output
    /// configuration too: in INDEPENDENT mode they leave the preset clean,
    /// as in the Console's diff, while the input source still counts.
    #[test]
    fn output_configuration_is_not_a_preset_change_in_independent_mode() {
        for (name, at) in [
            ("pins", 1),
            ("i2s_config", 0),
            ("adat_config", 1),
            ("input_config", 1),
        ] {
            let mut s = beta4_state();
            s.output_config_mode = output_config_mode::INDEPENDENT;
            s.mark_saved();
            let (_, o, _) = section(name);
            let was = s.bulk.as_bytes()[o + at];
            s.bulk.patch(o + at, &[was.wrapping_add(1)]).unwrap();
            assert!(!s.has_unsaved_changes(), "{name}");
            assert!(texts(&s).is_empty(), "{name}: {:?}", texts(&s));
            s.output_config_mode = output_config_mode::WITH_PRESET;
            assert!(s.has_unsaved_changes(), "{name} with the preset");
            assert!(!texts(&s).is_empty(), "{name} with the preset");
        }
        let mut s = beta4_state();
        s.output_config_mode = output_config_mode::INDEPENDENT;
        s.mark_saved();
        let (_, o, _) = section("input_config");
        let was = s.bulk.as_bytes()[o];
        s.bulk.patch(o, &[was ^ 1]).unwrap();
        assert!(s.has_unsaved_changes(), "the input source is the preset's");
    }

    /// PresetSnapshot.swift:186-198: the master volume is a preset change
    /// only in WITH_PRESET mode, gated on the live mode, so switching modes
    /// with a diverged volume dirties the preset.
    #[test]
    fn master_volume_counts_only_with_the_preset() {
        let mut s = state();
        assert_eq!(s.master_volume_mode, master_volume_mode::INDEPENDENT);
        let (_, m, _) = section("master_volume");
        put_f32(&mut s, m, -20.0);
        assert!(!s.has_unsaved_changes(), "not the preset's in INDEPENDENT");
        assert!(s.unsaved_diff().is_empty());
        s.master_volume_mode = master_volume_mode::WITH_PRESET;
        assert!(s.has_unsaved_changes());
        assert_eq!(texts(&s), vec!["Global: Master volume"]);
        s.mark_saved();
        assert!(!s.has_unsaved_changes());
    }

    fn set_name(s: &mut DeviceState, ch: usize, name: &str) {
        let (_, n, _) = section("channel_names");
        let mut b = [0u8; 32];
        b[..name.len()].copy_from_slice(name.as_bytes());
        s.bulk.patch(n + ch * 32, &b).unwrap();
    }

    /// The firmware's own names (usb_audio.c:318-330) on the fixture's
    /// shape: eight inputs, then four output pairs by slot type, PDM last.
    #[test]
    fn default_output_names_follow_the_slot_types() {
        let d = |name: &str, ch, types: &[u8]| {
            let mut b = [0u8; 32];
            b[..name.len()].copy_from_slice(name.as_bytes());
            is_default_output_name(&b, ch, 8, 9, types)
        };
        assert!(d("SPDIF 1 L", 8, &[0, 0, 0, 0]));
        assert!(d("SPDIF 1 R", 9, &[0, 0, 0, 0]));
        assert!(d("I2S 2 R", 11, &[0, 1, 0, 0]));
        assert!(!d("SPDIF 2 R", 11, &[0, 1, 0, 0]));
        assert!(d("SPDIF 4 L", 14, &[0, 0, 0, 0]));
        assert!(d("PDM", 16, &[1, 1, 1, 1]));
        assert!(!d("USB 1", 0, &[0, 0, 0, 0]), "inputs are never skipped");
        assert!(!d("SPDIF 1 L", 17, &[0, 0, 0, 0]), "past the outputs");
    }

    /// PresetSnapshot.swift:536-553: the firmware renames an output still at
    /// its default when its slot type flips (main.c:672-689), and that
    /// default-to-default rename is not an edit. A real rename still is.
    #[test]
    fn a_default_to_default_rename_is_not_a_preset_change() {
        let mut s = state();
        s.output_config_mode = output_config_mode::INDEPENDENT;
        set_name(&mut s, 8, "SPDIF 1 L");
        set_name(&mut s, 9, "SPDIF 1 R");
        s.mark_saved();
        // Slot 1 flips to I2S and the firmware relabels both sides.
        let (_, i2s, _) = section("i2s_config");
        s.bulk.patch(i2s, &[1]).unwrap();
        set_name(&mut s, 8, "I2S 1 L");
        set_name(&mut s, 9, "I2S 1 R");
        assert!(!s.has_unsaved_changes());
        assert!(texts(&s).is_empty(), "{:?}", texts(&s));
        // In WITH_PRESET mode the type change counts, the relabel still not.
        s.output_config_mode = output_config_mode::WITH_PRESET;
        assert!(s.has_unsaved_changes());
        assert_eq!(texts(&s), vec!["Hardware: I2S configuration"]);
        // A rename by hand counts.
        s.output_config_mode = output_config_mode::INDEPENDENT;
        set_name(&mut s, 8, "Woofer");
        assert!(s.has_unsaved_changes());
        assert_eq!(texts(&s), vec!["Names: SPDIF 1 L renamed to Woofer"]);
        // So does a rename to a default name that is not the slot type's.
        s.bulk.patch(i2s, &[0]).unwrap();
        set_name(&mut s, 8, "SPDIF 1 L");
        assert!(s.has_unsaved_changes(), "SPDIF 1 R to I2S 1 R on S/PDIF");
        assert_eq!(texts(&s), vec!["Names: SPDIF 1 R renamed to I2S 1 R"]);
    }

    /// In INDEPENDENT mode the limiters are output configuration: no preset
    /// change, no diff line, and the output-config category says unsaved
    /// instead.
    #[test]
    fn limiter_edits_mark_the_output_configuration_in_independent_mode() {
        let mut s = beta4_state();
        s.output_config_mode = output_config_mode::INDEPENDENT;
        s.mark_saved();
        s.begin_limiter_edit();
        edit_limiter(&mut s);
        assert!(!s.has_unsaved_changes(), "the preset is untouched");
        assert!(texts(&s).is_empty());
        assert!(s.limiter_unsaved());

        // A second edit keeps the first baseline.
        s.begin_limiter_edit();
        put_f32(&mut s, section("limiter").1 + 12 + 4, -9.0);
        assert_eq!(
            s.limiter_restore_commands(),
            vec![
                "limit.link 1 0",
                "limit.threshold 1 -1",
                "limit.release 1 100",
                "limit.on 1 off",
            ],
            "unlink first, so one write does not move the group"
        );

        // Put back by hand, it reads as saved again.
        let base = s.limiter_baseline.clone().unwrap();
        let (_, o, _) = section("limiter");
        s.bulk.patch(o, &base).unwrap();
        assert!(!s.limiter_unsaved());

        // Saved with 0x52, the baseline goes.
        s.begin_limiter_edit();
        edit_limiter(&mut s);
        s.limiter_saved();
        assert!(!s.limiter_unsaved());
        assert!(s.limiter_restore_commands().is_empty());

        // The same edit in WITH_PRESET mode never takes a baseline.
        let mut s = beta4_state();
        s.begin_limiter_edit();
        assert!(s.limiter_baseline.is_none());
    }

    /// Restoring a linked output relinks it after its values are back, so it
    /// adopts the group rather than dragging the group along.
    #[test]
    fn restoring_a_group_member_relinks_it_last() {
        let mut s = beta4_state();
        let (_, o, _) = section("limiter");
        s.bulk.patch(o + 24, &[1, 1]).unwrap();
        s.output_config_mode = output_config_mode::INDEPENDENT;
        s.begin_limiter_edit();
        s.bulk.patch(o + 24, &[0, 0]).unwrap();
        put_f32(&mut s, o + 24 + 8, 400.0);
        assert_eq!(
            s.limiter_restore_commands(),
            vec![
                "limit.threshold 2 -1",
                "limit.release 2 100",
                "limit.on 2 on",
                "limit.link 2 1",
            ]
        );
    }

    /// Solo and headroom are runtime only, so no snapshot can see them.
    #[test]
    fn solo_and_headroom_never_dirty_the_preset() {
        let mut s = beta4_state();
        s.subharm_solo = Some(true);
        s.subharm_headroom_db = Some(6.0);
        assert!(!s.has_unsaved_changes());
        assert!(texts(&s).is_empty());
    }
}
