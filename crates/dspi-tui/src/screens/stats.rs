//! Stats for Nerbs: `StatsView.swift` as a tool panel.
//!
//! Every section is the Console's, in the Console's order, with its labels and
//! its value strings. What is on screen is a projection of
//! [`actions::Stats`](crate::actions::Stats), which the runner refreshes every
//! two seconds while this panel is open; the panel itself never talks to the
//! device, so a stale reading is visibly stale rather than silently invented.
//!
//! A section whose read stalled is not drawn at all, which is how the Console
//! hides ADAT on an RP2040 and the S/PDIF section on firmware without it:
//! absent features are removed, not disabled.

use crossterm::event::{KeyCode, KeyEvent};
use dspi_session::DeviceState;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

use super::Shared;
use super::panel::{self, Header};
use crate::actions::{FillHistory, Stats};
use crate::shell::{Screen, ScreenEvent};
use crate::theme::{Glyphs, Theme};
use crate::widgets::text::{fit_left, fit_right, truncate};
use crate::widgets::{KeyHelp, SectionHeader, StatusPill, StatusTone};

const KEYS: &[KeyHelp] = &[
    KeyHelp::new("↑ ↓", "Scroll"),
    KeyHelp::new("PgUp PgDn", "Page"),
    KeyHelp::new("r", "Reset watermarks"),
];

/// The Console's footer, `StatsView.swift`.
pub const FOOTER: &str = "Updated every 2 seconds";

/// How the Console colours a buffer fill, by which buffer it is
/// (`BufferFillThresholds`). Green is healthy, amber is drifting, red is at an
/// edge where audio is about to break.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillKind {
    Spdif,
    PdmDma,
    PdmRing,
}

impl FillKind {
    pub fn tone(self, pct: u8) -> StatusTone {
        let v = pct as i32;
        match self {
            FillKind::Spdif => {
                if v == 0 || v == 100 {
                    StatusTone::Danger
                } else if !(25..=75).contains(&v) {
                    StatusTone::Warning
                } else {
                    StatusTone::Ok
                }
            }
            FillKind::PdmDma => {
                if v > 50 {
                    StatusTone::Danger
                } else if !(5..=30).contains(&v) {
                    StatusTone::Warning
                } else {
                    StatusTone::Ok
                }
            }
            FillKind::PdmRing => {
                if v > 50 {
                    StatusTone::Danger
                } else if v > 20 {
                    StatusTone::Warning
                } else {
                    StatusTone::Ok
                }
            }
        }
    }
}

/// One line of the panel.
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    Section(String),
    Blank,
    /// `Label ......... value`.
    Info {
        label: String,
        value: String,
    },
    /// A label with a state pill, the Console's coloured dot and word.
    Pill {
        label: String,
        text: String,
        tone: StatusTone,
    },
    /// The over/under counter pair, with its "Core 0 → Core 1" subtitle.
    Counters {
        label: String,
        detail: String,
        over: Option<u32>,
        under: Option<u32>,
    },
    /// A fill percentage with its min-max watermarks and the last few
    /// readings as a sparkline, oldest first.
    Fill {
        label: String,
        pct: u8,
        min: u8,
        max: u8,
        kind: FillKind,
        history: Vec<Option<u8>>,
    },
    /// A row of little state dots: Audio Streaming, PDM Active.
    Flags(Vec<(String, bool)>),
    /// The total, with the Console's red delta badge when it moved.
    Starvation {
        total: u32,
        delta: u32,
    },
}

impl Row {
    fn info(label: &str, value: impl Into<String>) -> Self {
        Row::Info {
            label: label.into(),
            value: value.into(),
        }
    }
    fn section(title: &str) -> Self {
        Row::Section(title.into())
    }
}

/// The Console's `stateString` tables, one per status packet.
fn spdif_state(state: dspi_proto::enums::SpdifRxState) -> (&'static str, StatusTone) {
    use dspi_proto::enums::SpdifRxState as S;
    match state {
        S::Inactive => ("Inactive", StatusTone::Neutral),
        S::Acquiring => ("Acquiring", StatusTone::Warning),
        S::Locked => ("Locked", StatusTone::Ok),
        S::Relocking => ("Relocking", StatusTone::Warning),
        _ => ("Unknown", StatusTone::Neutral),
    }
}

fn i2s_state(state: u8) -> (String, StatusTone) {
    match state {
        0 => ("Inactive".into(), StatusTone::Neutral),
        1 => ("Acquiring".into(), StatusTone::Warning),
        2 => ("Relocking".into(), StatusTone::Warning),
        3 => ("Locked".into(), StatusTone::Ok),
        other => (format!("Unknown ({other})"), StatusTone::Neutral),
    }
}

fn lib_state(state: u8) -> String {
    match state {
        0 => "No Signal".into(),
        1 => "Waiting Stable".into(),
        2 => "Stable".into(),
        other => format!("Unknown ({other})"),
    }
}

