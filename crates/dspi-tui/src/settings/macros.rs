//! Settings > Macros.
//!
//! A macro runs a short sequence of changes from a single press: select an
//! input and load a preset, switch monitors, mute after a delay. Bind a button,
//! switch or remote key to the `Macro` parameter to fire one.
//!
//! Each step is a stripped binding - parameter, action, target, operands - plus
//! a delay that elapses before it runs, so the editor reuses the same
//! caps-driven pickers the binding and IR cards use. Two things are specific to
//! this page. The steps are written before the header, because the header is
//! what makes them live and a macro fired mid-edit must never see a step count
//! reaching past the steps actually written (control_surfaces.h:488-491). And
//! the step delay is in 10 ms units, ten times finer than the indicator delays
//! on a binding: one second is 100 here and 10 there (firmware-notes 24).

use std::collections::{BTreeMap, HashSet};

use crossterm::event::{KeyCode, KeyEvent};

use crate::shell::{SessionReply, SessionRequest};
use crate::widgets::{Action, Button, Dialog, DialogOutcome, KeyHelp, PopupList, StatusTone};

use super::cs_model::{self as m, CsData, target_choice, target_choices};
use super::{Cx, PageEvent, Row, SettingsData, SettingsPage};
use dspi_session::surfaces::{CsMacro, CsMacroHeaderWire, CsMacroStep};

pub const EMPTY_TITLE: &str = "No Macros Configured";
pub const EMPTY_BODY: &str = "Run a short sequence of changes from a single press: select an input \
                              and load a preset, switch monitors, mute after a delay.";

/// The Console's footer, verbatim.
pub const FOOTER: &str = "A macro runs a short sequence of changes from a single press: select an \
                          input and load a preset, switch monitors, mute after a delay. Bind a \
                          button, switch, or remote key to \"Macro\" to fire one. Each step can \
                          wait before it runs, and can address a channel group. One macro runs at \
                          a time - firing another cancels the first at its current step. Macros \
                          are stored on the device alongside the controls and share their Save and \
                          Revert.";

/// Tags for the two kinds of session work this page starts.
const TAG_FIRE: u32 = 0x100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Add,
    Header(usize),
    /// Move up, move down, delete.
    StepHeader(usize, usize),
    StepNoun(usize, usize),
    StepAction(usize, usize),
    StepTarget(usize, usize),
    StepBand(usize, usize),
    /// `SET` on a continuous noun.
    StepValue(usize, usize),
    /// `SET` on a boolean or an enum.
    StepChoice(usize, usize),
    /// `INC` / `DEC` on a continuous noun.
    StepStep(usize, usize),
    StepWrap(usize, usize),
    StepDelay(usize, usize),
    AddStep(usize),
    Apply(usize),
}

pub struct MacrosPage {
    cursor: usize,
    drafts: Vec<CsMacro>,
    live: Vec<CsMacro>,
    health: [u8; 8],
    running: Option<(u8, u8)>,
    max_steps: usize,
    expanded: HashSet<usize>,
    header_button: BTreeMap<usize, usize>,
    step_button: BTreeMap<(usize, usize), usize>,
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
    KeyHelp::new("Space", "Toggle"),
    KeyHelp::new("Tab", "Sidebar, page, save bar"),
    KeyHelp::new("[ ]", "Back, forward"),
    KeyHelp::new("Esc", "Leave Settings"),
];

/// A macro read back may carry fewer step records than the device has slots;
/// the editor works on the full array so a removed step clears its slot.
fn normalise(m: &CsMacro, max_steps: usize) -> CsMacro {
    let mut m = m.clone();
    m.steps.resize(max_steps, CsMacroStep::default());
    m.step_count = m.step_count.min(max_steps as u8);
    m
}

impl MacrosPage {
    pub fn new(data: &SettingsData) -> Self {
        let cs = data.cs.clone().unwrap_or_default();
        let max_steps = cs.caps.max_macro_steps.max(1) as usize;
        let drafts: Vec<CsMacro> = cs.macros.iter().map(|x| normalise(x, max_steps)).collect();
        Self {
            cursor: 0,
            live: drafts.clone(),
            drafts,
            health: cs.ext.macro_status,
            running: None,
            max_steps,
            expanded: HashSet::new(),
            header_button: BTreeMap::new(),
            step_button: BTreeMap::new(),
            apply_button: BTreeMap::new(),
            messages: BTreeMap::new(),
            applying: None,
            popup: None,
            dialog: None,
        }
    }

