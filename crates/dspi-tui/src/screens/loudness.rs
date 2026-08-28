//! The Loudness Compensation panel: `LoudnessView.swift` as a tool panel.
//!
//! The graph is the ISO 226:2003 curve the device is applying, drawn at the
//! Console's fixed -40 dB listening level so the shape of the compensation is
//! visible without turning the volume down; the maths is in
//! [`crate::curves`].

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

/// The Console's outputs menu, `LoudnessView.swift:294-303`.
const OUTPUT_PRESETS: [&str; 3] = ["All outputs", "Slot 1 only (Headphones)", "None"];

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
}

pub struct LoudnessPanel {
    body: Body,
    chip: usize,
    pending: Option<Pending>,
}

impl Default for LoudnessPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl LoudnessPanel {
    pub fn new() -> Self {
        Self {
            body: Body::default(),
            chip: 0,
            pending: None,
        }
    }

    /// The per-output mask shipped in wire format V19 (`REQ_SET_LOUDNESS_
    /// MASK`, config.h:499-517); older firmware compensates every output and
    /// the Console hides the selector.
    fn shows_mask(state: &DeviceState) -> bool {
        state.caps.wire_format >= 19 && state.caps.num_outputs > 0
    }

    fn rows(&self, state: &DeviceState, theme: &Theme) -> Vec<Row> {
        let g = state.global();
        let mut rows = vec![
            Row::Section {
                title: "Compensation curve".into(),
                action: None,
            },
            Row::Graph(if g.loudness_enabled {
                PanelGraph::Curves {
                    series: vec![(
                        "Compensation".into(),
                        theme.accent,
                        curves::loudness_curve(
                            g.loudness_ref_spl as f64,
                            g.loudness_intensity_pct as f64,
                        ),
                    )],
                    legend: vec![("Curve at -40dB".into(), theme.accent)],
                }
            } else {
                PanelGraph::Disabled
            }),
        ];

        if Self::shows_mask(state) {
            rows.push(Row::Blank);
            rows.push(Row::Section {
                title: "Outputs".into(),
                action: Some("Presets ▾".into()),
            });
            rows.push(Row::Caption(
                "Compensate only the outputs feeding your low-level listening chain. Keep \
                 bass-managed pairs (mains + sub) together so the crossover stays coherent."
                    .into(),
            ));
            let chips = (0..state.caps.num_outputs as usize)
                .map(|o| ChipSpec {
                    label: (o + 1).to_string(),
                    state: if g.loudness_output_mask & (1 << o) != 0 {
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
                })
                .collect();
            rows.push(Row::Chips {
                chips,
                cursor: self.chip,
                polarity: false,
            });
        }

        rows.push(Row::Blank);
        rows.push(Row::Section {
            title: "Parameters".into(),
            action: None,
        });
        rows.push(Row::Param(
            Param::new(
                "Reference SPL",
                g.loudness_ref_spl as f64,
                40.0,
                100.0,
                "dB",
            )
            .step(1.0)
            .decimals(0)
            .caption(
                "SPL at 1 kHz when USB volume is 0 dB. Lower = more compensation per dB \
                     of volume reduction.",
            ),
        ));
        rows.push(Row::Param(
            Param::new(
                "Intensity",
                g.loudness_intensity_pct as f64,
                0.0,
                200.0,
                "%",
            )
            .step(1.0)
            .decimals(0)
            .caption(
                "Scales the ISO 226 compensation. 100% = standard curve. 0% = bypassed. \
                     >100% = exaggerated.",
            ),
        ));
        rows
    }

    fn param_command(label: &str, value: f64) -> ScreenEvent {
        let path = if label.starts_with("Reference") {
            "loud.ref"
        } else {
            "loud.intensity"
        };
        ScreenEvent::Command(format!("{path} {}", number(value as f32)))
    }

    fn act(&mut self, row: &Row, action: Action, state: &DeviceState) -> ScreenEvent {
        match (row, action) {
            (Row::Chips { .. }, Action::Selected(i)) => {
                self.chip = i;
                ScreenEvent::Handled
            }
            (Row::Chips { .. }, Action::Chip(i, next)) => {
                let bit = 1u16 << i;
                let mask = state.global().loudness_output_mask;
                let mask = if next == ChipState::On {
                    mask | bit
                } else {
                    mask & !bit
                };
                ScreenEvent::Command(format!("loud.mask 0x{mask:X}"))
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
                let v = if p.label.starts_with("Reference") {
                    80.0
                } else {
                    100.0
                };
                Self::param_command(&p.label, v)
            }
            _ => ScreenEvent::Handled,
        }
    }
}

impl Screen for LoudnessPanel {
    fn title(&self) -> String {
        "Loudness Compensation".into()
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
        let header = Header::new("Loudness Compensation", "ISO 226:2003 Fletcher-Munson")
            .toggle(state.global().loudness_enabled);
        self.body.draw(area, buf, theme, &header, &rows, focused);
    }