/// The Console's `sourceString`: which input the receiver is listening to.
///
/// The optional S/PDIF inputs are raw 4 to 6, contiguous from
/// `INPUT_SOURCE_SPDIF2` (survey 6.6), and have no named variant because the
/// firmware's own enum stops at ADAT.
fn source_name(s: dspi_proto::enums::InputSource) -> &'static str {
    match s.to_raw() {
        1 => "S/PDIF 1",
        2 => "I2S",
        4 => "S/PDIF 2",
        5 => "S/PDIF 3",
        6 => "S/PDIF 4",
        _ => "USB",
    }
}

pub struct StatsPanel {
    shared: Shared,
    first: usize,
}

impl StatsPanel {
    pub fn new(shared: Shared) -> Self {
        Self { shared, first: 0 }
    }

    /// The panel's three columns, as the Console lays them out
    /// (`StatsView.swift:673-734`): device facts and output counters, then
    /// buffer health, then the optional inputs and interfaces, which is empty
    /// on a device that reports none of them.
    pub fn columns(&self, state: &DeviceState, s: &Stats) -> [Vec<Row>; 3] {
        let mut a = vec![
            Row::section("Device Information"),
            Row::info(
                "Platform",
                format!("{:?}", state.caps.platform).to_uppercase(),
            ),
            Row::info("Firmware", format!("v{}", state.caps.firmware)),
            // `REQ_GET_BUILD_INFO` (config.h:326): the build's `git describe`
            // and date, which name a development build the version cannot.
            Row::info(
                "Build",
                state
                    .caps
                    .build_info
                    .as_ref()
                    .map_or_else(|| "-".into(), |b| b.describe.clone()),
            ),
            Row::info(
                "Build Date",
                state
                    .caps
                    .build_info
                    .as_ref()
                    .map_or_else(|| "-".into(), |b| b.date.clone()),
            ),
            Row::info(
                "Serial",
                if state.caps.serial.is_empty() {
                    "-".into()
                } else {
                    state.caps.serial.clone()
                },
            ),
            Row::info("Reconnects", s.reconnects.to_string()),
            Row::Blank,
            Row::section("System Information"),
            Row::info(
                "Clock Frequency",
                format!("{:.1} MHz", s.clock_hz as f64 / 1_000_000.0),
            ),
            Row::info(
                "Core Voltage",
                format!("{:.2} V", s.core_mv as f64 / 1000.0),
            ),
            Row::info(
                "Sample Rate",
                format!("{:.1} kHz", s.sample_rate_hz as f64 / 1000.0),
            ),
            Row::info(
                "Temperature",
                format!("{:.1} °C", s.temp_centi_c as f64 / 100.0),
            ),
        ];

        // `survey-console.md` 2.20 names core1 mode among the System
        // Information rows. Firmware that stalls `REQ_GET_CORE1_MODE` leaves it
        // `None`, and an absent reading takes its row with it.
        if let Some(mode) = s.core1 {
            a.push(Row::info("Core 1 Mode", mode.label()));
        }

        a.push(Row::Blank);
        a.push(Row::section("Audio Output"));
        a.push(Row::Counters {
            label: "USB Ring".into(),
            detail: "ISR → Main Loop".into(),
            over: Some(s.usb_ring_over),
            under: None,
        });
        a.push(Row::Counters {
            label: "Buffer Pool".into(),
            detail: "USB → DMA".into(),
            over: Some(s.spdif_over),
            under: Some(s.spdif_under),
        });

        a.push(Row::Blank);
        a.push(Row::section("PDM (Subwoofer)"));
        a.push(Row::Counters {
            label: "Ring Buffer".into(),
            detail: "Core 0 → Core 1".into(),
            over: Some(s.pdm_ring_over),
            under: Some(s.pdm_ring_under),
        });
        a.push(Row::Counters {
            label: "DMA Buffer".into(),
            detail: "Core 1 → PIO".into(),
            over: Some(s.pdm_dma_over),
            under: Some(s.pdm_dma_under),
        });

        let mut mid = vec![
            Row::section("SPDIF DMA Starvation"),
            Row::Starvation {
                total: s.starvation_total,
                delta: s.starvation_delta,
            },
        ];
        let instances = s
            .buffers
            .as_ref()
            .map(|b| (b.num_spdif as usize).max(2))
            .unwrap_or(2)
            .min(4);
        for i in 0..instances {
            mid.push(Row::info(
                &format!("Out {}/{}", i * 2 + 1, i * 2 + 2),
                s.starvation_per_instance[i].to_string(),
            ));
        }
        mid.push(Row::info(
            "Time since last event",
            polls_text(s.polls_since_starvation),
        ));
        mid.push(Row::info(
            "Time between last two",
            polls_text(s.polls_between_starvations),
        ));

        if let Some(b) = &s.buffers {
            mid.push(Row::Blank);
            mid.push(Row::section("Buffer Fill Levels"));
            mid.push(Row::Flags(vec![
                ("Audio Streaming".into(), b.streaming()),
                ("PDM Active".into(), b.pdm_active()),
            ]));
            for i in 0..(b.num_spdif as usize).min(4) {
                let f = &b.spdif[i];
                mid.push(Row::Fill {
                    label: format!("Out {}/{}", i * 2 + 1, i * 2 + 2),
                    pct: f.consumer_fill_pct,
                    min: f.consumer_min_fill_pct,
                    max: f.consumer_max_fill_pct,
                    kind: FillKind::Spdif,
                    history: s.fill_history.series[i].clone(),
                });
            }
            if b.pdm_active() {
                mid.push(Row::Fill {
                    label: "PDM DMA".into(),
                    pct: b.pdm.dma_fill_pct,
                    min: b.pdm.dma_min_fill_pct,
                    max: b.pdm.dma_max_fill_pct,
                    kind: FillKind::PdmDma,
                    history: s.fill_history.series[FillHistory::PDM_DMA].clone(),
                });
                mid.push(Row::Fill {
                    label: "PDM Ring".into(),
                    pct: b.pdm.ring_fill_pct,
                    min: b.pdm.ring_min_fill_pct,
                    max: b.pdm.ring_max_fill_pct,
                    kind: FillKind::PdmRing,
                    history: s.fill_history.series[FillHistory::PDM_RING].clone(),
                });
            }
        }

        mid.push(Row::Blank);
        // The Console's button, with the key that presses it in the value
        // column where every other row's value is. It read backwards before.
        mid.push(Row::info("Reset Watermarks", "r"));

        let mut c = Vec::new();
        if let Some(rx) = &s.spdif_rx {
            let (word, tone) = spdif_state(rx.state);
            let locked = rx.state == dspi_proto::enums::SpdifRxState::Locked;
            c.push(Row::Blank);
            c.push(Row::section("S/PDIF Input"));
            c.push(Row::Pill {
                label: "State".into(),
                text: word.into(),
                tone,
            });
            c.push(Row::info("Active Source", source_name(rx.input_source)));
            c.push(Row::info(
                "Sample Rate",
                if locked && rx.sample_rate > 0 {
                    format!("{:.1} kHz", rx.sample_rate as f64 / 1000.0)
                } else {
                    "-".into()
                },
            ));
            c.push(Row::info("Lock Count", rx.lock_count.to_string()));
            c.push(Row::info("Loss Count", rx.loss_count.to_string()));
            c.push(Row::info(
                "Parity Errors",
                if locked {
                    rx.parity_errors.to_string()
                } else {
                    "-".into()
                },
            ));
            c.push(Row::info(
                "FIFO Fill",
                if locked {
                    format!("{}%", rx.fifo_fill_pct)
                } else {
                    "-".into()
                },
            ));
            c.push(Row::info(
                "RX Pin",
                match s.spdif_rx_pin {
                    Some(p) => format!("GPIO {p}"),
                    None => "-".into(),
                },
            ));

            if let Some(cs) = &s.spdif_channel {
                c.push(Row::section("Channel Status"));
                c.push(Row::info(
                    "Format",
                    if cs.is_consumer() {
                        "Consumer"
                    } else {
                        "Professional"
                    },
                ));
                c.push(Row::info(
                    "Audio",
                    if cs.is_pcm() { "PCM" } else { "Non-PCM" },
                ));
                c.push(Row::info("Category", cs.category()));
                c.push(Row::info("Word Length", cs.word_length()));
                c.push(Row::info(
                    "Copy",
                    if cs.copy_permitted() {
                        "Permitted"
                    } else {
                        "Prohibited"
                    },
                ));
            }

            if rx.state != dspi_proto::enums::SpdifRxState::Inactive {
                c.push(Row::section("Debug"));
                c.push(Row::info("Library State", lib_state(rx.lib_state)));
                // The two counts share a byte, high nibble then low.
                c.push(Row::info(
                    "Stable Callbacks",
                    ((rx.callback_counts >> 4) & 0x0F).to_string(),
                ));
                c.push(Row::info(
                    "Lost Callbacks",
                    (rx.callback_counts & 0x0F).to_string(),
                ));
            }
        }

        if let Some(lg) = &s.lg {
            c.push(Row::Blank);
            c.push(Row::section("LG Sound Sync"));
            c.push(Row::info("Enabled", yes_no(lg.enabled)));
            c.push(Row::Pill {
                label: "Present".into(),
                text: yes_no(lg.present).into(),
                tone: if lg.present {
                    StatusTone::Ok
                } else {
                    StatusTone::Neutral
                },
            });
            c.push(Row::info(
                "TV Volume",
                if lg.volume == dspi_proto::packets::LgSoundSyncStatus::VOLUME_UNKNOWN {
                    "-".into()
                } else {
                    format!("{} / 100", lg.volume)
                },
            ));
            c.push(Row::info("TV Mute", on_off(lg.muted)));
        }

        if let Some(a) = &s.adat {
            c.push(Row::Blank);
            c.push(Row::section("ADAT Bulk Output"));
            c.push(Row::Pill {
                label: "State".into(),
                text: if a.enabled { "Enabled" } else { "Disabled" }.into(),
                tone: if a.active {
                    StatusTone::Ok
                } else if a.enabled {
                    StatusTone::Warning
                } else {
                    StatusTone::Neutral
                },
            });
            c.push(Row::info("Streaming", yes_no(a.active)));
            c.push(Row::info("Rate Supported", yes_no(a.rate_ok)));
            c.push(Row::info("Data Pin", format!("GPIO {}", a.pin)));
            c.push(Row::info("Resync Count", a.resync_count.to_string()));
            c.push(Row::info("Slip Count", a.slip_count.to_string()));
        }

        if let Some(i) = &s.i2s_slave {
            let (word, tone) = i2s_state(i.state);
            c.push(Row::Blank);
            c.push(Row::section("I2S Input (Slave Clock)"));
            c.push(Row::Pill {
                label: "State".into(),
                text: word,
                tone,
            });
            c.push(Row::info(
                "Detected Rate",
                if i.detected_rate > 0 {
                    format!("{:.1} kHz", i.detected_rate as f64 / 1000.0)
                } else {
                    "-".into()
                },
            ));
            c.push(Row::info(
                "Measured Rate",
                if i.measured_hz > 0 {
                    format!("{} Hz", i.measured_hz)
                } else {
                    "-".into()
                },
            ));
            c.push(Row::info("Lock Count", i.lock_count.to_string()));
            c.push(Row::info("Loss Count", i.loss_count.to_string()));
        }

        // Each section opens with a gap; the column's first does not need one.
        if c.first() == Some(&Row::Blank) {
            c.remove(0);
        }
        [a, mid, c]
    }

