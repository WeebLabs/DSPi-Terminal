//! Settings > Control Surfaces.
//!
//! Buttons, switches, potentiometers, rotary encoders, LEDs, an IR receiver and
//! an I2C display, wired to spare GPIOs and bound to device functions. Sixteen
//! slots, every picker built from the device's own capability tables.
//!
//! The page is three editors sharing one card list, because the firmware makes
//! two of the component types containers rather than controls:
//!
//! - A **control** is one `CsBinding`: type, parameter, action, gesture,
//!   target, pins, operands, flags. It stages its edits and pushes them with
//!   Apply.
//! - The **IR receiver** carries only its pin and its sense; its remote buttons
//!   are separate device-global `IrCommand` sub-slots nested inside its card,
//!   and the receiver's one Apply pushes them alongside it.
//! - The **display** carries its wiring and its model; what it shows is a
//!   separate config record and a list of page records, which apply as they are
//!   edited because each is one small write with no dependent edits.
//!
//! Every write here is a deferred, live-only preview: the device applies it a
//! tick later and reports the outcome through `REQ_GET_CS_STATUS`, and nothing
//! reaches flash until the save bar's Save. [`cs_model::run`] does the waiting.
//!
//! The same card list, filtered to the two auxiliary output types, is
//! Settings > Control > Auxiliary Outputs, as the Console builds it
//! (`visibleSlots`, DSPi_ConsoleApp.swift:2979-2985): an aux output is a
//! binding slot too, under the same Apply, Save and Revert, so each slot
//! appears on exactly one of the two pages. [`aux_outputs`] holds that page's
//! own rows.

mod aux_outputs;

use std::collections::{BTreeMap, HashSet};

use crossterm::event::{KeyCode, KeyEvent};
use dspi_proto::packets::{display_flags, page_flags};

use crate::shell::{SessionReply, SessionRequest};
use crate::widgets::{Action, Button, Dialog, DialogOutcome, KeyHelp, PopupList, StatusTone};

use super::cs_model::{self as m, CsData};
use super::cs_model::{target_choice, target_choices};
use super::{Cx, PageEvent, Row, SettingsData, SettingsPage};
use dspi_session::surfaces::{CsBinding, CsDisplayCfg, CsDisplayPage, GPIO_UNUSED, IrCommand};

pub const EMPTY_TITLE: &str = "No Controls Configured";
pub const EMPTY_BODY: &str = "Wire a button, switch, knob, encoder, or LED to a spare GPIO and \
                              bind it to a device function.";

/// The Console's footer, verbatim.
pub const FOOTER: &str = "Wire push buttons, toggle switches, potentiometers, rotary encoders, \
                          indicator LEDs, and an IR remote receiver to spare GPIOs and bind each \
                          to a device function. Buttons and switches wire between the GPIO and GND \
                          (internal pull-up); pots use an ADC pin (GPIO 26, 27, or 28) between 3V3 \
                          and GND; encoders use two GPIOs with the common wired to GND. LEDs drive \
                          active-high by default. An IR receiver module's OUT pin connects to any \
                          GPIO, and its remote buttons are learned by pressing them at the device. \
                          This wiring is a board-level setting: it is stored on the device, \
                          survives preset changes, and survives a factory reset. Changes take \
                          effect immediately as a live preview; use Save to keep them across a \
                          reboot, or Revert to discard them.";

/// The warning the Console puts under a binding that carries a delay.
pub const DELAY_WARNING: &str = "Applying, reverting, or rebooting briefly releases the pin and \
                                 restarts the timing from off. Driving an amplifier trigger, that \
                                 is a power cycle.";

/// The warning shown when editing is gated and nothing can lift the gate.
pub const EDIT_GATE_WARNING: &str = "Nothing can arm editing, so a Browse/Adjust control can only \
                                     browse pages. Bind a button or remote key to Allow Editing.";

pub const LEARN_WAITING: &str = "Waiting for a button...";
pub const LEARN_PROMPT: &str = "Point the remote at the receiver and press the button to learn.";
pub const LEARN_TIMEOUT: &str = "No remote button was detected - try again.";
pub const IR_NOT_LIVE: &str = "Apply the receiver above before learning remote buttons.";

/// Tags for the kinds of session work this page starts. A binding's Apply uses
/// the bare slot number, so the rest start above the sixteen slots.
const TAG_DISPLAY_CFG: u32 = 0x100;
const TAG_DISPLAY_PAGE: u32 = 0x200;
const TAG_LEARN: u32 = 0x300;
const TAG_LEARN_CANCEL: u32 = 0x400;
const TAG_IR_CLEAR: u32 = 0x500;
/// An aux output's live switch and level: immediate writes that are neither a
/// preview nor unsaved (config.h:136-142), so the save bar must not count them.
const TAG_AUX_STATE: u32 = 0x600;
const TAG_AUX_LEVEL: u32 = 0x700;
const TAG_MASK: u32 = 0xF00;

/// Whether a session tag is an aux output's live switch or level, which never
/// makes the configuration unsaved.
pub(crate) fn is_live_aux_tag(tag: u32) -> bool {
    matches!(tag & TAG_MASK, TAG_AUX_STATE | TAG_AUX_LEVEL)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Add,
    Header(usize),
    Type(usize),
    Noun(usize),
    ActionPick(usize),
    Event(usize),
    Target(usize),
    Band(usize),
    Pin(usize, bool),
    SpanOn(usize),
    SpanMin(usize),
    SpanMax(usize),
    StepSize(usize),
    EnumStep(usize),
    Value(usize),
    Choice(usize),
    Bright(usize),
    OnDelay(usize),
    OffDelay(usize),
    Flag(usize, u8),
    Apply(usize),

    IrAdd,
    IrHeader(usize),
    IrNoun(usize),
    IrAction(usize),
    IrTarget(usize),
    IrBand(usize),
    IrValue(usize),
    IrChoice(usize),
    IrStep(usize),
    IrEnumStep(usize),
    IrFlag(usize, u8),
    LearnCancel,

    DispModel(usize),
    DispPins(usize),
    DispAddress(usize),
    DispMode,
    DispHome,
    DispDwell,
    DispOverlay,
    DispOverlayAny,
    DispEditTimeout,
    DispEditGated,
    DispBright,
    DispLabelAlign,
    DispValueAlign,
    PageAdd,
    PageNoun(usize),
    PageTarget(usize),
    PageLarge(usize),
    PageBar(usize),
    PageRemove(usize),

    // An auxiliary output's card.
    AuxLive(usize),
    AuxLevel(usize),
    AuxLimit(usize),
    AuxLinear(usize),
    AuxBoot(usize),
    AuxStartsOn(usize),
    AuxStartLevel(usize),
}

/// Which remote button is listening, and what `DeviceState::ir_learn` held when
/// it was armed, so a result left over from a previous learn is not adopted the
/// moment the next one arms.
type Learning = (usize, Option<(u8, u8, u32)>);

pub struct SurfacesPage {
    cursor: usize,
    drafts: Vec<CsBinding>,
    names: Vec<String>,
    ir_drafts: Vec<IrCommand>,
    /// What the device is believed to hold.
    live: CsData,
    expanded: HashSet<usize>,
    expanded_ir: HashSet<usize>,
    header_button: BTreeMap<usize, usize>,
    ir_button: BTreeMap<usize, usize>,
    apply_button: BTreeMap<usize, usize>,
    page_button: BTreeMap<usize, usize>,
    add_button: usize,
    messages: BTreeMap<usize, (String, bool)>,
    ir_messages: BTreeMap<usize, (String, bool)>,
    applying: Option<usize>,
    learning: Option<Learning>,
    /// The device's answer to the last display config or page write.
    display_status: Option<String>,
    popup: Option<Item>,
    dialog: Option<Item>,
    /// True on Auxiliary Outputs, which shows the aux slots and nothing else;
    /// false on Control Surfaces, which shows everything but them.
    aux_page: bool,
    /// The live switch and level each aux output shows, taken from
    /// `DeviceState::cs_aux_states` on every look.
    aux: dspi_session::surfaces::CsAuxStates,
    /// A refused live switch or level, per slot.
    aux_messages: BTreeMap<usize, String>,
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

impl SurfacesPage {
    pub fn new(data: &SettingsData) -> Self {
        let live = data.cs.clone().unwrap_or_default();
        Self {
            cursor: 0,
            drafts: live.bindings.clone(),
            names: live.names.clone(),
            ir_drafts: live.ir.clone(),
            live,
            expanded: HashSet::new(),
            expanded_ir: HashSet::new(),
            header_button: BTreeMap::new(),
            ir_button: BTreeMap::new(),
            apply_button: BTreeMap::new(),
            page_button: BTreeMap::new(),
            add_button: 0,
            messages: BTreeMap::new(),
            ir_messages: BTreeMap::new(),
            applying: None,
            learning: None,
            display_status: None,
            popup: None,
            dialog: None,
            aux_page: false,
            aux: Default::default(),
            aux_messages: BTreeMap::new(),
        }
    }

    /// Settings > Control > Auxiliary Outputs: the same cards, filtered to
    /// the two aux types.
    pub fn aux(data: &SettingsData) -> Self {
        Self {
            aux_page: true,
            ..Self::new(data)
        }
    }

    fn adopt(&mut self, cs: &CsData) {
        if self.drafts.len() != cs.bindings.len() {
            self.drafts = cs.bindings.clone();
            self.names = cs.names.clone();
            self.ir_drafts = cs.ir.clone();
        }
        // The records themselves follow the device wherever this page holds
        // no edit of its own. The other page of the pair (Control Surfaces
        // and Auxiliary Outputs share the slots) and the Console beside this
        // can both change a slot, and a card that kept its snapshot would
        // offer that slot as free or re-apply the old record over the new.
        // A staged edit is kept: it is the user's, and Apply is how it goes.
        // The slot being applied is left to the apply's own answer.
        let same_shape = self.live.bindings.len() == cs.bindings.len()
            && self.live.names.len() == cs.names.len();
        for slot in 0..self.drafts.len().min(cs.bindings.len()) {
            if self.applying == Some(slot) {
                continue;
            }
            if self.drafts[slot] == self.live.binding(slot) {
                self.drafts[slot] = cs.bindings[slot].clone();
            }
            if self.names[slot] == self.live.name(slot) {
                self.names[slot] = cs.name(slot);
            }
            if same_shape {
                self.live.bindings[slot] = cs.bindings[slot].clone();
                self.live.names[slot] = cs.name(slot);
            }
        }
        if !same_shape {
            self.live.bindings = cs.bindings.clone();
            self.live.names = cs.names.clone();
        }
        for sub in 0..self.ir_drafts.len().min(cs.ir.len()) {
            if self.live.ir.get(sub) == Some(&self.ir_drafts[sub]) {
                self.ir_drafts[sub] = cs.ir[sub].clone();
            }
        }
        // Remote buttons are applied with the receiver's slot.
        if self.applying.is_none() {
            self.live.ir = cs.ir.clone();
        }
        // Everything the device reports about itself rather than holds for us:
        // slot health, the panel's own state, and the group and macro tables a
        // target picker reads. `SettingsData` is re-read after every write on
        // this page, so these follow the device rather than freezing at open.
        self.live.status = cs.status.clone();
        self.live.ext = cs.ext.clone();
        self.live.display_status = cs.display_status.clone();
        self.live.groups = cs.groups.clone();
        self.live.macros = cs.macros.clone();
    }

    /// A learn result arrives on the notification stream, not as the answer to
    /// anything this page asked for, so it is picked up here.
    fn observe_learn(&mut self, cx: &Cx<'_>) {
        let Some((sub, armed_with)) = self.learning else {
            return;
        };
        let Some(result) = cx.state.ir_learn else {
            return;
        };
        if Some(result) == armed_with {
            return; // the previous learn's result, still standing
        }
        let (state, protocol, code) = result;
        match state {
            // ARMED: still listening.
            1 => {}
            2 if code != 0 => {
                self.learning = None;
                if let Some(c) = self.ir_drafts.get_mut(sub) {
                    c.protocol = protocol;
                    c.code = code;
                }
                self.ir_messages.insert(
                    sub,
                    (
                        format!(
                            "Learned a {} code. Apply to keep it.",
                            m::ir_protocol_name(protocol)
                        ),
                        false,
                    ),
                );
            }
            3 => {
                self.learning = None;
                self.ir_messages.insert(sub, (LEARN_TIMEOUT.into(), true));
            }
            _ => {
                self.learning = None;
                self.ir_messages
                    .insert(sub, ("Learn stopped.".into(), false));
            }
        }
    }

    // ------------------------------------------------------------- the model

    fn slot_count(&self) -> usize {
        self.drafts.len()
    }

    fn configured(&self, slot: usize) -> bool {
        !self.drafts[slot].is_empty() || !self.live.binding(slot).is_empty()
    }

    /// True when the slot holds an aux output, staged or live, which puts it
    /// on the Auxiliary Outputs page and off this one's twin.
    fn is_aux_slot(&self, slot: usize) -> bool {
        m::is_aux(self.drafts[slot].component) || m::is_aux(self.live.binding(slot).component)
    }

    fn visible(&self) -> Vec<usize> {
        (0..self.slot_count())
            .filter(|s| self.configured(*s) && self.is_aux_slot(*s) == self.aux_page)
            .collect()
    }

    fn first_free(&self) -> Option<usize> {
        (0..self.slot_count())
            .find(|s| self.drafts[*s].is_empty() && self.live.binding(*s).is_empty())
    }

    /// The card's own unapplied edits: a binding edit or a staged rename.
    fn self_dirty(&self, slot: usize) -> bool {
        self.drafts[slot] != self.live.binding(slot) || self.names[slot] != self.live.name(slot)
    }

    /// Unapplied edits to any learned remote button. They are device-global and
    /// edited inside the receiver's card, so they count as pending work on it.
    fn ir_dirty(&self) -> bool {
        (0..self.ir_drafts.len()).any(|s| {
            let (d, live) = (&self.ir_drafts[s], &self.live.ir[s]);
            d != live && (!d.is_empty() || !live.is_empty())
        })
    }

    fn dirty(&self, slot: usize) -> bool {
        self.self_dirty(slot) || (self.drafts[slot].component == m::ty::IR && self.ir_dirty())
    }

    /// Types offerable when adding a control: the IR receiver and the display
    /// are one per device, so each drops out once it exists. The aux outputs
    /// are added from their own page and nowhere else (the Console's
    /// `addableTypes`, DSPi_ConsoleApp.swift:6110-6119).
    fn addable_types(&self) -> Vec<u8> {
        self.live
            .real_types()
            .into_iter()
            .filter(|t| m::is_aux(*t) == self.aux_page)
            .filter(|t| match *t {
                m::ty::IR => m::container_slot(&self.drafts, &self.live, m::ty::IR).is_none(),
                m::ty::DISPLAY => {
                    m::container_slot(&self.drafts, &self.live, m::ty::DISPLAY).is_none()
                }
                _ => true,
            })
            .collect()
    }

    /// Types a slot may become: every type, minus a container another slot
    /// already holds. An aux slot stays an aux output (of either kind) and a
    /// control never becomes one, because the two live on different pages
    /// (`typeMenuTypes`, DSPi_ConsoleApp.swift:3079-3084).
    fn type_options(&self, slot: usize) -> Vec<u8> {
        let aux = self.is_aux_slot(slot);
        self.live
            .real_types()
            .into_iter()
            .filter(|t| m::is_aux(*t) == aux)
            .filter(|t| match *t {
                m::ty::IR => {
                    m::container_slot(&self.drafts, &self.live, m::ty::IR).is_none_or(|s| s == slot)
                }
                m::ty::DISPLAY => m::container_slot(&self.drafts, &self.live, m::ty::DISPLAY)
                    .is_none_or(|s| s == slot),
                _ => true,
            })
            .collect()
    }

    fn ir_visible(&self) -> Vec<usize> {
        (0..self.ir_drafts.len())
            .filter(|s| self.ir_drafts[*s] != IrCommand::default() || !self.live.ir[*s].is_empty())
            .collect()
    }

    fn ir_first_free(&self) -> Option<usize> {
        (0..self.ir_drafts.len())
            .find(|s| self.ir_drafts[*s] == IrCommand::default() && self.live.ir[*s].is_empty())
    }

    /// Whether anything can actually arm editing: a control, a remote key, or a
    /// macro step that writes the noun. An LED bound to it only reports the
    /// state, so the indicator actions do not count.
    fn can_arm_editing(&self) -> bool {
        let writes = |n: u8, a: u8| {
            n == m::noun::DISPLAY_EDIT
                && a != m::act::IND_EQUALS
                && a != m::act::IND_ABOVE
                && a != m::act::IND_LEVEL
        };
        self.drafts.iter().any(|b| writes(b.noun, b.action))
            || self.ir_drafts.iter().any(|c| writes(c.noun, c.action))
            || self
                .live
                .macros
                .iter()
                .any(|mac| mac.active_steps().iter().any(|s| writes(s.noun, s.action)))
    }

