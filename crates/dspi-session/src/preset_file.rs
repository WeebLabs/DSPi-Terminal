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
///
/// Every scalar is optional, which is not the reference implementation's shape
/// and is deliberate: there, an absent key falls back to a platform default and
/// is written anyway. Here an absent key means "the document does not say", and
/// nothing is written. The difference matters because a hand-edited or
/// truncated file would otherwise read as "move the bit clock to GPIO 0", and a
/// pin move is exactly the change that is expensive to undo. A file written by
/// either Console carries every key, so interchange is unaffected.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct IoBlock {
    /// GPIOs for the pin outputs: one per S/PDIF slot, then the PDM sub
    /// (config.h:667-673, `NUM_PIN_OUTPUTS`).
    #[serde(default)]
    pub output_pins: Vec<u8>,
    /// 0 = S/PDIF, 1 = I2S per output slot (config.h:615-617).
    #[serde(default)]
    pub output_slot_types: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub i2s_bck_pin: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mck_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mck_pin: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mck_multiplier: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub i2s_clock_mode: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub i2s_clock_pin_mode: Option<u8>,
    /// The slave clock pair, stored while dormant so entering SPLIT finds it
    /// already valid (config.h:485-487).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub i2s_bck_pin_slave: Option<u8>,
    /// S/PDIF RX GPIOs for inputs 0..2. The shared schema is three long; the
    /// fourth input is newer than the schema and travels in `spdifRxPin4`.
    #[serde(default)]
    pub spdif_rx_pins: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spdif_rx_pin4: Option<u8>,
    /// Enable mask for the optional inputs: bit 0 is input 1, bit 1 input 2,
    /// bit 2 input 3 (config.h:454, indices 1..3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spdif_enabled_ext: Option<u8>,
    #[serde(default)]
    pub i2s_rx_pins: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub i2s_input_channels: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub i2s_input_rate_hz: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adat_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adat_pin: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adat_input_enabled: Option<bool>,
    /// 0xFF means unset, which is the ADAT input's own default
    /// (config.h:595-604).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adat_input_pin: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adat_input_clock_mode: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dac_hw_mute: Option<DacHwMuteBlock>,
}

/// The external DAC's mute pin (config.h:439-442, a 16-byte `DacHwMuteConfig`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DacHwMuteBlock {
    pub enabled: bool,
    pub active_low: bool,
    pub pin: u8,
    pub hold_ms: u16,
    pub release_ms: u16,
}

impl DacHwMuteBlock {
    /// The 16-byte wire layout: `{enabled, active_low, pin, rsvd, hold_ms u16,
    /// release_ms u16}`, the rest reserved and zero.
    pub fn encode(&self) -> Vec<u8> {
        let mut d = vec![0u8; 16];
        d[0] = self.enabled as u8;
        d[1] = self.active_low as u8;
        d[2] = self.pin;
        d[4..6].copy_from_slice(&self.hold_ms.to_le_bytes());
        d[6..8].copy_from_slice(&self.release_ms.to_le_bytes());
        d
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 16 {
            return None;
        }
        Some(Self {
            enabled: bytes[0] != 0,
            active_low: bytes[1] != 0,
            pin: bytes[2],
            hold_ms: u16::from_le_bytes([bytes[4], bytes[5]]),
            release_ms: u16::from_le_bytes([bytes[6], bytes[7]]),
        })
    }
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

impl ApplyReport {
    /// Record a reason once.
    ///
    /// Per-band and per-pin problems would otherwise repeat for every item in
    /// the file and bury the one line that matters.
    pub fn skip(&mut self, reason: impl Into<String>) {
        let reason = reason.into();
        if !self.skipped.contains(&reason) {
            self.skipped.push(reason);
        }
    }

    /// Nothing was missing and nothing was refused.
    pub fn is_clean(&self) -> bool {
        self.missing_channels.is_empty() && self.skipped.is_empty()
    }

    /// The result report, worded as the Console words it.
    ///
    /// The closing line is the one that stops someone power-cycling and losing
    /// the whole import, so it is not dropped when everything went well. A dry
    /// run omits it, because nothing is live either.
    pub fn lines(&self, dry_run: bool) -> Vec<String> {
        let mut lines = vec![format!(
            "{} {} channels, {} EQ bands, {} crossover bands, {} crosspoints.",
            if dry_run { "Would apply" } else { "Applied" },
            self.channels_applied,
            self.bands_applied,
            self.crossover_bands_applied,
            self.crosspoints_applied
        )];

        if !self.missing_channels.is_empty() {
            lines.push(format!(
                "Not present on this device: {}",
                self.missing_channels.join(", ")
            ));
        }
        for skipped in &self.skipped {
            lines.push(format!("Skipped: {skipped}"));
        }

        if !dry_run {
            lines.push(
                "These changes are live but not yet stored on the device. \
                 Save them to a preset slot to keep them."
                    .into(),
            );
        }
        lines
    }
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
use dspi_proto::generated::status;
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
        report.skip("volume levels (not requested)");
    }

    if options.hardware_io {
        apply_io(session, &doc.io, &mut report);
    } else {
        report.skip("hardware I/O (not requested)");
    }

