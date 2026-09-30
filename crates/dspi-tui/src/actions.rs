//! What the interface does that is not a parameter write: the polled
//! diagnostics behind the Stats panel, the notification log behind the
//! Interrupt Monitor, the file actions, and the Tools menu.
//!
//! These are the Console's File, Tools and AutoEQ menus. Each one is a small
//! function of the session plus, where the Console shows a sheet, a dialog
//! built here so the wording lives next to the code that acts on it rather
//! than in the runner. Nothing here draws; the panels and the runner do that.

use std::path::{Path, PathBuf};

use dspi_proto::packets;
use dspi_session::{DeviceState, Notification, Session, Source, filterfile, preset_file};

use crate::widgets::{Button, Dialog};

// ---------------------------------------------------------------------------
// Stats: the polled half of the device's state
// ---------------------------------------------------------------------------

/// `REQ_GET_STATUS` sub-reads, by `wValue` (survey 6.4).
///
/// The bulk packet does not carry any of this: it is counters and clocks the
/// firmware answers one word at a time, so the Stats panel is the one screen
/// whose content has to be asked for rather than mirrored.
mod status {
    pub const PDM_RING_OVERRUNS: u16 = 3;
    pub const PDM_RING_UNDERRUNS: u16 = 4;
    pub const PDM_DMA_OVERRUNS: u16 = 5;
    pub const PDM_DMA_UNDERRUNS: u16 = 6;
    pub const SPDIF_OVERRUNS: u16 = 7;
    pub const SPDIF_UNDERRUNS: u16 = 8;
    pub const CLOCK_HZ: u16 = 13;
    pub const CORE_MV: u16 = 14;
    pub const SAMPLE_RATE: u16 = 15;
    pub const TEMP_CENTI_C: u16 = 16;
    pub const STARVATION_TOTAL: u16 = 17;
    /// 18..21, one per S/PDIF instance.
    pub const STARVATION_FIRST: u16 = 18;
    pub const USB_RING_OVERRUNS: u16 = 22;
}

/// The IEC 60958 channel status, as the Console reads it (`StatsView.swift`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChannelStatus {
    pub raw: [u8; 24],
}

impl ChannelStatus {
    pub fn is_consumer(&self) -> bool {
        self.raw[0] & 0x01 == 0
    }
    pub fn is_pcm(&self) -> bool {
        self.raw[0] & 0x02 == 0
    }
    pub fn copy_permitted(&self) -> bool {
        self.raw[0] & 0x04 != 0
    }

    pub fn category(&self) -> String {
        match self.raw[1] {
            0x00 => "General".into(),
            0x01 => "CD Player".into(),
            0x02 => "DAT".into(),
            0x03 => "DCC".into(),
            0x04 => "MiniDisc".into(),
            0x06 => "Synthesizer".into(),
            0x08 => "Broadcast Receiver".into(),
            0x09 => "Musical Instrument".into(),
            0x0A => "A/D Converter".into(),
            0x0C => "Mixer".into(),
            0x0D => "Rate Converter".into(),
            0x0E => "Sampler".into(),
            0x0F => "Digital Signal Processor".into(),
            other => format!("0x{other:02X}"),
        }
    }

    pub fn word_length(&self) -> &'static str {
        match self.raw[4] & 0x0F {
            0x00 => "Not indicated",
            0x02 => "16-bit",
            0x04 => "20-bit",
            0x08 => "17-bit",
            0x0A => "22-bit",
            0x0B => "24-bit",
            _ => "-",
        }
    }
}

/// The last few buffer fill readings, for the Stats panel's sparklines.
///
/// The Console keeps 256 samples of each buffer from a 60 ms poll, about
/// 15 s (`StatsView.swift`, `BufferFillHistory`). The panel reads the buffers
/// on its two-second poll instead, so [`FillHistory::LEN`] samples cover
/// about the same span. The series are the Console's: S/PDIF slots 0 to 3,
/// then the PDM DMA and PDM ring buffers. `None` is a sample where that
/// buffer was not running, or no reading came back.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FillHistory {
    pub series: [Vec<Option<u8>>; FillHistory::SERIES],
}

impl FillHistory {
    /// Eight two-second samples: 16 s.
    pub const LEN: usize = 8;
    pub const SERIES: usize = 6;
    pub const PDM_DMA: usize = 4;
    pub const PDM_RING: usize = 5;

    /// Add one poll's reading, dropping the oldest once full.
    pub fn push(&mut self, b: Option<&packets::BufferStatsPacket>) {
        for (i, ring) in self.series.iter_mut().enumerate() {
            let v = b.and_then(|b| match i {
                0..=3 if i < (b.num_spdif as usize).min(4) => Some(b.spdif[i].consumer_fill_pct),
                Self::PDM_DMA if b.pdm_active() => Some(b.pdm.dma_fill_pct),
                Self::PDM_RING if b.pdm_active() => Some(b.pdm.ring_fill_pct),
                _ => None,
            });
            ring.push(v);
            if ring.len() > Self::LEN {
                ring.remove(0);
            }
        }
    }
}

/// Everything the Stats panel shows that is not in the bulk packet.
///
/// Every section is an `Option`: absent means the firmware stalled the read,
/// which is how the Console decides whether to draw the section at all. A
/// device that does not have ADAT never shows an ADAT section rather than
/// showing one full of zeros.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stats {
    /// False until the first successful poll, so the panel can say so.
    pub read: bool,
    /// How many times the device has come back after going away. The runner
    /// counts it; nothing else can, because the firmware has no such counter.
    pub reconnects: u32,
    pub clock_hz: u32,
    pub core_mv: u32,
    pub sample_rate_hz: u32,
    pub temp_centi_c: i32,
    /// What the second core is doing (`REQ_GET_CORE1_MODE`, config.h:251).
    /// `None` on firmware that stalls the opcode, in which case the row goes.
    pub core1: Option<dspi_proto::enums::Core1Mode>,
    pub pdm_ring_over: u32,
    pub pdm_ring_under: u32,
    pub pdm_dma_over: u32,
    pub pdm_dma_under: u32,
    pub spdif_over: u32,
    pub spdif_under: u32,
    pub usb_ring_over: u32,
    pub starvation_total: u32,
    pub starvation_per_instance: [u32; 4],
    /// The jump since the previous poll, which is what the Console badges red.
    pub starvation_delta: u32,
    /// Polls since the last starvation event, and between the two before it.
    /// Counted in polls rather than wall clock so the panel says the same
    /// thing in a test as it does on a device.
    pub polls_since_starvation: Option<u32>,
    pub polls_between_starvations: Option<u32>,
    pub buffers: Option<packets::BufferStatsPacket>,
    /// The buffer fills of the last few polls, oldest first.
    pub fill_history: FillHistory,
    pub spdif_rx: Option<packets::SpdifRxStatusPacket>,
    pub spdif_channel: Option<ChannelStatus>,
    pub spdif_rx_pin: Option<u8>,
    pub lg: Option<packets::LgSoundSyncStatus>,
    pub adat: Option<packets::AdatStatus>,
    pub i2s_slave: Option<packets::I2sSlaveStatusPacket>,
}

