//! Settings > I2S Configuration.
//!
//! The clock half of the I2S wiring: the bit clock pin (the word clock always
//! sits one pin above it), whether the master and slave roles share that pair,
//! the master clock, and the rate the device runs at when it is the clock
//! authority. Hidden on a platform whose clock pins are not assignable.

use std::rc::Rc;

use dspi_proto::value::Value;

use crate::shell::SessionReply;
use crate::widgets::{Action, KeyHelp, PopupList};

use super::pin_status as st;
use super::{Cx, Explain, INPUT_RATES_HZ, PageEvent, Row, SettingsPage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Bck,
    ClockPins,
    SlaveBck,
    Mck,
    MckPin,
    MckMult,
    Rate,
}

#[derive(Debug, Default)]
pub struct I2sPage {
    cursor: usize,
    popup_item: Option<Item>,
    status: Option<(String, bool)>,
}

const KEYS: &[KeyHelp] = &[
    KeyHelp::new("↑ ↓", "Move between controls"),
    KeyHelp::new("← →", "Change"),
    KeyHelp::new("Enter", "Open the list"),
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

impl I2sPage {
    /// The live sample rate, from the input configuration's rate code.
    fn rate_hz(cx: &Cx<'_>) -> u32 {
        cx.state
            .input_config()
            .and_then(|c| INPUT_RATES_HZ.get(c.i2s_input_rate as usize).copied())
            .unwrap_or(48_000)
    }

    /// Whether the device is actually clocking as an I2S slave: the mode says
    /// slave and I2S is the live source.
    fn slave_active(cx: &Cx<'_>) -> bool {
        cx.state
            .input_config()
            .is_some_and(|c| c.i2s_clock_mode == 1 && c.input_source == 2)
    }

    /// A BCK candidate needs both its own pin and the LRCLK above it free.
    fn bck_candidates(cx: &Cx<'_>, owner: &str, current: u8) -> Vec<u8> {
        dspi_session::pins::valid_pins(cx.platform())
            .into_iter()
            .filter(|p| {
                *p == current
                    || (cx.data.owner_of(cx.state, *p, owner).is_none()
                        && cx
                            .data
                            .owner_of(cx.state, p.wrapping_add(1), owner)
                            .is_none())
            })
            .collect()
    }

    /// The `CLK_GPOUT` pins, minus GPIO 15 while a slot is I2S: it is that
    /// slot's LRCLK, so the firmware would always refuse it.
    fn mck_candidates(cx: &Cx<'_>) -> Vec<u8> {
        let any_i2s = cx.state.i2s().output_types.contains(&1);
        dspi_session::pins::mck_pins(cx.platform())
            .into_iter()
            .filter(|p| !(any_i2s && *p == 15))
            .filter(|p| {
                *p == cx.state.i2s().mck_pin || cx.data.owner_of(cx.state, *p, "I2S MCK").is_none()
            })
            .collect()
    }

    fn build(&self, cx: &Cx<'_>) -> Vec<(Option<Item>, Row)> {
        let i2s = cx.state.i2s();
        let any_i2s = i2s.output_types.contains(&1);
        let slave = Self::slave_active(cx);
        let rate = Self::rate_hz(cx);
        let mut rows: Vec<(Option<Item>, Row)> = vec![(None, Row::section("I2S Clock"))];

        let bck = Self::bck_candidates(cx, "I2S BCK", i2s.bck_pin);
        rows.push((
            Some(Item::Bck),
            Row::Pick {
                label: "BCK Pin".into(),
                choices: bck.iter().map(|p| format!("GPIO {p}")).collect(),
                selected: bck.iter().position(|p| *p == i2s.bck_pin).unwrap_or(0),
                caption: Some(format!(
                    "LRCK: GPIO {} (BCK + 1)",
                    i2s.bck_pin.wrapping_add(1)
                )),
                // The firmware refuses a move while an I2S output is running.
                enabled: !any_i2s && cx.connected,
            },
        ));

        if let Some(mode) = i2s.clock_pin_mode {
            rows.push((
                Some(Item::ClockPins),
                Row::Pick {
                    label: "Clock Pins".into(),
                    choices: vec!["Unified".into(), "Split".into()],
                    selected: usize::from(mode == 1),
                    caption: Some(
                        "Unified: Master and Slave modes share pins. Split: Separate pins for \
                         Master and Slave modes."
                            .into(),
                    ),
                    enabled: cx.connected,
                },
            ));
            let slave_pins = Self::bck_candidates(cx, "I2S Slave BCK", i2s.bck_pin_slave);
            rows.push((
                Some(Item::SlaveBck),
                Row::Pick {
                    label: "Slave BCK Pin".into(),
                    choices: slave_pins.iter().map(|p| format!("GPIO {p}")).collect(),
                    selected: slave_pins
                        .iter()
                        .position(|p| *p == i2s.bck_pin_slave)
                        .unwrap_or(0),
                    caption: Some(if mode == 1 {
                        format!("LRCK: GPIO {} (BCK + 1)", i2s.bck_pin_slave.wrapping_add(1))
                    } else {
                        "Stored but inactive while clock pins are shared.".to_string()
                    }),
                    // Editable in Split, where the pair is live; in Unified it
                    // is stored but inactive and the Console disables it.
                    enabled: mode == 1 && cx.connected,
                },
            ));
        }

        rows.push((
            Some(Item::Mck),
            Row::Toggle {
                label: "Master Clock (MCK)".into(),
                on: i2s.mck_enabled,
                caption: Some(if slave {
                    "Forced off in I2S slave mode; a local MCK would be asynchronous to the \
                     external clocks. Kept and restored when leaving slave mode."
                        .into()
                } else {
                    "Clock reference for external DACs".to_string()
                }),
                enabled: !slave && cx.connected,
            },
        ));
        let mck = Self::mck_candidates(cx);
        rows.push((
            Some(Item::MckPin),
            Row::Pick {
                label: "MCK Pin".into(),
                choices: mck.iter().map(|p| format!("GPIO {p}")).collect(),
                selected: mck.iter().position(|p| *p == i2s.mck_pin).unwrap_or(0),
                caption: None,
                // The firmware answers OUTPUT_ACTIVE for a move while MCK runs.
                enabled: !i2s.mck_enabled && !slave && !mck.is_empty() && cx.connected,
            },
        ));
        let locked_128 = rate >= 96_000;
        rows.push((
            Some(Item::MckMult),
            Row::Pick {
                label: "MCK Multiplier".into(),
                choices: vec!["128x".into(), "256x".into()],
                selected: usize::from(i2s.mck_multiplier == 1),
                caption: locked_128.then(|| format!("Locked to 128x at {}", rate_label(rate))),
                enabled: !locked_128 && !slave && cx.connected,
            },
        ));

        if cx.feature("i2s_input_channels") {
            rows.push((
                Some(Item::Rate),
                Row::Pick {
                    label: "Input Sample Rate".into(),
                    choices: INPUT_RATES_HZ.iter().map(|h| rate_label(*h)).collect(),
                    selected: INPUT_RATES_HZ.iter().position(|h| *h == rate).unwrap_or(1),
                    caption: Some(if slave {
                        format!(
                            "Rate used in master mode. In slave mode the external master sets \
                             the rate (detected: {}).",
                            rate_label(cx.state.i2s_slave_state.map(|(_, r)| r).unwrap_or(0))
                        )
                    } else {
                        "Rate for I2S input; DSPi drives the clocks, so the source must follow."
                            .to_string()
                    }),
                    enabled: !slave && cx.connected,
                },
            ));
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

    /// The Console's status row for one clock write: the device answers a
    /// `PIN_CONFIG_*` code and each row has its own sentence for each of them.
    fn explain(item: Item, pin: u8, slave: u8, split: bool, rate: u32) -> Rc<Explain> {
        let lrclk = pin.wrapping_add(1);
        Rc::new(move |code| match (item, code) {
            (Item::Bck, st::SUCCESS) => (
                format!("BCK pin set to GPIO {pin}, LRCLK = GPIO {lrclk}"),
                false,
            ),
            (Item::Bck, st::OUTPUT_ACTIVE) => (
                "All outputs must be S/PDIF before changing BCK pin".into(),
                true,
            ),
            (Item::Bck, st::PIN_IN_USE) => {
                (format!("GPIO {pin} or {lrclk} is already in use"), true)
            }
            (Item::Bck, _) => ("Failed to set BCK pin".into(), true),

            (Item::SlaveBck, st::SUCCESS) => (
                format!("Slave BCK pin set to GPIO {pin}, LRCLK = GPIO {lrclk}"),
                false,
            ),
            (Item::SlaveBck, st::PIN_IN_USE) => {
                (format!("GPIO {pin} or {lrclk} is already in use"), true)
            }
            (Item::SlaveBck, st::OUTPUT_ACTIVE) => (
                "Can't move the slave clock pins while an I2S output is active".into(),
                true,
            ),
            (Item::SlaveBck, st::INVALID_PIN) => {
                (format!("GPIO {pin} isn't available on this device"), true)
            }
            (Item::SlaveBck, _) => ("Failed to set slave BCK pin".into(), true),

            (Item::ClockPins, st::SUCCESS) if split => (
                format!(
                    "Separate clock pins - slave uses GPIO {slave}/{}",
                    slave.wrapping_add(1)
                ),
                false,
            ),
            (Item::ClockPins, st::SUCCESS) => {
                ("Shared clock pins for master and slave".into(), false)
            }
            (Item::ClockPins, st::PIN_IN_USE) => (
                format!(
                    "Slave clock pair GPIO {slave}/{} conflicts with another function - move it \
                     first",
                    slave.wrapping_add(1)
                ),
                true,
            ),
            (Item::ClockPins, st::OUTPUT_ACTIVE) => (
                "Switch I2S output slots to S/PDIF before changing clock pins".into(),
                true,
            ),
            (Item::ClockPins, _) => ("Failed to change clock pins".into(), true),

            (Item::MckPin, st::SUCCESS) => (format!("MCK pin set to GPIO {pin}"), false),
            (Item::MckPin, st::OUTPUT_ACTIVE) => {
                ("Disable MCK before changing its pin".into(), true)
            }
            (Item::MckPin, st::PIN_IN_USE) => (format!("GPIO {pin} is already in use"), true),
            (Item::MckPin, _) => ("Failed to set MCK pin".into(), true),

            (Item::Mck, st::SUCCESS) => (
                format!(
                    "Master clock {}",
                    if split { "enabled" } else { "disabled" }
                ),
                false,
            ),
            (Item::Mck, _) => ("Failed to set the master clock".into(), true),

            (Item::MckMult, st::SUCCESS) => (
                format!(
                    "MCK multiplier set to {}",
                    if split { "256x" } else { "128x" }
                ),
                false,
            ),
            (Item::MckMult, _) => ("Failed to set the MCK multiplier".into(), true),

            (Item::Rate, st::SUCCESS) => (
                format!("Input sample rate set to {}", rate_label(rate)),
                false,
            ),
            (Item::Rate, _) => ("Failed to set the input sample rate".into(), true),
        })
    }

    fn write(item: Item, path: &'static str, value: Value, explain: Rc<Explain>) -> PageEvent {
        PageEvent::IoSession(super::device_write(
            item as u32,
            path,
            Vec::new(),
            value,
            explain,
        ))
    }

    fn apply(&mut self, item: Item, choice: usize, cx: &Cx<'_>) -> PageEvent {
        let i2s = cx.state.i2s();
        self.status = None;
        match item {
            Item::Bck => {
                let pins = Self::bck_candidates(cx, "I2S BCK", i2s.bck_pin);
                let Some(p) = pins.get(choice).copied() else {
                    return PageEvent::Handled;
                };
                let e = Self::explain(item, p, i2s.bck_pin_slave, false, 0);
                Self::write(item, "i2s.bck", Value::Int(p as i64), e)
            }
            Item::ClockPins => {
                let split = choice == 1;
                let e = Self::explain(item, 0, i2s.bck_pin_slave, split, 0);
                Self::write(item, "i2s.clockpins", Value::Choice(u8::from(split)), e)
            }
            Item::SlaveBck => {
                let pins = Self::bck_candidates(cx, "I2S Slave BCK", i2s.bck_pin_slave);
                let Some(p) = pins.get(choice).copied() else {
                    return PageEvent::Handled;
                };
                let e = Self::explain(item, p, p, false, 0);
                Self::write(item, "i2s.bck.slave", Value::Int(p as i64), e)
            }
            Item::MckPin => {
                let pins = Self::mck_candidates(cx);
                let Some(p) = pins.get(choice).copied() else {
                    return PageEvent::Handled;
                };
                let e = Self::explain(item, p, 0, false, 0);
                Self::write(item, "i2s.mck.pin", Value::Int(p as i64), e)
            }
            Item::MckMult => {
                let x256 = choice == 1;
                let e = Self::explain(item, 0, 0, x256, 0);
                Self::write(item, "i2s.mck.mult", Value::Choice(u8::from(x256)), e)
            }
            Item::Rate => {
                let Some(hz) = INPUT_RATES_HZ.get(choice).copied() else {
                    return PageEvent::Handled;
                };
                let e = Self::explain(item, 0, 0, false, hz);
                Self::write(item, "in.rate", Value::Int(hz as i64), e)
            }
            Item::Mck => PageEvent::Handled,
        }
    }
}

impl SettingsPage for I2sPage {
    fn rows(&self, cx: &Cx<'_>) -> Vec<Row> {
        self.build(cx).into_iter().map(|(_, r)| r).collect()
    }

    fn act(&mut self, index: usize, action: Action, cx: &Cx<'_>) -> PageEvent {
        let Some(item) = self.item(index, cx) else {
            return PageEvent::Handled;
        };
        match (item, action) {
            (Item::Mck, Action::Toggled(on)) => {
                self.status = None;
                let e = Self::explain(item, 0, 0, on, 0);
                Self::write(item, "i2s.mck", Value::Bool(on), e)
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

    fn session_result(&mut self, _tag: u32, reply: SessionReply, _cx: &Cx<'_>) -> PageEvent {
        self.status = match reply {
            SessionReply::Ok(m) => Some((m, false)),
            SessionReply::Err(m) => Some((m, true)),
            SessionReply::Bytes(_) => None,
        };
        match &self.status {
            Some((m, _)) => PageEvent::Status(m.clone()),
            None => PageEvent::Handled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{data, frame, key, screen, state};
    use super::super::{AppConfig, Page, SettingsScreen};
    use super::*;
    use crate::shell::{Screen, ScreenEvent, SessionRequest};
    use crossterm::event::KeyCode;
    use dspi_transport::MockTransport;

    /// Run a page's write against a device that answers `code`, and hand back
    /// what the row was told. Every clock setter is a write-as-read, so the
    /// answer is the status byte and not the transfer's success.
    fn answered(req: &SessionRequest, code: u8) -> SessionReply {
        let t = MockTransport::new().answering_everything(vec![code]);
        let mut session = dspi_session::Session::new(Box::new(t), crate::shell::fixture::caps())
            .expect("session");
        (req.run)(&mut session)
    }

    fn request(ev: ScreenEvent) -> SessionRequest {
        match ev {
            ScreenEvent::Session(r) => r,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_page_carries_the_clock_pins_and_the_master_clock() {
        let (mut s, st) = screen(Page::I2s);
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            let f = frame(&mut s, &st, w, h);
            assert_eq!(f.lines().count(), h as usize);
            assert!(f.contains("I2S CLOCK"), "{w}x{h}: {f}");
            assert!(f.contains("BCK Pin"), "{w}x{h}: {f}");
        }
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("LRCK: GPIO 15 (BCK + 1)"), "{f}");
        assert!(f.contains("Clock Pins"), "{f}");
        assert!(
            f.contains("Unified: Master and Slave modes share pins."),
            "{f}"
        );
        assert!(f.contains("Slave BCK Pin"), "{f}");
    }

    /// A split-mode device has two clock pairs, and the slave one is only
    /// reachable through role 1 of `REQ_SET_I2S_BCK_PIN` (config.h:485).
    #[test]
    fn the_slave_bck_pin_is_editable_in_split_mode() {
        let sec = dspi_proto::generated::SECTIONS
            .iter()
            .find(|(n, _, _)| *n == "i2s_config")
            .map(|(_, o, _)| *o)
            .unwrap();
        // clock_pin_mode_p1 = 2 is SPLIT (bulk_params.h:160).
        let mut st = state();
        st.bulk.patch(sec + 8, &[2]);
        let mut s = SettingsScreen::new(&st, data(), AppConfig::default()).open(Page::I2s, &st);
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("Slave BCK Pin"), "{f}");
        assert!(f.contains("LRCK: GPIO 21 (BCK + 1)"), "the live pair:\n{f}");

        // The row is the third focusable one: BCK, Clock Pins, Slave BCK.
        s.handle(key(KeyCode::Tab), &st);
        s.handle(key(KeyCode::Down), &st);
        s.handle(key(KeyCode::Down), &st);
        let req = request(s.handle(key(KeyCode::Right), &st));
        assert!(
            s.output_dirty(),
            "moving a clock pin is an output-config edit"
        );

        let reply = answered(&req, super::super::pin_status::SUCCESS);
        s.session_result(req.tag, reply, &st);
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("Slave BCK pin set to GPIO"), "{f}");
        assert!(f.contains("LRCLK = GPIO"), "{f}");

        // Role 1 of REQ_SET_I2S_BCK_PIN, with the GPIO in the low byte.
        let t = MockTransport::new().answering_everything(vec![0]);
        let log = t.log_handle();
        let mut session = dspi_session::Session::new(Box::new(t), crate::shell::fixture::caps())
            .expect("session");
        (req.run)(&mut session);
        let set = log
            .lock()
            .unwrap()
            .iter()
            .find(|e| e.opcode == dspi_proto::generated::opcodes::REQ_SET_I2S_BCK_PIN)
            .cloned()
            .expect("one BCK write");
        assert_eq!(set.value >> 8, 1, "the slave role: {:#06X}", set.value);
    }

    /// In Unified the pair is stored but dormant, which is what the Console
    /// says and why it disables the row there.
    #[test]
    fn the_slave_bck_row_is_inert_while_the_clock_pins_are_shared() {
        let (mut s, st) = screen(Page::I2s);
        let f = frame(&mut s, &st, 120, 40);
        assert!(
            f.contains("Stored but inactive while clock pins are shared."),
            "{f}"
        );
        s.handle(key(KeyCode::Tab), &st);
        s.handle(key(KeyCode::Down), &st);
        s.handle(key(KeyCode::Down), &st);
        assert_eq!(s.handle(key(KeyCode::Right), &st), ScreenEvent::Unhandled);
    }

    #[test]
    fn the_master_clock_rows_are_reachable_and_write() {
        let (mut s, st) = screen(Page::I2s);
        s.handle(key(KeyCode::Tab), &st);
        for _ in 0..3 {
            s.handle(key(KeyCode::Down), &st);
        }
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("Master Clock (MCK)"), "{f}");
        assert!(f.contains("MCK Multiplier"), "{f}");
        assert!(f.contains("Input Sample Rate"), "{f}");
        let req = request(s.handle(key(KeyCode::Char(' ')), &st));
        assert!(s.output_dirty());
        let reply = answered(&req, super::super::pin_status::SUCCESS);
        s.session_result(req.tag, reply, &st);
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("Master clock disabled"), "{f}");
    }

    /// The firmware answers a `PIN_CONFIG_*` code and keeps the old value on a
    /// refusal (config.h:606-612). The row used to claim success regardless,
    /// so a move the device had rejected read as done.
    #[test]
    fn a_refused_clock_move_says_why_instead_of_claiming_success() {
        // The BCK pin only moves while every slot is S/PDIF, so free slot 2.
        let sec = dspi_proto::generated::SECTIONS
            .iter()
            .find(|(n, _, _)| *n == "i2s_config")
            .map(|(_, o, _)| *o)
            .unwrap();
        let spdif_only = || {
            let mut st = state();
            st.bulk.patch(sec + 1, &[0]);
            st
        };
        let st = spdif_only();
        let mut s = SettingsScreen::new(&st, data(), AppConfig::default()).open(Page::I2s, &st);
        s.handle(key(KeyCode::Tab), &st);
        let req = request(s.handle(key(KeyCode::Right), &st));

        let reply = answered(&req, super::super::pin_status::OUTPUT_ACTIVE);
        assert_eq!(
            reply,
            SessionReply::Err("All outputs must be S/PDIF before changing BCK pin".into())
        );
        s.session_result(req.tag, reply, &st);
        let f = frame(&mut s, &st, 120, 40);
        assert!(
            f.contains("All outputs must be S/PDIF before changing BCK pin"),
            "{f}"
        );
        assert!(!f.contains("BCK pin set to GPIO"), "{f}");

        // And the other reasons the firmware can give.
        let st = spdif_only();
        let mut s = SettingsScreen::new(&st, data(), AppConfig::default()).open(Page::I2s, &st);
        s.handle(key(KeyCode::Tab), &st);
        let req = request(s.handle(key(KeyCode::Right), &st));
        let reply = answered(&req, super::super::pin_status::PIN_IN_USE);
        s.session_result(req.tag, reply, &st);
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("is already in use"), "{f}");
    }

    #[test]
    fn the_mck_picker_only_offers_clock_capable_pins() {
        let st = state();
        let d = data();
        let cx = Cx {
            state: &st,
            data: &d,
            config: &AppConfig::default(),
            connected: true,
            global_dirty: false,
        };
        let pins = I2sPage::mck_candidates(&cx);
        // The fixture has an I2S slot, so GPIO 15 (an LRCLK) is dropped, and
        // GPIO 21 is the second S/PDIF receiver's.
        assert_eq!(pins, vec![13]);
    }

    #[test]
    fn the_bck_picker_needs_both_its_pin_and_the_one_above_it() {
        let st = state();
        let d = data();
        let cx = Cx {
            state: &st,
            data: &d,
            config: &AppConfig::default(),
            connected: true,
            global_dirty: false,
        };
        let pins = I2sPage::bck_candidates(&cx, "I2S BCK", 14);
        assert!(pins.contains(&14), "its own pair");
        assert!(!pins.contains(&5), "GPIO 5 is the S/PDIF RX");
        assert!(!pins.contains(&4), "GPIO 5 would be its LRCLK");
        assert!(!pins.contains(&9), "GPIO 10 is the sub");
    }

    #[test]
    fn the_multiplier_is_locked_to_128x_above_48_khz() {
        let mut st = state();
        let sec = dspi_proto::generated::SECTIONS
            .iter()
            .find(|(n, _, _)| *n == "input_config")
            .map(|(_, o, _)| *o)
            .unwrap();
        // Rate code 2 is 96 kHz.
        st.bulk.patch(sec + 3, &[2]);
        let mut s = SettingsScreen::new(&st, data(), AppConfig::default()).open(Page::I2s, &st);
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("Locked to 128x at 96 kHz"), "{f}");
    }

    #[test]
    fn the_i2s_page_is_hidden_on_a_platform_whose_clock_pins_are_fixed() {
        let mut st = state();
        st.caps.platform = dspi_proto::Platform::Unknown(2);
        assert!(!super::super::available(Page::I2s, &st, true));
    }
}
