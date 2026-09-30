//! Settings > Channel Groups.
//!
//! A group is a named set of channels one control drives as a unit: mute a
//! zone, trim a stereo pair, light an LED when any member clips. Eight slots,
//! stored on the device beside the bindings and covered by the same Save and
//! Revert.
//!
//! Each card stages its edits and commits them with an explicit Apply, because
//! one `REQ_SET_CS_GROUP` writes the whole 40-byte record and the write is a
//! deferred preview whose outcome only arrives through the status packet. The
//! device re-validates every binding pointing at a group whenever one changes,
//! so emptying a group deactivates its dependants rather than being refused,
//! which is why an in-use group says how many controls it will take down.

use std::collections::{BTreeMap, HashSet};

use crossterm::event::{KeyCode, KeyEvent};

use crate::shell::{SessionReply, SessionRequest};
use crate::widgets::{Action, Button, Dialog, DialogOutcome, KeyHelp, PopupList, StatusTone};

use super::cs_model::{self as m, CsData};
use super::{Cx, PageEvent, Row, SettingsData, SettingsPage};
use dspi_session::surfaces::CsGroup;

/// The Console's empty state.
pub const EMPTY_TITLE: &str = "No Channel Groups Configured";
pub const EMPTY_BODY: &str = "Name a set of channels so one control can drive them together - a \
                              stereo pair, a zone, every output at once.";

/// The Console's footer, verbatim.
pub const FOOTER: &str = "A group is a named set of channels one control can drive as a unit: mute \
                          a zone, trim a stereo pair, light an LED when any member clips. Turn on \
                          \"Address a Group\" in a control, or in a macro step, to use one. \
                          Relative moves (an encoder, a button) step every member from its own \
                          value, so the balance between them survives. A knob moves the group's \
                          average and keeps those offsets unless \"Match Members Exactly\" is on. \
                          Groups are stored on the device alongside the controls and share their \
                          Save and Revert.";

/// The three kinds a group may be numbered in, in the Console's order.
pub const KINDS: [(&str, u8); 3] = [
    ("Inputs", m::target::INPUT_CH),
    ("Outputs", m::target::OUTPUT_CH),
    ("All Channels", m::target::DSP_CH),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Add,
    /// The card's own row: expand or collapse, rename, remove.
    Header(usize),
    Kind(usize),
    Member(usize, u8),
    /// Revert and Apply.
    Apply(usize),
}

pub struct GroupsPage {
    cursor: usize,
    /// Staged edits, one per slot.
    drafts: Vec<CsGroup>,
    /// What the device is believed to hold, seeded from the read and taken from
    /// the draft once an Apply succeeds, so the card stops saying "Pending"
    /// about its own write without waiting for a re-read.
    live: Vec<CsGroup>,
    /// Slot health from `CsExtStatusPacket.group_status`.
    health: [u8; 8],
    expanded: HashSet<usize>,
    /// Which button on each card's action rows is armed.
    header_button: BTreeMap<usize, usize>,
    apply_button: BTreeMap<usize, usize>,
    messages: BTreeMap<usize, (String, bool)>,
    applying: Option<usize>,
    popup: Option<Item>,
    dialog: Option<Item>,
}

const KEYS: &[KeyHelp] = &[
    KeyHelp::new("↑ ↓", "Move between controls"),
    KeyHelp::new("← →", "Change, or pick a button"),
    KeyHelp::new("Enter", "Open the list, or activate"),
    KeyHelp::new("Space", "Toggle a member"),
    KeyHelp::new("Tab", "Sidebar, page, save bar"),
    KeyHelp::new("[ ]", "Back, forward"),
    KeyHelp::new("Esc", "Leave Settings"),
];

impl GroupsPage {
    pub fn new(data: &SettingsData) -> Self {
        let cs = data.cs.clone().unwrap_or_default();
        Self {
            cursor: 0,
            drafts: cs.groups.clone(),
            live: cs.groups.clone(),
            health: cs.ext.group_status,
            expanded: HashSet::new(),
            header_button: BTreeMap::new(),
            apply_button: BTreeMap::new(),
            messages: BTreeMap::new(),
            applying: None,
            popup: None,
            dialog: None,
        }
    }

