//! Settings > Inputs.
//!
//! The three input sections the Console puts on one page: S/PDIF (with the
//! optional receivers and LG Sound Sync), I2S (clock role, channel count and a
//! data pin per pair) and ADAT. Each section only appears when the device
//! answered its probe, because an absent feature is removed rather than dimmed.

use crate::widgets::{Action, Button, Dialog, DialogOutcome, KeyHelp, PopupList, StatusTone};

use super::{Cx, INPUT_RATES_HZ, PageEvent, Row, SettingsPage};

/// The default S/PDIF RX pins, and the ADAT input's "unset" sentinel.
pub const ADAT_INPUT_PIN_UNSET: u8 = 0xFF;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Instances,
    SpdifPin(usize),
    Lg,
    I2sClock,
    I2sChannels,
    I2sPin(usize),
    AdatEnable,
    AdatPin,
    AdatClock,
    EnableAdatOutput,
}

#[derive(Debug, Default)]
pub struct InputsPage {
    cursor: usize,
    popup_item: Option<Item>,
    status: Option<(String, bool)>,
    /// The clock-mode change the confirm is standing in front of.
    pending_clock: Option<u8>,
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

fn rate_label(hz: u32) -> String {
    if hz.is_multiple_of(1000) {
        format!("{} kHz", hz / 1000)
    } else {
        format!("{:.1} kHz", hz as f64 / 1000.0)
    }
}

impl InputsPage {
    /// How many S/PDIF inputs are switched on: input 1 always, plus whichever
    /// optional ones the mask names.
    fn spdif_count(cx: &Cx<'_>) -> usize {
        let mask = cx
            .state
            .input_config()
            .and_then(|c| c.spdif_rx_enabled_ext)
            .unwrap_or(0);
        1 + (0..3).filter(|i| mask & (1u8 << i) != 0).count()
    }

    fn spdif_pin(cx: &Cx<'_>, index: usize) -> u8 {
        let Some(c) = cx.state.input_config() else {
            return 0;
        };
        if index == 0 {
            c.spdif_rx_pin
        } else {
            c.spdif_rx_pin_ext
                .get(index - 1)
                .and_then(|p| *p)
                .unwrap_or(0)
        }
    }

    fn i2s_pins(cx: &Cx<'_>) -> Vec<u8> {
        let Some(c) = cx.state.input_config() else {
            return vec![0];
        };
        let mut v = vec![c.i2s_rx_pin];
        v.extend(c.i2s_rx_pin_ext.iter().map(|p| p.unwrap_or(0)));
        v
    }

    fn spdif_owner(index: usize) -> String {
        if index == 0 {
            "S/PDIF 1 RX".to_string()
        } else {
            format!("S/PDIF {} RX", index + 1)
        }
    }

    fn i2s_owner(pair: usize, pairs: usize) -> String {
        if pairs > 1 {
            format!("I2S RX {}", pair + 1)
        } else {
            "I2S RX".to_string()
        }
    }

