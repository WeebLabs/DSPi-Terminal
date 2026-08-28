//! Settings > Global Parameters.
//!
//! Everything that lives in the device's flash directory rather than in a
//! preset. Unlike every other page, edits here are staged in a local draft and
//! not written until Save, so the directory sector is not re-flashed on every
//! picker change. That is the Console's `GlobalSettingsDraft`, and the save
//! bar's first dirty category.

use dspi_proto::packets::DacHwMuteConfig;
use dspi_session::DeviceState;

use crate::widgets::{Action, KeyHelp, PopupList};

use super::{Cx, PageEvent, Row, SettingsData, SettingsPage};

/// The Console's `GlobalSettingsDraft`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub startup_mode: u8,
    pub default_slot: u8,
    pub master_volume_mode: u8,
    pub output_config_mode: u8,
    pub dac: DacHwMuteConfig,
}

impl Draft {
    pub fn from_device(state: &DeviceState, data: &SettingsData) -> Self {
        let d = state.dac_hw_mute();
        Self {
            startup_mode: data.startup_mode(),
            default_slot: data.default_slot(),
            master_volume_mode: data.master_volume_mode(),
            output_config_mode: data.output_config_mode(),
            dac: DacHwMuteConfig {
                enabled: d.enabled,
                active_low: d.active_low,
                pin: d.pin,
                hold_ms: d.hold_ms,
                release_ms: d.release_ms,
            },
        }
    }
}

const STARTUP_MODES: [&str; 2] = ["Specified Default", "Last Used"];
const PERSIST_MODES: [&str; 2] = ["Independent", "With Preset"];
const POLARITY: [&str; 2] = ["Active Low", "Active High"];
const HOLD_MS: [u16; 5] = [5, 10, 20, 50, 100];
const RELEASE_MS: [u16; 6] = [0, 5, 10, 20, 50, 100];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    StartupMode,
    DefaultPreset,
    MuteEnable,
    Polarity,
    MutePin,
    Hold,
    Release,
    Test,
    MasterMode,
    HardwareMode,
}

pub struct GlobalPage {
    cursor: usize,
    draft: Draft,
    /// True once the person has changed something, which is what stops fresh
    /// device data from looking unsaved (the Console's `globalUserEdited`).
    edited: bool,
    /// Which picker opened the popup that is up.
    popup_item: Option<Item>,
}

const KEYS: &[KeyHelp] = &[
    KeyHelp::new("↑ ↓", "Move between controls"),
    KeyHelp::new("← →", "Change"),
    KeyHelp::new("Enter", "Open the list, or start the test"),
    KeyHelp::new("Space", "Toggle"),
    KeyHelp::new("Tab", "Sidebar, page, save bar"),
    KeyHelp::new("[ ]", "Back, forward"),
    KeyHelp::new("Esc", "Leave Settings"),
];

impl GlobalPage {
    pub fn new(state: &DeviceState, data: &SettingsData) -> Self {
        Self {
            cursor: 0,
            draft: Draft::from_device(state, data),
            edited: false,
            popup_item: None,
        }
    }

    /// Whether the staged draft differs from what the device holds.
    pub fn dirty(&self, state: &DeviceState, data: &SettingsData) -> bool {
        self.edited && self.draft != Draft::from_device(state, data)
    }

    /// Take the draft as saved; the next `dirty` reads clean.
    pub fn mark_saved(&mut self) {
        self.edited = false;
    }

    pub fn revert(&mut self) {
        self.edited = false;
    }