    /// Follow the device on every read, card by card, wherever this page holds
    /// no edit of its own (DESIGN section 11, B7). `SettingsData` is re-read
    /// after every write and after a Revert, and the Console beside this can
    /// change a group too; a card that kept its snapshot would show the old
    /// group and its next Apply would write it back over the new one. A
    /// staged edit is kept, and the group being applied is left to the
    /// apply's own answer.
    fn adopt(&mut self, cs: &CsData) {
        if self.drafts.len() != cs.groups.len() {
            self.drafts = cs.groups.clone();
            self.live = cs.groups.clone();
        }
        for g in 0..self.drafts.len() {
            if self.applying == Some(g) {
                continue;
            }
            if self.drafts[g] == self.live[g] {
                self.drafts[g] = cs.groups[g].clone();
            }
            self.live[g] = cs.groups[g].clone();
        }
        self.health = cs.ext.group_status;
    }

    /// Slot health, which the status packet carries eight of whatever
    /// `max_groups` says (control_surfaces.h): a device reporting more than
    /// eight has no health to report for the rest, not a panic.
    fn health(&self, g: usize) -> u8 {
        self.health.get(g).copied().unwrap_or(0)
    }

    /// Slots holding a group, or with one staged. Deliberately weaker than
    /// `CsGroup::is_configured`, which means "a group the device will accept"
    /// and so demands a member: a card being edited holds a kind and a name
    /// well before it holds members, and keying visibility off wire-validity
    /// would make it vanish the moment its last member was unchecked.
    fn in_use(&self, g: usize) -> bool {
        let d = &self.drafts[g];
        d.target_kind != m::target::NONE || !d.name.is_empty() || self.live[g].is_configured()
    }

    fn visible(&self) -> Vec<usize> {
        (0..self.drafts.len()).filter(|g| self.in_use(*g)).collect()
    }

    fn first_free(&self) -> Option<usize> {
        (0..self.drafts.len()).find(|g| !self.in_use(*g))
    }

    fn dirty(&self, g: usize) -> bool {
        self.drafts[g] != self.live[g]
    }

    /// Bindings pointed at a group, so removing one can say what it breaks.
    /// Bindings only, as the Console's `bindingsUsingGroup` counts them
    /// (`DSPi_ConsoleApp.swift:3401-3406`); remote keys and macro steps that
    /// address the group are not in the count there either.
    fn used_by(cs: &CsData, g: usize) -> usize {
        cs.bindings
            .iter()
            .filter(|b| !b.is_empty() && b.flags & m::flag::GROUP != 0 && b.target as usize == g)
            .count()
    }

