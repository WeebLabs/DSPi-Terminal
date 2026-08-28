//! Settings > Outputs.
//!
//! One row per physical output: the pair it drives, a `Default` mark when both
//! its type and its pin are the factory ones, the bus it speaks and the GPIO it
//! drives. Then the ADAT bulk output, and Reset Pins.
//!
//! These writes are live: they land in RAM as they are made and are flashed by
//! `dev.save.io`, which is why every one of them is an
//! [`IoCommand`](super::PageEvent::IoCommand) and why the save bar comes up
//! while the wiring is device-global.

use crate::widgets::{Action, Button, Dialog, DialogOutcome, KeyHelp, PopupList};

use super::{Cx, PageEvent, Row, SettingsPage};

/// The factory data pins for the four output slots
/// (`HardwareSettingsTab.defaultDataPins`).
pub const DEFAULT_DATA_PINS: [u8; 4] = [6, 7, 8, 9];
/// The factory pin for the PDM subwoofer.
pub const DEFAULT_PDM_PIN: u8 = 10;
/// The factory pin for the ADAT optical output (`ADAT_PIN_DEFAULT`).
pub const ADAT_PIN_DEFAULT: u8 = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Type(usize),
    Pin(usize),
    AdatEnable,
    AdatPin,
    ResetPins,
}

#[derive(Debug, Default)]
pub struct OutputsPage {
    cursor: usize,
    popup_item: Option<Item>,
    status: Option<(String, bool)>,
    confirming_reset: bool,
}

const KEYS: &[KeyHelp] = &[
    KeyHelp::new("↑ ↓", "Move between controls"),
    KeyHelp::new("← →", "Change"),
    KeyHelp::new("Enter", "Open the list, or activate"),
    KeyHelp::new("Space", "Toggle"),
    KeyHelp::new("Tab", "Sidebar, page, save bar"),
    KeyHelp::new("[ ]", "Back, forward"),
    KeyHelp::new("Esc", "Leave Settings"),
];

impl OutputsPage {
    /// How many assignable slots this device has: everything but the PDM sub.
    fn slots(cx: &Cx<'_>) -> usize {
        cx.state.output_pins().len().saturating_sub(1)
    }

    fn pin_owner(index: usize) -> String {
        // The pin map names the slots `Output n` and the sub `Subwoofer`, so a
        // picker excludes its own claim by that name.
        format!("Output {}", index + 1)
    }

    fn build(&self, cx: &Cx<'_>) -> Vec<(Option<Item>, Row)> {
        let mut rows: Vec<(Option<Item>, Row)> = vec![(None, Row::section("Slots"))];
        let pins = cx.state.output_pins();
        let types = cx.state.i2s().output_types;
        let slots = Self::slots(cx);
        for (i, pin) in pins.iter().enumerate() {
            let is_slot = i < slots;
            let title = if is_slot {
                format!("OUT {}/{}", i * 2 + 1, i * 2 + 2)
            } else {
                "Sub".to_string()
            };
            let type_default = !is_slot || types.get(i) == Some(&0);
            let default_pin = if is_slot {
                DEFAULT_DATA_PINS.get(i).copied().unwrap_or(0)
            } else {
                DEFAULT_PDM_PIN
            };
            let is_default = type_default && *pin == default_pin;
            let mark = if is_default { "  Default" } else { "" };
            rows.push((
                Some(Item::Type(i)),
                Row::Pick {
                    label: format!("{title}{mark}"),
                    choices: if is_slot {
                        vec!["S/PDIF".into(), "I2S".into()]
                    } else {
                        vec!["PDM".into()]
                    },
                    selected: if is_slot {
                        usize::from(types.get(i) == Some(&1))
                    } else {
                        0
                    },
                    caption: None,
                    // The sub is always PDM; the Console draws the same control
                    // and disables it.
                    enabled: is_slot,
                },
            ));
            let owner = if is_slot {
                Self::pin_owner(i)
            } else {
                "Subwoofer".to_string()
            };
            let candidates = cx.free_pins(&owner, Some(*pin));
            rows.push((
                Some(Item::Pin(i)),
                Row::Pick {
                    label: "    GPIO".into(),
                    choices: candidates.iter().map(|p| format!("GPIO {p}")).collect(),
                    selected: candidates.iter().position(|p| p == pin).unwrap_or(0),
                    caption: None,
                    enabled: !candidates.is_empty(),
                },
            ));
        }

        if cx.feature("adat_output") {
            let (on, pin) = cx.state.adat_output();
            rows.push((None, Row::Blank));
            rows.push((None, Row::section("Bulk Output")));
            rows.push((
                Some(Item::AdatEnable),
                Row::Toggle {
                    label: "Enable ADAT".into(),
                    on,
                    caption: Some(
                        "Stream all 8 output channels as one optical ADAT lightpipe (44.1/48 \
                         kHz, 24-bit). Runs alongside the existing outputs; drive a TOSLINK \
                         transmitter from the data pin."
                            .into(),
                    ),
                    enabled: cx.connected,
                },
            ));
            let candidates = cx.free_pins("ADAT Output", Some(pin));
            rows.push((
                Some(Item::AdatPin),
                Row::Pick {
                    label: "Serial Data".into(),
                    choices: candidates.iter().map(|p| format!("GPIO {p}")).collect(),
                    selected: candidates.iter().position(|p| *p == pin).unwrap_or(0),
                    caption: Some(format!(
                        "GPIO driving the ADAT optical output. Default GPIO {ADAT_PIN_DEFAULT}."
                    )),
                    enabled: !candidates.is_empty(),
                },
            ));
        }

        rows.push((None, Row::Blank));
        if let Some((text, err)) = &self.status {
            rows.push((None, Row::Status(text.clone(), *err)));
        }
        rows.push((
            Some(Item::ResetPins),
            Row::Buttons {
                label: String::new(),
                caption: None,
                buttons: vec!["Reset Pins".into()],
                cursor: 0,
                enabled: cx.connected,
            },
        ));
        rows
    }