    /// Every row in one column, for a pane too narrow for more: the
    /// Console's columns in order.
    pub fn rows(&self, state: &DeviceState, s: &Stats) -> Vec<Row> {
        let [a, b, c] = self.columns(state, s);
        let mut rows = a;
        for col in [b, c] {
            if !col.is_empty() {
                rows.push(Row::Blank);
                rows.extend(col);
            }
        }
        rows
    }
}

/// A reading from 0 to 100 % in eight steps.
const SPARK: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
const SPARK_ASCII: [&str; 8] = ["_", ".", ",", "-", "~", "=", "*", "#"];

/// The narrowest column that still reads: a fill row's name, eight
/// readings and `100-100%  100%`, or `Time between last two` and its time.
const MIN_COL: u16 = 36;

/// The columns the page is drawn in at `width`: the Console's three when
/// they fit, else two, else one. Two columns keep the Console's order and
/// break it where the taller column is shortest.
fn arrange(width: u16, cols: [Vec<Row>; 3]) -> Vec<Vec<Row>> {
    let [a, b, c] = cols;
    let join = |mut x: Vec<Row>, y: Vec<Row>| {
        if !y.is_empty() {
            x.push(Row::Blank);
            x.extend(y);
        }
        x
    };
    if c.is_empty() {
        return if width > 2 * MIN_COL {
            vec![a, b]
        } else {
            vec![join(a, b)]
        };
    }
    if width > 3 * MIN_COL + 1 {
        vec![a, b, c]
    } else if width > 2 * MIN_COL {
        let first = (a.len() + b.len() + 1).max(c.len());
        let second = a.len().max(b.len() + c.len() + 1);
        if first <= second {
            vec![join(a, b), c]
        } else {
            vec![a, join(b, c)]
        }
    } else {
        vec![join(join(a, b), c)]
    }
}

