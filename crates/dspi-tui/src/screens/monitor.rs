//! The Interrupt Monitor: `InterruptMonitor.swift` as a tool panel.
//!
//! This is `dspi watch` in a panel. The runner already drains the notification
//! reader every tick to keep the device state current, and the log is filled
//! from that drain, so the panel is a view of
//! [`actions::EventLog`](crate::actions::EventLog) and never touches the
//! endpoint itself. That also means the log is running before the panel opens,
//! which is what makes it useful: `I` shows what just happened rather than an
//! empty box waiting for the next event.

use crossterm::event::{KeyCode, KeyEvent};
use dspi_session::DeviceState;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

use super::Shared;
use super::panel::{self, Header};
use crate::shell::{Screen, ScreenEvent};
use crate::theme::Theme;
use crate::widgets::table::Cell;
use crate::widgets::{Column, KeyHelp, StatusTone, Table};

const KEYS: &[KeyHelp] = &[
    KeyHelp::new("Space", "Pause"),
    KeyHelp::new("D", "Clear"),
    KeyHelp::new("↑ ↓", "Scroll"),
    KeyHelp::new("End", "Follow"),
];

/// The Console's columns, plus the decoded field name its log line packs into
/// one string. Splitting them is what makes the terminal's version readable at
/// 80 columns: the field column can be clipped without taking the value with
/// it.
const COLUMNS: [Column; 6] = [
    Column::right("Time", 9),
    Column::right("Seq", 4),
    Column::left("Event", 16),
    Column::left("Source", 8),
    Column::left("Field", 24),
    Column::left("Value", 40),
];

pub struct MonitorPanel {
    shared: Shared,
    /// The first visible row, or `None` while following the tail.
    scroll: Option<usize>,
}

impl MonitorPanel {
    pub fn new(shared: Shared) -> Self {
        Self {
            shared,
            scroll: None,
        }
    }

    /// The header's right-hand side: the Console's state word and its count.
    pub fn status(&self) -> (String, StatusTone, String) {
        let log = &self.shared.borrow().log;
        let word = log.state();
        let tone = match word {
            "Listening" => StatusTone::Ok,
            "Paused" => StatusTone::Warning,
            _ => StatusTone::Neutral,
        };
        (word.into(), tone, log.count_text())
    }
}

impl Screen for MonitorPanel {
    fn title(&self) -> String {
        "Interrupt Monitor".into()
    }

    fn draw(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        _state: &DeviceState,
        focused: bool,
    ) {
        if area.height == 0 {
            return;
        }
        let (word, tone, count) = self.status();
        let header = Header::new(&count).pill(word, tone);
        panel::draw_header(area, buf, theme, &header, false);
        if area.height < 3 {
            return;
        }
        let body = Rect::new(area.x, area.y + 1, area.width, area.height - 1);

        let log = self.shared.borrow();
        let rows: Vec<Vec<Cell>> = log
            .log
            .entries
            .iter()
            .map(|e| {
                vec![
                    Cell::dim(format!("{:.3}", e.at)),
                    Cell::dim(format!("{}", e.seq)),
                    // A sequence gap means the device dropped something and the
                    // shadow can no longer be trusted; that has to be visible.
                    if e.lost {
                        Cell::colored(format!("{} !", e.event), theme.danger)
                    } else {
                        Cell::from(e.event)
                    },
                    Cell::dim(e.source.clone()),
                    Cell::from(e.field.clone()),
                    Cell::from(e.value.clone()),
                ]
            })
            .collect();

        // One row of the table is the header, so the body is one shorter.
        let visible = body.height.saturating_sub(1) as usize;
        let scroll = match self.scroll {
            Some(s) => s.min(rows.len().saturating_sub(1)),
            // Following: keep the newest line on the bottom row, which is what
            // the Console's scroll-to-bottom does.
            None => rows.len().saturating_sub(visible),
        };
        if rows.is_empty() {
            buf.set_string(
                body.x + 1,
                body.y,
                if log.log.active {
                    "No events yet."
                } else {
                    "Not listening: this transport has no notification endpoint."
                },
                theme.label(),
            );
            return;
        }
        Table::new(&COLUMNS, theme)
            .rows(rows)
            .scroll(scroll)
            .focused(focused, (usize::MAX, usize::MAX))
            .render(body, buf);
    }