    fn item(&self, index: usize, cx: &Cx<'_>) -> Option<Item> {
        self.build(cx)
            .into_iter()
            .filter(|(_, r)| r.focusable())
            .nth(index)
            .and_then(|(i, _)| i)
    }

    fn apply(&mut self, item: Item, choice: usize, cx: &Cx<'_>) -> PageEvent {
        match item {
            Item::Type(slot) => {
                if slot >= Self::slots(cx) {
                    return PageEvent::Handled;
                }
                let kind = if choice == 1 { "i2s" } else { "spdif" };
                self.status = Some((
                    format!(
                        "Output {} set to {}",
                        slot + 1,
                        if choice == 1 { "I2S" } else { "S/PDIF" }
                    ),
                    false,
                ));
                PageEvent::IoCommand(format!("out.type {slot} {kind}"))
            }
            Item::Pin(index) => {
                let owner = if index < Self::slots(cx) {
                    Self::pin_owner(index)
                } else {
                    "Subwoofer".to_string()
                };
                let current = cx.state.output_pins().get(index).copied();
                let candidates = cx.free_pins(&owner, current);
                let Some(pin) = candidates.get(choice) else {
                    return PageEvent::Handled;
                };
                self.status = Some((format!("{owner} pin set to GPIO {pin}"), false));
                PageEvent::IoCommand(format!("out.pin {index} {pin}"))
            }
            Item::AdatPin => {
                let (_, current) = cx.state.adat_output();
                let candidates = cx.free_pins("ADAT Output", Some(current));
                let Some(pin) = candidates.get(choice) else {
                    return PageEvent::Handled;
                };
                self.status = Some((format!("ADAT data pin set to GPIO {pin}"), false));
                PageEvent::IoCommand(format!("adat.pin {pin}"))
            }
            _ => PageEvent::Handled,
        }
    }

    /// Every pin back to its factory value, which is what `PIN_RESET_TO_DEFAULT`
    /// (0xFF) does one output at a time.
    fn reset_commands(cx: &Cx<'_>) -> Vec<String> {
        (0..cx.state.output_pins().len())
            .map(|i| format!("out.pin {i} 255"))
            .collect()
    }
}

impl SettingsPage for OutputsPage {
    fn rows(&self, cx: &Cx<'_>) -> Vec<Row> {
        self.build(cx).into_iter().map(|(_, r)| r).collect()
    }

