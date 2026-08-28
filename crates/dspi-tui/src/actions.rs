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
use dspi_session::{DeviceState, Notification, Session, Source};

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

/// `REQ_RESET_BUFFER_STATS`, the panel's `r`.
pub fn reset_watermarks(session: &mut Session) -> Result<(), String> {
    session
        .write("diag.buffers.reset", &[], dspi_proto::value::Value::Trigger)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

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
