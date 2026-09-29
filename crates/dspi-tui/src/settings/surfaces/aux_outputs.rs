//! Settings > Control > Auxiliary Outputs: an aux output's card.
//!
//! An auxiliary output is a binding slot of type On/Off Output or Dimmable
//! Output (caps v18, control_surfaces.h:141-144, 425-437) that owns a GPIO a
//! button, knob, remote key or macro switches through the `Aux Switch` and
//! `Aux Level` nouns. Its wiring, delays and power-on behaviour are the
//! binding, staged and applied like any control's, and saved with them. The
//! live switch and level are not: they are two immediate writes (0x04 and
//! 0x06, config.h:136-142) that apply at once, never reach flash and never
//! make the configuration unsaved, so a refusal shows only in the status
//! packet, which [`Surfaces::set_aux_state`] reads.
//!
//! The card follows the Console's order (`slotSection`, DSPi_ConsoleApp.swift:
//! 4438-4449): the live rows, the pin and its sense, the dimmable output's
//! ceiling and curve, the delays, the power-on rows, and what drives it.
//!
//! [`Surfaces::set_aux_state`]: dspi_session::surfaces::Surfaces::set_aux_state

use super::*;

pub(super) const EMPTY_TITLE: &str = "No Auxiliary Outputs Set Up";
pub(super) const EMPTY_BODY: &str = "Put a relay, lamp or fan on a spare GPIO, then point a \
                                     button, knob or remote key at it from the Control Surfaces \
                                     page.";
pub(super) const ADD: &str = "Add Output";

/// The Console's footer, verbatim, one paragraph per note.
pub(super) const FOOTER: [&str; 4] = [
    "An auxiliary output is a GPIO the DSPi switches or dims for you and never reads itself: it \
     changes nothing about the sound. It exists so a button, knob, remote key or macro can drive \
     something the device knows nothing about - an amplifier trigger, a speaker relay, a panel \
     lamp, a fan.",
    "An on/off output follows its switch. A dimmable output follows its switch and its level, so \
     one button and one knob can share a lamp. Turn on \"Active-Low Output\" for the relay and \
     opto-isolator boards that switch when the pin goes low.",
    "A GPIO is a 3.3 V pin good for a few milliamps. Anything real needs a MOSFET, a transistor \
     with a flyback diode, or an opto-isolated relay module in between, and a dimmed load should \
     have its own supply so its switching noise stays out of the DAC.",
    "Switching an output is instant and never writes to flash. The pin, name and power-on \
     behaviour are stored on the device alongside the controls and share their Save and Revert.",
];

pub(super) const LIVE: &str = "Switches the pin now. Instant, and never written to flash.";
pub(super) const NOT_LIVE: &str = "Apply the output first; the switch works once it is running.";
const LEVEL: &str = "How bright or fast the load runs while the output is on.";
const LIMIT: &str = "Cap on the output's duty as a share of full. Everything below the cap \
                     scales with it.";
const LINEAR: &str = "Off: the level follows the eye's curve, right for a lamp. On: duty is \
                      proportional to the level, right for a fan or heater.";
const BOOT: &str = "What this output does when the device starts up.";
pub(super) const SAVED: &str = "Saving takes a copy of the switch and level as they are at that \
                                moment, and the output comes back that way after a restart. \
                                Changing them afterwards does not move the stored values until \
                                the next save.";
const STARTS_ON: &str = "Leave this off for anything that should never wake with the device, \
                         such as an amplifier trigger.";
const START_LEVEL: &str = "The level this output comes up at.";
const DRIVEN_BY: &str =
    "Controls on the Control Surfaces page, remote keys and macros pointed at this output.";
pub(super) const NOTHING_DIMMABLE: &str = "Nothing yet. Add a button on \"Aux Switch\" or an \
                                           encoder or fader on \"Aux Level\" and point it at \
                                           this output.";
pub(super) const NOTHING_ON_OFF: &str =
    "Nothing yet. Add a button or switch on \"Aux Switch\" and point it at this output.";

/// Percent in 8.8, as the level SET and the boot level carry it, clamped to
/// 100 % (config.h:141-142, control_surfaces.h:425-428).
fn level_q8(percent: f64) -> u16 {
    (percent * 256.0)
        .round()
        .clamp(0.0, dspi_session::surfaces::AUX_LEVEL_MAX_Q8 as f64) as u16
}