    fn uses_page_value(&self) -> bool {
        self.drafts
            .iter()
            .any(|b| !b.is_empty() && b.noun == m::noun::PAGE_VALUE)
            || self
                .ir_drafts
                .iter()
                .any(|c| !c.is_empty() && c.noun == m::noun::PAGE_VALUE)
    }

    // -------------------------------------------------------------- the rows

    fn pin_rows(&self, cx: &Cx<'_>, slot: usize) -> Vec<(Option<Item>, Row)> {
        let b = &self.drafts[slot];
        let td = self
            .live
            .type_desc(b.component)
            .copied()
            .unwrap_or_default();
        let two_pin = td.pin_count >= 2;
        let mut rows = Vec::new();
        let mut one = |item: Item, label: &str, detail: &str, is_second: bool| {
            let pins = m::pin_candidates(cx, &self.live, &self.drafts, slot, is_second);
            let current = if is_second { b.gpio[1] } else { b.gpio[0] };
            // A free pin is just "GPIO n"; a claimed one that is still offered
            // - a button sharing another button's GPIO, one gesture each -
            // names its owner, so nothing is picked up by accident.
            let owner = m::slot_owner(slot);
            let mut choices: Vec<String> = pins
                .iter()
                .map(|p| match cx.data.owner_of(cx.state, *p, &owner) {
                    Some(who) => format!("GPIO {p} ({who})"),
                    None => format!("GPIO {p}"),
                })
                .collect();
            let selected = match pins.iter().position(|p| *p == current) {
                Some(i) => i,
                None => {
                    choices.insert(
                        0,
                        if current == GPIO_UNUSED {
                            "None".into()
                        } else {
                            format!("GPIO {current}")
                        },
                    );
                    0
                }
            };
            rows.push((
                Some(item),
                Row::Pick {
                    label: label.into(),
                    choices,
                    selected,
                    caption: Some(detail.into()),
                    enabled: true,
                },
            ));
        };
        if two_pin {
            one(
                Item::Pin(slot, false),
                "GPIO A",
                "Encoder channel A.",
                false,
            );
            one(Item::Pin(slot, true), "GPIO B", "Encoder channel B.", true);
        } else {
            one(
                Item::Pin(slot, false),
                "GPIO",
                m::pin_detail(b.component),
                false,
            );
        }
        rows
    }

    fn operand_rows(&self, cx: &Cx<'_>, slot: usize) -> Vec<(Option<Item>, Row)> {
        let b = &self.drafts[slot];
        let cs = &self.live;
        let mut rows: Vec<(Option<Item>, Row)> = Vec::new();
        let k = m::operand_kind(cs, b.noun);
        let u = m::noun_unit(cs, b.noun);
        let (lo, hi) = m::noun_range(cs, b.noun);

        let span =
            |rows: &mut Vec<(Option<Item>, Row)>, title: &str, lo_detail: &str, hi_detail: &str| {
                let custom = b.range_min != 0 || b.range_max != 0;
                rows.push((
                    Some(Item::SpanOn(slot)),
                    Row::Toggle {
                        label: title.into(),
                        on: custom,
                        caption: Some(format!(
                            "Map onto a portion of the {} to {} range.",
                            m::fmt_unit(lo, u),
                            m::fmt_unit(hi, u)
                        )),
                        enabled: true,
                    },
                ));
                if custom {
                    rows.push((
                        Some(Item::SpanMin(slot)),
                        Row::Number {
                            label: "Minimum".into(),
                            value: m::decode_value(b.range_min, u),
                            min: lo,
                            max: hi,
                            step: m::unit_scroll_step(u),
                            unit: m::unit_symbol(u).into(),
                            decimals: m::unit_decimals(u),
                            caption: Some(lo_detail.into()),
                            enabled: true,
                        },
                    ));
                    rows.push((
                        Some(Item::SpanMax(slot)),
                        Row::Number {
                            label: "Maximum".into(),
                            value: m::decode_value(b.range_max, u),
                            min: lo,
                            max: hi,
                            step: m::unit_scroll_step(u),
                            unit: m::unit_symbol(u).into(),
                            decimals: m::unit_decimals(u),
                            caption: Some(hi_detail.into()),
                            enabled: true,
                        },
                    ));
                }
            };

        let value_row = |rows: &mut Vec<(Option<Item>, Row)>, title: &str, detail: &str| {
            // A threshold parked on the range floor compares true even in
            // silence, so the LED never goes out. The floor is a legal value,
            // so say what it does rather than refuse it.
            let at_floor = b.action == m::act::IND_ABOVE
                && b.value <= cs.noun_desc(b.noun).map(|d| d.min_q).unwrap_or(i16::MIN);
            let range = format!("({} to {}).", m::fmt_unit(lo, u), m::fmt_unit(hi, u));
            let caption = if at_floor {
                format!(
                    "{detail} {range} At the {} floor this is always true, so the LED stays lit.",
                    m::fmt_unit(lo, u)
                )
            } else {
                format!("{detail} {range}")
            };
            rows.push((
                Some(Item::Value(slot)),
                Row::Number {
                    label: title.into(),
                    value: m::decode_value(b.value, u),
                    min: lo,
                    max: hi,
                    step: m::unit_scroll_step(u),
                    unit: m::unit_symbol(u).into(),
                    decimals: m::unit_decimals(u),
                    caption: Some(caption),
                    enabled: true,
                },
            ));
        };

        let choice_row = |rows: &mut Vec<(Option<Item>, Row)>, title: &str, detail: &str| {
            if k == m::kind::BOOL {
                rows.push((
                    Some(Item::Choice(slot)),
                    Row::Pick {
                        label: title.into(),
                        choices: vec![
                            m::bool_label(b.noun, false).into(),
                            m::bool_label(b.noun, true).into(),
                        ],
                        selected: usize::from(b.value != 0),
                        caption: Some(detail.into()),
                        enabled: true,
                    },
                ));
            } else {
                let count = cs
                    .noun_desc(b.noun)
                    .map(|d| d.enum_count.max(1))
                    .unwrap_or(1);
                rows.push((
                    Some(Item::Choice(slot)),
                    Row::Pick {
                        label: title.into(),
                        choices: (0..count)
                            .map(|v| m::enum_value_label(cx, cs, b.noun, v as i32))
                            .collect(),
                        selected: (b.value.max(0) as usize).min(count as usize - 1),
                        caption: Some(detail.into()),
                        enabled: true,
                    },
                ));
            }
        };

        let step_row = |rows: &mut Vec<(Option<Item>, Row)>| {
            if k == m::kind::CONTINUOUS {
                let is_log = m::unit_is_log(u);
                let cur = if b.step == 0 {
                    m::default_step(u)
                } else {
                    m::decode_step(b.step, u)
                };
                rows.push((
                    Some(Item::StepSize(slot)),
                    Row::Number {
                        label: "Step Size".into(),
                        value: cur,
                        min: if is_log {
                            1.0 / 48.0
                        } else {
                            m::unit_min_step(u)
                        },
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
                                "Ratio per detent/press, in octaves."
                            } else {
                                "Amount added or removed per detent/press."
                            }
                            .into(),
                        ),
                        enabled: true,
                    },
                ));
            } else if k == m::kind::ENUM {
                let max = cs
                    .noun_desc(b.noun)
                    .map(|d| d.enum_count.max(2) - 1)
                    .unwrap_or(1);
                rows.push((
                    Some(Item::EnumStep(slot)),
                    Row::Number {
                        label: "Step Size".into(),
                        value: if b.step <= 0 { 1.0 } else { b.step as f64 },
                        min: 1.0,
                        max: max as f64,
                        step: 1.0,
                        unit: String::new(),
                        decimals: 0,
                        caption: Some("Positions advanced per detent/press.".into()),
                        enabled: true,
                    },
                ));
            }
        };

        match b.action {
            m::act::ADJUST => {
                if k == m::kind::CONTINUOUS {
                    span(
                        &mut rows,
                        "Limit Range",
                        "Level at the fully counter-clockwise position.",
                        "Level at the fully clockwise position.",
                    );
                }
            }
            m::act::STEP | m::act::INC | m::act::DEC => step_row(&mut rows),
            m::act::SET | m::act::MOMENTARY => {
                let verb = if b.action == m::act::MOMENTARY {
                    "Hold Value"
                } else {
                    "Set To"
                };
                match k {
                    m::kind::CONTINUOUS => value_row(&mut rows, verb, "Level each press applies."),
                    m::kind::BOOL => choice_row(&mut rows, verb, "The state a press applies."),
                    m::kind::ENUM => choice_row(&mut rows, verb, "The selection a press applies."),
                    _ => {}
                }
            }
            m::act::IND_EQUALS => match k {
                m::kind::BOOL => choice_row(
                    &mut rows,
                    "Light When",
                    "Light the LED when the function is in this state.",
                ),
                m::kind::ENUM => {
                    choice_row(&mut rows, "Light When", "Light the LED for this selection.")
                }
                _ => {}
            },
            m::act::IND_ABOVE => {
                if k == m::kind::CONTINUOUS {
                    value_row(
                        &mut rows,
                        "Light Above",
                        "Light the LED once the value reaches this.",
                    );
                }
            }
            m::act::IND_LEVEL => {
                if k == m::kind::CONTINUOUS {
                    span(
                        &mut rows,
                        "Brightness Range",
                        "Value mapped to the LED fully off.",
                        "Value mapped to the LED fully lit.",
                    );
                }
            }
            _ => {}
        }

        // Condition timing rides alongside whichever operand row the indicator
        // action drew: a brightness meter has no edge to time.
        if m::delays_allowed(b.component, b.action) {
            rows.extend(self.delay_rows(slot));
        }

