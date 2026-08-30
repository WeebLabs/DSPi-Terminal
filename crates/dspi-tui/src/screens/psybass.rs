//! The Psychoacoustic Bass panel: `PsychoacousticBassView.swift` as a tool
//! panel.
//!
//! The spectrum is the Console's schematic rather than a measured response:
//! the original band below the cutoff at whatever the Original Bass control
//! has left of it, and the synthesised harmonics from the cutoff to four times
//! it. The starting points are five writes each, so they are one command with
//! five lines.

use crossterm::event::KeyEvent;
use dspi_session::DeviceState;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::number;
use super::panel::{self, Body, ChipSpec, Header, PanelGraph, Param, Row};
use crate::curves;
use crate::shell::{Screen, ScreenEvent};
use crate::theme::{ChannelRole, Theme};
use crate::widgets::{Action, ChipState, KeyHelp, PopupList};

/// `psybassStartingPoints`, `PsychoacousticBassView.swift:56-61`: name,
/// detail, then cutoff, harmonics, drive, character and original.
const STARTING_POINTS: [(&str, &str, f32, f32, f32, f32, f32); 4] = [
    (
        "Bookshelf speakers",
        "Gentle low-end help",
        60.0,
        0.0,
        6.0,
        50.0,
        0.0,
    ),
    (
        "Small Bluetooth",
        "Portable speaker",
        100.0,
        3.0,
        9.0,
        40.0,
        -12.0,
    ),
    (
        "Laptop / tablet",
        "Tiny drivers, protect them",
        180.0,
        6.0,
        12.0,
        50.0,
        -24.0,
    ),
    (
        "Headphone bass feel",
        "Extra sub sensation",
        45.0,
        -3.0,
        6.0,
        30.0,
        0.0,
    ),
];

/// The Console's outputs menu, `PsychoacousticBassView.swift:245-254`.
const OUTPUT_PRESETS: [&str; 3] = ["All outputs", "Exclude sub (recommended)", "None"];

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
    Outputs,
    StartingPoint,
}

pub struct PsybassPanel {
    body: Body,
    chip: usize,
    pending: Option<Pending>,
}

impl Default for PsybassPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl PsybassPanel {
    pub fn new() -> Self {
        Self {
            body: Body::default(),
            chip: 0,
            pending: None,
        }
    }

    /// The whole feature ships in wire format V23; the Console hides the body
    /// and shows an upgrade note on anything older.
    pub fn supported(state: &DeviceState) -> bool {
        panel::has_feature(state, "psychoacoustic_bass")
    }

    fn rows(&self, state: &DeviceState, theme: &Theme) -> Vec<Row> {
        if !Self::supported(state) {
            return vec![Row::Banner {
                title: "Requires firmware with wire format V23 or newer.".into(),
                body: "Update the DSPi firmware to use Psychoacoustic Bass.".into(),
            }];
        }
        let b = state.psybass();
        let mut rows = vec![
            Row::Section {
                title: "Spectrum".into(),
                action: None,
            },
            Row::Graph(if b.enabled {
                PanelGraph::Bars {
                    fc: b.cutoff_hz as f64,
                    original: curves::psybass_original_fraction(b.original_db as f64),
                    harmonics: curves::psybass_harmonics_fraction(b.harmonics_db as f64),
                }
            } else {
                PanelGraph::Disabled
            }),
            Row::Blank,
            Row::Menu {
                title: "Starting points".into(),
                action: "Apply preset ▾".into(),
            },
            Row::Blank,
            Row::Section {
                title: "Outputs".into(),
                action: Some("Presets ▾".into()),
            },
            Row::Caption(
                "Enhance only the small-speaker outputs. Mask off the sub and any full-range \
                 outputs - synthesizing harmonics on a channel that can reproduce real bass is \
                 counterproductive."
                    .into(),
            ),
        ];
        let chips = (0..state.caps.num_outputs as usize)
            .map(|o| ChipSpec {
                label: (o + 1).to_string(),
                state: if b.output_mask & (1 << o) != 0 {
                    ChipState::On
                } else {
                    ChipState::Off
                },
                color: theme.role_color(ChannelRole::of(
                    state.caps.num_inputs + o as u8,
                    state.caps.num_inputs,
                    state.caps.num_outputs,
                )),
                enabled: true,
                // `PsychoacousticBassView.swift:282-296` dims a chip only for
                // being off, never for the matrix mixer.
                dimmed: false,
            })
            .collect();
        rows.push(Row::Chips {
            chips,
            cursor: self.chip,
            polarity: false,
        });
        rows.push(Row::Blank);
        rows.push(Row::Section {
            title: "Parameters".into(),
            action: None,
        });
        rows.push(Row::Param(
            Param::new("Cutoff Frequency", b.cutoff_hz as f64, 30.0, 300.0, "Hz")
                .step(1.0)
                .decimals(0)
                .caption(
                    "The speaker's low-frequency limit. Content below this feeds the harmonic \
                     generator; generated harmonics span roughly this to 4x.",
                ),
        ));
        rows.push(Row::Param(
            Param::new("Harmonics", b.harmonics_db as f64, -24.0, 12.0, "dB")
                .step(0.5)
                .decimals(1)
                .caption(
                    "Level of the synthesized harmonics. The primary amount-of-effect control. \
                     Higher = more perceived bass.",
                ),
        ));
        rows.push(Row::Param(
            Param::new("Drive", b.drive_db as f64, 0.0, 18.0, "dB")
                .step(0.5)
                .decimals(1)
                .caption(
                    "Pre-gain into the odd-harmonic soft clipper. Higher makes the effect \
                     audible on quieter passages. Mostly affects aggressive character.",
                ),
        ));
        rows.push(Row::Param(
            Param::new("Character", b.character_pct as f64, 0.0, 100.0, "%")
                .step(1.0)
                .decimals(0)
                .ends("Warm", "Aggressive"),
        ));
        rows.push(Row::Param(
            Param::new("Original Bass", b.original_db as f64, -60.0, 0.0, "dB")
                .step(1.0)
                .decimals(1)
                .caption(
                    "Level of the un-reproducible fundamental below the cutoff. Lower \
                     attenuates it, freeing driver excursion and headroom. -60 dB is full \
                     removal. Speaker protection.",
                ),
        ));
        rows
    }