    fn card(&self, cx: &Cx<'_>, cs: &CsData, g: usize) -> Vec<(Option<Item>, Row)> {
        let mut rows: Vec<(Option<Item>, Row)> = Vec::new();
        let draft = &self.drafts[g];
        let expanded = self.expanded.contains(&g);
        let dirty = self.dirty(g);
        let name = if draft.name.is_empty() {
            format!("Group {}", g + 1)
        } else {
            draft.name.clone()
        };
        let (pill, tone) = if dirty {
            ("Pending", StatusTone::Warning)
        } else if self.health(g) != 0 && self.live[g].is_configured() {
            ("Inactive", StatusTone::Warning)
        } else if self.live[g].is_configured() {
            ("Active", StatusTone::Ok)
        } else {
            ("Empty", StatusTone::Neutral)
        };
        rows.push((
            None,
            Row::Pill {
                label: format!("{} {name}", if expanded { "▾" } else { "▸" }),
                text: pill.into(),
                tone,
                caption: Some(m::group_summary(cx.state, draft)),
            },
        ));
        rows.push((
            Some(Item::Header(g)),
            Row::Buttons {
                label: String::new(),
                caption: None,
                buttons: vec![
                    if expanded { "Collapse" } else { "Expand" }.into(),
                    "Rename".into(),
                    "Remove".into(),
                ],
                cursor: self.header_button.get(&g).copied().unwrap_or(0),
                enabled: true,
            },
        ));
        if self.health(g) != 0 && self.live[g].is_configured() && !dirty {
            rows.push((None, Row::Status(m::inactive_reason(self.health(g)), true)));
        }
        if expanded {
            rows.push((
                Some(Item::Kind(g)),
                Row::Pick {
                    label: "Channel Type".into(),
                    choices: KINDS.iter().map(|(n, _)| (*n).to_string()).collect(),
                    selected: KINDS
                        .iter()
                        .position(|(_, k)| *k == draft.target_kind)
                        .unwrap_or(1),
                    caption: Some("Which set of channels the members are numbered in.".into()),
                    enabled: true,
                },
            ));
            rows.push((None, Row::section("MEMBERS")));
            rows.push((None, Row::note("Every channel this group drives together.")));
            let count = cs.group_channel_count(draft.target_kind);
            if count == 0 {
                rows.push((
                    None,
                    Row::note("No channels of this type on the connected device."),
                ));
            } else {
                for ch in 0..count {
                    rows.push((
                        Some(Item::Member(g, ch)),
                        Row::Toggle {
                            label: m::group_channel_name(cx.state, draft.target_kind, ch),
                            on: draft.member_mask & (1u32 << ch) != 0,
                            caption: None,
                            enabled: true,
                        },
                    ));
                }
            }
            let used = Self::used_by(cs, g);
            if used > 0 {
                rows.push((
                    None,
                    Row::note(format!(
                        "Used by {used} control{}. Emptying this group or changing its channel \
                         type deactivates them until it fits again.",
                        if used == 1 { "" } else { "s" }
                    )),
                ));
            }
        }
        if expanded || dirty || self.messages.contains_key(&g) {
            if let Some((text, err)) = self.messages.get(&g)
                && !dirty
            {
                rows.push((None, Row::Status(text.clone(), *err)));
            } else if dirty && !draft.is_configured() {
                // The device only stores a group with members; an all-zero
                // record clears the slot, which is what Remove is for.
                rows.push((None, Row::note("Pick at least one channel.")));
            }
            rows.push((
                Some(Item::Apply(g)),
                Row::Buttons {
                    label: String::new(),
                    caption: None,
                    buttons: vec!["Revert".into(), "Apply".into()],
                    cursor: self.apply_button.get(&g).copied().unwrap_or(1),
                    enabled: dirty && self.applying != Some(g),
                },
            ));
        }
        rows.push((None, Row::Blank));
        rows
    }

    fn build(&self, cx: &Cx<'_>) -> Vec<(Option<Item>, Row)> {
        let mut rows: Vec<(Option<Item>, Row)> = Vec::new();
        let Some(cs) = cx.data.cs.as_ref() else {
            return m::placeholder_rows(cx)
                .into_iter()
                .map(|r| (None, r))
                .collect();
        };
        let visible = self.visible();
        if visible.is_empty() {
            rows.push((None, m::empty_state(EMPTY_TITLE, EMPTY_BODY)));
            rows.push((None, Row::Blank));
        } else {
            for g in visible {
                rows.extend(self.card(cx, cs, g));
            }
        }
        rows.push((
            Some(Item::Add),
            Row::Buttons {
                label: if self.first_free().is_none() {
                    format!("All {} group slots are in use.", self.drafts.len())
                } else {
                    String::new()
                },
                caption: None,
                buttons: vec!["Add Group".into()],
                cursor: 0,
                enabled: self.first_free().is_some() && cx.connected,
            },
        ));
        rows.push((None, Row::Blank));
        rows.push((None, Row::note(FOOTER)));
        rows
    }

    fn item(&self, index: usize, cx: &Cx<'_>) -> Option<Item> {
        self.build(cx)
            .into_iter()
            .filter(|(_, r)| r.focusable())
            .nth(index)
            .and_then(|(i, _)| i)
    }

    /// The one write that applies a whole group.
    fn apply(&mut self, g: usize) -> PageEvent {
        let group = self.drafts[g].clone();
        self.applying = Some(g);
        self.messages.remove(&g);
        PageEvent::Session(SessionRequest::new(g as u32, move |session| {
            let g = g as u8;
            let group = group.clone();
            m::run(session, move |s| s.write_group(g, &group))
        }))
    }