    fn build(&self, cx: &Cx<'_>) -> Vec<(Option<Item>, Row)> {
        let mut rows: Vec<(Option<Item>, Row)> = Vec::new();
        let input = cx.state.input_config();
        let multi = cx.feature("spdif_multi_input");

        // ---- S/PDIF Input -------------------------------------------------
        rows.push((None, Row::section("S/PDIF Input")));
        let count = if multi { Self::spdif_count(cx) } else { 1 };
        if multi {
            rows.push((
                Some(Item::Instances),
                Row::Pick {
                    label: "Instances".into(),
                    choices: (1..=4).map(|n| n.to_string()).collect(),
                    selected: count.saturating_sub(1),
                    caption: Some(format!(
                        "{count} selectable input{} sharing one receiver",
                        if count == 1 { "" } else { "s" }
                    )),
                    enabled: cx.connected,
                },
            ));
        }
        for idx in 0..count {
            let title = if multi {
                format!("S/PDIF {}", idx + 1)
            } else {
                "SPDIF RX".to_string()
            };
            let pin = Self::spdif_pin(cx, idx);
            let candidates = cx.free_pins(&Self::spdif_owner(idx), Some(pin));
            rows.push((
                Some(Item::SpdifPin(idx)),
                Row::Pick {
                    label: title,
                    choices: candidates.iter().map(|p| format!("GPIO {p}")).collect(),
                    selected: candidates.iter().position(|p| *p == pin).unwrap_or(0),
                    caption: Some(if multi {
                        format!(
                            "GPIO pin for S/PDIF input {} (TOSLINK RX module or comparator).",
                            idx + 1
                        )
                    } else {
                        "GPIO pin for incoming S/PDIF signal from a TOSLINK RX module or \
                         comparator."
                            .to_string()
                    }),
                    enabled: !candidates.is_empty(),
                },
            ));
        }
        let lg_supported = cx.feature("lg_sound_sync");
        rows.push((
            Some(Item::Lg),
            Row::Toggle {
                label: "LG Sound Sync".into(),
                on: cx.state.lg_sound_sync().enabled,
                caption: Some(
                    "Decode the LG TV's TOSLINK volume + mute signaling and apply it as the host \
                     volume - TV remote becomes the volume control. Per-preset; saved with the \
                     active preset."
                        .into(),
                ),
                enabled: lg_supported && cx.connected,
            },
        ));
        if !lg_supported {
            rows.push((
                None,
                Row::note(
                    "Connected device firmware doesn't support LG Sound Sync. Update to firmware \
                     V8 or later.",
                ),
            ));
        }

        // ---- I2S Input ----------------------------------------------------
        if cx.feature("i2s_input_channels") {
            let channels = input
                .as_ref()
                .and_then(|c| c.i2s_input_channels)
                .unwrap_or(2);
            let pairs = (channels.max(2) / 2) as usize;
            rows.push((None, Row::Blank));
            rows.push((None, Row::section("I2S Input")));
            if cx.feature("i2s_slave_clock") {
                let mode = input.as_ref().map(|c| c.i2s_clock_mode).unwrap_or(0);
                rows.push((
                    Some(Item::I2sClock),
                    Row::Pick {
                        label: "Clock Mode".into(),
                        choices: vec!["Master".into(), "Slave".into()],
                        selected: usize::from(mode == 1),
                        caption: Some(
                            "Master: DSPi drives BCK/LRCLK. Slave: an external master drives the \
                             clocks and the rate is auto-detected."
                                .into(),
                        ),
                        enabled: cx.connected,
                    },
                ));
                if mode == 1 {
                    let (state, rate) = cx.state.i2s_slave_state.unwrap_or((0, 0));
                    let locked = state == 2;
                    rows.push((
                        None,
                        Row::Pill {
                            label: "Lock Status".into(),
                            text: if locked { "Locked" } else { "Waiting" }.into(),
                            tone: if locked {
                                StatusTone::Ok
                            } else {
                                StatusTone::Warning
                            },
                            caption: Some(if locked {
                                format!("Locked to external clock at {}.", rate_label(rate))
                            } else {
                                format!("Waiting for external clock (measured {rate} Hz).")
                            }),
                        },
                    ));
                }
            }
            rows.push((
                Some(Item::I2sChannels),
                Row::Pick {
                    label: "Channels".into(),
                    choices: vec!["2".into(), "4".into(), "6".into(), "8".into()],
                    selected: (pairs.max(1) - 1).min(3),
                    caption: Some(format!(
                        "{pairs} stereo pair{} of 24-bit audio, sample-aligned",
                        if pairs == 1 { "" } else { "s" }
                    )),
                    enabled: cx.connected,
                },
            ));
            let pins = Self::i2s_pins(cx);
            for pair in 0..pairs {
                let pin = pins.get(pair).copied().unwrap_or(0);
                let candidates = cx.free_pins(&Self::i2s_owner(pair, pairs), Some(pin));
                rows.push((
                    Some(Item::I2sPin(pair)),
                    Row::Pick {
                        label: format!("Serial Data {}", pair + 1),
                        choices: candidates.iter().map(|p| format!("GPIO {p}")).collect(),
                        selected: candidates.iter().position(|p| *p == pin).unwrap_or(0),
                        caption: Some(format!(
                            "GPIO data pin for input channels {}-{}",
                            pair * 2 + 1,
                            pair * 2 + 2
                        )),
                        enabled: !candidates.is_empty(),
                    },
                ));
            }
        }

        // ---- ADAT Input ---------------------------------------------------
        if cx.feature("adat_input") {
            let enabled = input
                .as_ref()
                .and_then(|c| c.adat_input_enabled)
                .unwrap_or(false);
            let pin = input.as_ref().and_then(|c| c.adat_input_pin);
            let clock = input.as_ref().and_then(|c| c.adat_clock_mode).unwrap_or(0);
            rows.push((None, Row::Blank));
            rows.push((None, Row::section("ADAT Input")));
            rows.push((
                Some(Item::AdatEnable),
                Row::Toggle {
                    label: "Enable ADAT Input".into(),
                    on: enabled,
                    caption: Some(
                        "Receive 8 channels of 24-bit audio (44.1/48 kHz) from one TOSLINK \
                         optical input into input channels 1-8. Assign a data pin below, then \
                         select ADAT as the input source."
                            .into(),
                    ),
                    // The firmware refuses an enable with no pin.
                    enabled: cx.connected && (enabled || pin.is_some()),
                },
            ));
            let current = pin.unwrap_or(ADAT_INPUT_PIN_UNSET);
            let candidates = cx.free_pins("ADAT Input", pin);
            let mut choices: Vec<String> = Vec::new();
            if pin.is_none() {
                choices.push("Not set".into());
            }
            choices.extend(candidates.iter().map(|p| format!("GPIO {p}")));
            rows.push((
                Some(Item::AdatPin),
                Row::Pick {
                    label: "Serial Data".into(),
                    choices,
                    selected: match pin {
                        None => 0,
                        Some(p) => candidates.iter().position(|c| *c == p).unwrap_or(0),
                    },
                    caption: Some(
                        "GPIO receiving the ADAT optical input. No default - assign a spare pin \
                         (it may match the ADAT output pin for a loopback self-test)."
                            .into(),
                    ),
                    enabled: cx.connected,
                },
            ));
            rows.push((
                Some(Item::AdatClock),
                Row::Pick {
                    label: "Clock Mode".into(),
                    choices: vec!["Master".into(), "Slave".into()],
                    selected: usize::from(clock == 1),
                    caption: Some(
                        "Master: DSPi owns the sample rate (set in I2S Configuration) and the \
                         source syncs to the ADAT output. Slave: an external master owns the \
                         clock and the rate is auto-detected."
                            .into(),
                    ),
                    enabled: cx.connected,
                },
            ));
            let (adat_out, _) = cx.state.adat_output();
            if enabled && clock == 0 && !adat_out {
                rows.push((
                    None,
                    Row::warning(
                        "Clock is free-running",
                        "Master mode uses the ADAT output to clock the outboard device but the \
                         ADAT output is off. The source runs on its own clock, asynchronous to \
                         DSPi, so you will hear periodic disturbances.",
                    ),
                ));
                rows.push((
                    Some(Item::EnableAdatOutput),
                    Row::Buttons {
                        label: String::new(),
                        caption: Some("or switch to Slave mode above".into()),
                        buttons: vec!["Enable ADAT Output".into()],
                        cursor: 0,
                        enabled: cx.connected,
                    },
                ));
            }
            if enabled {
                let (st, rate, _) = cx.state.adat_input_state.unwrap_or((0, 0, 0));
                let locked = st == 3;
                rows.push((
                    None,
                    Row::Pill {
                        label: "Lock Status".into(),
                        text: if locked { "Locked" } else { "Waiting" }.into(),
                        tone: if locked {
                            StatusTone::Ok
                        } else {
                            StatusTone::Warning
                        },
                        caption: Some(if locked {
                            format!("Locked and decoding at {}.", rate_label(rate))
                        } else {
                            "Waiting for a valid ADAT signal on the data pin.".to_string()
                        }),
                    },
                ));
            }
            let _ = current;
        }

        if let Some((text, err)) = &self.status {
            rows.push((None, Row::Blank));
            rows.push((None, Row::Status(text.clone(), *err)));
        }
        rows
    }