    fn path_for(label: &str) -> &'static str {
        match label {
            "Cutoff Frequency" => "bass.cutoff",
            "Harmonics" => "bass.harmonics",
            "Drive" => "bass.drive",
            "Character" => "bass.character",
            _ => "bass.original",
        }
    }

    fn param_command(label: &str, value: f64) -> ScreenEvent {
        ScreenEvent::Command(format!(
            "{} {}",
            Self::path_for(label),
            number(value as f32)
        ))
    }

    fn act(&mut self, row: &Row, action: Action, state: &DeviceState) -> ScreenEvent {
        match (row, action) {
            (Row::Menu { .. }, Action::Open) => {
                self.pending = Some(Pending::StartingPoint);
                ScreenEvent::Popup(PopupList::new(
                    "Starting points",
                    STARTING_POINTS
                        .iter()
                        .map(|(n, d, ..)| format!("{n} - {d}"))
                        .collect(),
                    0,
                ))
            }
            (Row::Chips { .. }, Action::Selected(i)) => {
                self.chip = i;
                ScreenEvent::Handled
            }
            (Row::Chips { .. }, Action::Chip(i, next)) => {
                let bit = 1u16 << i;
                let mask = state.psybass().output_mask;
                let mask = if next == ChipState::On {
                    mask | bit
                } else {
                    mask & !bit
                };
                ScreenEvent::Command(format!("bass.mask 0x{mask:X}"))
            }
            (Row::Chips { .. }, Action::Open) => {
                self.pending = Some(Pending::Outputs);
                ScreenEvent::Popup(PopupList::new(
                    "Outputs",
                    OUTPUT_PRESETS.iter().map(|s| s.to_string()).collect(),
                    0,
                ))
            }
            (Row::Param(p), Action::Changed(v)) => Self::param_command(&p.label, v),
            (Row::Param(p), Action::Reset) => {
                // The firmware's defaults, survey-firmware 1.3.
                let v = match p.label.as_str() {
                    "Cutoff Frequency" => 80.0,
                    "Harmonics" => 0.0,
                    "Drive" => 6.0,
                    "Character" => 50.0,
                    _ => 0.0,
                };
                Self::param_command(&p.label, v)
            }
            _ => ScreenEvent::Handled,
        }
    }
}

impl Screen for PsybassPanel {
    fn title(&self) -> String {
        "Psychoacoustic Bass".into()
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
        let header = Header::new(
            "Psychoacoustic Bass",
            "Phantom fundamental bass enhancement",
        )
        .toggle(state.psybass().enabled)
        .enabled(Self::supported(state));
        self.body.draw(area, buf, theme, &header, &rows, focused);
    }