/// One `REQ_GET_STATUS` word, or `None` when the firmware stalls it.
fn status_word(session: &mut Session, value: u16) -> Option<u32> {
    let bytes = session
        .with_transport(|t| t.control_in(dspi_proto::generated::opcodes::REQ_GET_STATUS, value, 4))
        .ok()?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn packet<T>(
    session: &mut Session,
    opcode: u8,
    value: u16,
    len: usize,
    decode: impl Fn(&[u8]) -> Result<T, packets::PacketError>,
) -> Option<T> {
    let bytes = session
        .with_transport(|t| t.control_in(opcode, value, len as u16))
        .ok()?;
    decode(&bytes).ok()
}

/// Read every diagnostic the Stats panel shows, once.
///
/// Called on the panel's two-second cadence and nowhere else: none of it is
/// notified, and none of it is worth a transfer while the panel is closed.
pub fn read_stats(session: &mut Session, state: &DeviceState, previous: &Stats) -> Stats {
    use dspi_proto::generated::opcodes as op;
    let feature = |name: &str| {
        state
            .caps
            .features
            .iter()
            .any(|f| f.name == name && f.present)
    };

    let mut s = Stats {
        read: true,
        clock_hz: status_word(session, status::CLOCK_HZ).unwrap_or_default(),
        core_mv: status_word(session, status::CORE_MV).unwrap_or_default(),
        sample_rate_hz: status_word(session, status::SAMPLE_RATE).unwrap_or_default(),
        temp_centi_c: status_word(session, status::TEMP_CENTI_C).unwrap_or_default() as i32,
        pdm_ring_over: status_word(session, status::PDM_RING_OVERRUNS).unwrap_or_default(),
        pdm_ring_under: status_word(session, status::PDM_RING_UNDERRUNS).unwrap_or_default(),
        pdm_dma_over: status_word(session, status::PDM_DMA_OVERRUNS).unwrap_or_default(),
        pdm_dma_under: status_word(session, status::PDM_DMA_UNDERRUNS).unwrap_or_default(),
        spdif_over: status_word(session, status::SPDIF_OVERRUNS).unwrap_or_default(),
        spdif_under: status_word(session, status::SPDIF_UNDERRUNS).unwrap_or_default(),
        usb_ring_over: status_word(session, status::USB_RING_OVERRUNS).unwrap_or_default(),
        starvation_total: status_word(session, status::STARVATION_TOTAL).unwrap_or_default(),
        ..Default::default()
    };

    // `REQ_GET_CORE1_MODE` answers one byte of `Core1Mode` (config.h:251,
    // :757-761). The enum is open, so an unrecognised mode says its number
    // rather than being clamped to Idle.
    s.core1 = session
        .with_transport(|t| t.control_in(op::REQ_GET_CORE1_MODE, 0, 1))
        .ok()
        .and_then(|b| b.first().copied())
        .map(dspi_proto::enums::Core1Mode::from_raw);

    // Wrap-safe, because the counter is a u32 that wraps rather than saturating.
    s.starvation_delta = s.starvation_total.wrapping_sub(previous.starvation_total);
    if previous.read && s.starvation_delta > 0 {
        s.polls_between_starvations = previous.polls_since_starvation;
        s.polls_since_starvation = Some(0);
    } else {
        s.polls_between_starvations = previous.polls_between_starvations;
        s.polls_since_starvation = previous.polls_since_starvation.map(|n| n + 1);
    }
    if s.starvation_total > 0 {
        for i in 0..4 {
            s.starvation_per_instance[i] =
                status_word(session, status::STARVATION_FIRST + i as u16).unwrap_or_default();
        }
    }

    s.buffers = packet(
        session,
        op::REQ_GET_BUFFER_STATS,
        0,
        packets::BufferStatsPacket::SIZE,
        packets::BufferStatsPacket::decode,
    );
    s.fill_history = previous.fill_history.clone();
    s.fill_history.push(s.buffers.as_ref());

    if feature("spdif_multi_input") || state.input_config().is_some() {
        s.spdif_rx = packet(
            session,
            op::REQ_GET_SPDIF_RX_STATUS,
            0,
            packets::SpdifRxStatusPacket::SIZE,
            packets::SpdifRxStatusPacket::decode,
        );
        // The channel status is only meaningful while the receiver is locked,
        // and only then does the Console ask for it.
        if s.spdif_rx
            .as_ref()
            .is_some_and(|r| r.state == dspi_proto::enums::SpdifRxState::Locked)
        {
            s.spdif_channel = session
                .with_transport(|t| t.control_in(op::REQ_GET_SPDIF_RX_CH_STATUS, 0, 24))
                .ok()
                .map(|b| {
                    let mut raw = [0u8; 24];
                    raw.copy_from_slice(&b[..24]);
                    ChannelStatus { raw }
                });
        }
        // Inputs 2 to 4 have their own pins, so the row has to name whichever
        // one is actually carrying audio.
        let index = s
            .spdif_rx
            .as_ref()
            .map(|r| match r.input_source.to_raw() {
                // INPUT_SOURCE_SPDIF2..4 are contiguous (survey 6.6).
                n @ 4..=6 => (n - 3) as u16,
                _ => 0,
            })
            .unwrap_or(0);
        s.spdif_rx_pin = session
            .with_transport(|t| t.control_in(op::REQ_GET_SPDIF_RX_PIN, index, 1))
            .ok()
            .and_then(|b| b.first().copied())
            .filter(|p| *p != 0);
    }

    if feature("lg_sound_sync") {
        s.lg = packet(
            session,
            op::REQ_GET_LG_SOUND_SYNC_STATUS,
            0,
            packets::LgSoundSyncStatus::SIZE,
            packets::LgSoundSyncStatus::decode,
        );
    }
    if feature("adat_output") {
        // RP2040 answers all zeros rather than stalling, which the Console
        // reads as "no ADAT here".
        s.adat = packet(
            session,
            op::REQ_GET_ADAT_STATUS,
            0,
            packets::AdatStatus::SIZE,
            packets::AdatStatus::decode,
        )
        .filter(|a| *a != packets::AdatStatus::default());
    }
    if feature("i2s_slave_clock") {
        s.i2s_slave = packet(
            session,
            op::REQ_GET_I2S_SLAVE_STATUS,
            0,
            packets::I2sSlaveStatusPacket::SIZE,
            packets::I2sSlaveStatusPacket::decode,
        )
        // The Console shows the section only while the device is in the slave
        // role; in master mode the numbers are meaningless.
        .filter(|i| i.clock_mode == 1);
    }
    s
}

// The panel's `r` goes through `ScreenEvent::Command("diag.buffers.reset")`
// like every other write a screen asks for, so there is no second path here.

// ---------------------------------------------------------------------------
// The notification log
// ---------------------------------------------------------------------------

/// One line of the Interrupt Monitor.
#[derive(Debug, Clone, PartialEq)]
pub struct LogEntry {
    /// Seconds since the log started. The Console stamps wall-clock local
    /// time; there is no local-time source here without another dependency,
    /// and an elapsed stamp is what `dspi watch` already prints.
    pub at: f64,
    pub seq: u8,
    pub event: &'static str,
    pub source: String,
    /// The `WireBulkParams` field the event landed in, decoded from its offset.
    pub field: String,
    pub value: String,
    pub lost: bool,
}

/// The always-on log the Interrupt Monitor draws.
///
/// The runner already drains the notification reader every tick to keep the
/// device state current, so the log is filled from there rather than by a
/// second reader: two readers on one endpoint would each see half the events.
/// It keeps running while the panel is closed, so opening it shows what just
/// happened rather than an empty box.
#[derive(Debug, Clone, PartialEq)]
pub struct EventLog {
    pub entries: std::collections::VecDeque<LogEntry>,
    pub paused: bool,
    /// Set once the reader has been attached, which is the Console's
    /// Listening / Inactive distinction.
    pub active: bool,
    /// Everything seen since the log was started, including what has since
    /// been dropped off the front.
    pub seen: usize,
    pub capacity: usize,
}

impl Default for EventLog {
    fn default() -> Self {
        Self {
            entries: std::collections::VecDeque::new(),
            paused: false,
            active: false,
            seen: 0,
            capacity: 2000,
        }
    }
}

impl EventLog {
    /// The header's word: the Console's Inactive / Paused / Listening.
    pub fn state(&self) -> &'static str {
        if !self.active {
            "Inactive"
        } else if self.paused {
            "Paused"
        } else {
            "Listening"
        }
    }

    /// The Console's "N events", with its singular.
    pub fn count_text(&self) -> String {
        format!(
            "{} event{}",
            self.seen,
            if self.seen == 1 { "" } else { "s" }
        )
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.seen = 0;
    }

    /// Record one notification. A paused log drops what arrives, which is what
    /// the Console's Pause does.
    pub fn push(&mut self, at: f64, n: &Notification) {
        if self.paused {
            return;
        }
        self.seen += 1;
        if self.entries.len() >= self.capacity {
            self.entries.pop_front();
        }
        let (event, source, field, value) = describe(&n.event);
        self.entries.push_back(LogEntry {
            at,
            seq: n.seq,
            event,
            source,
            field,
            value,
            lost: n.lost,
        });
    }
}