/// True when a binding, remote key or macro step drives this aux slot: one
/// of the two aux nouns, ungrouped, pointed at it.
fn drives(n: u8, flags: u8, target: u8, slot: usize) -> bool {
    (n == m::noun::AUX || n == m::noun::AUX_LEVEL)
        && flags & m::flag::GROUP == 0
        && target as usize == slot
}

impl SurfacesPage {
    /// The body of an aux output's card, below its Component row.
    pub(super) fn aux_rows(&self, cx: &Cx<'_>, slot: usize) -> Vec<(Option<Item>, Row)> {
        let b = &self.drafts[slot];
        let dimmable = b.component == m::ty::AUX_PWM;
        let mut rows: Vec<(Option<Item>, Row)> = Vec::new();

        // The live values exist only while the output is running, so a slot
        // not applied yet, or down on a pin conflict, shows them disabled
        // (`auxLiveRows`, DSPi_ConsoleApp.swift:3181-3223).
        let running = !self.live.binding(slot).is_empty()
            && self.live.status.is_slot_active(slot as u8)
            && cx.connected;
        rows.push((
            Some(Item::AuxLive(slot)),
            Row::Toggle {
                label: "Output".into(),
                on: self.aux.is_on(slot),
                caption: Some(if running { LIVE } else { NOT_LIVE }.into()),
                enabled: running,
            },
        ));
        if dimmable {
            rows.push((
                Some(Item::AuxLevel(slot)),
                Row::Number {
                    label: "Level".into(),
                    value: self.aux.level_percent(slot).round() as f64,
                    min: 0.0,
                    max: 100.0,
                    step: 5.0,
                    unit: "%".into(),
                    decimals: 0,
                    caption: Some(LEVEL.into()),
                    enabled: running,
                },
            ));
        }
        if let Some(why) = self.aux_messages.get(&slot) {
            rows.push((None, Row::Status(why.clone(), true)));
        }

        rows.extend(self.pin_rows(cx, slot));
        rows.push((
            Some(Item::Flag(slot, m::flag::INVERT)),
            Row::Toggle {
                label: m::invert_title(b.component).into(),
                on: b.flags & m::flag::INVERT != 0,
                caption: Some(m::invert_detail(b.component).into()),
                enabled: true,
            },
        ));

        if dimmable {
            rows.push((
                Some(Item::AuxLimit(slot)),
                Row::Number {
                    label: "Level Limit".into(),
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
                    caption: Some(LIMIT.into()),
                    enabled: true,
                },
            ));
            rows.push((
                Some(Item::AuxLinear(slot)),
                Row::Toggle {
                    label: "Linear Response".into(),
                    on: b.extras & m::aux_extras::LINEAR != 0,
                    caption: Some(LINEAR.into()),
                    enabled: true,
                },
            ));
        }

        rows.extend(self.delay_rows(slot));

        // Power-on behaviour, carried in `extras` and, for a dimmable output,
        // the boot level in `value` (`auxBootRows`, :3265-3308).
        let saved = b.extras & m::aux_extras::BOOT_SAVED != 0;
        rows.push((
            Some(Item::AuxBoot(slot)),
            Row::Pick {
                label: "At Power-On".into(),
                choices: vec!["Fixed".into(), "As Last Saved".into()],
                selected: usize::from(saved),
                caption: Some(BOOT.into()),
                enabled: true,
            },
        ));
        if saved {
            rows.push((None, Row::note(SAVED)));
        } else {
            rows.push((
                Some(Item::AuxStartsOn(slot)),
                Row::Toggle {
                    label: "Starts On".into(),
                    on: b.extras & m::aux_extras::BOOT_ON != 0,
                    caption: Some(STARTS_ON.into()),
                    enabled: true,
                },
            ));
            if dimmable {
                rows.push((
                    Some(Item::AuxStartLevel(slot)),
                    Row::Number {
                        label: "Starting Level".into(),
                        value: (b.value.max(0) as f64 / 256.0).round(),
                        min: 0.0,
                        max: 100.0,
                        step: 5.0,
                        unit: "%".into(),
                        decimals: 0,
                        caption: Some(START_LEVEL.into()),
                        enabled: true,
                    },
                ));
            }
        }

        rows.extend(self.driver_rows(slot));
        rows
    }