    // The input source picks which wiring the device listens to, so it goes
    // after the wiring has moved rather than before it.
    if options.audio_processing {
        let v = Value::Choice(doc.global.input_source);
        if let Err(crate::WriteError::Unavailable { .. }) = session.write("in.source", &[], v) {
            report.skip("Input source (not supported by this firmware)");
        }
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
                report.skip(format!("{label} (not on this device)"));
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

    // The input source is applied at the very end of `apply`, after any
    // hardware wiring has moved.
    set(
        "bypass",
        Value::Bool(doc.global.bypass),
        "EQ bypass",
        report,
    );
}

// ---------------------------------------------------------------------------
// Hardware I/O
// ---------------------------------------------------------------------------

/// Push the wiring block, in the reference implementation's order.
///
/// The order is the whole of the difficulty, because most of these settings
/// validate against each other. It follows `PresetDocumentTransfer.swift`
/// (`applyIO`, lines 571-715), with one deliberate departure noted at the MCK
/// step. Every write goes through [`Session::write`], so each one is validated,
/// gated and read back like a typed command; a `PIN_CONFIG_*` refusal is
/// recorded and the apply carries on with the next item, because a board that
/// cannot take one pin can usually still take the other twenty.
fn apply_io(session: &mut Session, io: &IoBlock, report: &mut ApplyReport) {
    let caps = session.capabilities().clone();
    let has = |name: &str| caps.features.iter().any(|f| f.name == name && f.present);

    // One pin output per S/PDIF slot plus the PDM sub (config.h:667-673), and a
    // slot is a stereo pair of the output channels.
    let slots = (caps.num_outputs.saturating_sub(1) / 2) as usize;
    let pdm_pin_index = slots as u8;
    let pdm_output = caps.num_outputs.saturating_sub(1);

    // 1. Slot types before pins (swift:584-588). A type change is deferred and
    //    reallocates the slot, so a pin written first would be reassigned.
    for (slot, want) in io.output_slot_types.iter().take(slots).enumerate() {
        if current_u8(session, "out.type", &[slot as u8]) == Some(*want) {
            continue;
        }
        attempt(
            session,
            report,
            &format!("Output {} type", slot + 1),
            "out.type",
            &[slot as u8],
            Value::Choice(*want),
        );
    }

    // 2. Output pins, the slots and then PDM (swift:591-614).
    for index in 0..=pdm_pin_index {
        let Some(want) = io.output_pins.get(index as usize).copied() else {
            continue;
        };
        if current_u8(session, "out.pin", &[index]) == Some(want) {
            continue;
        }
        let what = format!("Output {} GPIO", index + 1);
        attempt(
            session,
            report,
            &what,
            "out.pin",
            &[index],
            Value::Int(want as i64),
        );

        // The firmware refuses to move the PDM pin while PDM is running
        // (survey-firmware 6.7, OUTPUT_ACTIVE). Cycle the output the way the
        // Console does and put its enable state back afterwards.
        if index == pdm_pin_index
            && session.last_write_status() == Some(status::PIN_CONFIG_OUTPUT_ACTIVE as u8)
        {
            let was_enabled = session
                .read("out.enable", &[pdm_output])
                .ok()
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let _ = session.enable_output(pdm_output, false);
            report.skipped.retain(|s| !s.starts_with(&what));
            attempt(
                session,
                report,
                &what,
                "out.pin",
                &[index],
                Value::Int(want as i64),
            );
            let _ = session.enable_output(pdm_output, was_enabled);
        }
    }

    // 3. The I2S bit clock (swift:617); the word clock sits on the next pin up.
    if let Some(want) = io.i2s_bck_pin
        && current_u8(session, "i2s.bck", &[]) != Some(want)
    {
        attempt(
            session,
            report,
            "I2S BCK pin",
            "i2s.bck",
            &[],
            Value::Int(want as i64),
        );
    }

    // 4. The master clock (swift:618-622). Departure from the reference: the
    //    firmware answers OUTPUT_ACTIVE for a pin change while MCK is running
    //    (survey-firmware 6.7), so the enable is dropped first and the
    //    document's enable state applied last, rather than enable-then-pin.
    let mck_pin_moves = io
        .mck_pin
        .is_some_and(|want| current_u8(session, "i2s.mck.pin", &[]) != Some(want));
    let mck_live = current_bool(session, "i2s.mck");
    if mck_pin_moves && mck_live == Some(true) {
        attempt(
            session,
            report,
            "MCK enable",
            "i2s.mck",
            &[],
            Value::Bool(false),
        );
    }
    if let Some(want) = io.mck_pin
        && mck_pin_moves
    {
        attempt(
            session,
            report,
            "MCK pin",
            "i2s.mck.pin",
            &[],
            Value::Int(want as i64),
        );
    }
    // The document carries the multiple itself (128 or 256, matching the
    // reference implementation's schema); the wire carries a selector
    // (config.h:342-343, 0 = 128x, 1 = 256x).
    if let Some(want) = io.mck_multiplier.map(mck_selector)
        && current_u8(session, "i2s.mck.mult", &[]) != Some(want)
    {
        attempt(
            session,
            report,
            "MCK multiplier",
            "i2s.mck.mult",
            &[],
            Value::Choice(want),
        );
    }
    if let Some(want) = io.mck_enabled
        && current_bool(session, "i2s.mck") != Some(want)
    {
        attempt(
            session,
            report,
            "MCK enable",
            "i2s.mck",
            &[],
            Value::Bool(want),
        );
    }

    // 5. S/PDIF inputs (swift:625-649). Drop an enable before repinning so a
    //    move cannot clash with the pin it is leaving, and apply each pin
    //    before switching its input on so the enable validates against the pin
    //    it will use.
    if let Some(want) = io.spdif_rx_pins.first().copied()
        && current_u8(session, "in.spdif.pin", &[0]) != Some(want)
    {
        attempt(
            session,
            report,
            "S/PDIF RX pin",
            "in.spdif.pin",
            &[0],
            Value::Int(want as i64),
        );
    }
    if has("spdif_multi_input") {
        let (count, mask) = spdif_input_config(session);
        for index in 1..count.min(4) {
            let want_pin = document_spdif_pin(io, index);
            let want_enabled = io.spdif_enabled_ext.map(|m| m & (1 << (index - 1)) != 0);
            let is_enabled = mask & (1 << index) != 0;
            let what = format!("S/PDIF input {}", index + 1);

            if is_enabled && want_enabled == Some(false) {
                attempt(
                    session,
                    report,
                    &what,
                    "in.spdif.enable",
                    &[index],
                    Value::Bool(false),
                );
            }
            if let Some(pin) = want_pin
                && current_u8(session, "in.spdif.pin", &[index]) != Some(pin)
            {
                attempt(
                    session,
                    report,
                    &format!("S/PDIF {} RX pin", index + 1),
                    "in.spdif.pin",
                    &[index],
                    Value::Int(pin as i64),
                );
            }
            if !is_enabled && want_enabled == Some(true) {
                attempt(
                    session,
                    report,
                    &what,
                    "in.spdif.enable",
                    &[index],
                    Value::Bool(true),
                );
            }
        }
    } else if io.spdif_enabled_ext.is_some_and(|m| m != 0) {
        report.skip("Additional S/PDIF inputs (not supported by this firmware)");
    }

    // 6. I2S input (swift:653-663): the channel count first, because lowering
    //    it frees pairs and never fails, then each pair's data pin, then the
    //    rate.
    if let Some(want) = io.i2s_input_channels
        && has("i2s_input_channels")
        && matches!(want, 2 | 4 | 6 | 8)
        && current_u8(session, "in.i2s.channels", &[]) != Some(want as u8)
    {
        attempt(
            session,
            report,
            "I2S input channels",
            "in.i2s.channels",
            &[],
            Value::Int(want as i64),
        );
    }
    for (pair, want) in io.i2s_rx_pins.iter().take(4).enumerate() {
        if current_u8(session, "in.i2s.pin", &[pair as u8]) == Some(*want) {
            continue;
        }
        attempt(
            session,
            report,
            &format!("I2S serial data {} pin", pair + 1),
            "in.i2s.pin",
            &[pair as u8],
            Value::Int(*want as i64),
        );
    }
    if let Some(want) = io.i2s_input_rate_hz {
        let _ = session.write("in.rate", &[], Value::Int(want as i64));
    }

    // 7. Clocking (swift:665-679). The slave pair is a dormant store accepted
    //    at any time, so it goes in before the mode that starts using it.
    if let Some(want) = io.i2s_clock_mode {
        if has("i2s_slave_clock") {
            if current_u8(session, "in.i2s.clock", &[]) != Some(want) {
                attempt(
                    session,
                    report,
                    "I2S clock mode",
                    "in.i2s.clock",
                    &[],
                    Value::Choice(want),
                );
            }
        } else if want != 0 {
            report.skip("I2S clock-slave mode (not supported by this firmware)");
        }
    }
    if let Some(want) = io.i2s_bck_pin_slave {
        // Role 1 selects the slave pair; the registry row addresses the
        // unified pair only, so this one goes out as a raw write-as-read
        // (config.h:334-336, wValue = (role << 8) | GPIO).
        let status = session.with_transport(|t| {
            t.control_in(
                dspi_proto::generated::opcodes::REQ_SET_I2S_BCK_PIN,
                (1 << 8) | want as u16,
                1,
            )
        });
        match status {
            Ok(d) if d.first().copied().unwrap_or(0) != 0 => {
                let code = d[0];
                report.skip(format!(
                    "I2S slave BCK pin rejected by the device: {}",
                    describe_pin_status(code)
                ));
            }
            Ok(_) => {}
            Err(e) => report.skip(format!("I2S slave BCK pin rejected by the device: {e}")),
        }
    }
    if let Some(want) = io.i2s_clock_pin_mode
        && current_u8(session, "i2s.clockpins", &[]) != Some(want)
    {
        attempt(
            session,
            report,
            "I2S clock pin mode",
            "i2s.clockpins",
            &[],
            Value::Choice(want),
        );
    }

    // 8. ADAT output (swift:683-691): the data pin first, since it re-routes
    //    under a muted restart while enabled, then the enable state.
    if has("adat_output") {
        if let Some(want) = io.adat_pin
            && current_u8(session, "adat.pin", &[]) != Some(want)
        {
            attempt(
                session,
                report,
                "ADAT output pin",
                "adat.pin",
                &[],
                Value::Int(want as i64),
            );
        }
        if let Some(want) = io.adat_enabled
            && current_bool(session, "adat.enable") != Some(want)
        {
            attempt(
                session,
                report,
                "ADAT output enable",
                "adat.enable",
                &[],
                Value::Bool(want),
            );
        }
    } else if io.adat_enabled == Some(true) {
        report.skip("ADAT output (not supported by this device)");
    }

    // 9. ADAT input (swift:693-705).
    if has("adat_input") {
        if let Some(want) = io.adat_input_pin
            && want != PIN_UNSET
            && current_u8(session, "in.adat.pin", &[]) != Some(want)
        {
            attempt(
                session,
                report,
                "ADAT input pin",
                "in.adat.pin",
                &[],
                Value::Int(want as i64),
            );
        }
        if let Some(want) = io.adat_input_clock_mode
            && current_u8(session, "in.adat.clock", &[]) != Some(want)
        {
            attempt(
                session,
                report,
                "ADAT input clock mode",
                "in.adat.clock",
                &[],
                Value::Choice(want),
            );
        }
        if let Some(want) = io.adat_input_enabled
            && current_bool(session, "in.adat.enable") != Some(want)
        {
            attempt(
                session,
                report,
                "ADAT input enable",
                "in.adat.enable",
                &[],
                Value::Bool(want),
            );
        }
    } else if io.adat_input_enabled == Some(true) {
        report.skip("ADAT input (not supported by this device)");
    }

    // 10. The external DAC's mute pin (swift:708-714).
    if let Some(mute) = &io.dac_hw_mute {
        if has("dac_hardware_mute") {
            let what = "External DAC hardware mute";
            match session.write("dev.dacmute", &[], Value::Bytes(mute.encode())) {
                Err(crate::WriteError::Unavailable { .. }) => {
                    report.skip(format!("{what} (not supported by this firmware)"));
                }
                Err(e) => report.skip(format!("{what} rejected by the device: {e}")),
                // The registry reads one byte for a packet parameter, so the
                // write path's own readback cannot confirm a 16-byte config
                // (config.h:441). Confirm it here instead.
                Ok(_) => {
                    if read_dac_mute(session).is_some_and(|live| live != *mute) {
                        report.skip(format!("{what} was not applied by the device"));
                    }
                }
            }
        } else {
            report.skip("External DAC hardware mute (not supported by this firmware)");
        }
    }
}

/// Read the wiring back off the device.
///
/// Only what the registry can actually read: a field with no reader is left
/// absent rather than defaulted, because [`apply_io`] treats absent as "the
/// document does not say" and a made-up default here would come back as a real
/// pin move on the next import. The slave BCK pin is the one such gap; it has
/// no scalar GET of its own (config.h:334-336 puts the role in the SET's
/// wValue only) and lives in the bulk snapshot, which Phase 2A decodes.
fn capture_io(session: &mut Session) -> IoBlock {
    let caps = session.capabilities().clone();
    let has = |name: &str| caps.features.iter().any(|f| f.name == name && f.present);
    let slots = (caps.num_outputs.saturating_sub(1) / 2) as usize;

    let mut io = IoBlock {
        output_slot_types: (0..slots)
            .filter_map(|s| current_u8(session, "out.type", &[s as u8]))
            .collect(),
        i2s_bck_pin: current_u8(session, "i2s.bck", &[]),
        mck_enabled: current_bool(session, "i2s.mck"),
        mck_pin: current_u8(session, "i2s.mck.pin", &[]),
        mck_multiplier: current_u8(session, "i2s.mck.mult", &[]).map(mck_multiple),
        i2s_clock_mode: current_u8(session, "in.i2s.clock", &[]),
        i2s_clock_pin_mode: current_u8(session, "i2s.clockpins", &[]),
        i2s_input_channels: current_u8(session, "in.i2s.channels", &[]).map(i32::from),
        i2s_input_rate_hz: session
            .read("in.rate", &[])
            .ok()
            .and_then(|v| v.as_f32())
            .map(|hz| hz as u32),
        ..Default::default()
    };

    // One pin output per slot, then the PDM sub (config.h:667-673).
    io.output_pins = (0..=slots as u8)
        .filter_map(|i| current_u8(session, "out.pin", &[i]))
        .collect();

    let (spdif_count, spdif_mask) = spdif_input_config(session);
    io.spdif_rx_pins = (0..3)
        .filter_map(|i| current_u8(session, "in.spdif.pin", &[i]))
        .collect();
    if spdif_count > 3 {
        io.spdif_rx_pin4 = current_u8(session, "in.spdif.pin", &[3]);
    }
    if has("spdif_multi_input") {
        // Bit 0 of the wire mask is input 0, which is always on; the document's
        // mask starts at input 1 (config.h:454-457).
        io.spdif_enabled_ext = Some(spdif_mask >> 1);
    }

    io.i2s_rx_pins = (0..4)
        .filter_map(|p| current_u8(session, "in.i2s.pin", &[p]))
        .collect();

    if has("adat_output") {
        io.adat_enabled = current_bool(session, "adat.enable");
        io.adat_pin = current_u8(session, "adat.pin", &[]);
    }
    if has("adat_input") {
        io.adat_input_enabled = current_bool(session, "in.adat.enable");
        io.adat_input_pin = current_u8(session, "in.adat.pin", &[]);
        io.adat_input_clock_mode = current_u8(session, "in.adat.clock", &[]);
    }
    if has("dac_hardware_mute") {
        io.dac_hw_mute = read_dac_mute(session);
    }

    io
}

/// The live 16-byte DAC mute config (config.h:441).
fn read_dac_mute(session: &mut Session) -> Option<DacHwMuteBlock> {
    let bytes = session
        .with_transport(|t| {
            t.control_in(
                dspi_proto::generated::opcodes::REQ_GET_DAC_HW_MUTE_CONFIG,
                0,
                16,
            )
        })
        .ok()?;
    DacHwMuteBlock::decode(&bytes)
}

/// `PIN_RESET_TO_DEFAULT`, which for the ADAT input means "no pin"
/// (config.h:595-604).
const PIN_UNSET: u8 = 0xFF;

/// The document holds the master-clock multiple; the wire holds a selector.
fn mck_selector(multiple: i32) -> u8 {
    u8::from(multiple == 256)
}

/// The other direction, for capture.
fn mck_multiple(selector: u8) -> i32 {
    if selector == 1 { 256 } else { 128 }
}

/// Write one hardware setting and record a refusal instead of throwing it.
///
/// The status byte is the firmware's own verdict for these write-as-read
/// setters, so it is believed first; the readback is what catches the
/// parameters that answer no status at all.
fn attempt(
    session: &mut Session,
    report: &mut ApplyReport,
    what: &str,
    path: &str,
    indices: &[u8],
    value: Value,
) {
    let outcome = session.write(path, indices, value);
    let status = session.last_write_status();

    match (outcome, status) {
        (Ok(_), Some(0))
        | (Ok(Outcome::Confirmed(_) | Outcome::Accepted | Outcome::Triggered), None) => {}
        (Ok(_), Some(code)) => report.skip(format!(
            "{what} rejected by the device: {}",
            describe_pin_status(code)
        )),
        (Ok(Outcome::Rejected { .. }), None) => {
            report.skip(format!("{what} was not applied by the device"));
        }
        (Err(crate::WriteError::Unavailable { .. }), _) => {
            report.skip(format!("{what} (not supported by this device)"));
        }
        (Err(e), _) => report.skip(format!("{what} rejected by the device: {e}")),
    }
}

/// The Console's explanation for a `PIN_CONFIG_*` code (config.h:607-613).
///
/// The first two sentences are the Console's own
/// (`DSPi_ConsoleApp.swift:1988-1989`, minus the interface name it substitutes
/// there); the rest are the shorter phrases its preset-apply path uses
/// (`PresetDocumentTransfer.swift:724-732`).
pub fn describe_pin_status(code: u8) -> &'static str {
    match code as u16 {
        status::PIN_CONFIG_INVALID_PIN => {
            "A pin is out of range or lacks the required mux function"
        }
        status::PIN_CONFIG_PIN_IN_USE => "A pin is already claimed by another output or interface",
        status::PIN_CONFIG_INVALID_OUTPUT => "invalid output",
        status::PIN_CONFIG_OUTPUT_ACTIVE => "output is active",
        status::PIN_CONFIG_INVALID_PARAM => "invalid parameter",
        _ => "unknown status",
    }
}