fn yes_no(v: bool) -> &'static str {
    if v { "Yes" } else { "No" }
}

fn on_off(v: bool) -> &'static str {
    if v { "On" } else { "Off" }
}

/// The Console shows an elapsed clock here; the panel counts its own two-second
/// polls, which says the same thing without a wall clock the tests cannot pin.
fn polls_text(polls: Option<u32>) -> String {
    match polls {
        None => "-".into(),
        Some(n) => {
            let secs = n * 2;
            if secs >= 3600 {
                format!(
                    "{}h {:02}m {:02}s",
                    secs / 3600,
                    (secs % 3600) / 60,
                    secs % 60
                )
            } else {
                format!("{}m {:02}s", secs / 60, secs % 60)
            }
        }
    }
}

fn draw_row(area: Rect, buf: &mut Buffer, theme: &Theme, row: &Row) {
    let w = area.width as usize;
    match row {
        Row::Blank => {}
        Row::Section(title) => SectionHeader::new(title, theme).render(area, buf),
        Row::Info { label, value } => {
            // `Label ......... value`: the leader is what keeps a long column
            // of pairs readable at a glance. A value too long for a narrow
            // column (a development build's describe) gives way to its label.
            let value = &truncate(value, w.saturating_sub(label.chars().count() + 4).max(1));
            let used = label.chars().count() + value.chars().count() + 4;
            buf.set_string(
                area.x + 1,
                area.y,
                truncate(label, w.saturating_sub(2)),
                theme.value(),
            );
            if w > used {
                let dots = "·".repeat(w - used);
                buf.set_string(
                    area.x + 2 + label.chars().count() as u16,
                    area.y,
                    &dots,
                    theme.label(),
                );
            }
            buf.set_string(
                area.x + area.width - value.chars().count() as u16 - 1,
                area.y,
                value,
                theme.value(),
            );
        }
        Row::Pill { label, text, tone } => {
            buf.set_string(area.x + 1, area.y, label, theme.value());
            let pill = StatusPill::new(text, *tone, theme);
            let pw = pill.width();
            if pw + 2 < area.width {
                pill.render(Rect::new(area.x + area.width - pw - 1, area.y, pw, 1), buf);
            }
        }
        Row::Counters {
            label,
            detail,
            over,
            under,
        } => {
            buf.set_string(area.x + 1, area.y, label, theme.value());
            let numbers = match (over, under) {
                (Some(o), Some(u)) => format!("{o} over  {u} under"),
                (Some(o), None) => format!("{o} over"),
                _ => String::new(),
            };
            let bad = over.is_some_and(|o| o > 0) || under.is_some_and(|u| u > 0);
            buf.set_string(
                area.x + area.width - numbers.chars().count() as u16 - 1,
                area.y,
                &numbers,
                if bad {
                    Style::default().fg(theme.warning)
                } else {
                    theme.value()
                },
            );
            let dx = area.x + 3 + label.chars().count() as u16;
            let room = (area.width as usize)
                .saturating_sub(numbers.chars().count() + label.chars().count() + 6);
            buf.set_string(dx, area.y, truncate(detail, room), theme.label());
        }
        Row::Fill {
            label,
            pct,
            min,
            max,
            kind,
            history,
        } => {
            // The Console's row: name, then the history, then the watermarks
            // (lowest and highest since the last reset) and the reading in
            // its threshold colour (`StatsView.swift`, `BufferFillRow`).
            let tone_color = |v: u8| match kind.tone(v) {
                StatusTone::Ok => theme.ok,
                StatusTone::Warning => theme.warning,
                StatusTone::Danger => theme.danger,
                StatusTone::Neutral => theme.dim,
            };
            let marks = format!("{min}-{max}%");
            let reading = format!("{pct:>3}%");
            buf.set_string(area.x + 1, area.y, fit_left(label, 9), theme.value());
            let right = area.x + area.width - 1;
            let rx = right.saturating_sub(reading.chars().count() as u16);
            buf.set_string(rx, area.y, &reading, Style::default().fg(tone_color(*pct)));
            let mx = rx.saturating_sub(marks.chars().count() as u16 + 2);
            buf.set_string(mx, area.y, &marks, theme.label());
            // One cell per two-second reading, newest on the right, each in
            // the colour its reading had.
            let sx = area.x + 11;
            let room = mx.saturating_sub(sx + 1) as usize;
            let shown = &history[history.len().saturating_sub(room.min(FillHistory::LEN))..];
            for (i, v) in shown.iter().enumerate() {
                let Some(v) = v else { continue };
                let level = (*v as usize * 7 + 50) / 100;
                let sym = if theme.glyphs == Glyphs::Ascii {
                    SPARK_ASCII[level.min(7)]
                } else {
                    SPARK[level.min(7)]
                };
                buf[(sx + i as u16, area.y)]
                    .set_symbol(sym)
                    .set_style(Style::default().fg(tone_color(*v)));
            }
        }
        Row::Flags(flags) => {
            let mut x = area.x + 1;
            for (name, on) in flags {
                let dot = match (on, theme.glyphs) {
                    (_, Glyphs::Ascii) => {
                        if *on {
                            "*"
                        } else {
                            "o"
                        }
                    }
                    (true, _) => "●",
                    (false, _) => "○",
                };
                let text = format!("{dot} {name}");
                if x + text.chars().count() as u16 >= area.x + area.width {
                    break;
                }
                buf.set_string(
                    x,
                    area.y,
                    &text,
                    if *on {
                        Style::default().fg(theme.ok)
                    } else {
                        theme.label()
                    },
                );
                x += text.chars().count() as u16 + 3;
            }
        }
        Row::Starvation { total, delta } => {
            buf.set_string(area.x + 1, area.y, "Total", theme.value());
            let number = total.to_string();
            let badge = if *delta > 0 {
                format!(" +{delta} ")
            } else {
                String::new()
            };
            let right = area.x + area.width - number.chars().count() as u16 - 1;
            buf.set_string(
                right,
                area.y,
                &number,
                if *total > 0 {
                    Style::default().fg(theme.danger)
                } else {
                    theme.value()
                },
            );
            if !badge.is_empty() {
                buf.set_string(
                    right - badge.chars().count() as u16 - 1,
                    area.y,
                    &badge,
                    theme.pill(theme.danger),
                );
            }
        }
    }
}

