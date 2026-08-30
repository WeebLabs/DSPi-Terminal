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
use crate::actions::Stats;
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
    /// A fill percentage with its min-max watermark band.
    Fill {
        label: String,
        pct: u8,
        min: u8,
        max: u8,
        kind: FillKind,
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

    /// Every row, rebuilt from the last poll each frame.
    pub fn rows(&self, state: &DeviceState, s: &Stats) -> Vec<Row> {
        let mut rows = vec![
            Row::section("Device Information"),
            Row::info(
                "Platform",
                format!("{:?}", state.caps.platform).to_uppercase(),
            ),
            Row::info("Firmware", format!("v{}", state.caps.firmware)),
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
            rows.push(Row::info("Core 1 Mode", mode.label()));
        }

        if let Some(rx) = &s.spdif_rx {
            let (word, tone) = spdif_state(rx.state);
            let locked = rx.state == dspi_proto::enums::SpdifRxState::Locked;
            rows.push(Row::Blank);
            rows.push(Row::section("S/PDIF Input"));
            rows.push(Row::Pill {
                label: "State".into(),
                text: word.into(),
                tone,
            });
            rows.push(Row::info("Active Source", source_name(rx.input_source)));
            rows.push(Row::info(
                "Sample Rate",
                if locked && rx.sample_rate > 0 {
                    format!("{:.1} kHz", rx.sample_rate as f64 / 1000.0)
                } else {
                    "-".into()
                },
            ));
            rows.push(Row::info("Lock Count", rx.lock_count.to_string()));
            rows.push(Row::info("Loss Count", rx.loss_count.to_string()));
            rows.push(Row::info(
                "Parity Errors",
                if locked {
                    rx.parity_errors.to_string()
                } else {
                    "-".into()
                },
            ));
            rows.push(Row::info(
                "FIFO Fill",
                if locked {
                    format!("{}%", rx.fifo_fill_pct)
                } else {
                    "-".into()
                },
            ));
            rows.push(Row::info(
                "RX Pin",
                match s.spdif_rx_pin {
                    Some(p) => format!("GPIO {p}"),
                    None => "-".into(),
                },
            ));

            if let Some(cs) = &s.spdif_channel {
                rows.push(Row::section("Channel Status"));
                rows.push(Row::info(
                    "Format",
                    if cs.is_consumer() {
                        "Consumer"
                    } else {
                        "Professional"
                    },
                ));
                rows.push(Row::info(
                    "Audio",
                    if cs.is_pcm() { "PCM" } else { "Non-PCM" },
                ));
                rows.push(Row::info("Category", cs.category()));
                rows.push(Row::info("Word Length", cs.word_length()));
                rows.push(Row::info(
                    "Copy",
                    if cs.copy_permitted() {
                        "Permitted"
                    } else {
                        "Prohibited"
                    },
                ));
            }

            if rx.state != dspi_proto::enums::SpdifRxState::Inactive {
                rows.push(Row::section("Debug"));
                rows.push(Row::info("Library State", lib_state(rx.lib_state)));
                // The two counts share a byte, high nibble then low.
                rows.push(Row::info(
                    "Stable Callbacks",
                    ((rx.callback_counts >> 4) & 0x0F).to_string(),
                ));
                rows.push(Row::info(
                    "Lost Callbacks",
                    (rx.callback_counts & 0x0F).to_string(),
                ));
            }
        }

        if let Some(lg) = &s.lg {
            rows.push(Row::Blank);
            rows.push(Row::section("LG Sound Sync"));
            rows.push(Row::info("Enabled", yes_no(lg.enabled)));
            rows.push(Row::Pill {
                label: "Present".into(),
                text: yes_no(lg.present).into(),
                tone: if lg.present {
                    StatusTone::Ok
                } else {
                    StatusTone::Neutral
                },
            });
            rows.push(Row::info(
                "TV Volume",
                if lg.volume == dspi_proto::packets::LgSoundSyncStatus::VOLUME_UNKNOWN {
                    "-".into()
                } else {
                    format!("{} / 100", lg.volume)
                },
            ));
            rows.push(Row::info("TV Mute", on_off(lg.muted)));
        }

        if let Some(a) = &s.adat {
            rows.push(Row::Blank);
            rows.push(Row::section("ADAT Bulk Output"));
            rows.push(Row::Pill {
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
            rows.push(Row::info("Streaming", yes_no(a.active)));
            rows.push(Row::info("Rate Supported", yes_no(a.rate_ok)));
            rows.push(Row::info("Data Pin", format!("GPIO {}", a.pin)));
            rows.push(Row::info("Resync Count", a.resync_count.to_string()));
            rows.push(Row::info("Slip Count", a.slip_count.to_string()));
        }

        if let Some(i) = &s.i2s_slave {
            let (word, tone) = i2s_state(i.state);
            rows.push(Row::Blank);
            rows.push(Row::section("I2S Input (Slave Clock)"));
            rows.push(Row::Pill {
                label: "State".into(),
                text: word,
                tone,
            });
            rows.push(Row::info(
                "Detected Rate",
                if i.detected_rate > 0 {
                    format!("{:.1} kHz", i.detected_rate as f64 / 1000.0)
                } else {
                    "-".into()
                },
            ));
            rows.push(Row::info(
                "Measured Rate",
                if i.measured_hz > 0 {
                    format!("{} Hz", i.measured_hz)
                } else {
                    "-".into()
                },
            ));
            rows.push(Row::info("Lock Count", i.lock_count.to_string()));
            rows.push(Row::info("Loss Count", i.loss_count.to_string()));
        }

        rows.push(Row::Blank);
        rows.push(Row::section("PDM (Subwoofer)"));
        rows.push(Row::Counters {
            label: "Ring Buffer".into(),
            detail: "Core 0 → Core 1".into(),
            over: Some(s.pdm_ring_over),
            under: Some(s.pdm_ring_under),
        });
        rows.push(Row::Counters {
            label: "DMA Buffer".into(),
            detail: "Core 1 → PIO".into(),
            over: Some(s.pdm_dma_over),
            under: Some(s.pdm_dma_under),
        });

        rows.push(Row::Blank);
        rows.push(Row::section("Audio Output"));
        rows.push(Row::Counters {
            label: "USB Ring".into(),
            detail: "ISR → Main Loop".into(),
            over: Some(s.usb_ring_over),
            under: None,
        });
        rows.push(Row::Counters {
            label: "Buffer Pool".into(),
            detail: "USB → DMA".into(),
            over: Some(s.spdif_over),
            under: Some(s.spdif_under),
        });

        rows.push(Row::Blank);
        rows.push(Row::section("SPDIF DMA Starvation"));
        rows.push(Row::Starvation {
            total: s.starvation_total,
            delta: s.starvation_delta,
        });
        let instances = s
            .buffers
            .as_ref()
            .map(|b| (b.num_spdif as usize).max(2))
            .unwrap_or(2)
            .min(4);
        for i in 0..instances {
            rows.push(Row::info(
                &format!("Out {}/{}", i * 2 + 1, i * 2 + 2),
                s.starvation_per_instance[i].to_string(),
            ));
        }
        rows.push(Row::info(
            "Time since last event",
            polls_text(s.polls_since_starvation),
        ));
        rows.push(Row::info(
            "Time between last two",
            polls_text(s.polls_between_starvations),
        ));

        if let Some(b) = &s.buffers {
            rows.push(Row::Blank);
            rows.push(Row::section("Buffer Fill Levels"));
            rows.push(Row::Flags(vec![
                ("Audio Streaming".into(), b.streaming()),
                ("PDM Active".into(), b.pdm_active()),
            ]));
            for i in 0..(b.num_spdif as usize).min(4) {
                let f = &b.spdif[i];
                rows.push(Row::Fill {
                    label: format!("Out {}/{}", i * 2 + 1, i * 2 + 2),
                    pct: f.consumer_fill_pct,
                    min: f.consumer_min_fill_pct,
                    max: f.consumer_max_fill_pct,
                    kind: FillKind::Spdif,
                });
            }
            if b.pdm_active() {
                rows.push(Row::Fill {
                    label: "PDM DMA".into(),
                    pct: b.pdm.dma_fill_pct,
                    min: b.pdm.dma_min_fill_pct,
                    max: b.pdm.dma_max_fill_pct,
                    kind: FillKind::PdmDma,
                });
                rows.push(Row::Fill {
                    label: "PDM Ring".into(),
                    pct: b.pdm.ring_fill_pct,
                    min: b.pdm.ring_min_fill_pct,
                    max: b.pdm.ring_max_fill_pct,
                    kind: FillKind::PdmRing,
                });
            }
        }

        rows.push(Row::Blank);
        // The Console's button, with the key that presses it in the value
        // column where every other row's value is. It read backwards before.
        rows.push(Row::info("Reset Watermarks", "r"));
        rows
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
            // of pairs readable at a glance.
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
        } => {
            let readout = format!("{pct:>3}%  {min}-{max}%");
            buf.set_string(area.x + 1, area.y, fit_left(label, 14), theme.value());
            let bx = area.x + 16;
            // A short meter, as `DESIGN.md` 7.10 asks: the number is the
            // reading, the bar is only there to be glanced at.
            let bw = area
                .width
                .saturating_sub(17 + readout.chars().count() as u16)
                .min(24);
            let tone = kind.tone(*pct);
            let color = match tone {
                StatusTone::Ok => theme.ok,
                StatusTone::Warning => theme.warning,
                StatusTone::Danger => theme.danger,
                StatusTone::Neutral => theme.dim,
            };
            if bw >= 6 {
                let cell = |v: u8| ((v as u32 * (bw as u32 - 1)) / 100) as u16;
                let filled = cell(*pct);
                let (fill, empty) = if theme.glyphs == Glyphs::Ascii {
                    ("#", ".")
                } else {
                    ("▓", "░")
                };
                for i in 0..bw {
                    let (sym, style) = if i <= filled {
                        (fill, Style::default().fg(color))
                    } else {
                        (empty, Style::default().fg(theme.chrome_faint))
                    };
                    buf[(bx + i, area.y)].set_symbol(sym).set_style(style);
                }
                // The watermarks: how far the buffer has been in either
                // direction since the counters were last reset.
                for (v, mark) in [(min, "▏"), (max, "▕")] {
                    let x = bx + cell(*v);
                    buf[(x, area.y)]
                        .set_symbol(if theme.glyphs == Glyphs::Ascii {
                            "|"
                        } else {
                            mark
                        })
                        .set_style(Style::default().fg(theme.fg));
                }
            }
            buf.set_string(
                area.x + area.width - readout.chars().count() as u16 - 1,
                area.y,
                &readout,
                theme.value(),
            );
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
        let rows = self.rows(state, &stats);
        let header = Header::new(FOOTER);
        panel::draw_header(area, buf, theme, &header, false);
        if area.height < 2 {
            return;
        }
        let body = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
        // Every row is one line tall, so the window is a plain clamp.
        self.first = self
            .first
            .min(rows.len().saturating_sub(body.height as usize));
        for (i, row) in rows.iter().enumerate().skip(self.first) {
            let y = body.y + (i - self.first) as u16;
            if y >= body.y + body.height {
                break;
            }
            draw_row(Rect::new(body.x, y, body.width, 1), buf, theme, row);
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

    #[test]
    fn a_fill_row_carries_its_watermarks() {
        let (mut p, state, _) = panel();
        let f = testing::draw(&mut p, &state, 100, 90);
        assert!(f.contains("50%  20-80%"), "{f}");
        assert!(f.contains("▏") && f.contains("▕"), "the watermarks: {f}");
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
        // the way round every other row on the panel reads.
        let rows = p.rows(&state, &p.shared.borrow().stats);
        assert_eq!(
            rows.last(),
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
            assert!(f.contains("Reset watermarks"), "the key line: {w}x{h}\n{f}");
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
