//! The Crossfeed panel: `CrossfeedView.swift` as a tool panel.
//!
//! The Console's four voicings are a radio group over one device parameter,
//! and the two sliders beneath them belong to the Custom voicing: editing
//! either one while a named voicing is selected switches to Custom first, the
//! same way clicking the Console's slider does, so what is on screen never
//! claims to be a preset it no longer is.

use crossterm::event::KeyEvent;
use dspi_session::DeviceState;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::number;
use super::panel::{self, Body, ChipSpec, Header, PanelGraph, Param, Row};
use crate::curves;
use crate::shell::{Screen, ScreenEvent};
use crate::theme::Theme;
use crate::widgets::{Action, ChipState, KeyHelp, PopupList};

/// The Console's voicings, `crossfeedPresets` in `CrossfeedView.swift:52-57`.
/// The em-dash of the Console's own strings is a hyphen here, as the house
/// style requires.
const PRESETS: [(&str, &str, &str); 4] = [
    (
        "Default",
        "700 Hz / 4.5 dB - Balanced, most popular",
        "default",
    ),
    (
        "Chu Moy",
        "700 Hz / 6.0 dB - Stronger spatial effect",
        "chumoy",
    ),
    (
        "Jan Meier",
        "650 Hz / 9.5 dB - Natural speaker-like",
        "meier",
    ),
    ("Custom", "User-defined parameters", "custom"),
];

/// The Console's output-pair presets menu.
const PAIR_PRESETS: [&str; 3] = ["All pairs", "Pair 1 only (Headphones)", "None"];

const CUSTOM: u8 = 3;