/// Name one event, its source, the field it touched and its new value.
///
/// This is `dspi watch`'s `describe_event` split into the monitor's columns,
/// with the offset resolved all the way to a field name rather than only to a
/// section.
pub fn describe(e: &dspi_session::Event) -> (&'static str, String, String, String) {
    use dspi_session::Event;
    let none = String::new();
    match e {
        Event::Idle => ("idle", none.clone(), none.clone(), none),
        Event::MasterVolume(db) => (
            "master_volume",
            "legacy".into(),
            "master_volume".into(),
            format!("{db:.1} dB"),
        ),
        Event::ParamChanged {
            offset,
            source,
            bytes,
        } => (
            "param_changed",
            source_word(*source),
            field_name(*offset as usize),
            field_value(*offset as usize, bytes),
        ),
        Event::BulkInvalidated { source } => (
            "bulk_invalidated",
            source_word(*source),
            none,
            "re-read".into(),
        ),
        Event::PresetLoaded { slot } => (
            "preset_loaded",
            none.clone(),
            "preset".into(),
            format!("slot {}", slot + 1),
        ),
        Event::InputFormat { channels } => (
            "input_format",
            none.clone(),
            "input channels".into(),
            format!("{channels}"),
        ),
        Event::SiggenState {
            state,
            reason,
            signal_type,
            channel,
        } => {
            const STATES: [&str; 5] = ["IDLE", "FADE_IN", "RUN", "GAP", "FADE_OUT"];
            const REASONS: [&str; 5] = ["-", "HOST", "COMPLETED", "PRESET", "RECONFIG"];
            (
                "siggen_state",
                none,
                "generator".into(),
                format!(
                    "{} reason={} type={signal_type} ch={}",
                    STATES.get(*state as usize).copied().unwrap_or("?"),
                    REASONS.get(*reason as usize).copied().unwrap_or("?"),
                    if *channel == 0xFF {
                        "-".to_string()
                    } else {
                        channel.to_string()
                    }
                ),
            )
        }
        Event::AdatState {
            enabled,
            active,
            pin,
        } => (
            "adat_state",
            none,
            "adat".into(),
            format!("enabled={enabled} active={active} pin={pin}"),
        ),
        Event::I2sSlaveState { state, rate_hz } => {
            const STATES: [&str; 4] = ["INACTIVE", "ACQUIRING", "RELOCKING", "LOCKED"];
            (
                "i2s_slave_state",
                none,
                "i2s".into(),
                format!(
                    "{} rate={rate_hz}",
                    STATES.get(*state as usize).copied().unwrap_or("?")
                ),
            )
        }
        Event::AdatInputState {
            state,
            rate_hz,
            clock_mode,
        } => {
            const STATES: [&str; 5] = ["INACTIVE", "ACQUIRING", "SYNCING", "LOCKED", "RELOCKING"];
            (
                "adat_input_state",
                none,
                "adat input".into(),
                format!(
                    "{} rate={rate_hz} mode={}",
                    STATES.get(*state as usize).copied().unwrap_or("?"),
                    if *clock_mode == 1 { "slave" } else { "master" }
                ),
            )
        }
        Event::IrLearn {
            state,
            protocol,
            code,
        } => (
            "cs_ir_learn",
            none,
            "ir learn".into(),
            format!("state={state} protocol={protocol} code=0x{code:08X}"),
        ),
        Event::CsAux {
            slot,
            state,
            level_q8,
            source,
        } => (
            "cs_aux",
            source_word(*source),
            format!("aux slot {slot}"),
            format!(
                "{} level={:.0}%",
                if *state != 0 { "on" } else { "off" },
                *level_q8 as f32 / 256.0
            ),
        ),
        Event::Unknown { id, bytes } => (
            "unknown",
            none,
            format!("0x{id:02X}"),
            bytes
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(" "),
        ),
    }
}

/// One word for the source column.
fn source_word(s: Source) -> String {
    match s {
        Source::Unknown => "unknown".into(),
        Source::HostSet => "host".into(),
        Source::BulkSet => "bulk".into(),
        Source::Preset => "preset".into(),
        Source::Factory => "factory".into(),
        Source::Gpio => "gpio".into(),
        Source::Internal => "device".into(),
        Source::Uac1 => "uac1".into(),
        Source::Uart => "uart".into(),
        Source::I2c => "i2c".into(),
        Source::Other(n) => format!("src{n}"),
    }
}

/// Resolve a `WireBulkParams` offset all the way to a field.
///
/// A `PARAM_CHANGED` carries an offset and nothing else, so this is what turns
/// "something at 1284" into "eq[3][2]". The section table is generated from the
/// firmware headers, so the arithmetic below is the only part that could drift,
/// and it is the same arithmetic the packet decoders use.
pub fn field_name(offset: usize) -> String {
    use dspi_proto::generated::{SECTIONS, wire};
    let Some((section, base, _)) = SECTIONS
        .iter()
        .copied()
        .find(|(_, off, len)| offset >= *off && offset < *off + *len)
    else {
        return format!("+0x{offset:X}");
    };
    let rel = offset - base;
    let indexed = |name: &str, stride: usize| {
        if rel.is_multiple_of(stride) {
            format!("{name}[{}]", rel / stride)
        } else {
            format!("{name}[{}]+0x{:X}", rel / stride, rel % stride)
        }
    };
    match section {
        "global" => match rel {
            0 => "global.preamp_gain_db".into(),
            4 => "global.bypass".into(),
            5 => "global.loudness_enabled".into(),
            6 => "global.loudness_output_mask".into(),
            8 => "global.loudness_ref_spl".into(),
            12 => "global.loudness_intensity_pct".into(),
            _ => format!("global+0x{rel:X}"),
        },
        "crossfeed" => match rel {
            0 => "crossfeed.enabled".into(),
            1 => "crossfeed.preset".into(),
            2 => "crossfeed.itd_enabled".into(),
            4 => "crossfeed.custom_fc".into(),
            8 => "crossfeed.custom_feed_db".into(),
            _ => format!("crossfeed+0x{rel:X}"),
        },
        "delays" => indexed("delays.delay_ms", 4),
        "preamp" => indexed("preamp.gain_db", 4),
        "crosspoints" => {
            let idx = rel / 8;
            let sub = rel % 8;
            let (input, output) = (idx / 9, idx % 9);
            match sub {
                0 => format!("crosspoints[{input}][{output}]"),
                _ => format!("crosspoints[{input}][{output}]+0x{sub:X}"),
            }
        }
        "outputs" => {
            let idx = rel / 12;
            match rel % 12 {
                0 => format!("outputs[{idx}].enabled"),
                1 => format!("outputs[{idx}].mute"),
                4 => format!("outputs[{idx}].gain_db"),
                8 => format!("outputs[{idx}].delay_ms"),
                sub => format!("outputs[{idx}]+0x{sub:X}"),
            }
        }
        "eq" | "crossovers" => {
            let bands = if section == "eq" {
                wire::WIRE_MAX_BANDS as usize
            } else {
                wire::WIRE_MAX_XOVER_BANDS as usize
            };
            let idx = rel / 16;
            let (ch, band) = (idx / bands, idx % bands);
            let name = if section == "eq" { "eq" } else { "xover" };
            match rel % 16 {
                0 => format!("{name}[{ch}][{band}]"),
                sub => format!("{name}[{ch}][{band}]+0x{sub:X}"),
            }
        }
        "channel_names" => indexed("channel_names", wire::WIRE_NAME_LEN as usize),
        "pins" => match rel {
            0 => "pins.num_pin_outputs".into(),
            n => format!("pins.pins[{}]", n - 1),
        },
        // Section 15 changed shape at V28, so its field table is the one in
        // `wire.rs` rather than a transcription of it.
        "input_config" => dspi_proto::wire::INPUT_CONFIG_FIELDS
            .iter()
            .find(|(_, off, len)| rel >= *off && rel < *off + *len)
            .map(|(name, off, _)| {
                if rel == *off {
                    format!("input_config.{name}")
                } else {
                    format!("input_config.{name}[{}]", rel - off)
                }
            })
            .unwrap_or_else(|| format!("input_config+0x{rel:X}")),
        "lg_sound_sync" => match rel {
            0 => "lg_sound_sync.enabled".into(),
            1 => "lg_sound_sync.present".into(),
            2 => "lg_sound_sync.volume".into(),
            3 => "lg_sound_sync.muted".into(),
            _ => format!("lg_sound_sync+0x{rel:X}"),
        },
        "user_volume" => match rel {
            0 => "user_volume.volume_db".into(),
            4 => "user_volume.muted".into(),
            _ => format!("user_volume+0x{rel:X}"),
        },
        "master_volume" => match rel {
            0 => "master_volume.volume_db".into(),
            _ => format!("master_volume+0x{rel:X}"),
        },
        other => {
            if rel == 0 {
                other.to_string()
            } else {
                format!("{other}+0x{rel:X}")
            }
        }
    }
}