    fn adopt(&mut self, cs: &CsData) {
        if self.drafts.len() != cs.macros.len() {
            self.max_steps = cs.caps.max_macro_steps.max(1) as usize;
            self.drafts = cs
                .macros
                .iter()
                .map(|x| normalise(x, self.max_steps))
                .collect();
            self.live = self.drafts.clone();
        }
        self.health = cs.ext.macro_status;
        // Nothing pushes sequencer progress to the host, so the running badge
        // is only as fresh as the last status read.
        self.running = cs
            .ext
            .is_running()
            .then_some((cs.ext.macro_running, cs.ext.macro_step));
    }

    fn in_use(&self, i: usize) -> bool {
        !self.drafts[i].name.is_empty()
            || self.drafts[i].step_count > 0
            || !self.live[i].name.is_empty()
            || self.live[i].step_count > 0
    }

    fn visible(&self) -> Vec<usize> {
        (0..self.drafts.len()).filter(|i| self.in_use(*i)).collect()
    }

    fn first_free(&self) -> Option<usize> {
        (0..self.drafts.len()).find(|i| !self.in_use(*i))
    }

    fn dirty(&self, i: usize) -> bool {
        self.drafts[i] != self.live[i]
    }

    fn is_running(&self, i: usize) -> bool {
        self.running.map(|(m, _)| m as usize) == Some(i)
    }

    // ------------------------------------------------------------- the rows