    fn handle(&mut self, key: KeyEvent, state: &DeviceState) -> ScreenEvent {
        if !Self::supported(state) {
            return ScreenEvent::Unhandled;
        }
        let rows = self.rows(state, panel::key_theme());
        self.body.clamp(&rows);
        match panel::dispatch(&mut self.body, &rows, key) {
            panel::Step::Done(ev) => ev,
            panel::Step::Header => ScreenEvent::Command(format!(
                "bass.on {}",
                if state.psybass().enabled { "off" } else { "on" }
            )),
            panel::Step::Commit(i, v) => match &rows[i] {
                Row::Param(p) => Self::param_command(&p.label, v.clamp(p.min, p.max)),
                _ => ScreenEvent::Handled,
            },
            panel::Step::Row(i, action) => self.act(&rows[i], action, state),
        }
    }

    fn keys(&self) -> &'static [KeyHelp] {
        KEYS
    }

    fn popup_result(&mut self, choice: Option<usize>, state: &DeviceState) -> ScreenEvent {
        match (self.pending.take(), choice) {
            (Some(Pending::Outputs), Some(i)) => {
                let all = panel::all_outputs_mask(state);
                let mask = match i {
                    0 => all,
                    1 => all & !(1u16 << panel::sub_output(state)),
                    _ => 0x0000,
                };
                ScreenEvent::Command(format!("bass.mask 0x{mask:X}"))
            }
            (Some(Pending::StartingPoint), Some(i)) => {
                let (_, _, cutoff, harmonics, drive, character, original) =
                    STARTING_POINTS[i.min(STARTING_POINTS.len() - 1)];
                ScreenEvent::Command(
                    [
                        format!("bass.cutoff {}", number(cutoff)),
                        format!("bass.harmonics {}", number(harmonics)),
                        format!("bass.drive {}", number(drive)),
                        format!("bass.character {}", number(character)),
                        format!("bass.original {}", number(original)),
                    ]
                    .join("\n"),
                )
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

    fn section(name: &str) -> usize {
        dspi_proto::generated::SECTIONS
            .iter()
            .find(|(n, _, _)| *n == name)
            .map(|(_, o, _)| *o)
            .expect("section")
    }

    fn panel() -> (PsybassPanel, DeviceState) {
        let mut state = testing::state();
        let b = section("psybass");
        state.bulk.patch(b, &[1, 0]);
        state.bulk.patch(b + 2, &0x00FFu16.to_le_bytes());
        state.bulk.patch(b + 4, &80.0f32.to_le_bytes());
        state.bulk.patch(b + 8, &0.0f32.to_le_bytes());
        state.bulk.patch(b + 12, &6.0f32.to_le_bytes());
        state.bulk.patch(b + 16, &50.0f32.to_le_bytes());
        state.bulk.patch(b + 20, &0.0f32.to_le_bytes());
        (PsybassPanel::new(), state)
    }

    #[test]
    fn the_panel_carries_the_consoles_sections_and_the_end_labels() {
        let (mut p, state) = panel();
        let f = testing::draw(&mut p, &state, 100, 44);
        assert!(
            f.contains("Psychoacoustic Bass · Phantom fundamental bass enhancement"),
            "{f}"
        );
        assert!(f.contains("SPECTRUM"), "{f}");
        assert!(f.contains("fc") && f.contains("4fc"), "the marks: {f}");
        assert!(
            f.contains("STARTING POINTS") && f.contains("Apply preset ▾"),
            "{f}"
        );
        assert!(f.contains("OUTPUTS") && f.contains("Presets ▾"), "{f}");
        assert!(f.contains("Cutoff Frequency") && f.contains("80 Hz"), "{f}");
        assert!(f.contains("Character"), "{f}");
        assert!(f.contains("Warm") && f.contains("Aggressive"), "{f}");
        assert!(f.contains("Original Bass"), "{f}");
    }

    #[test]
    fn an_old_firmware_gets_the_consoles_banner_instead_of_the_body() {
        // The plain fixture reports no features at all, which is what an old
        // firmware looks like after probing.
        let state = crate::shell::fixture::state();
        let mut p = PsybassPanel::new();
        let f = testing::draw(&mut p, &state, 100, 40);
        assert!(
            f.contains("Requires firmware with wire format V23 or newer."),
            "{f}"
        );
        assert!(
            f.contains("Update the DSPi firmware to use Psychoacoustic Bass."),
            "{f}"
        );
        assert!(!f.contains("SPECTRUM"), "the body is gone: {f}");
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &state),
            ScreenEvent::Unhandled,
            "and nothing on it is operable"
        );
    }

    /// D47: the four dB rows here are the clearest case. Harmonics runs
    /// -24..12 and is a signed quantity; Drive 0..18 and Original Bass -60..0
    /// are not, and both used to carry a `+`.
    #[test]
    fn only_the_db_row_whose_range_crosses_zero_carries_a_sign() {
        let (mut p, state) = panel();
        let f = testing::draw(&mut p, &state, 100, 60);
        assert!(f.contains("Harmonics"), "{f}");
        assert!(
            f.lines()
                .any(|l| l.contains("Harmonics") && l.contains('+'))
                || f.lines()
                    .any(|l| l.contains("Harmonics") && l.contains("-")),
            "a signed row keeps its sign: {f}"
        );
        for row in ["Drive", "Original Bass"] {
            let line = f
                .lines()
                .find(|l| l.contains(row))
                .unwrap_or_else(|| panic!("no {row} row:\n{f}"));
            assert!(
                !line.contains('+'),
                "{row} is not a signed quantity: {line}"
            );
            assert!(line.contains(" dB"), "{line}");
        }
    }

    #[test]
    fn golden_frames_at_both_sizes() {
        let (_, state) = panel();
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            let f = testing::frame(Tool::Psybass, Box::new(PsybassPanel::new()), &state, w, h);
            assert_eq!(f.lines().count(), h as usize, "{w}x{h}");
            assert!(f.contains("Psychoacoustic Bass"), "{w}x{h}:\n{f}");
            assert!(f.contains("P closes"), "{w}x{h}:\n{f}");
        }
    }

    #[test]
    fn a_starting_point_writes_all_five_parameters() {
        let (mut p, state) = panel();
        let rows = p.rows(&state, panel::key_theme());
        p.body.focus = panel::focus_rows(&rows)
            .iter()
            .position(|r| matches!(rows[*r], Row::Menu { .. }))
            .unwrap()
            + 1;
        match p.handle(key(KeyCode::Enter), &state) {
            ScreenEvent::Popup(list) => {
                assert_eq!(list.items[1], "Small Bluetooth - Portable speaker");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            p.popup_result(Some(1), &state),
            ScreenEvent::Command(
                "bass.cutoff 100\nbass.harmonics 3\nbass.drive 9\nbass.character 40\n\
                 bass.original -12"
                    .into()
            )
        );
    }

    #[test]
    fn the_outputs_menu_can_exclude_the_sub() {
        let (mut p, state) = panel();
        let rows = p.rows(&state, panel::key_theme());
        p.body.focus = panel::focus_rows(&rows)
            .iter()
            .position(|r| matches!(rows[*r], Row::Chips { .. }))
            .unwrap()
            + 1;
        match p.handle(key(KeyCode::Char('p')), &state) {
            ScreenEvent::Popup(list) => assert_eq!(list.items[1], "Exclude sub (recommended)"),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            p.popup_result(Some(1), &state),
            ScreenEvent::Command("bass.mask 0xFF".into()),
            "nine outputs less the ninth"
        );
        assert_eq!(
            p.popup_result(Some(0), &state),
            ScreenEvent::Handled,
            "the menu closed, so a second answer is ignored"
        );
    }

    #[test]
    fn the_parameters_step_at_the_consoles_grain() {
        let (mut p, state) = panel();
        let rows = p.rows(&state, panel::key_theme());
        let params: Vec<usize> = panel::focus_rows(&rows)
            .iter()
            .enumerate()
            .filter(|(_, r)| matches!(rows[**r], Row::Param(_)))
            .map(|(i, _)| i + 1)
            .collect();
        p.body.focus = params[0];
        assert_eq!(
            p.handle(key(KeyCode::Right), &state),
            ScreenEvent::Command("bass.cutoff 81".into())
        );
        p.body.focus = params[1];
        assert_eq!(
            p.handle(key(KeyCode::Right), &state),
            ScreenEvent::Command("bass.harmonics 0.5".into())
        );
        p.body.focus = params[4];
        assert_eq!(
            p.handle(key(KeyCode::Left), &state),
            ScreenEvent::Command("bass.original -1".into())
        );
    }

    #[test]
    fn every_advertised_key_is_handled_somewhere() {
        let (_, state) = panel();
        for help in KEYS {
            for k in crate::screens::tests::keys_for(help.key) {
                let mut hit = false;
                for focus in 0..10 {
                    let mut p = PsybassPanel::new();
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
