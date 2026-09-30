//! The `.dspipreset` whole-device document.
//!
//! Schema and semantics follow the Windows Console's `PresetDocument.cs` and
//! `PresetFileService.cs`, pinned at `DSPi-Console-Windows@81ae00b`. These files
//! are destined for the macOS Console too, so divergence would break
//! interchange rather than merely inconvenience it.
//!
//! The macOS Console's `PresetDocument.swift` and `PresetDocumentTransfer.swift`
//! (DSPi Console `9dbb07a`) are the reference for everything the Windows schema
//! lacks, and for how a channel entry is placed. `docs/preset-format.md` writes
//! the format down, including the channel numbering, which is the part most
//! easily got wrong: a channel entry's `channelId` is the Windows Console's
//! channel id, not the firmware's unified channel index. See [`channel_id`].
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
    /// Absent when the source device had no subharmonic synthesizer
    /// (PresetDocument.swift:45).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subharm: Option<SubharmBlock>,
    /// Absent when the source device had no tube modeller
    /// (PresetDocument.swift:47).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tube: Option<TubeBlock>,
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
    /// The source device's `MASTER_VOLUME_MODE_*` (config.h:484-485), so an
    /// import can say why it left master volume alone. Additive in the
    /// Console (PresetDocument.swift:90-93).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub master_volume_mode: Option<i32>,
    /// The source device's `OUTPUT_CONFIG_MODE_*` (config.h:497-498).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_config_mode: Option<i32>,
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

/// Loudness compensation. A missing key takes the Console's default
/// (PresetDocument.swift:149-164), which is the firmware's: a Windows file
/// written before the output mask existed must not switch loudness off on
/// every output by reading the mask as zero.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LoudnessBlock {
    pub enabled: bool,
    pub ref_spl: f32,
    pub intensity_pct: f32,
    /// Bit k: loudness runs on output k (bulk_params.h:64).
    pub output_mask: i32,
}

impl Default for LoudnessBlock {
    fn default() -> Self {
        Self {
            enabled: false,
            ref_spl: 83.0,
            intensity_pct: 100.0,
            // LOUDNESS_DEFAULT_OUTPUT_MASK (firmware loudness.h:11, not
            // vendored; Constants.swift:34).
            output_mask: 0xFFFF,
        }
    }
}

/// Crossfeed, defaulted as the Console defaults it
/// (PresetDocument.swift:166-185).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CrossfeedBlock {
    pub enabled: bool,
    pub preset: i32,
    pub freq_hz: f32,
    pub feed_db: f32,
    pub itd: bool,
    /// Bit p: crossfeed runs on output pair p (bulk_params.h:76).
    pub output_pair_mask: i32,
}

impl Default for CrossfeedBlock {
    fn default() -> Self {
        Self {
            enabled: false,
            preset: 0,
            freq_hz: 700.0,
            feed_db: 4.5,
            itd: true,
            // The firmware's pair 1 only (usb_audio.c:261, not vendored;
            // CROSSFEED_DEFAULT_OUTPUT_MASK, Constants.swift:50).
            output_pair_mask: 0x01,
        }
    }
}

/// The volume leveller, defaulted as the Console defaults it
/// (PresetDocument.swift:187-210).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LevellerBlock {
    pub enabled: bool,
    pub speed: i32,
    pub lookahead: bool,
    pub amount_pct: f32,
    pub max_gain_db: f32,
    pub gate_db: f32,
    /// `[detector_mask, apply_mask]` on the wire (config.h:441).
    pub detector_mask: i32,
    pub apply_mask: i32,
}

impl Default for LevellerBlock {
    fn default() -> Self {
        Self {
            enabled: false,
            speed: 0,
            lookahead: true,
            amount_pct: 50.0,
            max_gain_db: 15.0,
            gate_db: -96.0,
            detector_mask: 0xFF,
            apply_mask: 0xFF,
        }
    }
}

/// Psychoacoustic bass, defaulted to the firmware's values
/// (psybass.h:50-55, PresetDocument.swift:212-233).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

impl Default for PsybassBlock {
    fn default() -> Self {
        Self {
            enabled: false,
            cutoff_hz: 80.0,
            harmonics_db: 0.0,
            drive_db: 6.0,
            character_pct: 50.0,
            original_db: 0.0,
            // PSYBASS_DEFAULT_OUTPUT_MASK (psybass.h:55).
            output_mask: 0xFFFF,
        }
    }
}

/// The stereo upmixer, defaulted to the firmware's values (upmix.h:111-123,
/// PresetDocument.swift:311-346). The two modes are the raw
/// `UPMIX_CENTER_*` and `UPMIX_SURROUND_*` numbers (upmix.h:78-85).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

impl Default for UpmixBlock {
    fn default() -> Self {
        use dspi_proto::generated::{ranges as r, upmix as u};
        Self {
            enabled: false,
            center_mode: u::UPMIX_CENTER_ADAPTIVE as i32,
            surround_mode: u::UPMIX_SURROUND_ADAPTIVE as i32,
            strength_pct: r::UPMIX_DEFAULT_STRENGTH,
            center_width_pct: r::UPMIX_DEFAULT_WIDTH,
            threshold_pct: r::UPMIX_DEFAULT_THRESH,
            attack_ms: r::UPMIX_DEFAULT_ATTACK,
            release_ms: r::UPMIX_DEFAULT_RELEASE,
            detector_hpf_hz: r::UPMIX_DEFAULT_DET_HPF,
            surround_delay_ms: r::UPMIX_DEFAULT_SUR_DELAY,
            surround_hpf_hz: r::UPMIX_DEFAULT_SUR_HPF,
            surround_lpf_hz: r::UPMIX_DEFAULT_SUR_LPF,
            decorr_pct: r::UPMIX_DEFAULT_DECORR,
            presence_db: r::UPMIX_DEFAULT_PRESENCE,
        }
    }
}

/// The subharmonic synthesizer, as the Console's `SubharmBlock`
/// (PresetDocument.swift:235-268). A missing key takes the firmware's
/// default (subharm.h:85-94), so a document from an older writer restores a
/// two-band setup rather than an arbitrary one. Solo is absent by design: it
/// is runtime only, and no saved configuration may switch the program off.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SubharmBlock {
    pub enabled: bool,
    pub low_db: f32,
    pub high_db: f32,
    pub top_db: f32,
    pub boost_db: f32,
    pub output_mask: i32,
    /// `SUBHARM_SELECT_*` (subharm.h:71-73), as the wire number.
    pub select_mode: i32,
    pub select_depth_pct: f32,
    pub select_hold_ms: f32,
    pub ceiling_db: f32,
    pub link_pairs: bool,
}

impl Default for SubharmBlock {
    fn default() -> Self {
        use dspi_proto::generated::{ranges as r, subharm as s};
        Self {
            enabled: false,
            low_db: r::SUBHARM_DEFAULT_LOW,
            high_db: r::SUBHARM_DEFAULT_HIGH,
            // SUBHARM_DEFAULT_TOP is SUBHARM_LEVEL_MIN: the band ships off.
            top_db: r::SUBHARM_LEVEL_MIN,
            boost_db: r::SUBHARM_DEFAULT_BOOST,
            output_mask: s::SUBHARM_DEFAULT_OUTPUT_MASK as i32,
            select_mode: s::SUBHARM_SELECT_ALL as i32,
            select_depth_pct: r::SUBHARM_DEFAULT_DEPTH,
            select_hold_ms: r::SUBHARM_DEFAULT_HOLD_MS,
            ceiling_db: r::SUBHARM_DEFAULT_CEILING,
            // SUBHARM_DEFAULT_LINK_PAIRS (subharm.h:94).
            link_pairs: true,
        }
    }
}

/// The tube modeller, as the Console's `TubeBlock` (PresetDocument.swift:
/// 270-309). Every value is stored as the firmware holds it, the four
/// character values included, so a file saved on a tube type restores that
/// exact sound even if a later firmware retunes the row. Defaults are the
/// firmware's (tube.h:60-73).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TubeBlock {
    pub enabled: bool,
    pub output_mask: i32,
    /// 0 Custom, 1..=`TUBE_TYPE_MAX`.
    pub tube_type: i32,
    pub drive_db: f32,
    pub bias_pct: f32,
    pub asym_db: f32,
    pub hardness_pct: f32,
    pub sag_pct: f32,
    /// 0..=`TUBE_RECT_MAX`.
    pub rectifier: i32,
    pub xfmr_enabled: bool,
    pub xfmr_damping: f32,
    pub xfmr_res_hz: f32,
    pub mix_pct: f32,
    pub trim_db: f32,
}

impl Default for TubeBlock {
    fn default() -> Self {
        use dspi_proto::generated::{ranges as r, tube as t};
        Self {
            enabled: false,
            output_mask: t::TUBE_DEFAULT_OUTPUT_MASK as i32,
            tube_type: t::TUBE_DEFAULT_TUBE_TYPE as i32,
            drive_db: r::TUBE_DEFAULT_DRIVE,
            bias_pct: r::TUBE_DEFAULT_BIAS,
            asym_db: r::TUBE_DEFAULT_ASYM,
            hardness_pct: r::TUBE_DEFAULT_HARDNESS,
            sag_pct: r::TUBE_DEFAULT_SAG,
            rectifier: t::TUBE_DEFAULT_RECTIFIER as i32,
            // TUBE_DEFAULT_XFMR_ENABLED (tube.h:68).
            xfmr_enabled: true,
            xfmr_damping: r::TUBE_DEFAULT_XFMR_DAMPING,
            xfmr_res_hz: r::TUBE_DEFAULT_XFMR_RES,
            mix_pct: r::TUBE_DEFAULT_MIX,
            trim_db: r::TUBE_DEFAULT_TRIM,
        }
    }
}

/// One output's limiter, as the Console's `LimiterBlock`
/// (PresetDocument.swift:406-432). Defaults are the firmware's
/// (limiter.h:32-33).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LimiterBlock {
    pub enabled: bool,
    pub threshold_db: f32,
    pub release_ms: f32,
    /// 0 unlinked, 1..=`LIMITER_LINK_GROUP_MAX`.
    pub link_group: i32,
}