/// Format a notified value by the shape of the field it landed in.
fn field_value(offset: usize, bytes: &[u8]) -> String {
    // A whole band arrives as one 16-byte packet, and reading it as four
    // floats would say nothing.
    if bytes.len() == 16
        && let Some(section) = dspi_proto::wire::BulkPacket::section_at(offset)
        && (section == "eq" || section == "crossovers")
    {
        let f = |o: usize| f32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
        return format!(
            "type={} byp={} f={:.1} Hz Q={:.2} g={:+.2} dB",
            bytes[0],
            bytes[1],
            f(4),
            f(8),
            f(12)
        );
    }
    if bytes.len() == 8 && dspi_proto::wire::BulkPacket::section_at(offset) == Some("crosspoints") {
        let gain = f32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        return format!(
            "en={} inv={} {gain:+.2} dB",
            u8::from(bytes[0] != 0),
            u8::from(bytes[1] != 0)
        );
    }
    match bytes.len() {
        1 => bytes[0].to_string(),
        2 => u16::from_le_bytes([bytes[0], bytes[1]]).to_string(),
        4 => {
            let v = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            // A float field is the common case; an integer one read as a float
            // comes out as a denormal, so fall back to the integer reading.
            if v == 0.0 || (v.abs() > 1e-6 && v.abs() < 1e9) {
                format!("{v}")
            } else {
                u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]).to_string()
            }
        }
        _ => bytes
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(" "),
    }
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// The Console's default export names, from its `NSSavePanel` calls.
pub const DEFAULT_FILTER_NAME: &str = "DSPi Filters.txt";
pub const DEFAULT_CONFIG_NAME: &str = "DSPi Configuration.dspipreset";

/// A path dialog, which is the terminal's version of the Console's file panel.
pub fn path_dialog(title: &str, body: &str, value: &str, action: &str) -> Dialog {
    let mut d = Dialog::text(title, body, value, "Path");
    d.buttons = vec![Button::new(action), Button::new("Cancel")];
    d
}

/// Expand a leading `~`, so a typed path behaves the way it does in a shell.
pub fn expand(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(path)
}

/// Complete a partly typed path, the way `Tab` does in a shell.
///
/// Returns the longest common prefix of what matches, with a trailing `/` when
/// the single match is a directory, or `None` when nothing matches. Directory
/// listing order is not stable, so the candidates are sorted before the prefix
/// is taken.
pub fn complete_path(typed: &str) -> Option<String> {
    let expanded = expand(typed);
    let (dir, prefix) = match typed.ends_with('/') {
        true => (expanded.clone(), String::new()),
        false => (
            expanded.parent().unwrap_or(Path::new(".")).to_path_buf(),
            expanded
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default(),
        ),
    };
    let dir = if dir.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        dir
    };

    let mut names: Vec<(String, bool)> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.starts_with(&prefix).then(|| (name, e.path().is_dir()))
        })
        .collect();
    if names.is_empty() {
        return None;
    }
    names.sort();

    let common = if names.len() == 1 {
        let (name, is_dir) = &names[0];
        let mut n = name.clone();
        if *is_dir {
            n.push('/');
        }
        n
    } else {
        let first = &names[0].0;
        let mut len = first.len();
        for (other, _) in &names[1..] {
            len = len.min(
                first
                    .chars()
                    .zip(other.chars())
                    .take_while(|(a, b)| a == b)
                    .count(),
            );
        }
        first[..len].to_string()
    };

    // Put the completion back onto the text the person typed, so a `~` stays a
    // `~` rather than being rewritten to their home directory under them.
    let head = match typed.rfind('/') {
        Some(i) => &typed[..=i],
        None => "",
    };
    Some(format!("{head}{common}"))
}

// ---------------------------------------------------------------------------
// Filter files
// ---------------------------------------------------------------------------

/// The whole device as a filter file, read from the bulk shadow.
///
/// Nothing is asked of the device: the snapshot the interface already holds is
/// the same table `GET_EQ_PARAM` answers from, so an export is instant and
/// cannot disagree with what is on screen.
pub fn filter_file(state: &DeviceState) -> filterfile::FilterFile {
    let ni = state.caps.num_inputs as usize;
    let mut file = filterfile::FilterFile {
        format_version: filterfile::FORMAT_VERSION,
        channels: Vec::new(),
    };
    for ch in 0..state.caps.num_channels as usize {
        let is_output = ch >= ni;
        let name = crate::screens::channel_name(state, ch);
        let header = if is_output {
            let out = state.output(ch - ni);
            format!(
                "Output {}: {name} ({})",
                ch - ni,
                if out.enabled { "Enabled" } else { "Disabled" }
            )
        } else {
            format!("Input {ch}: {name}")
        };
        let bands = |b: Vec<dspi_proto::value::EqParamPacket>| -> Vec<dspi_proto::dsp::Band> {
            b.iter()
                .map(|p| dspi_proto::dsp::Band {
                    filter_type: p.filter_type,
                    freq: p.freq,
                    q: p.q,
                    gain_db: p.gain_db,
                    bypass: p.bypass,
                })
                .collect()
        };
        file.channels.push(filterfile::ChannelBank {
            header,
            index: Some(if is_output { (ch - ni) as u8 } else { ch as u8 }),
            is_output,
            // Outputs have no input trim, so they carry no preamp line.
            preamp_db: (!is_output).then(|| state.preamp_db(ch)),
            peq: bands(state.bands(ch as u8)),
            crossover: if is_output && crate::screens::supports_crossover(state) {
                bands(state.xover_bands(ch as u8))
            } else {
                Vec::new()
            },
        });
    }
    file
}

/// Write a filter file, returning the Console's success line.
pub fn export_filters(state: &DeviceState, path: &str) -> Result<String, String> {
    let file = filter_file(state);
    let text = filterfile::write(&file, &timestamp());
    std::fs::write(expand(path), text).map_err(|e| format!("Failed to write file: {e}"))?;
    Ok("Filters exported successfully".into())
}

/// A date stamp for the file header.
///
/// `date` is on every platform this runs on, and reaching for a time crate to
/// print one line into a comment is not worth the dependency.
fn timestamp() -> String {
    std::process::Command::new("date")
        .arg("+%Y-%m-%d %H:%M:%S")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// One channel a filter file could be imported onto.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportTarget {
    /// The unified channel index on this device.
    pub channel: u8,
    /// Which of the file's sections supplies it.
    pub bank: usize,
    pub label: String,
    pub checked: bool,
}

/// Which channel a file section belongs to on this device.
///
/// Index-keyed headers are authoritative, then the device's own channel names,
/// then the Windows default names. A section that matches none of them cannot
/// be placed.
pub fn resolve_channel(bank: &filterfile::ChannelBank, state: &DeviceState) -> Option<u8> {
    let caps = &state.caps;
    if let Some(ix) = bank.index {
        let ch = if bank.is_output {
            caps.num_inputs.checked_add(ix)?
        } else {
            ix
        };
        return (ch < caps.num_channels).then_some(ch);
    }
    let want = bank.header.trim().to_ascii_lowercase();
    if !want.is_empty()
        && let Some(c) = caps
            .channels
            .iter()
            .find(|c| c.name.to_ascii_lowercase() == want)
    {
        return Some(c.index);
    }
    let pdm = caps.num_outputs.saturating_sub(1);
    match filterfile::windows_channel(&bank.header, caps.num_inputs, pdm)? {
        filterfile::ChannelRef::Input(i) => (i < caps.num_inputs).then_some(i),
        filterfile::ChannelRef::Output(o) => caps
            .num_inputs
            .checked_add(o)
            .filter(|c| *c < caps.num_channels),
    }
}

/// Is this a single-channel file (a REW export), or one with named sections?
///
/// The Console asks the two questions differently: a REW file has one bank and
/// no idea where it belongs, so the person picks the channels; a DSPi or
/// Windows file names its own, so the person only confirms them.
pub fn is_single_channel(file: &filterfile::FilterFile, state: &DeviceState) -> bool {
    file.channels.len() == 1 && resolve_channel(&file.channels[0], state).is_none()
}