    /// The writes Save issues, in the Console's order and only for what
    /// actually changed.
    pub fn save_commands(&self, state: &DeviceState, data: &SettingsData) -> Vec<String> {
        let device = Draft::from_device(state, data);
        let d = &self.draft;
        let mut out = Vec::new();
        if d.startup_mode != device.startup_mode || d.default_slot != device.default_slot {
            out.push(format!(
                "preset.startup mode={} slot={}",
                if d.startup_mode == 1 {
                    "last_active"
                } else {
                    "specified"
                },
                d.default_slot
            ));
        }
        if d.master_volume_mode != device.master_volume_mode {
            out.push(format!(
                "vol.master.mode {}",
                if d.master_volume_mode == 1 {
                    "with-preset"
                } else {
                    "independent"
                }
            ));
        }
        if d.output_config_mode != device.output_config_mode {
            out.push(format!(
                "preset.iomode {}",
                if d.output_config_mode == 1 {
                    "with-preset"
                } else {
                    "independent"
                }
            ));
        }
        if d.dac != device.dac {
            out.push(format!(
                "dev.dacmute enabled={} active_low={} pin={} hold_ms={} release_ms={}",
                if d.dac.enabled { "on" } else { "off" },
                if d.dac.active_low { "on" } else { "off" },
                d.dac.pin,
                d.dac.hold_ms,
                d.dac.release_ms
            ));
        }
        out
    }

    /// `n: <name|Empty>`, the Console's `slotLabel`.
    fn slot_label(data: &SettingsData, slot: u8) -> String {
        let name = if data.occupied(slot) {
            let n = data
                .preset_names
                .get(slot as usize)
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            if n.is_empty() {
                format!("Preset {}", slot + 1)
            } else {
                n
            }
        } else {
            "Empty".to_string()
        };
        format!("{}: {name}", slot + 1)
    }

    fn pin_choices(&self, cx: &Cx<'_>) -> Vec<u8> {
        cx.free_pins("DAC Mute", Some(self.draft.dac.pin))
    }

