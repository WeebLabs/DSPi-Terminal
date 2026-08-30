//! The Volume Leveller panel: `VolumeLevellerView.swift` as a tool panel.
//!
//! The only DSP window with no graph, because the Console has none: upward
//! compression has nothing to plot against frequency. The channel masks are
//! two rows of chips over one two-byte parameter, a detector mask that decides
//! the shared gain and an apply mask that receives it.

use crossterm::event::KeyEvent;
use dspi_session::DeviceState;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::number;
use super::panel::{self, Body, ChipSpec, Header, Param, Row};
use crate::shell::{Screen, ScreenEvent};
use crate::theme::{ChannelRole, Theme};
use crate::widgets::{Action, ChipState, KeyHelp, PopupList};

/// The Console's channels menu, `VolumeLevellerView.swift:262-271`, with the
/// detector and apply masks each preset writes.
const CHANNEL_PRESETS: [(&str, u8, u8); 3] = [
    ("All channels (Night mode)", 0xFF, 0xFF),
    ("Center only (Dialog boost)", 0x04, 0x04),
    ("Front L / R only", 0x03, 0x03),
];

const SPEEDS: [&str; 3] = ["Slow", "Medium", "Fast"];

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
    Channels,
}

pub struct LevellerPanel {
    body: Body,
    chip: usize,
    pending: Option<Pending>,
}

impl Default for LevellerPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl LevellerPanel {
    pub fn new() -> Self {
        Self {
            body: Body::default(),
            chip: 0,
            pending: None,
        }
    }

    /// The Console's `channelCount`: the live input layout, 2 to 8.
    fn channels(state: &DeviceState) -> usize {
        (state.caps.num_inputs as usize).clamp(2, 8)
    }

    /// `showMasks`: only worth offering on a multichannel device that is
    /// actually carrying more than a stereo pair.
    fn shows_masks(state: &DeviceState) -> bool {
        state.caps.num_inputs > 2 && Self::channels(state) > 2
    }

    /// The Console's dynamic caption under the speed control.
    fn speed_caption(speed: u8) -> &'static str {
        match speed {
            0 => "Slow - Gentle response for music and wide dynamic range content.",
            2 => "Fast - Tight response for speech, dialogue, and podcasts.",
            _ => "Medium - Balanced response for general purpose use.",
        }
    }

    fn chips(state: &DeviceState, theme: &Theme, mask: u8, cursor: usize) -> Row {
        let chips = (0..Self::channels(state))
            .map(|c| ChipSpec {
                label: (c + 1).to_string(),
                state: if mask & (1 << c) != 0 {
                    ChipState::On
                } else {
                    ChipState::Off
                },
                color: theme.role_color(ChannelRole::Input(c as u8)),
                enabled: true,
                dimmed: false,
            })
            .collect();
        Row::Chips {
            chips,
            cursor,
            polarity: false,
        }
    }

    fn rows(&self, state: &DeviceState, theme: &Theme) -> Vec<Row> {
        let l = state.leveller();
        let mut rows = Vec::new();
        if Self::shows_masks(state) {
            rows.push(Row::Section {
                title: "Channels".into(),
                action: Some("Presets ▾".into()),
            });
            rows.push(Row::Caption("Detector  sets the shared gain".into()));
            rows.push(Self::chips(state, theme, l.detector_mask, self.chip));
            rows.push(Row::Caption("Apply  receives the gain".into()));
            rows.push(Self::chips(state, theme, l.apply_mask, self.chip));
            rows.push(Row::Blank);
        }
        rows.push(Row::Section {
            title: "Parameters".into(),
            action: None,
        });
        rows.push(Row::Param(
            Param::new("Amount", l.amount_pct as f64, 0.0, 100.0, "%")
                .step(1.0)
                .decimals(0)
                .caption(
                    "Compression strength. Higher values reduce dynamic range more \
                     aggressively.",
                ),
        ));
        rows.push(Row::Segmented {
            label: "Speed".into(),
            choices: SPEEDS.iter().map(|s| s.to_string()).collect(),
            selected: l.speed.min(2) as usize,
            enabled: true,
        });
        rows.push(Row::Caption(Self::speed_caption(l.speed).into()));
        rows.push(Row::Param(
            Param::new("Max Gain", l.max_gain_db as f64, 0.0, 35.0, "dB")
                .step(0.5)
                .decimals(1)
                .caption("Maximum boost for quiet passages. Higher values risk amplifying noise."),
        ));
        rows.push(Row::Param(
            Param::new(
                "Gate Threshold",
                l.gate_threshold_db as f64,
                -96.0,
                0.0,
                "dB",
            )
            .step(1.0)
            .decimals(0)
            .caption(
                "Silence gate. Signals below this level are not boosted, preventing noise \
                     amplification.",
            ),
        ));
        rows.push(Row::Toggle {
            label: "Lookahead".into(),
            on: l.lookahead,
            caption: Some("Adds 5ms latency. Improves transient handling.".into()),
            enabled: true,
        });
        rows
    }

    /// The masks travel as one two-byte parameter, detector first
    /// (config.h:363-376, `REQ_SET_LEVELLER_MASKS`).
    fn mask_command(detector: u8, apply: u8) -> ScreenEvent {
        let word = detector as u16 | ((apply as u16) << 8);
        ScreenEvent::Command(format!("lev.masks 0x{word:X}"))
    }

    fn param_command(label: &str, value: f64) -> ScreenEvent {
        let path = match label {
            "Amount" => "lev.amount",
            "Max Gain" => "lev.maxgain",
            _ => "lev.gate",
        };
        ScreenEvent::Command(format!("{path} {}", number(value as f32)))
    }

    fn act(
        &mut self,
        rows: &[Row],
        index: usize,
        action: Action,
        state: &DeviceState,
    ) -> ScreenEvent {
        let l = state.leveller();
        // Which of the two chip rows the cursor is on: the first is the
        // detector mask, the second the apply mask.
        let chip_rows: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| matches!(r, Row::Chips { .. }))
            .map(|(i, _)| i)
            .collect();
        match (&rows[index], action) {
            (Row::Chips { .. }, Action::Selected(i)) => {
                self.chip = i;
                ScreenEvent::Handled
            }
            (Row::Chips { .. }, Action::Chip(i, next)) => {
                let bit = 1u8 << i;
                let set = |m: u8| {
                    if next == ChipState::On {
                        m | bit
                    } else {
                        m & !bit
                    }
                };
                if chip_rows.first() == Some(&index) {
                    Self::mask_command(set(l.detector_mask), l.apply_mask)
                } else {
                    Self::mask_command(l.detector_mask, set(l.apply_mask))
                }
            }
            (Row::Chips { .. }, Action::Open) => {
                self.pending = Some(Pending::Channels);
                ScreenEvent::Popup(PopupList::new(
                    "Channels",
                    CHANNEL_PRESETS
                        .iter()
                        .map(|(n, _, _)| n.to_string())
                        .collect(),
                    0,
                ))
            }
            (Row::Segmented { .. }, Action::Selected(i)) => ScreenEvent::Command(format!(
                "lev.speed {}",
                ["slow", "medium", "fast"][i.min(2)]
            )),
            (Row::Toggle { .. }, Action::Toggled(next)) => {
                ScreenEvent::Command(format!("lev.lookahead {}", if next { "on" } else { "off" }))
            }
            (Row::Param(p), Action::Changed(v)) => Self::param_command(&p.label, v),
            (Row::Param(p), Action::Reset) => {
                let v = match p.label.as_str() {
                    "Amount" => 50.0,
                    "Max Gain" => 12.0,
                    _ => -60.0,
                };
                Self::param_command(&p.label, v)
            }
            _ => ScreenEvent::Handled,
        }
    }
}