/// The channel picker for an import, and what each row stands for.
pub fn channel_picker(
    file: &filterfile::FilterFile,
    state: &DeviceState,
) -> (Dialog, Vec<ImportTarget>) {
    let ni = state.caps.num_inputs as usize;
    let mut targets = Vec::new();

    if is_single_channel(file, state) {
        // Active inputs, checked; enabled outputs, unchecked. The Console's
        // default: a REW correction is for what is playing, not for a speaker
        // tap.
        for ch in 0..ni {
            targets.push(ImportTarget {
                channel: ch as u8,
                bank: 0,
                label: crate::screens::channel_name(state, ch),
                checked: true,
            });
        }
        for o in 0..state.caps.num_outputs as usize {
            if !state.output(o).enabled {
                continue;
            }
            targets.push(ImportTarget {
                channel: (ni + o) as u8,
                bank: 0,
                label: crate::screens::channel_name(state, ni + o),
                checked: false,
            });
        }
        let bank = &file.channels[0];
        let mut prompt = format!("Found {} filter(s)", bank.peq.len());
        if let Some(p) = bank.preamp_db {
            prompt.push_str(&format!(" and a {p:+.1} dB preamp"));
        }
        prompt.push_str(". Select which channel(s) to apply them to:");
        let items = targets
            .iter()
            .map(|t| (t.label.clone(), t.checked))
            .collect();
        return (
            Dialog::checklist(
                "Import Filters",
                prompt,
                items,
                vec![Button::new("Import"), Button::new("Cancel")],
            ),
            targets,
        );
    }

    for (i, bank) in file.channels.iter().enumerate() {
        let Some(channel) = resolve_channel(bank, state) else {
            continue;
        };
        targets.push(ImportTarget {
            channel,
            bank: i,
            label: crate::screens::channel_name(state, channel as usize),
            checked: true,
        });
    }
    let items = targets
        .iter()
        .map(|t| (t.label.clone(), t.checked))
        .collect();
    (
        Dialog::checklist(
            "Import Filters",
            "This file contains filter settings for multiple channels. \
             Select which channels to import:",
            items,
            vec![Button::new("Import"), Button::new("Cancel")],
        ),
        targets,
    )
}

/// What an import could not carry across, in the Console's words.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportNotes {
    pub truncated: usize,
    pub unsupported: Vec<String>,
    pub crossover_skipped: bool,
    pub preamp_dropped: bool,
    pub newer_format: bool,
}

impl ImportNotes {
    /// The Console's `ImportReport.notes`, verbatim.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if self.truncated > 0 {
            lines.push(format!(
                "{} filter(s) past the end of each channel's band bank were not applied.",
                self.truncated
            ));
        }
        if !self.unsupported.is_empty() {
            let mut names = self.unsupported.clone();
            names.sort();
            lines.push(format!(
                "Skipped filter types the connected firmware doesn't support: {}.",
                names.join(", ")
            ));
        }
        if self.crossover_skipped {
            lines.push(
                "Crossover settings were skipped - the connected firmware doesn't support \
                 crossovers."
                    .into(),
            );
        }
        if self.preamp_dropped {
            lines.push(
                "The file's preamp was not applied to output channels, which have no input trim."
                    .into(),
            );
        }
        if self.newer_format {
            lines.push(
                "This file was written by a newer version of DSPi Console; some settings may \
                 have been ignored."
                    .into(),
            );
        }
        lines
    }
}

/// The commands one import comes to, and what it could not carry.
pub fn import_commands(
    file: &filterfile::FilterFile,
    state: &DeviceState,
    targets: &[ImportTarget],
) -> (Vec<String>, ImportNotes) {
    let ni = state.caps.num_inputs as usize;
    let max = state.caps.max_bands as usize;
    let allowed = crate::screens::available_types(state, true);
    let mut notes = ImportNotes {
        newer_format: file.format_version > filterfile::FORMAT_VERSION,
        ..Default::default()
    };
    let mut out = Vec::new();

    for t in targets.iter().filter(|t| t.checked) {
        let Some(bank) = file.channels.get(t.bank) else {
            continue;
        };
        let token = crate::screens::channel_token(state, t.channel as usize);
        notes.truncated = notes.truncated.max(bank.peq.len().saturating_sub(max));

        for band in 0..max {
            let packet = match bank.peq.get(band) {
                Some(b) if allowed.contains(&b.filter_type) => dspi_proto::value::EqParamPacket {
                    channel: t.channel,
                    band: band as u8,
                    filter_type: b.filter_type,
                    bypass: b.bypass,
                    freq: b.freq,
                    q: b.q,
                    gain_db: b.gain_db,
                    qp: None,
                },
                Some(b) => {
                    let name = crate::screens::type_name(b.filter_type);
                    if !notes.unsupported.contains(&name) {
                        notes.unsupported.push(name);
                    }
                    continue;
                }
                // Bands the file does not fill are cleared, or the tuning that
                // was there before survives underneath the imported one.
                None => crate::screens::cleared_band(t.channel, band as u8),
            };
            out.extend(crate::screens::band_command(
                state,
                t.channel as usize,
                band as u8,
                &packet,
            ));
        }

        if let Some(p) = bank.preamp_db {
            if (t.channel as usize) < ni {
                out.push(format!("pre {token} {}", crate::screens::number(p)));
            } else {
                notes.preamp_dropped = true;
            }
        }

        if !bank.crossover.is_empty() {
            if !crate::screens::supports_crossover(state) || (t.channel as usize) < ni {
                notes.crossover_skipped = true;
            } else {
                for (i, b) in bank.crossover.iter().enumerate().take(4) {
                    let packet = dspi_proto::value::EqParamPacket {
                        channel: t.channel,
                        band: 20 + i as u8,
                        filter_type: b.filter_type,
                        bypass: b.bypass,
                        freq: b.freq,
                        q: b.q,
                        gain_db: b.gain_db,
                        qp: None,
                    };
                    out.extend(crate::screens::band_command(
                        state,
                        t.channel as usize,
                        20 + i as u8,
                        &packet,
                    ));
                }
            }
        }
    }
    (out, notes)
}

/// The Console's success line for an import, with its notes appended.
pub fn import_report(
    file: &filterfile::FilterFile,
    state: &DeviceState,
    count: usize,
    notes: &ImportNotes,
) -> String {
    let mut text = if is_single_channel(file, state) {
        format!("Filters imported to {count} channel(s)")
    } else {
        "Filters imported successfully".to_string()
    };
    for line in notes.lines() {
        text.push_str("  ");
        text.push_str(&line);
    }
    text
}

// ---------------------------------------------------------------------------
// Device configuration documents
// ---------------------------------------------------------------------------

/// Capture the whole device into a `.dspipreset`.
///
/// The input pair links are the app's rather than the device's, so they come
/// from the caller (PresetDocumentTransfer.swift:43).
pub fn export_config(
    session: &mut Session,
    path: &str,
    linked_pairs: &[bool],
) -> Result<String, String> {
    let path = expand(path);
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string());
    let mut doc = preset_file::capture(session, name);
    doc.global.input_pair_linked = linked_pairs.to_vec();
    std::fs::write(&path, preset_file::write(&doc))
        .map_err(|e| format!("Failed to write file: {e}"))?;

    // The Console counts the bands that do something, not the empty slots that
    // pad every bank out to its full width.
    let active = |b: &Vec<preset_file::BandBlock>| b.iter().filter(|b| b.r#type != 0).count();
    let bands: usize = doc.channels.iter().map(|c| active(&c.eq)).sum();
    let xover: usize = doc.channels.iter().map(|c| active(&c.crossover)).sum();
    Ok(format!(
        "Configuration exported.  {} channels, {bands} active EQ bands, {xover} crossover \
         bands, {} crosspoints.",
        doc.channels.len(),
        doc.matrix.len()
    ))
}

pub fn read_config(path: &str) -> Result<preset_file::PresetDocument, String> {
    let text = std::fs::read_to_string(expand(path)).map_err(|e| e.to_string())?;
    preset_file::parse(&text).map_err(|e| e.to_string())
}