    fn build(&self, cx: &Cx<'_>) -> Vec<(Option<Item>, Row)> {
        let d = &self.draft;
        let mut rows: Vec<(Option<Item>, Row)> = vec![
            (None, Row::section("Startup Preset")),
            (
                Some(Item::StartupMode),
                Row::Pick {
                    label: "Mode".into(),
                    choices: STARTUP_MODES.iter().map(|s| (*s).to_string()).collect(),
                    selected: d.startup_mode.min(1) as usize,
                    caption: None,
                    enabled: true,
                },
            ),
        ];
        if d.startup_mode == 0 {
            rows.push((
                Some(Item::DefaultPreset),
                Row::Pick {
                    label: "Default Preset".into(),
                    choices: (0..10).map(|s| Self::slot_label(cx.data, s)).collect(),
                    selected: d.default_slot.min(9) as usize,
                    caption: None,
                    enabled: true,
                },
            ));
        }
        rows.push((
            None,
            Row::note("Choose which preset loads when the device powers on."),
        ));

        if cx.feature("dac_hardware_mute") {
            rows.push((None, Row::Blank));
            rows.push((None, Row::section("External Mute Control")));
            rows.push((
                None,
                Row::warning(
                    "Adjust only with audio stopped",
                    "Changing these settings while audio is playing can send a loud pop or \
                     full-level transient to your amplifier and speakers. Stop playback before \
                     making changes.",
                ),
            ));
            rows.push((
                Some(Item::MuteEnable),
                Row::Toggle {
                    label: "Enable Automatic Mute".into(),
                    on: d.dac.enabled,
                    caption: Some(
                        "Briefly mute an external DAC or amplifier to suppress loud pops during \
                         system state changes."
                            .into(),
                    ),
                    enabled: true,
                },
            ));
            if d.dac.enabled {
                rows.push((
                    Some(Item::Polarity),
                    Row::Pick {
                        label: "Polarity".into(),
                        choices: POLARITY.iter().map(|s| (*s).to_string()).collect(),
                        selected: usize::from(!d.dac.active_low),
                        caption: None,
                        enabled: true,
                    },
                ));
                let pins = self.pin_choices(cx);
                rows.push((
                    Some(Item::MutePin),
                    Row::Pick {
                        label: "Mute Pin".into(),
                        choices: pins.iter().map(|p| format!("GPIO {p}")).collect(),
                        selected: pins.iter().position(|p| *p == d.dac.pin).unwrap_or(0),
                        caption: None,
                        enabled: !pins.is_empty(),
                    },
                ));
                rows.push((
                    Some(Item::Hold),
                    Row::Pick {
                        label: "Hold Time".into(),
                        choices: HOLD_MS.iter().map(|m| format!("{m} ms")).collect(),
                        selected: HOLD_MS
                            .iter()
                            .position(|m| *m == d.dac.hold_ms)
                            .unwrap_or(0),
                        caption: None,
                        enabled: true,
                    },
                ));
                rows.push((
                    Some(Item::Release),
                    Row::Pick {
                        label: "Release Time".into(),
                        choices: RELEASE_MS.iter().map(|m| format!("{m} ms")).collect(),
                        selected: RELEASE_MS
                            .iter()
                            .position(|m| *m == d.dac.release_ms)
                            .unwrap_or(0),
                        caption: None,
                        enabled: true,
                    },
                ));
                // The Console tests the device's live configuration, not the
                // draft, so a staged change has to be saved first.
                let live = cx.state.dac_hw_mute();
                let can_test = cx.connected
                    && live.enabled
                    && live.pin != DacHwMuteConfig::PIN_NONE
                    && !cx.global_dirty;
                rows.push((
                    Some(Item::Test),
                    Row::Buttons {
                        label: "Test".into(),
                        caption: Some(if cx.global_dirty {
                            "Save your changes first to test the mute pin.".into()
                        } else {
                            "Toggle automatic mute for one second to confirm hardware \
                             configuration."
                                .to_string()
                        }),
                        buttons: vec!["Start".into()],
                        cursor: 0,
                        enabled: can_test,
                    },
                ));
            }
        }

        rows.push((None, Row::Blank));
        rows.push((None, Row::section("Master Volume")));
        rows.push((
            Some(Item::MasterMode),
            Row::Pick {
                label: "Mode".into(),
                choices: PERSIST_MODES.iter().map(|s| (*s).to_string()).collect(),
                selected: d.master_volume_mode.min(1) as usize,
                caption: None,
                enabled: true,
            },
        ));
        rows.push((
            None,
            Row::note(if d.master_volume_mode == 0 {
                "Master volume is stored on the device independently of presets and applied at \
                 boot. Loading a preset never changes it."
            } else {
                "Master volume is part of each preset. Saved with the preset, restored on preset \
                 load."
            }),
        ));

        rows.push((None, Row::Blank));
        rows.push((None, Row::section("Hardware Configuration")));
        rows.push((
            Some(Item::HardwareMode),
            Row::Pick {
                label: "Mode".into(),
                choices: PERSIST_MODES.iter().map(|s| (*s).to_string()).collect(),
                selected: d.output_config_mode.min(1) as usize,
                caption: None,
                enabled: true,
            },
        ));
        rows.push((
            None,
            Row::note(if d.output_config_mode == 0 {
                "Input and output configuration is stored on the device independently and applied \
                 at boot. Loading a preset never changes your wiring."
            } else {
                "Input and output configuration is part of each preset. Saved with the preset and \
                 restored on preset load."
            }),
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
        let d = &mut self.draft;
        match item {
            Item::StartupMode => d.startup_mode = choice.min(1) as u8,
            Item::DefaultPreset => d.default_slot = choice.min(9) as u8,
            Item::Polarity => d.active_low_from(choice),
            Item::MutePin => {
                let pins = cx.free_pins("DAC Mute", Some(d.dac.pin));
                if let Some(p) = pins.get(choice) {
                    d.dac.pin = *p;
                }
            }
            Item::Hold => {
                if let Some(m) = HOLD_MS.get(choice) {
                    d.dac.hold_ms = *m;
                }
            }
            Item::Release => {
                if let Some(m) = RELEASE_MS.get(choice) {
                    d.dac.release_ms = *m;
                }
            }
            Item::MasterMode => d.master_volume_mode = choice.min(1) as u8,
            Item::HardwareMode => d.output_config_mode = choice.min(1) as u8,
            _ => return PageEvent::Handled,
        }
        self.edited = true;
        PageEvent::Handled
    }
}

impl Draft {
    fn active_low_from(&mut self, choice: usize) {
        self.dac.active_low = choice == 0;
    }
}

impl SettingsPage for GlobalPage {
    fn rows(&self, cx: &Cx<'_>) -> Vec<Row> {
        self.build(cx).into_iter().map(|(_, r)| r).collect()
    }

    fn act(&mut self, index: usize, action: Action, cx: &Cx<'_>) -> PageEvent {
        let Some(item) = self.item(index, cx) else {
            return PageEvent::Handled;
        };
        match (item, action) {
            (Item::MuteEnable, Action::Toggled(on)) => {
                self.draft.dac.enabled = on;
                // Enabling with no pin is nonsensical, so the first free GPIO
                // is taken, as the Console does.
                if on
                    && self.draft.dac.pin == DacHwMuteConfig::PIN_NONE
                    && let Some(p) = cx.free_pins("DAC Mute", None).first()
                {
                    self.draft.dac.pin = *p;
                }
                self.edited = true;
                PageEvent::Handled
            }
            (Item::Test, Action::Open) => PageEvent::Command("dev.dacmute.test".into()),
            (_, Action::Selected(i)) => self.apply(item, i, cx),
            (_, Action::Open) => {
                // A picker with more than a handful of entries gets the list.
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
}

#[cfg(test)]
mod tests {
    use super::super::Page;
    use super::super::tests::{data, frame, key, screen, state};
    use super::*;
    use crate::shell::{Screen, ScreenEvent};
    use crossterm::event::KeyCode;

    #[test]
    fn the_page_carries_every_section_in_the_consoles_order() {
        let (mut s, st) = screen(Page::Global);
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            let f = frame(&mut s, &st, w, h);
            assert_eq!(f.lines().count(), h as usize);
            assert!(f.contains("STARTUP PRESET"), "{w}x{h}: {f}");
            assert!(f.contains("Specified Default"), "{w}x{h}: {f}");
        }
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("3: Living Room"), "the default preset: {f}");
        assert!(
            f.contains("Choose which preset loads when the device powers on."),
            "{f}"
        );
        assert!(f.contains("EXTERNAL MUTE CONTROL"), "{f}");
        assert!(f.contains("▲ Adjust only with audio stopped"), "{f}");
        assert!(f.contains("Enable Automatic Mute"), "{f}");
    }

    #[test]
    fn the_lower_sections_carry_their_captions() {
        let (mut s, st) = screen(Page::Global);
        s.handle(key(KeyCode::Tab), &st);
        for _ in 0..10 {
            s.handle(key(KeyCode::Down), &st);
        }
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("MASTER VOLUME"), "{f}");
        assert!(f.contains("HARDWARE CONFIGURATION"), "{f}");
        assert!(f.contains("Independent"), "{f}");
        assert!(
            f.contains("Loading a preset never changes your wiring."),
            "{f}"
        );
    }

    #[test]
    fn the_draft_is_staged_and_only_written_on_save() {
        let st = state();
        let d = data();
        let mut p = GlobalPage::new(&st, &d);
        assert!(!p.dirty(&st, &d));
        let cx = Cx {
            state: &st,
            data: &d,
            config: &super::super::AppConfig::default(),
            connected: true,
            global_dirty: false,
        };
        // Startup mode is the first focusable row.
        assert_eq!(p.act(0, Action::Selected(1), &cx), PageEvent::Handled);
        assert!(p.dirty(&st, &d));
        let cmds = p.save_commands(&st, &d);
        assert_eq!(cmds, vec!["preset.startup mode=last_active slot=2"]);
        p.mark_saved();
        assert!(!p.dirty(&st, &d));
    }

    #[test]
    fn every_changed_field_gets_its_own_write() {
        let st = state();
        let d = data();
        let mut p = GlobalPage::new(&st, &d);
        p.draft.startup_mode = 1;
        p.draft.master_volume_mode = 1;
        p.draft.output_config_mode = 1;
        p.draft.dac.hold_ms = 20;
        p.edited = true;
        let cmds = p.save_commands(&st, &d);
        assert_eq!(
            cmds,
            vec![
                "preset.startup mode=last_active slot=2",
                "vol.master.mode with-preset",
                "preset.iomode with-preset",
                "dev.dacmute enabled=on active_low=on pin=11 hold_ms=20 release_ms=0",
            ]
        );
    }

    #[test]
    fn the_mute_test_waits_for_the_draft_to_be_saved() {
        let st = state();
        let d = data();
        let mut p = GlobalPage::new(&st, &d);
        let dirty_cx = Cx {
            state: &st,
            data: &d,
            config: &super::super::AppConfig::default(),
            connected: true,
            global_dirty: true,
        };
        let rows = p.build(&dirty_cx);
        let test = rows
            .iter()
            .find(|(i, _)| *i == Some(Item::Test))
            .map(|(_, r)| r)
            .expect("the test row");
        match test {
            Row::Buttons {
                enabled, caption, ..
            } => {
                assert!(!enabled);
                assert_eq!(
                    caption.as_deref(),
                    Some("Save your changes first to test the mute pin.")
                );
            }
            other => panic!("{other:?}"),
        }
        // Clean, it fires the one-second pulse.
        let clean = Cx {
            state: &st,
            data: &d,
            config: &super::super::AppConfig::default(),
            connected: true,
            global_dirty: false,
        };
        let index = p
            .build(&clean)
            .iter()
            .filter(|(_, r)| r.focusable())
            .position(|(i, _)| *i == Some(Item::Test))
            .expect("the test row is focusable");
        assert_eq!(
            p.act(index, Action::Open, &clean),
            PageEvent::Command("dev.dacmute.test".into())
        );
    }

    #[test]
    fn enabling_the_mute_takes_the_first_free_pin() {
        let st = state();
        let d = data();
        let mut p = GlobalPage::new(&st, &d);
        p.draft.dac.enabled = false;
        p.draft.dac.pin = DacHwMuteConfig::PIN_NONE;
        let cx = Cx {
            state: &st,
            data: &d,
            config: &super::super::AppConfig::default(),
            connected: true,
            global_dirty: false,
        };
        let index = p
            .build(&cx)
            .iter()
            .filter(|(_, r)| r.focusable())
            .position(|(i, _)| *i == Some(Item::MuteEnable))
            .expect("the enable row");
        p.act(index, Action::Toggled(true), &cx);
        assert_ne!(p.draft.dac.pin, DacHwMuteConfig::PIN_NONE);
        assert!(
            !claimed(&st, &d, p.draft.dac.pin),
            "it took a pin nothing else holds"
        );
    }

    fn claimed(st: &DeviceState, d: &SettingsData, pin: u8) -> bool {
        d.pin_claims(st).iter().any(|c| c.gpio == pin)
    }

    #[test]
    fn the_pin_picker_marks_a_claimed_pin_by_leaving_it_out() {
        let st = state();
        let d = data();
        let p = GlobalPage::new(&st, &d);
        let cx = Cx {
            state: &st,
            data: &d,
            config: &super::super::AppConfig::default(),
            connected: true,
            global_dirty: false,
        };
        let pins = p.pin_choices(&cx);
        assert!(pins.contains(&11), "its own pin is always offered");
        assert!(!pins.contains(&6), "GPIO 6 is Output 1");
        assert!(!pins.contains(&14), "GPIO 14 is the I2S BCK");
    }

    #[test]
    fn enter_on_a_picker_opens_its_list() {
        let (mut s, st) = screen(Page::Global);
        s.handle(key(KeyCode::Tab), &st);
        s.handle(key(KeyCode::Down), &st);
        match s.handle(key(KeyCode::Enter), &st) {
            ScreenEvent::Popup(p) => {
                assert_eq!(p.title, "Default Preset");
                assert_eq!(p.items[0], "1: Movies");
                assert_eq!(p.items[3], "4: Empty");
            }
            other => panic!("{other:?}"),
        }
        s.popup_result(Some(5), &st);
        assert!(s.dirty(&st));
    }
}