impl Screen for LevellerPanel {
    fn title(&self) -> String {
        "Volume Leveller".into()
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
        let header =
            Header::new("Upward Dynamic Range Compression").toggle(state.leveller().enabled);
        self.body.draw(area, buf, theme, &header, &rows, focused);
    }

    fn handle(&mut self, key: KeyEvent, state: &DeviceState) -> ScreenEvent {
        let rows = self.rows(state, panel::key_theme());
        self.body.clamp(&rows);
        match panel::dispatch(&mut self.body, &rows, key) {
            panel::Step::Done(ev) => ev,
            panel::Step::Header => ScreenEvent::Command(format!(
                "lev.on {}",
                if state.leveller().enabled {
                    "off"
                } else {
                    "on"
                }
            )),
            panel::Step::Commit(i, v) => match &rows[i] {
                Row::Param(p) => Self::param_command(&p.label, v.clamp(p.min, p.max)),
                _ => ScreenEvent::Handled,
            },
            panel::Step::Row(i, action) => self.act(&rows, i, action, state),
        }
    }

    fn keys(&self) -> &'static [KeyHelp] {
        KEYS
    }

    fn popup_result(&mut self, choice: Option<usize>, _state: &DeviceState) -> ScreenEvent {
        match (self.pending.take(), choice) {
            (Some(Pending::Channels), Some(i)) => {
                let (_, detector, apply) = CHANNEL_PRESETS[i.min(CHANNEL_PRESETS.len() - 1)];
                Self::mask_command(detector, apply)
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

    fn panel() -> (LevellerPanel, DeviceState) {
        let mut state = testing::state();
        let l = section("leveller");
        // enabled, medium, lookahead off; 50 %, 12 dB, -60 dB; masks 0x03.
        state.bulk.patch(l, &[1, 1, 0, 0]);
        state.bulk.patch(l + 4, &50.0f32.to_le_bytes());
        state.bulk.patch(l + 8, &12.0f32.to_le_bytes());
        state.bulk.patch(l + 12, &(-60.0f32).to_le_bytes());
        state.bulk.patch(l + 16, &[0x03, 0x03]);
        (LevellerPanel::new(), state)
    }

    #[test]
    fn the_panel_has_no_graph_and_carries_every_control() {
        let (mut p, state) = panel();
        let f = testing::draw(&mut p, &state, 100, 40);
        assert!(f.contains("Upward Dynamic Range Compression"), "{f}");
        assert!(
            !f.contains("Disabled"),
            "the Console has no graph here: {f}"
        );
        assert!(f.contains("CHANNELS") && f.contains("Presets ▾"), "{f}");
        assert!(f.contains("Detector") && f.contains("Apply"), "{f}");
        assert!(f.contains("Amount") && f.contains("50%"), "{f}");
        assert!(
            f.contains("Slow") && f.contains("Medium") && f.contains("Fast"),
            "{f}"
        );
        assert!(f.contains("Balanced response"), "the speed caption: {f}");
        assert!(
            f.contains("Max Gain") && f.contains("Gate Threshold"),
            "{f}"
        );
        assert!(f.contains("Lookahead"), "{f}");
    }

    #[test]
    fn the_channel_section_is_hidden_on_a_stereo_device() {
        let (mut p, mut state) = panel();
        state.caps.num_inputs = 2;
        let f = testing::draw(&mut p, &state, 100, 40);
        assert!(!f.contains("CHANNELS"), "{f}");
        assert!(f.contains("PARAMETERS"), "{f}");
    }

    #[test]
    fn golden_frames_at_both_sizes() {
        let (_, state) = panel();
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            let f = testing::frame(Tool::Leveller, Box::new(LevellerPanel::new()), &state, w, h);
            assert_eq!(f.lines().count(), h as usize, "{w}x{h}");
            assert!(f.contains("Volume Leveller"), "{w}x{h}:\n{f}");
            assert!(f.contains("V closes"), "{w}x{h}:\n{f}");
        }
    }

    #[test]
    fn the_two_chip_rows_write_the_two_halves_of_one_parameter() {
        let (mut p, state) = panel();
        let rows = p.rows(&state, panel::key_theme());
        let chip_focus: Vec<usize> = panel::focus_rows(&rows)
            .iter()
            .enumerate()
            .filter(|(_, r)| matches!(rows[**r], Row::Chips { .. }))
            .map(|(i, _)| i + 1)
            .collect();
        assert_eq!(chip_focus.len(), 2);
        p.body.focus = chip_focus[0];
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &state),
            ScreenEvent::Command("lev.masks 0x302".into()),
            "the detector mask loses channel 1, the apply mask is untouched"
        );
        p.body.focus = chip_focus[1];
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &state),
            ScreenEvent::Command("lev.masks 0x203".into())
        );
    }

    #[test]
    fn the_channel_presets_are_the_consoles_three() {
        let (mut p, state) = panel();
        let rows = p.rows(&state, panel::key_theme());
        p.body.focus = panel::focus_rows(&rows)
            .iter()
            .position(|r| matches!(rows[*r], Row::Chips { .. }))
            .unwrap()
            + 1;
        match p.handle(key(KeyCode::Char('p')), &state) {
            ScreenEvent::Popup(list) => {
                assert_eq!(list.items[0], "All channels (Night mode)");
                assert_eq!(list.items[1], "Center only (Dialog boost)");
                assert_eq!(list.items[2], "Front L / R only");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            p.popup_result(Some(1), &state),
            ScreenEvent::Command("lev.masks 0x404".into())
        );
    }

    #[test]
    fn speed_is_a_segmented_control_over_the_registrys_names() {
        let (mut p, state) = panel();
        let rows = p.rows(&state, panel::key_theme());
        p.body.focus = panel::focus_rows(&rows)
            .iter()
            .position(|r| matches!(rows[*r], Row::Segmented { .. }))
            .unwrap()
            + 1;
        assert_eq!(
            p.handle(key(KeyCode::Right), &state),
            ScreenEvent::Command("lev.speed fast".into())
        );
        assert_eq!(
            p.handle(key(KeyCode::Left), &state),
            ScreenEvent::Command("lev.speed slow".into())
        );
    }

    #[test]
    fn the_parameters_and_the_toggle_write_their_paths() {
        let (mut p, state) = panel();
        let rows = p.rows(&state, panel::key_theme());
        let focus = panel::focus_rows(&rows);
        let params: Vec<usize> = focus
            .iter()
            .enumerate()
            .filter(|(_, r)| matches!(rows[**r], Row::Param(_)))
            .map(|(i, _)| i + 1)
            .collect();
        p.body.focus = params[0];
        assert_eq!(
            p.handle(key(KeyCode::Right), &state),
            ScreenEvent::Command("lev.amount 51".into())
        );
        p.body.focus = params[1];
        assert_eq!(
            p.handle(key(KeyCode::Right), &state),
            ScreenEvent::Command("lev.maxgain 12.5".into())
        );
        p.body.focus = params[2];
        assert_eq!(
            p.handle(key(KeyCode::Left), &state),
            ScreenEvent::Command("lev.gate -61".into())
        );
        p.body.focus = focus.len();
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &state),
            ScreenEvent::Command("lev.lookahead on".into())
        );
    }

    #[test]
    fn every_advertised_key_is_handled_somewhere() {
        let (_, state) = panel();
        for help in KEYS {
            for k in crate::screens::tests::keys_for(help.key) {
                let mut hit = false;
                for focus in 0..10 {
                    let mut p = LevellerPanel::new();
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