    fn item(&self, index: usize, cx: &Cx<'_>) -> Option<Item> {
        self.build(cx)
            .into_iter()
            .filter(|(_, r)| r.focusable())
            .nth(index)
            .and_then(|(i, _)| i)
    }

    /// Whether optional S/PDIF input `idx` (1 is S/PDIF 2) is switched on.
    fn optional_enabled(cx: &Cx<'_>, idx: usize) -> bool {
        let mask = cx
            .state
            .input_config()
            .and_then(|c| c.spdif_rx_enabled_ext)
            .unwrap_or(0);
        (1..=3).contains(&idx) && mask & (1u8 << (idx - 1)) != 0
    }

    /// Enable inputs up to `target` ascending, then disable the rest
    /// descending, which is the order the Console uses so a lower input frees
    /// its state first. An input already in the wanted state is left alone.
    fn instance_commands(cx: &Cx<'_>, target: usize) -> Vec<String> {
        let mut out = Vec::new();
        if target >= 2 {
            for idx in 1..target {
                if !Self::optional_enabled(cx, idx) {
                    out.push(format!("in.spdif.enable {idx} on"));
                }
            }
        }
        for idx in (target.max(1)..4).rev() {
            if Self::optional_enabled(cx, idx) {
                out.push(format!("in.spdif.enable {idx} off"));
            }
        }
        out
    }

