//! Settings > Advanced.
//!
//! Two sections, both the Console's: Channel Names with its Reset, and
//! Diagnostics with the debug toggle. The reset writes the platform's factory
//! names one channel at a time, which is what the Console does; there is no
//! opcode that does it in one go.

use crossterm::event::KeyEvent;

use crate::widgets::{Action, Button, Dialog, DialogOutcome, KeyHelp};

use super::{AppConfig, Cx, PageEvent, Row, SettingsPage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Reset,
    Debug,
}

#[derive(Debug, Default)]
pub struct AdvancedPage {
    cursor: usize,
    confirming: bool,
}

const KEYS: &[KeyHelp] = &[
    KeyHelp::new("↑ ↓", "Move between controls"),
    KeyHelp::new("Enter", "Reset, or toggle"),
    KeyHelp::new("Space", "Toggle"),
    KeyHelp::new("Tab", "Sidebar, page, save bar"),
    KeyHelp::new("[ ]", "Back, forward"),
    KeyHelp::new("Esc", "Leave Settings"),
];

impl AdvancedPage {
    fn build(&self, cx: &Cx<'_>) -> Vec<(Option<Item>, Row)> {
        vec![
            (None, Row::section("Channel Names")),
            (
                Some(Item::Reset),
                Row::Buttons {
                    label: "Reset all channel names to factory defaults.".into(),
                    caption: None,
                    buttons: vec!["Reset".into()],
                    cursor: 0,
                    enabled: cx.connected,
                },
            ),
            (None, Row::Blank),
            (None, Row::section("Diagnostics")),
            (
                Some(Item::Debug),
                Row::Toggle {
                    label: "Show Debug Information".into(),
                    on: cx.config.advanced.show_debug_info,
                    caption: Some("Display additional diagnostic data in the UI".into()),
                    enabled: true,
                },
            ),
        ]
    }

    fn item(&self, index: usize, cx: &Cx<'_>) -> Option<Item> {
        self.build(cx)
            .into_iter()
            .filter(|(_, r)| r.focusable())
            .nth(index)
            .and_then(|(i, _)| i)
    }

    /// The firmware's `get_default_channel_name()`, which is what the
    /// Console's `defaultChannelNames(for:slotTypes:)` mirrors: `USB L`,
    /// `USB R`, `USB 3`.. for the inputs, then `<type> N L/R` per output pair
    /// with the PDM sub last.
    fn factory_names(cx: &Cx<'_>) -> Vec<String> {
        let caps = &cx.state.caps;
        let types = cx.state.i2s().output_types;
        let mut names = Vec::new();
        for ch in 0..caps.num_inputs as usize {
            names.push(match ch {
                0 => "USB L".to_string(),
                1 => "USB R".to_string(),
                n => format!("USB {}", n + 1),
            });
        }
        let outputs = caps.num_outputs as usize;
        for o in 0..outputs {
            if o + 1 == outputs {
                names.push("PDM".to_string());
                continue;
            }
            let slot = o / 2;
            let side = if o % 2 == 0 { "L" } else { "R" };
            let kind = if types.get(slot) == Some(&1) {
                "I2S"
            } else {
                "SPDIF"
            };
            names.push(format!("{kind} {} {side}", slot + 1));
        }
        names
    }
}

impl SettingsPage for AdvancedPage {
    fn rows(&self, cx: &Cx<'_>) -> Vec<Row> {
        self.build(cx).into_iter().map(|(_, r)| r).collect()
    }

    fn act(&mut self, index: usize, action: Action, cx: &Cx<'_>) -> PageEvent {
        match (self.item(index, cx), action) {
            (Some(Item::Reset), Action::Open) => {
                self.confirming = true;
                PageEvent::Dialog(Dialog::confirm(
                    "Reset Channel Names?",
                    "Every channel goes back to its factory name. This cannot be undone.",
                    vec![Button::destructive("Reset"), Button::new("Cancel")],
                ))
            }
            (Some(Item::Debug), Action::Toggled(on)) => {
                let mut c: AppConfig = cx.config.clone();
                c.advanced.show_debug_info = on;
                PageEvent::Config(Box::new(c))
            }
            _ => PageEvent::Handled,
        }
    }

    fn cursor(&self) -> usize {
        self.cursor
    }

    fn set_cursor(&mut self, i: usize) {
        self.cursor = i;
    }

    fn keys(&self) -> &'static [KeyHelp] {
        KEYS
    }

    fn dialog_result(&mut self, outcome: DialogOutcome, cx: &Cx<'_>) -> PageEvent {
        if !std::mem::take(&mut self.confirming) || outcome != DialogOutcome::Button(0) {
            return PageEvent::Handled;
        }
        let lines: Vec<String> = Self::factory_names(cx)
            .into_iter()
            .enumerate()
            .map(|(c, name)| format!("ch.name {c} \"{name}\""))
            .collect();
        PageEvent::Command(lines.join("\n"))
    }

    fn key(&mut self, _key: KeyEvent, _cx: &Cx<'_>) -> PageEvent {
        PageEvent::Unhandled
    }
}

#[cfg(test)]
mod tests {
    use super::super::Page;
    use super::super::tests::{frame, key, screen};
    use super::*;
    use crate::shell::{Screen, ScreenEvent};
    use crossterm::event::KeyCode;

    #[test]
    fn advanced_carries_the_consoles_two_sections() {
        let (mut s, st) = screen(Page::Advanced);
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            let f = frame(&mut s, &st, w, h);
            assert!(f.contains("CHANNEL NAMES"), "{w}x{h}: {f}");
            assert!(
                f.contains("Reset all channel names to factory defaults."),
                "{w}x{h}: {f}"
            );
            assert!(f.contains("DIAGNOSTICS"), "{w}x{h}: {f}");
            assert!(f.contains("Show Debug Information"), "{w}x{h}: {f}");
        }
    }

    #[test]
    fn reset_confirms_and_then_renames_every_channel() {
        let (mut s, st) = screen(Page::Advanced);
        s.handle(key(KeyCode::Tab), &st);
        match s.handle(key(KeyCode::Enter), &st) {
            ScreenEvent::Dialog(d) => assert_eq!(d.title, "Reset Channel Names?"),
            other => panic!("{other:?}"),
        }
        match s.dialog_result(DialogOutcome::Button(0), &st) {
            ScreenEvent::Command(c) => {
                assert_eq!(c.lines().count(), 17);
                assert_eq!(c.lines().next().unwrap(), "ch.name 0 \"USB L\"");
                assert_eq!(c.lines().nth(8).unwrap(), "ch.name 8 \"SPDIF 1 L\"");
                assert_eq!(c.lines().last().unwrap(), "ch.name 16 \"PDM\"");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_debug_toggle_writes_the_config_file() {
        let (mut s, st) = screen(Page::Advanced);
        s.handle(key(KeyCode::Tab), &st);
        s.handle(key(KeyCode::Down), &st);
        // Saving reports where it went, or why it could not; either way the
        // toggle has been taken.
        match s.handle(key(KeyCode::Char(' ')), &st) {
            ScreenEvent::Status(_) => {}
            other => panic!("{other:?}"),
        }
        assert!(s.config.advanced.show_debug_info);
    }
}