    fn step_rows(&self, cx: &Cx<'_>, cs: &CsData, i: usize, s: usize) -> Vec<(Option<Item>, Row)> {
        let mut rows: Vec<(Option<Item>, Row)> = Vec::new();
        let step = self.drafts[i].steps[s].clone();
        rows.push((
            Some(Item::StepHeader(i, s)),
            Row::Buttons {
                label: format!("{}  {}", s + 1, m::macro_step_summary(cx.state, cs, &step)),
                caption: None,
                buttons: vec!["Up".into(), "Down".into(), "Delete".into()],
                cursor: self.step_button.get(&(i, s)).copied().unwrap_or(0),
                enabled: true,
            },
        ));

        let nouns = m::macro_step_nouns(cs);
        rows.push((
            Some(Item::StepNoun(i, s)),
            Row::Pick {
                label: "Change".into(),
                choices: nouns
                    .iter()
                    .map(|n| m::noun_name(*n, m::ty::BUTTON))
                    .collect(),
                selected: nouns.iter().position(|n| *n == step.noun).unwrap_or(0),
                caption: Some("Which function this step changes.".into()),
                enabled: true,
            },
        ));

        let acts = m::macro_step_actions(cs, step.noun);
        let is_enum = m::operand_kind(cs, step.noun) == m::kind::ENUM;
        if acts.len() > 1 {
            rows.push((
                Some(Item::StepAction(i, s)),
                Row::Pick {
                    label: "How".into(),
                    choices: acts
                        .iter()
                        .map(|a| m::action_name(*a, step.noun, is_enum))
                        .collect(),
                    selected: acts.iter().position(|a| *a == step.action).unwrap_or(0),
                    caption: Some("What this step does to it.".into()),
                    enabled: true,
                },
            ));
        }

        // Target, as one merged Channels / Groups picker: `target` meaning a
        // channel or a group index depending on the flag is a wire detail.
        if let Some(nd) = cs.noun_desc(step.noun)
            && nd.is_targeted()
        {
            let grouped = step.flags & m::flag::GROUP != 0;
            let usable = if step.action == m::act::TRIGGER {
                Vec::new()
            } else {
                m::compatible_groups(cs, step.noun)
            };
            let (choices, selected) = target_choices(cx, cs, nd, &usable, step.target, grouped);
            rows.push((
                Some(Item::StepTarget(i, s)),
                Row::Pick {
                    label: if usable.is_empty() && !grouped {
                        "Channel".into()
                    } else {
                        "Channel or Group".into()
                    },
                    choices,
                    selected,
                    caption: Some(
                        "Which channel, or named set of channels, this step affects.".into(),
                    ),
                    enabled: true,
                },
            ));
            if nd.has_band() {
                let bands = m::band_options(cs, cx.state, step.noun, step.target, grouped);
                rows.push((
                    Some(Item::StepBand(i, s)),
                    Row::Pick {
                        label: "Band".into(),
                        choices: bands.iter().map(|b| m::band_name(*b)).collect(),
                        selected: bands.iter().position(|b| *b == step.index).unwrap_or(0),
                        caption: Some("Which filter band this step affects.".into()),
                        enabled: true,
                    },
                ));
            }
        }

        // Operands. TOGGLE and TRIGGER carry none.
        let k = m::operand_kind(cs, step.noun);
        let u = m::noun_unit(cs, step.noun);
        let (lo, hi) = m::noun_range(cs, step.noun);
        match step.action {
            m::act::SET => match k {
                m::kind::CONTINUOUS => rows.push((
                    Some(Item::StepValue(i, s)),
                    Row::Number {
                        label: "Set To".into(),
                        value: m::decode_value(step.value, u),
                        min: lo,
                        max: hi,
                        step: m::unit_scroll_step(u),
                        unit: m::unit_symbol(u).into(),
                        decimals: m::unit_decimals(u),
                        caption: Some(format!(
                            "The value this step applies ({} to {}).",
                            m::fmt_unit(lo, u),
                            m::fmt_unit(hi, u)
                        )),
                        enabled: true,
                    },
                )),
                m::kind::BOOL => rows.push((
                    Some(Item::StepChoice(i, s)),
                    Row::Pick {
                        label: "Set To".into(),
                        choices: vec![
                            m::bool_label(step.noun, false).into(),
                            m::bool_label(step.noun, true).into(),
                        ],
                        selected: usize::from(step.value != 0),
                        caption: Some("The state this step applies.".into()),
                        enabled: true,
                    },
                )),
                _ => {
                    let count = cs
                        .noun_desc(step.noun)
                        .map(|d| d.enum_count.max(1))
                        .unwrap_or(1);
                    rows.push((
                        Some(Item::StepChoice(i, s)),
                        Row::Pick {
                            label: "Set To".into(),
                            choices: (0..count)
                                .map(|v| m::enum_value_label(cx, cs, step.noun, v as i32))
                                .collect(),
                            selected: (step.value.max(0) as usize).min(count as usize - 1),
                            caption: Some("The selection this step applies.".into()),
                            enabled: true,
                        },
                    ));
                }
            },
            m::act::INC | m::act::DEC => {
                if k == m::kind::CONTINUOUS {
                    let is_log = m::unit_is_log(u);
                    let cur = if step.step == 0 {
                        m::default_step(u)
                    } else {
                        m::decode_step(step.step, u)
                    };
                    rows.push((
                        Some(Item::StepStep(i, s)),
                        Row::Number {
                            label: "Step Size".into(),
                            value: cur,
                            min: m::unit_min_step(u),
                            max: (hi - lo).abs().max(1.0),
                            step: if is_log {
                                m::default_step(u)
                            } else {
                                m::unit_scroll_step(u)
                            },
                            unit: if is_log {
                                "oct".into()
                            } else {
                                m::unit_symbol(u).into()
                            },
                            decimals: if is_log { 3 } else { m::unit_decimals(u) },
                            caption: Some(
                                if is_log {
                                    "How far each run moves it, in octaves."
                                } else {
                                    "How far each run moves it."
                                }
                                .into(),
                            ),
                            enabled: true,
                        },
                    ));
                } else if k == m::kind::ENUM {
                    rows.push((
                        Some(Item::StepWrap(i, s)),
                        Row::Toggle {
                            label: "Wrap Around".into(),
                            on: step.flags & m::flag::WRAP != 0,
                            caption: Some("Step past the last position back to the first.".into()),
                            enabled: true,
                        },
                    ));
                }
            }
            _ => {}
        }

        rows.push((
            Some(Item::StepDelay(i, s)),
            Row::Number {
                label: "Wait Before".into(),
                value: m::decode_step_delay(step.pre_delay),
                min: 0.0,
                max: m::STEP_DELAY_MAX_SECONDS,
                step: 0.1,
                unit: "s".into(),
                decimals: 2,
                caption: Some("Delay after the previous step before this one runs.".into()),
                enabled: true,
            },
        ));
        rows.push((None, Row::Blank));
        rows
    }