impl Screen for StatsPanel {
    fn title(&self) -> String {
        "System Statistics".into()
    }

    fn draw(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        state: &DeviceState,
        _focused: bool,
    ) {
        if area.height == 0 {
            return;
        }
        let stats = self.shared.borrow().stats.clone();
        let columns = arrange(area.width, self.columns(state, &stats));
        let header = Header::new(FOOTER);
        panel::draw_header(area, buf, theme, &header, false);
        if area.height < 2 {
            return;
        }
        let body = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
        // Every row is one line tall, so the window is a plain clamp, and
        // the columns scroll together.
        let longest = columns.iter().map(Vec::len).max().unwrap_or(0);
        self.first = self.first.min(longest.saturating_sub(body.height as usize));
        let n = columns.len() as u16;
        let col_w = (body.width + 1) / n - 1;
        for (k, rows) in columns.iter().enumerate() {
            let x = body.x + k as u16 * (col_w + 1);
            // The last column takes whatever the division left over.
            let w = if k as u16 == n - 1 {
                body.x + body.width - x
            } else {
                col_w
            };
            if k > 0 {
                for y in body.y..body.y + body.height {
                    buf[(x - 1, y)]
                        .set_symbol(if theme.glyphs == Glyphs::Ascii {
                            "|"
                        } else {
                            "│"
                        })
                        .set_style(theme.chrome_style());
                }
            }
            for (i, row) in rows.iter().enumerate().skip(self.first) {
                let y = body.y + (i - self.first) as u16;
                if y >= body.y + body.height {
                    break;
                }
                draw_row(Rect::new(x, y, w, 1), buf, theme, row);
            }
        }
        if !stats.read {
            buf.set_string(
                body.x + 1,
                body.y + body.height.saturating_sub(1),
                fit_right("waiting for the first reading", body.width as usize - 2),
                theme.label(),
            );
        }
    }