/// The import-options checklist: what the file came from, what will always be
/// applied, and the two things that are opt-in.
///
/// Volume and wiring describe a room and a board rather than a tuning, so
/// neither necessarily belongs to the machine the file is being applied to.
pub fn import_options_dialog(doc: &preset_file::PresetDocument, state: &DeviceState) -> Dialog {
    let mut provenance: Vec<String> = Vec::new();
    if let Some(p) = doc.meta.platform.as_ref().filter(|p| !p.is_empty()) {
        provenance.push(p.clone());
    }
    if let Some(f) = doc.meta.firmware_version.as_ref().filter(|f| !f.is_empty()) {
        provenance.push(format!("firmware {f}"));
    }
    if let Some(s) = doc.meta.saved_utc.as_ref().filter(|s| !s.is_empty()) {
        provenance.push(s.clone());
    }

    let mut body = if provenance.is_empty() {
        "Applies the settings in this file to the connected device.".to_string()
    } else {
        format!("Saved from {}.", provenance.join(", "))
    };

    // A file from a device with a different shape still applies; say so up
    // front rather than leaving it to be discovered in the report.
    let here = format!("{:?}", state.caps.platform).to_uppercase();
    if let Some(source) = doc.meta.platform.as_ref().filter(|p| !p.is_empty())
        && !source.eq_ignore_ascii_case(&here)
    {
        body.push_str(&format!(
            "\n\nThis file came from a {source} device and you are connected to {here}. \
             Anything the connected device doesn't have will be skipped."
        ));
    }
    body.push_str(
        "\n\nEQ, crossover, delays, gains, routing and the DSP features are always applied.",
    );

    Dialog::checklist(
        "Import Device Configuration",
        body,
        vec![
            ("Volume levels (master and listening volume)".into(), false),
            (
                "Hardware I/O (GPIO pins, clocks, ADAT, inputs, output limiters)".into(),
                false,
            ),
        ],
        vec![Button::new("Import"), Button::new("Cancel")],
    )
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

/// Commit Parameters: the Console's confirm, naming the slot.
pub fn commit_dialog(slot: u8) -> Dialog {
    Dialog::confirm(
        "Save Preset",
        format!("Save current parameters to preset slot {}?", slot + 1),
        vec![Button::new("Save"), Button::new("Cancel")],
    )
}

pub fn revert_dialog() -> Dialog {
    Dialog::confirm(
        "Revert to Saved",
        "Revert to last saved parameters?\n\nCurrent unsaved changes will be lost.",
        vec![Button::destructive("Revert"), Button::new("Cancel")],
    )
}

pub fn factory_reset_dialog() -> Dialog {
    Dialog::confirm(
        "Factory Reset",
        "Do you wish to clear all active parameters?\n\nThis will not overwrite your saved \
         parameters unless you run 'Commit Parameters'.",
        vec![Button::destructive("Reset"), Button::new("Cancel")],
    )
    .critical()
    .default_button(1)
}

pub fn firmware_dialog() -> Dialog {
    Dialog::confirm(
        "Firmware Update",
        "This will reboot the device into bootloader mode.\n\nAudio output will stop \
         immediately. The device will appear as a USB drive to which you can drag a .uf2 \
         firmware file.",
        vec![
            Button::destructive("Reboot into Bootloader"),
            Button::new("Cancel"),
        ],
    )
    .critical()
    .default_button(1)
}

/// Persist the live master volume to the device's independent storage.
pub fn save_master_volume(session: &mut Session, state: &DeviceState) -> Result<String, String> {
    match session.write("vol.master.save", &[], dspi_proto::value::Value::Trigger) {
        Ok(_) => {
            let db = state.master_volume_db();
            let shown = if db <= -128.0 {
                "-inf dB (mute)".to_string()
            } else {
                format!("{db:.1} dB")
            };
            Ok(format!(
                "Master volume saved ({shown}). It will be applied on next boot."
            ))
        }
        Err(_) => {
            Err("Failed to save master volume - the device did not acknowledge the request.".into())
        }
    }
}

pub fn save_output_config(session: &mut Session) -> Result<String, String> {
    match session.write("dev.save.io", &[], dspi_proto::value::Value::Trigger) {
        Ok(_) => Ok("Output configuration saved. It will be applied on next boot.".into()),
        Err(_) => Err(
            "Failed to save output configuration - the device did not acknowledge the request."
                .into(),
        ),
    }
}

// ---------------------------------------------------------------------------
// The bootloader handoff
// ---------------------------------------------------------------------------

/// What the interface can see of the world while the device reboots.
///
/// Behind a trait because the whole point of this flow is what happens when
/// the device disappears, which is not something a test can arrange for real.
pub trait BootProbe {
    /// Is a DSPi still enumerated?
    fn device_present(&self) -> bool;
    /// Where the `RPI-RP2` volume is mounted, once it appears.
    fn volume(&self) -> Option<PathBuf>;
}

pub struct RealBootProbe;

impl BootProbe for RealBootProbe {
    fn device_present(&self) -> bool {
        dspi_transport::list_devices().is_ok_and(|d| !d.is_empty())
    }

    fn volume(&self) -> Option<PathBuf> {
        // The RP2 bootloader mounts as a FAT volume called RPI-RP2. Where that
        // lands is the platform's business, so every usual mount root is
        // looked at rather than one being assumed.
        let mut roots: Vec<PathBuf> = vec![PathBuf::from("/Volumes")];
        for base in ["/media", "/run/media", "/mnt"] {
            let base = PathBuf::from(base);
            roots.push(base.clone());
            if let Ok(entries) = std::fs::read_dir(&base) {
                roots.extend(entries.filter_map(|e| e.ok()).map(|e| e.path()));
            }
        }
        for root in roots {
            let candidate = root.join("RPI-RP2");
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
        // Windows mounts it as a drive letter with no predictable name.
        ('D'..='Z')
            .map(|letter| PathBuf::from(format!("{letter}:\\")))
            .find(|p| p.join("INFO_UF2.TXT").is_file())
    }
}

/// How far the bootloader handoff has got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootPhase {
    /// The reboot has been asked for; the device is still enumerated.
    WaitingForDisconnect,
    /// It has gone; the drive has not appeared yet.
    WaitingForVolume,
    /// The drive is mounted here.
    Ready(PathBuf),
    /// Nothing appeared in time.
    TimedOut,
}

/// The wait after `dev.bootloader`.
///
/// The firmware answers, waits 100 ms and resets into the bootloader; there is
/// no confirmation and no further traffic, so the host owns the rest of the
/// experience (survey 6.2). This is that: watch the device go, watch the drive
/// arrive, and then say where to put the file.
#[derive(Debug, Clone)]
pub struct BootloaderWatch {
    pub phase: BootPhase,
    polls: u32,
    /// How many polls to wait before giving up, at the runner's tick rate.
    limit: u32,
}

impl Default for BootloaderWatch {
    fn default() -> Self {
        Self::new()
    }
}

impl BootloaderWatch {
    /// A little over thirty seconds at the default tick, which is long enough
    /// for a slow hub and short enough not to look hung.
    pub const DEFAULT_LIMIT: u32 = 600;

    pub fn new() -> Self {
        Self {
            phase: BootPhase::WaitingForDisconnect,
            polls: 0,
            limit: Self::DEFAULT_LIMIT,
        }
    }

    pub fn with_limit(limit: u32) -> Self {
        Self {
            limit,
            ..Self::new()
        }
    }

    /// Advance one tick. Returns the phase it is now in.
    pub fn poll(&mut self, probe: &dyn BootProbe) -> &BootPhase {
        if matches!(self.phase, BootPhase::Ready(_) | BootPhase::TimedOut) {
            return &self.phase;
        }
        self.polls += 1;
        if self.phase == BootPhase::WaitingForDisconnect && !probe.device_present() {
            self.phase = BootPhase::WaitingForVolume;
        }
        // The drive can appear before the poll that notices the device left,
        // so it is looked for in both phases rather than only the second.
        if let Some(path) = probe.volume() {
            self.phase = BootPhase::Ready(path);
        } else if self.polls >= self.limit {
            self.phase = BootPhase::TimedOut;
        }
        &self.phase
    }

    /// The progress dialog's status line.
    pub fn status(&self) -> String {
        match &self.phase {
            BootPhase::WaitingForDisconnect => "Waiting for the device to disconnect...".into(),
            BootPhase::WaitingForVolume => "Waiting for the RPI-RP2 drive to appear...".into(),
            BootPhase::Ready(path) => format!(
                "Copy the .uf2 firmware file to {}. The device restarts on its own once the \
                 copy finishes.",
                path.display()
            ),
            BootPhase::TimedOut => "The RPI-RP2 drive did not appear. Look for it in your file \
                                    manager, or unplug the device and hold BOOTSEL while \
                                    plugging it back in."
                .into(),
        }
    }

    /// How far along the bar is; a wait with no known end sits at a third.
    pub fn fraction(&self) -> f32 {
        match self.phase {
            BootPhase::WaitingForDisconnect => 0.15,
            BootPhase::WaitingForVolume => 0.5,
            _ => 1.0,
        }
    }

    pub fn finished(&self) -> bool {
        matches!(self.phase, BootPhase::Ready(_) | BootPhase::TimedOut)
    }
}

/// Every DSPi currently plugged in, for the device picker.
pub fn device_list() -> Vec<dspi_transport::DeviceDescriptor> {
    dspi_transport::list_devices().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_fill_history_keeps_the_last_eight_readings_of_each_buffer() {
        use dspi_proto::packets::BufferStatsPacket;
        let mut h = super::FillHistory::default();
        let mut b = BufferStatsPacket {
            num_spdif: 2,
            flags: BufferStatsPacket::FLAG_PDM_ACTIVE,
            ..Default::default()
        };
        for k in 0..10u8 {
            b.spdif[1].consumer_fill_pct = k;
            b.pdm.ring_fill_pct = 50 + k;
            h.push(Some(&b));
        }
        assert_eq!(h.series[1].len(), super::FillHistory::LEN);
        assert_eq!(h.series[1][0], Some(2), "the oldest two aged out");
        assert_eq!(h.series[1][7], Some(9));
        assert_eq!(h.series[super::FillHistory::PDM_RING][7], Some(59));
        // Slots past `num_spdif` never ran.
        assert!(h.series[2].iter().all(Option::is_none));
        // A poll with no reading, or with PDM stopped, leaves a gap.
        h.push(None);
        assert_eq!(h.series[1][7], None);
        b.flags = 0;
        h.push(Some(&b));
        assert_eq!(h.series[super::FillHistory::PDM_DMA][7], None);
        assert_eq!(h.series[0][7], Some(0));
    }

    use super::*;
    use crate::shell::fixture;

    fn state() -> DeviceState {
        fixture::state()
    }

    /// A `.dspipreset` with nothing in it but the meta block under test.
    fn document(meta: &str) -> preset_file::PresetDocument {
        preset_file::parse(&format!(
            r#"{{"schemaVersion":1,"meta":{meta},"channels":[{{"channelId":0}}]}}"#
        ))
        .expect("document")
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dspi-actions-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        dir.join(name)
    }

    // ----------------------------------------------------------- filter files

    #[test]
    fn an_export_names_every_channel_and_says_which_outputs_are_on() {
        let s = state();
        let file = filter_file(&s);
        assert_eq!(file.channels.len(), s.caps.num_channels as usize);
        assert_eq!(file.channels[0].header, "Input 0: FL");
        assert_eq!(file.channels[0].index, Some(0));
        assert!(
            file.channels[0].preamp_db.is_some(),
            "inputs carry a preamp"
        );
        let out = &file.channels[s.caps.num_inputs as usize];
        assert!(out.header.starts_with("Output 0: "), "{}", out.header);
        assert!(
            out.header.ends_with("(Enabled)") || out.header.ends_with("(Disabled)"),
            "{}",
            out.header
        );
        assert!(out.preamp_db.is_none(), "outputs have no input trim");
        assert_eq!(out.crossover.len(), 4, "the crossover bank travels too");
    }

    #[test]
    fn an_export_writes_a_file_that_reads_back() {
        let path = scratch("tuning.txt");
        let s = state();
        let msg = export_filters(&s, path.to_str().unwrap()).expect("write");
        assert_eq!(msg, "Filters exported successfully");
        let text = std::fs::read_to_string(&path).unwrap();
        let back = filterfile::parse(&text).unwrap();
        assert_eq!(back.channels.len(), s.caps.num_channels as usize);
        let _ = std::fs::remove_file(&path);
    }

    /// A REW file names no channel, so the person is asked which ones it goes
    /// on; a DSPi or Windows file names its own, so they only confirm them.
    #[test]
    fn the_channel_picker_asks_the_consoles_two_questions() {
        let s = state();
        let rew =
            filterfile::parse(include_str!("../../../tests/fixtures/REWFilters.txt")).unwrap();
        assert!(is_single_channel(&rew, &s));
        let (dialog, targets) = channel_picker(&rew, &s);
        assert_eq!(dialog.title, "Import Filters");
        assert!(
            dialog
                .body
                .starts_with("Found 4 filter(s) and a -6.5 dB preamp. Select which channel(s)"),
            "{}",
            dialog.body
        );
        assert!(
            targets.iter().take(8).all(|t| t.checked),
            "inputs are checked by default"
        );
        assert!(
            targets.iter().skip(8).all(|t| !t.checked),
            "outputs are not"
        );

        let dspi =
            filterfile::parse(include_str!("../../../tests/fixtures/MacOSFilters.txt")).unwrap();
        assert!(!is_single_channel(&dspi, &s));
        let (dialog, targets) = channel_picker(&dspi, &s);
        assert_eq!(
            dialog.body,
            "This file contains filter settings for multiple channels. Select which channels \
             to import:"
        );
        assert_eq!(targets.len(), 4);
        assert!(targets.iter().all(|t| t.checked));
        assert_eq!(targets[2].channel, 8, "Output 0 is the first output");
    }

    /// A Windows file keys its sections by name, and they have to land on the
    /// right channels here or the import is silently wrong.
    #[test]
    fn a_windows_file_lands_on_the_right_channels() {
        let s = state();
        let file =
            filterfile::parse(include_str!("../../../tests/fixtures/WindowsFilters.txt")).unwrap();
        let (_, targets) = channel_picker(&file, &s);
        let placed: Vec<u8> = targets.iter().map(|t| t.channel).collect();
        assert_eq!(placed, vec![0, 1, 8, 9, 16], "Master L/R, SPDIF 1 L/R, PDM");
    }

    #[test]
    fn an_import_writes_every_band_and_clears_the_rest() {
        let s = state();
        let file = filterfile::parse(
            "[Input 0: FL]\nPreamp -3.5 dB\nFilter 1: ON PK Fc 100 Hz Gain 3.0 dB Q 1.00\n",
        )
        .unwrap();
        let (_, targets) = channel_picker(&file, &s);
        let (commands, notes) = import_commands(&file, &s, &targets);
        assert_eq!(commands[0], "eq in.1 1 peak 100 1 3");
        assert_eq!(
            commands[1], "eq in.1 2 flat 1000 0.707 0",
            "an unfilled band is cleared, not left alone"
        );
        assert_eq!(commands.len(), s.caps.max_bands as usize + 1);
        assert_eq!(commands.last().unwrap(), "pre in.1 -3.5");
        assert!(notes.lines().is_empty(), "{:?}", notes);
    }

    /// The report is the only thing that tells someone a band did not survive,
    /// so its wording is the Console's exactly.
    #[test]
    fn the_import_report_is_the_consoles_wording() {
        let notes = ImportNotes {
            truncated: 3,
            unsupported: vec!["Notch".into(), "All Pass (360°)".into()],
            crossover_skipped: true,
            preamp_dropped: true,
            newer_format: true,
        };
        assert_eq!(
            notes.lines(),
            vec![
                "3 filter(s) past the end of each channel's band bank were not applied.",
                "Skipped filter types the connected firmware doesn't support: All Pass (360°), \
                 Notch.",
                "Crossover settings were skipped - the connected firmware doesn't support \
                 crossovers.",
                "The file's preamp was not applied to output channels, which have no input trim.",
                "This file was written by a newer version of DSPi Console; some settings may \
                 have been ignored.",
            ]
        );
        assert!(ImportNotes::default().lines().is_empty());

        let s = state();
        let rew =
            filterfile::parse(include_str!("../../../tests/fixtures/REWFilters.txt")).unwrap();
        assert!(
            import_report(&rew, &s, 2, &ImportNotes::default())
                .starts_with("Filters imported to 2 channel(s)"),
            "the single-channel wording"
        );
        let dspi =
            filterfile::parse(include_str!("../../../tests/fixtures/MacOSFilters.txt")).unwrap();
        let report = import_report(&dspi, &s, 4, &notes);
        assert!(
            report.starts_with("Filters imported successfully"),
            "{report}"
        );
        assert!(report.contains("3 filter(s) past the end"), "{report}");
    }

    /// Old firmware would reject a type it does not have, so it is left out and
    /// named rather than sent and silently dropped.
    #[test]
    fn a_type_this_firmware_lacks_is_reported_not_sent() {
        let mut s = state();
        s.caps.firmware = "1.1.3".into();
        s.caps.wire_format = 10;
        let file = filterfile::parse("[Input 0: FL]\nFilter 1: ON NT Fc 60 Hz Q 8.00\n").unwrap();
        let (_, targets) = channel_picker(&file, &s);
        let (commands, notes) = import_commands(&file, &s, &targets);
        assert!(
            !commands.iter().any(|c| c.contains("notch")),
            "{commands:?}"
        );
        assert_eq!(notes.unsupported, vec!["Notch".to_string()]);
        assert!(notes.lines()[0].contains("Skipped filter types"));
    }

    #[test]
    fn a_files_crossovers_are_skipped_on_an_input_and_on_old_firmware() {
        let mut s = state();
        let file =
            filterfile::parse("[Output 0: OUT L]\nXover 1: ON LR4LP Fc 2000.0 Hz\n").unwrap();
        let (_, targets) = channel_picker(&file, &s);
        let (commands, notes) = import_commands(&file, &s, &targets);
        assert!(
            commands.iter().any(|c| c.starts_with("eq out.1 20 lr4lp")),
            "{commands:?}"
        );
        assert!(!notes.crossover_skipped);

        s.caps.wire_format = 10;
        let (commands, notes) = import_commands(&file, &s, &targets);
        assert!(
            !commands.iter().any(|c| c.contains("lr4lp")),
            "{commands:?}"
        );
        assert!(notes.crossover_skipped);
    }

    // ----------------------------------------------------- configuration files

    #[test]
    fn the_import_options_show_provenance_and_the_cross_platform_warning() {
        let s = state();
        let doc = document(
            r#"{"platform":"RP2040","firmwareVersion":"1.1.5","savedUtc":"2026-03-14 09:21"}"#,
        );
        let d = import_options_dialog(&doc, &s);
        assert_eq!(d.title, "Import Device Configuration");
        assert!(
            d.body
                .starts_with("Saved from RP2040, firmware 1.1.5, 2026-03-14 09:21."),
            "{}",
            d.body
        );
        assert!(
            d.body.contains(
                "This file came from a RP2040 device and you are connected to RP2350. \
                 Anything the connected device doesn't have will be skipped."
            ),
            "{}",
            d.body
        );
        assert!(
            d.body.contains(
                "EQ, crossover, delays, gains, routing and the DSP features are always applied."
            ),
            "{}",
            d.body
        );
        match &d.kind {
            crate::widgets::DialogKind::Checklist { items, .. } => {
                assert_eq!(items[0].0, "Volume levels (master and listening volume)");
                assert_eq!(
                    items[1].0,
                    "Hardware I/O (GPIO pins, clocks, ADAT, inputs, output limiters)"
                );
                assert!(!items[0].1 && !items[1].1, "both off by default");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(d.buttons[0].label, "Import");
        assert_eq!(d.buttons[1].label, "Cancel");
    }

    /// A file from this same platform gets no warning; saying one is there
    /// when it is not is as bad as leaving it out when it is.
    #[test]
    fn a_file_from_the_same_platform_carries_no_warning() {
        let s = state();
        let doc = document(r#"{"platform":"RP2350"}"#);
        let d = import_options_dialog(&doc, &s);
        assert!(!d.body.contains("This file came from"), "{}", d.body);

        let d = import_options_dialog(&document("{}"), &s);
        assert!(
            d.body
                .starts_with("Applies the settings in this file to the connected device."),
            "{}",
            d.body
        );
    }

    // ---------------------------------------------------------------- tools

    #[test]
    fn the_tools_dialogs_say_what_the_console_says() {
        assert_eq!(
            commit_dialog(2).body,
            "Save current parameters to preset slot 3?"
        );
        assert_eq!(
            revert_dialog().body,
            "Revert to last saved parameters?\n\nCurrent unsaved changes will be lost."
        );
        let r = factory_reset_dialog();
        assert!(r.critical, "a factory reset is a critical confirm");
        assert_eq!(r.default, 1, "Cancel is the default");
        assert_eq!(
            r.body,
            "Do you wish to clear all active parameters?\n\nThis will not overwrite your saved \
             parameters unless you run 'Commit Parameters'."
        );
        let f = firmware_dialog();
        assert!(f.critical);
        assert_eq!(f.buttons[0].label, "Reboot into Bootloader");
        assert_eq!(
            f.body,
            "This will reboot the device into bootloader mode.\n\nAudio output will stop \
             immediately. The device will appear as a USB drive to which you can drag a .uf2 \
             firmware file."
        );
    }

    // -------------------------------------------------- the bootloader handoff

    /// A world the test controls: the device is there until it is not, and the
    /// drive appears some time after that.
    struct FakeWorld {
        polls: std::cell::Cell<u32>,
        gone_after: u32,
        drive_after: Option<u32>,
    }

    impl BootProbe for FakeWorld {
        fn device_present(&self) -> bool {
            self.polls.get() < self.gone_after
        }
        /// Asked once per poll, in every phase, so this is where time passes.
        fn volume(&self) -> Option<PathBuf> {
            self.polls.set(self.polls.get() + 1);
            self.drive_after
                .filter(|n| self.polls.get() > *n)
                .map(|_| PathBuf::from("/Volumes/RPI-RP2"))
        }
    }

    #[test]
    fn the_bootloader_wait_watches_the_device_go_and_the_drive_arrive() {
        let world = FakeWorld {
            polls: std::cell::Cell::new(0),
            gone_after: 2,
            drive_after: Some(4),
        };
        let mut watch = BootloaderWatch::new();
        assert_eq!(watch.phase, BootPhase::WaitingForDisconnect);
        assert_eq!(watch.status(), "Waiting for the device to disconnect...");

        assert_eq!(watch.poll(&world), &BootPhase::WaitingForDisconnect);
        assert_eq!(watch.poll(&world), &BootPhase::WaitingForDisconnect);
        assert_eq!(watch.poll(&world), &BootPhase::WaitingForVolume);
        assert_eq!(watch.status(), "Waiting for the RPI-RP2 drive to appear...");
        assert!(!watch.finished());

        assert_eq!(watch.poll(&world), &BootPhase::WaitingForVolume);
        assert_eq!(
            watch.poll(&world),
            &BootPhase::Ready(PathBuf::from("/Volumes/RPI-RP2"))
        );
        assert!(watch.finished());
        assert!(
            watch
                .status()
                .contains("Copy the .uf2 firmware file to /Volumes/RPI-RP2"),
            "{}",
            watch.status()
        );
        // Once it has landed it stays landed, however long the loop runs.
        assert!(matches!(watch.poll(&world), BootPhase::Ready(_)));
    }

    /// A drive that never appears has to say so rather than spinning for ever.
    #[test]
    fn a_drive_that_never_appears_times_out_with_advice() {
        let world = FakeWorld {
            polls: std::cell::Cell::new(0),
            gone_after: 1,
            drive_after: None,
        };
        let mut watch = BootloaderWatch::with_limit(5);
        for _ in 0..5 {
            watch.poll(&world);
        }
        assert_eq!(watch.phase, BootPhase::TimedOut);
        assert!(watch.finished());
        assert!(watch.status().contains("BOOTSEL"), "{}", watch.status());
    }

    /// The drive can be mounted before the poll that notices the device left,
    /// which on a fast machine is the usual case.
    #[test]
    fn a_drive_that_appears_first_is_still_found() {
        let world = FakeWorld {
            polls: std::cell::Cell::new(0),
            gone_after: 10,
            drive_after: Some(0),
        };
        let mut watch = BootloaderWatch::new();
        assert!(matches!(watch.poll(&world), BootPhase::Ready(_)));
    }

    // ----------------------------------------------------------------- paths

    #[test]
    fn a_path_completes_the_way_a_shell_does() {
        let dir = std::env::temp_dir().join(format!("dspi-complete-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("tuning-room")).unwrap();
        std::fs::write(dir.join("tuning-a.txt"), "x").unwrap();
        std::fs::write(dir.join("tuning-b.txt"), "x").unwrap();
        std::fs::write(dir.join("other.txt"), "x").unwrap();

        let base = dir.to_string_lossy().to_string();
        // Several matches: the longest common prefix, and no further.
        assert_eq!(
            complete_path(&format!("{base}/tuning-")).as_deref(),
            Some(format!("{base}/tuning-").as_str())
        );
        // One match, a file: the whole name.
        assert_eq!(
            complete_path(&format!("{base}/tuning-a")).as_deref(),
            Some(format!("{base}/tuning-a.txt").as_str())
        );
        // One match, a directory: the name and a separator, so the next Tab
        // carries on inside it.
        assert_eq!(
            complete_path(&format!("{base}/tuning-r")).as_deref(),
            Some(format!("{base}/tuning-room/").as_str())
        );
        assert_eq!(complete_path(&format!("{base}/nothing")), None);
        assert_eq!(complete_path("/no/such/directory/x"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tilde_is_expanded_but_not_written_back() {
        let home = std::env::var("HOME").unwrap_or_default();
        assert_eq!(
            expand("~/tuning.txt"),
            PathBuf::from(home).join("tuning.txt")
        );
        assert_eq!(expand("/tmp/x"), PathBuf::from("/tmp/x"));
        assert_eq!(expand("x"), PathBuf::from("x"));
    }
}