const KEYS: &[KeyHelp] = &[
    KeyHelp::new("↑ ↓", "Move"),
    KeyHelp::new("← →", "Adjust"),
    KeyHelp::new("Enter", "Edit or choose"),
    KeyHelp::new("Space", "Toggle"),
    KeyHelp::new("p", "Presets"),
    KeyHelp::new("Backspace", "Reset"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    Pairs,
}

pub struct CrossfeedPanel {
    body: Body,
    chip: usize,
    pending: Option<Pending>,
}

impl Default for CrossfeedPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl CrossfeedPanel {
    pub fn new() -> Self {
        Self {
            body: Body::default(),
            chip: 0,
            pending: None,
        }
    }

    /// Stereo output pairs: the S/PDIF instances, which is every output slot
    /// except the mono PDM sub. Two on RP2040, four on RP2350.
    fn pairs(state: &DeviceState) -> usize {
        (state.caps.num_outputs as usize).saturating_sub(1) / 2
    }

    /// The per-pair mask shipped in wire format V20 (`REQ_SET_CROSSFEED_
    /// OUTPUTS`, config.h:499-517); older firmware crossfeeds a fixed set and
    /// the Console hides the selector.
    fn shows_pairs(state: &DeviceState) -> bool {
        state.caps.wire_format >= 20 && Self::pairs(state) > 0
    }

    fn all_pairs(state: &DeviceState) -> u8 {
        let n = Self::pairs(state);
        if n >= 8 { 0xFF } else { (1u16 << n) as u8 - 1 }
    }

    fn rows(&self, state: &DeviceState, theme: &Theme) -> Vec<Row> {
        let cf = state.crossfeed();
        let mut rows = vec![Row::Section {
            title: "Frequency response".into(),
            action: None,
        }];
        rows.push(Row::Graph(if cf.enabled {
            let (cross, direct) =
                curves::crossfeed_curves(cf.custom_fc as f64, cf.custom_feed_db as f64);
            PanelGraph::Curves {
                series: vec![
                    ("Crossfeed".into(), theme.warning, cross),
                    ("Direct".into(), theme.accent, direct),
                ],
                legend: vec![
                    ("Direct".into(), theme.accent),
                    ("Crossfeed".into(), theme.warning),
                ],
            }
        } else {
            PanelGraph::Disabled
        }));

        if Self::shows_pairs(state) {
            rows.push(Row::Blank);
            rows.push(Row::Section {
                title: "Output pairs".into(),
                action: Some("Presets ▾".into()),
            });
            let chips = (0..Self::pairs(state))
                .map(|p| ChipSpec {
                    label: (p + 1).to_string(),
                    state: if cf.output_pair_mask & (1 << p) != 0 {
                        ChipState::On
                    } else {
                        ChipState::Off
                    },
                    color: theme.role_color(crate::theme::ChannelRole::Output(2 * p as u8)),
                    enabled: true,
                    dimmed: false,
                })
                .collect();
            // `DESIGN.md` 7.8's template is header, chips, caption.
            rows.push(Row::Chips {
                chips,
                cursor: self.chip,
                polarity: false,
            });
            rows.push(Row::Caption(
                "Crossfeed only the stereo output pairs feeding headphones. Speaker pairs stay \
                 bit-accurate. The mono sub is never crossfed."
                    .into(),
            ));
        }

        rows.push(Row::Blank);
        rows.push(Row::Section {
            title: "Preset".into(),
            action: None,
        });
        for (i, (name, detail, _)) in PRESETS.iter().enumerate() {
            rows.push(Row::Radio {
                label: (*name).into(),
                detail: (*detail).into(),
                on: cf.preset as usize == i,
            });
        }

        rows.push(Row::Blank);
        rows.push(Row::Section {
            title: "Parameters".into(),
            action: None,
        });
        // `CrossfeedView.swift:301`: the whole section is at 0.5 opacity unless
        // the voicing is Custom, and it stays live, because an edit is what
        // switches the voicing.
        let custom = cf.preset == CUSTOM;
        rows.push(Row::Param(
            Param::new("Cutoff Frequency", cf.custom_fc as f64, 500.0, 2000.0, "Hz")
                .step(10.0)
                .decimals(0)
                .dimmed(!custom)
                .caption(
                    "Simulates head shadow lowpass cutoff. Lower = more bass crossfeed. \
                     Typical: 650-700 Hz.",
                ),
        ));
        rows.push(Row::Param(
            Param::new("Feed Level", cf.custom_feed_db as f64, 0.0, 15.0, "dB")
                .step(0.5)
                .decimals(1)
                .dimmed(!custom)
                .caption(
                    "Crossfeed attenuation below direct signal. Higher = more crossfeed. \
                     Typical: 4.5-9.5 dB.",
                ),
        ));
        rows.push(Row::Blank);
        rows.push(Row::Toggle {
            label: "Interaural Time Delay".into(),
            on: cf.itd_enabled,
            caption: Some("Simulates ~220 us path difference via all-pass filter".into()),
            enabled: true,
        });
        rows
    }

    /// What one row's action means, as a command.
    fn act(&mut self, row: &Row, action: Action, state: &DeviceState) -> ScreenEvent {
        match (row, action) {
            (Row::Chips { .. }, Action::Selected(i)) => {
                self.chip = i;
                ScreenEvent::Handled
            }
            (Row::Chips { .. }, Action::Chip(i, next)) => {
                let bit = 1u8 << i;
                let mask = state.crossfeed().output_pair_mask;
                let mask = if next == ChipState::On {
                    mask | bit
                } else {
                    mask & !bit
                };
                ScreenEvent::Command(format!("cf.outputs 0x{mask:X}"))
            }
            (Row::Chips { .. }, Action::Open) => {
                self.pending = Some(Pending::Pairs);
                ScreenEvent::Popup(PopupList::new(
                    "Output pairs",
                    PAIR_PRESETS.iter().map(|s| s.to_string()).collect(),
                    0,
                ))
            }
            (Row::Radio { label, .. }, Action::Toggled(_)) => {
                let token = PRESETS
                    .iter()
                    .find(|(n, _, _)| n == label)
                    .map(|(_, _, t)| *t)
                    .unwrap_or("default");
                ScreenEvent::Command(format!("cf.preset {token}"))
            }
            (Row::Toggle { .. }, Action::Toggled(next)) => {
                ScreenEvent::Command(format!("cf.itd {}", if next { "on" } else { "off" }))
            }
            (Row::Param(p), Action::Changed(v)) => self.param_command(state, &p.label, v),
            (Row::Param(p), Action::Reset) => {
                let v = if p.label.starts_with("Cutoff") {
                    700.0
                } else {
                    4.5
                };
                self.param_command(state, &p.label, v)
            }
            _ => ScreenEvent::Handled,
        }
    }

    /// A parameter edit belongs to the Custom voicing, so it carries the
    /// voicing change with it.
    fn param_command(&self, state: &DeviceState, label: &str, value: f64) -> ScreenEvent {
        let path = if label.starts_with("Cutoff") {
            "cf.freq"
        } else {
            "cf.feed"
        };
        let mut lines = Vec::new();
        if state.crossfeed().preset != CUSTOM {
            lines.push("cf.preset custom".to_string());
        }
        lines.push(format!("{path} {}", number(value as f32)));
        ScreenEvent::Command(lines.join("\n"))
    }
}

impl Screen for CrossfeedPanel {
    fn title(&self) -> String {
        "Crossfeed".into()
    }

    fn draw(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        state: &DeviceState,
        focused: bool,
    ) {
        let rows = self.rows(state, theme);
        self.body.clamp(&rows);
        let header = Header::new("Crossfeed", "BS2B Bauer Stereophonic-to-Binaural")
            .toggle(state.crossfeed().enabled);
        self.body.draw(area, buf, theme, &header, &rows, focused);
    }

    fn handle(&mut self, key: KeyEvent, state: &DeviceState) -> ScreenEvent {
        let rows = self.rows(state, panel::key_theme());
        self.body.clamp(&rows);
        match panel::dispatch(&mut self.body, &rows, key) {
            panel::Step::Done(ev) => ev,
            panel::Step::Header => ScreenEvent::Command(format!(
                "cf.on {}",
                if state.crossfeed().enabled {
                    "off"
                } else {
                    "on"
                }
            )),
            panel::Step::Commit(i, v) => match &rows[i] {
                Row::Param(p) => self.param_command(state, &p.label, v.clamp(p.min, p.max)),
                _ => ScreenEvent::Handled,
            },
            panel::Step::Row(i, action) => self.act(&rows[i], action, state),
        }
    }

    fn keys(&self) -> &'static [KeyHelp] {
        KEYS
    }

    fn popup_result(&mut self, choice: Option<usize>, state: &DeviceState) -> ScreenEvent {
        let pending = self.pending.take();
        match (pending, choice) {
            (Some(Pending::Pairs), Some(i)) => {
                let mask = match i {
                    0 => Self::all_pairs(state),
                    1 => 0x01,
                    _ => 0x00,
                };
                ScreenEvent::Command(format!("cf.outputs 0x{mask:X}"))
            }
            _ => ScreenEvent::Handled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::panel::testing;
    use crate::shell::Tool;
    use crate::widgets::testing::key;
    use crossterm::event::KeyCode;

    fn panel() -> (CrossfeedPanel, DeviceState) {
        let mut state = testing::state();
        // The fixture leaves crossfeed off; the panel is more interesting on.
        let (_, cf, _) = dspi_proto::generated::SECTIONS
            .iter()
            .find(|(n, _, _)| *n == "crossfeed")
            .copied()
            .unwrap();
        state.bulk.patch(cf, &[1, 0, 1, 0x03]);
        state.bulk.patch(cf + 4, &700.0f32.to_le_bytes());
        state.bulk.patch(cf + 8, &4.5f32.to_le_bytes());
        (CrossfeedPanel::new(), state)
    }

    #[test]
    fn the_panel_carries_every_section_the_console_has() {
        let (mut p, state) = panel();
        let f = testing::draw(&mut p, &state, 100, 40);
        assert!(
            f.contains("Crossfeed · BS2B Bauer Stereophonic-to-Binaural"),
            "{f}"
        );
        assert!(f.contains("● On"), "the master toggle: {f}");
        assert!(f.contains("FREQUENCY RESPONSE"), "{f}");
        assert!(f.contains("OUTPUT PAIRS") && f.contains("Presets ▾"), "{f}");
        assert!(f.contains("PRESET"), "{f}");
        assert!(
            f.contains("700 Hz / 4.5 dB - Balanced, most popular"),
            "{f}"
        );
        assert!(f.contains("Jan Meier"), "{f}");
        assert!(f.contains("Cutoff Frequency"), "{f}");
        assert!(f.contains("700 Hz"), "{f}");
    }

    /// D24: `CrossfeedView.swift:301` draws PARAMETERS at 0.5 opacity outside
    /// the Custom voicing, and `survey-console.md` 2.14 says the same. The rows
    /// stay live, because editing one is what switches the voicing.
    #[test]
    fn the_parameters_dim_outside_custom_but_still_take_an_edit() {
        let (mut p, mut state) = panel();
        let theme = panel::key_theme();
        let (_, cf, _) = dspi_proto::generated::SECTIONS
            .iter()
            .find(|(n, _, _)| *n == "crossfeed")
            .copied()
            .unwrap();

        let dimmed = |p: &CrossfeedPanel, state: &DeviceState| -> Vec<bool> {
            p.rows(state, theme)
                .iter()
                .filter_map(|r| match r {
                    Row::Param(param) => Some(param.dimmed),
                    _ => None,
                })
                .collect()
        };

        // The fixture is on the Default voicing.
        assert_eq!(state.crossfeed().preset, 0);
        assert_eq!(dimmed(&p, &state), vec![true, true]);
        assert_eq!(
            testing::fg_of(&mut p, &state, 100, 40, "Cutoff Frequency"),
            theme.dim,
            "a dimmed parameter is drawn in the caption colour"
        );

        // Custom (wire 3) brings them back up.
        state.bulk.patch(cf + 1, &[3]);
        assert_eq!(state.crossfeed().preset, CUSTOM);
        assert_eq!(dimmed(&p, &state), vec![false, false]);
        assert_eq!(
            testing::fg_of(&mut p, &state, 100, 40, "Cutoff Frequency"),
            theme.fg
        );
    }

    /// The dim is presentation only: a nudge on the Default voicing still
    /// writes, and carries `cf.preset custom` with it.
    #[test]
    fn a_dimmed_parameter_is_still_editable_and_switches_the_voicing() {
        let (mut p, state) = panel();
        let rows = p.rows(&state, panel::key_theme());
        let param = panel::focus_rows(&rows)
            .iter()
            .position(|r| matches!(rows[*r], Row::Param(_)))
            .expect("a parameter row")
            + 1;
        p.body.focus = param;
        assert_eq!(
            p.handle(key(KeyCode::Right), &state),
            ScreenEvent::Command("cf.preset custom\ncf.freq 710".into())
        );
    }

    /// D46: `DESIGN.md` 7.8 draws a PRESET row as one line,
    /// `● Default    700 Hz / 4.5 dB - Balanced, most popular`. Four rows were
    /// costing eight, with the width to spare.
    #[test]
    fn a_preset_row_is_one_line_where_it_fits_and_two_where_it_does_not() {
        let (mut p, state) = panel();
        let f = testing::draw(&mut p, &state, 100, 40);
        let line = f
            .lines()
            .find(|l| l.contains("Default"))
            .expect("the Default row");
        assert!(
            line.contains("700 Hz / 4.5 dB - Balanced, most popular"),
            "label and detail on one line: {line}"
        );
        // The four voicings cost four rows, not eight.
        for name in ["Default", "Chu Moy", "Jan Meier", "Custom"] {
            assert_eq!(
                f.lines().filter(|l| l.contains(name)).count(),
                1,
                "{name}:\n{f}"
            );
        }

        // Too narrow for both, and the detail goes back under its label.
        let f = testing::draw(&mut p, &state, 46, 40);
        let line = f
            .lines()
            .find(|l| l.contains("Default"))
            .expect("the Default row");
        assert!(!line.contains("Balanced"), "{f}");
        assert!(f.contains("Balanced"), "but it is still drawn:\n{f}");
    }

    /// D45: `DESIGN.md` 7.8's template is header, chips, caption. The caption
    /// was drawn above the chips it describes.
    #[test]
    fn the_caption_sits_under_the_chip_row() {
        let (p, state) = panel();
        let rows = p.rows(&state, panel::key_theme());
        let chips = rows
            .iter()
            .position(|r| matches!(r, Row::Chips { .. }))
            .expect("a chip row");
        assert!(
            matches!(rows.get(chips + 1), Some(Row::Caption(_))),
            "{rows:?}"
        );
    }

    #[test]
    fn the_graph_says_disabled_when_the_master_switch_is_off() {
        let (mut p, mut state) = panel();
        let (_, cf, _) = dspi_proto::generated::SECTIONS
            .iter()
            .find(|(n, _, _)| *n == "crossfeed")
            .copied()
            .unwrap();
        state.bulk.patch(cf, &[0]);
        let f = testing::draw(&mut p, &state, 100, 40);
        assert!(f.contains("Disabled"), "{f}");
        assert!(f.contains("○ Off"), "{f}");
    }

    #[test]
    fn golden_frames_at_both_sizes() {
        let (_, state) = panel();
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            let f = testing::frame(
                Tool::Crossfeed,
                Box::new(CrossfeedPanel::new()),
                &state,
                w,
                h,
            );
            assert_eq!(f.lines().count(), h as usize, "{w}x{h}");
            assert!(f.contains("Crossfeed"), "{w}x{h}:\n{f}");
            assert!(f.contains("X closes"), "{w}x{h}:\n{f}");
            assert!(f.contains("INPUTS"), "the sidebar stays: {w}x{h}");
        }
    }

    #[test]
    fn the_header_toggle_is_first_in_focus_order() {
        let (mut p, state) = panel();
        assert_eq!(p.body.focus, 0);
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &state),
            ScreenEvent::Command("cf.on off".into())
        );
        p.handle(key(KeyCode::Down), &state);
        assert_eq!(p.body.focus, 1);
    }

    #[test]
    fn the_chips_write_the_pair_mask_and_the_menu_sets_it_wholesale() {
        let (mut p, state) = panel();
        // Focus 1 is the chip row: the graph and the section headers do not
        // take focus.
        p.body.focus = 1;
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &state),
            ScreenEvent::Command("cf.outputs 0x2".into()),
            "pair 1 was on, so Space clears it"
        );
        assert_eq!(p.handle(key(KeyCode::Right), &state), ScreenEvent::Handled);
        assert_eq!(p.chip, 1);
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &state),
            ScreenEvent::Command("cf.outputs 0x1".into())
        );
        match p.handle(key(KeyCode::Char('p')), &state) {
            ScreenEvent::Popup(list) => assert_eq!(list.items[1], "Pair 1 only (Headphones)"),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            p.popup_result(Some(0), &state),
            ScreenEvent::Command("cf.outputs 0xF".into())
        );
        assert_eq!(
            p.popup_result(Some(2), &state),
            ScreenEvent::Handled,
            "a popup result with nothing pending is ignored"
        );
    }

    #[test]
    fn a_voicing_is_one_write_and_a_slider_edit_switches_to_custom() {
        let (mut p, state) = panel();
        let rows = p.rows(&state, panel::key_theme());
        let radios: Vec<usize> = panel::focus_rows(&rows)
            .iter()
            .enumerate()
            .filter(|(_, r)| matches!(rows[**r], Row::Radio { .. }))
            .map(|(i, _)| i + 1)
            .collect();
        p.body.focus = radios[1];
        assert_eq!(
            p.handle(key(KeyCode::Enter), &state),
            ScreenEvent::Command("cf.preset chumoy".into())
        );
        // The cutoff slider: the preset is Default, so a nudge says Custom
        // first and then the value.
        p.body.focus = radios[3] + 1;
        assert_eq!(
            p.handle(key(KeyCode::Right), &state),
            ScreenEvent::Command("cf.preset custom\ncf.freq 710".into())
        );
        assert_eq!(
            p.handle(key(KeyCode::Backspace), &state),
            ScreenEvent::Command("cf.preset custom\ncf.freq 700".into())
        );
    }

    #[test]
    fn the_time_delay_toggle_is_last() {
        let (mut p, state) = panel();
        let rows = p.rows(&state, panel::key_theme());
        p.body.focus = panel::focus_rows(&rows).len();
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &state),
            ScreenEvent::Command("cf.itd off".into())
        );
    }

    #[test]
    fn every_advertised_key_is_handled_somewhere() {
        let (_, state) = panel();
        for help in KEYS {
            for k in crate::screens::tests::keys_for(help.key) {
                let mut hit = false;
                for focus in 0..12 {
                    let mut p = CrossfeedPanel::new();
                    p.body.focus = focus;
                    let before = (p.body.focus, p.chip);
                    let ev = p.handle(k, &state);
                    if ev != ScreenEvent::Unhandled || (p.body.focus, p.chip) != before {
                        hit = true;
                        break;
                    }
                }
                assert!(hit, "{:?} from {:?} is not bound", k.code, help.key);
            }
        }
    }
}