    fn handle(&mut self, key: KeyEvent, _state: &DeviceState) -> ScreenEvent {
        match key.code {
            KeyCode::Up => {
                self.first = self.first.saturating_sub(1);
                ScreenEvent::Handled
            }
            KeyCode::Down => {
                self.first += 1;
                ScreenEvent::Handled
            }
            KeyCode::PageUp => {
                self.first = self.first.saturating_sub(10);
                ScreenEvent::Handled
            }
            KeyCode::PageDown => {
                self.first += 10;
                ScreenEvent::Handled
            }
            KeyCode::Home => {
                self.first = 0;
                ScreenEvent::Handled
            }
            // `diag.buffers.reset` is a trigger, so it goes through the same
            // command path as everything else and lands on the echo line.
            KeyCode::Char('r') => ScreenEvent::Command("diag.buffers.reset".into()),
            _ => ScreenEvent::Unhandled,
        }
    }

    fn keys(&self) -> &'static [KeyHelp] {
        KEYS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::panel::testing;
    use crate::screens::shared;
    use crate::shell::Tool;
    use crate::widgets::testing::key;
    use dspi_proto::packets;

    fn stats() -> Stats {
        Stats {
            read: true,
            clock_hz: 300_000_000,
            core_mv: 1150,
            sample_rate_hz: 48_000,
            temp_centi_c: 4231,
            core1: Some(dspi_proto::enums::Core1Mode::EqWorker),
            pdm_ring_over: 0,
            pdm_ring_under: 3,
            usb_ring_over: 1,
            starvation_total: 7,
            starvation_delta: 2,
            starvation_per_instance: [4, 3, 0, 0],
            polls_since_starvation: Some(0),
            polls_between_starvations: Some(15),
            buffers: Some(packets::BufferStatsPacket {
                num_spdif: 2,
                flags: packets::BufferStatsPacket::FLAG_STREAMING
                    | packets::BufferStatsPacket::FLAG_PDM_ACTIVE,
                sequence: 9,
                spdif: [
                    packets::SpdifBufferStats {
                        consumer_fill_pct: 50,
                        consumer_min_fill_pct: 20,
                        consumer_max_fill_pct: 80,
                        ..Default::default()
                    },
                    packets::SpdifBufferStats {
                        consumer_fill_pct: 100,
                        consumer_min_fill_pct: 40,
                        consumer_max_fill_pct: 100,
                        ..Default::default()
                    },
                    Default::default(),
                    Default::default(),
                ],
                pdm: packets::PdmBufferStats {
                    dma_fill_pct: 20,
                    dma_min_fill_pct: 10,
                    dma_max_fill_pct: 35,
                    ring_fill_pct: 12,
                    ring_min_fill_pct: 8,
                    ring_max_fill_pct: 60,
                },
            }),
            spdif_rx: Some(packets::SpdifRxStatusPacket {
                state: dspi_proto::enums::SpdifRxState::Locked,
                input_source: dspi_proto::enums::InputSource::from_raw(4),
                lock_count: 2,
                loss_count: 1,
                sample_rate: 44_100,
                parity_errors: 0,
                fifo_fill_pct: 55,
                lib_state: 2,
                callback_counts: 0x21,
            }),
            spdif_channel: Some(crate::actions::ChannelStatus {
                raw: {
                    let mut r = [0u8; 24];
                    r[0] = 0x04; // consumer, PCM, copy permitted
                    r[1] = 0x01; // CD Player
                    r[4] = 0x0B; // 24-bit
                    r
                },
            }),
            spdif_rx_pin: Some(20),
            lg: Some(packets::LgSoundSyncStatus {
                enabled: true,
                present: true,
                volume: 42,
                muted: false,
            }),
            adat: Some(packets::AdatStatus {
                enabled: true,
                active: true,
                pin: 6,
                rate_ok: true,
                resync_count: 1,
                slip_count: 0,
            }),
            i2s_slave: Some(packets::I2sSlaveStatusPacket {
                state: 3,
                clock_mode: 1,
                lock_count: 1,
                loss_count: 0,
                detected_rate: 48_000,
                measured_hz: 47_998,
            }),
            ..Default::default()
        }
    }

    fn panel() -> (StatsPanel, DeviceState, crate::screens::Shared) {
        let shared = shared();
        shared.borrow_mut().stats = stats();
        (StatsPanel::new(shared.clone()), testing::state(), shared)
    }