    fn act(&mut self, index: usize, action: Action, cx: &Cx<'_>) -> PageEvent {
        let Some(item) = self.item(index, cx) else {
            return PageEvent::Handled;
        };
        match (item, action) {
            (Item::AdatEnable, Action::Toggled(on)) => {
                self.status = Some((
                    format!("ADAT output {}", if on { "enabled" } else { "disabled" }),
                    false,
                ));
                PageEvent::IoCommand(format!("adat.enable {}", if on { "on" } else { "off" }))
            }
            (Item::ResetPins, Action::Open) => {
                self.confirming_reset = true;
                PageEvent::Dialog(Dialog::confirm(
                    "Reset Pins?",
                    "Every output goes back to its factory GPIO. This cannot be undone.",
                    vec![Button::destructive("Reset Pins"), Button::new("Cancel")],
                ))
            }
            (_, Action::Selected(i)) => self.apply(item, i, cx),
            (_, Action::Open) => {
                let rows = self.build(cx);
                let row = rows
                    .iter()
                    .filter(|(_, r)| r.focusable())
                    .nth(index)
                    .map(|(_, r)| r);
                if let Some(Row::Pick {
                    label,
                    choices,
                    selected,
                    enabled: true,
                    ..
                }) = row
                {
                    self.popup_item = Some(item);
                    return PageEvent::Popup(PopupList::new(
                        label.trim().to_string(),
                        choices.clone(),
                        *selected,
                    ));
                }
                PageEvent::Handled
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

    fn popup_result(&mut self, choice: Option<usize>, cx: &Cx<'_>) -> PageEvent {
        let Some(item) = self.popup_item.take() else {
            return PageEvent::Handled;
        };
        match choice {
            Some(i) => self.apply(item, i, cx),
            None => PageEvent::Handled,
        }
    }

    fn dialog_result(&mut self, outcome: DialogOutcome, cx: &Cx<'_>) -> PageEvent {
        if !std::mem::take(&mut self.confirming_reset) || outcome != DialogOutcome::Button(0) {
            return PageEvent::Handled;
        }
        self.status = Some(("Output pins reset to defaults".into(), false));
        PageEvent::IoCommand(Self::reset_commands(cx).join("\n"))
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
    fn every_slot_gets_a_type_and_a_pin() {
        let (mut s, st) = screen(Page::Outputs);
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            let f = frame(&mut s, &st, w, h);
            assert_eq!(f.lines().count(), h as usize);
            assert!(f.contains("SLOTS"), "{w}x{h}: {f}");
            assert!(f.contains("OUT 1/2"), "{w}x{h}: {f}");
            assert!(f.contains("GPIO 6"), "{w}x{h}: {f}");
        }
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("S/PDIF"), "{f}");
        assert!(f.contains("OUT 3/4"), "{f}");
        assert!(f.contains("I2S"), "slot 2 is I2S in the fixture: {f}");
    }

    #[test]
    fn the_default_mark_appears_only_when_both_type_and_pin_are_factory() {
        let (mut s, st) = screen(Page::Outputs);
        let f = frame(&mut s, &st, 120, 40);
        // Slot 1 is S/PDIF on GPIO 6, both factory.
        let line = f.lines().find(|l| l.contains("OUT 1/2")).unwrap();
        assert!(line.contains("Default"), "{line}");
        // Slot 2 was switched to I2S, so it is not.
        let line = f.lines().find(|l| l.contains("OUT 3/4")).unwrap();
        assert!(!line.contains("Default"), "{line}");
    }

    #[test]
    fn the_sub_row_is_pdm_and_cannot_be_changed() {
        let (mut s, st) = screen(Page::Outputs);
        s.handle(key(KeyCode::Tab), &st);
        // Eight rows down is the sub's type row (four slots, two rows each).
        for _ in 0..8 {
            s.handle(key(KeyCode::Down), &st);
        }
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("Sub"), "{f}");
        assert!(f.contains("PDM"), "{f}");
        assert_eq!(
            s.handle(key(KeyCode::Right), &st),
            ScreenEvent::Unhandled,
            "a disabled picker never answers"
        );
    }

    #[test]
    fn changing_a_type_or_a_pin_is_an_output_config_edit() {
        let (mut s, st) = screen(Page::Outputs);
        s.handle(key(KeyCode::Tab), &st);
        match s.handle(key(KeyCode::Right), &st) {
            ScreenEvent::Command(c) => assert_eq!(c, "out.type 0 i2s"),
            other => panic!("{other:?}"),
        }
        assert!(s.output_dirty(), "the save bar's second category");
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("Output 1 set to I2S"), "the status row: {f}");
    }

    #[test]
    fn a_pin_picker_leaves_out_the_pins_other_features_hold() {
        let (mut s, st) = screen(Page::Outputs);
        s.handle(key(KeyCode::Tab), &st);
        s.handle(key(KeyCode::Down), &st);
        match s.handle(key(KeyCode::Enter), &st) {
            ScreenEvent::Popup(p) => {
                assert!(p.items.contains(&"GPIO 6".to_string()), "its own pin");
                assert!(
                    !p.items.contains(&"GPIO 7".to_string()),
                    "Output 2 holds it"
                );
                assert!(!p.items.contains(&"GPIO 14".to_string()), "the I2S BCK");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_adat_section_carries_its_caption_and_writes() {
        let (mut s, st) = screen(Page::Outputs);
        s.handle(key(KeyCode::Tab), &st);
        for _ in 0..10 {
            s.handle(key(KeyCode::Down), &st);
        }
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("BULK OUTPUT"), "{f}");
        assert!(f.contains("Enable ADAT"), "{f}");
        assert!(
            f.contains("Stream all 8 output channels as one optical ADAT lightpipe"),
            "{f}"
        );
        match s.handle(key(KeyCode::Char(' ')), &st) {
            ScreenEvent::Command(c) => assert_eq!(c, "adat.enable off"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn reset_pins_confirms_and_then_sends_the_reset_sentinel() {
        let (mut s, st) = screen(Page::Outputs);
        s.handle(key(KeyCode::Tab), &st);
        for _ in 0..20 {
            s.handle(key(KeyCode::Down), &st);
        }
        match s.handle(key(KeyCode::Enter), &st) {
            ScreenEvent::Dialog(d) => assert_eq!(d.title, "Reset Pins?"),
            other => panic!("{other:?}"),
        }
        match s.dialog_result(DialogOutcome::Button(0), &st) {
            ScreenEvent::Command(c) => {
                assert_eq!(c.lines().count(), 5);
                assert_eq!(c.lines().next().unwrap(), "out.pin 0 255");
            }
            other => panic!("{other:?}"),
        }
    }
}
