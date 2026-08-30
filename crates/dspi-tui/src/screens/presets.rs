//! The Preset row's menu: the ten slots and every context action the Console
//! offers on them.
//!
//! The Console puts the slots in a popup and the actions in a context menu on
//! the same control. A terminal has one popup, so both live in it, separated
//! by a rule: the slots first, since switching preset is what the row is for.

use crate::widgets::{Button, Dialog, PopupList};

use super::SharedState;

/// The ten slots the firmware has (`preset.save <index>` takes 0 to 9).
pub const SLOTS: usize = 10;

/// What the person picked out of the Preset menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetChoice {
    Slot(u8),
    Save,
    Rename,
    SetDefault,
    CopyTo,
    Clear,
    ClearAll,
}

pub struct PresetMenu;

impl PresetMenu {
    /// What a slot is called on its own: its name, or `Empty` when it has
    /// none. The Console's `presetDropdownLabel`.
    pub fn dropdown_label(shared: &SharedState, slot: u8) -> String {
        if shared.occupied & (1 << slot) == 0 {
            return "Empty".to_string();
        }
        match shared.preset_names.get(slot as usize) {
            Some(name) if !name.trim().is_empty() => name.trim().to_string(),
            _ => format!("Preset {}", slot + 1),
        }
    }

    /// `3: Living Room`, the Console's `presetLabel`.
    pub fn slot_label(shared: &SharedState, slot: u8) -> String {
        format!("{}: {}", slot + 1, Self::dropdown_label(shared, slot))
    }

    /// The Preset row's popup: the slots, a rule, then the actions.
    /// The Preset row's popup: the slots, a rule, then the actions, with
    /// what each row means alongside so the caller never counts rows. The
    /// Clear rows appear only when there is something to clear
    /// (`ContentView.swift:1249-1257`).
    pub fn popup(
        shared: &SharedState,
        active: u8,
        dirty: bool,
    ) -> (PopupList, Vec<Option<PresetChoice>>) {
        let mut items: Vec<String> = Vec::new();
        let mut choices: Vec<Option<PresetChoice>> = Vec::new();
        for slot in 0..SLOTS as u8 {
            let mut label = Self::slot_label(shared, slot);
            // Only the active slot ever carries the dirty marker, so at
            // most one `*` is ever on screen.
            if slot == active && dirty {
                label.push('*');
            }
            items.push(label);
            choices.push(Some(PresetChoice::Slot(slot)));
        }
        let mut push = |label: String, choice: Option<PresetChoice>| {
            items.push(label);
            choices.push(choice);
        };
        // A header with no text is the rule between the slots and the actions.
        push("#".into(), None);
        push("Save".into(), Some(PresetChoice::Save));
        push("Rename...".into(), Some(PresetChoice::Rename));
        // `ContentView.swift:1238-1239` disables Set as Default when the
        // startup mode already points at this slot; `default_slot` is `Some`
        // only in that mode. A leading `#` is the popup's un-pickable, dimmed
        // form, which is what a disabled menu item looks like.
        if shared.default_slot == Some(active) {
            push("#Set as Default".into(), None);
        } else {
            push("Set as Default".into(), Some(PresetChoice::SetDefault));
        }
        push("Copy to...".into(), Some(PresetChoice::CopyTo));
        if shared.occupied & (1 << active) != 0 {
            push(
                format!("Clear \"{}\"...", Self::slot_label(shared, active)),
                Some(PresetChoice::Clear),
            );
        }
        if shared.occupied != 0 {
            push("Clear All Slots...".into(), Some(PresetChoice::ClearAll));
        }
        (PopupList::new("Preset", items, active as usize), choices)
    }

    /// The `Copy to...` submenu: the nine other slots.
    pub fn copy_to_popup(shared: &SharedState, active: u8) -> (PopupList, Vec<u8>) {
        let slots: Vec<u8> = (0..SLOTS as u8).filter(|s| *s != active).collect();
        let items = slots.iter().map(|s| Self::slot_label(shared, *s)).collect();
        (PopupList::new("Copy to", items, 0), slots)
    }

    pub fn clear_dialog(label: &str) -> Dialog {
        Dialog::confirm(
            "Clear Preset?",
            format!("Clear \"{label}\" and restore factory defaults? This cannot be undone."),
            vec![Button::destructive("Clear"), Button::new("Cancel")],
        )
        .critical()
    }

    pub fn clear_all_dialog() -> Dialog {
        Dialog::confirm(
            "Clear All Presets?",
            "This will erase all preset data and names, restoring every slot to factory \
             defaults. This cannot be undone.",
            vec![Button::destructive("Clear All"), Button::new("Cancel")],
        )
        .critical()
    }