    /// What drives this output: the controls on the other page, then the
    /// remote keys and macros, which the list cannot show one by one
    /// (`auxDriverRows`, DSPi_ConsoleApp.swift:3310-3362).
    fn driver_rows(&self, slot: usize) -> Vec<(Option<Item>, Row)> {
        let mut rows: Vec<(Option<Item>, Row)> = Vec::new();
        rows.push((None, Row::pair("Driven By", String::new())));
        rows.push((None, Row::note(DRIVEN_BY)));
        let controls: Vec<usize> = (0..self.slot_count())
            .filter(|s| {
                let b = &self.drafts[*s];
                !b.is_empty() && drives(b.noun, b.flags, b.target, slot)
            })
            .collect();
        let remotes = self
            .live
            .ir
            .iter()
            .filter(|c| !c.is_empty() && drives(c.noun, c.flags, c.target, slot))
            .count();
        let macros: Vec<usize> = (0..self.live.macros.len())
            .filter(|i| {
                self.live.macros[*i]
                    .active_steps()
                    .iter()
                    .any(|st| drives(st.noun, st.flags, st.target, slot))
            })
            .collect();
        if controls.is_empty() && remotes == 0 && macros.is_empty() {
            let dimmable = self.drafts[slot].component == m::ty::AUX_PWM;
            rows.push((
                None,
                Row::note(if dimmable {
                    NOTHING_DIMMABLE
                } else {
                    NOTHING_ON_OFF
                }),
            ));
            return rows;
        }
        for s in controls {
            let b = &self.drafts[s];
            let name = if self.names[s].is_empty() {
                m::type_name(b.component)
            } else {
                self.names[s].clone()
            };
            let is_enum = m::operand_kind(&self.live, b.noun) == m::kind::ENUM;
            rows.push((
                None,
                Row::note(format!(
                    "{name} - {} on {}",
                    m::action_name(b.action, b.noun, is_enum),
                    m::noun_name(b.noun, b.component)
                )),
            ));
        }
        if remotes > 0 || !macros.is_empty() {
            rows.push((None, Row::note(self.other_drivers(remotes, &macros))));
        }
        rows
    }