    fn card(&self, cx: &Cx<'_>, cs: &CsData, i: usize) -> Vec<(Option<Item>, Row)> {
        let mut rows: Vec<(Option<Item>, Row)> = Vec::new();
        let draft = &self.drafts[i];
        let expanded = self.expanded.contains(&i);
        let dirty = self.dirty(i);
        let running = self.is_running(i);
        let name = if draft.name.is_empty() {
            format!("Macro {}", i + 1)
        } else {
            draft.name.clone()
        };
        let (pill, tone) = if dirty {
            ("Pending", StatusTone::Warning)
        } else if running {
            ("Running", StatusTone::Ok)
        } else if self.health[i] != 0 && self.live[i].step_count > 0 {
            ("Inactive", StatusTone::Warning)
        } else if self.live[i].step_count > 0 {
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
                caption: Some(m::macro_summary(
                    draft,
                    running.then(|| self.running.map(|(_, s)| s).unwrap_or(0)),
                )),
            },
        ));
        rows.push((
            Some(Item::Header(i)),
            Row::Buttons {
                label: String::new(),
                caption: None,
                buttons: vec![
                    if expanded { "Collapse" } else { "Expand" }.into(),
                    "Rename".into(),
                    if running { "Stop" } else { "Run" }.into(),
                    "Remove".into(),
                ],
                cursor: self.header_button.get(&i).copied().unwrap_or(0),
                enabled: true,
            },
        ));
        if self.health[i] != 0 && self.live[i].step_count > 0 && !dirty {
            rows.push((None, Row::Status(m::inactive_reason(self.health[i]), true)));
        }
        if expanded {
            for s in 0..draft.step_count as usize {
                rows.extend(self.step_rows(cx, cs, i, s));
            }
            let full = draft.step_count as usize >= self.max_steps;
            rows.push((
                Some(Item::AddStep(i)),
                Row::Buttons {
                    label: if full {
                        format!("A macro holds up to {} steps.", self.max_steps)
                    } else {
                        String::new()
                    },
                    caption: None,
                    buttons: vec!["Add Step".into()],
                    cursor: 0,
                    enabled: !full,
                },
            ));
        }
        if expanded || dirty || self.messages.contains_key(&i) {
            if let Some((text, err)) = self.messages.get(&i)
                && !dirty
            {
                rows.push((None, Row::Status(text.clone(), *err)));
            }
            rows.push((
                Some(Item::Apply(i)),
                Row::Buttons {
                    label: String::new(),
                    caption: None,
                    buttons: vec!["Revert".into(), "Apply".into()],
                    cursor: self.apply_button.get(&i).copied().unwrap_or(1),
                    enabled: dirty && self.applying != Some(i),
                },
            ));
        }
        rows.push((None, Row::Blank));
        rows
    }

    fn build(&self, cx: &Cx<'_>) -> Vec<(Option<Item>, Row)> {
        let Some(cs) = cx.data.cs.as_ref() else {
            return m::placeholder_rows(cx)
                .into_iter()
                .map(|r| (None, r))
                .collect();
        };
        let mut rows: Vec<(Option<Item>, Row)> = Vec::new();
        let visible = self.visible();
        if visible.is_empty() {
            rows.push((None, m::empty_state(EMPTY_TITLE, EMPTY_BODY)));
            rows.push((None, Row::Blank));
        } else {
            for i in visible {
                rows.extend(self.card(cx, cs, i));
            }
        }
        rows.push((
            Some(Item::Add),
            Row::Buttons {
                label: if self.first_free().is_none() {
                    format!("All {} macro slots are in use.", self.drafts.len())
                } else {
                    String::new()
                },
                caption: None,
                buttons: vec!["Add Macro".into()],
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

    // ------------------------------------------------------------ the edits

    fn add(&mut self) -> PageEvent {
        let Some(i) = self.first_free() else {
            return PageEvent::Handled;
        };
        self.drafts[i] = normalise(
            &CsMacro {
                name: format!("Macro {}", i + 1),
                step_count: 0,
                steps: Vec::new(),
            },
            self.max_steps,
        );
        self.expanded.insert(i);
        self.messages.remove(&i);
        PageEvent::Handled
    }

    fn remove(&mut self, i: usize) -> PageEvent {
        self.messages.remove(&i);
        self.expanded.remove(&i);
        let was_live = self.live[i].step_count > 0 || !self.live[i].name.is_empty();
        self.drafts[i] = normalise(&CsMacro::default(), self.max_steps);
        if !was_live {
            return PageEvent::Handled;
        }
        self.apply(i)
    }

    /// Remove a step and close the gap: the sequencer runs `steps[0..count]`,
    /// so a hole in the middle would execute as a skipped empty record rather
    /// than shortening the macro.
    fn remove_step(&mut self, i: usize, s: usize) {
        let count = self.drafts[i].step_count as usize;
        if s >= count {
            return;
        }
        self.drafts[i].steps.remove(s);
        self.drafts[i].steps.push(CsMacroStep::default());
        self.drafts[i].step_count = (count - 1) as u8;
    }

    fn move_step(&mut self, i: usize, s: usize, delta: isize) {
        let dest = s as isize + delta;
        if dest < 0 || dest >= self.drafts[i].step_count as isize {
            return;
        }
        self.drafts[i].steps.swap(s, dest as usize);
    }

    fn apply(&mut self, i: usize) -> PageEvent {
        let m0 = self.drafts[i].clone();
        self.applying = Some(i);
        self.messages.remove(&i);
        PageEvent::Session(SessionRequest::new(i as u32, move |session| {
            let index = i as u8;
            let header = CsMacroHeaderWire {
                name: m0.name.clone(),
                step_count: m0.step_count,
            };
            let steps = m0.steps.clone();
            m::run(session, move |s| s.write_macro(index, &header, &steps))
        }))
    }

    fn fire(&mut self, i: usize) -> PageEvent {
        let running = self.is_running(i);
        PageEvent::Session(SessionRequest::new(TAG_FIRE | i as u32, move |session| {
            m::run_code(
                session,
                if running {
                    "Macro cancelled"
                } else {
                    "Macro fired"
                },
                move |s| {
                    if running {
                        s.cancel_macro()
                    } else {
                        s.fire_macro(i as u8)
                    }
                },
            )
        }))
    }
}

impl SettingsPage for MacrosPage {
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

            (Item::Header(i), Action::Selected(b)) => {
                self.header_button.insert(i, b.min(3));
                PageEvent::Handled
            }
            (Item::Header(i), Action::Open) => {
                match self.header_button.get(&i).copied().unwrap_or(0) {
                    0 => {
                        if !self.expanded.remove(&i) {
                            self.expanded.insert(i);
                        }
                        PageEvent::Handled
                    }
                    1 => {
                        self.dialog = Some(item);
                        PageEvent::Dialog(Dialog::text(
                            "Rename Macro",
                            "A name for this sequence.",
                            self.drafts[i].name.clone(),
                            format!("Macro {}", i + 1),
                        ))
                    }
                    2 => {
                        // Firing tests the sequence as stored on the device, so
                        // it is offered only once the draft has been applied.
                        if !self.is_running(i) && self.live[i].step_count == 0 {
                            return PageEvent::Status("Apply this macro before running it".into());
                        }
                        self.fire(i)
                    }
                    _ => {
                        self.dialog = Some(item);
                        PageEvent::Dialog(Dialog::confirm(
                            "Remove Macro?",
                            "The macro is cleared on the device. Anything bound to it stops \
                             working until it is pointed somewhere else.",
                            vec![Button::destructive("Remove"), Button::new("Cancel")],
                        ))
                    }
                }
            }

            (Item::StepHeader(i, s), Action::Selected(b)) => {
                self.step_button.insert((i, s), b.min(2));
                PageEvent::Handled
            }
            (Item::StepHeader(i, s), Action::Open) => {
                match self.step_button.get(&(i, s)).copied().unwrap_or(0) {
                    0 => self.move_step(i, s, -1),
                    1 => self.move_step(i, s, 1),
                    _ => self.remove_step(i, s),
                }
                PageEvent::Handled
            }

            (Item::AddStep(i), Action::Open) => {
                let n = self.drafts[i].step_count as usize;
                if n < self.max_steps {
                    self.drafts[i].steps[n] = m::default_macro_step(&cs);
                    self.drafts[i].step_count = (n + 1) as u8;
                }
                PageEvent::Handled
            }

            (Item::StepNoun(i, s), Action::Selected(c)) => {
                let nouns = m::macro_step_nouns(&cs);
                if let Some(n) = nouns.get(c) {
                    let mut st = self.drafts[i].steps[s].clone();
                    if st.noun != *n {
                        st.noun = *n;
                        st.target = 0;
                        st.index = 0;
                        let acts = m::macro_step_actions(&cs, *n);
                        if !acts.contains(&st.action) {
                            st.action = acts.first().copied().unwrap_or(m::act::SET);
                        }
                        self.drafts[i].steps[s] = m::default_step_operands(&cs, &st);
                    }
                }
                PageEvent::Handled
            }
            (Item::StepAction(i, s), Action::Selected(c)) => {
                let acts = m::macro_step_actions(&cs, self.drafts[i].steps[s].noun);
                if let Some(a) = acts.get(c) {
                    let mut st = self.drafts[i].steps[s].clone();
                    if st.action != *a {
                        st.action = *a;
                        self.drafts[i].steps[s] = m::default_step_operands(&cs, &st);
                    }
                }
                PageEvent::Handled
            }
            (Item::StepTarget(i, s), Action::Selected(c)) => {
                let st = self.drafts[i].steps[s].clone();
                let Some(nd) = cs.noun_desc(st.noun).copied() else {
                    return PageEvent::Handled;
                };
                let grouped = st.flags & m::flag::GROUP != 0;
                let usable = if st.action == m::act::TRIGGER {
                    Vec::new()
                } else {
                    m::compatible_groups(&cs, st.noun)
                };
                let (is_group, t) = target_choice(&nd, &usable, st.target, grouped, c);
                let mut st = st;
                if is_group {
                    st.flags |= m::flag::GROUP;
                } else {
                    st.flags &= !m::flag::GROUP;
                }
                st.target = t;
                if nd.has_band() {
                    let opts = m::band_options(&cs, cx.state, st.noun, st.target, is_group);
                    if !opts.contains(&st.index) {
                        st.index = opts.first().copied().unwrap_or(0);
                    }
                }
                self.drafts[i].steps[s] = st;
                PageEvent::Handled
            }
            (Item::StepBand(i, s), Action::Selected(c)) => {
                let st = &self.drafts[i].steps[s];
                let bands = m::band_options(
                    &cs,
                    cx.state,
                    st.noun,
                    st.target,
                    st.flags & m::flag::GROUP != 0,
                );
                if let Some(b) = bands.get(c) {
                    self.drafts[i].steps[s].index = *b;
                }
                PageEvent::Handled
            }
            (Item::StepChoice(i, s), Action::Selected(c)) => {
                let st = &self.drafts[i].steps[s];
                let k = m::operand_kind(&cs, st.noun);
                self.drafts[i].steps[s].value = if k == m::kind::BOOL {
                    i16::from(c != 0)
                } else {
                    c as i16
                };
                PageEvent::Handled
            }
            (Item::StepValue(i, s), Action::Changed(v) | Action::Committed(v)) => {
                let u = m::noun_unit(&cs, self.drafts[i].steps[s].noun);
                let (lo, hi) = m::noun_range(&cs, self.drafts[i].steps[s].noun);
                self.drafts[i].steps[s].value = m::encode_value(v.clamp(lo, hi), u);
                PageEvent::Handled
            }
            (Item::StepStep(i, s), Action::Changed(v) | Action::Committed(v)) => {
                let u = m::noun_unit(&cs, self.drafts[i].steps[s].noun);
                self.drafts[i].steps[s].step = m::encode_step(v.max(m::unit_min_step(u)), u);
                PageEvent::Handled
            }
            (Item::StepDelay(i, s), Action::Changed(v) | Action::Committed(v)) => {
                self.drafts[i].steps[s].pre_delay = m::encode_step_delay(v);
                PageEvent::Handled
            }
            (Item::StepWrap(i, s), Action::Toggled(on)) => {
                if on {
                    self.drafts[i].steps[s].flags |= m::flag::WRAP;
                } else {
                    self.drafts[i].steps[s].flags &= !m::flag::WRAP;
                }
                PageEvent::Handled
            }
            (
                Item::StepValue(i, s) | Item::StepStep(i, s) | Item::StepDelay(i, s),
                Action::Reset,
            ) => {
                let st = m::default_step_operands(&cs, &self.drafts[i].steps[s]);
                self.drafts[i].steps[s] = CsMacroStep { pre_delay: 0, ..st };
                PageEvent::Handled
            }

            (Item::Apply(i), Action::Selected(b)) => {
                self.apply_button.insert(i, b.min(1));
                PageEvent::Handled
            }
            (Item::Apply(i), Action::Open) => {
                if self.apply_button.get(&i).copied().unwrap_or(1) == 0 {
                    self.drafts[i] = self.live[i].clone();
                    self.messages.remove(&i);
                    return PageEvent::Status("Macro changes reverted".into());
                }
                self.apply(i)
            }

            // Every picker opens the same way: the popup carries the row's own
            // choices, so it never disagrees with what the row draws.
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
                    self.popup = Some(item);
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

    fn key(&mut self, key: KeyEvent, _cx: &Cx<'_>) -> PageEvent {
        if let KeyCode::Char(c) = key.code
            && let Some(d) = c.to_digit(10)
            && d >= 1
        {
            let i = d as usize - 1;
            if i < self.drafts.len() && self.in_use(i) {
                self.expanded.insert(i);
                return PageEvent::Handled;
            }
        }
        PageEvent::Unhandled
    }

    fn popup_result(&mut self, choice: Option<usize>, cx: &Cx<'_>) -> PageEvent {
        let Some(item) = self.popup.take() else {
            return PageEvent::Handled;
        };
        let Some(c) = choice else {
            return PageEvent::Handled;
        };
        // The popup answers what the row would have: one path for both.
        let index = self
            .build(cx)
            .into_iter()
            .filter(|(_, r)| r.focusable())
            .position(|(i, _)| i == Some(item))
            .unwrap_or(0);
        self.act(index, Action::Selected(c), cx)
    }

    fn dialog_result(&mut self, outcome: DialogOutcome, _cx: &Cx<'_>) -> PageEvent {
        let Some(Item::Header(i)) = self.dialog.take() else {
            return PageEvent::Handled;
        };
        match outcome {
            DialogOutcome::Text(name) => {
                self.drafts[i].name = name.chars().take(31).collect();
                PageEvent::Handled
            }
            DialogOutcome::Button(0) => self.remove(i),
            _ => PageEvent::Handled,
        }
    }

    fn session_result(&mut self, tag: u32, reply: SessionReply, _cx: &Cx<'_>) -> PageEvent {
        if tag & TAG_FIRE != 0 {
            return match reply {
                SessionReply::Ok(msg) => PageEvent::Status(msg),
                SessionReply::Err(why) => PageEvent::Status(why),
                SessionReply::Bytes(_) => PageEvent::Handled,
            };
        }
        let i = tag as usize;
        if i >= self.drafts.len() {
            return PageEvent::Handled;
        }
        self.applying = None;
        match reply {
            SessionReply::Ok(_) => {
                self.live[i] = self.drafts[i].clone();
                self.messages.remove(&i);
                PageEvent::Status(format!("Macro {} applied", i + 1))
            }
            SessionReply::Err(why) => {
                self.messages.insert(i, (why.clone(), true));
                PageEvent::Status(why)
            }
            SessionReply::Bytes(_) => PageEvent::Handled,
        }
    }
}

/// The macro header as it goes on the wire, exposed for the encode test.
pub fn header_of(m: &CsMacro) -> CsMacroHeaderWire {
    CsMacroHeaderWire {
        name: m.name.clone(),
        step_count: m.step_count,
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{frame, key};
    use super::super::{AppConfig, Page, SettingsScreen};
    use super::*;
    use crate::shell::Screen;
    use crossterm::event::KeyCode;
    use dspi_session::DeviceState;

    fn screen(data: SettingsData) -> (SettingsScreen, DeviceState) {
        let st = m::demo::state();
        let s = SettingsScreen::new(&st, data, AppConfig::default()).open(Page::Macros, &st);
        (s, st)
    }

    #[test]
    fn the_page_lists_its_macros_and_their_steps() {
        let (mut s, st) = screen(m::demo::settings_data());
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            let f = frame(&mut s, &st, w, h);
            assert_eq!(f.lines().count(), h as usize, "{w}x{h}");
            assert!(f.contains("Night"), "{w}x{h}:\n{f}");
        }
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("2 steps"), "the collapsed summary:\n{f}");
    }

    #[test]
    fn an_empty_device_shows_the_consoles_empty_state() {
        let mut d = m::demo::settings_data();
        if let Some(cs) = d.cs.as_mut() {
            cs.macros = vec![CsMacro::default(); 8];
        }
        let (mut s, st) = screen(d);
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("No Macros Configured"), "{f}");
        assert!(f.contains("mute after a delay"), "{f}");
        assert!(f.contains("Add Macro"), "{f}");
    }

    #[test]
    fn expanding_a_macro_shows_its_ordered_steps() {
        let (mut s, st) = screen(m::demo::settings_data());
        s.handle(key(KeyCode::Tab), &st);
        s.handle(key(KeyCode::Enter), &st);
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("Change"), "the step's parameter picker:\n{f}");
        assert!(f.contains("Wait Before"), "{f}");
        assert!(f.contains("Add Step"), "{f}");
    }

    /// Byte-exact, mirroring the Console's wire test. The delay is in 10 ms
    /// units, so 1.5 s is 150 and not 15.
    #[test]
    fn a_filled_step_encodes_the_consoles_record() {
        let st = CsMacroStep {
            noun: m::noun::OUTPUT_MUTE,
            action: m::act::SET,
            flags: 0,
            target: 2,
            index: 0,
            value: 1,
            step: 0,
            pre_delay: m::encode_step_delay(1.5),
        };
        let w = st.encode();
        assert_eq!(w.len(), 12);
        assert_eq!(w[0], 18, "noun = OUTPUT_MUTE");
        assert_eq!(w[1], 5, "action = SET");
        assert_eq!(w[3], 2, "target");
        assert_eq!(w[5], 0, "reserved");
        assert_eq!(&w[6..8], &[1, 0], "value little endian");
        assert_eq!(&w[10..12], &[150, 0], "pre_delay in 10 ms units");
        assert_eq!(CsMacroStep::decode(&w).unwrap(), st);
    }

    #[test]
    fn the_header_carries_the_name_and_the_step_count() {
        let mut mac = CsMacro {
            name: "Night".into(),
            step_count: 2,
            steps: vec![CsMacroStep::default(); 8],
        };
        mac.steps[0].noun = m::noun::INPUT_SOURCE;
        let w = header_of(&mac).encode();
        assert_eq!(w.len(), 36);
        assert_eq!(&w[0..5], b"Night");
        assert_eq!(w[32], 2, "step_count");
        assert_eq!(&w[33..36], &[0, 0, 0], "reserved");
    }

    #[test]
    fn removing_a_step_closes_the_gap() {
        let d = m::demo::settings_data();
        let mut p = MacrosPage::new(&d);
        assert_eq!(p.drafts[0].step_count, 2);
        let second = p.drafts[0].steps[1].clone();
        p.remove_step(0, 0);
        assert_eq!(p.drafts[0].step_count, 1);
        assert_eq!(p.drafts[0].steps[0], second, "the tail moved up");
        assert!(p.drafts[0].steps[1].is_empty(), "and the slot cleared");
    }

    #[test]
    fn moving_a_step_swaps_it_with_its_neighbour() {
        let d = m::demo::settings_data();
        let mut p = MacrosPage::new(&d);
        let (a, b) = (p.drafts[0].steps[0].clone(), p.drafts[0].steps[1].clone());
        p.move_step(0, 0, 1);
        assert_eq!(p.drafts[0].steps[0], b);
        assert_eq!(p.drafts[0].steps[1], a);
        // The ends do not wrap.
        p.move_step(0, 0, -1);
        assert_eq!(p.drafts[0].steps[0], b);
    }

    #[test]
    fn the_step_pickers_never_offer_macro_or_browse_adjust() {
        let d = m::demo::settings_data();
        let cs = d.cs.clone().unwrap();
        let nouns = m::macro_step_nouns(&cs);
        assert!(!nouns.contains(&m::noun::MACRO));
        assert!(!nouns.contains(&m::noun::PAGE_VALUE));
    }
}