    fn handle(&mut self, key: KeyEvent, state: &DeviceState) -> ScreenEvent {
        let rows = self.rows(state, panel::key_theme());
        self.body.clamp(&rows);
        match panel::dispatch(&mut self.body, &rows, key) {
            panel::Step::Done(ev) => ev,
            panel::Step::Header => ScreenEvent::Command(format!(
                "loud.on {}",
                if state.global().loudness_enabled {
                    "off"
                } else {
                    "on"
                }
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
                let mask = match i {
                    0 => panel::all_outputs_mask(state),
                    // The Console's "Slot 1 only": the first S/PDIF pair.
                    1 => 0x0003,
                    _ => 0x0000,
                };
                ScreenEvent::Command(format!("loud.mask 0x{mask:X}"))
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

    fn panel() -> (LoudnessPanel, DeviceState) {
        let mut state = testing::state();
        let g = section("global");
        // enabled, mask 0x1FF, 80 dB SPL reference, 100 % intensity.
        state.bulk.patch(g + 5, &[1]);
        state.bulk.patch(g + 6, &0x01FFu16.to_le_bytes());
        state.bulk.patch(g + 8, &80.0f32.to_le_bytes());
        state.bulk.patch(g + 12, &100.0f32.to_le_bytes());
        (LoudnessPanel::new(), state)
    }

    #[test]
    fn the_panel_carries_the_consoles_sections_and_captions() {
        let (mut p, state) = panel();
        let f = testing::draw(&mut p, &state, 100, 40);
        assert!(
            f.contains("Loudness Compensation · ISO 226:2003 Fletcher-Munson"),
            "{f}"
        );
        assert!(f.contains("COMPENSATION CURVE"), "{f}");
        assert!(f.contains("Curve at -40dB"), "the Console's badge: {f}");
        assert!(f.contains("OUTPUTS") && f.contains("Presets ▾"), "{f}");
        assert!(f.contains("Reference SPL") && f.contains("80 dB"), "{f}");
        assert!(f.contains("Intensity") && f.contains("100%"), "{f}");
        assert!(f.contains("standard curve"), "the caption: {f}");
    }

    #[test]
    fn the_graph_says_disabled_when_the_master_switch_is_off() {
        let (mut p, mut state) = panel();
        state.bulk.patch(section("global") + 5, &[0]);
        let f = testing::draw(&mut p, &state, 100, 40);
        assert!(f.contains("Disabled"), "{f}");
    }

    #[test]
    fn golden_frames_at_both_sizes() {
        let (_, state) = panel();
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            let f = testing::frame(Tool::Loudness, Box::new(LoudnessPanel::new()), &state, w, h);
            assert_eq!(f.lines().count(), h as usize, "{w}x{h}");
            assert!(f.contains("Loudness Compensation"), "{w}x{h}:\n{f}");
            assert!(f.contains("L closes"), "{w}x{h}:\n{f}");
        }
    }

    #[test]
    fn the_header_toggles_and_the_parameters_write_their_paths() {
        let (mut p, state) = panel();
        assert_eq!(
            p.handle(key(KeyCode::Enter), &state),
            ScreenEvent::Command("loud.on off".into())
        );
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
            ScreenEvent::Command("loud.ref 81".into())
        );
        assert_eq!(
            p.handle(key(KeyCode::Backspace), &state),
            ScreenEvent::Command("loud.ref 80".into())
        );
        p.body.focus = params[1];
        assert_eq!(
            p.handle(key(KeyCode::Left), &state),
            ScreenEvent::Command("loud.intensity 99".into())
        );
    }

    #[test]
    fn a_typed_value_is_clamped_to_the_consoles_range() {
        let (mut p, state) = panel();
        let rows = p.rows(&state, panel::key_theme());
        let param = panel::focus_rows(&rows)
            .iter()
            .position(|r| matches!(rows[*r], Row::Param(_)))
            .unwrap()
            + 1;
        p.body.focus = param;
        p.handle(key(KeyCode::Enter), &state);
        assert!(p.body.edit.is_some(), "Enter arms the field");
        for c in "999".chars() {
            p.handle(key(KeyCode::Char(c)), &state);
        }
        assert_eq!(
            p.handle(key(KeyCode::Enter), &state),
            ScreenEvent::Command("loud.ref 100".into())
        );
    }

    #[test]
    fn the_outputs_menu_writes_the_masks_the_console_offers() {
        let (mut p, state) = panel();
        let rows = p.rows(&state, panel::key_theme());
        let chips = panel::focus_rows(&rows)
            .iter()
            .position(|r| matches!(rows[*r], Row::Chips { .. }))
            .unwrap()
            + 1;
        p.body.focus = chips;
        match p.handle(key(KeyCode::Char('p')), &state) {
            ScreenEvent::Popup(list) => assert_eq!(list.items[1], "Slot 1 only (Headphones)"),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            p.popup_result(Some(1), &state),
            ScreenEvent::Command("loud.mask 0x3".into())
        );
        p.body.focus = chips;
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &state),
            ScreenEvent::Command("loud.mask 0x1FE".into()),
            "output 1 was on, so Space clears its bit"
        );
    }

    #[test]
    fn every_advertised_key_is_handled_somewhere() {
        let (_, state) = panel();
        for help in KEYS {
            for k in crate::screens::tests::keys_for(help.key) {
                let mut hit = false;
                for focus in 0..8 {
                    let mut p = LoudnessPanel::new();
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