    fn apply(&mut self, item: Item, choice: usize, cx: &Cx<'_>) -> PageEvent {
        match item {
            Item::Instances => {
                let target = choice + 1;
                let cmds = Self::instance_commands(cx, target);
                if cmds.is_empty() {
                    return PageEvent::Handled;
                }
                self.status = Some((format!("S/PDIF inputs set to {target}"), false));
                PageEvent::IoCommand(cmds.join("\n"))
            }
            Item::SpdifPin(idx) => {
                let pin = Self::spdif_pin(cx, idx);
                let candidates = cx.free_pins(&Self::spdif_owner(idx), Some(pin));
                let Some(p) = candidates.get(choice) else {
                    return PageEvent::Handled;
                };
                self.status = Some((format!("S/PDIF {} RX pin set to GPIO {p}", idx + 1), false));
                PageEvent::IoCommand(format!("in.spdif.pin {idx} {p}"))
            }
            Item::I2sClock => {
                let mode = if choice == 1 { 1u8 } else { 0 };
                let now = cx
                    .state
                    .input_config()
                    .map(|c| c.i2s_clock_mode)
                    .unwrap_or(0);
                if mode == now {
                    return PageEvent::Handled;
                }
                // Switching the clock role restarts I2S clocking, which can
                // glitch a connected DAC, so the Console asks first while an
                // output slot is driving one.
                if cx.state.i2s().output_types.contains(&1) {
                    self.pending_clock = Some(mode);
                    return PageEvent::Dialog(Dialog::confirm(
                        "Change I2S clock mode?",
                        "One or more I2S outputs are active. Switching between Master and Slave \
                         modes may cause sustained loud noises to be emitted by the connected I2S \
                         DAC if wiring has not been adjusted.",
                        vec![
                            Button::destructive("Change Clock Mode"),
                            Button::new("Cancel"),
                        ],
                    ));
                }
                PageEvent::IoCommand(format!(
                    "in.i2s.clock {}",
                    if mode == 1 { "slave" } else { "master" }
                ))
            }
            Item::I2sChannels => {
                let count = (choice + 1) * 2;
                self.status = Some((
                    format!(
                        "I2S input set to {count} channels ({} pair{})",
                        count / 2,
                        if count == 2 { "" } else { "s" }
                    ),
                    false,
                ));
                PageEvent::IoCommand(format!("in.i2s.channels {count}"))
            }
            Item::I2sPin(pair) => {
                let pairs = Self::i2s_pins(cx).len();
                let pin = Self::i2s_pins(cx).get(pair).copied().unwrap_or(0);
                let candidates = cx.free_pins(&Self::i2s_owner(pair, pairs), Some(pin));
                let Some(p) = candidates.get(choice) else {
                    return PageEvent::Handled;
                };
                self.status = Some((format!("Serial Data {} set to GPIO {p}", pair + 1), false));
                PageEvent::IoCommand(format!("in.i2s.pin {pair} {p}"))
            }
            Item::AdatPin => {
                let pin = cx.state.input_config().and_then(|c| c.adat_input_pin);
                let candidates = cx.free_pins("ADAT Input", pin);
                let offset = usize::from(pin.is_none());
                if choice < offset {
                    return PageEvent::Handled;
                }
                let Some(p) = candidates.get(choice - offset) else {
                    return PageEvent::Handled;
                };
                self.status = Some((format!("ADAT input pin set to GPIO {p}"), false));
                PageEvent::IoCommand(format!("in.adat.pin {p}"))
            }
            Item::AdatClock => PageEvent::IoCommand(format!(
                "in.adat.clock {}",
                if choice == 1 { "slave" } else { "master" }
            )),
            _ => PageEvent::Handled,
        }
    }
}

impl SettingsPage for InputsPage {
    fn rows(&self, cx: &Cx<'_>) -> Vec<Row> {
        self.build(cx).into_iter().map(|(_, r)| r).collect()
    }