impl Default for LimiterBlock {
    fn default() -> Self {
        use dspi_proto::generated::ranges as r;
        Self {
            enabled: false,
            threshold_db: r::LIMITER_DEFAULT_THRESHOLD,
            release_ms: r::LIMITER_DEFAULT_RELEASE,
            link_group: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChannelBlock {
    /// The Windows Console's channel id, which is not the firmware's unified
    /// channel index: see [`channel_id`]. Written for every channel, and read
    /// when the entry carries no `inputIndex` or `outputIndex`.
    pub channel_id: i32,
    #[serde(default)]
    pub name: String,
    pub is_output: bool,
    /// The unified channel index (config.h:788-790): inputs from 0, outputs
    /// from `CH_OUT_1`. Written as the Console writes it
    /// (PresetDocumentTransfer.swift:192); neither Console places a channel
    /// by it, and nor does this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eq_channel: Option<i32>,
    /// Wire input index 0..7, on inputs only. Wins over `channelId`
    /// (PresetDocument.swift:679-683).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_index: Option<i32>,
    /// Matrix output index, on outputs only. Wins over `channelId`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_index: Option<i32>,
    /// Pre-matrix channel delay (`REQ_SET_DELAY`, config.h:245).
    pub delay_ms: f32,
    pub gain_db: f32,
    pub muted: bool,
    pub enabled: bool,
    /// Post-matrix output delay (`REQ_SET_OUTPUT_DELAY`, config.h:311), a
    /// separate value from `delayMs` that the Windows schema omits. Outputs
    /// only; absent leaves the device's alone (PresetDocument.swift:372-374).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_delay_ms: Option<f32>,
    #[serde(default)]
    pub eq: Vec<BandBlock>,
    /// Empty for inputs.
    #[serde(default)]
    pub crossover: Vec<BandBlock>,
    /// This output's limiter (PresetDocument.swift:375-379). Absent on inputs
    /// and when the source device had no limiter, which leaves the device's
    /// own alone. Applied only with the hardware I/O option, because the
    /// firmware keeps it with the output configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limiter: Option<LimiterBlock>,
}

impl Default for ChannelBlock {
    fn default() -> Self {
        Self {
            channel_id: 0,
            name: String::new(),
            is_output: false,
            eq_channel: None,
            input_index: None,
            output_index: None,
            delay_ms: 0.0,
            gain_db: 0.0,
            muted: false,
            // A channel is enabled unless the file says otherwise, matching the
            // reference implementation's property default.
            enabled: true,
            output_delay_ms: None,
            eq: Vec::new(),
            crossover: Vec::new(),
            limiter: None,
        }
    }
}

/// Where a channel entry lands on a device: a wire input index, or a matrix
/// output index (the Console's `PresetChannelRef`, PresetDocument.swift:
/// 617-621). Inputs sort before outputs, which is the order the Console
/// applies them in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChannelRef {
    Input(u8),
    Output(u8),
}

impl ChannelRef {
    /// The unified channel index on a device with `num_inputs` inputs:
    /// outputs start at `CH_OUT_1 = NUM_INPUT_CHANNELS` (config.h:788-790).
    pub fn unified(self, num_inputs: u8) -> u8 {
        match self {
            ChannelRef::Input(i) => i,
            ChannelRef::Output(o) => num_inputs.saturating_add(o),
        }
    }
}

/// The Windows Console's channel ids, which both Consoles write as
/// `channelId` (PresetDocument.swift:623-672).
///
/// They were laid down when the device had two inputs, and outputs grew in
/// the middle: inputs 0 and 1 keep their index, outputs follow from 2, and
/// the extra inputs of the eight-input model sit above the widest output
/// bank at 11..16. On RP2040 (two inputs, five outputs, config.h:776-777)
/// this coincides with the unified index; on RP2350 (eight inputs, nine
/// outputs, config.h:773-774) it does not, which is why the unified index
/// must never be written here. The Console gives RP2350's PDM output id 10
/// by a special case, which is the same number the arithmetic gives, so no
/// platform test is needed and none is compiled in.
///
/// These numbers are the file format's, not the device's: a device's own
/// input and output counts decide only whether a resolved channel exists.
pub mod channel_id {
    use super::ChannelRef;

    /// Inputs below this keep their index as their id: the stereo inputs
    /// (`NUM_STEREO_INPUTS`, config.h:784; the Console's
    /// `BASE_MATRIX_INPUTS`, Constants.swift:772).
    const STEREO_INPUTS: i32 = 2;
    /// The id of the first output (S/PDIF 1 L).
    const OUTPUT_BASE: i32 = 2;
    /// The id of wire input 2, the first extra input.
    const EXTRA_INPUT_BASE: i32 = 11;
    /// Extra inputs the schema has ids for: eight inputs less the stereo two
    /// (`MAX_MATRIX_INPUTS`, Constants.swift:771).
    const EXTRA_INPUTS: i32 = 6;

    /// The id for a wire input index.
    pub fn for_input(input: u8) -> i32 {
        let i = input as i32;
        if i < STEREO_INPUTS {
            i
        } else {
            EXTRA_INPUT_BASE + (i - STEREO_INPUTS)
        }
    }

    /// The id for a matrix output index.
    pub fn for_output(output: u8) -> i32 {
        OUTPUT_BASE + output as i32
    }

    /// The channel an id names, before asking whether the device has it
    /// (`PresetChannelID.ref(forID:platform:)`). Inputs are tested first, so
    /// 11..16 are always inputs.
    pub fn resolve(id: i32) -> Option<ChannelRef> {
        if (0..STEREO_INPUTS).contains(&id) {
            return Some(ChannelRef::Input(id as u8));
        }
        if (EXTRA_INPUT_BASE..EXTRA_INPUT_BASE + EXTRA_INPUTS).contains(&id) {
            return Some(ChannelRef::Input(
                (STEREO_INPUTS + id - EXTRA_INPUT_BASE) as u8,
            ));
        }
        u8::try_from(id - OUTPUT_BASE).ok().map(ChannelRef::Output)
    }

    /// Whether an entry's direction agrees with the id table. Every file
    /// either Console writes agrees; an older Terminal file written on an
    /// eight-input device does not.
    pub(super) fn agrees(id: i32, is_output: bool) -> bool {
        match resolve(id) {
            Some(ChannelRef::Input(_)) => !is_output,
            Some(ChannelRef::Output(_)) => is_output,
            None => false,
        }
    }
}

impl ChannelBlock {
    /// The channel this entry refers to, as the Console reads it
    /// (PresetDocument.swift:674-684): its own `inputIndex` or `outputIndex`
    /// when present, otherwise the shared `channelId`.
    ///
    /// `legacy_inputs` is set only for a document an older Terminal wrote,
    /// whose `channelId` is the unified index on a device with that many
    /// inputs (see [`legacy_numbering`]).
    pub fn placement(&self, legacy_inputs: Option<u8>) -> Option<ChannelRef> {
        if self.is_output
            && let Some(o) = self.output_index
        {
            return u8::try_from(o).ok().map(ChannelRef::Output);
        }
        if !self.is_output
            && let Some(i) = self.input_index
        {
            return u8::try_from(i).ok().map(ChannelRef::Input);
        }
        if let Some(n) = legacy_inputs {
            let n = n as i32;
            return match self.channel_id {
                id if (0..n).contains(&id) => Some(ChannelRef::Input(id as u8)),
                id => u8::try_from(id - n).ok().map(ChannelRef::Output),
            };
        }
        channel_id::resolve(self.channel_id)
    }
}

/// Whether a document uses the numbering Terminal builds before this one
/// wrote, and with how many inputs.
///
/// Those builds wrote the unified channel index as `channelId` and none of
/// the Console's own index fields. On a two-input device the two numberings
/// coincide, so only an eight-input document can differ, and there the
/// difference shows: the older numbering marks ids 2..7 as inputs and 8..16
/// as outputs, where the id table has 2..10 as outputs and 11..16 as inputs.
/// A document counts as older only when no entry carries an index field,
/// every entry agrees with the unified numbering, and at least one disagrees
/// with the id table. No file either Console writes can pass that test,
/// since every one of their entries agrees with the table.
///
/// The input count is the document's own `inputChannelCount`, or the
/// connected device's when the document does not say.
pub fn legacy_numbering(doc: &PresetDocument, device_inputs: u8) -> Option<u8> {
    let n = u8::try_from(doc.meta.input_channel_count)
        .ok()
        .filter(|&n| n > 0)
        .unwrap_or(device_inputs);
    let own_fields = doc
        .channels
        .iter()
        .any(|c| c.eq_channel.is_some() || c.input_index.is_some() || c.output_index.is_some());
    let unified = doc
        .channels
        .iter()
        .all(|c| c.channel_id >= 0 && (c.channel_id < n as i32) != c.is_output);
    let table = doc
        .channels
        .iter()
        .all(|c| channel_id::agrees(c.channel_id, c.is_output));
    (!own_fields && unified && !table).then_some(n)
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

/// A document's channels placed on a device, and the names of those it has
/// nowhere to put.
pub type Placement<'a> = std::collections::BTreeMap<ChannelRef, &'a ChannelBlock>;

/// Place a document's channels on a device with this many inputs and
/// outputs, as the Console does (PresetDocumentTransfer.swift:302-313).
///
/// Each entry lands where its own index fields or its `channelId` say, read
/// against the connected device; one that names a channel the device does
/// not have is reported rather than moved somewhere else, so an eight-input
/// document on a two-input part reports its extra inputs. A duplicated
/// channel takes the last entry rather than failing.
pub fn place_channels(
    doc: &PresetDocument,
    num_inputs: u8,
    num_outputs: u8,
) -> (Placement<'_>, Vec<String>) {
    let legacy = legacy_numbering(doc, num_inputs);
    let mut placed = Placement::new();
    let mut missing = Vec::new();

    for c in &doc.channels {
        let exists = |r: &ChannelRef| match *r {
            ChannelRef::Input(i) => i < num_inputs,
            ChannelRef::Output(o) => o < num_outputs,
        };
        match c.placement(legacy).filter(exists) {
            Some(r) => {
                placed.insert(r, c);
            }
            None => missing.push(if c.name.is_empty() {
                format!("channel {}", c.channel_id)
            } else {
                c.name.clone()
            }),
        }
    }
    (placed, missing)
}

// ---------------------------------------------------------------------------
// Applying
// ---------------------------------------------------------------------------

use crate::{EnableOutcome, Outcome, Session};
use dspi_proto::generated::status;
use dspi_proto::value::{EqParamPacket, Value};

/// `MASTER_VOLUME_MODE_WITH_PRESET` (config.h:485), which the generated
/// constants do not carry.
const MASTER_VOLUME_MODE_WITH_PRESET: u8 = 1;

/// Crossover bands per output, at wire band indices 20..23.
const CROSSOVER_BANDS: u8 = 4;
const CROSSOVER_FIRST_BAND: u8 = 20;

/// Apply a document to a device.
///
/// Everything goes through the ordinary write path, so each value gets the same
/// clamping, capability gating and readback verification as a typed command.
/// Nothing is applied that the options did not ask for, and the report says what
/// actually happened rather than leaving the user to infer it from the UI.
///
/// The input pair links (`global.inputPairLinked`) are the app's, not the
/// device's, so this leaves them to the caller; the Terminal applies them to
/// its own link state after this returns.
pub fn apply(session: &mut Session, doc: &PresetDocument, options: ApplyOptions) -> ApplyReport {
    let mut report = ApplyReport::default();

    let caps = session.capabilities().clone();
    let (placed, missing) = place_channels(doc, caps.num_inputs, caps.num_outputs);
    report.missing_channels = missing;

    if !options.audio_processing {
        report
            .skipped
            .push("audio processing (not requested)".into());
    } else {
        apply_output_enables(session, &placed, &mut report);

        // One channel at a time, inputs first, as the Console does
        // (PresetDocumentTransfer.swift:394-415).
        for (&r, block) in &placed {
            apply_channel(session, r, block, &mut report);
            if let ChannelRef::Input(i) = r {
                let db = doc
                    .global
                    .input_preamps_db
                    .get(i as usize)
                    .copied()
                    .unwrap_or(0.0);
                let _ = session.write("pre", &[i], Value::Float(db));
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
        apply_volumes(session, doc, &mut report);
    } else {
        report.skip("volume levels (not requested)");
    }

    if options.hardware_io {
        apply_io(session, &doc.io, &mut report);
        apply_limiters(session, &placed, &mut report);
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

/// Output enables: every disable, then every enable
/// (PresetDocumentTransfer.swift:377-392). An enable can collide with an
/// output the document is about to switch off (PDM against the Core 1 EQ
/// workers), so freeing first is what lets the pair land. An enable that
/// would still collide is reported in the Console's words, not forced.
fn apply_output_enables(session: &mut Session, placed: &Placement, report: &mut ApplyReport) {
    for enabling in [false, true] {
        for (&r, block) in placed {
            let ChannelRef::Output(o) = r else { continue };
            if block.enabled != enabling {
                continue;
            }
            let now = session
                .read("out.enable", &[o])
                .ok()
                .and_then(|v| v.as_bool());
            if now == Some(enabling) {
                continue;
            }
            if let Ok(EnableOutcome::NeedsConfirm(_)) = session.enable_output(o, enabling) {
                report.skip(format!(
                    "{} could not be enabled (conflicts with another output)",
                    block.name
                ));
            }
        }
    }
}

/// One channel's name, delay, output strip and filter banks
/// (PresetDocumentTransfer.swift:451-505).
fn apply_channel(
    session: &mut Session,
    r: ChannelRef,
    block: &ChannelBlock,
    report: &mut ApplyReport,
) {
    let caps = session.capabilities().clone();
    let ch = r.unified(caps.num_inputs);

    let current_name = caps
        .channels
        .iter()
        .find(|c| c.index == ch)
        .map(|c| c.name.as_str());
    if !block.name.is_empty() && current_name != Some(block.name.as_str()) {
        let _ = session.write("ch.name", &[ch], Value::Text(block.name.clone()));
    }
    let _ = session.write("ch.delay", &[ch], Value::Float(block.delay_ms));

    if let ChannelRef::Output(o) = r {
        let _ = session.write("out.gain", &[o], Value::Float(block.gain_db));
        let muted = session
            .read("out.mute", &[o])
            .ok()
            .and_then(|v| v.as_bool());
        if muted != Some(block.muted) {
            let _ = session.write("out.mute", &[o], Value::Bool(block.muted));
        }
        // Additive: a document without it leaves the output delay alone.
        if let Some(ms) = block.output_delay_ms {
            let _ = session.write("out.delay", &[o], Value::Float(ms));
        }
    }

    // A document with no bands for a channel leaves its EQ alone; one with
    // some flattens the rest, so an imported channel is never a blend of two
    // configurations. Bands past this device's bank are dropped.
    let flat = BandBlock::default();
    if !block.eq.is_empty() {
        for band in 0..caps.max_bands {
            let b = block.eq.get(band as usize).unwrap_or(&flat);
            if apply_band(session, ch, band, b).is_some() {
                report.bands_applied += 1;
            }
        }
    }
    if matches!(r, ChannelRef::Output(_)) && !block.crossover.is_empty() {
        for band in 0..CROSSOVER_BANDS {
            let b = block.crossover.get(band as usize).unwrap_or(&flat);
            if apply_band(session, ch, CROSSOVER_FIRST_BAND + band, b).is_some() {
                report.crossover_bands_applied += 1;
            }
        }
    }

    report.channels_applied += 1;
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

/// Master and listening volume (PresetDocumentTransfer.swift:669-680).
/// Master volume belongs to a preset only when the device says so; in
/// independent mode it is device-global, and a file overwriting it would
/// fight the user's own setting.
fn apply_volumes(session: &mut Session, doc: &PresetDocument, report: &mut ApplyReport) {
    let mode = current_u8(session, "vol.master.mode", &[]);
    if mode == Some(MASTER_VOLUME_MODE_WITH_PRESET) {
        let _ = session.write("vol.master", &[], Value::Float(doc.global.master_volume_db));
    } else {
        report.skip("Master volume (device is in independent master-volume mode)");
    }
    let _ = session.write("vol.user", &[], Value::Float(doc.global.user_volume_db));
}

/// The feature blocks, in the Console's order
/// (PresetDocumentTransfer.swift:537-667): each feature's parameters before
/// its switch, so the device never runs it for a moment on the old values,
/// and each mask only where the firmware has one. A block the device lacks
/// is skipped with a reason.
fn apply_features(session: &mut Session, doc: &PresetDocument, report: &mut ApplyReport) {
    let caps = session.capabilities().clone();
    let has = |name: &str| caps.features.iter().any(|f| f.name == name && f.present);
    let mut set = |path: &str, v: Value, label: &str, report: &mut ApplyReport| {
        if let Err(e) = session.write(path, &[], v) {
            // An absent feature is information, not a failure: a document from a
            // better-equipped device should still apply everything else.
            if matches!(e, crate::WriteError::Unavailable { .. }) {
                report.skip(format!("{label} (not on this device)"));
            }
        }
    };
    let mask = |m: i32| Value::Mask(m as u16 as u32);
    let choice = |c: i32| Value::Choice(c.clamp(0, 255) as u8);

    // The input source is applied at the very end of `apply`, after any
    // hardware wiring has moved.
    set(
        "bypass",
        Value::Bool(doc.global.bypass),
        "EQ bypass",
        report,
    );

    if has("lg_sound_sync") {
        set(
            "in.lg",
            Value::Bool(doc.global.lg_sound_sync_enabled),
            "LG Sound Sync",
            report,
        );
    } else if doc.global.lg_sound_sync_enabled {
        report.skip("LG Sound Sync (not supported by this firmware)");
    }

    let l = &doc.loudness;
    set("loud.ref", Value::Float(l.ref_spl), "loudness", report);
    set(
        "loud.intensity",
        Value::Float(l.intensity_pct),
        "loudness",
        report,
    );
    if has("loudness_output_mask") {
        set("loud.mask", mask(l.output_mask), "loudness", report);
    }
    set("loud.on", Value::Bool(l.enabled), "loudness", report);

    let c = &doc.crossfeed;
    set("cf.preset", choice(c.preset), "crossfeed", report);
    set("cf.freq", Value::Float(c.freq_hz), "crossfeed", report);
    set("cf.feed", Value::Float(c.feed_db), "crossfeed", report);
    set("cf.itd", Value::Bool(c.itd), "crossfeed", report);
    if has("crossfeed_output_mask") {
        set(
            "cf.outputs",
            mask(c.output_pair_mask & 0xFF),
            "crossfeed",
            report,
        );
    }
    set("cf.on", Value::Bool(c.enabled), "crossfeed", report);

    let v = &doc.leveller;
    set("lev.speed", choice(v.speed), "leveller", report);
    set(
        "lev.lookahead",
        Value::Bool(v.lookahead),
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
    set("lev.gate", Value::Float(v.gate_db), "leveller", report);
    if has("leveller_masks") {
        // One two-byte parameter, detector first (config.h:441).
        let word = (v.detector_mask & 0xFF) | ((v.apply_mask & 0xFF) << 8);
        set("lev.masks", mask(word), "leveller", report);
    }
    set("lev.on", Value::Bool(v.enabled), "leveller", report);

    if let Some(b) = &doc.psybass {
        if has("psychoacoustic_bass") {
            let label = "psychoacoustic bass";
            set("bass.cutoff", Value::Float(b.cutoff_hz), label, report);
            set(
                "bass.harmonics",
                Value::Float(b.harmonics_db),
                label,
                report,
            );
            set("bass.drive", Value::Float(b.drive_db), label, report);
            set(
                "bass.character",
                Value::Float(b.character_pct),
                label,
                report,
            );
            set("bass.original", Value::Float(b.original_db), label, report);
            set("bass.mask", mask(b.output_mask), label, report);
            set("bass.on", Value::Bool(b.enabled), label, report);
        } else {
            report.skip("Psychoacoustic bass (not supported by this firmware)");
        }
    }

    apply_subharm_and_tube(session, doc, report);

    if let Some(u) = &doc.upmix {
        if has("upmixer") {
            // UPMIX_PARAM_* (upmix.h:188-201) through the registry's rows.
            let mut set = |path: &str, v: Value| {
                let _ = session.write(path, &[], v);
            };
            set("up.center_mode", choice(u.center_mode));
            set("up.surround_mode", choice(u.surround_mode));
            set("up.strength", Value::Float(u.strength_pct));
            set("up.width", Value::Float(u.center_width_pct));
            set("up.threshold", Value::Float(u.threshold_pct));
            set("up.attack", Value::Float(u.attack_ms));
            set("up.release", Value::Float(u.release_ms));
            set("up.det_hpf", Value::Float(u.detector_hpf_hz));
            set("up.sur_delay", Value::Float(u.surround_delay_ms));
            set("up.sur_hpf", Value::Float(u.surround_hpf_hz));
            set("up.sur_lpf", Value::Float(u.surround_lpf_hz));
            set("up.decorr", Value::Float(u.decorr_pct));
            set("up.presence", Value::Float(u.presence_db));
            set("up.on", Value::Bool(u.enabled));
        } else {
            report.skip("Stereo upmixer (not supported by this device)");
        }
    }
}

/// The subharmonic synthesizer and the tube modeller, in the Console's order
/// (PresetDocumentTransfer.swift:600-644): every parameter first and the
/// switch last, so a block never plays half-applied, and the tube type
/// before the four values it loads. A stored value that differs from the
/// type's row then drops the type to Custom on the device (tube.c:122-212),
/// so a file matching its row keeps its type and one that does not still
/// restores its exact sound.
fn apply_subharm_and_tube(session: &mut Session, doc: &PresetDocument, report: &mut ApplyReport) {
    let caps = session.capabilities().clone();
    let has = |name: &str| caps.features.iter().any(|f| f.name == name && f.present);
    // A refused value is left to the re-read to show; the Console does not
    // report one either.
    let mut set = |path: &str, v: Value| {
        let _ = session.write(path, &[], v);
    };

    if let Some(b) = &doc.subharm {
        if has("subharmonic_synth") {
            set("sub.low", Value::Float(b.low_db));
            set("sub.high", Value::Float(b.high_db));
            set("sub.boost", Value::Float(b.boost_db));
            set("sub.mask", Value::Mask(b.output_mask as u16 as u32));
            set("sub.top", Value::Float(b.top_db));
            set(
                "sub.select",
                Value::Choice(b.select_mode.clamp(0, 255) as u8),
            );
            set("sub.depth", Value::Float(b.select_depth_pct));
            set("sub.hold", Value::Float(b.select_hold_ms));
            set("sub.ceiling", Value::Float(b.ceiling_db));
            set("sub.link", Value::Bool(b.link_pairs));
            set("sub.on", Value::Bool(b.enabled));
        } else {
            report.skip("Subharmonic synthesizer (not supported by this firmware)");
        }
    }

    if let Some(b) = &doc.tube {
        if has("tube_preamp") {
            set("tube.type", Value::Int(b.tube_type as i64));
            set("tube.bias", Value::Float(b.bias_pct));
            set("tube.asym", Value::Float(b.asym_db));
            set("tube.hardness", Value::Float(b.hardness_pct));
            set("tube.sag", Value::Float(b.sag_pct));
            set("tube.drive", Value::Float(b.drive_db));
            set(
                "tube.rectifier",
                Value::Choice(b.rectifier.clamp(0, 255) as u8),
            );
            set("tube.damping", Value::Float(b.xfmr_damping));
            set("tube.resonance", Value::Float(b.xfmr_res_hz));
            set("tube.xfmr", Value::Bool(b.xfmr_enabled));
            set("tube.mix", Value::Float(b.mix_pct));
            set("tube.trim", Value::Float(b.trim_db));
            set("tube.mask", Value::Mask(b.output_mask as u16 as u32));
            set("tube.on", Value::Bool(b.enabled));
        } else {
            report.skip("Tube preamp (not supported by this firmware)");
        }
    }
}

/// The output limiters, from each output's block, in the Console's order
/// (`applyLimiterSettings`, Commands.swift:1658-1672).
///
/// They travel with the hardware I/O option, not with the audio, because
/// the firmware keeps them with the output configuration
/// (PresetDocumentTransfer.swift:422-445). A write to one member of a link
/// group moves the whole group (limiter.c:143-194), so every output about to
/// change is unlinked first, then given its values, and the groups are set
/// again last in ascending order, each joining output adopting the settings
/// its lowest member already holds. An output with no block is left alone.
fn apply_limiters(session: &mut Session, placed: &Placement, report: &mut ApplyReport) {
    let caps = session.capabilities().clone();
    // Placed as every other channel value is, so a limiter lands on the
    // output its gain and EQ land on.
    let blocks: std::collections::BTreeMap<u8, &LimiterBlock> = placed
        .iter()
        .filter_map(|(r, c)| match (r, &c.limiter) {
            (ChannelRef::Output(o), Some(l)) => Some((*o, l)),
            _ => None,
        })
        .collect();
    if blocks.is_empty() {
        return;
    }
    if !caps
        .features
        .iter()
        .any(|f| f.name == "output_limiter" && f.present)
    {
        report.skip("Output limiter (not supported by this firmware)");
        return;
    }
    for &o in blocks.keys() {
        if current_u8(session, "limit.link", &[o]).is_some_and(|g| g != 0) {
            let _ = session.write("limit.link", &[o], Value::Int(0));
        }
    }
    for (&o, l) in &blocks {
        let _ = session.write("limit.threshold", &[o], Value::Float(l.threshold_db));
        let _ = session.write("limit.release", &[o], Value::Float(l.release_ms));
        let _ = session.write("limit.on", &[o], Value::Bool(l.enabled));
    }
    for (&o, l) in &blocks {
        if l.link_group != 0 {
            let _ = session.write("limit.link", &[o], Value::Int(l.link_group as i64));
        }
    }
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

/// Capture the current device state as a document, in the Console's shape
/// (PresetDocumentTransfer.swift:20-197): every channel carries the Windows
/// id as `channelId` and the Console's own index fields beside it, so either
/// Console places it where it came from.
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
    // A mask the device will not give up reads as the given default, which is
    // what the Console's model holds before its first fetch.
    let read_mask = |s: &mut Session, path: &str, default: i32| -> i32 {
        match s.read(path, &[]) {
            Ok(Value::Mask(m)) => m as i32,
            _ => default,
        }
    };
    let has = |name: &str| caps.features.iter().any(|f| f.name == name && f.present);

    let mut channels = Vec::new();
    for c in &caps.channels {
        let r = if c.is_output {
            ChannelRef::Output(c.index.saturating_sub(caps.num_inputs))
        } else {
            ChannelRef::Input(c.index)
        };
        let mut block = ChannelBlock {
            name: c.name.clone(),
            is_output: c.is_output,
            eq_channel: Some(c.index as i32),
            delay_ms: read_f32(session, "ch.delay", &[c.index]),
            ..Default::default()
        };
        match r {
            ChannelRef::Input(i) => {
                block.channel_id = channel_id::for_input(i);
                block.input_index = Some(i as i32);
            }
            ChannelRef::Output(out) => {
                block.channel_id = channel_id::for_output(out);
                block.output_index = Some(out as i32);
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
                block.output_delay_ms = session
                    .read("out.delay", &[out])
                    .ok()
                    .and_then(|v| v.as_f32());
                if has("output_limiter") {
                    let on = session
                        .read("limit.on", &[out])
                        .ok()
                        .and_then(|v| v.as_bool());
                    block.limiter = on.map(|enabled| LimiterBlock {
                        enabled,
                        threshold_db: read_f32(session, "limit.threshold", &[out]),
                        release_ms: read_f32(session, "limit.release", &[out]),
                        link_group: session
                            .read("limit.link", &[out])
                            .ok()
                            .and_then(|v| v.as_u8())
                            .unwrap_or(0) as i32,
                    });
                }
            }
        }
        for b in 0..caps.max_bands {
            if let Ok(p) = session.read_band(c.index, b) {
                block.eq.push(band_block(&p));
            }
        }
        if c.is_output {
            for b in CROSSOVER_FIRST_BAND..CROSSOVER_FIRST_BAND + CROSSOVER_BANDS {
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

    let defaults = (
        LoudnessBlock::default(),
        CrossfeedBlock::default(),
        LevellerBlock::default(),
        PsybassBlock::default(),
    );
    // `[detector, apply]` in one word (config.h:441).
    let lev_masks = if has("leveller_masks") {
        read_mask(session, "lev.masks", 0xFFFF)
    } else {
        0xFFFF
    };
    let mode = |s: &mut Session, path: &str| {
        s.read(path, &[])
            .ok()
            .and_then(|v| v.as_u8())
            .map(i32::from)
    };

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
            saved_utc: Some(utc_now()),
            master_volume_mode: mode(session, "vol.master.mode"),
            output_config_mode: mode(session, "preset.iomode"),
        },
        global: GlobalBlock {
            input_preamps_db: preamps,
            bypass: read_bool(session, "bypass"),
            master_volume_db: read_f32(session, "vol.master", &[]),
            user_volume_db: read_f32(session, "vol.user", &[]),
            input_source: read_u8(session, "in.source"),
            lg_sound_sync_enabled: read_bool(session, "in.lg"),
            // The app's own; the caller fills it in (see `apply`).
            input_pair_linked: vec![false; 4],
        },
        loudness: LoudnessBlock {
            enabled: read_bool(session, "loud.on"),
            ref_spl: read_f32(session, "loud.ref", &[]),
            intensity_pct: read_f32(session, "loud.intensity", &[]),
            output_mask: if has("loudness_output_mask") {
                read_mask(session, "loud.mask", defaults.0.output_mask)
            } else {
                defaults.0.output_mask
            },
        },
        crossfeed: CrossfeedBlock {
            enabled: read_bool(session, "cf.on"),
            preset: read_u8(session, "cf.preset") as i32,
            freq_hz: read_f32(session, "cf.freq", &[]),
            feed_db: read_f32(session, "cf.feed", &[]),
            itd: read_bool(session, "cf.itd"),
            output_pair_mask: if has("crossfeed_output_mask") {
                read_mask(session, "cf.outputs", defaults.1.output_pair_mask) & 0xFF
            } else {
                defaults.1.output_pair_mask
            },
        },
        leveller: LevellerBlock {
            enabled: read_bool(session, "lev.on"),
            speed: read_u8(session, "lev.speed") as i32,
            lookahead: read_bool(session, "lev.lookahead"),
            amount_pct: read_f32(session, "lev.amount", &[]),
            max_gain_db: read_f32(session, "lev.maxgain", &[]),
            gate_db: read_f32(session, "lev.gate", &[]),
            detector_mask: lev_masks & 0xFF,
            apply_mask: (lev_masks >> 8) & 0xFF,
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
            output_mask: read_mask(session, "bass.mask", defaults.3.output_mask),
        }),
        upmix: has("upmixer").then(|| UpmixBlock {
            enabled: read_bool(session, "up.on"),
            center_mode: read_u8(session, "up.center_mode") as i32,
            surround_mode: read_u8(session, "up.surround_mode") as i32,
            strength_pct: read_f32(session, "up.strength", &[]),
            center_width_pct: read_f32(session, "up.width", &[]),
            threshold_pct: read_f32(session, "up.threshold", &[]),
            attack_ms: read_f32(session, "up.attack", &[]),
            release_ms: read_f32(session, "up.release", &[]),
            detector_hpf_hz: read_f32(session, "up.det_hpf", &[]),
            surround_delay_ms: read_f32(session, "up.sur_delay", &[]),
            surround_hpf_hz: read_f32(session, "up.sur_hpf", &[]),
            surround_lpf_hz: read_f32(session, "up.sur_lpf", &[]),
            decorr_pct: read_f32(session, "up.decorr", &[]),
            presence_db: read_f32(session, "up.presence", &[]),
        }),
        // Every field, as the Console exports them (PresetDocumentTransfer.
        // swift:100-137).
        subharm: has("subharmonic_synth").then(|| SubharmBlock {
            enabled: read_bool(session, "sub.on"),
            low_db: read_f32(session, "sub.low", &[]),
            high_db: read_f32(session, "sub.high", &[]),
            top_db: read_f32(session, "sub.top", &[]),
            boost_db: read_f32(session, "sub.boost", &[]),
            output_mask: read_mask(session, "sub.mask", 0xFFFF),
            select_mode: read_u8(session, "sub.select") as i32,
            select_depth_pct: read_f32(session, "sub.depth", &[]),
            select_hold_ms: read_f32(session, "sub.hold", &[]),
            ceiling_db: read_f32(session, "sub.ceiling", &[]),
            link_pairs: read_bool(session, "sub.link"),
        }),
        tube: has("tube_preamp").then(|| TubeBlock {
            enabled: read_bool(session, "tube.on"),
            output_mask: read_mask(session, "tube.mask", 0xFFFF),
            tube_type: read_u8(session, "tube.type") as i32,
            drive_db: read_f32(session, "tube.drive", &[]),
            bias_pct: read_f32(session, "tube.bias", &[]),
            asym_db: read_f32(session, "tube.asym", &[]),
            hardness_pct: read_f32(session, "tube.hardness", &[]),
            sag_pct: read_f32(session, "tube.sag", &[]),
            rectifier: read_u8(session, "tube.rectifier") as i32,
            xfmr_enabled: read_bool(session, "tube.xfmr"),
            xfmr_damping: read_f32(session, "tube.damping", &[]),
            xfmr_res_hz: read_f32(session, "tube.resonance", &[]),
            mix_pct: read_f32(session, "tube.mix", &[]),
            trim_db: read_f32(session, "tube.trim", &[]),
        }),
        channels,
        matrix,
        io,
    }
}

/// The current time as the ISO-8601 UTC string the Console writes
/// (`ISO8601DateFormatter`, PresetDocumentTransfer.swift:25), such as
/// `2026-09-30T10:00:00Z`.
fn utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    iso8601(secs)
}

/// Seconds since the epoch as an ISO-8601 UTC timestamp, by the civil-from-
/// days algorithm (Howard Hinnant's), which needs no calendar crate.
fn iso8601(secs: i64) -> String {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
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
            subharm: None,
            tube: None,
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

    /// A channel the device does not have is reported, never moved somewhere
    /// else; the reference implementation reports rather than translating.
    #[test]
    fn a_channel_the_device_lacks_is_reported() {
        let mut d = doc();
        d.channels.push(ChannelBlock {
            // Windows id 8: S/PDIF 4 L, output 6.
            channel_id: 8,
            name: "SPDIF 4 L".into(),
            is_output: true,
            ..Default::default()
        });

        // A two-input, five-output part: output 6 does not exist there.
        let (placed, missing) = place_channels(&d, 2, 5);
        assert_eq!(placed.len(), 1);
        assert_eq!(placed[&ChannelRef::Input(0)].channel_id, 0);
        assert_eq!(missing, vec!["SPDIF 4 L"]);
    }

    #[test]
    fn a_duplicated_channel_takes_the_last_rather_than_failing() {
        let mut d = doc();
        d.channels.push(ChannelBlock {
            channel_id: 0,
            name: "USB 1 again".into(),
            delay_ms: 5.0,
            ..Default::default()
        });
        let (placed, missing) = place_channels(&d, 2, 5);
        assert_eq!(placed.len(), 1);
        assert_eq!(
            placed[&ChannelRef::Input(0)].delay_ms,
            5.0,
            "the last block should win"
        );
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
            firmware_version: dspi_proto::packets::FirmwareVersion::new(1, 1, 6, 0),
            build_info: None,
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
            subharm: None,
            tube: None,
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

/// The beta4 blocks: subharm and tube at the top level, and a limiter inside
/// each output's channel entry (PresetDocument.swift:45-47, 235-432).
#[cfg(test)]
mod beta4_tests {
    use super::*;
    use crate::probe::{Capabilities, ChannelInfo, Feature};
    use dspi_proto::Platform;
    use dspi_proto::generated::opcodes as op;
    use dspi_transport::MockTransport;
    use dspi_transport::mock::{Direction, LogHandle};

    fn caps(features: &[&str]) -> Capabilities {
        Capabilities {
            serial: "TEST".into(),
            platform: Platform::Rp2350,
            firmware: "1.1.6 beta 4".into(),
            firmware_version: dspi_proto::packets::FirmwareVersion::new(1, 1, 6, 4),
            build_info: None,
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
            features: features
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

    const ALL: &[&str] = &["subharmonic_synth", "tube_preamp", "output_limiter"];

    /// A device that answers everything, so the apply's readbacks never wait
    /// out a stall.
    fn rig(features: &[&str]) -> (Session, LogHandle) {
        let t = MockTransport::new().answering_everything(vec![0; 64]);
        let log = t.log_handle();
        (Session::new(Box::new(t), caps(features)).unwrap(), log)
    }

    /// `(opcode, wValue, payload)` of every write, in order.
    fn writes(log: &LogHandle, opcodes: &[u8]) -> Vec<(u8, u16, Vec<u8>)> {
        log.lock()
            .unwrap()
            .iter()
            .filter(|e| e.direction == Direction::Out && opcodes.contains(&e.opcode))
            .map(|e| (e.opcode, e.value, e.payload.clone()))
            .collect()
    }

    fn doc() -> PresetDocument {
        let mut d = parse(r#"{"schemaVersion":1,"channels":[{"channelId":0}]}"#).unwrap();
        d.subharm = Some(SubharmBlock {
            enabled: true,
            low_db: -6.0,
            high_db: -6.0,
            top_db: 0.0,
            boost_db: 3.0,
            output_mask: 0x0100,
            select_mode: 1,
            select_depth_pct: 80.0,
            select_hold_ms: 200.0,
            ceiling_db: -10.0,
            link_pairs: false,
        });
        d.tube = Some(TubeBlock {
            enabled: true,
            tube_type: 0,
            bias_pct: 20.0,
            ..Default::default()
        });
        d.channels.push(ChannelBlock {
            channel_id: channel_id::for_output(0),
            name: "Main L".into(),
            is_output: true,
            limiter: Some(LimiterBlock {
                enabled: true,
                threshold_db: -3.0,
                release_ms: 200.0,
                link_group: 1,
            }),
            ..Default::default()
        });
        d
    }

    #[test]
    fn the_new_blocks_round_trip() {
        let original = doc();
        let text = write(&original);
        assert_eq!(parse(&text).unwrap(), original);
    }

    /// Exactly the Console's key names; anything else is a file the Console
    /// reads as defaults.
    #[test]
    fn the_new_blocks_use_the_consoles_keys() {
        let text = write(&doc());
        for key in [
            "\"subharm\"",
            "\"lowDb\"",
            "\"highDb\"",
            "\"topDb\"",
            "\"boostDb\"",
            "\"selectMode\"",
            "\"selectDepthPct\"",
            "\"selectHoldMs\"",
            "\"ceilingDb\"",
            "\"linkPairs\"",
            "\"tube\"",
            "\"tubeType\"",
            "\"driveDb\"",
            "\"biasPct\"",
            "\"asymDb\"",
            "\"hardnessPct\"",
            "\"sagPct\"",
            "\"rectifier\"",
            "\"xfmrEnabled\"",
            "\"xfmrDamping\"",
            "\"xfmrResHz\"",
            "\"mixPct\"",
            "\"trimDb\"",
            "\"limiter\"",
            "\"thresholdDb\"",
            "\"releaseMs\"",
            "\"linkGroup\"",
        ] {
            assert!(text.contains(key), "missing {key} in:\n{text}");
        }
        assert!(!text.contains("outputIndex"), "this build writes none");
        // Masks and enums are raw numbers (PresetDocument.swift:245-246, 279).
        assert!(text.contains("\"outputMask\": 256"), "{text}");
        assert!(text.contains("\"selectMode\": 1"), "{text}");
    }

    /// Absent means the source device lacked the feature, and an input never
    /// carries a limiter.
    #[test]
    fn absent_blocks_stay_absent() {
        let d = parse(r#"{"schemaVersion":1,"channels":[{"channelId":0}]}"#).unwrap();
        assert!(d.subharm.is_none() && d.tube.is_none());
        assert!(d.channels[0].limiter.is_none());
        let text = write(&d);
        assert!(!text.contains("subharm") && !text.contains("tube"));
        assert!(!text.contains("limiter"));
    }

    /// A document as the Console writes one: JSONEncoder, pretty printed with
    /// sorted keys (PresetDocument.swift:705-711), from `capture`
    /// (PresetDocumentTransfer.swift:20-163), on an RP2350 at wire V32. The
    /// shape was built by hand from the Swift encoder, since no exported
    /// sample is checked in.
    const CONSOLE_DOCUMENT: &str = r#"{
  "channels" : [
    {
      "channelId" : 0,
      "crossover" : [

      ],
      "delayMs" : 0,
      "enabled" : true,
      "eq" : [
        {
          "bypass" : false,
          "freqHz" : 105,
          "gain" : 6.5,
          "q" : 0.707,
          "qp" : 0.707,
          "type" : 2
        }
      ],
      "eqChannel" : 0,
      "gainDb" : 0,
      "inputIndex" : 0,
      "isOutput" : false,
      "muted" : false,
      "name" : "USB L"
    },
    {
      "channelId" : 3,
      "crossover" : [

      ],
      "delayMs" : 0,
      "enabled" : true,
      "eq" : [

      ],
      "eqChannel" : 9,
      "gainDb" : -3,
      "isOutput" : true,
      "limiter" : {
        "enabled" : true,
        "linkGroup" : 2,
        "releaseMs" : 250,
        "thresholdDb" : -4.5
      },
      "muted" : false,
      "name" : "SPDIF 1 R",
      "outputDelayMs" : 1.5,
      "outputIndex" : 1
    }
  ],
  "crossfeed" : {
    "enabled" : false,
    "feedDb" : 4.5,
    "freqHz" : 700,
    "itd" : true,
    "outputPairMask" : 1,
    "preset" : 0
  },
  "global" : {
    "bypass" : false,
    "inputPairLinked" : [
      false,
      false,
      false,
      false
    ],
    "inputPreampsDb" : [
      0,
      0,
      0,
      0,
      0,
      0,
      0,
      0
    ],
    "inputSource" : 0,
    "lgSoundSyncEnabled" : false,
    "masterVolumeDb" : -20,
    "userVolumeDb" : -12
  },
  "io" : {
    "adatEnabled" : false,
    "adatInputClockMode" : 0,
    "adatInputEnabled" : false,
    "adatInputPin" : 255,
    "adatPin" : 12,
    "i2sBckPin" : 14,
    "i2sBckPinSlave" : 26,
    "i2sClockMode" : 0,
    "i2sClockPinMode" : 0,
    "i2sInputChannels" : 2,
    "i2sInputRateHz" : 48000,
    "i2sRxPins" : [
      1,
      2,
      3,
      4
    ],
    "mckEnabled" : false,
    "mckMultiplier" : 128,
    "mckPin" : 13,
    "outputPins" : [
      6,
      7,
      8,
      9,
      10
    ],
    "outputSlotTypes" : [
      0,
      0,
      0,
      0
    ],
    "spdifEnabledExt" : 0,
    "spdifRxPins" : [
      5,
      20,
      21
    ]
  },
  "leveller" : {
    "amountPct" : 50,
    "applyMask" : 255,
    "detectorMask" : 255,
    "enabled" : false,
    "gateDb" : -96,
    "lookahead" : true,
    "maxGainDb" : 15,
    "speed" : 0
  },
  "loudness" : {
    "enabled" : false,
    "intensityPct" : 100,
    "outputMask" : 65535,
    "refSpl" : 83
  },
  "matrix" : [
    {
      "enabled" : true,
      "gainDb" : 0,
      "input" : 0,
      "invert" : false,
      "output" : 0
    }
  ],
  "meta" : {
    "appVersion" : "1.1.6-beta4",
    "firmwareVersion" : "1.1.6 beta 4",
    "inputChannelCount" : 8,
    "masterVolumeMode" : 0,
    "name" : "Living Room",
    "outputChannelCount" : 9,
    "outputConfigMode" : 1,
    "platform" : "RP2350",
    "savedUtc" : "2026-09-29T10:00:00Z",
    "wireFormatVersion" : 32
  },
  "psybass" : {
    "characterPct" : 50,
    "cutoffHz" : 80,
    "driveDb" : 6,
    "enabled" : false,
    "harmonicsDb" : 0,
    "originalDb" : 0,
    "outputMask" : 65535
  },
  "schemaVersion" : 1,
  "subharm" : {
    "boostDb" : 3,
    "ceilingDb" : -12,
    "enabled" : true,
    "highDb" : -6,
    "linkPairs" : true,
    "lowDb" : -6,
    "outputMask" : 256,
    "selectDepthPct" : 100,
    "selectHoldMs" : 150,
    "selectMode" : 2,
    "topDb" : -30
  },
  "tube" : {
    "asymDb" : 2,
    "biasPct" : 5,
    "driveDb" : -6,
    "enabled" : true,
    "hardnessPct" : 55,
    "mixPct" : 80,
    "outputMask" : 255,
    "rectifier" : 2,
    "sagPct" : 10,
    "trimDb" : -1.5,
    "tubeType" : 3,
    "xfmrDamping" : 4,
    "xfmrEnabled" : false,
    "xfmrResHz" : 80
  },
  "upmix" : {
    "attackMs" : 10,
    "centerMode" : 1,
    "centerWidthPct" : 25,
    "decorrPct" : 90,
    "detectorHpfHz" : 200,
    "enabled" : false,
    "presenceDb" : 0,
    "releaseMs" : 100,
    "strengthPct" : 100,
    "surroundDelayMs" : 12,
    "surroundHpfHz" : 300,
    "surroundLpfHz" : 7000,
    "surroundMode" : 1,
    "thresholdPct" : 30
  }
}"#;

    #[test]
    fn a_document_in_the_consoles_format_parses() {
        let d = parse(CONSOLE_DOCUMENT).unwrap();
        assert_eq!(d.meta.wire_format_version, 32);
        assert_eq!(
            d.subharm,
            Some(SubharmBlock {
                enabled: true,
                low_db: -6.0,
                high_db: -6.0,
                top_db: -30.0,
                boost_db: 3.0,
                output_mask: 0x0100,
                select_mode: 2,
                select_depth_pct: 100.0,
                select_hold_ms: 150.0,
                ceiling_db: -12.0,
                link_pairs: true,
            })
        );
        assert_eq!(
            d.tube,
            Some(TubeBlock {
                enabled: true,
                output_mask: 0xFF,
                tube_type: 3,
                drive_db: -6.0,
                bias_pct: 5.0,
                asym_db: 2.0,
                hardness_pct: 55.0,
                sag_pct: 10.0,
                rectifier: 2,
                xfmr_enabled: false,
                xfmr_damping: 4.0,
                xfmr_res_hz: 80.0,
                mix_pct: 80.0,
                trim_db: -1.5,
            })
        );
        assert!(d.channels[0].limiter.is_none(), "inputs carry none");
        let out = &d.channels[1];
        assert_eq!(out.output_index, Some(1));
        assert_eq!(
            out.limiter,
            Some(LimiterBlock {
                enabled: true,
                threshold_db: -4.5,
                release_ms: 250.0,
                link_group: 2,
            })
        );
    }

    /// A Console before V30 wrote a two-band subharm block, and a hand-edited
    /// file may drop any key: the rest take the firmware's defaults, as the
    /// Console's lenient decoder gives them (PresetDocument.swift:252-266).
    #[test]
    fn a_partial_block_takes_the_firmware_defaults() {
        let d = parse(
            r#"{"schemaVersion":1,
                "subharm":{"enabled":true,"lowDb":-3,"highDb":0,"boostDb":2,"outputMask":256},
                "tube":{"enabled":true},
                "channels":[{"channelId":8,"isOutput":true,"limiter":{"enabled":true}}]}"#,
        )
        .unwrap();
        let s = d.subharm.unwrap();
        assert_eq!(s.top_db, -30.0, "the third band ships off");
        assert_eq!(
            (s.select_mode, s.select_depth_pct, s.select_hold_ms),
            (0, 100.0, 150.0)
        );
        assert_eq!(s.ceiling_db, 0.0);
        assert!(s.link_pairs);
        let t = d.tube.unwrap();
        assert_eq!(
            t,
            TubeBlock {
                enabled: true,
                ..Default::default()
            }
        );
        assert_eq!((t.tube_type, t.drive_db, t.xfmr_res_hz), (1, -12.0, 95.0));
        assert!(t.xfmr_enabled);
        let l = d.channels[0].limiter.clone().unwrap();
        assert_eq!(
            (l.threshold_db, l.release_ms, l.link_group),
            (-1.0, 100.0, 0)
        );
    }

    /// Every value first and the switch last, and the tube type before the
    /// four values it loads (PresetDocumentTransfer.swift:600-644).
    #[test]
    fn subharm_and_tube_apply_in_the_consoles_order() {
        let (mut s, log) = rig(ALL);
        let report = apply(&mut s, &doc(), ApplyOptions::default());
        let subharm = [
            op::REQ_SET_SUBHARM_LOW,
            op::REQ_SET_SUBHARM_HIGH,
            op::REQ_SET_SUBHARM_BOOST,
            op::REQ_SET_SUBHARM_MASK,
            op::REQ_SET_SUBHARM_TOP,
            op::REQ_SET_SUBHARM_SELECT,
            op::REQ_SET_SUBHARM_DEPTH,
            op::REQ_SET_SUBHARM_HOLD,
            op::REQ_SET_SUBHARM_CEILING,
            op::REQ_SET_SUBHARM_LINK,
            op::REQ_SET_SUBHARM,
        ];
        let sent: Vec<u8> = writes(&log, &subharm).iter().map(|w| w.0).collect();
        assert_eq!(sent, subharm.to_vec());
        let tube: Vec<u16> = writes(&log, &[op::REQ_SET_TUBE_PARAM])
            .iter()
            .map(|w| w.1)
            .collect();
        // TUBE_PARAM_* (tube.h:17-30): type, bias, asym, hardness, sag,
        // drive, rectifier, damping, resonance, output stage, mix, trim,
        // mask, enable.
        assert_eq!(tube, vec![2, 4, 5, 6, 7, 3, 8, 10, 11, 9, 12, 13, 1, 0]);
        // Every tube value is a float on the wire, the mask included.
        let mask = writes(&log, &[op::REQ_SET_TUBE_PARAM])[12].2.clone();
        assert_eq!(mask, 65535.0f32.to_le_bytes().to_vec());
        assert!(
            !report.skipped.iter().any(|r| r.contains("not supported")),
            "{:?}",
            report.skipped
        );
    }

    /// The Console's skip reasons (PresetDocumentTransfer.swift:435, 618,
    /// 643), and nothing written for a feature the device lacks.
    #[test]
    fn a_missing_feature_is_skipped_with_the_consoles_reason() {
        let (mut s, log) = rig(&[]);
        let report = apply(
            &mut s,
            &doc(),
            ApplyOptions {
                hardware_io: true,
                ..Default::default()
            },
        );
        for reason in [
            "Subharmonic synthesizer (not supported by this firmware)",
            "Tube preamp (not supported by this firmware)",
            "Output limiter (not supported by this firmware)",
        ] {
            assert!(
                report.skipped.iter().any(|r| r == reason),
                "{reason} not in {:?}",
                report.skipped
            );
        }
        let touched = writes(
            &log,
            &[op::REQ_SET_SUBHARM, op::REQ_SET_TUBE_PARAM, op::REQ_LIMITER],
        );
        assert!(touched.is_empty(), "{touched:?}");
    }

    /// Limiters are output configuration: only the hardware I/O option brings
    /// them in (PresetDocumentTransfer.swift:246-250, 323-326).
    #[test]
    fn limiters_apply_only_with_the_hardware_option() {
        let (mut s, log) = rig(ALL);
        apply(&mut s, &doc(), ApplyOptions::default());
        assert!(writes(&log, &[op::REQ_LIMITER]).is_empty());

        let (mut s, log) = rig(ALL);
        apply(
            &mut s,
            &doc(),
            ApplyOptions {
                hardware_io: true,
                ..Default::default()
            },
        );
        let w = writes(&log, &[op::REQ_LIMITER]);
        // Output 0 (Windows id 2): threshold, release, enable, then the group.
        let values: Vec<u16> = w.iter().map(|w| w.1).collect();
        assert_eq!(values, vec![0x0001, 0x0002, 0x0000, 0x0003]);
        assert_eq!(w[0].2, (-3.0f32).to_le_bytes().to_vec());
        assert_eq!(w[3].2, 1.0f32.to_le_bytes().to_vec());
    }

    /// A linked output is unlinked before its values go out, so one write
    /// does not move its whole group (limiter.c:143-194), and relinked last.
    /// The Console's own output index places the block.
    #[test]
    fn a_linked_output_is_unlinked_first_and_relinked_last() {
        let t = MockTransport::new()
            .answering_everything(vec![0; 64])
            // Every output reads as linked to group 2.
            .data(op::REQ_LIMITER, 2.0f32.to_le_bytes().to_vec());
        let log = t.log_handle();
        let mut s = Session::new(Box::new(t), caps(ALL)).unwrap();
        let d = parse(CONSOLE_DOCUMENT).unwrap();
        apply(
            &mut s,
            &d,
            ApplyOptions {
                hardware_io: true,
                ..Default::default()
            },
        );
        let w = writes(&log, &[op::REQ_LIMITER]);
        let values: Vec<u16> = w.iter().map(|w| w.1).collect();
        // Output 1, from `outputIndex` rather than `channelId` 3.
        assert_eq!(values, vec![0x0103, 0x0101, 0x0102, 0x0100, 0x0103]);
        assert_eq!(w[0].2, 0.0f32.to_le_bytes().to_vec(), "unlinked");
        assert_eq!(w[4].2, 2.0f32.to_le_bytes().to_vec(), "relinked");
    }

    /// An export carries the blocks the device has, and reads back as the
    /// same document.
    #[test]
    fn an_export_carries_the_new_blocks_and_round_trips() {
        let (mut s, _) = rig(ALL);
        let d = capture(&mut s, Some("Test".into()));
        assert!(d.subharm.is_some() && d.tube.is_some());
        for c in &d.channels {
            assert_eq!(c.limiter.is_some(), c.is_output, "channel {}", c.channel_id);
        }
        assert_eq!(parse(&write(&d)).unwrap(), d);

        let (mut s, _) = rig(&[]);
        let d = capture(&mut s, None);
        assert!(d.subharm.is_none() && d.tube.is_none());
        assert!(d.channels.iter().all(|c| c.limiter.is_none()));
    }
}

/// Interchange with the Consoles: the channel numbering on both platforms,
/// the fields the Console carries, and files from older Terminal builds
/// (PresetDocument.swift:615-684, PresetDocumentTransfer.swift:20-667).
#[cfg(test)]
mod interop_tests {
    use super::*;
    use crate::probe::{Capabilities, ChannelInfo, Feature};
    use dspi_proto::Platform;
    use dspi_proto::generated::opcodes as op;
    use dspi_transport::MockTransport;
    use dspi_transport::mock::{Direction, LogHandle};

    /// A device of either shape: RP2350 has eight inputs and nine outputs
    /// (config.h:773-774), RP2040 two and five (config.h:776-777).
    fn caps(platform: Platform, features: &[&str]) -> Capabilities {
        let (inputs, outputs) = match platform {
            Platform::Rp2040 => (2u8, 5u8),
            _ => (8, 9),
        };
        let n = inputs + outputs;
        Capabilities {
            serial: "TEST".into(),
            platform,
            firmware: "1.1.6 beta 4".into(),
            firmware_version: dspi_proto::packets::FirmwareVersion::new(1, 1, 6, 4),
            build_info: None,
            wire_format: dspi_proto::generated::wire::WIRE_FORMAT_VERSION as u8,
            num_channels: n,
            num_inputs: inputs,
            num_outputs: outputs,
            max_bands: 10,
            band_storage: 12,
            channels: (0..n)
                .map(|i| ChannelInfo {
                    index: i,
                    name: format!("Ch {i}"),
                    slug: format!("ch.{i}"),
                    is_output: i >= inputs,
                })
                .collect(),
            features: features
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

    const FEATURES: &[&str] = &[
        "loudness_output_mask",
        "crossfeed_output_mask",
        "leveller_masks",
        "psychoacoustic_bass",
        "upmixer",
        "lg_sound_sync",
        "subharmonic_synth",
        "tube_preamp",
        "output_limiter",
    ];

    fn rig_with(t: MockTransport, platform: Platform, features: &[&str]) -> (Session, LogHandle) {
        let log = t.log_handle();
        (
            Session::new(Box::new(t), caps(platform, features)).unwrap(),
            log,
        )
    }

    fn rig(platform: Platform) -> (Session, LogHandle) {
        rig_with(
            MockTransport::new().answering_everything(vec![0; 64]),
            platform,
            FEATURES,
        )
    }

    /// `(wValue, payload)` of every OUT transfer with this opcode, in order.
    fn sent(log: &LogHandle, opcode: u8) -> Vec<(u16, Vec<u8>)> {
        log.lock()
            .unwrap()
            .iter()
            .filter(|e| e.direction == Direction::Out && e.opcode == opcode)
            .map(|e| (e.value, e.payload.clone()))
            .collect()
    }

    /// The unified channels that received an EQ band with this frequency.
    fn eq_channels_at(log: &LogHandle, freq: f32) -> Vec<u8> {
        let mut chs: Vec<u8> = sent(log, op::REQ_SET_EQ_PARAM)
            .iter()
            .filter(|(_, p)| p[4..8] == freq.to_le_bytes())
            .map(|(_, p)| p[0])
            .collect();
        chs.dedup();
        chs
    }

    // ------------------------------------------------------------ the table

    /// PresetDocumentTests.swift:195-240, both directions and the fixed
    /// points.
    #[test]
    fn the_channel_ids_are_the_windows_consoles() {
        for i in 0..8 {
            assert_eq!(
                channel_id::resolve(channel_id::for_input(i)),
                Some(ChannelRef::Input(i))
            );
        }
        for o in 0..9 {
            assert_eq!(
                channel_id::resolve(channel_id::for_output(o)),
                Some(ChannelRef::Output(o))
            );
        }
        assert_eq!(channel_id::for_input(0), 0);
        assert_eq!(channel_id::for_input(1), 1);
        assert_eq!(channel_id::for_input(2), 11);
        assert_eq!(channel_id::for_input(7), 16);
        assert_eq!(channel_id::for_output(0), 2);
        assert_eq!(channel_id::for_output(4), 6, "RP2040's PDM");
        assert_eq!(channel_id::for_output(8), 10, "RP2350's PDM");
        assert_eq!(channel_id::resolve(-1), None);
    }

    /// The Console's own index fields win over the shared id
    /// (PresetDocumentTests.swift:244-253).
    #[test]
    fn the_consoles_index_fields_win_over_the_shared_id() {
        let mut b = ChannelBlock {
            channel_id: 2,
            is_output: true,
            output_index: Some(5),
            ..Default::default()
        };
        assert_eq!(b.placement(None), Some(ChannelRef::Output(5)));
        b.output_index = None;
        assert_eq!(b.placement(None), Some(ChannelRef::Output(0)));
        // An input's own index wins too, and an output index on an input
        // entry is not read.
        let b = ChannelBlock {
            channel_id: 11,
            input_index: Some(3),
            output_index: Some(1),
            ..Default::default()
        };
        assert_eq!(b.placement(None), Some(ChannelRef::Input(3)));
    }

    // ------------------------------------------------- Console-written files

    /// An RP2350 document as the Console writes one: the Windows id, the
    /// unified `eqChannel`, and its own index (PresetDocumentTransfer.swift:
    /// 166-197). The ids and the unified numbers differ for every channel but
    /// the first two, so a placement by either would show here.
    const CONSOLE_RP2350: &str = r#"{
      "schemaVersion": 1,
      "meta": {"platform": "RP2350", "savedUtc": "2026-09-29T10:00:00Z",
               "inputChannelCount": 8, "outputChannelCount": 9, "wireFormatVersion": 32},
      "channels": [
        {"channelId": 0, "eqChannel": 0, "inputIndex": 0, "isOutput": false, "name": "USB 1",
         "delayMs": 0, "eq": [{"type": 1, "freqHz": 100, "q": 1, "gain": 3}], "crossover": []},
        {"channelId": 11, "eqChannel": 2, "inputIndex": 2, "isOutput": false, "name": "USB 3",
         "delayMs": 1, "eq": [{"type": 1, "freqHz": 102, "q": 1, "gain": 3}], "crossover": []},
        {"channelId": 16, "eqChannel": 7, "inputIndex": 7, "isOutput": false, "name": "USB 8",
         "delayMs": 0, "eq": [{"type": 1, "freqHz": 107, "q": 1, "gain": 3}], "crossover": []},
        {"channelId": 2, "eqChannel": 8, "outputIndex": 0, "isOutput": true, "name": "SPDIF 1 L",
         "gainDb": -2, "muted": false, "enabled": true, "outputDelayMs": 1.5, "delayMs": 0,
         "eq": [{"type": 1, "freqHz": 200, "q": 1, "gain": 3}],
         "crossover": [{"type": 34, "freqHz": 80, "q": 0.707, "gain": 0}]},
        {"channelId": 10, "eqChannel": 16, "outputIndex": 8, "isOutput": true, "name": "PDM",
         "gainDb": -4, "muted": false, "enabled": true, "outputDelayMs": 0, "delayMs": 0,
         "eq": [{"type": 1, "freqHz": 208, "q": 1, "gain": 3}], "crossover": []}
      ]
    }"#;

    /// The same channels as a Windows file carries them: the shared id and
    /// nothing else (PresetDocumentTests.swift:260-323).
    const WINDOWS_RP2350: &str = r#"{
      "schemaVersion": 1,
      "meta": {"platform": "RP2350", "savedUtc": "2026-07-14T09:31:07.4821563+00:00",
               "inputChannelCount": 8, "outputChannelCount": 9},
      "channels": [
        {"channelId": 0, "name": "Master L", "isOutput": false, "eq": [{"type": 1, "freqHz": 100}]},
        {"channelId": 11, "name": "Input 3", "isOutput": false, "eq": [{"type": 1, "freqHz": 102}]},
        {"channelId": 16, "name": "Input 8", "isOutput": false, "eq": [{"type": 1, "freqHz": 107}]},
        {"channelId": 2, "name": "SPDIF 1 L", "isOutput": true, "gainDb": -2,
         "eq": [{"type": 1, "freqHz": 200}]},
        {"channelId": 10, "name": "PDM", "isOutput": true, "gainDb": -4,
         "eq": [{"type": 1, "freqHz": 208}]}
      ]
    }"#;

    /// An RP2040 document as the Console writes one. Here the id and the
    /// unified index coincide, and PDM is the fifth output at id 6.
    const CONSOLE_RP2040: &str = r#"{
      "schemaVersion": 1,
      "meta": {"platform": "RP2040", "savedUtc": "2026-09-29T10:00:00Z",
               "inputChannelCount": 2, "outputChannelCount": 5},
      "channels": [
        {"channelId": 0, "eqChannel": 0, "inputIndex": 0, "isOutput": false, "name": "USB L",
         "eq": [{"type": 1, "freqHz": 100}]},
        {"channelId": 1, "eqChannel": 1, "inputIndex": 1, "isOutput": false, "name": "USB R",
         "eq": [{"type": 1, "freqHz": 101}]},
        {"channelId": 2, "eqChannel": 2, "outputIndex": 0, "isOutput": true, "name": "SPDIF 1 L",
         "gainDb": -2, "eq": [{"type": 1, "freqHz": 200}]},
        {"channelId": 6, "eqChannel": 6, "outputIndex": 4, "isOutput": true, "name": "PDM",
         "gainDb": -4, "eq": [{"type": 1, "freqHz": 204}]}
      ]
    }"#;

    fn places(text: &str, inputs: u8, outputs: u8) -> Vec<(ChannelRef, String)> {
        let doc = parse(text).unwrap();
        let (placed, missing) = place_channels(&doc, inputs, outputs);
        assert!(missing.is_empty(), "{missing:?}");
        placed.iter().map(|(r, c)| (*r, c.name.clone())).collect()
    }

    #[test]
    fn a_console_document_places_every_channel_on_rp2350() {
        use ChannelRef::*;
        for text in [CONSOLE_RP2350, WINDOWS_RP2350] {
            let got: Vec<ChannelRef> = places(text, 8, 9).into_iter().map(|p| p.0).collect();
            assert_eq!(
                got,
                vec![Input(0), Input(2), Input(7), Output(0), Output(8)],
                "{text}"
            );
        }
    }

    #[test]
    fn a_console_document_places_every_channel_on_rp2040() {
        use ChannelRef::*;
        let got: Vec<ChannelRef> = places(CONSOLE_RP2040, 2, 5)
            .into_iter()
            .map(|p| p.0)
            .collect();
        assert_eq!(got, vec![Input(0), Input(1), Output(0), Output(4)]);
    }

    /// Applied to an RP2350, each channel's EQ reaches its unified channel
    /// (config.h:788-790) and each output strip its output index.
    #[test]
    fn a_console_document_applies_to_the_right_channels_on_rp2350() {
        for text in [CONSOLE_RP2350, WINDOWS_RP2350] {
            let (mut s, log) = rig(Platform::Rp2350);
            let report = apply(&mut s, &parse(text).unwrap(), ApplyOptions::default());
            assert!(report.missing_channels.is_empty());
            assert_eq!(report.channels_applied, 5);
            for (freq, unified) in [(100.0, 0), (102.0, 2), (107.0, 7), (200.0, 8), (208.0, 16)] {
                assert_eq!(eq_channels_at(&log, freq), vec![unified], "{freq} Hz");
            }
            let gains: Vec<(u16, Vec<u8>)> = sent(&log, op::REQ_SET_OUTPUT_GAIN);
            assert_eq!(
                gains,
                vec![
                    (0, (-2.0f32).to_le_bytes().to_vec()),
                    (8, (-4.0f32).to_le_bytes().to_vec())
                ]
            );
        }
    }

    #[test]
    fn a_console_document_applies_to_the_right_channels_on_rp2040() {
        let (mut s, log) = rig(Platform::Rp2040);
        let report = apply(
            &mut s,
            &parse(CONSOLE_RP2040).unwrap(),
            ApplyOptions::default(),
        );
        assert!(report.missing_channels.is_empty());
        for (freq, unified) in [(100.0, 0), (101.0, 1), (200.0, 2), (204.0, 6)] {
            assert_eq!(eq_channels_at(&log, freq), vec![unified], "{freq} Hz");
        }
        let gains: Vec<u16> = sent(&log, op::REQ_SET_OUTPUT_GAIN)
            .iter()
            .map(|g| g.0)
            .collect();
        assert_eq!(gains, vec![0, 4]);
    }

    /// An eight-input document on a two-input part reports what the part
    /// lacks rather than folding it onto what it has.
    #[test]
    fn an_rp2350_document_on_an_rp2040_reports_what_is_missing() {
        let doc = parse(CONSOLE_RP2350).unwrap();
        let (placed, missing) = place_channels(&doc, 2, 5);
        assert_eq!(
            placed.keys().copied().collect::<Vec<_>>(),
            vec![ChannelRef::Input(0), ChannelRef::Output(0)]
        );
        assert_eq!(missing, vec!["USB 3", "USB 8", "PDM"]);
    }

    /// The post-matrix output delay and the channel name travel too.
    #[test]
    fn the_output_delay_and_names_are_applied() {
        let (mut s, log) = rig(Platform::Rp2350);
        apply(
            &mut s,
            &parse(CONSOLE_RP2350).unwrap(),
            ApplyOptions::default(),
        );
        let delays = sent(&log, op::REQ_SET_OUTPUT_DELAY);
        assert_eq!(delays[0], (0, 1.5f32.to_le_bytes().to_vec()));
        let names: Vec<(u16, String)> = sent(&log, op::REQ_SET_CHANNEL_NAME)
            .into_iter()
            .map(|(v, p)| (v, String::from_utf8_lossy(&p).trim_end_matches('\0').into()))
            .collect();
        assert!(names.contains(&(2, "USB 3".into())), "{names:?}");
        assert!(names.contains(&(16, "PDM".into())), "{names:?}");
    }

    /// Bands a document carries replace the channel's; the rest of the bank
    /// goes flat, and an empty list leaves the channel alone
    /// (PresetDocumentTransfer.swift:474-502).
    #[test]
    fn a_partial_bank_flattens_the_rest_and_an_empty_one_is_left_alone() {
        let (mut s, log) = rig(Platform::Rp2350);
        apply(
            &mut s,
            &parse(CONSOLE_RP2350).unwrap(),
            ApplyOptions::default(),
        );
        let eq = sent(&log, op::REQ_SET_EQ_PARAM);
        let on = |ch: u8| eq.iter().filter(|(_, p)| p[0] == ch).count();
        // Ten EQ bands on input 0; ten plus four crossover on output 0.
        assert_eq!(on(0), 10);
        assert_eq!(on(8), 14);
        // Output 8 carries no crossover, so only its EQ is written.
        assert_eq!(on(16), 10);
        // Input 1 is not in the document at all.
        assert_eq!(on(1), 0);
    }

    // ---------------------------------------------- the fields it now carries

    /// The masks and the upmix modes, each where the firmware has one.
    #[test]
    fn masks_and_upmix_modes_are_applied() {
        let mut doc = parse(CONSOLE_RP2350).unwrap();
        doc.loudness.output_mask = 0x0003;
        doc.crossfeed.output_pair_mask = 0x02;
        doc.leveller.detector_mask = 0x03;
        doc.leveller.apply_mask = 0x02;
        doc.psybass = Some(PsybassBlock {
            output_mask: 0x0100,
            ..Default::default()
        });
        doc.upmix = Some(UpmixBlock {
            center_mode: 0,
            surround_mode: 1,
            threshold_pct: 40.0,
            surround_lpf_hz: 6000.0,
            ..Default::default()
        });
        doc.global.lg_sound_sync_enabled = true;

        let (mut s, log) = rig(Platform::Rp2350);
        let report = apply(&mut s, &doc, ApplyOptions::default());
        assert!(
            !report.skipped.iter().any(|r| r.contains("not supported")),
            "{:?}",
            report.skipped
        );

        assert_eq!(sent(&log, op::REQ_SET_LOUDNESS_MASK)[0].1, vec![0x03, 0x00]);
        assert_eq!(sent(&log, op::REQ_SET_CROSSFEED_OUTPUTS)[0].1, vec![0x02]);
        assert_eq!(
            sent(&log, op::REQ_SET_LEVELLER_MASKS)[0].1,
            vec![0x03, 0x02]
        );
        assert_eq!(sent(&log, op::REQ_SET_PSYBASS_MASK)[0].1, vec![0x00, 0x01]);
        assert_eq!(sent(&log, op::REQ_SET_LG_SOUND_SYNC_ENABLE)[0].1, vec![1]);

        // UPMIX_PARAM_* by wValue (upmix.h:188-201), every one a float.
        let up: Vec<(u16, Vec<u8>)> = sent(&log, op::REQ_UPMIX_SET_PARAM);
        let wvalues: Vec<u16> = up.iter().map(|u| u.0).collect();
        assert_eq!(wvalues, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 0]);
        assert_eq!(up[0].1, 0.0f32.to_le_bytes().to_vec());
        assert_eq!(up[1].1, 1.0f32.to_le_bytes().to_vec());
        assert_eq!(up[4].1, 40.0f32.to_le_bytes().to_vec());
        assert_eq!(up[10].1, 6000.0f32.to_le_bytes().to_vec());
    }

    /// Each feature's parameters go before its switch
    /// (PresetDocumentTransfer.swift:557-584).
    #[test]
    fn every_switch_follows_its_parameters() {
        let (mut s, log) = rig(Platform::Rp2350);
        apply(
            &mut s,
            &parse(CONSOLE_RP2350).unwrap(),
            ApplyOptions::default(),
        );
        let order: Vec<u8> = log
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.direction == Direction::Out)
            .map(|e| e.opcode)
            .collect();
        let at = |o: u8| order.iter().position(|x| *x == o).unwrap();
        assert!(at(op::REQ_SET_LOUDNESS_MASK) < at(op::REQ_SET_LOUDNESS));
        assert!(at(op::REQ_SET_CROSSFEED_OUTPUTS) < at(op::REQ_SET_CROSSFEED));
        assert!(at(op::REQ_SET_LEVELLER_MASKS) < at(op::REQ_SET_LEVELLER_ENABLE));
    }

    /// A feature the device lacks is skipped in the Console's words, and a
    /// mask the firmware predates is not sent.
    #[test]
    fn missing_features_are_skipped_in_the_consoles_words() {
        let mut doc = parse(CONSOLE_RP2350).unwrap();
        doc.psybass = Some(PsybassBlock::default());
        doc.upmix = Some(UpmixBlock::default());
        doc.global.lg_sound_sync_enabled = true;
        let (mut s, log) = rig_with(
            MockTransport::new().answering_everything(vec![0; 64]),
            Platform::Rp2350,
            &[],
        );
        let report = apply(&mut s, &doc, ApplyOptions::default());
        for reason in [
            "Psychoacoustic bass (not supported by this firmware)",
            "Stereo upmixer (not supported by this device)",
            "LG Sound Sync (not supported by this firmware)",
        ] {
            assert!(report.skipped.iter().any(|r| r == reason), "{reason}");
        }
        for opcode in [
            op::REQ_SET_LOUDNESS_MASK,
            op::REQ_SET_CROSSFEED_OUTPUTS,
            op::REQ_SET_LEVELLER_MASKS,
            op::REQ_UPMIX_SET_PARAM,
        ] {
            assert!(sent(&log, opcode).is_empty(), "0x{opcode:02X}");
        }
    }

    /// A Windows file from before the masks existed takes the firmware's
    /// defaults, not zero, which would switch loudness off everywhere
    /// (PresetDocument.swift:149-185).
    #[test]
    fn absent_masks_take_the_firmware_defaults() {
        let d = parse(
            r#"{"schemaVersion":1,"loudness":{"enabled":true},"crossfeed":{"enabled":true},
                "leveller":{},"upmix":{},"psybass":{},"channels":[{"channelId":0}]}"#,
        )
        .unwrap();
        assert_eq!(d.loudness.output_mask, 0xFFFF);
        assert_eq!(
            (d.loudness.ref_spl, d.loudness.intensity_pct),
            (83.0, 100.0)
        );
        assert_eq!(d.crossfeed.output_pair_mask, 0x01);
        assert_eq!((d.crossfeed.freq_hz, d.crossfeed.feed_db), (700.0, 4.5));
        assert_eq!(
            (d.leveller.detector_mask, d.leveller.apply_mask),
            (0xFF, 0xFF)
        );
        assert_eq!(d.psybass.unwrap().output_mask, 0xFFFF);
        let u = d.upmix.unwrap();
        assert_eq!((u.center_mode, u.surround_mode), (1, 2));
        assert_eq!(u.surround_lpf_hz, 7000.0);
    }

    /// Master volume is a preset's only in with-preset mode
    /// (PresetDocumentTransfer.swift:669-680); listening volume always is.
    #[test]
    fn master_volume_follows_the_devices_mode() {
        let doc = parse(CONSOLE_RP2350).unwrap();
        let options = ApplyOptions {
            volume_levels: true,
            ..Default::default()
        };

        let (mut s, log) = rig(Platform::Rp2350);
        let report = apply(&mut s, &doc, options);
        assert!(sent(&log, op::REQ_SET_MASTER_VOLUME).is_empty());
        assert_eq!(sent(&log, op::REQ_SET_USER_VOLUME).len(), 1);
        assert!(
            report
                .skipped
                .contains(&"Master volume (device is in independent master-volume mode)".into())
        );

        let (mut s, log) = rig_with(
            MockTransport::new()
                .answering_everything(vec![0; 64])
                .data(op::REQ_GET_MASTER_VOLUME_MODE, vec![1]),
            Platform::Rp2350,
            FEATURES,
        );
        apply(&mut s, &doc, options);
        assert_eq!(sent(&log, op::REQ_SET_MASTER_VOLUME).len(), 1);
    }

    /// Disables first, then enables, so an output the document switches off
    /// is free before the one it switches on needs it
    /// (PresetDocumentTransfer.swift:377-392).
    #[test]
    fn outputs_are_disabled_before_any_is_enabled() {
        let mut doc = parse(CONSOLE_RP2350).unwrap();
        // The document turns output 0 off and PDM on; the device holds
        // output 0 on and PDM off.
        doc.channels[3].enabled = false;
        doc.channels[4].enabled = true;
        use dspi_transport::mock::Reply;
        let reads = |v: &[u8]| v.iter().map(|b| Reply::Data(vec![*b])).collect::<Vec<_>>();
        let (mut s, log) = rig_with(
            MockTransport::new()
                .answering_everything(vec![0; 64])
                // Output 0: the check, the write's own read, its readback;
                // then the same three for PDM.
                .reply(
                    op::REQ_GET_OUTPUT_ENABLE,
                    Reply::Sequence(reads(&[1, 1, 0, 0, 0, 1])),
                ),
            Platform::Rp2350,
            FEATURES,
        );
        apply(&mut s, &doc, ApplyOptions::default());
        let enables: Vec<(u16, u8)> = sent(&log, op::REQ_SET_OUTPUT_ENABLE)
            .iter()
            .map(|(v, p)| (*v, p[0]))
            .collect();
        assert_eq!(enables, vec![(0, 0), (8, 1)]);
    }

    /// An enable that still collides with Core 1 is reported, not forced.
    #[test]
    fn a_colliding_enable_is_reported() {
        let mut doc = parse(CONSOLE_RP2350).unwrap();
        doc.channels[4].enabled = true;
        let (mut s, log) = rig_with(
            MockTransport::new()
                .answering_everything(vec![0; 64])
                .data(op::REQ_GET_CORE1_CONFLICT, vec![1]),
            Platform::Rp2350,
            FEATURES,
        );
        let report = apply(&mut s, &doc, ApplyOptions::default());
        assert!(
            report
                .skipped
                .contains(&"PDM could not be enabled (conflicts with another output)".into()),
            "{:?}",
            report.skipped
        );
        assert!(
            !sent(&log, op::REQ_SET_OUTPUT_ENABLE)
                .iter()
                .any(|(_, p)| p[0] == 1)
        );
    }

    // ------------------------------------------------------------- exports

    /// An export numbers channels as the Console does and carries its keys
    /// (PresetDocumentTests.swift:337-433, PresetDocumentTransfer.swift:
    /// 166-197).
    #[test]
    fn an_rp2350_export_uses_the_consoles_numbering_and_keys() {
        let (mut s, _) = rig(Platform::Rp2350);
        let d = capture(&mut s, Some("Test".into()));

        let ids: Vec<i32> = d.channels.iter().map(|c| c.channel_id).collect();
        assert_eq!(
            ids,
            vec![0, 1, 11, 12, 13, 14, 15, 16, 2, 3, 4, 5, 6, 7, 8, 9, 10]
        );
        for (unified, c) in d.channels.iter().enumerate() {
            assert_eq!(c.eq_channel, Some(unified as i32));
            if c.is_output {
                assert_eq!(c.output_index, Some(unified as i32 - 8));
                assert_eq!(c.input_index, None);
                assert!(c.output_delay_ms.is_some());
            } else {
                assert_eq!(c.input_index, Some(unified as i32));
                assert_eq!(c.output_index, None);
            }
        }

        let json: serde_json::Value = serde_json::from_str(&write(&d)).unwrap();
        let keys = |v: &serde_json::Value| -> Vec<String> {
            v.as_object().unwrap().keys().cloned().collect()
        };
        let has_all = |v: &serde_json::Value, want: &[&str]| {
            let got = keys(v);
            for k in want {
                assert!(got.iter().any(|g| g == k), "missing {k} in {got:?}");
            }
        };
        has_all(
            &json,
            &[
                "schemaVersion",
                "meta",
                "global",
                "loudness",
                "crossfeed",
                "leveller",
                "psybass",
                "upmix",
                "subharm",
                "tube",
                "channels",
                "matrix",
                "io",
            ],
        );
        has_all(
            &json["meta"],
            &[
                "name",
                "savedUtc",
                "appVersion",
                "platform",
                "firmwareVersion",
                "wireFormatVersion",
                "inputChannelCount",
                "outputChannelCount",
                "masterVolumeMode",
                "outputConfigMode",
            ],
        );
        has_all(
            &json["upmix"],
            &[
                "enabled",
                "centerMode",
                "surroundMode",
                "strengthPct",
                "centerWidthPct",
                "thresholdPct",
                "attackMs",
                "releaseMs",
                "detectorHpfHz",
                "surroundDelayMs",
                "surroundHpfHz",
                "surroundLpfHz",
                "decorrPct",
                "presenceDb",
            ],
        );
        has_all(
            &json["channels"][2],
            &["channelId", "eqChannel", "inputIndex", "name"],
        );
        has_all(
            &json["channels"][16],
            &[
                "channelId",
                "name",
                "isOutput",
                "delayMs",
                "gainDb",
                "muted",
                "enabled",
                "eq",
                "crossover",
                "eqChannel",
                "outputIndex",
                "outputDelayMs",
                "limiter",
            ],
        );
        assert_eq!(json["channels"][16]["channelId"], 10, "PDM");
        assert_eq!(json["channels"][16]["outputIndex"], 8);
        assert_eq!(json["channels"][16]["eqChannel"], 16);
        let saved = json["meta"]["savedUtc"].as_str().unwrap();
        assert_eq!(saved.len(), 20, "{saved}");
        assert!(saved.ends_with('Z'));
    }

    #[test]
    fn an_rp2040_export_uses_the_consoles_numbering() {
        let (mut s, _) = rig(Platform::Rp2040);
        let d = capture(&mut s, None);
        let ids: Vec<(i32, Option<i32>, Option<i32>)> = d
            .channels
            .iter()
            .map(|c| (c.channel_id, c.input_index, c.output_index))
            .collect();
        assert_eq!(
            ids,
            vec![
                (0, Some(0), None),
                (1, Some(1), None),
                (2, None, Some(0)),
                (3, None, Some(1)),
                (4, None, Some(2)),
                (5, None, Some(3)),
                (6, None, Some(4)),
            ]
        );
    }

    /// An export read back lands every channel where it came from, on both
    /// platforms, whether read by its own index fields or, as a Windows
    /// reader does, by `channelId` alone.
    #[test]
    fn an_export_round_trips_on_both_platforms() {
        for (platform, inputs, outputs) in [(Platform::Rp2350, 8, 9), (Platform::Rp2040, 2, 5)] {
            let (mut s, _) = rig(platform);
            let d = parse(&write(&capture(&mut s, None))).unwrap();
            let (placed, missing) = place_channels(&d, inputs, outputs);
            assert!(missing.is_empty());
            assert_eq!(placed.len(), (inputs + outputs) as usize);
            for (r, c) in &placed {
                assert_eq!(Some(r.unified(inputs) as i32), c.eq_channel, "{platform:?}");
                // What a Windows reader sees.
                assert_eq!(channel_id::resolve(c.channel_id), Some(*r));
            }

            // Applied back, every channel's bank reaches its own channel.
            let (mut s, log) = rig(platform);
            apply(&mut s, &d, ApplyOptions::default());
            let mut chs: Vec<u8> = sent(&log, op::REQ_SET_EQ_PARAM)
                .iter()
                .map(|(_, p)| p[0])
                .collect();
            chs.dedup();
            assert_eq!(chs, (0..inputs + outputs).collect::<Vec<_>>());
        }
    }

    // --------------------------------------------- older Terminal documents

    /// An eight-input file an older Terminal build wrote: the unified index
    /// as `channelId`, no index fields, no timestamp.
    fn older_terminal_rp2350() -> String {
        let chans: Vec<String> = (0..17)
            .map(|i| {
                format!(
                    r#"{{"channelId":{i},"name":"Ch {i}","isOutput":{},"eq":[{{"type":1,"freqHz":{}}}]}}"#,
                    i >= 8,
                    300 + i
                )
            })
            .collect();
        format!(
            r#"{{"schemaVersion":1,"meta":{{"appVersion":"2.0.0-dev","platform":"RP2350",
                "inputChannelCount":8,"outputChannelCount":9}},"channels":[{}]}}"#,
            chans.join(",")
        )
    }

    #[test]
    fn an_older_terminal_document_is_recognised_and_placed() {
        let d = parse(&older_terminal_rp2350()).unwrap();
        assert_eq!(legacy_numbering(&d, 8), Some(8));
        let (placed, missing) = place_channels(&d, 8, 9);
        assert!(missing.is_empty());
        for (r, c) in &placed {
            assert_eq!(r.unified(8) as i32, c.channel_id, "{}", c.name);
        }

        let (mut s, log) = rig(Platform::Rp2350);
        apply(&mut s, &d, ApplyOptions::default());
        for i in 0..17u8 {
            assert_eq!(eq_channels_at(&log, 300.0 + i as f32), vec![i]);
        }
    }

    /// No file either Console writes looks like an older Terminal one, and on
    /// a two-input device the two numberings are the same thing.
    #[test]
    fn console_documents_are_never_taken_for_older_terminal_ones() {
        for text in [CONSOLE_RP2350, WINDOWS_RP2350, CONSOLE_RP2040] {
            assert_eq!(legacy_numbering(&parse(text).unwrap(), 8), None, "{text}");
        }
        let (mut s, _) = rig(Platform::Rp2350);
        let exported = capture(&mut s, None);
        assert_eq!(legacy_numbering(&exported, 8), None);
        // An older RP2040 file already agrees with the table.
        let old = r#"{"schemaVersion":1,"meta":{"inputChannelCount":2},"channels":[
            {"channelId":0},{"channelId":1},{"channelId":2,"isOutput":true},
            {"channelId":6,"isOutput":true}]}"#;
        assert_eq!(legacy_numbering(&parse(old).unwrap(), 2), None);
        assert_eq!(
            place_channels(&parse(old).unwrap(), 2, 5)
                .0
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            vec![
                ChannelRef::Input(0),
                ChannelRef::Input(1),
                ChannelRef::Output(0),
                ChannelRef::Output(4)
            ]
        );
    }

    #[test]
    fn the_timestamp_is_iso_8601_utc() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(iso8601(1_790_762_645), "2026-09-30T10:04:05Z");
    }
}
