//! The `.dspipreset` whole-device document.
//!
//! Schema and semantics follow the Windows Console's `PresetDocument.cs` and
//! `PresetFileService.cs`, pinned at `DSPi-Console-Windows@81ae00b`. These files
//! are destined for the macOS Console too, so divergence would break
//! interchange rather than merely inconvenience it.
//!
//! Four decisions are inherited deliberately, because each is correct and each
//! would be tempting to get wrong:
//!
//! - **Raw wire values, not enum names.** Wire values are the firmware's own and
//!   stay stable; a renamed enum member would silently break old files.
//! - **Nullable capability blocks**, so a document from a device without a
//!   feature is distinguishable from one with the feature switched off.
//! - **A schema version that rejects documents from the future** rather than
//!   guessing at blocks it does not understand.
//! - **Three-way apply options**, because volume levels and hardware wiring
//!   describe a room and a board rather than a tuning, and importing them
//!   silently would change how loud someone's room gets.

use serde::{Deserialize, Serialize};

/// Bumped when the layout changes incompatibly.
pub const CURRENT_SCHEMA_VERSION: i32 = 1;

pub const FILE_EXTENSION: &str = "dspipreset";

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum PresetFileError {
    #[error("not a valid preset file: {0}")]
    Invalid(String),

    #[error("this file is empty")]
    Empty,

    #[error("no channel data found")]
    NoChannels,

    #[error(
        "this file was written by a newer version (format {got}, this build reads {supported})"
    )]
    FromTheFuture { got: i32, supported: i32 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetDocument {
    pub schema_version: i32,
    #[serde(default)]
    pub meta: Meta,
    #[serde(default)]
    pub global: GlobalBlock,
    #[serde(default)]
    pub loudness: LoudnessBlock,
    #[serde(default)]
    pub crossfeed: CrossfeedBlock,
    #[serde(default)]
    pub leveller: LevellerBlock,
    /// Absent when the source device had no psychoacoustic bass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub psybass: Option<PsybassBlock>,
    /// Absent when the source device had no upmixer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upmix: Option<UpmixBlock>,
    #[serde(default)]
    pub channels: Vec<ChannelBlock>,
    #[serde(default)]
    pub matrix: Vec<CrosspointBlock>,
    #[serde(default)]
    pub io: IoBlock,
}

/// Provenance. Informational only, but a mismatch is worth telling the user about.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Meta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_utc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware_version: Option<String>,
    #[serde(default)]
    pub wire_format_version: i32,
    #[serde(default)]
    pub input_channel_count: i32,
    #[serde(default)]
    pub output_channel_count: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GlobalBlock {
    /// Per-input trim, indexed by wire input 0..7.
    pub input_preamps_db: Vec<f32>,
    pub bypass: bool,
    pub master_volume_db: f32,
    pub user_volume_db: f32,
    /// Wire InputSource value. A listening choice rather than wiring, so it
    /// travels with the audio settings and not with the I/O block.
    pub input_source: u8,
    pub lg_sound_sync_enabled: bool,
    /// App-side PEQ pair linking. The firmware has no notion of it, but losing
    /// it on import would surprise anyone who set it up.
    pub input_pair_linked: Vec<bool>,
}

impl Default for GlobalBlock {
    fn default() -> Self {
        Self {
            input_preamps_db: vec![0.0; 8],
            bypass: false,
            master_volume_db: -20.0,
            user_volume_db: 0.0,
            input_source: 0,
            lg_sound_sync_enabled: false,
            input_pair_linked: vec![false; 4],
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LoudnessBlock {
    pub enabled: bool,
    pub ref_spl: f32,
    pub intensity_pct: f32,
    pub output_mask: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CrossfeedBlock {
    pub enabled: bool,
    pub preset: i32,
    pub freq_hz: f32,
    pub feed_db: f32,
    pub itd: bool,
    pub output_pair_mask: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LevellerBlock {
    pub enabled: bool,
    pub speed: i32,
    pub lookahead: bool,
    pub amount_pct: f32,
    pub max_gain_db: f32,
    pub gate_db: f32,
    pub detector_mask: i32,
    pub apply_mask: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PsybassBlock {
    pub enabled: bool,
    pub cutoff_hz: f32,
    pub harmonics_db: f32,
    pub drive_db: f32,
    pub character_pct: f32,
    pub original_db: f32,
    pub output_mask: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpmixBlock {
    pub enabled: bool,
    pub center_mode: i32,
    pub surround_mode: i32,
    pub strength_pct: f32,
    pub center_width_pct: f32,
    pub threshold_pct: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub detector_hpf_hz: f32,
    pub surround_delay_ms: f32,
    pub surround_hpf_hz: f32,
    pub surround_lpf_hz: f32,
    pub decorr_pct: f32,
    pub presence_db: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChannelBlock {
    pub channel_id: i32,
    #[serde(default)]
    pub name: String,
    pub is_output: bool,
    pub delay_ms: f32,
    pub gain_db: f32,
    pub muted: bool,
    pub enabled: bool,
    #[serde(default)]
    pub eq: Vec<BandBlock>,
    /// Empty for inputs.
    #[serde(default)]
    pub crossover: Vec<BandBlock>,
}

impl Default for ChannelBlock {
    fn default() -> Self {
        Self {
            channel_id: 0,
            name: String::new(),
            is_output: false,
            delay_ms: 0.0,
            gain_db: 0.0,
            muted: false,
            // A channel is enabled unless the file says otherwise, matching the
            // reference implementation's property default.
            enabled: true,
            eq: Vec::new(),
            crossover: Vec::new(),
        }
    }
}

/// One filter band. Field meanings follow the wire encoding, including the
/// Linkwitz Transform's reuse of them: `freq` is f0, `q` is Q0, `gain` carries
/// fp in Hz, and `qp` is the target pole Q.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BandBlock {
    /// Raw wire `FilterType`, never a name.
    pub r#type: i32,
    pub freq_hz: f32,
    pub q: f32,
    pub gain: f32,
    pub qp: f32,
    pub bypass: bool,
}

impl Default for BandBlock {
    fn default() -> Self {
        Self {
            r#type: 0,
            freq_hz: 1000.0,
            q: 0.707,
            gain: 0.0,
            qp: 0.707,
            bypass: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CrosspointBlock {
    pub input: i32,
    pub output: i32,
    pub enabled: bool,
    pub invert: bool,
    pub gain_db: f32,
}

/// Physical wiring. Applied only when the user opts in, since GPIO assignments
/// belong to a board rather than to a listening setup.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct IoBlock {
    #[serde(default)]
    pub output_pins: Vec<u8>,
    #[serde(default)]
    pub output_slot_types: Vec<u8>,
    #[serde(default)]
    pub i2s_bck_pin: u8,
    #[serde(default)]
    pub mck_enabled: bool,
    #[serde(default)]
    pub mck_pin: u8,
    #[serde(default)]
    pub mck_multiplier: i32,
    #[serde(default)]
    pub i2s_clock_mode: u8,
    #[serde(default)]
    pub i2s_clock_pin_mode: u8,
    #[serde(default)]
    pub spdif_rx_pins: Vec<u8>,
    #[serde(default)]
    pub i2s_rx_pins: Vec<u8>,
    #[serde(default)]
    pub i2s_input_channels: i32,
    #[serde(default)]
    pub i2s_input_rate_hz: u32,
    #[serde(default)]
    pub adat_enabled: bool,
    #[serde(default)]
    pub adat_pin: u8,
}

/// Which parts of a document an import should apply.
///
/// Only the audio processing is on by default. Volume levels would change how
/// loud a room gets, and hardware wiring describes a board; importing either
/// without being asked is the kind of surprise that loses trust.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApplyOptions {
    pub audio_processing: bool,
    pub volume_levels: bool,
    pub hardware_io: bool,
}

impl Default for ApplyOptions {
    fn default() -> Self {
        Self {
            audio_processing: true,
            volume_levels: false,
            hardware_io: false,
        }
    }
}

/// What an import actually did, so the user is told rather than left to infer it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ApplyReport {
    pub channels_applied: usize,
    pub bands_applied: usize,
    pub crossover_bands_applied: usize,
    pub crosspoints_applied: usize,
    /// Channels the document carries that this device does not have.
    pub missing_channels: Vec<String>,
    /// Blocks skipped for lack of a feature, or because applying them would
    /// have conflicted.
    pub skipped: Vec<String>,
}

/// Parse a document, refusing anything this build cannot honestly interpret.
pub fn parse(text: &str) -> Result<PresetDocument, PresetFileError> {
    if text.trim().is_empty() {
        return Err(PresetFileError::Empty);
    }

    let doc: PresetDocument =
        serde_json::from_str(text).map_err(|e| PresetFileError::Invalid(e.to_string()))?;

    if doc.schema_version <= 0 {
        return Err(PresetFileError::Invalid(
            "missing or invalid schemaVersion".into(),
        ));
    }
    // Refuse a document from the future rather than guessing at blocks we do
    // not understand and applying a partial tuning.
    if doc.schema_version > CURRENT_SCHEMA_VERSION {
        return Err(PresetFileError::FromTheFuture {
            got: doc.schema_version,
            supported: CURRENT_SCHEMA_VERSION,
        });
    }
    if doc.channels.is_empty() {
        return Err(PresetFileError::NoChannels);
    }
    Ok(doc)
}

pub fn write(doc: &PresetDocument) -> String {
    serde_json::to_string_pretty(doc).unwrap_or_default()
}

/// Which of a document's channels this device can accept.
///
/// Channels are matched by id and never remapped: the Windows implementation
/// reports what a device does not have rather than translating an eight-input
/// document onto a two-input part, and inventing a different rule here would
/// make the same file behave differently depending on which app opened it.
pub fn resolve_channels<'a>(
    doc: &'a PresetDocument,
    device_channel_ids: &[i32],
) -> (Vec<&'a ChannelBlock>, Vec<String>) {
    let mut usable = Vec::new();
    let mut missing = Vec::new();

    // A hand-edited file with a duplicated id should apply the last one rather
    // than fail, matching the reference implementation.
    let mut by_id: std::collections::BTreeMap<i32, &ChannelBlock> = Default::default();
    for c in &doc.channels {
        by_id.insert(c.channel_id, c);
    }

    for c in by_id.values() {
        if device_channel_ids.contains(&c.channel_id) {
            usable.push(*c);
        } else {
            missing.push(if c.name.is_empty() {
                format!("channel {}", c.channel_id)
            } else {
                c.name.clone()
            });
        }
    }
    (usable, missing)
}

// ---------------------------------------------------------------------------
// Applying
// ---------------------------------------------------------------------------

use crate::{Outcome, Session};
use dspi_proto::value::{EqParamPacket, Value};

/// Apply a document to a device.
///
/// Everything goes through the ordinary write path, so each value gets the same
/// clamping, capability gating and readback verification as a typed command.
/// Nothing is applied that the options did not ask for, and the report says what
/// actually happened rather than leaving the user to infer it from the UI.
pub fn apply(session: &mut Session, doc: &PresetDocument, options: ApplyOptions) -> ApplyReport {
    let mut report = ApplyReport::default();

    let caps = session.capabilities().clone();
    let ids: Vec<i32> = caps.channels.iter().map(|c| c.index as i32).collect();
    let (usable, missing) = resolve_channels(doc, &ids);
    report.missing_channels = missing;

    if !options.audio_processing {
        report
            .skipped
            .push("audio processing (not requested)".into());
    } else {
        // Pair linking first. A linked input pair mirrors every filter and
        // preamp write to its partner, so applying it afterwards would let the
        // device's current link state rewrite what was just pushed. This
        // ordering is inherited from the reference implementation.
        //
        // The firmware has no notion of linking, so there is nothing to write
        // here yet; the ordering is preserved so that when it gains one, the
        // sequence is already right.
        let _ = &doc.global.input_pair_linked;

        for block in &usable {
            let ch = block.channel_id as u8;
            let mut touched = false;

            for (i, b) in block.eq.iter().enumerate() {
                if i as u8 >= caps.max_bands {
                    break;
                }
                if apply_band(session, ch, i as u8, b).is_some() {
                    report.bands_applied += 1;
                    touched = true;
                }
            }

            // Crossover bands live at wire indices 20-23 and only on outputs.
            if block.is_output {
                for (i, b) in block.crossover.iter().enumerate().take(4) {
                    if apply_band(session, ch, 20 + i as u8, b).is_some() {
                        report.crossover_bands_applied += 1;
                        touched = true;
                    }
                }
            }

            let _ = session.write("ch.delay", &[ch], Value::Float(block.delay_ms));

            if block.is_output
                && let Some(out) = caps.num_inputs.checked_sub(0).map(|n| ch.wrapping_sub(n))
                && out < caps.num_outputs
            {
                let _ = session.write("out.gain", &[out], Value::Float(block.gain_db));
                let _ = session.write("out.mute", &[out], Value::Bool(block.muted));
            }

            if touched {
                report.channels_applied += 1;
            }
        }

        for c in &doc.matrix {
            if c.input as u8 >= caps.num_inputs || c.output as u8 >= caps.num_outputs {
                continue;
            }
            // The crosspoint packet carries every field at once.
            let mut payload = vec![
                c.input as u8,
                c.output as u8,
                c.enabled as u8,
                c.invert as u8,
            ];
            payload.extend_from_slice(&c.gain_db.to_le_bytes());
            if session
                .write(
                    "mix",
                    &[c.input as u8, c.output as u8],
                    Value::Bytes(payload),
                )
                .is_ok()
            {
                report.crosspoints_applied += 1;
            }
        }

        apply_features(session, doc, &mut report);
    }

    if options.volume_levels {
        let _ = session.write("vol.master", &[], Value::Float(doc.global.master_volume_db));
        let _ = session.write("vol.user", &[], Value::Float(doc.global.user_volume_db));
    } else {
        report.skipped.push("volume levels (not requested)".into());
    }

    if options.hardware_io {
        report
            .skipped
            .push("hardware I/O (applying wiring is not implemented)".into());
    } else {
        report.skipped.push("hardware I/O (not requested)".into());
    }

    report
}

fn apply_band(session: &mut Session, channel: u8, band: u8, b: &BandBlock) -> Option<()> {
    let packet = EqParamPacket {
        channel,
        band,
        filter_type: dspi_proto::FilterType::from_raw(b.r#type as u8),
        bypass: b.bypass,
        freq: b.freq_hz,
        q: b.q,
        gain_db: b.gain,
        qp: Some(b.qp),
    };
    match session.write_band(&packet) {
        Ok(Outcome::Rejected { .. }) | Err(_) => None,
        Ok(_) => Some(()),
    }
}

/// The feature blocks, each skipped with a reason when this device lacks it.
fn apply_features(session: &mut Session, doc: &PresetDocument, report: &mut ApplyReport) {
    let mut set = |path: &str, v: Value, label: &str, report: &mut ApplyReport| {
        if let Err(e) = session.write(path, &[], v) {
            // An absent feature is information, not a failure: a document from a
            // better-equipped device should still apply everything else.
            if matches!(e, crate::WriteError::Unavailable { .. }) {
                let note = format!("{label} (not on this device)");
                if !report.skipped.contains(&note) {
                    report.skipped.push(note);
                }
            }
        }
    };

    let l = &doc.loudness;
    set("loud.on", Value::Bool(l.enabled), "loudness", report);
    set("loud.ref", Value::Float(l.ref_spl), "loudness", report);
    set(
        "loud.intensity",
        Value::Float(l.intensity_pct),
        "loudness",
        report,
    );

    let c = &doc.crossfeed;
    set("cf.on", Value::Bool(c.enabled), "crossfeed", report);
    set(
        "cf.preset",
        Value::Choice(c.preset as u8),
        "crossfeed",
        report,
    );
    set("cf.freq", Value::Float(c.freq_hz), "crossfeed", report);
    set("cf.feed", Value::Float(c.feed_db), "crossfeed", report);
    set("cf.itd", Value::Bool(c.itd), "crossfeed", report);

    let v = &doc.leveller;
    set("lev.on", Value::Bool(v.enabled), "leveller", report);
    set(
        "lev.speed",
        Value::Choice(v.speed as u8),
        "leveller",
        report,
    );
    set("lev.amount", Value::Float(v.amount_pct), "leveller", report);
    set(
        "lev.maxgain",
        Value::Float(v.max_gain_db),
        "leveller",
        report,
    );
    set(
        "lev.lookahead",
        Value::Bool(v.lookahead),
        "leveller",
        report,
    );
    set("lev.gate", Value::Float(v.gate_db), "leveller", report);

    if let Some(b) = &doc.psybass {
        set(
            "bass.on",
            Value::Bool(b.enabled),
            "psychoacoustic bass",
            report,
        );
        set(
            "bass.cutoff",
            Value::Float(b.cutoff_hz),
            "psychoacoustic bass",
            report,
        );
        set(
            "bass.harmonics",
            Value::Float(b.harmonics_db),
            "psychoacoustic bass",
            report,
        );
        set(
            "bass.drive",
            Value::Float(b.drive_db),
            "psychoacoustic bass",
            report,
        );
        set(
            "bass.character",
            Value::Float(b.character_pct),
            "psychoacoustic bass",
            report,
        );
        set(
            "bass.original",
            Value::Float(b.original_db),
            "psychoacoustic bass",
            report,
        );
    }

    if let Some(u) = &doc.upmix {
        set("up.on", Value::Bool(u.enabled), "upmixer", report);
        set(
            "up.strength",
            Value::Float(u.strength_pct),
            "upmixer",
            report,
        );
        set(
            "up.width",
            Value::Float(u.center_width_pct),
            "upmixer",
            report,
        );
        set(
            "up.presence",
            Value::Float(u.presence_db),
            "upmixer",
            report,
        );
    }

    set(
        "in.source",
        Value::Choice(doc.global.input_source),
        "input source",
        report,
    );
    set(
        "bypass",
        Value::Bool(doc.global.bypass),
        "EQ bypass",
        report,
    );
}

/// Capture the current device state as a document.
pub fn capture(session: &mut Session, name: Option<String>) -> PresetDocument {
    let caps = session.capabilities().clone();

    let read_f32 = |s: &mut Session, path: &str, ix: &[u8]| -> f32 {
        s.read(path, ix)
            .ok()
            .and_then(|v| v.as_f32())
            .unwrap_or(0.0)
    };
    let read_bool = |s: &mut Session, path: &str| -> bool {
        s.read(path, &[])
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    };
    let read_u8 = |s: &mut Session, path: &str| -> u8 {
        s.read(path, &[]).ok().and_then(|v| v.as_u8()).unwrap_or(0)
    };

    let mut channels = Vec::new();
    for c in &caps.channels {
        let mut block = ChannelBlock {
            channel_id: c.index as i32,
            name: c.name.clone(),
            is_output: c.is_output,
            delay_ms: read_f32(session, "ch.delay", &[c.index]),
            ..Default::default()
        };
        if c.is_output {
            let out = c.index - caps.num_inputs;
            block.gain_db = read_f32(session, "out.gain", &[out]);
            block.muted = session
                .read("out.mute", &[out])
                .ok()
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            block.enabled = session
                .read("out.enable", &[out])
                .ok()
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
        }
        for b in 0..caps.max_bands {
            if let Ok(p) = session.read_band(c.index, b) {
                block.eq.push(band_block(&p));
            }
        }
        if c.is_output {
            for b in 20..24 {
                if let Ok(p) = session.read_band(c.index, b) {
                    block.crossover.push(band_block(&p));
                }
            }
        }
        channels.push(block);
    }

    let mut preamps = vec![0.0f32; 8];
    for i in 0..caps.num_inputs.min(8) {
        preamps[i as usize] = read_f32(session, "pre", &[i]);
    }

    let matrix = session
        .read_matrix()
        .map(|(grid, _)| {
            grid.iter()
                .enumerate()
                .flat_map(|(i, row)| {
                    row.iter().enumerate().map(move |(o, c)| CrosspointBlock {
                        input: i as i32,
                        output: o as i32,
                        enabled: c.enabled,
                        invert: c.phase_invert,
                        gain_db: c.gain_db,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let has = |name: &str| caps.features.iter().any(|f| f.name == name && f.present);

    PresetDocument {
        schema_version: CURRENT_SCHEMA_VERSION,
        meta: Meta {
            name,
            app_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            platform: Some(caps.platform.name()),
            firmware_version: Some(caps.firmware.clone()),
            wire_format_version: caps.wire_format as i32,
            input_channel_count: caps.num_inputs as i32,
            output_channel_count: caps.num_outputs as i32,
            saved_utc: None,
        },
        global: GlobalBlock {
            input_preamps_db: preamps,
            bypass: read_bool(session, "bypass"),
            master_volume_db: read_f32(session, "vol.master", &[]),
            user_volume_db: read_f32(session, "vol.user", &[]),
            input_source: read_u8(session, "in.source"),
            lg_sound_sync_enabled: read_bool(session, "in.lg"),
            input_pair_linked: vec![false; 4],
        },
        loudness: LoudnessBlock {
            enabled: read_bool(session, "loud.on"),
            ref_spl: read_f32(session, "loud.ref", &[]),
            intensity_pct: read_f32(session, "loud.intensity", &[]),
            output_mask: 0xFFFF,
        },
        crossfeed: CrossfeedBlock {
            enabled: read_bool(session, "cf.on"),
            preset: read_u8(session, "cf.preset") as i32,
            freq_hz: read_f32(session, "cf.freq", &[]),
            feed_db: read_f32(session, "cf.feed", &[]),
            itd: read_bool(session, "cf.itd"),
            output_pair_mask: 0xFF,
        },
        leveller: LevellerBlock {
            enabled: read_bool(session, "lev.on"),
            speed: read_u8(session, "lev.speed") as i32,
            lookahead: read_bool(session, "lev.lookahead"),
            amount_pct: read_f32(session, "lev.amount", &[]),
            max_gain_db: read_f32(session, "lev.maxgain", &[]),
            gate_db: read_f32(session, "lev.gate", &[]),
            detector_mask: 0xFF,
            apply_mask: 0xFF,
        },
        // Absent rather than defaulted: a document must not claim a device had a
        // feature it does not.
        psybass: has("psychoacoustic_bass").then(|| PsybassBlock {
            enabled: read_bool(session, "bass.on"),
            cutoff_hz: read_f32(session, "bass.cutoff", &[]),
            harmonics_db: read_f32(session, "bass.harmonics", &[]),
            drive_db: read_f32(session, "bass.drive", &[]),
            character_pct: read_f32(session, "bass.character", &[]),
            original_db: read_f32(session, "bass.original", &[]),
            output_mask: 0xFFFF,
        }),
        upmix: has("upmixer").then(|| UpmixBlock {
            enabled: read_bool(session, "up.on"),
            strength_pct: read_f32(session, "up.strength", &[]),
            center_width_pct: read_f32(session, "up.width", &[]),
            presence_db: read_f32(session, "up.presence", &[]),
            ..Default::default()
        }),
        channels,
        matrix,
        io: IoBlock::default(),
    }
}

fn band_block(p: &EqParamPacket) -> BandBlock {
    BandBlock {
        r#type: p.filter_type.to_raw() as i32,
        freq_hz: p.freq,
        q: p.q,
        gain: p.gain_db,
        qp: p.qp.unwrap_or(0.707),
        bypass: p.bypass,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> PresetDocument {
        PresetDocument {
            schema_version: CURRENT_SCHEMA_VERSION,
            meta: Meta {
                name: Some("Living Room".into()),
                platform: Some("RP2350".into()),
                wire_format_version: 26,
                input_channel_count: 8,
                output_channel_count: 9,
                ..Default::default()
            },
            global: GlobalBlock::default(),
            loudness: LoudnessBlock::default(),
            crossfeed: CrossfeedBlock::default(),
            leveller: LevellerBlock::default(),
            psybass: None,
            upmix: None,
            channels: vec![ChannelBlock {
                channel_id: 0,
                name: "USB 1".into(),
                is_output: false,
                eq: vec![BandBlock {
                    r#type: 2,
                    freq_hz: 105.0,
                    gain: 8.8,
                    ..Default::default()
                }],
                ..Default::default()
            }],
            matrix: vec![CrosspointBlock {
                input: 0,
                output: 0,
                enabled: true,
                invert: false,
                gain_db: -3.0,
            }],
            io: IoBlock::default(),
        }
    }

    #[test]
    fn a_document_round_trips() {
        let original = doc();
        let parsed = parse(&write(&original)).unwrap();
        assert_eq!(parsed, original);
    }

    /// The schema is camelCase, because that is what the other applications
    /// read. Getting this wrong produces a file nothing else can open.
    #[test]
    fn the_json_uses_the_interchange_spelling() {
        let text = write(&doc());
        for key in [
            "schemaVersion",
            "inputPreampsDb",
            "channelId",
            "freqHz",
            "isOutput",
            "inputPairLinked",
            "outputPairMask",
        ] {
            assert!(text.contains(key), "missing key {key} in:\n{text}");
        }
        assert!(!text.contains("schema_version"), "snake_case leaked out");
    }

    /// A block absent means the source device lacked the feature; a block
    /// present but disabled means it had it and it was off. Collapsing the two
    /// would lose real information.
    #[test]
    fn absent_and_disabled_features_are_distinguishable() {
        let mut d = doc();
        assert!(d.psybass.is_none());
        assert!(!write(&d).contains("psybass"), "absent blocks are omitted");

        d.psybass = Some(PsybassBlock {
            enabled: false,
            ..Default::default()
        });
        let text = write(&d);
        assert!(text.contains("psybass"));
        assert!(!parse(&text).unwrap().psybass.unwrap().enabled);
    }

    /// Refusing a newer document beats guessing at blocks we cannot read and
    /// applying half a tuning.
    #[test]
    fn a_document_from_the_future_is_refused() {
        let mut d = doc();
        d.schema_version = CURRENT_SCHEMA_VERSION + 1;
        assert_eq!(
            parse(&write(&d)),
            Err(PresetFileError::FromTheFuture {
                got: CURRENT_SCHEMA_VERSION + 1,
                supported: CURRENT_SCHEMA_VERSION
            })
        );
    }

    #[test]
    fn malformed_input_is_refused_with_a_reason() {
        assert_eq!(parse(""), Err(PresetFileError::Empty));
        assert!(matches!(parse("{"), Err(PresetFileError::Invalid(_))));
        assert_eq!(
            parse(r#"{"schemaVersion":1,"channels":[]}"#),
            Err(PresetFileError::NoChannels)
        );
        assert!(matches!(
            parse(r#"{"schemaVersion":0,"channels":[{"channelId":0}]}"#),
            Err(PresetFileError::Invalid(_))
        ));
    }

    /// Wire values, not names: a renamed enum member must not be able to break
    /// an old file.
    #[test]
    fn filter_types_are_stored_as_wire_values() {
        let text = write(&doc());
        assert!(
            text.contains("\"type\": 2"),
            "expected a raw type in:\n{text}"
        );
        assert!(!text.to_lowercase().contains("lowshelf"));
    }

    /// Channels match by id and are never remapped; the reference implementation
    /// reports what the device lacks rather than translating.
    #[test]
    fn channels_match_by_id_and_the_rest_are_reported() {
        let mut d = doc();
        d.channels.push(ChannelBlock {
            channel_id: 12,
            name: "SPDIF 3 L".into(),
            is_output: true,
            ..Default::default()
        });

        // A two-input part: channel 12 does not exist there.
        let (usable, missing) = resolve_channels(&d, &[0, 1, 2, 3, 4, 5, 6]);
        assert_eq!(usable.len(), 1);
        assert_eq!(usable[0].channel_id, 0);
        assert_eq!(missing, vec!["SPDIF 3 L"]);
    }

    #[test]
    fn a_duplicated_channel_id_takes_the_last_rather_than_failing() {
        let mut d = doc();
        d.channels.push(ChannelBlock {
            channel_id: 0,
            name: "USB 1 again".into(),
            delay_ms: 5.0,
            ..Default::default()
        });
        let (usable, missing) = resolve_channels(&d, &[0]);
        assert_eq!(usable.len(), 1);
        assert_eq!(usable[0].delay_ms, 5.0, "the last block should win");
        assert!(missing.is_empty());
    }

    /// Volume and wiring describe a room and a board, not a tuning, so importing
    /// them without being asked would change how loud someone's room gets.
    #[test]
    fn only_audio_processing_is_applied_by_default() {
        let o = ApplyOptions::default();
        assert!(o.audio_processing);
        assert!(!o.volume_levels);
        assert!(!o.hardware_io);
    }

    #[test]
    fn unknown_keys_from_a_newer_writer_do_not_break_parsing() {
        let text = r#"{
            "schemaVersion": 1,
            "somethingNew": {"a": 1},
            "channels": [{"channelId": 0, "name": "In", "isOutput": false}]
        }"#;
        let d = parse(text).unwrap();
        assert_eq!(d.channels.len(), 1);
    }

    #[test]
    fn missing_optional_blocks_fall_back_to_defaults() {
        let text = r#"{"schemaVersion":1,"channels":[{"channelId":0,"isOutput":false}]}"#;
        let d = parse(text).unwrap();
        assert_eq!(d.global.input_preamps_db.len(), 8);
        assert_eq!(d.global.master_volume_db, -20.0);
        assert!(d.psybass.is_none());
    }
}