    fn act(&mut self, index: usize, action: Action, cx: &Cx<'_>) -> PageEvent {
        let Some(item) = self.item(index, cx) else {
            return PageEvent::Handled;
        };
        match (item, action) {
            (Item::Lg, Action::Toggled(on)) => {
                // Per-preset and live, so it is not an output-config edit.
                PageEvent::Command(format!("in.lg {}", if on { "on" } else { "off" }))
            }
            (Item::AdatEnable, Action::Toggled(on)) => {
                self.status = Some((
                    format!("ADAT input {}", if on { "enabled" } else { "disabled" }),
                    false,
                ));
                PageEvent::IoCommand(format!("in.adat.enable {}", if on { "on" } else { "off" }))
            }
            (Item::EnableAdatOutput, Action::Open) => {
                self.status = Some(("ADAT output enabled".into(), false));
                PageEvent::IoCommand("adat.enable on".into())
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
                        label.clone(),
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

    fn dialog_result(&mut self, outcome: DialogOutcome, _cx: &Cx<'_>) -> PageEvent {
        let Some(mode) = self.pending_clock.take() else {
            return PageEvent::Handled;
        };
        if outcome != DialogOutcome::Button(0) {
            return PageEvent::Handled;
        }
        PageEvent::IoCommand(format!(
            "in.i2s.clock {}",
            if mode == 1 { "slave" } else { "master" }
        ))
    }
}

/// The rates the I2S Configuration page offers, shared with it so the two
/// cannot disagree.
pub const RATES: [u32; 3] = INPUT_RATES_HZ;

#[cfg(test)]
mod tests {
    use super::super::Page;
    use super::super::tests::{frame, key, screen};
    use super::*;
    use crate::shell::{Screen, ScreenEvent};
    use crossterm::event::KeyCode;

    #[test]
    fn the_page_carries_all_three_input_sections() {
        let (mut s, st) = screen(Page::Inputs);
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            let f = frame(&mut s, &st, w, h);
            assert_eq!(f.lines().count(), h as usize);
            assert!(f.contains("S/PDIF INPUT"), "{w}x{h}: {f}");
            assert!(f.contains("Instances"), "{w}x{h}: {f}");
        }
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("S/PDIF 1"), "{f}");
        assert!(
            f.contains("GPIO pin for S/PDIF input 1 (TOSLINK RX module or comparator)."),
            "{f}"
        );
        assert!(f.contains("LG Sound Sync"), "{f}");
    }

    #[test]
    fn the_lower_sections_are_reachable_and_carry_their_captions() {
        let (mut s, st) = screen(Page::Inputs);
        s.handle(key(KeyCode::Tab), &st);
        for _ in 0..6 {
            s.handle(key(KeyCode::Down), &st);
        }
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("I2S INPUT"), "{f}");
        assert!(f.contains("Clock Mode"), "{f}");
        assert!(f.contains("Master: DSPi drives BCK/LRCLK."), "{f}");
        for _ in 0..8 {
            s.handle(key(KeyCode::Down), &st);
        }
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("ADAT INPUT"), "{f}");
        assert!(f.contains("Enable ADAT Input"), "{f}");
    }

