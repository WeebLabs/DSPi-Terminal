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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetSnapshot {
    bytes: Vec<u8>,
}

impl PresetSnapshot {
    pub fn capture(state: &DeviceState) -> Self {
        let mut bytes = state.bulk.as_bytes().to_vec();
        Self::blank_runtime(&mut bytes);
        Self { bytes }
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
        if differs("pins", a, b) {
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
            for ch in 0..(num_inputs + num_outputs).min(17) {
                if na[ch * 32..ch * 32 + 32] != nb[ch * 32..ch * 32 + 32] {
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
        if differs("i2s_config", a, b) {
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
        if differs("master_volume", a, b) {
            line(&mut out, "Global", "Master volume".into());
        }
        if differs("input_config", a, b) {
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
        if differs("adat_config", a, b) {
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

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
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
            Some(s) => *s != PresetSnapshot::capture(self),
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
                self.clip_latched |= 1 << i;
            }
        }
        self.meters = m;
    }

    pub fn clear_clip_latch(&mut self) {
        self.clip_latched = 0;
    }

    pub fn is_clipped(&self, channel: usize) -> bool {
        self.clip_latched & (1 << channel) != 0
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
}