    /// Every section the Console has, with its labels and its value strings.
    #[test]
    fn every_section_is_there_with_the_consoles_wording() {
        let (mut p, state, _) = panel();
        let f = testing::draw(&mut p, &state, 100, 90);
        for want in [
            "DEVICE INFORMATION",
            "SYSTEM INFORMATION",
            "S/PDIF INPUT",
            "CHANNEL STATUS",
            "DEBUG",
            "LG SOUND SYNC",
            "ADAT BULK OUTPUT",
            "I2S INPUT (SLAVE CLOCK)",
            "PDM (SUBWOOFER)",
            "AUDIO OUTPUT",
            "SPDIF DMA STARVATION",
            "BUFFER FILL LEVELS",
        ] {
            assert!(f.contains(want), "missing section {want}:\n{f}");
        }
        for want in [
            "Clock Frequency",
            "300.0 MHz",
            "1.15 V",
            "48.0 kHz",
            "42.3 °C",
            "Core 1 Mode",
            "EQ worker",
            "Active Source",
            "S/PDIF 2",
            "44.1 kHz",
            "GPIO 20",
            "CD Player",
            "24-bit",
            "Permitted",
            "Library State",
            "Stable Callbacks",
            "TV Volume",
            "42 / 100",
            "Resync Count",
            "Measured Rate",
            "47998 Hz",
            "Core 0 → Core 1",
            "USB → DMA",
            "Time since last event",
            "Time between last two",
            "Audio Streaming",
            "PDM Active",
            "Reset Watermarks",
        ] {
            assert!(f.contains(want), "missing row {want}:\n{f}");
        }
        assert!(f.contains(FOOTER), "the refresh cadence: {f}");
    }

    /// D42: `survey-console.md` 2.20 lists core1 mode in System Information,
    /// and `REQ_GET_CORE1_MODE` (0x7A) has been in the registry all along.
    #[test]
    fn the_core_one_mode_row_names_what_the_second_core_is_doing() {
        use dspi_proto::enums::Core1Mode;
        let (mut p, state, shared) = panel();
        for (mode, want) in [
            (Core1Mode::Idle, "Idle"),
            (Core1Mode::Pdm, "PDM"),
            (Core1Mode::EqWorker, "EQ worker"),
            // Open enum: an unknown mode says its number rather than lying.
            (Core1Mode::from_raw(9), "type 9, unrecognised"),
        ] {
            shared.borrow_mut().stats.core1 = Some(mode);
            let f = testing::draw(&mut p, &state, 100, 90);
            assert!(
                f.contains("Core 1 Mode") && f.contains(want),
                "{mode:?}: {f}"
            );
        }
        // Firmware that stalls the opcode loses the row, as every other
        // stalled read here does.
        shared.borrow_mut().stats.core1 = None;
        let f = testing::draw(&mut p, &state, 100, 90);
        assert!(!f.contains("Core 1 Mode"), "{f}");
    }

    /// A stalled read means the firmware does not have the feature, and the
    /// Console removes the section rather than showing it full of zeros.
    #[test]
    fn a_section_the_firmware_stalled_is_not_drawn() {
        let shared = shared();
        shared.borrow_mut().stats = Stats {
            read: true,
            ..Default::default()
        };
        let mut p = StatsPanel::new(shared);
        let f = testing::draw(&mut p, &testing::state(), 100, 60);
        assert!(!f.contains("ADAT"), "{f}");
        assert!(!f.contains("LG SOUND SYNC"), "{f}");
        assert!(!f.contains("S/PDIF INPUT"), "{f}");
        assert!(!f.contains("BUFFER FILL LEVELS"), "{f}");
        assert!(f.contains("DEVICE INFORMATION"), "{f}");
    }

    /// The I2S section is the Console's one role-gated section: in master mode
    /// the numbers mean nothing, so it is not shown.
    #[test]
    fn the_fill_thresholds_are_the_consoles() {
        assert_eq!(FillKind::Spdif.tone(0), StatusTone::Danger);
        assert_eq!(FillKind::Spdif.tone(100), StatusTone::Danger);
        assert_eq!(FillKind::Spdif.tone(20), StatusTone::Warning);
        assert_eq!(FillKind::Spdif.tone(50), StatusTone::Ok);
        assert_eq!(FillKind::PdmDma.tone(60), StatusTone::Danger);
        assert_eq!(FillKind::PdmDma.tone(4), StatusTone::Warning);
        assert_eq!(FillKind::PdmDma.tone(20), StatusTone::Ok);
        assert_eq!(FillKind::PdmRing.tone(25), StatusTone::Warning);
        assert_eq!(FillKind::PdmRing.tone(10), StatusTone::Ok);
    }

    /// The Console's row: the watermarks, then the reading, and before
    /// them the last readings as a sparkline, newest on the right.
    #[test]
    fn a_fill_row_carries_its_watermarks_and_its_history() {
        let (mut p, state, shared) = panel();
        let f = testing::draw(&mut p, &state, 100, 90);
        assert!(f.contains("20-80%   50%"), "{f}");
        let mut b = shared.borrow().stats.buffers.clone().unwrap();
        for pct in [0u8, 50, 100] {
            b.spdif[0].consumer_fill_pct = pct;
            shared.borrow_mut().stats.fill_history.push(Some(&b));
        }
        let f = testing::draw(&mut p, &state, 100, 90);
        let row = f.lines().find(|l| l.contains("20-80%")).unwrap();
        assert!(row.contains("▁▅█"), "{row}");
        // Nothing yet for a buffer with no readings.
        let row = f.lines().find(|l| l.contains("8-60%")).unwrap();
        assert!(!row.contains('▁'), "{row}");
    }