        // The ceiling scales whatever duty the action worked out, so it belongs
        // to the LED rather than to any one action.
        if b.component == m::ty::LED_PWM {
            rows.push((
                Some(Item::Bright(slot)),
                Row::Number {
                    label: "Brightness Limit".into(),
                    value: if b.base_bright == 0 {
                        m::LED_BRIGHT_MAX as f64
                    } else {
                        b.base_bright as f64
                    },
                    min: 1.0,
                    max: m::LED_BRIGHT_MAX as f64,
                    step: 5.0,
                    unit: "%".into(),
                    decimals: 0,
                    caption: Some(
                        "Cap on how bright this LED gets, as a share of full. Everything below \
                         the cap scales with it."
                            .into(),
                    ),
                    enabled: true,
                },
            ));
        }
        rows
    }

    /// The turn-on and turn-off delays, the TON/TOF filter on an indicator's
    /// condition or an aux output's switch, and the warning a delay carries.
    fn delay_rows(&self, slot: usize) -> Vec<(Option<Item>, Row)> {
        let b = &self.drafts[slot];
        let mut rows: Vec<(Option<Item>, Row)> = Vec::new();
        let cap = format!(
            "Up to {} min {} s.",
            m::DELAY_MAX_SECONDS / 60,
            m::DELAY_MAX_SECONDS % 60
        );
        rows.push((
            Some(Item::OnDelay(slot)),
            Row::Number {
                label: "Turn-On Delay".into(),
                value: m::decode_delay(b.on_delay) as f64,
                min: 0.0,
                max: m::DELAY_MAX_SECONDS as f64,
                step: 1.0,
                unit: "s".into(),
                decimals: 0,
                caption: Some(format!(
                    "Hold off until the condition has been true this long. Any interruption \
                         restarts the wait. {cap}"
                )),
                enabled: true,
            },
        ));
        rows.push((
            Some(Item::OffDelay(slot)),
            Row::Number {
                label: "Turn-Off Delay".into(),
                value: m::decode_delay(b.off_delay) as f64,
                min: 0.0,
                max: m::DELAY_MAX_SECONDS as f64,
                step: 1.0,
                unit: "s".into(),
                decimals: 0,
                caption: Some(format!(
                    "Stay lit until the condition has been false this long - long enough to \
                         hold an amplifier trigger on through quiet passages. {cap}"
                )),
                enabled: true,
            },
        ));
        if b.on_delay != 0 || b.off_delay != 0 {
            rows.push((None, Row::note(DELAY_WARNING)));
        }
        rows
    }

    fn flag_rows(&self, slot: usize) -> Vec<(Option<Item>, Row)> {
        let b = &self.drafts[slot];
        let cs = &self.live;
        let k = m::operand_kind(cs, b.noun);
        let is_enum_step =
            k == m::kind::ENUM && matches!(b.action, m::act::STEP | m::act::INC | m::act::DEC);
        let mut rows: Vec<(Option<Item>, Row)> = Vec::new();
        let mut flag = |mask: u8, title: String, detail: String| {
            rows.push((
                Some(Item::Flag(slot, mask)),
                Row::Toggle {
                    label: title,
                    on: b.flags & mask != 0,
                    caption: Some(detail),
                    enabled: true,
                },
            ));
        };
        if b.component == m::ty::POT || b.component == m::ty::ENCODER {
            flag(
                m::flag::REVERSE,
                "Reverse Direction".into(),
                if b.component == m::ty::POT {
                    "Clockwise decreases the value.".into()
                } else {
                    "Clockwise steps down.".into()
                },
            );
        }
        if b.component == m::ty::ENCODER {
            flag(
                m::flag::ACCEL,
                "Acceleration".into(),
                "Fast spins move in larger steps; slow spins stay fine.".into(),
            );
        }
        if is_enum_step {
            flag(
                m::flag::WRAP,
                "Wrap Around".into(),
                "Step past the last position back to the first.".into(),
            );
        }
        if b.component == m::ty::BUTTON
            && (b.action == m::act::INC || b.action == m::act::DEC)
            && b.event == 0
        {
            flag(
                m::flag::REPEAT,
                "Repeat While Held".into(),
                "After holding ~0.4 s the press repeats automatically.".into(),
            );
        }
        // Both group modifiers need GROUP, and each is legal for exactly one
        // shape of control: ADJUST is the only action where the two link laws
        // differ, and an any/all choice needs a boolean condition to combine.
        if b.flags & m::flag::GROUP != 0 {
            if b.action == m::act::ADJUST && k == m::kind::CONTINUOUS {
                flag(
                    m::flag::LINK_ABS,
                    "Match Members Exactly".into(),
                    "Drive every member to the same value. Off keeps the offsets between them, \
                     moving the group's average to the knob."
                        .into(),
                );
            }
            if b.action == m::act::IND_EQUALS || b.action == m::act::IND_ABOVE {
                flag(
                    m::flag::GROUP_ALL,
                    "Require Every Member".into(),
                    "Light only when all members match. Off lights when any one does.".into(),
                );
            }
        }
        flag(
            m::flag::INVERT,
            m::invert_title(b.component).into(),
            m::invert_detail(b.component).into(),
        );
        rows
    }

    fn target_rows(&self, cx: &Cx<'_>, slot: usize) -> Vec<(Option<Item>, Row)> {
        let b = &self.drafts[slot];
        let cs = &self.live;
        let mut rows: Vec<(Option<Item>, Row)> = Vec::new();
        let Some(nd) = cs.noun_desc(b.noun).copied() else {
            return rows;
        };
        if !nd.is_targeted() {
            return rows;
        }
        let grouped = b.flags & m::flag::GROUP != 0;
        // TRIGGER is never groupable: the firmware has no targeted trigger.
        let usable = if b.action == m::act::TRIGGER {
            Vec::new()
        } else {
            m::compatible_groups(cs, b.noun)
        };
        let (choices, selected) =
            target_choices(cx, cs, b.noun, Some(slot), &usable, b.target, grouped);
        // An aux noun is not choosing a channel, so the row stops saying so
        // (`targetRows`, DSPi_ConsoleApp.swift:4690-4693).
        let alone = usable.is_empty() && !grouped;
        rows.push((
            Some(Item::Target(slot)),
            Row::Pick {
                label: if alone {
                    m::target_noun(&nd).into()
                } else {
                    "Channel or Group".into()
                },
                choices,
                selected,
                caption: Some(if alone {
                    format!(
                        "Which {} this control affects.",
                        m::target_noun(&nd).to_lowercase()
                    )
                } else {
                    "Which channel, or named set of channels, this control affects.".into()
                }),
                enabled: true,
            },
        ));
        if nd.has_band() {
            let bands = m::band_options(cs, cx.state, b.noun, b.target, grouped);
            rows.push((
                Some(Item::Band(slot)),
                Row::Pick {
                    label: "Band".into(),
                    choices: bands.iter().map(|x| m::band_name(*x)).collect(),
                    selected: bands.iter().position(|x| *x == b.index).unwrap_or(0),
                    caption: Some(
                        if grouped {
                            "Which filter band this control affects, on every member of the group."
                        } else {
                            "Which filter band this control affects."
                        }
                        .into(),
                    ),
                    enabled: true,
                },
            ));
        }
        rows
    }

    // ------------------------------------------------------------------- IR

    fn ir_card(&self, cx: &Cx<'_>, sub: usize, receiver_live: bool) -> Vec<(Option<Item>, Row)> {
        let c = self.ir_drafts[sub].clone();
        let cs = &self.live;
        let learning = self.learning.map(|(s, _)| s) == Some(sub);
        let expanded = self.expanded_ir.contains(&sub);
        let mut rows: Vec<(Option<Item>, Row)> = Vec::new();
        let chip = if c.is_empty() {
            "Not learned".to_string()
        } else {
            format!("{} 0x{:08X}", m::ir_protocol_name(c.protocol), c.code)
        };
        rows.push((
            None,
            Row::Pill {
                label: format!("  {} {chip}", if expanded { "▾" } else { "▸" }),
                text: if c.is_empty() { "Empty" } else { "Learned" }.into(),
                tone: if c.is_empty() {
                    StatusTone::Neutral
                } else {
                    StatusTone::Ok
                },
                caption: (!c.is_empty()).then(|| m::ir_verb_phrase(cx.state, cs, &c)),
            },
        ));
        if learning {
            rows.push((
                None,
                Row::Status(format!("{LEARN_WAITING} {LEARN_PROMPT}"), false),
            ));
            rows.push((
                Some(Item::LearnCancel),
                Row::Buttons {
                    label: String::new(),
                    caption: None,
                    buttons: vec!["Cancel".into()],
                    cursor: 0,
                    enabled: true,
                },
            ));
        } else {
            rows.push((
                Some(Item::IrHeader(sub)),
                Row::Buttons {
                    label: String::new(),
                    caption: None,
                    buttons: vec![
                        if expanded { "Collapse" } else { "Expand" }.into(),
                        if c.is_empty() {
                            "Learn Button"
                        } else {
                            "Re-learn"
                        }
                        .into(),
                        "Remove".into(),
                    ],
                    cursor: self.ir_button.get(&sub).copied().unwrap_or(0),
                    enabled: true,
                },
            ));
        }
        if !learning && let Some((text, err)) = self.ir_messages.get(&sub) {
            rows.push((None, Row::Status(text.clone(), *err)));
        }
        if !expanded {
            return rows;
        }

        let nouns = m::noun_choices(cs, m::ty::IR);
        rows.push((
            Some(Item::IrNoun(sub)),
            Row::Pick {
                label: "Controls".into(),
                choices: nouns
                    .iter()
                    .map(|(cat, n)| m::noun_choice_label(cat, *n, m::ty::IR))
                    .collect(),
                selected: nouns.iter().position(|(_, n)| *n == c.noun).unwrap_or(0),
                caption: Some("The device function this remote button drives.".into()),
                enabled: true,
            },
        ));

        if let Some(nd) = cs.noun_desc(c.noun).copied()
            && nd.is_targeted()
        {
            let grouped = c.flags & m::flag::GROUP != 0;
            let usable = m::compatible_groups(cs, c.noun);
            let (choices, selected) =
                target_choices(cx, cs, c.noun, None, &usable, c.target, grouped);
            rows.push((
                Some(Item::IrTarget(sub)),
                Row::Pick {
                    label: if usable.is_empty() && !grouped {
                        m::target_noun(&nd).into()
                    } else {
                        "Channel or Group".into()
                    },
                    choices,
                    selected,
                    caption: Some("Which channel, or named set of channels, this affects.".into()),
                    enabled: true,
                },
            ));
            if nd.has_band() {
                let bands = m::band_options(cs, cx.state, c.noun, c.target, grouped);
                rows.push((
                    Some(Item::IrBand(sub)),
                    Row::Pick {
                        label: "Band".into(),
                        choices: bands.iter().map(|x| m::band_name(*x)).collect(),
                        selected: bands.iter().position(|x| *x == c.index).unwrap_or(0),
                        caption: Some("Which filter band this affects.".into()),
                        enabled: true,
                    },
                ));
            }
        }

        let acts = m::valid_actions(cs, m::ty::IR, c.noun);
        let k = m::operand_kind(cs, c.noun);
        if acts.len() > 1 {
            rows.push((
                Some(Item::IrAction(sub)),
                Row::Pick {
                    label: "On Press".into(),
                    choices: acts
                        .iter()
                        .map(|a| m::action_name(*a, c.noun, k == m::kind::ENUM))
                        .collect(),
                    selected: acts.iter().position(|a| *a == c.action).unwrap_or(0),
                    caption: Some("What pressing the remote button does.".into()),
                    enabled: true,
                },
            ));
        }

        let u = m::noun_unit(cs, c.noun);
        let (lo, hi) = m::noun_range(cs, c.noun);
        match c.action {
            m::act::INC | m::act::DEC => {
                if k == m::kind::CONTINUOUS {
                    let is_log = m::unit_is_log(u);
                    let cur = if c.step == 0 {
                        m::default_step(u)
                    } else {
                        m::decode_step(c.step, u)
                    };
                    rows.push((
                        Some(Item::IrStep(sub)),
                        Row::Number {
                            label: "Step Size".into(),
                            value: cur,
                            min: if is_log {
                                1.0 / 48.0
                            } else {
                                m::unit_min_step(u)
                            },
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
                                    "Ratio per press, in octaves."
                                } else {
                                    "Amount added or removed per press."
                                }
                                .into(),
                            ),
                            enabled: true,
                        },
                    ));
                } else if k == m::kind::ENUM {
                    let max = cs
                        .noun_desc(c.noun)
                        .map(|d| d.enum_count.max(2) - 1)
                        .unwrap_or(1);
                    rows.push((
                        Some(Item::IrEnumStep(sub)),
                        Row::Number {
                            label: "Step Size".into(),
                            value: if c.step <= 0 { 1.0 } else { c.step as f64 },
                            min: 1.0,
                            max: max as f64,
                            step: 1.0,
                            unit: String::new(),
                            decimals: 0,
                            caption: Some("Positions advanced per press.".into()),
                            enabled: true,
                        },
                    ));
                }
            }
            m::act::SET | m::act::MOMENTARY => {
                let verb = if c.action == m::act::MOMENTARY {
                    "Hold Value"
                } else {
                    "Set To"
                };
                match k {
                    m::kind::CONTINUOUS => rows.push((
                        Some(Item::IrValue(sub)),
                        Row::Number {
                            label: verb.into(),
                            value: m::decode_value(c.value, u),
                            min: lo,
                            max: hi,
                            step: m::unit_scroll_step(u),
                            unit: m::unit_symbol(u).into(),
                            decimals: m::unit_decimals(u),
                            caption: Some(format!(
                                "Level each press applies ({} to {}).",
                                m::fmt_unit(lo, u),
                                m::fmt_unit(hi, u)
                            )),
                            enabled: true,
                        },
                    )),
                    m::kind::BOOL => rows.push((
                        Some(Item::IrChoice(sub)),
                        Row::Pick {
                            label: verb.into(),
                            choices: vec![
                                m::bool_label(c.noun, false).into(),
                                m::bool_label(c.noun, true).into(),
                            ],
                            selected: usize::from(c.value != 0),
                            caption: Some("The state a press applies.".into()),
                            enabled: true,
                        },
                    )),
                    m::kind::ENUM => {
                        let count = cs
                            .noun_desc(c.noun)
                            .map(|d| d.enum_count.max(1))
                            .unwrap_or(1);
                        rows.push((
                            Some(Item::IrChoice(sub)),
                            Row::Pick {
                                label: verb.into(),
                                choices: (0..count)
                                    .map(|v| m::enum_value_label(cx, cs, c.noun, v as i32))
                                    .collect(),
                                selected: (c.value.max(0) as usize).min(count as usize - 1),
                                caption: Some("The selection a press applies.".into()),
                                enabled: true,
                            },
                        ));
                    }
                    _ => {}
                }
            }
            _ => {}
        }

        // An IR command carries only WRAP and REPEAT.
        if k == m::kind::ENUM && (c.action == m::act::INC || c.action == m::act::DEC) {
            rows.push((
                Some(Item::IrFlag(sub, m::flag::WRAP)),
                Row::Toggle {
                    label: "Wrap Around".into(),
                    on: c.flags & m::flag::WRAP != 0,
                    caption: Some("Step past the last position back to the first.".into()),
                    enabled: true,
                },
            ));
        }
        if c.action == m::act::INC || c.action == m::act::DEC {
            rows.push((
                Some(Item::IrFlag(sub, m::flag::REPEAT)),
                Row::Toggle {
                    label: "Repeat While Held".into(),
                    on: c.flags & m::flag::REPEAT != 0,
                    caption: Some("Holding the remote button repeats the step.".into()),
                    enabled: true,
                },
            ));
        }
        let _ = receiver_live;
        rows
    }

    fn ir_section(&self, cx: &Cx<'_>, slot: usize) -> Vec<(Option<Item>, Row)> {
        let mut rows: Vec<(Option<Item>, Row)> = Vec::new();
        let receiver_live = self.live.status.is_slot_active(slot as u8);
        let visible = self.ir_visible();
        rows.push((None, Row::Blank));
        rows.push((
            None,
            Row::pair(
                "REMOTE BUTTONS",
                format!("{}/{}", visible.len(), self.ir_drafts.len()),
            ),
        ));
        rows.push((
            None,
            Row::note("Learn buttons on any remote and bind each to a function."),
        ));
        if !receiver_live {
            rows.push((None, Row::note(IR_NOT_LIVE)));
        }
        for sub in visible {
            rows.extend(self.ir_card(cx, sub, receiver_live));
        }
        rows.push((
            Some(Item::IrAdd),
            Row::Buttons {
                label: String::new(),
                caption: None,
                buttons: vec!["Add Remote Button".into()],
                cursor: 0,
                enabled: self.ir_first_free().is_some() && cx.connected,
            },
        ));
        rows
    }

    // -------------------------------------------------------------- display

    fn display_rows(&self, cx: &Cx<'_>, slot: usize) -> Vec<(Option<Item>, Row)> {
        let b = &self.drafts[slot];
        let cs = &self.live;
        let cfg = &cs.display_cfg;
        let mut rows: Vec<(Option<Item>, Row)> = Vec::new();

        if let Some(msg) = &self.display_status {
            rows.push((None, Row::Status(msg.clone(), true)));
        }

        rows.push((None, Row::section("WIRING")));
        let models: Vec<u8> = (1..=cs.caps.display_models.max(1)).collect();
        rows.push((
            Some(Item::DispModel(slot)),
            Row::Pick {
                label: "Model".into(),
                choices: models.iter().map(|x| m::display_model_name(*x)).collect(),
                selected: models.iter().position(|x| *x == b.index).unwrap_or(0),
                caption: Some("The type of display connected.".into()),
                enabled: true,
            },
        ));
        // SCL is not offered separately: the pin mux fixes it as SDA + 1, so a
        // second picker could only build a pair the device rejects.
        let sdas = m::i2c_sda_candidates(cx, &self.drafts, slot);
        let mut pair_choices: Vec<String> = sdas
            .iter()
            .map(|p| format!("GPIO {p} / {}", p + 1))
            .collect();
        let pair_selected = match sdas.iter().position(|p| *p == b.gpio[0]) {
            Some(i) => i,
            None => {
                pair_choices.insert(0, format!("GPIO {} / {}", b.gpio[0], b.gpio[0] + 1));
                0
            }
        };
        rows.push((
            Some(Item::DispPins(slot)),
            Row::Pick {
                label: "SDA/SCL Pins".into(),
                choices: pair_choices,
                selected: pair_selected,
                caption: Some("Clock and data pins are chosen in fixed pairs.".into()),
                enabled: true,
            },
        ));
        let mut addr_choices = vec!["Default".to_string()];
        addr_choices.extend(m::DISPLAY_ADDRESSES.iter().map(|a| format!("0x{a:X}")));
        rows.push((
            Some(Item::DispAddress(slot)),
            Row::Pick {
                label: "Address".into(),
                choices: addr_choices,
                selected: if b.value == 0 {
                    0
                } else {
                    m::DISPLAY_ADDRESSES
                        .iter()
                        .position(|a| *a == b.value)
                        .map(|i| i + 1)
                        .unwrap_or(0)
                },
                caption: Some(format!(
                    "7-bit I2C address. Default is the model's usual one (0x{:X}).",
                    m::display_default_address(b.index)
                )),
                enabled: true,
            },
        ));
        let st = &cs.display_status;
        let (text, tone) = match st.init_state {
            2 => ("Running", StatusTone::Ok),
            1 => ("Starting up", StatusTone::Neutral),
            3 => ("Not responding", StatusTone::Warning),
            _ => ("Not started", StatusTone::Neutral),
        };
        rows.push((
            None,
            Row::Pill {
                label: "Panel State".into(),
                text: text.into(),
                tone,
                caption: Some(if st.nak_count > 0 {
                    format!(
                        "{} I2C error{} so far. Check wiring, pull-up resistors, and the address.",
                        st.nak_count,
                        if st.nak_count == 1 { "" } else { "s" }
                    )
                } else {
                    "Reported by the device.".into()
                }),
            },
        ));

        rows.push((None, Row::section("BEHAVIOR")));
        rows.push((
            Some(Item::DispMode),
            Row::Pick {
                label: "Idle Behavior".into(),
                choices: vec![
                    "One page".into(),
                    "Cycle Dashboard".into(),
                    "Cycle All".into(),
                ],
                selected: (cfg.mode as usize).min(2),
                caption: Some("What the panel rests on between changes.".into()),
                enabled: true,
            },
        ));
        if cfg.mode == 0 {
            let pages: Vec<usize> = (0..cs.max_pages as usize).collect();
            rows.push((
                Some(Item::DispHome),
                Row::Pick {
                    label: "Page".into(),
                    choices: pages.iter().map(|i| self.page_menu_label(cx, *i)).collect(),
                    selected: (cfg.home_page as usize).min(pages.len().saturating_sub(1)),
                    caption: Some("Which page rests on screen.".into()),
                    enabled: true,
                },
            ));
        } else {
            rows.push((
                Some(Item::DispDwell),
                Row::Number {
                    label: "Cycle Every".into(),
                    value: cfg.dwell as f64 / 10.0,
                    min: m::DISPLAY_MIN_DWELL as f64 / 10.0,
                    max: 655.0,
                    step: 1.0,
                    unit: "s".into(),
                    decimals: 1,
                    caption: Some("How long each page stays up.".into()),
                    enabled: true,
                },
            ));
        }
        rows.push((
            Some(Item::DispOverlay),
            Row::Number {
                label: "Pop-Up Hold".into(),
                value: cfg.overlay_hold as f64 / 10.0,
                min: 0.0,
                max: 655.0,
                step: 1.0,
                unit: "s".into(),
                decimals: 1,
                caption: Some(
                    "Duration for which a change remains on-screen. Zero turns pop-ups off.".into(),
                ),
                enabled: true,
            },
        ));
        if cfg.overlay_hold > 0 {
            rows.push((
                Some(Item::DispOverlayAny),
                Row::Toggle {
                    label: "All Changes Pop-Up".into(),
                    on: cfg.flags & display_flags::OVERLAY_ANY != 0,
                    caption: Some(
                        "Show changes made by a knob, button or remote key even if the parameter \
                         doesn't correspond to a dashboard page."
                            .into(),
                    ),
                    enabled: true,
                },
            ));
        }

        rows.push((None, Row::section("EDITING")));
        rows.push((
            Some(Item::DispEditTimeout),
            Row::Number {
                label: "Editing Times Out".into(),
                value: cfg.edit_timeout as f64 / 10.0,
                min: 0.0,
                max: 655.0,
                step: 1.0,
                unit: "s".into(),
                decimals: 1,
                caption: Some(
                    "Disarm editing after this long untouched. Zero leaves it armed until \
                     switched off."
                        .into(),
                ),
                enabled: true,
            },
        ));
        rows.push((
            Some(Item::DispEditGated),
            Row::Toggle {
                label: "Arm Before Editing".into(),
                on: cfg.flags & display_flags::EDIT_GATED != 0,
                caption: Some(
                    "When enabled, an encoder or button browses pages unless Allow Editing is \
                     toggled. When disabled, an encoder or button will always adjust the \
                     displayed value."
                        .into(),
                ),
                enabled: true,
            },
        ));
        // Gated with nothing able to arm it is a dead end the device cannot
        // refuse: both halves are valid on their own, and the control just
        // browses forever. Only says so once a control depends on it.
        if cfg.flags & display_flags::EDIT_GATED != 0
            && self.uses_page_value()
            && !self.can_arm_editing()
        {
            rows.push((
                None,
                Row::warning("Nothing can arm editing", EDIT_GATE_WARNING),
            ));
        }

        rows.push((None, Row::section("APPEARANCE")));
        rows.push((
            Some(Item::DispBright),
            Row::Number {
                label: "Brightness".into(),
                value: cfg.brightness as f64,
                min: 0.0,
                max: 255.0,
                step: 8.0,
                unit: String::new(),
                decimals: 0,
                caption: Some(
                    "OLED contrast, applied when the panel next starts. Zero is the driver \
                     default."
                        .into(),
                ),
                enabled: true,
            },
        ));
        rows.push((
            Some(Item::DispLabelAlign),
            Row::Pick {
                label: "Name Alignment".into(),
                choices: m::ALIGN_NAMES.iter().map(|s| (*s).to_string()).collect(),
                selected: (cfg.label_align() as usize).min(2),
                caption: Some("Horizontal placement of the current page's name.".into()),
                enabled: true,
            },
        ));
        rows.push((
            Some(Item::DispValueAlign),
            Row::Pick {
                label: "Value Alignment".into(),
                choices: m::ALIGN_NAMES.iter().map(|s| (*s).to_string()).collect(),
                selected: (cfg.value_align() as usize).min(2),
                caption: Some("Horizontal placement of the current page's value.".into()),
                enabled: true,
            },
        ));

        rows.extend(self.page_rows(cx, slot));
        rows
    }

    fn page_menu_label(&self, cx: &Cx<'_>, i: usize) -> String {
        match self.live.pages.get(i) {
            Some(p) if p.is_active() => {
                format!(
                    "{}. {}",
                    i + 1,
                    m::display_page_summary(cx.state, &self.live, p)
                )
            }
            _ => format!("Page {} (empty)", i + 1),
        }
    }

    fn page_rows(&self, cx: &Cx<'_>, slot: usize) -> Vec<(Option<Item>, Row)> {
        let cs = &self.live;
        let mut rows: Vec<(Option<Item>, Row)> = Vec::new();
        rows.push((None, Row::section("DASHBOARD PAGES")));
        let nouns = m::display_page_nouns(cs);
        let graphic = m::display_is_graphic(self.drafts[slot].index);
        let active: Vec<usize> = (0..cs.pages.len())
            .filter(|i| cs.pages[*i].is_active())
            .collect();
        for i in active {
            let p = cs.pages[i].clone();
            let shown = cs.display_status.current_page as usize == i;
            rows.push((
                Some(Item::PageNoun(i)),
                Row::Pick {
                    label: format!("{}{} Shows", if shown { "◉" } else { " " }, i + 1),
                    choices: nouns
                        .iter()
                        .map(|n| m::noun_name(*n, m::ty::NONE))
                        .collect(),
                    selected: nouns.iter().position(|n| *n == p.noun).unwrap_or(0),
                    caption: None,
                    enabled: true,
                },
            ));
            if let Some(nd) = cs.noun_desc(p.noun).copied()
                && nd.is_targeted()
            {
                let grouped = p.flags & page_flags::GROUP != 0;
                let usable = m::compatible_groups(cs, p.noun);
                let (choices, selected) =
                    target_choices(cx, cs, p.noun, None, &usable, p.target, grouped);
                rows.push((
                    Some(Item::PageTarget(i)),
                    Row::Pick {
                        label: "  For".into(),
                        choices,
                        selected,
                        caption: None,
                        enabled: true,
                    },
                ));
            }
            if graphic {
                rows.push((
                    Some(Item::PageLarge(i)),
                    Row::Toggle {
                        label: "  Large value".into(),
                        on: p.flags & page_flags::LARGE != 0,
                        caption: None,
                        enabled: true,
                    },
                ));
            }
            let bar_ok = m::page_bar_allowed(cs, p.noun);
            rows.push((
                Some(Item::PageBar(i)),
                Row::Toggle {
                    label: "  Level bar".into(),
                    on: p.flags & page_flags::BAR != 0,
                    caption: Some(if bar_ok {
                        if graphic {
                            "Fills the value's own row behind the text, with no row given up."
                                .to_string()
                        } else {
                            "Draws the bar on the bottom row.".to_string()
                        }
                    } else {
                        "Only a value with a range can be drawn as a bar.".to_string()
                    }),
                    enabled: bar_ok,
                },
            ));
            rows.push((
                Some(Item::PageRemove(i)),
                Row::Buttons {
                    label: String::new(),
                    caption: None,
                    buttons: vec!["Remove page".into()],
                    cursor: self.page_button.get(&i).copied().unwrap_or(0),
                    enabled: true,
                },
            ));
        }
        let free = (0..cs.pages.len()).find(|i| !cs.pages[*i].is_active());
        rows.push((
            Some(Item::PageAdd),
            Row::Buttons {
                label: if free.is_none() {
                    format!("All {} page slots are in use.", cs.pages.len())
                } else {
                    String::new()
                },
                caption: None,
                buttons: vec!["Add Page".into()],
                cursor: 0,
                enabled: free.is_some() && cx.connected,
            },
        ));
        rows
    }

    // ----------------------------------------------------------- the card

    fn card(&self, cx: &Cx<'_>, slot: usize) -> Vec<(Option<Item>, Row)> {
        let b = self.drafts[slot].clone();
        let cs = &self.live;
        let expanded = self.expanded.contains(&slot);
        let dirty = self.dirty(slot);
        let mut rows: Vec<(Option<Item>, Row)> = Vec::new();
        let name = if self.names[slot].is_empty() {
            m::type_name(b.component)
        } else {
            self.names[slot].clone()
        };
        let (pill, tone) = if dirty {
            ("Pending", StatusTone::Warning)
        } else if cs.status.is_slot_active(slot as u8) {
            ("Active", StatusTone::Ok)
        } else {
            ("Inactive", StatusTone::Warning)
        };
        let summary = if self.names[slot].is_empty() {
            m::verb_phrase(cx.state, cs, &b)
        } else {
            format!(
                "{} - {}",
                m::type_name(b.component),
                m::verb_phrase(cx.state, cs, &b)
            )
        };
        rows.push((
            None,
            Row::Pill {
                label: format!("{} {name}", if expanded { "▾" } else { "▸" }),
                text: pill.into(),
                tone,
                caption: Some(summary),
            },
        ));
        rows.push((
            Some(Item::Header(slot)),
            Row::Buttons {
                label: String::new(),
                caption: None,
                buttons: vec![
                    if expanded { "Collapse" } else { "Expand" }.into(),
                    "Rename".into(),
                    "Remove".into(),
                ],
                cursor: self.header_button.get(&slot).copied().unwrap_or(0),
                enabled: true,
            },
        ));

        if expanded && !b.is_empty() {
            if !cs.binding(slot).is_empty() && !cs.status.is_slot_active(slot as u8) {
                rows.push((
                    None,
                    Row::Status(m::inactive_reason(cs.status.slot_health(slot as u8)), true),
                ));
            }
            let types = self.type_options(slot);
            rows.push((
                Some(Item::Type(slot)),
                Row::Pick {
                    label: "Component".into(),
                    choices: types.iter().map(|t| m::type_name(*t)).collect(),
                    selected: types.iter().position(|t| *t == b.component).unwrap_or(0),
                    caption: Some("What is wired to the GPIO.".into()),
                    enabled: true,
                },
            ));
            if b.component == m::ty::IR {
                rows.extend(self.pin_rows(cx, slot));
                rows.push((
                    Some(Item::Flag(slot, m::flag::INVERT)),
                    Row::Toggle {
                        label: m::invert_title(m::ty::IR).into(),
                        on: b.flags & m::flag::INVERT != 0,
                        caption: Some(m::invert_detail(m::ty::IR).into()),
                        enabled: true,
                    },
                ));
                rows.extend(self.ir_section(cx, slot));
            } else if b.component == m::ty::DISPLAY {
                rows.extend(self.display_rows(cx, slot));
            } else if m::is_aux(b.component) {
                rows.extend(self.aux_rows(cx, slot));
            } else {
                let nouns = m::noun_choices(cs, b.component);
                rows.push((
                    Some(Item::Noun(slot)),
                    Row::Pick {
                        label: "Controls".into(),
                        choices: nouns
                            .iter()
                            .map(|(cat, n)| m::noun_choice_label(cat, *n, b.component))
                            .collect(),
                        selected: nouns.iter().position(|(_, n)| *n == b.noun).unwrap_or(0),
                        caption: Some("The device function this control drives.".into()),
                        enabled: true,
                    },
                ));
                rows.extend(self.target_rows(cx, slot));
                let acts = m::valid_actions(cs, b.component, b.noun);
                if acts.len() > 1 {
                    let (title, detail) = if m::is_indicator(b.component) {
                        ("Indicates", "How the LED reflects the function.")
                    } else if b.component == m::ty::BUTTON {
                        ("On Press", "What a press does.")
                    } else {
                        ("Behavior", "How this control drives the function.")
                    };
                    let is_enum = m::operand_kind(cs, b.noun) == m::kind::ENUM;
                    rows.push((
                        Some(Item::ActionPick(slot)),
                        Row::Pick {
                            label: title.into(),
                            choices: acts
                                .iter()
                                .map(|a| m::action_name(*a, b.noun, is_enum))
                                .collect(),
                            selected: acts.iter().position(|a| *a == b.action).unwrap_or(0),
                            caption: Some(detail.into()),
                            enabled: true,
                        },
                    ));
                }
                if b.component == m::ty::BUTTON {
                    rows.push((
                        Some(Item::Event(slot)),
                        Row::Pick {
                            label: "Gesture".into(),
                            choices: m::EVENT_NAMES.iter().map(|s| (*s).to_string()).collect(),
                            selected: (b.event as usize).min(2),
                            caption: Some(
                                "Which press gesture triggers this. Bind several to one button \
                                 GPIO for multiple functions."
                                    .into(),
                            ),
                            enabled: true,
                        },
                    ));
                }
                rows.extend(self.pin_rows(cx, slot));
                rows.extend(self.operand_rows(cx, slot));
                rows.extend(self.flag_rows(slot));
            }
        }

        if expanded || dirty || self.messages.contains_key(&slot) {
            if let Some((text, err)) = self.messages.get(&slot)
                && !dirty
            {
                rows.push((None, Row::Status(text.clone(), *err)));
            }
            rows.push((
                Some(Item::Apply(slot)),
                Row::Buttons {
                    label: String::new(),
                    caption: None,
                    buttons: vec!["Revert".into(), "Apply".into()],
                    cursor: self.apply_button.get(&slot).copied().unwrap_or(1),
                    enabled: dirty && self.applying != Some(slot),
                },
            ));
        }
        rows.push((None, Row::Blank));
        rows
    }

    fn build(&self, cx: &Cx<'_>) -> Vec<(Option<Item>, Row)> {
        if cx.data.cs.is_none() {
            return m::placeholder_rows(cx)
                .into_iter()
                .map(|r| (None, r))
                .collect();
        }
        let mut rows: Vec<(Option<Item>, Row)> = Vec::new();
        let visible = self.visible();
        if visible.is_empty() {
            rows.push((
                None,
                if self.aux_page {
                    m::empty_state(aux_outputs::EMPTY_TITLE, aux_outputs::EMPTY_BODY)
                } else {
                    m::empty_state(EMPTY_TITLE, EMPTY_BODY)
                },
            ));
            rows.push((None, Row::Blank));
        } else {
            for slot in visible {
                rows.extend(self.card(cx, slot));
            }
        }
        rows.push((
            Some(Item::Add),
            Row::Buttons {
                label: if self.first_free().is_none() {
                    format!("All {} control slots are in use.", self.slot_count())
                } else {
                    String::new()
                },
                caption: None,
                buttons: vec![
                    if self.aux_page {
                        aux_outputs::ADD
                    } else {
                        "Add Control"
                    }
                    .into(),
                ],
                cursor: self.add_button,
                enabled: self.first_free().is_some()
                    && cx.connected
                    && !self.addable_types().is_empty(),
            },
        ));
        rows.push((None, Row::Blank));
        if self.aux_page {
            rows.extend(
                aux_outputs::FOOTER
                    .iter()
                    .flat_map(|p| [(None, Row::note(*p)), (None, Row::Blank)]),
            );
            rows.pop();
        } else {
            rows.push((None, Row::note(FOOTER)));
        }
        if self.live.caps.caps_version != 0 {
            rows.push((
                None,
                Row::note(format!(
                    "Control-surface capability version {}.",
                    self.live.caps.caps_version
                )),
            ));
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

    // ------------------------------------------------------------ the writes

    /// The receiver's Apply pushes the binding, the staged name and every dirty
    /// remote button, stopping at the first refusal.
    fn apply(&mut self, slot: usize) -> PageEvent {
        let mut binding = self.drafts[slot].clone();
        // Byte 22 is an aux output's power-on flags and must be 0 on every
        // other type (control_surfaces.h:468-469). It rides along untouched
        // on an aux slot, whichever page edited it.
        if !m::is_aux(binding.component) {
            binding.extras = 0;
        }
        let binding_changed = binding != self.live.binding(slot);
        let name = (self.names[slot] != self.live.name(slot)).then(|| self.names[slot].clone());
        let ir: Vec<(u8, IrCommand)> = if binding.component == m::ty::IR {
            (0..self.ir_drafts.len())
                .filter(|s| {
                    let (d, live) = (&self.ir_drafts[*s], &self.live.ir[*s]);
                    d != live && (!d.is_empty() || !live.is_empty())
                })
                .map(|s| (s as u8, self.ir_drafts[s].clone()))
                .collect()
        } else {
            Vec::new()
        };
        self.applying = Some(slot);
        self.messages.remove(&slot);
        PageEvent::Session(SessionRequest::new(slot as u32, move |session| {
            let slot = slot as u8;
            let binding = binding.clone();
            let name = name.clone();
            let ir = ir.clone();
            m::run(session, move |s| {
                let mut st = if binding_changed {
                    s.write_binding(slot, &binding)?
                } else {
                    s.read_status()?
                };
                if st.last_status != dspi_session::surfaces::status::SUCCESS {
                    return Ok(st);
                }
                if let Some(n) = name {
                    st = s.write_name(slot, &n)?;
                    if st.last_status != dspi_session::surfaces::status::SUCCESS {
                        return Ok(st);
                    }
                }
                for (sub, cmd) in ir {
                    st = s.write_ir_command(sub, &cmd)?;
                    if st.last_status != dspi_session::surfaces::status::SUCCESS {
                        return Ok(st);
                    }
                }
                Ok(st)
            })
        }))
    }

    /// A new control is applied the moment it is added: seeded with sensible
    /// defaults it is already valid, so it should work straight away. Only
    /// later edits are staged behind Apply.
    fn add_control(&mut self, cx: &Cx<'_>, t: u8) -> PageEvent {
        let Some(slot) = self.first_free() else {
            return PageEvent::Handled;
        };
        self.drafts[slot] = m::make_binding(cx, &self.live, &self.drafts, slot, t);
        self.expanded.insert(slot);
        self.messages.remove(&slot);
        self.apply(slot)
    }

    fn remove(&mut self, slot: usize) -> PageEvent {
        self.messages.remove(&slot);
        self.expanded.remove(&slot);
        let was_live = !self.live.binding(slot).is_empty();
        self.drafts[slot] = CsBinding::default();
        self.names[slot] = String::new();
        if !was_live {
            return PageEvent::Handled;
        }
        self.apply(slot)
    }

    fn write_display_cfg(&mut self, cfg: CsDisplayCfg) -> PageEvent {
        self.live.display_cfg = cfg.clone();
        self.display_status = None;
        PageEvent::Session(SessionRequest::new(TAG_DISPLAY_CFG, move |session| {
            let cfg = cfg.clone();
            m::run(session, move |s| s.write_display_cfg(&cfg))
        }))
    }

    fn write_page(&mut self, index: usize, page: CsDisplayPage) -> PageEvent {
        if index < self.live.pages.len() {
            self.live.pages[index] = page.clone();
        }
        self.display_status = None;
        PageEvent::Session(SessionRequest::new(
            TAG_DISPLAY_PAGE | index as u32,
            move |session| {
                let page = page.clone();
                m::run(session, move |s| s.write_display_page(index as u8, &page))
            },
        ))
    }
}

impl SettingsPage for SurfacesPage {
    fn rows(&self, cx: &Cx<'_>) -> Vec<Row> {
        self.build(cx).into_iter().map(|(_, r)| r).collect()
    }

    fn observe(&mut self, cx: &Cx<'_>) {
        if let Some(cs) = cx.data.cs.as_ref() {
            self.adopt(cs);
        }
        self.observe_learn(cx);
        self.observe_aux(cx);
    }

    fn act(&mut self, index: usize, action: Action, cx: &Cx<'_>) -> PageEvent {
        let Some(item) = self.item(index, cx) else {
            return PageEvent::Handled;
        };
        if let Some(ev) = self.act_aux(item, &action) {
            return ev;
        }
        match (item, action) {
            // -------------------------------------------------- the card list
            (Item::Add, Action::Open) => {
                let types = self.addable_types();
                if types.is_empty() {
                    return PageEvent::Handled;
                }
                self.popup = Some(Item::Add);
                PageEvent::Popup(PopupList::new(
                    if self.aux_page {
                        aux_outputs::ADD
                    } else {
                        "Add Control"
                    },
                    types.iter().map(|t| m::type_name(*t)).collect(),
                    0,
                ))
            }
            (Item::Header(slot), Action::Selected(b)) => {
                self.header_button.insert(slot, b.min(2));
                PageEvent::Handled
            }
            (Item::Header(slot), Action::Open) => {
                match self.header_button.get(&slot).copied().unwrap_or(0) {
                    0 => {
                        if !self.expanded.remove(&slot) {
                            self.expanded.insert(slot);
                        }
                        PageEvent::Handled
                    }
                    1 => {
                        self.dialog = Some(item);
                        let (title, body) = if self.aux_page {
                            (
                                "Rename Output",
                                "A label for this output, stored on the device.",
                            )
                        } else {
                            (
                                "Rename Control",
                                "A label for this control, stored on the device.",
                            )
                        };
                        PageEvent::Dialog(Dialog::text(
                            title,
                            body,
                            self.names[slot].clone(),
                            m::type_name(self.drafts[slot].component),
                        ))
                    }
                    _ => {
                        self.dialog = Some(item);
                        PageEvent::Dialog(Dialog::confirm(
                            if self.aux_page {
                                "Remove Output?"
                            } else {
                                "Remove Control?"
                            },
                            "The slot is cleared on the device and its GPIOs are released.",
                            vec![Button::destructive("Remove"), Button::new("Cancel")],
                        ))
                    }
                }
            }
            (Item::Apply(slot), Action::Selected(b)) => {
                self.apply_button.insert(slot, b.min(1));
                PageEvent::Handled
            }
            (Item::Apply(slot), Action::Open) => {
                if self.apply_button.get(&slot).copied().unwrap_or(1) == 0 {
                    self.drafts[slot] = self.live.binding(slot);
                    self.names[slot] = self.live.name(slot);
                    if self.drafts[slot].component == m::ty::IR {
                        self.ir_drafts = self.live.ir.clone();
                        self.ir_messages.clear();
                    }
                    self.messages.remove(&slot);
                    return PageEvent::Status(
                        if self.aux_page {
                            "Output changes reverted"
                        } else {
                            "Control changes reverted"
                        }
                        .into(),
                    );
                }
                self.apply(slot)
            }

            // ------------------------------------------------- the binding
            (Item::Type(slot), Action::Selected(c)) => {
                let types = self.type_options(slot);
                if let Some(t) = types.get(c).copied()
                    && t != self.drafts[slot].component
                {
                    let old = &self.drafts[slot];
                    self.drafts[slot] = if m::is_aux(old.component) && m::is_aux(t) {
                        // The other kind of aux output keeps its wiring and
                        // its power-on switch.
                        m::switch_aux_kind(old, t)
                    } else {
                        m::make_binding(cx, &self.live, &self.drafts, slot, t)
                    };
                }
                PageEvent::Handled
            }
            (Item::Noun(slot), Action::Selected(c)) => {
                let nouns = m::noun_choices(&self.live, self.drafts[slot].component);
                if let Some((_, n)) = nouns.get(c).copied()
                    && n != self.drafts[slot].noun
                {
                    self.drafts[slot] =
                        m::set_noun_in(&self.live, &self.drafts[slot], n, Some(slot));
                }
                PageEvent::Handled
            }
            (Item::ActionPick(slot), Action::Selected(c)) => {
                let b = self.drafts[slot].clone();
                let acts = m::valid_actions(&self.live, b.component, b.noun);
                if let Some(a) = acts.get(c).copied()
                    && a != b.action
                {
                    let mut nb = b;
                    nb.action = a;
                    self.drafts[slot] = m::default_operands(&self.live, &nb);
                }
                PageEvent::Handled
            }
            (Item::Event(slot), Action::Selected(c)) => {
                self.drafts[slot].event = c.min(2) as u8;
                PageEvent::Handled
            }
            (Item::Target(slot), Action::Selected(c)) => {
                let b = self.drafts[slot].clone();
                let Some(nd) = self.live.noun_desc(b.noun).copied() else {
                    return PageEvent::Handled;
                };
                let grouped = b.flags & m::flag::GROUP != 0;
                let usable = if b.action == m::act::TRIGGER {
                    Vec::new()
                } else {
                    m::compatible_groups(&self.live, b.noun)
                };
                let (is_group, t) = target_choice(
                    &self.live,
                    b.noun,
                    Some(slot),
                    &usable,
                    b.target,
                    grouped,
                    c,
                );
                let mut nb = b;
                if is_group {
                    nb.flags |= m::flag::GROUP;
                } else {
                    // The two group modifiers exist only while the binding
                    // addresses a group; left set, the whole binding is
                    // rejected.
                    nb.flags &= !(m::flag::GROUP | m::flag::LINK_ABS | m::flag::GROUP_ALL);
                }
                nb.target = t;
                if nd.has_band() {
                    let opts = m::band_options(&self.live, cx.state, nb.noun, nb.target, is_group);
                    if !opts.contains(&nb.index) {
                        nb.index = opts.first().copied().unwrap_or(0);
                    }
                }
                self.drafts[slot] = nb;
                PageEvent::Handled
            }
            (Item::Band(slot), Action::Selected(c)) => {
                let b = &self.drafts[slot];
                let bands = m::band_options(
                    &self.live,
                    cx.state,
                    b.noun,
                    b.target,
                    b.flags & m::flag::GROUP != 0,
                );
                if let Some(x) = bands.get(c) {
                    self.drafts[slot].index = *x;
                }
                PageEvent::Handled
            }
            (Item::Pin(slot, second), Action::Selected(c)) => {
                let pins = m::pin_candidates(cx, &self.live, &self.drafts, slot, second);
                let current = if second {
                    self.drafts[slot].gpio[1]
                } else {
                    self.drafts[slot].gpio[0]
                };
                // The row keeps the current pin visible at the head when
                // nothing else would offer it, so the index shifts by one.
                let offset = usize::from(!pins.contains(&current));
                if let Some(p) = pins.get(c.saturating_sub(offset)) {
                    if second {
                        self.drafts[slot].gpio[1] = *p;
                    } else {
                        self.drafts[slot].gpio[0] = *p;
                    }
                }
                PageEvent::Handled
            }
            (Item::SpanOn(slot), Action::Toggled(on)) => {
                let nd = self
                    .live
                    .noun_desc(self.drafts[slot].noun)
                    .copied()
                    .unwrap_or_default();
                if on {
                    self.drafts[slot].range_min = nd.min_q;
                    self.drafts[slot].range_max = nd.max_q;
                } else {
                    self.drafts[slot].range_min = 0;
                    self.drafts[slot].range_max = 0;
                }
                PageEvent::Handled
            }
            (Item::SpanMin(slot), Action::Changed(v) | Action::Committed(v)) => {
                let u = m::noun_unit(&self.live, self.drafts[slot].noun);
                let (lo, hi) = m::noun_range(&self.live, self.drafts[slot].noun);
                self.drafts[slot].range_min = m::encode_value(v.clamp(lo, hi), u);
                PageEvent::Handled
            }
            (Item::SpanMax(slot), Action::Changed(v) | Action::Committed(v)) => {
                let u = m::noun_unit(&self.live, self.drafts[slot].noun);
                let (lo, hi) = m::noun_range(&self.live, self.drafts[slot].noun);
                self.drafts[slot].range_max = m::encode_value(v.clamp(lo, hi), u);
                PageEvent::Handled
            }
            (Item::Value(slot), Action::Changed(v) | Action::Committed(v)) => {
                let u = m::noun_unit(&self.live, self.drafts[slot].noun);
                let (lo, hi) = m::noun_range(&self.live, self.drafts[slot].noun);
                self.drafts[slot].value = m::encode_value(v.clamp(lo, hi), u);
                PageEvent::Handled
            }
            (Item::StepSize(slot), Action::Changed(v) | Action::Committed(v)) => {
                let u = m::noun_unit(&self.live, self.drafts[slot].noun);
                let min = if m::unit_is_log(u) {
                    1.0 / 48.0
                } else {
                    m::unit_min_step(u)
                };
                self.drafts[slot].step = m::encode_step(v.max(min), u);
                PageEvent::Handled
            }
            (Item::EnumStep(slot), Action::Changed(v) | Action::Committed(v)) => {
                self.drafts[slot].step = v.round().max(1.0) as i16;
                PageEvent::Handled
            }
            (Item::Choice(slot), Action::Selected(c)) => {
                let k = m::operand_kind(&self.live, self.drafts[slot].noun);
                self.drafts[slot].value = if k == m::kind::BOOL {
                    i16::from(c != 0)
                } else {
                    c as i16
                };
                PageEvent::Handled
            }
            (Item::Bright(slot), Action::Changed(v) | Action::Committed(v)) => {
                self.drafts[slot].base_bright =
                    v.round().clamp(1.0, m::LED_BRIGHT_MAX as f64) as u8;
                PageEvent::Handled
            }
            (Item::OnDelay(slot), Action::Changed(v) | Action::Committed(v)) => {
                self.drafts[slot].on_delay = m::encode_delay(v.max(0.0) as u32);
                PageEvent::Handled
            }
            (Item::OffDelay(slot), Action::Changed(v) | Action::Committed(v)) => {
                self.drafts[slot].off_delay = m::encode_delay(v.max(0.0) as u32);
                PageEvent::Handled
            }
            (Item::Flag(slot, mask), Action::Toggled(on)) => {
                if on {
                    self.drafts[slot].flags |= mask;
                } else {
                    self.drafts[slot].flags &= !mask;
                }
                PageEvent::Handled
            }

            // ------------------------------------------------------------ IR
            (Item::IrAdd, Action::Open) => {
                let Some(sub) = self.ir_first_free() else {
                    return PageEvent::Handled;
                };
                let n = m::valid_nouns(&self.live, m::ty::IR)
                    .first()
                    .copied()
                    .unwrap_or(m::noun::USER_VOLUME);
                let mut c = IrCommand {
                    noun: n,
                    action: m::default_action(&self.live, m::ty::IR, n),
                    ..Default::default()
                };
                c = default_ir_operands(&self.live, &c);
                self.ir_drafts[sub] = c;
                self.expanded_ir.insert(sub);
                self.ir_messages.remove(&sub);
                PageEvent::Handled
            }
            (Item::IrHeader(sub), Action::Selected(b)) => {
                self.ir_button.insert(sub, b.min(2));
                PageEvent::Handled
            }
            (Item::IrHeader(sub), Action::Open) => {
                match self.ir_button.get(&sub).copied().unwrap_or(0) {
                    0 => {
                        if !self.expanded_ir.remove(&sub) {
                            self.expanded_ir.insert(sub);
                        }
                        PageEvent::Handled
                    }
                    1 => {
                        let slot = m::container_slot(&self.drafts, &self.live, m::ty::IR);
                        if !slot.is_some_and(|s| self.live.status.is_slot_active(s as u8)) {
                            return PageEvent::Status(
                                "No IR receiver is active - apply the receiver first.".into(),
                            );
                        }
                        self.learning = Some((sub, cx.state.ir_learn));
                        self.ir_messages.remove(&sub);
                        PageEvent::Session(SessionRequest::new(
                            TAG_LEARN | sub as u32,
                            // The arm answers PIN_CONFIG_SUCCESS or
                            // CS_STATUS_NO_IR (control_surfaces.h:798), so a
                            // refusal says so rather than waiting for a button
                            // the device is not listening for.
                            move |session| m::run_code(session, LEARN_PROMPT, |s| s.arm_learn()),
                        ))
                    }
                    _ => {
                        self.ir_messages.remove(&sub);
                        self.expanded_ir.remove(&sub);
                        let was_live = !self.live.ir[sub].is_empty();
                        self.ir_drafts[sub] = IrCommand::default();
                        if !was_live {
                            return PageEvent::Handled;
                        }
                        self.live.ir[sub] = IrCommand::default();
                        PageEvent::Session(SessionRequest::new(
                            TAG_IR_CLEAR | sub as u32,
                            move |session| {
                                m::run(session, move |s| {
                                    s.write_ir_command(sub as u8, &IrCommand::default())
                                })
                            },
                        ))
                    }
                }
            }
            (Item::LearnCancel, Action::Open) => {
                let sub = self.learning.take().map(|(s, _)| s);
                if let Some(sub) = sub {
                    self.ir_messages
                        .insert(sub, ("Learn cancelled.".into(), false));
                }
                PageEvent::Session(SessionRequest::new(TAG_LEARN_CANCEL, |session| {
                    m::run_code(session, "Learn cancelled", |s| s.cancel_learn())
                }))
            }
            (Item::IrNoun(sub), Action::Selected(c)) => {
                let nouns = m::noun_choices(&self.live, m::ty::IR);
                if let Some((_, n)) = nouns.get(c).copied()
                    && n != self.ir_drafts[sub].noun
                {
                    let mut cmd = self.ir_drafts[sub].clone();
                    cmd.noun = n;
                    cmd.target = 0;
                    cmd.index = 0;
                    // An aux noun needs an aux output, never slot 0 by default,
                    // and takes no group.
                    if self
                        .live
                        .noun_desc(n)
                        .is_some_and(|d| d.target_kind == m::target::AUX)
                    {
                        cmd.flags &= !m::flag::GROUP;
                        cmd.target = m::target_addresses(&self.live, n, None)
                            .first()
                            .copied()
                            .unwrap_or(0);
                    }
                    let acts = m::valid_actions(&self.live, m::ty::IR, n);
                    if !acts.contains(&cmd.action) {
                        cmd.action = m::default_action(&self.live, m::ty::IR, n);
                    }
                    self.ir_drafts[sub] = default_ir_operands(&self.live, &cmd);
                }
                PageEvent::Handled
            }
            (Item::IrAction(sub), Action::Selected(c)) => {
                let acts = m::valid_actions(&self.live, m::ty::IR, self.ir_drafts[sub].noun);
                if let Some(a) = acts.get(c).copied()
                    && a != self.ir_drafts[sub].action
                {
                    let mut cmd = self.ir_drafts[sub].clone();
                    cmd.action = a;
                    self.ir_drafts[sub] = default_ir_operands(&self.live, &cmd);
                }
                PageEvent::Handled
            }
            (Item::IrTarget(sub), Action::Selected(c)) => {
                let cmd = self.ir_drafts[sub].clone();
                let Some(nd) = self.live.noun_desc(cmd.noun).copied() else {
                    return PageEvent::Handled;
                };
                let grouped = cmd.flags & m::flag::GROUP != 0;
                let usable = m::compatible_groups(&self.live, cmd.noun);
                let (is_group, t) =
                    target_choice(&self.live, cmd.noun, None, &usable, cmd.target, grouped, c);
                let mut cmd = cmd;
                if is_group {
                    cmd.flags |= m::flag::GROUP;
                } else {
                    cmd.flags &= !m::flag::GROUP;
                }
                cmd.target = t;
                if nd.has_band() {
                    let opts =
                        m::band_options(&self.live, cx.state, cmd.noun, cmd.target, is_group);
                    if !opts.contains(&cmd.index) {
                        cmd.index = opts.first().copied().unwrap_or(0);
                    }
                }
                self.ir_drafts[sub] = cmd;
                PageEvent::Handled
            }
            (Item::IrBand(sub), Action::Selected(c)) => {
                let cmd = &self.ir_drafts[sub];
                let bands = m::band_options(
                    &self.live,
                    cx.state,
                    cmd.noun,
                    cmd.target,
                    cmd.flags & m::flag::GROUP != 0,
                );
                if let Some(x) = bands.get(c) {
                    self.ir_drafts[sub].index = *x;
                }
                PageEvent::Handled
            }
            (Item::IrValue(sub), Action::Changed(v) | Action::Committed(v)) => {
                let u = m::noun_unit(&self.live, self.ir_drafts[sub].noun);
                let (lo, hi) = m::noun_range(&self.live, self.ir_drafts[sub].noun);
                self.ir_drafts[sub].value = m::encode_value(v.clamp(lo, hi), u);
                PageEvent::Handled
            }
            (Item::IrStep(sub), Action::Changed(v) | Action::Committed(v)) => {
                let u = m::noun_unit(&self.live, self.ir_drafts[sub].noun);
                let min = if m::unit_is_log(u) {
                    1.0 / 48.0
                } else {
                    m::unit_min_step(u)
                };
                self.ir_drafts[sub].step = m::encode_step(v.max(min), u);
                PageEvent::Handled
            }
            (Item::IrEnumStep(sub), Action::Changed(v) | Action::Committed(v)) => {
                self.ir_drafts[sub].step = v.round().max(1.0) as i16;
                PageEvent::Handled
            }
            (Item::IrChoice(sub), Action::Selected(c)) => {
                let k = m::operand_kind(&self.live, self.ir_drafts[sub].noun);
                self.ir_drafts[sub].value = if k == m::kind::BOOL {
                    i16::from(c != 0)
                } else {
                    c as i16
                };
                PageEvent::Handled
            }
            (Item::IrFlag(sub, mask), Action::Toggled(on)) => {
                if on {
                    self.ir_drafts[sub].flags |= mask;
                } else {
                    self.ir_drafts[sub].flags &= !mask;
                }
                PageEvent::Handled
            }

            // ------------------------------------------------------- display
            (Item::DispModel(slot), Action::Selected(c)) => {
                let models: Vec<u8> = (1..=self.live.caps.display_models.max(1)).collect();
                if let Some(model) = models.get(c).copied() {
                    let b = &mut self.drafts[slot];
                    // The address stores 0 for "the model's usual one", so a
                    // model change carries that convention rather than
                    // stranding the previous model's default.
                    if b.value != 0 && b.value as u8 == m::display_default_address(b.index) {
                        b.value = 0;
                    }
                    b.index = model;
                }
                PageEvent::Handled
            }
            (Item::DispPins(slot), Action::Selected(c)) => {
                let sdas = m::i2c_sda_candidates(cx, &self.drafts, slot);
                let offset = usize::from(!sdas.contains(&self.drafts[slot].gpio[0]));
                if let Some(sda) = sdas.get(c.saturating_sub(offset)).copied() {
                    self.drafts[slot].gpio = [sda, sda.wrapping_add(1)];
                }
                PageEvent::Handled
            }
            (Item::DispAddress(slot), Action::Selected(c)) => {
                self.drafts[slot].value = if c == 0 {
                    0
                } else {
                    m::DISPLAY_ADDRESSES.get(c - 1).copied().unwrap_or(0)
                };
                PageEvent::Handled
            }
            (Item::DispMode, Action::Selected(c)) => {
                let mut cfg = self.live.display_cfg.clone();
                cfg.mode = c.min(2) as u8;
                // The firmware refuses a dwell under its floor in either cycle
                // mode, so entering one from FIXED has to lift it.
                if cfg.mode != 0 {
                    cfg.dwell = cfg.dwell.max(m::DISPLAY_MIN_DWELL);
                }
                self.write_display_cfg(cfg)
            }
            (Item::DispHome, Action::Selected(c)) => {
                let mut cfg = self.live.display_cfg.clone();
                cfg.home_page = c as u8;
                self.write_display_cfg(cfg)
            }
            (Item::DispDwell, Action::Committed(v) | Action::Changed(v)) => {
                let mut cfg = self.live.display_cfg.clone();
                cfg.dwell =
                    ((v * 10.0).round().clamp(0.0, 6553.0) as u16).max(m::DISPLAY_MIN_DWELL);
                self.write_display_cfg(cfg)
            }
            (Item::DispOverlay, Action::Committed(v) | Action::Changed(v)) => {
                let mut cfg = self.live.display_cfg.clone();
                cfg.overlay_hold = (v * 10.0).round().clamp(0.0, 6553.0) as u16;
                self.write_display_cfg(cfg)
            }
            (Item::DispEditTimeout, Action::Committed(v) | Action::Changed(v)) => {
                let mut cfg = self.live.display_cfg.clone();
                cfg.edit_timeout = (v * 10.0).round().clamp(0.0, 6553.0) as u16;
                self.write_display_cfg(cfg)
            }
            (Item::DispBright, Action::Committed(v) | Action::Changed(v)) => {
                let mut cfg = self.live.display_cfg.clone();
                cfg.brightness = v.round().clamp(0.0, 255.0) as u8;
                self.write_display_cfg(cfg)
            }
            (Item::DispOverlayAny, Action::Toggled(on)) => {
                let mut cfg = self.live.display_cfg.clone();
                set_flag(&mut cfg.flags, display_flags::OVERLAY_ANY, on);
                self.write_display_cfg(cfg)
            }
            (Item::DispEditGated, Action::Toggled(on)) => {
                let mut cfg = self.live.display_cfg.clone();
                set_flag(&mut cfg.flags, display_flags::EDIT_GATED, on);
                self.write_display_cfg(cfg)
            }
            (Item::DispLabelAlign, Action::Selected(c)) => {
                let mut cfg = self.live.display_cfg.clone();
                cfg.set_label_align(c.min(2) as u8);
                self.write_display_cfg(cfg)
            }
            (Item::DispValueAlign, Action::Selected(c)) => {
                let mut cfg = self.live.display_cfg.clone();
                cfg.set_value_align(c.min(2) as u8);
                self.write_display_cfg(cfg)
            }
            (Item::PageAdd, Action::Open) => {
                let Some(i) = (0..self.live.pages.len()).find(|i| !self.live.pages[*i].is_active())
                else {
                    return PageEvent::Handled;
                };
                let page = CsDisplayPage {
                    noun: m::display_page_nouns(&self.live)
                        .first()
                        .copied()
                        .unwrap_or(m::noun::USER_VOLUME),
                    target: 0,
                    index: 0,
                    flags: page_flags::ACTIVE,
                };
                self.write_page(i, page)
            }
            (Item::PageNoun(i), Action::Selected(c)) => {
                let nouns = m::display_page_nouns(&self.live);
                let Some(n) = nouns.get(c).copied() else {
                    return PageEvent::Handled;
                };
                let mut p = self.live.pages[i].clone();
                p.noun = n;
                p.target = 0;
                p.index = 0;
                p.flags &= !page_flags::GROUP;
                // A bar needs a range to plot inside; switching to a switch or
                // a mode leaves it with none, and the device rejects the whole
                // page rather than ignoring the flag.
                if !m::page_bar_allowed(&self.live, n) {
                    p.flags &= !page_flags::BAR;
                }
                self.write_page(i, p)
            }
            (Item::PageTarget(i), Action::Selected(c)) => {
                let p = self.live.pages[i].clone();
                if self.live.noun_desc(p.noun).is_none() {
                    return PageEvent::Handled;
                }
                let grouped = p.flags & page_flags::GROUP != 0;
                let usable = m::compatible_groups(&self.live, p.noun);
                let (is_group, t) =
                    target_choice(&self.live, p.noun, None, &usable, p.target, grouped, c);
                let mut p = p;
                set_flag(&mut p.flags, page_flags::GROUP, is_group);
                p.target = t;
                self.write_page(i, p)
            }
            (Item::PageLarge(i), Action::Toggled(on)) => {
                let mut p = self.live.pages[i].clone();
                set_flag(&mut p.flags, page_flags::LARGE, on);
                self.write_page(i, p)
            }
            (Item::PageBar(i), Action::Toggled(on)) => {
                let mut p = self.live.pages[i].clone();
                if on && !m::page_bar_allowed(&self.live, p.noun) {
                    return PageEvent::Status(
                        "Only a value with a range can be drawn as a bar.".into(),
                    );
                }
                set_flag(&mut p.flags, page_flags::BAR, on);
                self.write_page(i, p)
            }
            (Item::PageRemove(i), Action::Open) => self.write_page(i, CsDisplayPage::default()),

            // Every picker opens the same way, from the row's own choices.
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
            // The nth card on screen, the way a digit jumps to the nth band:
            // with controls in slots 1 and 6, `2` opens the second card.
            if let Some(slot) = self.visible().get(d as usize - 1).copied() {
                self.expanded.insert(slot);
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
        if item == Item::Add {
            let types = self.addable_types();
            let Some(t) = types.get(c).copied() else {
                return PageEvent::Handled;
            };
            return self.add_control(cx, t);
        }
        let index = self
            .build(cx)
            .into_iter()
            .filter(|(_, r)| r.focusable())
            .position(|(i, _)| i == Some(item))
            .unwrap_or(0);
        self.act(index, Action::Selected(c), cx)
    }

    fn dialog_result(&mut self, outcome: DialogOutcome, _cx: &Cx<'_>) -> PageEvent {
        let Some(Item::Header(slot)) = self.dialog.take() else {
            return PageEvent::Handled;
        };
        match outcome {
            DialogOutcome::Text(name) => {
                self.names[slot] = name.chars().take(31).collect();
                PageEvent::Handled
            }
            DialogOutcome::Button(0) => self.remove(slot),
            _ => PageEvent::Handled,
        }
    }

    fn session_result(&mut self, tag: u32, reply: SessionReply, _cx: &Cx<'_>) -> PageEvent {
        if is_live_aux_tag(tag) {
            return self.aux_result(tag, reply);
        }
        match tag & TAG_MASK {
            TAG_DISPLAY_CFG | TAG_DISPLAY_PAGE => {
                return match reply {
                    SessionReply::Ok(_) => {
                        self.display_status = None;
                        PageEvent::Handled
                    }
                    SessionReply::Err(why) => {
                        self.display_status = Some(why.clone());
                        PageEvent::Status(why)
                    }
                    SessionReply::Bytes(_) => PageEvent::Handled,
                };
            }
            TAG_LEARN | TAG_LEARN_CANCEL => {
                return match reply {
                    SessionReply::Ok(msg) => PageEvent::Status(msg),
                    SessionReply::Err(why) => {
                        if let Some((sub, _)) = self.learning.take() {
                            self.ir_messages.insert(sub, (why.clone(), true));
                        }
                        PageEvent::Status(why)
                    }
                    SessionReply::Bytes(_) => PageEvent::Handled,
                };
            }
            TAG_IR_CLEAR => {
                return match reply {
                    SessionReply::Err(why) => PageEvent::Status(why),
                    _ => PageEvent::Handled,
                };
            }
            _ => {}
        }
        let slot = tag as usize;
        if slot >= self.slot_count() {
            return PageEvent::Handled;
        }
        self.applying = None;
        match reply {
            SessionReply::Ok(_) => {
                self.live.bindings[slot] = self.drafts[slot].clone();
                self.live.names[slot] = self.names[slot].clone();
                if self.drafts[slot].component == m::ty::IR {
                    self.live.ir = self.ir_drafts.clone();
                    self.ir_messages.clear();
                }
                self.messages.remove(&slot);
                PageEvent::Status(if self.aux_page {
                    format!("{} applied", self.live.aux_name(slot))
                } else {
                    format!("Control {} applied", slot + 1)
                })
            }
            SessionReply::Err(why) => {
                // The rejected draft stays put. Re-seeding from the device
                // would restore an empty slot for a control that was never
                // applied, so the card would vanish and take this message with
                // it, reading as "adding it flashed and did nothing".
                self.messages.insert(slot, (why.clone(), true));
                PageEvent::Status(why)
            }
            SessionReply::Bytes(_) => PageEvent::Handled,
        }
    }
}

fn set_flag(flags: &mut u8, mask: u8, on: bool) {
    if on {
        *flags |= mask;
    } else {
        *flags &= !mask;
    }
}

/// Reset a remote button's value and step for its action and noun kind,
/// leaving the learned protocol, code, target and band alone.
pub fn default_ir_operands(cs: &CsData, c: &IrCommand) -> IrCommand {
    let mut c = c.clone();
    let nd = cs.noun_desc(c.noun).copied().unwrap_or_default();
    let k = m::operand_kind(cs, c.noun);
    c.value = 0;
    c.step = 0;
    c.flags = 0;
    match c.action {
        m::act::INC | m::act::DEC => {
            if k == m::kind::ENUM {
                c.step = 1;
            }
        }
        m::act::SET | m::act::MOMENTARY => {
            if k == m::kind::CONTINUOUS {
                c.value = nd.max_q;
            } else if k == m::kind::BOOL {
                c.value = 1;
            }
        }
        _ => {}
    }
    c
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
        let s = SettingsScreen::new(&st, data, AppConfig::default()).open(Page::Surfaces, &st);
        (s, st)
    }

    fn page() -> (SurfacesPage, SettingsData, DeviceState) {
        let d = m::demo::settings_data();
        (SurfacesPage::new(&d), d, m::demo::state())
    }

    fn cx<'a>(d: &'a SettingsData, st: &'a DeviceState, cfg: &'a AppConfig) -> Cx<'a> {
        Cx {
            state: st,
            data: d,
            config: cfg,
            connected: true,
            global_dirty: false,
        }
    }

    fn index_of(p: &SurfacesPage, cx: &Cx<'_>, want: Item) -> usize {
        p.build(cx)
            .into_iter()
            .filter(|(_, r)| r.focusable())
            .position(|(i, _)| i == Some(want))
            .unwrap_or_else(|| panic!("no row for {want:?}"))
    }

    /// Open the nth card and draw it. The cards start collapsed, so a card
    /// body is only reachable the way a user reaches it.
    fn opened(nth: usize, w: u16, h: u16) -> String {
        let st = m::demo::state();
        let mut s = SettingsScreen::new(&st, m::demo::settings_data(), AppConfig::default())
            .open(Page::Surfaces, &st);
        s.handle(key(KeyCode::Tab), &st);
        for _ in 0..nth {
            s.handle(key(KeyCode::Down), &st);
        }
        s.handle(key(KeyCode::Enter), &st);
        frame(&mut s, &st, w, h)
    }

    // -------------------------------------------------------- golden frames

    #[test]
    fn the_page_lists_its_controls_at_both_sizes() {
        let (mut s, st) = screen(m::demo::settings_data());
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            let f = frame(&mut s, &st, w, h);
            assert_eq!(f.lines().count(), h as usize, "{w}x{h}");
            assert!(f.contains("Volume Knob"), "{w}x{h}:\n{f}");
        }
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("Turn to step Volume."), "the summary line:\n{f}");
        assert!(f.contains("Active"), "the status pill:\n{f}");
    }

    #[test]
    fn an_empty_device_shows_the_consoles_empty_state() {
        let mut d = m::demo::settings_data();
        if let Some(cs) = d.cs.as_mut() {
            cs.bindings = vec![CsBinding::default(); 16];
            cs.status.active_mask = 0;
        }
        let (mut s, st) = screen(d);
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("No Controls Configured"), "{f}");
        assert!(f.contains("spare GPIO"), "{f}");
        assert!(f.contains("Add Control"), "{f}");
    }

    #[test]
    fn a_disconnected_device_says_so_rather_than_inventing_a_page() {
        let mut d = m::demo::settings_data();
        d.cs = None;
        let st = m::demo::state();
        let mut s = SettingsScreen::new(&st, d, AppConfig::default())
            .connected(false)
            .open(Page::Surfaces, &st);
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("No Device Connected"), "{f}");
        assert!(
            f.contains("Control surfaces are stored on the device"),
            "{f}"
        );
    }

    /// One golden frame per card type, in the Console's row order.
    #[test]
    fn an_encoder_card_draws_the_consoles_rows() {
        let f = opened(0, 120, 40);
        for want in [
            "Component",
            "Rotary Encoder",
            "Controls",
            "Volume & Mute / Volume",
            "GPIO A",
            "Encoder channel A.",
            "GPIO B",
            "Step Size",
            "Amount added or removed per detent/press.",
            "Reverse Direction",
            "Acceleration",
            "Pull-Down Wiring",
            "Revert",
            "Apply",
        ] {
            assert!(f.contains(want), "missing {want:?}:\n{f}");
        }
        // A pot's flags are not an encoder's.
        assert!(!f.contains("Repeat While Held"), "{f}");
    }

    #[test]
    fn a_button_card_draws_its_gesture_and_press_rows() {
        let f = opened(1, 120, 40);
        assert!(f.contains("Push Button"), "{f}");
        assert!(f.contains("On Press"), "{f}");
        assert!(f.contains("Gesture"), "{f}");
        assert!(
            f.contains("Bind several to one button GPIO for multiple functions."),
            "{f}"
        );
        assert!(f.contains("Active-High Wiring"), "the invert title:\n{f}");
    }

    #[test]
    fn a_pot_card_draws_its_target_and_its_span() {
        let f = opened(2, 120, 40);
        assert!(f.contains("Potentiometer / Fader"), "{f}");
        assert!(f.contains("Channel"), "{f}");
        assert!(f.contains("Limit Range"), "{f}");
        assert!(f.contains("GPIO 26"), "an ADC pin:\n{f}");
        assert!(f.contains("Reverse Direction"), "{f}");
    }

    #[test]
    fn the_ir_card_nests_its_remote_buttons() {
        let f = opened(3, 120, 40);
        assert!(f.contains("Idle-Low Receiver"), "{f}");
        assert!(f.contains("REMOTE BUTTONS"), "{f}");
        assert!(f.contains("1/16"), "the n/max count:\n{f}");
        assert!(f.contains("NEC 0x20DF40BF"), "the code chip:\n{f}");
        assert!(f.contains("Raise Volume"), "the summary:\n{f}");
        assert!(f.contains("Learn Button") || f.contains("Re-learn"), "{f}");
        assert!(f.contains("Add Remote Button"), "{f}");
    }

    #[test]
    fn the_display_card_draws_its_wiring_config_and_pages() {
        // The display's own three sections and its page list run past a
        // 40-row pane, so this one is drawn tall enough to hold the lot.
        let f = opened(4, 120, 80);
        for want in [
            "WIRING",
            "OLED 128x64 (SSD1306)",
            "SDA/SCL Pins",
            "GPIO 2 / 3",
            "Address",
            "Panel State",
            "BEHAVIOR",
            "Idle Behavior",
            "Cycle Every",
            "Pop-Up Hold",
            "EDITING",
            "Arm Before Editing",
            "APPEARANCE",
            "Brightness",
            "Name Alignment",
            "Value Alignment",
            "DASHBOARD PAGES",
            "Large value",
            "Level bar",
            "Add Page",
        ] {
            assert!(f.contains(want), "missing {want:?}:\n{f}");
        }
    }

    #[test]
    fn the_footer_names_the_capability_version() {
        let (mut s, st) = screen(m::demo::settings_data());
        s.handle(key(KeyCode::Tab), &st);
        for _ in 0..40 {
            s.handle(key(KeyCode::Down), &st);
        }
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("Control-surface capability version 20."), "{f}");
    }

    // ------------------------------------------------ the caps-driven lists

    #[test]
    fn an_unavailable_noun_and_a_disallowed_action_never_appear() {
        let (mut p, d, st) = page();
        let cfg = AppConfig::default();
        let c = cx(&d, &st, &cfg);
        p.expanded.insert(0); // the encoder
        let rows = p.build(&c);
        let noun_row = rows
            .iter()
            .find(|(i, _)| *i == Some(Item::Noun(0)))
            .map(|(_, r)| r.clone())
            .expect("a parameter picker");
        let Row::Pick { choices, .. } = noun_row else {
            panic!("not a picker");
        };
        // ADAT Active has a zero action mask on this platform.
        assert!(
            !choices.iter().any(|x| x.contains("ADAT Active")),
            "{choices:?}"
        );
        // An encoder can only STEP, so Toggle is never offered as its action.
        assert_eq!(
            m::valid_actions(&p.live, m::ty::ENCODER, m::noun::USER_VOLUME),
            vec![m::act::STEP]
        );
    }

    #[test]
    fn a_pot_cannot_pick_a_pin_that_is_not_analogue() {
        let (mut p, d, st) = page();
        let cfg = AppConfig::default();
        let c = cx(&d, &st, &cfg);
        p.expanded.insert(2); // the pot on GPIO 26
        let pins = m::pin_candidates(&c, &p.live, &p.drafts, 2, false);
        assert!(!pins.is_empty());
        assert!(
            pins.iter().all(|x| (26..=28).contains(x)),
            "a potentiometer needs an ADC pin: {pins:?}"
        );
        // And the row offers exactly those.
        let rows = p.build(&c);
        let Some((_, Row::Pick { choices, .. })) =
            rows.iter().find(|(i, _)| *i == Some(Item::Pin(2, false)))
        else {
            panic!("no pin row");
        };
        assert!(
            choices.iter().all(|x| x.starts_with("GPIO 2")),
            "{choices:?}"
        );
    }

    #[test]
    fn an_encoder_offers_two_pins_and_never_the_same_one_twice() {
        let (mut p, d, st) = page();
        let cfg = AppConfig::default();
        let c = cx(&d, &st, &cfg);
        p.expanded.insert(0);
        let a = m::pin_candidates(&c, &p.live, &p.drafts, 0, false);
        let b = m::pin_candidates(&c, &p.live, &p.drafts, 0, true);
        assert!(!a.contains(&p.drafts[0].gpio[1]), "A never offers B's pin");
        assert!(!b.contains(&p.drafts[0].gpio[0]), "B never offers A's pin");
    }

    // ---------------------------------------------------------- the encoding

    /// Byte-exact, mirroring the Console's `ControlSurfacesWireTests`.
    #[test]
    fn a_filled_card_encodes_the_consoles_binding_record() {
        let b = CsBinding {
            component: m::ty::ENCODER,
            noun: m::noun::MASTER_VOLUME,
            action: m::act::STEP,
            flags: m::flag::ACCEL,
            gpio: [27, 28],
            event: 0,
            target: 0,
            index: 0,
            base_bright: 0,
            value: 0,
            step: m::encode_step(1.0, m::unit::DB),
            range_min: 0,
            range_max: 0,
            on_delay: 0,
            off_delay: 0,
            extras: 0,
        };
        let w = b.encode();
        assert_eq!(w.len(), 24);
        assert_eq!(w[0], 4, "type = ENCODER");
        assert_eq!(w[1], 1, "noun = MASTER_VOLUME");
        assert_eq!(w[2], 1, "action = STEP");
        assert_eq!(w[3], 0x08, "flags = ACCEL");
        assert_eq!(&w[4..6], &[27, 28], "gpio");
        assert_eq!(w[9], 0, "base_bright unset on anything but a PWM LED");
        assert_eq!(&w[12..14], &[0x00, 0x01], "step = 1.0 dB in 8.8");
        assert_eq!(&w[22..24], &[0, 0], "reserved");
        assert_eq!(CsBinding::decode(&w).unwrap(), b);
    }

    #[test]
    fn a_learned_remote_button_encodes_the_consoles_ir_record() {
        let c = IrCommand {
            noun: m::noun::USER_VOLUME,
            action: m::act::INC,
            flags: m::flag::REPEAT,
            target: 0,
            index: 0,
            protocol: 1,
            value: 0,
            step: m::encode_step(1.0, m::unit::DB),
            code: 0x20DF_40BF,
        };
        let w = c.encode();
        assert_eq!(w.len(), 16);
        assert_eq!(w[1], 2, "action = INC");
        assert_eq!(w[2], 0x10, "flags = REPEAT");
        assert_eq!(w[5], 1, "protocol = NEC");
        assert_eq!(&w[8..10], &[0x00, 0x01], "step");
        assert_eq!(&w[10..12], &[0, 0], "reserved");
        assert_eq!(&w[12..16], &0x20DF_40BFu32.to_le_bytes(), "code");
        assert_eq!(IrCommand::decode(&w).unwrap(), c);
    }

    #[test]
    fn the_display_config_and_a_page_encode_their_records() {
        let cfg = CsDisplayCfg {
            mode: 1,
            home_page: 2,
            dwell: 50,
            overlay_hold: 20,
            brightness: 128,
            flags: display_flags::OVERLAY_ANY | display_flags::EDIT_GATED,
            edit_timeout: 100,
        };
        assert_eq!(
            cfg.encode().to_vec(),
            vec![
                0x01, 0x02, 0x32, 0x00, 0x14, 0x00, 0x80, 0x03, 0x64, 0x00, 0x00, 0x00
            ]
        );
        let mut aligned = cfg.clone();
        aligned.set_label_align(1);
        aligned.set_value_align(2);
        assert_eq!(aligned.label_align(), 1);
        assert_eq!(aligned.value_align(), 2);
        assert_eq!(aligned.flags & 0x03, 0x03, "the two booleans survive");

        let p = CsDisplayPage {
            noun: m::noun::OUTPUT_GAIN,
            target: 2,
            index: 0,
            flags: page_flags::ACTIVE | page_flags::LARGE,
        };
        assert_eq!(p.encode(), [17, 2, 0, 0x05]);
    }

    // ------------------------------------------------------------ behaviour

    #[test]
    fn adding_a_control_offers_the_types_and_applies_at_once() {
        let (mut s, st) = screen(m::demo::settings_data());
        s.handle(key(KeyCode::Tab), &st);
        // Walk to the Add Control row: it is the last focusable one.
        for _ in 0..60 {
            s.handle(key(KeyCode::Down), &st);
        }
        match s.handle(key(KeyCode::Enter), &st) {
            ScreenEvent::Popup(p) => {
                assert_eq!(p.title, "Add Control");
                assert!(p.items.iter().any(|x| x == "Push Button"), "{:?}", p.items);
                // The IR receiver and the display are one per device, and the
                // fixture already has both.
                assert!(!p.items.iter().any(|x| x == "IR Remote"), "{:?}", p.items);
                assert!(!p.items.iter().any(|x| x == "Display"), "{:?}", p.items);
            }
            other => panic!("{other:?}"),
        }
        match s.popup_result(Some(0), &st) {
            ScreenEvent::Session(r) => assert_eq!(r.tag, 5, "the first free slot"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn apply_sends_the_binding_through_the_session() {
        let (mut p, d, st) = page();
        let cfg = AppConfig::default();
        let c = cx(&d, &st, &cfg);
        p.expanded.insert(1);
        // Change the gesture, which makes the card dirty.
        let ev = p.act(index_of(&p, &c, Item::Event(1)), Action::Selected(1), &c);
        assert_eq!(ev, PageEvent::Handled);
        assert_eq!(p.drafts[1].event, 1);
        assert!(p.dirty(1));
        p.apply_button.insert(1, 1);
        match p.act(index_of(&p, &c, Item::Apply(1)), Action::Open, &c) {
            PageEvent::Session(r) => assert_eq!(r.tag, 1),
            other => panic!("{other:?}"),
        }
    }

    /// The whole point of the write path: a SET returns before the firmware has
    /// run it, so Apply has to see PENDING and then the real answer.
    #[test]
    fn an_apply_waits_out_pending_and_reports_the_final_status() {
        use dspi_proto::generated::opcodes as op;
        use dspi_session::surfaces::status;
        use dspi_transport::MockTransport;
        use dspi_transport::mock::Reply;

        fn status_bytes(slot: u8, code: u8) -> Vec<u8> {
            let mut d = vec![code, slot, 16, 1, 0x01, 0x00];
            d.extend([0u8; 16]);
            d.extend(0u16.to_le_bytes());
            d.push(0);
            d.extend([0u8; 16]);
            d
        }

        let t = MockTransport::new()
            .data(op::REQ_SET_CS_BINDING, vec![])
            .reply(
                op::REQ_GET_CS_STATUS,
                Reply::Sequence(vec![
                    Reply::Data(status_bytes(3, status::PENDING)),
                    Reply::Data(status_bytes(3, status::SUCCESS)),
                ]),
            );
        let mut caps = crate::shell::fixture::caps();
        caps.cs = Some(dspi_session::probe::ControlSurfaceCaps {
            caps_version: 13,
            max_bindings: 16,
            type_count: 9,
            noun_count: 57,
            max_ir_commands: 16,
            max_groups: 8,
            max_macros: 8,
            max_macro_steps: 8,
            max_pages: 16,
            display_models: 8,
            types: Vec::new(),
        });
        let mut session = dspi_session::Session::new(Box::new(t), caps.clone()).expect("session");
        let b = CsBinding {
            component: m::ty::BUTTON,
            gpio: [16, GPIO_UNUSED],
            ..Default::default()
        };
        let bb = b.clone();
        assert_eq!(
            m::run(&mut session, move |s| s.write_binding(3, &bb)),
            SessionReply::Ok("Applied".into()),
            "PENDING is waited out, not reported"
        );

        // And a refusal comes back as the device's own reason, capitalised.
        let t = MockTransport::new()
            .data(op::REQ_SET_CS_BINDING, vec![])
            .data(op::REQ_GET_CS_STATUS, status_bytes(3, 0x15));
        let mut session = dspi_session::Session::new(Box::new(t), caps).expect("session");
        assert_eq!(
            m::run(&mut session, move |s| s.write_binding(3, &b)),
            SessionReply::Err("A potentiometer needs an analogue-capable pin".into())
        );
    }

    /// Nothing on the control-surface path pushes a notification
    /// (`survey-firmware.md` 3.14), so an apply that only ever hears "Applied"
    /// used to leave the card drawing the snapshot Settings opened with: a
    /// control the device had just brought up still read **Not running**.
    #[test]
    fn a_card_follows_the_device_after_a_successful_apply() {
        use dspi_proto::generated::opcodes as op;
        use dspi_proto::packets::CsStatusPacket;
        use dspi_transport::MockTransport;

        let status = |active: u16| {
            CsStatusPacket {
                last_status: 0,
                last_slot: 4,
                max_bindings: 16,
                dirty: true,
                active_mask: active,
                slot_status: vec![0; 16],
                ir_active_mask: 0b0001,
                ir_learn_state: 0,
                ir_cmd_status: vec![0; 16],
            }
            .encode()
        };

        // The device is holding slot 4 but not running it.
        let mut d = m::demo::settings_data();
        d.cs.as_mut().expect("cs").status.active_mask = 0b0000_1111;
        let (mut s, st) = screen(d);
        s.pages.surfaces.expanded.insert(4);
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("Not running:"), "the card starts inactive:\n{f}");

        // Apply it. The device now reports it running; the page has no other
        // way to hear that.
        let mut caps = crate::shell::fixture::caps();
        caps.cs = Some(m::demo::caps());
        let wired = CsBinding {
            component: m::ty::BUTTON,
            gpio: [16, GPIO_UNUSED],
            ..Default::default()
        };
        let t = MockTransport::new()
            .data(op::REQ_SET_CS_BINDING, vec![])
            .data(op::REQ_GET_CS_STATUS, status(0b0001_1111))
            .data(op::REQ_GET_CS_BINDING, wired.encode().to_vec())
            .window(
                op::REQ_GET_ALL_PARAMS_CHUNK,
                crate::shell::fixture::packet(),
            );
        let mut session = dspi_session::Session::new(Box::new(t), caps).expect("session");
        assert!(s.data.claims.is_none(), "nothing has read the pin map yet");

        let ev = s.pages.surfaces.apply(4);
        let req = match s.absorb(ev, &st) {
            ScreenEvent::Session(r) => r,
            other => panic!("{other:?}"),
        };
        let reply = (req.run)(&mut session);
        assert_eq!(reply, SessionReply::Ok("Applied".into()));
        s.session_result(req.tag, reply, &st);

        assert!(
            s.data.cs.as_ref().expect("cs").status.is_slot_active(4),
            "the re-read reached SettingsData"
        );
        let f = frame(&mut s, &st, 120, 40);
        assert!(!f.contains("Not running:"), "the card caught up:\n{f}");

        // And the pin map came with it, so a GPIO this control now holds is
        // not offered as free to the next one.
        let claims = s.data.claims.as_ref().expect("the pin map was re-read");
        assert!(
            claims
                .iter()
                .any(|c| c.gpio == 16 && c.owner.starts_with("Control Surface")),
            "{claims:?}"
        );
    }

    /// A digit counts the cards on screen. The demo device has controls in
    /// slots 1 to 5, so this also pins the 1-based convention.
    #[test]
    fn a_digit_opens_the_nth_card_not_the_nth_slot() {
        let mut d = m::demo::settings_data();
        let cs = d.cs.as_mut().expect("cs");
        // Leave only slots 2 and 5 configured.
        for slot in [0usize, 2, 3] {
            cs.bindings[slot] = CsBinding::default();
            cs.names[slot] = String::new();
        }
        let mut p = SurfacesPage::new(&d);
        // Slot 13 holds the button on the aux output; the outputs themselves
        // are on the other page.
        assert_eq!(p.visible(), vec![1, 4, 12]);
        let (st, cfg) = (m::demo::state(), AppConfig::default());
        let c = cx(&d, &st, &cfg);
        assert_eq!(p.key(key(KeyCode::Char('1')), &c), PageEvent::Handled);
        assert!(p.expanded.contains(&1), "the first card, in slot 2");
        assert_eq!(p.key(key(KeyCode::Char('2')), &c), PageEvent::Handled);
        assert!(p.expanded.contains(&4), "the second card, in slot 5");
        assert_eq!(p.key(key(KeyCode::Char('4')), &c), PageEvent::Unhandled);
    }

    /// Arming a learn is a write with an answer: `CS_STATUS_NO_IR` when no
    /// receiver is live (control_surfaces.h:798). Dropping it left the card
    /// saying "Waiting for a button..." forever.
    #[test]
    fn a_refused_learn_arm_says_so_instead_of_waiting() {
        use dspi_proto::generated::opcodes as op;
        use dspi_transport::MockTransport;

        let caps = |cs| {
            let mut c = crate::shell::fixture::caps();
            c.cs = Some(cs);
            c
        };
        // CS_STATUS_NO_IR from the arm.
        let t = MockTransport::new().data(op::REQ_CS_IR_LEARN, vec![0x1E]);
        let mut session =
            dspi_session::Session::new(Box::new(t), caps(m::demo::caps())).expect("session");
        assert_eq!(
            m::run_code(&mut session, LEARN_PROMPT, |s| s.arm_learn()),
            SessionReply::Err("Set up an IR receiver first".into())
        );

        // And the page turns that into the card's own error, with nothing left
        // listening.
        let (mut p, _, _) = page();
        p.learning = Some((2, None));
        let (d, st, cfg) = (
            m::demo::settings_data(),
            m::demo::state(),
            AppConfig::default(),
        );
        let c = cx(&d, &st, &cfg);
        p.session_result(
            TAG_LEARN | 2,
            SessionReply::Err("Set up an IR receiver first".into()),
            &c,
        );
        assert!(p.learning.is_none(), "nothing is still listening");
        assert_eq!(
            p.ir_messages.get(&2).map(|(t, e)| (t.as_str(), *e)),
            Some(("Set up an IR receiver first", true))
        );
    }

    /// A control-surface write is a live preview the device holds in RAM, so a
    /// successful apply raises the save bar's third category and its own
    /// subtitle, not the flash one.
    #[test]
    fn an_applied_control_raises_the_control_surface_save_category() {
        let (mut s, st) = screen(m::demo::settings_data());
        assert!(!s.dirty(&st));
        s.session_result(1, SessionReply::Ok("Applied".into()), &st);
        assert!(s.dirty(&st));
        let f = frame(&mut s, &st, 120, 40);
        assert!(
            f.contains("Your controls are live now; saving keeps them across a reboot."),
            "{f}"
        );
        // A refusal is not a change the device is holding.
        let (mut s, st) = screen(m::demo::settings_data());
        s.session_result(1, SessionReply::Err("nope".into()), &st);
        assert!(!s.dirty(&st));
    }

    /// A group and a channel share the `target` byte, so the merged picker has
    /// to say which one a row is.
    #[test]
    fn the_target_picker_lists_channels_then_groups() {
        let (mut p, d, st) = page();
        let cfg = AppConfig::default();
        let c = cx(&d, &st, &cfg);
        p.expanded.insert(2); // the pot, on Output Gain
        let rows = p.build(&c);
        let Some((_, Row::Pick { label, choices, .. })) =
            rows.iter().find(|(i, _)| *i == Some(Item::Target(2)))
        else {
            panic!("no target row");
        };
        assert_eq!(label, "Channel or Group");
        assert!(choices[0].starts_with("OUT"), "{choices:?}");
        assert!(
            choices.iter().any(|x| x == "Group: Front Pair (2 ch)"),
            "{choices:?}"
        );
        // Picking the group sets the flag rather than a channel number.
        let n = choices.len() - 1;
        p.act(index_of(&p, &c, Item::Target(2)), Action::Selected(n), &c);
        assert_eq!(p.drafts[2].flags & m::flag::GROUP, m::flag::GROUP);
        assert_eq!(p.drafts[2].target, 0, "the group index, not a channel");
        // And picking a channel back drops the two group modifiers with it.
        p.drafts[2].flags |= m::flag::LINK_ABS;
        p.act(index_of(&p, &c, Item::Target(2)), Action::Selected(1), &c);
        assert_eq!(
            p.drafts[2].flags & (m::flag::GROUP | m::flag::LINK_ABS),
            0,
            "a modifier left set is rejected with the whole binding"
        );
    }

    #[test]
    fn reverting_a_card_puts_the_device_values_back() {
        let (mut p, d, st) = page();
        let cfg = AppConfig::default();
        let c = cx(&d, &st, &cfg);
        p.expanded.insert(1);
        p.act(index_of(&p, &c, Item::Event(1)), Action::Selected(2), &c);
        assert!(p.dirty(1));
        p.apply_button.insert(1, 0);
        assert_eq!(
            p.act(index_of(&p, &c, Item::Apply(1)), Action::Open, &c),
            PageEvent::Status("Control changes reverted".into())
        );
        assert!(!p.dirty(1));
    }

    #[test]
    fn a_learn_result_from_the_notification_stream_lands_in_the_draft() {
        let (mut p, d, mut st) = page();
        let cfg = AppConfig::default();
        p.learning = Some((1, None));
        st.ir_learn = Some((2, 1, 0x1234_5678));
        p.observe_learn(&cx(&d, &st, &cfg));
        assert_eq!(p.ir_drafts[1].protocol, 1);
        assert_eq!(p.ir_drafts[1].code, 0x1234_5678);
        assert!(p.learning.is_none());
        assert!(
            p.ir_messages[&1].0.contains("Learned a NEC code"),
            "{:?}",
            p.ir_messages[&1]
        );
    }

    #[test]
    fn a_learn_that_times_out_says_so() {
        let (mut p, d, mut st) = page();
        let cfg = AppConfig::default();
        p.learning = Some((1, None));
        st.ir_learn = Some((3, 0, 0));
        p.observe_learn(&cx(&d, &st, &cfg));
        assert!(p.learning.is_none());
        assert_eq!(p.ir_messages[&1].0, LEARN_TIMEOUT);
    }

    /// A result left over from an earlier learn must not be adopted the moment
    /// the next one is armed.
    #[test]
    fn a_stale_learn_result_is_not_adopted() {
        let (mut p, d, mut st) = page();
        let cfg = AppConfig::default();
        st.ir_learn = Some((2, 1, 0xAAAA_AAAA));
        p.learning = Some((2, st.ir_learn));
        p.observe_learn(&cx(&d, &st, &cfg));
        assert!(p.learning.is_some(), "still waiting for a fresh press");
        assert_eq!(p.ir_drafts[2].code, 0);
    }

    #[test]
    fn the_ir_section_counts_its_buttons_and_gates_learning() {
        let (mut s, st) = screen(m::demo::settings_data());
        s.handle(key(KeyCode::Tab), &st);
        // Expand the IR receiver, slot 4 in the fixture (card 4).
        for _ in 0..6 {
            s.handle(key(KeyCode::Down), &st);
        }
        let f = frame(&mut s, &st, 120, 40);
        let _ = f;
        let (mut p, d, st2) = page();
        let cfg = AppConfig::default();
        p.expanded.insert(3);
        let c = cx(&d, &st2, &cfg);
        let rows = p.build(&c);
        let text: Vec<String> = rows.iter().map(|(_, r)| format!("{r:?}")).collect();
        let joined = text.join("\n");
        assert!(joined.contains("REMOTE BUTTONS"), "{joined}");
        assert!(joined.contains("1/16"), "the n/max count: {joined}");
        assert!(joined.contains("NEC 0x20DF40BF"), "the code chip: {joined}");
    }

    #[test]
    fn a_display_page_write_goes_out_as_it_is_edited() {
        let (mut p, d, st) = page();
        let cfg = AppConfig::default();
        let c = cx(&d, &st, &cfg);
        p.expanded.insert(4); // the display
        match p.act(
            index_of(&p, &c, Item::PageLarge(1)),
            Action::Toggled(true),
            &c,
        ) {
            PageEvent::Session(r) => assert_eq!(r.tag, TAG_DISPLAY_PAGE | 1),
            other => panic!("{other:?}"),
        }
        assert_eq!(p.live.pages[1].flags & page_flags::LARGE, page_flags::LARGE);
    }

    /// A bar plots the value inside the noun's range, so the flag is refused
    /// before the write on anything that has none.
    #[test]
    fn a_level_bar_is_refused_on_a_bool_or_enum_page() {
        let (mut p, d, st) = page();
        let cfg = AppConfig::default();
        let c = cx(&d, &st, &cfg);
        p.expanded.insert(4);
        // Page 1 shows the preset, an enum.
        assert!(!m::page_bar_allowed(&p.live, m::noun::PRESET));
        let ev = p.act(
            index_of(&p, &c, Item::PageBar(1)),
            Action::Toggled(true),
            &c,
        );
        assert_eq!(
            ev,
            PageEvent::Status("Only a value with a range can be drawn as a bar.".into())
        );
        assert_eq!(p.live.pages[1].flags & page_flags::BAR, 0);
        // Page 0 shows the volume, which does have a range.
        assert!(m::page_bar_allowed(&p.live, m::noun::USER_VOLUME));
    }

    #[test]
    fn the_display_warns_when_nothing_can_arm_editing() {
        let (mut p, d, st) = page();
        let cfg = AppConfig::default();
        let c = cx(&d, &st, &cfg);
        p.expanded.insert(4);
        // Nothing uses Browse/Adjust yet, so the warning stays away.
        let joined = |p: &SurfacesPage| {
            p.build(&c)
                .iter()
                .map(|(_, r)| format!("{r:?}"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert!(!joined(&p).contains("Nothing can arm editing"));
        // Bind the encoder to Browse/Adjust and the dead end appears.
        p.drafts[0] = m::set_noun(&p.live, &p.drafts[0], m::noun::PAGE_VALUE);
        assert!(p.uses_page_value());
        assert!(!p.can_arm_editing());
        assert!(
            joined(&p).contains("Nothing can arm editing"),
            "{}",
            joined(&p)
        );
        // A button that writes Allow Editing lifts it.
        p.drafts[1] = m::set_noun(&p.live, &p.drafts[1], m::noun::DISPLAY_EDIT);
        assert!(p.can_arm_editing());
    }

    #[test]
    fn a_delayed_indicator_carries_the_consoles_warning() {
        let (mut p, d, st) = page();
        let cfg = AppConfig::default();
        let c = cx(&d, &st, &cfg);
        // Turn the button into an LED indicating mute, then give it a delay.
        p.drafts[1] = m::make_binding(&c, &p.live, &p.drafts, 1, m::ty::LED);
        p.drafts[1] = m::set_noun(&p.live, &p.drafts[1], m::noun::USER_MUTE);
        p.expanded.insert(1);
        assert_eq!(p.drafts[1].action, m::act::IND_EQUALS);
        let ev = p.act(
            index_of(&p, &c, Item::OnDelay(1)),
            Action::Committed(3.0),
            &c,
        );
        assert_eq!(ev, PageEvent::Handled);
        assert_eq!(p.drafts[1].on_delay, 30, "0.1 s units");
        let joined = p
            .build(&c)
            .iter()
            .map(|(_, r)| format!("{r:?}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("that is a power cycle"), "{joined}");
    }

    #[test]
    fn a_dimmable_led_gets_a_brightness_ceiling_and_nothing_else_does() {
        let (mut p, d, st) = page();
        let cfg = AppConfig::default();
        let c = cx(&d, &st, &cfg);
        p.drafts[1] = m::make_binding(&c, &p.live, &p.drafts, 1, m::ty::LED_PWM);
        p.expanded.insert(1);
        let rows = p.build(&c);
        assert!(rows.iter().any(|(i, _)| *i == Some(Item::Bright(1))));
        // The plain LED has none.
        p.drafts[1] = m::make_binding(&c, &p.live, &p.drafts, 1, m::ty::LED);
        let rows = p.build(&c);
        assert!(!rows.iter().any(|(i, _)| *i == Some(Item::Bright(1))));
    }

    /// The row a card draws for an item, as the page built it.
    fn row_for(p: &SurfacesPage, c: &Cx<'_>, want: Item) -> Row {
        p.build(c)
            .into_iter()
            .find(|(i, _)| *i == Some(want))
            .map(|(_, r)| r)
            .unwrap_or_else(|| panic!("no row for {want:?}"))
    }

    /// A control on an aux noun picks an auxiliary output, not a channel:
    /// only the aux slots (only the dimmable one for the level), named as the
    /// device names them, with no groups, and the choice lands on the slot.
    #[test]
    fn the_aux_target_picker_offers_only_aux_outputs() {
        let (mut p, d, st) = page();
        let cfg = AppConfig::default();
        let c = cx(&d, &st, &cfg);
        p.expanded.insert(12); // the button on Aux Switch
        let Row::Pick {
            label,
            choices,
            selected,
            caption,
            ..
        } = row_for(&p, &c, Item::Target(12))
        else {
            panic!("not a picker");
        };
        assert_eq!(label, "Auxiliary Output");
        assert_eq!(choices, vec!["Amp Trigger", "Aux 12"]);
        assert_eq!(selected, 0, "slot 10");
        assert_eq!(
            caption.as_deref(),
            Some("Which auxiliary output this control affects.")
        );
        p.act(index_of(&p, &c, Item::Target(12)), Action::Selected(1), &c);
        assert_eq!(p.drafts[12].target, 11);
        assert_eq!(p.drafts[12].flags & m::flag::GROUP, 0);

        // The level noun reaches only the dimmable output.
        p.drafts[12] = m::set_noun_in(&p.live, &p.drafts[12], m::noun::AUX_LEVEL, Some(12));
        let Row::Pick { choices, .. } = row_for(&p, &c, Item::Target(12)) else {
            panic!("not a picker");
        };
        assert_eq!(choices, vec!["Aux 12"]);
        assert_eq!(p.drafts[12].target, 11);

        // The Controls picker files both nouns under the Console's category.
        let Row::Pick { choices, .. } = row_for(&p, &c, Item::Noun(12)) else {
            panic!("not a picker");
        };
        assert!(
            choices
                .iter()
                .any(|x| x == "Auxiliary Outputs / Enable/Disable"),
            "{choices:?}"
        );
        assert!(
            choices.iter().any(|x| x == "Auxiliary Outputs / Level"),
            "{choices:?}"
        );
    }

    /// Limiter Release is `CS_UNIT_MS_LOG` (control_surfaces.h:268-269):
    /// plain whole milliseconds in `value` and the span, octaves in `step`.
    /// Written as 8.8 it would overflow at 127 ms.
    #[test]
    fn the_noun_editor_writes_ms_log_as_plain_milliseconds() {
        let (mut p, d, st) = page();
        let cfg = AppConfig::default();
        let c = cx(&d, &st, &cfg);

        // A button that sets it: 250 ms is 250 on the wire.
        p.drafts[1] = m::set_noun(&p.live, &p.drafts[1], m::noun::LIMITER_RELEASE);
        p.drafts[1].action = m::act::SET;
        p.drafts[1] = m::default_operands(&p.live, &p.drafts[1]);
        assert_eq!(
            p.drafts[1].value, 1000,
            "the default is the range top, in ms"
        );
        p.expanded.insert(1);
        let Row::Number {
            min,
            max,
            unit,
            decimals,
            ..
        } = row_for(&p, &c, Item::Value(1))
        else {
            panic!("not a number");
        };
        assert_eq!((min, max), (10.0, 1000.0));
        assert_eq!((unit.as_str(), decimals), ("ms", 0));
        p.act(
            index_of(&p, &c, Item::Value(1)),
            Action::Committed(250.0),
            &c,
        );
        assert_eq!(p.drafts[1].value, 250);
        assert_eq!(&p.drafts[1].encode()[10..12], &250i16.to_le_bytes());

        // An encoder steps it in octaves, 8.8.
        p.drafts[0] = m::set_noun(&p.live, &p.drafts[0], m::noun::LIMITER_RELEASE);
        p.expanded.insert(0);
        let Row::Number { unit, .. } = row_for(&p, &c, Item::StepSize(0)) else {
            panic!("not a number");
        };
        assert_eq!(unit, "oct");
        p.act(
            index_of(&p, &c, Item::StepSize(0)),
            Action::Committed(0.5),
            &c,
        );
        assert_eq!(p.drafts[0].step, 128, "half an octave in 8.8");

        // A pot's span is plain ms too.
        p.drafts[2] = m::set_noun(&p.live, &p.drafts[2], m::noun::LIMITER_RELEASE);
        p.expanded.insert(2);
        p.act(index_of(&p, &c, Item::SpanOn(2)), Action::Toggled(true), &c);
        assert_eq!((p.drafts[2].range_min, p.drafts[2].range_max), (10, 1000));
        p.act(
            index_of(&p, &c, Item::SpanMax(2)),
            Action::Committed(500.0),
            &c,
        );
        assert_eq!(p.drafts[2].range_max, 500);
    }

    /// Byte 22 is 0 on every type but an aux output (control_surfaces.h:
    /// 468-469), so Apply sends it as 0 whatever a draft carries.
    #[test]
    fn a_control_is_applied_with_extras_zero() {
        use dspi_proto::generated::opcodes as op;
        use dspi_transport::MockTransport;

        let (mut p, _, _) = page();
        p.drafts[1].event = 1;
        p.drafts[1].extras = 0x05;
        let PageEvent::Session(req) = p.apply(1) else {
            panic!("no apply");
        };
        let mut caps = crate::shell::fixture::caps();
        caps.cs = Some(m::demo::caps());
        let mut ok = p.live.status.clone();
        ok.last_slot = 1;
        let t = MockTransport::new()
            .data(op::REQ_SET_CS_BINDING, vec![])
            .data(op::REQ_GET_CS_STATUS, ok.encode());
        let log = t.log_handle();
        let mut session = dspi_session::Session::new(Box::new(t), caps).expect("session");
        assert_eq!((req.run)(&mut session), SessionReply::Ok("Applied".into()));
        let sent = log
            .lock()
            .expect("log")
            .iter()
            .find(|e| e.opcode == op::REQ_SET_CS_BINDING)
            .cloned()
            .expect("the SET");
        assert_eq!(sent.payload[22], 0);
    }
}