    /// The Console's Rename Preset sheet: one field labelled Name.
    pub fn rename_dialog(shared: &SharedState, slot: u8) -> Dialog {
        let current = shared
            .preset_names
            .get(slot as usize)
            .cloned()
            .unwrap_or_default();
        let mut d = Dialog::text("Rename Preset", "", current, "Name");
        d.buttons = vec![Button::new("Rename"), Button::new("Cancel")];
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared() -> SharedState {
        let mut names = vec![String::new(); SLOTS];
        names[0] = "Studio".into();
        names[2] = "Living Room".into();
        SharedState {
            preset_names: names,
            // Slots 1 and 3 hold presets; the rest are empty.
            occupied: 0b101,
            ..Default::default()
        }
    }

    #[test]
    fn the_menu_lists_the_slots_then_the_console_actions() {
        let s = shared();
        let (p, _) = PresetMenu::popup(&s, 2, true);
        assert_eq!(p.items[0], "1: Studio");
        assert_eq!(p.items[1], "2: Empty");
        assert_eq!(p.items[2], "3: Living Room*", "the dirty marker");
        assert_eq!(p.items[10], "#", "a rule, which cannot be picked");
        assert_eq!(p.items[11], "Save");
        assert_eq!(p.items[12], "Rename...");
        assert_eq!(p.items[13], "Set as Default");
        assert_eq!(p.items[14], "Copy to...");
        assert_eq!(p.items[15], "Clear \"3: Living Room\"...");
        assert_eq!(p.items[16], "Clear All Slots...");
        assert_eq!(p.cursor, 2, "opens on the active slot");
    }

    #[test]
    fn only_the_active_slot_carries_the_marker() {
        let s = shared();
        let (p, _) = PresetMenu::popup(&s, 2, false);
        assert_eq!(p.items[2], "3: Living Room");
        assert_eq!(p.items.iter().filter(|i| i.ends_with('*')).count(), 0);
    }

    /// D19: the Console greys Set as Default out when this slot is already the
    /// one the device boots into, and the answer is on hand in `default_slot`.
    #[test]
    fn set_as_default_is_disabled_on_the_slot_that_already_is_the_default() {
        let mut s = shared();
        s.default_slot = Some(2);
        let (p, _) = PresetMenu::popup(&s, 2, false);
        assert_eq!(p.items[13], "#Set as Default", "dimmed and unpickable");
        // The rest of the menu keeps its indices.
        assert_eq!(p.items[12], "Rename...");
        assert_eq!(p.items[14], "Copy to...");

        // Another slot still offers it.
        let (p, _) = PresetMenu::popup(&s, 0, false);
        assert_eq!(p.items[13], "Set as Default");
        // And so does a device whose startup mode is not "a specified slot".
        s.default_slot = None;
        let (p, _) = PresetMenu::popup(&s, 2, false);
        assert_eq!(p.items[13], "Set as Default");
    }

    #[test]
    fn indices_map_to_the_actions_and_the_rule_maps_to_nothing() {
        let s = shared();
        let (_, c) = PresetMenu::popup(&s, 2, false);
        assert_eq!(c[0], Some(PresetChoice::Slot(0)));
        assert_eq!(c[9], Some(PresetChoice::Slot(9)));
        assert_eq!(c[10], None);
        assert_eq!(c[11], Some(PresetChoice::Save));
        assert_eq!(*c.last().unwrap(), Some(PresetChoice::ClearAll));
        // An empty active slot has no Clear row; a device with nothing
        // stored has no Clear All either.
        let (p, c) = PresetMenu::popup(&s, 1, false);
        assert!(
            !p.items.iter().any(|i| i.starts_with("Clear \"")),
            "{:?}",
            p.items
        );
        assert!(c.contains(&Some(PresetChoice::ClearAll)));
        let mut empty = shared();
        empty.occupied = 0;
        let (p, _) = PresetMenu::popup(&empty, 1, false);
        assert!(
            !p.items.iter().any(|i| i.starts_with("Clear")),
            "{:?}",
            p.items
        );
    }

    #[test]
    fn copy_to_offers_the_other_nine_slots() {
        let s = shared();
        let (p, slots) = PresetMenu::copy_to_popup(&s, 2);
        assert_eq!(p.items.len(), 9);
        assert_eq!(slots.len(), 9);
        assert!(!slots.contains(&2));
        assert_eq!(p.items[0], "1: Studio");
    }

    #[test]
    fn the_confirmations_are_the_consoles_words() {
        let d = PresetMenu::clear_dialog("3: Living Room");
        assert_eq!(d.title, "Clear Preset?");
        assert_eq!(
            d.body,
            "Clear \"3: Living Room\" and restore factory defaults? This cannot be undone."
        );
        assert!(d.critical);
        let d = PresetMenu::clear_all_dialog();
        assert_eq!(d.title, "Clear All Presets?");
        assert!(
            d.body
                .starts_with("This will erase all preset data and names")
        );
        let d = PresetMenu::rename_dialog(&shared(), 2);
        assert_eq!(d.title, "Rename Preset");
        assert_eq!(d.buttons[0].label, "Rename");
    }
}