    /// Columns when the pane allows: three at a wide terminal, two at
    /// 120x40, one at 80x24, and never a column narrower than a fill row.
    #[test]
    fn the_page_takes_columns_when_the_width_allows() {
        let (p, state, shared) = panel();
        let stats = shared.borrow().stats.clone();
        let cols = |w: u16| arrange(w, p.columns(&state, &stats)).len();
        assert_eq!(cols(56), 1, "the 80x24 pane");
        assert_eq!(cols(90), 2, "the 120x40 pane");
        assert_eq!(cols(130), 3);
        // Without interface sections there is no third column.
        let mut bare = stats.clone();
        (bare.spdif_rx, bare.lg, bare.adat, bare.i2s_slave) = (None, None, None, None);
        assert_eq!(arrange(130, p.columns(&state, &bare)).len(), 2);
        // The Console's column order: device and outputs, buffers, inputs.
        let [a, b, c] = p.columns(&state, &stats);
        assert_eq!(a[0], Row::section("Device Information"));
        assert_eq!(b[0], Row::section("SPDIF DMA Starvation"));
        assert_eq!(c[0], Row::section("S/PDIF Input"));
        let audio = a.iter().position(|r| *r == Row::section("Audio Output"));
        let pdm = a.iter().position(|r| *r == Row::section("PDM (Subwoofer)"));
        assert!(audio < pdm, "audio output before PDM");
    }

    /// B1 reads `REQ_GET_BUILD_INFO`; the device section shows it, and "-"
    /// when the firmware did not answer.
    #[test]
    fn the_device_section_shows_the_build() {
        let (mut p, mut state, _) = panel();
        let f = testing::draw(&mut p, &state, 100, 90);
        assert!(f.contains("v1.1.6-beta4-0-g557bce7"), "{f}");
        assert!(f.contains("2026-09-28"), "{f}");
        state.caps.build_info = None;
        let rows = p.rows(&state, &p.shared.borrow().stats);
        assert!(rows.contains(&Row::info("Build", "-")), "{rows:?}");
        assert!(rows.contains(&Row::info("Build Date", "-")), "{rows:?}");
    }

    #[test]
    fn the_starvation_delta_is_badged() {
        let (mut p, state, _) = panel();
        let f = testing::draw(&mut p, &state, 100, 90);
        assert!(f.contains("+2"), "the delta badge: {f}");
        assert!(f.contains("0m 00s"), "elapsed since the last event: {f}");
        assert!(f.contains("0m 30s"), "the interval before it: {f}");
    }

    #[test]
    fn r_resets_the_watermarks_through_the_command_grammar() {
        let (mut p, state, _) = panel();
        assert_eq!(
            p.handle(key(KeyCode::Char('r')), &state),
            ScreenEvent::Command("diag.buffers.reset".into())
        );
        // D58: the label is the button, the value is the key that presses it,
        // the way round every other row on the panel reads. It closes the
        // buffer column, where the Console's button sits.
        let [_, buffers, _] = p.columns(&state, &p.shared.borrow().stats);
        assert_eq!(
            buffers.last(),
            Some(&Row::Info {
                label: "Reset Watermarks".into(),
                value: "r".into(),
            })
        );
    }

    #[test]
    fn golden_frames_at_both_sizes() {
        let shared = shared();
        shared.borrow_mut().stats = stats();
        let state = testing::state();
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            let f = testing::frame(
                Tool::Stats,
                Box::new(StatsPanel::new(shared.clone())),
                &state,
                w,
                h,
            );
            assert_eq!(f.lines().count(), h as usize, "{w}x{h}");
            assert!(f.contains("System Statistics"), "{w}x{h}:\n{f}");
            assert!(f.contains("T closes"), "{w}x{h}:\n{f}");
            // The Console's firmware row reads "v1.1.6 beta 4".
            assert!(f.contains("v1.1.6 beta 4"), "{w}x{h}:\n{f}");
            assert!(f.contains("Reset watermarks"), "the key line: {w}x{h}\n{f}");
            assert!(f.contains("Build"), "{w}x{h}:\n{f}");
            assert!(
                !f.contains('\u{2014}'),
                "a hyphen for a missing value: {w}x{h}\n{f}"
            );
            // Two columns side by side at 120x40, one at 80x24.
            let beside = f
                .lines()
                .any(|l| l.contains("DEVICE INFORMATION") && l.contains("S/PDIF INPUT"));
            assert_eq!(beside, w == 120, "{w}x{h}:\n{f}");
        }
    }

    #[test]
    fn every_advertised_key_is_handled() {
        let (mut p, state, _) = panel();
        for help in KEYS {
            for k in crate::screens::tests::keys_for(help.key) {
                let before = p.first;
                let ev = p.handle(k, &state);
                assert!(
                    ev != ScreenEvent::Unhandled || p.first != before,
                    "{:?} is not bound",
                    k.code
                );
            }
        }
    }
}