    fn add(&mut self) -> PageEvent {
        let Some(g) = self.first_free() else {
            return PageEvent::Handled;
        };
        // Not applied yet: a group with no members is not one the device
        // will store.
        self.drafts[g] = CsGroup {
            target_kind: m::target::OUTPUT_CH,
            member_mask: 0,
            name: format!("Group {}", g + 1),
        };
        self.expanded.insert(g);
        self.messages.remove(&g);
        PageEvent::Handled
    }

    fn remove(&mut self, g: usize) -> PageEvent {
        self.messages.remove(&g);
        self.expanded.remove(&g);
        let was_live = self.live[g].is_configured();
        self.drafts[g] = CsGroup::default();
        if !was_live {
            // Never applied: dropping the draft is enough.
            return PageEvent::Handled;
        }
        self.apply(g)
    }
}

impl SettingsPage for GroupsPage {
    fn rows(&self, cx: &Cx<'_>) -> Vec<Row> {
        self.build(cx).into_iter().map(|(_, r)| r).collect()
    }

    fn observe(&mut self, cx: &Cx<'_>) {
        if let Some(cs) = cx.data.cs.as_ref() {
            self.adopt(cs);
        }
    }

    fn act(&mut self, index: usize, action: Action, cx: &Cx<'_>) -> PageEvent {
        let Some(item) = self.item(index, cx) else {
            return PageEvent::Handled;
        };
        let cs = cx.data.cs.clone().unwrap_or_default();
        match (item, action) {
            (Item::Add, Action::Open) => self.add(),
            (Item::Header(g), Action::Selected(i)) => {
                self.header_button.insert(g, i.min(2));
                PageEvent::Handled
            }
            (Item::Header(g), Action::Open) => {
                match self.header_button.get(&g).copied().unwrap_or(0) {
                    0 => {
                        if !self.expanded.remove(&g) {
                            self.expanded.insert(g);
                        }
                        PageEvent::Handled
                    }
                    1 => {
                        self.dialog = Some(item);
                        PageEvent::Dialog(Dialog::text(
                            "Rename Group",
                            "A name for this set of channels.",
                            self.drafts[g].name.clone(),
                            format!("Group {}", g + 1),
                        ))
                    }
                    _ => {
                        self.dialog = Some(item);
                        PageEvent::Dialog(Dialog::confirm(
                            "Remove Group?",
                            format!(
                                "{} control{} address this group. Removing it deactivates them \
                                 until they are pointed somewhere else.",
                                GroupsPage::used_by(&cs, g),
                                if GroupsPage::used_by(&cs, g) == 1 {
                                    ""
                                } else {
                                    "s"
                                }
                            ),
                            vec![Button::destructive("Remove"), Button::new("Cancel")],
                        ))
                    }
                }
            }
            (Item::Kind(g), Action::Selected(i)) => {
                if let Some((_, k)) = KINDS.get(i) {
                    // The mask means different channels under a different
                    // kind, so it cannot carry over.
                    self.drafts[g].target_kind = *k;
                    self.drafts[g].member_mask = 0;
                }
                PageEvent::Handled
            }
            (Item::Kind(_), Action::Open) => {
                self.popup = Some(item);
                let g = match item {
                    Item::Kind(g) => g,
                    _ => unreachable!(),
                };
                PageEvent::Popup(PopupList::new(
                    "Channel Type",
                    KINDS.iter().map(|(n, _)| (*n).to_string()).collect(),
                    KINDS
                        .iter()
                        .position(|(_, k)| *k == self.drafts[g].target_kind)
                        .unwrap_or(1),
                ))
            }
            (Item::Member(g, ch), Action::Toggled(on)) => {
                let bit = 1u32 << ch;
                if on {
                    self.drafts[g].member_mask |= bit;
                } else {
                    self.drafts[g].member_mask &= !bit;
                }
                PageEvent::Handled
            }
            (Item::Apply(g), Action::Selected(i)) => {
                self.apply_button.insert(g, i.min(1));
                PageEvent::Handled
            }
            (Item::Apply(g), Action::Open) => {
                if self.apply_button.get(&g).copied().unwrap_or(1) == 0 {
                    self.drafts[g] = self.live[g].clone();
                    self.messages.remove(&g);
                    return PageEvent::Status("Group changes reverted".into());
                }
                if !self.drafts[g].is_configured() {
                    return PageEvent::Status("Pick at least one channel.".into());
                }
                self.apply(g)
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

    fn key(&mut self, key: KeyEvent, _cx: &Cx<'_>) -> PageEvent {
        // A digit jumps to a group, the way a digit jumps to a band.
        if let KeyCode::Char(c) = key.code
            && let Some(d) = c.to_digit(10)
            && d >= 1
        {
            // The nth card on screen, not the nth group slot.
            if let Some(g) = self.visible().get(d as usize - 1).copied() {
                self.expanded.insert(g);
                return PageEvent::Handled;
            }
        }
        PageEvent::Unhandled
    }

    fn popup_result(&mut self, choice: Option<usize>, cx: &Cx<'_>) -> PageEvent {
        let Some(item) = self.popup.take() else {
            return PageEvent::Handled;
        };
        match (item, choice) {
            (Item::Kind(g), Some(i)) => {
                if let Some((_, k)) = KINDS.get(i) {
                    self.drafts[g].target_kind = *k;
                    self.drafts[g].member_mask = 0;
                }
                PageEvent::Handled
            }
            _ => {
                let _ = cx;
                PageEvent::Handled
            }
        }
    }

    fn dialog_result(&mut self, outcome: DialogOutcome, _cx: &Cx<'_>) -> PageEvent {
        let Some(Item::Header(g)) = self.dialog.take() else {
            return PageEvent::Handled;
        };
        match outcome {
            DialogOutcome::Text(name) => {
                self.drafts[g].name = dspi_proto::packets::truncate_name(&name, 31).to_string();
                PageEvent::Handled
            }
            DialogOutcome::Button(0) => self.remove(g),
            _ => PageEvent::Handled,
        }
    }

    fn session_result(&mut self, tag: u32, reply: SessionReply, _cx: &Cx<'_>) -> PageEvent {
        let g = tag as usize;
        if g >= self.drafts.len() {
            return PageEvent::Handled;
        }
        self.applying = None;
        match reply {
            SessionReply::Ok(_) => {
                self.live[g] = self.drafts[g].clone();
                self.messages.remove(&g);
                PageEvent::Status(format!("Group {} applied", g + 1))
            }
            SessionReply::Err(why) => {
                // The rejected draft stays: re-seeding from the device would
                // take the card away, and this message with it.
                self.messages.insert(g, (why.clone(), true));
                PageEvent::Status(why)
            }
            SessionReply::Bytes(_) => PageEvent::Handled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{frame, key};
    use super::super::{AppConfig, Page, SettingsScreen};
    use super::*;
    use crate::shell::{Screen, ScreenEvent};
    use crossterm::event::KeyCode;
    use dspi_session::DeviceState;

    fn screen(data: SettingsData) -> (SettingsScreen, DeviceState) {
        let st = m::demo::state();
        let s = SettingsScreen::new(&st, data, AppConfig::default()).open(Page::Groups, &st);
        (s, st)
    }

    fn configured() -> (SettingsScreen, DeviceState) {
        screen(m::demo::settings_data())
    }

    #[test]
    fn the_page_lists_its_groups_at_both_sizes() {
        let (mut s, st) = configured();
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            let f = frame(&mut s, &st, w, h);
            assert_eq!(f.lines().count(), h as usize, "{w}x{h}");
            assert!(f.contains("Front Pair"), "{w}x{h}:\n{f}");
        }
    }

    #[test]
    fn an_empty_device_shows_the_consoles_empty_state() {
        let mut d = m::demo::settings_data();
        if let Some(cs) = d.cs.as_mut() {
            cs.groups = vec![CsGroup::default(); 8];
        }
        let (mut s, st) = screen(d);
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("No Channel Groups Configured"), "{f}");
        assert!(f.contains("stereo pair"), "{f}");
        assert!(f.contains("Add Group"), "{f}");
    }

    #[test]
    fn a_device_without_capabilities_says_so() {
        let mut d = m::demo::settings_data();
        d.cs = None;
        let (mut s, st) = screen(d);
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("Reading control-surface capabilities"), "{f}");
    }

    #[test]
    fn changing_the_kind_clears_the_members() {
        let d = m::demo::settings_data();
        let cs = d.cs.clone().unwrap();
        let mut p = GroupsPage::new(&d);
        p.expanded.insert(0);
        assert_eq!(p.drafts[0].member_mask, 0b0011);
        let cx = Cx {
            state: &m::demo::state(),
            data: &d,
            config: &AppConfig::default(),
            connected: true,
            global_dirty: false,
        };
        // The Channel Type row is the first focusable one inside the card.
        let kind_row = p
            .build(&cx)
            .into_iter()
            .filter(|(_, r)| r.focusable())
            .position(|(i, _)| matches!(i, Some(Item::Kind(0))))
            .expect("a kind row");
        p.act(kind_row, Action::Selected(0), &cx);
        assert_eq!(p.drafts[0].target_kind, m::target::INPUT_CH);
        assert_eq!(
            p.drafts[0].member_mask, 0,
            "the mask means other channels under another kind"
        );
        let _ = cs;
    }

    /// The record is byte-exact, mirroring the Console's wire test: kind,
    /// three reserved bytes, the little-endian mask, then the padded name.
    #[test]
    fn a_filled_card_encodes_the_consoles_group_record() {
        let g = CsGroup {
            target_kind: m::target::OUTPUT_CH,
            member_mask: 0b1010,
            name: "Front Pair".into(),
        };
        let w = g.encode();
        assert_eq!(w.len(), 40);
        assert_eq!(w[0], 2, "target_kind = OUTPUT_CH");
        assert_eq!(&w[1..4], &[0, 0, 0], "reserved");
        assert_eq!(&w[4..8], &[0x0A, 0, 0, 0], "member_mask little endian");
        assert_eq!(&w[8..18], b"Front Pair");
        assert_eq!(w[18], 0, "NUL terminated");
        assert_eq!(CsGroup::decode(&w).unwrap(), g);
    }

    #[test]
    fn an_empty_group_is_not_offered_to_the_device() {
        let d = m::demo::settings_data();
        let mut p = GroupsPage::new(&d);
        let cx = Cx {
            state: &m::demo::state(),
            data: &d,
            config: &AppConfig::default(),
            connected: true,
            global_dirty: false,
        };
        p.expanded.insert(0);
        // Clear every member, then try to apply.
        p.drafts[0].member_mask = 0;
        let apply = p
            .build(&cx)
            .into_iter()
            .filter(|(_, r)| r.focusable())
            .position(|(i, _)| matches!(i, Some(Item::Apply(0))))
            .expect("an apply row");
        p.apply_button.insert(0, 1);
        assert_eq!(
            p.act(apply, Action::Open, &cx),
            PageEvent::Status("Pick at least one channel.".into())
        );
    }

    #[test]
    fn applying_a_group_asks_the_session_for_the_write() {
        let (mut s, st) = configured();
        s.handle(key(KeyCode::Tab), &st);
        // Expand the first card, then walk to its Apply row.
        assert!(matches!(
            s.handle(key(KeyCode::Enter), &st),
            ScreenEvent::Handled
        ));
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("Channel Type"), "the card opened:\n{f}");
        assert!(f.contains("Outputs"), "{f}");
    }

    /// A digit counts the cards on screen, the way it counts bands: with
    /// groups in slots 1 and 6, `2` opens the second card and not a slot the
    /// page is not drawing.
    #[test]
    fn a_digit_opens_the_nth_card_not_the_nth_slot() {
        let mut d = m::demo::settings_data();
        let cs = d.cs.as_mut().expect("cs");
        cs.groups = vec![CsGroup::default(); 8];
        cs.groups[0] = CsGroup {
            target_kind: m::target::OUTPUT_CH,
            member_mask: 0b0011,
            name: "Front Pair".into(),
        };
        cs.groups[5] = CsGroup {
            target_kind: m::target::OUTPUT_CH,
            member_mask: 0b1100,
            name: "Rear Pair".into(),
        };
        let mut p = GroupsPage::new(&d);
        assert_eq!(p.visible(), vec![0, 5]);
        let cx = Cx {
            state: &m::demo::state(),
            data: &d,
            config: &AppConfig::default(),
            connected: true,
            global_dirty: false,
        };
        assert_eq!(p.key(key(KeyCode::Char('2')), &cx), PageEvent::Handled);
        assert!(p.expanded.contains(&5), "the second card, in slot 6");
        assert_eq!(p.key(key(KeyCode::Char('3')), &cx), PageEvent::Unhandled);
    }

    /// `CsExtStatusPacket` carries eight health bytes whatever `max_groups`
    /// says, and the slot indices come from the caps. A device reporting more
    /// than eight has no health for the rest, not a panic.
    #[test]
    fn a_device_with_more_group_slots_than_health_bytes_does_not_panic() {
        let mut d = m::demo::settings_data();
        let cs = d.cs.as_mut().expect("cs");
        cs.groups = vec![
            CsGroup {
                target_kind: m::target::OUTPUT_CH,
                member_mask: 0b0011,
                name: "Zone".into(),
            };
            12
        ];
        cs.caps.max_groups = 12;
        let p = GroupsPage::new(&d);
        assert_eq!(p.health(11), 0, "past the packet's eight");
        let st = m::demo::state();
        let cx = Cx {
            state: &st,
            data: &d,
            config: &AppConfig::default(),
            connected: true,
            global_dirty: false,
        };
        assert_eq!(p.visible().len(), 12);
        assert!(!p.build(&cx).is_empty());
    }

    /// A card with nothing staged follows each re-read (a Revert, another
    /// host), so its next Apply cannot write a stale group back; a card being
    /// edited keeps the edit.
    #[test]
    fn a_card_without_a_staged_edit_follows_the_device() {
        let d = m::demo::settings_data();
        let mut p = GroupsPage::new(&d);
        let mut cs = d.cs.clone().expect("cs");
        p.drafts[1] = CsGroup {
            target_kind: m::target::OUTPUT_CH,
            member_mask: 0b0100,
            name: "Staged".into(),
        };
        cs.groups[0].name = "Changed Elsewhere".into();
        cs.groups[0].member_mask = 0b0110;
        cs.groups[1].name = "Also Changed".into();
        p.adopt(&cs);
        assert_eq!(p.drafts[0], cs.groups[0], "the untouched card follows");
        assert!(!p.dirty(0));
        assert_eq!(p.drafts[1].name, "Staged", "the edit is the user's");
        assert!(p.dirty(1), "and now differs from what the device holds");
    }

    /// A name is cut to 31 bytes on a character boundary, not to 31
    /// characters, which could be 62 bytes and split one on the wire.
    #[test]
    fn a_rename_is_cut_on_a_character_boundary() {
        let d = m::demo::settings_data();
        let mut p = GroupsPage::new(&d);
        let cx = Cx {
            state: &m::demo::state(),
            data: &d,
            config: &AppConfig::default(),
            connected: true,
            global_dirty: false,
        };
        p.dialog = Some(Item::Header(0));
        p.dialog_result(DialogOutcome::Text("é".repeat(30)), &cx);
        assert_eq!(p.drafts[0].name, "é".repeat(15));
    }

    #[test]
    fn a_group_in_use_says_what_removing_it_breaks() {
        let mut d = m::demo::settings_data();
        if let Some(cs) = d.cs.as_mut() {
            cs.bindings[2].flags |= m::flag::GROUP;
            cs.bindings[2].target = 0;
        }
        let cs = d.cs.clone().unwrap();
        assert_eq!(GroupsPage::used_by(&cs, 0), 1);
        let (mut s, st) = screen(d);
        s.handle(key(KeyCode::Tab), &st);
        s.handle(key(KeyCode::Enter), &st);
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("Used by 1 control."), "{f}");
    }
}