    fn handle(&mut self, key: KeyEvent, _state: &DeviceState) -> ScreenEvent {
        let mut shared = self.shared.borrow_mut();
        let log = &mut shared.log;
        match key.code {
            KeyCode::Char(' ') => {
                log.paused = !log.paused;
                let paused = log.paused;
                drop(shared);
                ScreenEvent::Status(if paused { "Paused" } else { "Listening" }.into())
            }
            KeyCode::Char('D') => {
                log.clear();
                self.scroll = None;
                drop(shared);
                ScreenEvent::Status("Log cleared".into())
            }
            KeyCode::Up => {
                let last = log.entries.len().saturating_sub(1);
                self.scroll = Some(self.scroll.unwrap_or(last).saturating_sub(1));
                ScreenEvent::Handled
            }
            KeyCode::Down => {
                let last = log.entries.len().saturating_sub(1);
                self.scroll = Some((self.scroll.unwrap_or(0) + 1).min(last));
                ScreenEvent::Handled
            }
            KeyCode::PageUp => {
                let last = log.entries.len().saturating_sub(1);
                self.scroll = Some(self.scroll.unwrap_or(last).saturating_sub(10));
                ScreenEvent::Handled
            }
            KeyCode::PageDown => {
                let last = log.entries.len().saturating_sub(1);
                self.scroll = Some((self.scroll.unwrap_or(0) + 10).min(last));
                ScreenEvent::Handled
            }
            KeyCode::Home => {
                self.scroll = Some(0);
                ScreenEvent::Handled
            }
            // Back to following the tail.
            KeyCode::End => {
                self.scroll = None;
                ScreenEvent::Handled
            }
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
    use crate::actions;
    use crate::screens::panel::testing;
    use crate::screens::shared;
    use crate::shell::Tool;
    use crate::widgets::testing::{key, shift};
    use dspi_session::{Event, Notification, Source};

    fn note(seq: u8, event: Event) -> Notification {
        Notification {
            seq,
            event,
            lost: false,
        }
    }

    fn section(name: &str) -> usize {
        dspi_proto::generated::SECTIONS
            .iter()
            .find(|(n, _, _)| *n == name)
            .map(|(_, o, _)| *o)
            .expect("section")
    }

    fn panel() -> (MonitorPanel, DeviceState, crate::screens::Shared) {
        let shared = shared();
        {
            let mut s = shared.borrow_mut();
            s.log.active = true;
            s.log.push(
                0.125,
                &note(
                    7,
                    Event::ParamChanged {
                        offset: (section("user_volume")) as u16,
                        source: Source::Uac1,
                        bytes: (-30.0f32).to_le_bytes().to_vec(),
                    },
                ),
            );
            s.log.push(1.5, &note(8, Event::PresetLoaded { slot: 2 }));
        }
        (MonitorPanel::new(shared.clone()), testing::state(), shared)
    }

    #[test]
    fn the_log_is_a_table_of_decoded_events() {
        let (mut p, state, _) = panel();
        let f = testing::draw(&mut p, &state, 120, 20);
        assert!(!f.contains("Interrupt Monitor"), "the pane names it: {f}");
        assert!(f.contains("TIME") && f.contains("SEQ"), "the header: {f}");
        assert!(f.contains("FIELD") && f.contains("VALUE"), "{f}");
        assert!(f.contains("param_changed"), "{f}");
        assert!(f.contains("uac1"), "the source column: {f}");
        assert!(f.contains("user_volume.volume_db"), "the field name: {f}");
        assert!(f.contains("-30"), "the value: {f}");
        assert!(f.contains("preset_loaded") && f.contains("slot 3"), "{f}");
        assert!(f.contains("Listening"), "the state pill: {f}");
        assert!(f.contains("2 events"), "the count: {f}");
    }

    #[test]
    fn the_header_says_which_of_the_consoles_three_states_it_is_in() {
        let shared = shared();
        let p = MonitorPanel::new(shared.clone());
        assert_eq!(p.status().0, "Inactive");
        assert_eq!(p.status().2, "0 events");
        shared.borrow_mut().log.active = true;
        assert_eq!(p.status().0, "Listening");
        shared.borrow_mut().log.paused = true;
        assert_eq!(p.status().0, "Paused");
        shared
            .borrow_mut()
            .log
            .push(0.0, &note(1, Event::PresetLoaded { slot: 0 }));
        assert_eq!(p.status().2, "0 events", "a paused log drops what arrives");
        shared.borrow_mut().log.paused = false;
        shared
            .borrow_mut()
            .log
            .push(0.0, &note(2, Event::PresetLoaded { slot: 0 }));
        assert_eq!(p.status().2, "1 event", "the Console's singular");
    }

    #[test]
    fn space_pauses_and_shift_d_clears() {
        let (mut p, state, shared) = panel();
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &state),
            ScreenEvent::Status("Paused".into())
        );
        assert!(shared.borrow().log.paused);
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &state),
            ScreenEvent::Status("Listening".into())
        );
        assert_eq!(
            p.handle(shift(KeyCode::Char('D')), &state),
            ScreenEvent::Status("Log cleared".into())
        );
        assert!(shared.borrow().log.entries.is_empty());
        assert_eq!(shared.borrow().log.count_text(), "0 events");
    }

    /// The log has to be bounded, or a device that notifies at speed fills
    /// memory while nobody is looking.
    #[test]
    fn the_log_is_capped_and_keeps_the_newest() {
        let mut log = actions::EventLog {
            capacity: 3,
            ..Default::default()
        };
        for seq in 0..10u8 {
            log.push(seq as f64, &note(seq, Event::PresetLoaded { slot: seq }));
        }
        assert_eq!(log.entries.len(), 3);
        assert_eq!(log.entries.front().unwrap().seq, 7);
        assert_eq!(log.entries.back().unwrap().seq, 9);
        assert_eq!(log.seen, 10, "the count is of everything seen");
    }

    /// Every field a `PARAM_CHANGED` can name, decoded from its offset alone,
    /// which is all the notification carries.
    #[test]
    fn an_offset_decodes_to_the_field_it_landed_in() {
        let at = |name: &str, extra: usize| section(name) + extra;
        for (offset, want) in [
            (at("global", 4), "global.bypass"),
            (at("global", 0), "global.preamp_gain_db"),
            (at("crossfeed", 4), "crossfeed.custom_fc"),
            (at("delays", 12), "delays.delay_ms[3]"),
            (at("preamp", 8), "preamp.gain_db[2]"),
            (at("outputs", 12 + 4), "outputs[1].gain_db"),
            (at("outputs", 12), "outputs[1].enabled"),
            (at("crosspoints", 8 * (9 + 2)), "crosspoints[1][2]"),
            (at("user_volume", 0), "user_volume.volume_db"),
            (at("lg_sound_sync", 2), "lg_sound_sync.volume"),
            (at("input_config", 0), "input_config.input_source"),
            // V28 moved everything below `spdif_rx_pin_ext` down a byte, so
            // this is the offset that would silently mean the wrong field if
            // the table were transcribed rather than shared.
            (at("input_config", 12), "input_config.i2s_clock_mode"),
            (at("pins", 0), "pins.num_pin_outputs"),
            (at("pins", 3), "pins.pins[2]"),
        ] {
            assert_eq!(actions::field_name(offset), want, "at offset {offset}");
        }

        // The band tables are two dimensional, so their arithmetic is the part
        // most worth pinning.
        let bands = dspi_proto::generated::wire::WIRE_MAX_BANDS as usize;
        let eq = section("eq") + (3 * bands + 2) * 16;
        assert_eq!(actions::field_name(eq), "eq[3][2]");
        let xover_bands = dspi_proto::generated::wire::WIRE_MAX_XOVER_BANDS as usize;
        let xo = section("crossovers") + (xover_bands + 3) * 16;
        assert_eq!(actions::field_name(xo), "xover[1][3]");
        // An offset past the end of the packet is named, not guessed at.
        assert_eq!(actions::field_name(999_999), "+0xF423F");
    }

    /// A whole band arrives as one packet; reading it as four floats would say
    /// nothing at all.
    #[test]
    fn a_band_notification_is_decoded_as_a_band() {
        let mut bytes = vec![0u8; 16];
        bytes[0] = 1; // peaking
        bytes[4..8].copy_from_slice(&2856.0f32.to_le_bytes());
        bytes[8..12].copy_from_slice(&3.58f32.to_le_bytes());
        bytes[12..16].copy_from_slice(&(-8.6f32).to_le_bytes());
        let (event, source, field, value) = actions::describe(&Event::ParamChanged {
            offset: (section("eq") + 16) as u16,
            source: Source::Gpio,
            bytes,
        });
        assert_eq!(event, "param_changed");
        assert_eq!(source, "gpio");
        assert_eq!(field, "eq[0][1]");
        assert_eq!(value, "type=1 byp=0 f=2856.0 Hz Q=3.58 g=-8.60 dB");
    }

    #[test]
    fn golden_frames_at_both_sizes() {
        let (_, state, shared) = panel();
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            let f = testing::frame(
                Tool::Monitor,
                Box::new(MonitorPanel::new(shared.clone())),
                &state,
                w,
                h,
            );
            assert_eq!(f.lines().count(), h as usize, "{w}x{h}");
            assert!(f.contains("Interrupt Monitor"), "{w}x{h}:\n{f}");
            assert!(f.contains("I closes"), "{w}x{h}:\n{f}");
            assert!(f.contains("Pause"), "the key line: {w}x{h}\n{f}");
        }
    }

    #[test]
    fn every_advertised_key_is_handled() {
        let (mut p, state, _) = panel();
        for help in KEYS {
            for k in crate::screens::tests::keys_for(help.key) {
                let before = p.scroll;
                let ev = p.handle(k, &state);
                assert!(
                    ev != ScreenEvent::Unhandled || p.scroll != before,
                    "{:?} is not bound",
                    k.code
                );
            }
        }
    }
}