    /// "Also driven by 2 remote keys and the macro Night." (the Console's
    /// `auxOtherUsersSummary`).
    fn other_drivers(&self, remotes: usize, macros: &[usize]) -> String {
        let mut parts: Vec<String> = Vec::new();
        if remotes > 0 {
            parts.push(format!(
                "{remotes} remote key{}",
                if remotes == 1 { "" } else { "s" }
            ));
        }
        if !macros.is_empty() {
            parts.push(format!(
                "the macro{} {}",
                if macros.len() == 1 { "" } else { "s" },
                macros
                    .iter()
                    .map(|i| self.live.macro_name(*i))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        format!("Also driven by {}.", parts.join(" and "))
    }

    /// Take on an aux change from the notification stream
    /// (`NOTIFY_EVT_CS_AUX`, notify.h:78-83). `DeviceState` keeps the last
    /// event, so a change of it is a new event. The first look only notes
    /// it: an event from before Settings opened is older than the read the
    /// page opened with.
    pub(super) fn observe_aux(&mut self, cx: &Cx<'_>) {
        let now = cx.state.cs_aux;
        let first = !self.aux_seen;
        self.aux_seen = true;
        if first || now == self.aux_event {
            self.aux_event = now;
            return;
        }
        self.aux_event = now;
        if let Some((slot, state, level)) = now {
            let s = slot as usize;
            if s < self.aux.state.len() {
                self.aux.state[s] = state;
                self.aux.level_q8[s] = level;
            }
        }
    }

    /// The aux card's own rows. `None` leaves the action to the shared
    /// handling, which opens pickers.
    pub(super) fn act_aux(&mut self, item: Item, action: &Action) -> Option<PageEvent> {
        let extras = |page: &mut Self, slot: usize, mask: u8, on: bool| {
            set_flag(&mut page.drafts[slot].extras, mask, on);
            PageEvent::Handled
        };
        Some(match (item, action) {
            (Item::AuxLive(slot), Action::Toggled(on)) => self.write_aux_state(slot, *on),
            (Item::AuxLevel(slot), Action::Changed(v) | Action::Committed(v)) => {
                self.write_aux_level(slot, *v)
            }
            (Item::AuxLimit(slot), Action::Changed(v) | Action::Committed(v)) => {
                self.drafts[slot].base_bright =
                    v.round().clamp(1.0, m::LED_BRIGHT_MAX as f64) as u8;
                PageEvent::Handled
            }
            (Item::AuxLinear(slot), Action::Toggled(on)) => {
                extras(self, slot, m::aux_extras::LINEAR, *on)
            }
            (Item::AuxBoot(slot), Action::Selected(c)) => {
                extras(self, slot, m::aux_extras::BOOT_SAVED, *c == 1)
            }
            (Item::AuxStartsOn(slot), Action::Toggled(on)) => {
                extras(self, slot, m::aux_extras::BOOT_ON, *on)
            }
            (Item::AuxStartLevel(slot), Action::Changed(v) | Action::Committed(v)) => {
                self.drafts[slot].value = level_q8(*v) as i16;
                PageEvent::Handled
            }
            _ => return None,
        })
    }

    /// Switch an output now: shown at once, confirmed by the status packet.
    fn write_aux_state(&mut self, slot: usize, on: bool) -> PageEvent {
        if slot >= self.aux.state.len() {
            return PageEvent::Handled;
        }
        self.aux.state[slot] = u8::from(on);
        self.aux_messages.remove(&slot);
        PageEvent::Session(SessionRequest::new(
            TAG_AUX_STATE | slot as u32,
            move |session| m::run_code(session, "", move |s| s.set_aux_state(slot as u8, on)),
        ))
    }

    /// Set a dimmable output's level now, in 8.8 percent.
    fn write_aux_level(&mut self, slot: usize, percent: f64) -> PageEvent {
        if slot >= self.aux.level_q8.len() {
            return PageEvent::Handled;
        }
        let q8 = level_q8(percent);
        self.aux.level_q8[slot] = q8;
        self.aux_messages.remove(&slot);
        PageEvent::Session(SessionRequest::new(
            TAG_AUX_LEVEL | slot as u32,
            move |session| m::run_code(session, "", move |s| s.set_aux_level(slot as u8, q8)),
        ))
    }

    /// A live write's answer. A refusal puts the device's own values back
    /// (the request re-read them on its way home) and says why, in the
    /// Console's words for status 0x26.
    pub(super) fn aux_result(&mut self, tag: u32, reply: SessionReply, cx: &Cx<'_>) -> PageEvent {
        let slot = (tag & !TAG_MASK) as usize;
        match reply {
            SessionReply::Err(why) => {
                if let Some(a) = cx.data.cs.as_ref().and_then(|c| c.aux.clone()) {
                    self.aux = a;
                }
                self.aux_messages.insert(slot, why.clone());
                PageEvent::Status(why)
            }
            _ => {
                self.aux_messages.remove(&slot);
                PageEvent::Handled
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::tests::{frame, key};
    use super::super::super::{AppConfig, Page, SettingsScreen};
    use super::*;
    use crate::shell::{Screen, ScreenEvent};
    use crossterm::event::KeyCode;
    use dspi_session::DeviceState;

    /// The demo device with only the aux outputs `keep` names left on it.
    fn data_with(keep: &[usize]) -> SettingsData {
        let mut d = m::demo::settings_data();
        let cs = d.cs.as_mut().expect("cs");
        for slot in [10usize, 11] {
            if !keep.contains(&slot) {
                cs.bindings[slot] = CsBinding::default();
                cs.names[slot] = String::new();
            }
        }
        d
    }

    fn screen(data: SettingsData) -> (SettingsScreen, DeviceState) {
        let st = m::demo::state();
        let s = SettingsScreen::new(&st, data, AppConfig::default()).open(Page::Aux, &st);
        (s, st)
    }

    /// Open the first card on the page and draw it.
    fn opened(data: SettingsData, w: u16, h: u16) -> String {
        let (mut s, st) = screen(data);
        s.handle(key(KeyCode::Tab), &st);
        s.handle(key(KeyCode::Enter), &st);
        frame(&mut s, &st, w, h)
    }

    fn page(data: &SettingsData) -> SurfacesPage {
        SurfacesPage::aux(data)
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

    fn text(p: &SurfacesPage, cx: &Cx<'_>) -> String {
        p.build(cx)
            .iter()
            .map(|(_, r)| format!("{r:?}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    // -------------------------------------------------------- golden frames

    #[test]
    fn an_empty_page_shows_the_consoles_empty_state_at_both_sizes() {
        let (mut s, st) = screen(data_with(&[]));
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            let f = frame(&mut s, &st, w, h);
            assert_eq!(f.lines().count(), h as usize, "{w}x{h}");
            assert!(
                f.contains("Auxiliary Outputs"),
                "the title at {w}x{h}:\n{f}"
            );
            assert!(f.contains("No Auxiliary Outputs Set Up"), "{w}x{h}:\n{f}");
            assert!(f.contains("Add Output"), "{w}x{h}:\n{f}");
            // The controls live on the other page, never here.
            assert!(!f.contains("Volume Knob"), "{w}x{h}:\n{f}");
        }
    }

    #[test]
    fn an_on_off_output_draws_the_consoles_rows_at_both_sizes() {
        let f = opened(data_with(&[10]), 120, 40);
        for want in [
            "Amp Trigger",
            "On/off output on GPIO 27, starts off.",
            "Component",
            "On/Off Output",
            "Output",
            LIVE,
            "GPIO 27",
            "Output pin driving the relay or MOSFET input.",
            "Active-Low Output",
            "Turn-On Delay",
            "Turn-Off Delay",
            "At Power-On",
            "Fixed",
            "Starts On",
        ] {
            assert!(f.contains(want), "missing {want:?}:\n{f}");
        }
        // An on/off output has no level, no ceiling and no curve.
        for absent in ["Level Limit", "Linear Response", "Starting Level"] {
            assert!(!f.contains(absent), "{absent:?} on an on/off output:\n{f}");
        }
        let f = opened(data_with(&[10]), 80, 24);
        assert_eq!(f.lines().count(), 24);
        assert!(f.contains("Amp Trigger"), "{f}");
        assert!(f.contains("Output"), "{f}");
    }

    #[test]
    fn a_dimmable_output_draws_the_consoles_rows_at_both_sizes() {
        // The card's rows run past a 40-row pane, so it is drawn tall enough
        // to hold them, and again at the two sizes for the top of it.
        let f = opened(data_with(&[11]), 120, 70);
        for want in [
            "Dimmable output on GPIO 28, starts on.",
            "Dimmable Output",
            "Level",
            "50%",
            "PWM output pin driving the dimmer or fan input.",
            "Active-Low Output",
            "Level Limit",
            "80%",
            "Linear Response",
            "At Power-On",
            "Starts On",
            "Starting Level",
            "Driven By",
            "Nothing yet. Add a button on \"Aux Switch\"",
        ] {
            assert!(f.contains(want), "missing {want:?}:\n{f}");
        }
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            let f = opened(data_with(&[11]), w, h);
            assert_eq!(f.lines().count(), h as usize, "{w}x{h}");
            assert!(f.contains("Dimmable Output"), "{w}x{h}:\n{f}");
            assert!(f.contains("Level"), "{w}x{h}:\n{f}");
        }
    }

    /// The Control Surfaces page lists neither aux output, and its Add
    /// Control menu never offers the two aux types.
    #[test]
    fn the_control_surfaces_page_leaves_the_aux_outputs_out() {
        let d = m::demo::settings_data();
        let p = SurfacesPage::new(&d);
        assert!(!p.visible().contains(&10) && !p.visible().contains(&11));
        assert!(
            p.visible().contains(&12),
            "the button that drives one stays"
        );
        let types = p.addable_types();
        assert!(!types.contains(&m::ty::AUX_OUT) && !types.contains(&m::ty::AUX_PWM));
        assert!(types.contains(&m::ty::BUTTON));
        // And this page offers only the two aux types, in both lists.
        let a = page(&d);
        assert_eq!(a.visible(), vec![10, 11]);
        assert_eq!(a.addable_types(), vec![m::ty::AUX_OUT, m::ty::AUX_PWM]);
        assert_eq!(a.type_options(10), vec![m::ty::AUX_OUT, m::ty::AUX_PWM]);
        assert!(!p.type_options(12).contains(&m::ty::AUX_OUT));
    }

    #[test]
    fn adding_an_output_offers_the_two_kinds_and_applies_at_once() {
        let (mut s, st) = screen(data_with(&[]));
        s.handle(key(KeyCode::Tab), &st);
        match s.handle(key(KeyCode::Enter), &st) {
            ScreenEvent::Popup(p) => {
                assert_eq!(p.title, "Add Output");
                assert_eq!(p.items, vec!["On/Off Output", "Dimmable Output"]);
            }
            other => panic!("{other:?}"),
        }
        match s.popup_result(Some(1), &st) {
            ScreenEvent::Session(r) => assert_eq!(r.tag, 5, "the first free slot"),
            other => panic!("{other:?}"),
        }
        let b = &s.pages.aux.drafts[5];
        assert_eq!(b.component, m::ty::AUX_PWM);
        assert_eq!((b.noun, b.action, b.extras, b.value), (0, 0, 0, 0));
        assert_eq!(b.gpio[1], dspi_session::surfaces::GPIO_UNUSED);
    }

    // ------------------------------------------------------------ the live

    /// The switch goes out at once as its own write, and the save bar does
    /// not count it: an aux SET is neither a preview nor unsaved.
    #[test]
    fn the_live_switch_is_immediate_and_never_unsaved() {
        let (d, st, cfg) = (
            m::demo::settings_data(),
            m::demo::state(),
            AppConfig::default(),
        );
        let mut p = page(&d);
        p.expanded.insert(10);
        let c = cx(&d, &st, &cfg);
        match p.act(
            index_of(&p, &c, Item::AuxLive(10)),
            Action::Toggled(false),
            &c,
        ) {
            PageEvent::Session(r) => assert_eq!(r.tag, TAG_AUX_STATE | 10),
            other => panic!("{other:?}"),
        }
        assert!(!p.aux.is_on(10), "shown at once");
        assert!(!p.dirty(10), "the binding did not change");

        let (mut s, st) = screen(m::demo::settings_data());
        s.session_result(TAG_AUX_STATE | 10, SessionReply::Ok(String::new()), &st);
        assert!(!s.dirty(&st), "a live switch is not an unsaved change");
        s.session_result(10, SessionReply::Ok("Applied".into()), &st);
        assert!(s.dirty(&st), "an applied binding still is");
    }

    #[test]
    fn the_level_goes_out_as_eight_eight_percent() {
        use dspi_proto::generated::opcodes as op;
        use dspi_transport::MockTransport;

        let (d, st, cfg) = (
            m::demo::settings_data(),
            m::demo::state(),
            AppConfig::default(),
        );
        let mut p = page(&d);
        p.expanded.insert(11);
        let c = cx(&d, &st, &cfg);
        let ev = p.act(
            index_of(&p, &c, Item::AuxLevel(11)),
            Action::Committed(75.0),
            &c,
        );
        let PageEvent::Session(req) = ev else {
            panic!("{ev:?}");
        };
        assert_eq!(p.aux.level_q8[11], 75 * 256);

        let mut caps = crate::shell::fixture::caps();
        caps.cs = Some(m::demo::caps());
        let t = MockTransport::new()
            .data(op::REQ_SET_CS_AUX_LEVEL, vec![])
            .data(op::REQ_GET_CS_STATUS, p.live.status.encode());
        let log = t.log_handle();
        let mut session = dspi_session::Session::new(Box::new(t), caps).expect("session");
        assert!(matches!((req.run)(&mut session), SessionReply::Ok(_)));
        let set = log
            .lock()
            .expect("log")
            .iter()
            .find(|e| e.opcode == op::REQ_SET_CS_AUX_LEVEL)
            .cloned()
            .expect("the SET");
        assert_eq!(set.value, 11);
        assert_eq!(set.payload, (75u16 * 256).to_le_bytes().to_vec());
    }

    /// Status 0x26 comes back as the Console's sentence, on the card and on
    /// the echo line, and the device's own values go back up.
    #[test]
    fn a_refused_live_write_says_so_in_the_consoles_words() {
        let (d, st, cfg) = (
            m::demo::settings_data(),
            m::demo::state(),
            AppConfig::default(),
        );
        let mut p = page(&d);
        p.expanded.insert(11);
        let c = cx(&d, &st, &cfg);
        p.act(
            index_of(&p, &c, Item::AuxLevel(11)),
            Action::Committed(10.0),
            &c,
        );
        let why = m::capitalise(&dspi_session::surfaces::explain_status(0x26));
        assert_eq!(
            why,
            "The target isn't an auxiliary output, or a level control needs a dimmable one"
        );
        let ev = p.session_result(TAG_AUX_LEVEL | 11, SessionReply::Err(why.clone()), &c);
        assert_eq!(ev, PageEvent::Status(why.clone()));
        assert_eq!(p.aux.level_q8[11], 50 * 256, "the device's level is back");
        assert!(text(&p, &c).contains(&why), "{}", text(&p, &c));
    }

    /// The switch only works on an output that is running, so an unapplied
    /// or inactive slot shows it disabled with the Console's caption.
    #[test]
    fn the_switch_waits_for_the_output_to_run() {
        let mut d = m::demo::settings_data();
        d.cs.as_mut().expect("cs").status.active_mask &= !(1 << 10);
        let (st, cfg) = (m::demo::state(), AppConfig::default());
        let mut p = page(&d);
        p.expanded.insert(10);
        let c = cx(&d, &st, &cfg);
        let rows = p.build(&c);
        let Some((
            _,
            Row::Toggle {
                enabled, caption, ..
            },
        )) = rows.iter().find(|(i, _)| *i == Some(Item::AuxLive(10)))
        else {
            panic!("no switch");
        };
        assert!(!enabled);
        assert_eq!(caption.as_deref(), Some(NOT_LIVE));
    }

    /// A notification moves the switch and level; the first look only notes
    /// what came before Settings opened.
    #[test]
    fn a_notification_moves_the_switch_and_the_level() {
        let (d, mut st, cfg) = (
            m::demo::settings_data(),
            m::demo::state(),
            AppConfig::default(),
        );
        let mut p = page(&d);
        st.cs_aux = Some((11, 0, 0));
        p.observe(&cx(&d, &st, &cfg));
        assert!(p.aux.is_on(11), "a stale event is not taken on");
        st.cs_aux = Some((11, 1, 20 * 256));
        p.observe(&cx(&d, &st, &cfg));
        assert_eq!(p.aux.level_q8[11], 20 * 256);
        st.cs_aux = Some((10, 0, 0));
        p.observe(&cx(&d, &st, &cfg));
        assert!(!p.aux.is_on(10));
        // A poll that read the same block again does not undo it.
        p.observe(&cx(&d, &st, &cfg));
        assert!(!p.aux.is_on(10));
    }

    // ------------------------------------------------------------ the extras

    /// The power-on rows edit `extras` and the boot level, and each one
    /// reaches byte 22 or bytes 10-11 of the record Apply sends.
    #[test]
    fn the_power_on_rows_write_the_extras_bits() {
        let (d, st, cfg) = (
            m::demo::settings_data(),
            m::demo::state(),
            AppConfig::default(),
        );
        let mut p = page(&d);
        p.expanded.insert(11);
        let c = cx(&d, &st, &cfg);
        assert_eq!(p.drafts[11].extras, m::aux_extras::BOOT_ON);
        p.act(
            index_of(&p, &c, Item::AuxLinear(11)),
            Action::Toggled(true),
            &c,
        );
        p.act(
            index_of(&p, &c, Item::AuxStartLevel(11)),
            Action::Committed(30.0),
            &c,
        );
        p.act(
            index_of(&p, &c, Item::AuxStartsOn(11)),
            Action::Toggled(false),
            &c,
        );
        let b = &p.drafts[11];
        assert_eq!(b.extras, m::aux_extras::LINEAR);
        assert_eq!(b.value, 30 * 256, "8.8 percent");
        // As Last Saved hides Starts On and Starting Level behind a note.
        p.act(index_of(&p, &c, Item::AuxBoot(11)), Action::Selected(1), &c);
        assert_eq!(
            p.drafts[11].extras,
            m::aux_extras::LINEAR | m::aux_extras::BOOT_SAVED
        );
        let t = text(&p, &c);
        assert!(t.contains("Saving takes a copy"), "{t}");
        assert!(!t.contains("Starting Level"), "{t}");
        let w = p.drafts[11].encode();
        assert_eq!(w[22], 0x06, "LINEAR | BOOT_SAVED in byte 22");
        assert_eq!(w[23], 0, "reserved2");
        assert_eq!(&w[10..12], &(30i16 * 256).to_le_bytes(), "the boot level");
        assert!(p.dirty(11));
    }

    /// The live level and the boot level both stop at 100 %.
    #[test]
    fn a_level_is_clamped_to_one_hundred_percent() {
        assert_eq!(level_q8(150.0), 25_600);
        assert_eq!(level_q8(-5.0), 0);
        assert_eq!(level_q8(50.0), 12_800);
    }

    /// Changing an output's kind keeps its pin, sense, delays and power-on
    /// switch, and drops what only the dimmable kind may carry.
    #[test]
    fn changing_kind_keeps_the_wiring_and_drops_the_level() {
        let d = m::demo::settings_data();
        let b = CsBinding {
            flags: m::flag::INVERT,
            on_delay: 30,
            extras: m::aux_extras::BOOT_ON | m::aux_extras::LINEAR,
            ..d.cs.as_ref().expect("cs").bindings[11].clone()
        };
        let out = m::switch_aux_kind(&b, m::ty::AUX_OUT);
        assert_eq!(out.component, m::ty::AUX_OUT);
        assert_eq!(out.gpio, b.gpio);
        assert_eq!((out.flags, out.on_delay), (m::flag::INVERT, 30));
        assert_eq!(out.extras, m::aux_extras::BOOT_ON, "LINEAR is PWM only");
        assert_eq!((out.value, out.base_bright), (0, 0));
        let back = m::switch_aux_kind(&out, m::ty::AUX_PWM);
        assert_eq!(back.extras, m::aux_extras::BOOT_ON);
    }

    // ------------------------------------------------------------ driven by

    #[test]
    fn driven_by_names_the_controls_remotes_and_macros_that_point_here() {
        let (mut d, st, cfg) = (
            m::demo::settings_data(),
            m::demo::state(),
            AppConfig::default(),
        );
        {
            let cs = d.cs.as_mut().expect("cs");
            cs.ir[1] = IrCommand {
                noun: m::noun::AUX,
                action: m::act::TOGGLE,
                protocol: 1,
                target: 10,
                code: 0x1234,
                ..Default::default()
            };
            cs.macros[0].steps[1] = dspi_session::surfaces::CsMacroStep {
                noun: m::noun::AUX,
                action: m::act::SET,
                target: 10,
                value: 1,
                ..Default::default()
            };
        }
        let mut p = page(&d);
        p.expanded.insert(10);
        let t = text(&p, &cx(&d, &st, &cfg));
        assert!(t.contains("Push Button - Toggle on Aux Switch"), "{t}");
        assert!(
            t.contains("Also driven by 1 remote key and the macro Night."),
            "{t}"
        );
        // The dimmable output is driven by nothing.
        p.expanded.insert(11);
        let rows = p.build(&cx(&d, &st, &cfg));
        assert!(
            rows.iter().any(|(_, r)| *r == Row::note(NOTHING_DIMMABLE)),
            "{rows:?}"
        );
    }

    // ------------------------------------------------------ extras survive

    /// Every path that decodes, edits and re-encodes an aux binding keeps
    /// byte 22: Apply, a Revert, a device refresh, and each Control Surfaces
    /// helper a record can pass through.
    #[test]
    fn extras_survive_every_edit_path() {
        use dspi_proto::generated::opcodes as op;
        use dspi_transport::MockTransport;

        let (d, st, cfg) = (
            m::demo::settings_data(),
            m::demo::state(),
            AppConfig::default(),
        );
        let cs = d.cs.clone().expect("cs");
        let lamp = cs.bindings[11].clone();
        assert_ne!(lamp.extras, 0);

        // The model's helpers leave an aux record's extras alone.
        assert_eq!(m::default_operands(&cs, &lamp), lamp);
        assert_eq!(m::sanitize_group_flags(&cs, &lamp).extras, lamp.extras);
        // And clear it on everything else.
        let button = CsBinding {
            extras: 0x05,
            ..cs.bindings[1].clone()
        };
        assert_eq!(m::default_operands(&cs, &button).extras, 0);

        // An edit on the card, then Apply: the record on the wire has it.
        let mut p = page(&d);
        p.expanded.insert(11);
        let c = cx(&d, &st, &cfg);
        p.act(
            index_of(&p, &c, Item::Flag(11, m::flag::INVERT)),
            Action::Toggled(true),
            &c,
        );
        let PageEvent::Session(req) = p.apply(11) else {
            panic!("no apply");
        };
        let mut caps = crate::shell::fixture::caps();
        caps.cs = Some(m::demo::caps());
        let mut ok = cs.status.clone();
        ok.last_slot = 11;
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
            .expect("the binding SET");
        assert_eq!(sent.payload[22], lamp.extras, "byte 22 went out");
        assert_eq!(sent.payload[3], m::flag::INVERT);

        // Revert puts the device's record back, extras and all.
        let mut p = page(&d);
        p.expanded.insert(11);
        p.drafts[11].extras = 0;
        p.apply_button.insert(11, 0);
        p.act(index_of(&p, &c, Item::Apply(11)), Action::Open, &c);
        assert_eq!(p.drafts[11], lamp);

        // A device refresh brings a clean card the device's record, extras
        // included, and leaves a card with a staged edit alone.
        let mut moved = d.clone();
        let mc = moved.cs.as_mut().expect("cs");
        mc.bindings[11].extras = m::aux_extras::BOOT_SAVED;
        mc.bindings[10].extras = m::aux_extras::BOOT_ON;
        let mut p = page(&d);
        p.drafts[10].flags = m::flag::INVERT; // staged
        p.observe(&cx(&moved, &st, &cfg));
        assert_eq!(p.drafts[11].extras, m::aux_extras::BOOT_SAVED);
        assert_eq!(p.drafts[10].extras, 0, "a staged card keeps its draft");
        assert_eq!(p.live.bindings[10].extras, m::aux_extras::BOOT_ON);

        // The command line writes whole records, and prints extras so a
        // dump can be replayed.
        let fields = lamp.to_fields();
        let pairs: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (*k, v.as_str())).collect();
        assert_eq!(CsBinding::from_fields(&pairs).unwrap(), lamp);
    }
}