    #[test]
    fn the_instance_picker_enables_and_disables_in_the_consoles_order() {
        let (mut s, st) = screen(Page::Inputs);
        s.handle(key(KeyCode::Tab), &st);
        // The fixture has two enabled, so going to three turns on input 3 and
        // leaves the one that is already on alone.
        match s.handle(key(KeyCode::Right), &st) {
            ScreenEvent::Command(c) => assert_eq!(c, "in.spdif.enable 2 on"),
            other => panic!("{other:?}"),
        }
        assert!(s.output_dirty());
        // And going down to one turns them off from the top.
        let (mut s, st) = screen(Page::Inputs);
        s.handle(key(KeyCode::Tab), &st);
        match s.handle(key(KeyCode::Left), &st) {
            ScreenEvent::Command(c) => assert_eq!(c, "in.spdif.enable 1 off"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_spdif_pin_change_is_an_output_config_edit() {
        let (mut s, st) = screen(Page::Inputs);
        s.handle(key(KeyCode::Tab), &st);
        s.handle(key(KeyCode::Down), &st);
        match s.handle(key(KeyCode::Enter), &st) {
            ScreenEvent::Popup(p) => {
                assert!(p.items.contains(&"GPIO 5".to_string()), "its own pin");
                assert!(
                    !p.items.contains(&"GPIO 21".to_string()),
                    "S/PDIF 2 holds it"
                );
            }
            other => panic!("{other:?}"),
        }
        match s.popup_result(Some(0), &st) {
            ScreenEvent::Command(c) => assert!(c.starts_with("in.spdif.pin 0 "), "{c}"),
            other => panic!("{other:?}"),
        }
        assert!(s.output_dirty());
    }

    #[test]
    fn lg_sound_sync_is_a_live_per_preset_write() {
        let (mut s, st) = screen(Page::Inputs);
        s.handle(key(KeyCode::Tab), &st);
        for _ in 0..3 {
            s.handle(key(KeyCode::Down), &st);
        }
        match s.handle(key(KeyCode::Char(' ')), &st) {
            ScreenEvent::Command(c) => assert_eq!(c, "in.lg on"),
            other => panic!("{other:?}"),
        }
        assert!(
            !s.output_dirty(),
            "it travels with the preset, not the wiring"
        );
    }

    #[test]
    fn changing_the_i2s_clock_role_asks_first_while_an_i2s_output_is_live() {
        let (mut s, st) = screen(Page::Inputs);
        s.handle(key(KeyCode::Tab), &st);
        for _ in 0..4 {
            s.handle(key(KeyCode::Down), &st);
        }
        match s.handle(key(KeyCode::Right), &st) {
            ScreenEvent::Dialog(d) => {
                assert_eq!(d.title, "Change I2S clock mode?");
                assert!(d.body.contains("sustained loud noises"), "{}", d.body);
                assert_eq!(d.buttons[0].label, "Change Clock Mode");
            }
            other => panic!("{other:?}"),
        }
        match s.dialog_result(DialogOutcome::Button(0), &st) {
            ScreenEvent::Command(c) => assert_eq!(c, "in.i2s.clock slave"),
            other => panic!("{other:?}"),
        }
    }
}