fn current_u8(session: &mut Session, path: &str, indices: &[u8]) -> Option<u8> {
    session.read(path, indices).ok().and_then(|v| v.as_u8())
}

fn current_bool(session: &mut Session, path: &str) -> Option<bool> {
    session.read(path, &[]).ok().and_then(|v| v.as_bool())
}

/// The S/PDIF pin a document holds for an input index, or `None` when the
/// document predates that input. Index 3 lives in the additive field.
fn document_spdif_pin(io: &IoBlock, index: u8) -> Option<u8> {
    if index == 3 {
        return io.spdif_rx_pin4;
    }
    io.spdif_rx_pins.get(index as usize).copied()
}

/// `{count, enable_mask, gpio[0..3]}` (config.h:456-457). Bit 0 of the mask is
/// input 0, which is always on.
fn spdif_input_config(session: &mut Session) -> (u8, u8) {
    session
        .with_transport(|t| {
            t.control_in(
                dspi_proto::generated::opcodes::REQ_GET_SPDIF_INPUT_CONFIG,
                0,
                6,
            )
        })
        .ok()
        .map(|d| (d[0], d[1]))
        .unwrap_or((1, 1))
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

    // Before the document is built, because it needs the transport too.
    let io = capture_io(session);

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
        io,
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

#[cfg(test)]
mod io_tests {
    use super::*;
    use crate::probe::{Capabilities, ChannelInfo, Feature};
    use dspi_proto::Platform;
    use dspi_proto::generated::opcodes as op;
    use dspi_transport::MockTransport;
    use dspi_transport::mock::{Direction, LogHandle, Reply};

    fn caps() -> Capabilities {
        Capabilities {
            serial: "TEST".into(),
            platform: Platform::Rp2350,
            firmware: "1.1.6".into(),
            wire_format: dspi_proto::generated::wire::WIRE_FORMAT_VERSION as u8,
            num_channels: 17,
            num_inputs: 8,
            num_outputs: 9,
            max_bands: 10,
            band_storage: 12,
            channels: (0..17)
                .map(|i| ChannelInfo {
                    index: i,
                    name: format!("Ch {i}"),
                    slug: format!("ch.{i}"),
                    is_output: i >= 8,
                })
                .collect(),
            features: [
                "spdif_multi_input",
                "i2s_input_channels",
                "i2s_slave_clock",
                "adat_output",
                "adat_input",
                "dac_hardware_mute",
            ]
            .iter()
            .map(|n| Feature {
                name: (*n).into(),
                present: true,
                evidence: "test".into(),
            })
            .collect(),
            cs: None,
            siggen: None,
            active_preset: None,
        }
    }

    /// A document that moves every piece of wiring, so nothing is skipped for
    /// being already correct and the whole order is exercised.
    fn io() -> IoBlock {
        IoBlock {
            output_pins: vec![6, 7, 8, 9, 10],
            output_slot_types: vec![1, 1, 1, 1],
            i2s_bck_pin: Some(14),
            mck_enabled: Some(true),
            mck_pin: Some(13),
            mck_multiplier: Some(256),
            i2s_clock_mode: Some(1),
            i2s_clock_pin_mode: Some(1),
            i2s_bck_pin_slave: Some(26),
            spdif_rx_pins: vec![5, 20, 21],
            spdif_rx_pin4: Some(22),
            spdif_enabled_ext: Some(0b111),
            i2s_rx_pins: vec![1, 2, 3, 4],
            i2s_input_channels: Some(8),
            i2s_input_rate_hz: Some(48000),
            adat_enabled: Some(true),
            adat_pin: Some(12),
            adat_input_enabled: Some(true),
            adat_input_pin: Some(11),
            adat_input_clock_mode: Some(1),
            dac_hw_mute: Some(DacHwMuteBlock {
                enabled: true,
                active_low: true,
                pin: 11,
                hold_ms: 5,
                release_ms: 0,
            }),
        }
    }

    fn doc_with(io: IoBlock) -> PresetDocument {
        PresetDocument {
            schema_version: CURRENT_SCHEMA_VERSION,
            meta: Meta::default(),
            global: GlobalBlock::default(),
            loudness: LoudnessBlock::default(),
            crossfeed: CrossfeedBlock::default(),
            leveller: LevellerBlock::default(),
            psybass: None,
            upmix: None,
            channels: vec![ChannelBlock::default()],
            matrix: Vec::new(),
            io,
        }
    }

    /// Every GET answers a value the document disagrees with, and every SET
    /// answers `PIN_CONFIG_SUCCESS`.
    fn device() -> MockTransport {
        MockTransport::new()
            .data(op::REQ_GET_OUTPUT_TYPE, vec![0])
            .data(op::REQ_SET_OUTPUT_TYPE, vec![0])
            .data(op::REQ_GET_OUTPUT_PIN, vec![0])
            .data(op::REQ_SET_OUTPUT_PIN, vec![0])
            .data(op::REQ_GET_I2S_BCK_PIN, vec![0])
            .data(op::REQ_SET_I2S_BCK_PIN, vec![0])
            .data(op::REQ_GET_MCK_ENABLE, vec![1])
            .data(op::REQ_SET_MCK_ENABLE, vec![0])
            .data(op::REQ_GET_MCK_PIN, vec![0])
            .data(op::REQ_SET_MCK_PIN, vec![0])
            .data(op::REQ_GET_MCK_MULTIPLIER, vec![0])
            .data(op::REQ_SET_MCK_MULTIPLIER, vec![0])
            // {count, enable_mask, gpio[0..3]} (config.h:456-457).
            .data(op::REQ_GET_SPDIF_INPUT_CONFIG, vec![4, 0b0001, 0, 0, 0, 0])
            .data(op::REQ_GET_SPDIF_RX_PIN, vec![0])
            .data(op::REQ_SET_SPDIF_RX_PIN, vec![0])
            .data(op::REQ_SET_SPDIF_INPUT_ENABLE, vec![0])
            .data(op::REQ_GET_I2S_RX_PIN, vec![0])
            .data(op::REQ_SET_I2S_RX_PIN, vec![0])
            .data(op::REQ_GET_I2S_INPUT_CHANNELS, vec![2])
            .data(op::REQ_SET_I2S_INPUT_CHANNELS, vec![0])
            .data(op::REQ_GET_INPUT_RATE, 44100u32.to_le_bytes().to_vec())
            .data(op::REQ_SET_INPUT_RATE, vec![0])
            // Deferred OUT writes: the confirming readback has to see the new
            // value, or a correct write looks like a silent rejection.
            .reply(
                op::REQ_GET_I2S_CLOCK_MODE,
                Reply::Sequence(vec![Reply::Data(vec![0]), Reply::Data(vec![1])]),
            )
            .data(op::REQ_SET_I2S_CLOCK_MODE, vec![0])
            .data(op::REQ_GET_I2S_CLOCK_PIN_MODE, vec![0])
            .data(op::REQ_SET_I2S_CLOCK_PIN_MODE, vec![0])
            .data(op::REQ_GET_ADAT_PIN, vec![0])
            .data(op::REQ_SET_ADAT_PIN, vec![0])
            .data(op::REQ_GET_ADAT_ENABLE, vec![0])
            .data(op::REQ_SET_ADAT_ENABLE, vec![0])
            .data(op::REQ_GET_ADAT_INPUT_PIN, vec![0])
            .data(op::REQ_SET_ADAT_INPUT_PIN, vec![0])
            .reply(
                op::REQ_GET_ADAT_INPUT_CLOCK_MODE,
                Reply::Sequence(vec![Reply::Data(vec![0]), Reply::Data(vec![1])]),
            )
            .data(op::REQ_SET_ADAT_INPUT_CLOCK_MODE, vec![0])
            .reply(
                op::REQ_GET_ADAT_INPUT_ENABLE,
                Reply::Sequence(vec![Reply::Data(vec![0]), Reply::Data(vec![1])]),
            )
            .data(op::REQ_SET_ADAT_INPUT_ENABLE, vec![0])
            .data(op::REQ_GET_DAC_HW_MUTE_CONFIG, {
                let mut d = vec![0u8; 16];
                d[0] = 1;
                d[1] = 1;
                d[2] = 11;
                d[4] = 5;
                d
            })
            .data(op::REQ_SET_DAC_HW_MUTE_CONFIG, vec![0])
            .data(op::REQ_GET_INPUT_SOURCE, vec![0])
            .data(op::REQ_SET_INPUT_SOURCE, vec![0])
            .data(op::REQ_GET_OUTPUT_ENABLE, vec![1])
            .data(op::REQ_SET_OUTPUT_ENABLE, vec![0])
            .data(op::REQ_GET_CORE1_CONFLICT, vec![0])
    }

    fn rig(t: MockTransport) -> (Session, LogHandle) {
        let log = t.log_handle();
        (Session::new(Box::new(t), caps()).unwrap(), log)
    }

    const SETTERS: &[u8] = &[
        op::REQ_SET_OUTPUT_TYPE,
        op::REQ_SET_OUTPUT_PIN,
        op::REQ_SET_I2S_BCK_PIN,
        op::REQ_SET_MCK_ENABLE,
        op::REQ_SET_MCK_PIN,
        op::REQ_SET_MCK_MULTIPLIER,
        op::REQ_SET_SPDIF_RX_PIN,
        op::REQ_SET_SPDIF_INPUT_ENABLE,
        op::REQ_SET_I2S_INPUT_CHANNELS,
        op::REQ_SET_I2S_RX_PIN,
        op::REQ_SET_INPUT_RATE,
        op::REQ_SET_I2S_CLOCK_MODE,
        op::REQ_SET_I2S_CLOCK_PIN_MODE,
        op::REQ_SET_ADAT_PIN,
        op::REQ_SET_ADAT_ENABLE,
        op::REQ_SET_ADAT_INPUT_PIN,
        op::REQ_SET_ADAT_INPUT_CLOCK_MODE,
        op::REQ_SET_ADAT_INPUT_ENABLE,
        op::REQ_SET_DAC_HW_MUTE_CONFIG,
    ];

    /// The opcodes that changed something, in the order they went out.
    fn sets(log: &LogHandle) -> Vec<u8> {
        log.lock()
            .unwrap()
            .iter()
            .filter(|e| SETTERS.contains(&e.opcode))
            .map(|e| e.opcode)
            .collect()
    }

    /// The order is the whole of the difficulty: these settings validate
    /// against each other, so a rearrangement fails silently on hardware and
    /// nowhere else.
    #[test]
    fn the_apply_order_follows_the_reference_implementation() {
        let (mut s, log) = rig(device());
        let mut report = ApplyReport::default();
        apply_io(&mut s, &io(), &mut report);

        let seen = sets(&log);
        let first = |opcode: u8| {
            seen.iter()
                .position(|o| *o == opcode)
                .unwrap_or_else(|| panic!("0x{opcode:02X} never went out: {seen:02X?}"))
        };

        // Slot types before pins: a type change is deferred and reallocates.
        assert!(first(op::REQ_SET_OUTPUT_TYPE) < first(op::REQ_SET_OUTPUT_PIN));
        // The pins before the clocks that validate against them.
        assert!(first(op::REQ_SET_OUTPUT_PIN) < first(op::REQ_SET_I2S_BCK_PIN));
        // MCK off before its pin moves, which is where this departs from the
        // reference implementation: the firmware answers OUTPUT_ACTIVE for a
        // pin change while MCK runs (survey-firmware 6.7).
        assert!(first(op::REQ_SET_MCK_ENABLE) < first(op::REQ_SET_MCK_PIN));
        assert!(first(op::REQ_SET_I2S_BCK_PIN) < first(op::REQ_SET_SPDIF_RX_PIN));
        // Each S/PDIF pin lands before the enable that validates against it.
        assert!(first(op::REQ_SET_SPDIF_RX_PIN) < first(op::REQ_SET_SPDIF_INPUT_ENABLE));
        // The channel count first: lowering it frees pairs and never fails.
        assert!(first(op::REQ_SET_I2S_INPUT_CHANNELS) < first(op::REQ_SET_I2S_RX_PIN));
        assert!(first(op::REQ_SET_I2S_RX_PIN) < first(op::REQ_SET_INPUT_RATE));
        // The slave pair is stored before SPLIT starts using it.
        assert!(first(op::REQ_SET_I2S_CLOCK_PIN_MODE) > first(op::REQ_SET_I2S_CLOCK_MODE));
        // ADAT out re-routes under a muted restart, so the pin precedes the
        // enable; the same for the input.
        assert!(first(op::REQ_SET_ADAT_PIN) < first(op::REQ_SET_ADAT_ENABLE));
        assert!(first(op::REQ_SET_ADAT_INPUT_PIN) < first(op::REQ_SET_ADAT_INPUT_ENABLE));
        assert!(first(op::REQ_SET_ADAT_INPUT_CLOCK_MODE) < first(op::REQ_SET_ADAT_INPUT_ENABLE));
        assert!(first(op::REQ_SET_ADAT_INPUT_ENABLE) < first(op::REQ_SET_DAC_HW_MUTE_CONFIG));

        assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    }

    /// The input source picks which wiring the device listens to, so it lands
    /// after the wiring has moved.
    #[test]
    fn the_input_source_is_written_after_the_wiring() {
        // The audio pass touches every feature block, and a stalled read costs
        // the retry backoff; this test is about the order, not the gating.
        let (mut s, log) = rig(device().answering_everything(vec![0; 4]));
        let doc = doc_with(io());
        apply(
            &mut s,
            &doc,
            ApplyOptions {
                audio_processing: true,
                volume_levels: false,
                hardware_io: true,
            },
        );

        let seen: Vec<u8> = log
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.opcode == op::REQ_SET_INPUT_SOURCE || e.opcode == op::REQ_SET_ADAT_PIN)
            .map(|e| e.opcode)
            .collect();
        assert_eq!(
            seen.last(),
            Some(&op::REQ_SET_INPUT_SOURCE),
            "the source goes last: {seen:02X?}"
        );
    }

    /// The slave BCK pin carries its role in the SET's wValue high byte, which
    /// no registry row expresses (config.h:334-336).
    #[test]
    fn the_slave_bck_pin_is_addressed_by_role() {
        let (mut s, log) = rig(device());
        apply_io(&mut s, &io(), &mut ApplyReport::default());

        let slave = log
            .lock()
            .unwrap()
            .iter()
            .find(|e| e.opcode == op::REQ_SET_I2S_BCK_PIN && e.value >> 8 == 1)
            .cloned();
        assert_eq!(slave.map(|e| e.value & 0xFF), Some(26));
    }

    /// A refused pin is named with the reason and the apply carries on: a board
    /// that cannot take one pin can usually still take the other twenty.
    #[test]
    fn a_refused_pin_is_reported_and_the_rest_still_applies() {
        let t = device().data(
            op::REQ_SET_OUTPUT_PIN,
            vec![status::PIN_CONFIG_PIN_IN_USE as u8],
        );
        let (mut s, log) = rig(t);
        let mut report = ApplyReport::default();
        apply_io(&mut s, &io(), &mut report);

        assert!(
            report.skipped.iter().any(|x| x
                == "Output 1 GPIO rejected by the device: A pin is already claimed by another \
                    output or interface"),
            "{:?}",
            report.skipped
        );
        // One line per output, not one per attempt.
        assert_eq!(
            report.skipped.iter().filter(|x| x.contains("GPIO")).count(),
            5,
            "{:?}",
            report.skipped
        );
        // And the steps after it still went out.
        assert!(sets(&log).contains(&op::REQ_SET_ADAT_ENABLE));
    }

    /// The PDM pin cannot move while PDM runs (survey-firmware 6.7), so the
    /// output is cycled around the move the way the Console does.
    #[test]
    fn a_busy_pdm_pin_is_cycled_rather_than_given_up_on() {
        let t = device().data(
            op::REQ_SET_OUTPUT_PIN,
            vec![status::PIN_CONFIG_OUTPUT_ACTIVE as u8],
        );
        let (mut s, log) = rig(t);
        apply_io(&mut s, &io(), &mut ApplyReport::default());

        let enables: Vec<(u16, u8)> = log
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.opcode == op::REQ_SET_OUTPUT_ENABLE && e.direction == Direction::Out)
            .map(|e| (e.value, e.payload[0]))
            .collect();
        // Off, the retry, then back to what it was: only the PDM output, and
        // only for the pin the firmware called busy.
        assert_eq!(enables, vec![(8, 0), (8, 1)]);
    }

    /// An accepted pin move must not disturb the output at all.
    #[test]
    fn an_accepted_pin_move_leaves_the_output_alone() {
        let (mut s, log) = rig(device());
        apply_io(&mut s, &io(), &mut ApplyReport::default());
        assert!(
            !log.lock()
                .unwrap()
                .iter()
                .any(|e| e.opcode == op::REQ_SET_OUTPUT_ENABLE && e.direction == Direction::Out)
        );
    }

    /// Nothing in the wiring block is written when the import did not ask for
    /// it, whatever the document says.
    #[test]
    fn wiring_is_left_alone_unless_it_was_asked_for() {
        let (mut s, log) = rig(device());
        let report = apply(
            &mut s,
            &doc_with(io()),
            ApplyOptions {
                audio_processing: false,
                volume_levels: false,
                hardware_io: false,
            },
        );

        assert!(!sets(&log).contains(&op::REQ_SET_OUTPUT_PIN));
        assert!(
            report
                .skipped
                .contains(&"hardware I/O (not requested)".to_string())
        );
    }

    /// An absent key means "the document does not say", not "GPIO 0". That is
    /// the difference between a truncated file being harmless and it moving
    /// the bit clock onto pin 0.
    #[test]
    fn an_absent_field_moves_nothing() {
        let (mut s, log) = rig(device());
        apply_io(&mut s, &IoBlock::default(), &mut ApplyReport::default());
        assert!(sets(&log).is_empty(), "{:02X?}", sets(&log));
    }

    /// A value already in place is not rewritten, because several of these
    /// answer OUTPUT_ACTIVE for a no-op change while the peripheral runs.
    #[test]
    fn a_setting_already_in_place_is_not_rewritten() {
        let (mut s, log) = rig(device());
        let io = IoBlock {
            // The device answers 0 for every pin read.
            i2s_bck_pin: Some(0),
            adat_pin: Some(0),
            ..Default::default()
        };
        apply_io(&mut s, &io, &mut ApplyReport::default());
        assert!(sets(&log).is_empty(), "{:02X?}", sets(&log));
    }

    /// A feature this device does not have is reported, not attempted.
    #[test]
    fn a_missing_feature_is_named_the_way_the_console_names_it() {
        let mut caps = caps();
        caps.features.clear();
        let t = device();
        let log = t.log_handle();
        let mut s = Session::new(Box::new(t), caps).unwrap();

        let mut report = ApplyReport::default();
        apply_io(&mut s, &io(), &mut report);

        for expected in [
            "Additional S/PDIF inputs (not supported by this firmware)",
            "ADAT output (not supported by this device)",
            "ADAT input (not supported by this device)",
            "External DAC hardware mute (not supported by this firmware)",
        ] {
            assert!(
                report.skipped.iter().any(|s| s == expected),
                "missing `{expected}` in {:?}",
                report.skipped
            );
        }
        assert!(!sets(&log).contains(&op::REQ_SET_ADAT_PIN));
    }

    /// The result report is the Console's, word for word.
    #[test]
    fn the_report_reads_the_way_the_console_words_it() {
        let report = ApplyReport {
            channels_applied: 9,
            bands_applied: 90,
            crossover_bands_applied: 4,
            crosspoints_applied: 72,
            missing_channels: vec!["SPDIF 3 L".into()],
            skipped: vec!["ADAT output (not supported by this device)".into()],
        };
        assert_eq!(
            report.lines(false),
            vec![
                "Applied 9 channels, 90 EQ bands, 4 crossover bands, 72 crosspoints.",
                "Not present on this device: SPDIF 3 L",
                "Skipped: ADAT output (not supported by this device)",
                "These changes are live but not yet stored on the device. \
                 Save them to a preset slot to keep them.",
            ]
        );
        assert!(!report.is_clean());

        // A dry run promises nothing about what is live, because nothing is.
        let dry = report.lines(true);
        assert!(dry[0].starts_with("Would apply"));
        assert!(!dry.iter().any(|l| l.contains("live")));
    }

    /// A reason is recorded once, or a per-band problem buries the report.
    #[test]
    fn a_reason_is_recorded_once() {
        let mut report = ApplyReport::default();
        report.skip("ADAT output (not supported by this device)");
        report.skip("ADAT output (not supported by this device)");
        assert_eq!(report.skipped.len(), 1);
    }

    /// The document holds the multiple; the wire holds a selector.
    #[test]
    fn the_master_clock_multiple_is_not_the_wire_value() {
        assert_eq!(mck_selector(128), 0);
        assert_eq!(mck_selector(256), 1);
        assert_eq!(mck_multiple(0), 128);
        assert_eq!(mck_multiple(1), 256);
    }

    /// Captured wiring has to survive a round trip through the file, or an
    /// export followed by an import with `--hardware` would move pins that
    /// were never meant to move.
    #[test]
    fn the_wiring_round_trips_through_the_document() {
        let (mut s, _) = rig(device());
        let captured = capture_io(&mut s);
        assert_eq!(captured.output_pins.len(), 5, "four slots and the PDM sub");
        assert_eq!(captured.i2s_bck_pin, Some(0));
        assert_eq!(captured.spdif_enabled_ext, Some(0));
        assert_eq!(captured.dac_hw_mute.as_ref().map(|m| m.pin), Some(11));
        // The slave BCK pin has no scalar reader, so it stays absent rather
        // than being invented.
        assert_eq!(captured.i2s_bck_pin_slave, None);

        let doc = doc_with(captured.clone());
        let text = write(&doc);
        assert_eq!(parse(&text).unwrap().io, captured);
        assert!(!text.contains("i2sBckPinSlave"), "absent keys are omitted");
    }
}
