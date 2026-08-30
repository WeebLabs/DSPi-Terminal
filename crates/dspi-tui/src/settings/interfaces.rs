//! Settings > Control Interfaces.
//!
//! The two external control transports, UART and I2C. Both are configured over
//! USB only, so this is the one place they can be set up: an external
//! controller can read the configuration but never reconfigure the transport it
//! is talking on, and so cannot lock itself out.
//!
//! Each interface stages its edits in a draft and commits them with an explicit
//! Apply, because one SET writes the whole 8-byte config and persists it, and a
//! per-control write would flash once per keystroke. Apply and Revert are per
//! interface, not on the shared save bar, which is what the Console does too.

use dspi_proto::packets::{I2cCtrlConfig, UartCtrlConfig};

use crate::widgets::{Action, KeyHelp, PopupList, StatusTone};

use crate::shell::SessionReply;
use dspi_proto::value::Value;

use super::{Cx, PageEvent, Row, SettingsData, SettingsPage};

/// Tags for the two applies, so a reply reaches the right section.
const TAG_UART: u32 = 0;
const TAG_I2C: u32 = 1;

/// The Console's `UART_CTRL_BAUD_CHOICES` (`Constants.swift:549`).
pub const BAUD_CHOICES: [u32; 9] = [
    9600, 19200, 38400, 57600, 115_200, 230_400, 460_800, 921_600, 1_000_000,
];

/// `I2C_CTRL_ADDR_MIN` and `_MAX` (config.h).
pub const ADDR_MIN: u8 = 0x08;
pub const ADDR_MAX: u8 = 0x77;

/// The status strings the firmware's `PIN_CONFIG_*` outcome maps to, verbatim
/// from the Console's `statusMessage(_:iface:)`.
pub fn status_message(status: u8, iface: &str) -> (String, bool) {
    match status {
        0 => (format!("{iface} configuration applied and saved"), false),
        1 => (
            format!("A pin is out of range or lacks the required {iface} mux function"),
            true,
        ),
        2 => (
            "A pin is already claimed by another output or interface".to_string(),
            true,
        ),
        5 => (
            if iface == "UART" {
                "Baud rate is out of range (9600 - 1000000)".to_string()
            } else {
                "Address is out of range (0x08 - 0x77)".to_string()
            },
            true,
        ),
        _ => (format!("Failed to apply {iface} configuration"), true),
    }
}

/// The footer the Console puts under both sections.
pub const FOOTER: &str = "Both control interfaces are configured over USB only - an external \
                          controller can read the configuration but can never reconfigure or \
                          disable the transport it is talking on, so it cannot lock itself out. \
                          Settings persist across reboots and survive a factory reset. UART \
                          TX/RX and I2C SDA/SCL must land on GPIOs that carry the matching \
                          peripheral mux function; the device validates this when you apply. \
                          Fit external pull-ups (2.2k - 4.7k) on the I2C bus.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    UartEnable,
    UartTx,
    UartRx,
    UartBaud,
    UartNotify,
    UartApply,
    I2cEnable,
    I2cSda,
    I2cScl,
    I2cAddress,
    I2cApply,
}

pub struct InterfacesPage {
    cursor: usize,
    uart: UartCtrlConfig,
    i2c: I2cCtrlConfig,
    /// What the device is believed to hold. Seeded from the read and taken
    /// from the draft on a successful Apply, so the page does not wait on a
    /// re-read to stop saying "Unapplied changes" about its own write.
    uart_device: UartCtrlConfig,
    i2c_device: I2cCtrlConfig,
    /// Which of the two buttons on each Apply row `Enter` runs: 0 Revert,
    /// 1 Apply. The row is rebuilt every frame, so the cursor lives here.
    uart_button: usize,
    i2c_button: usize,
    popup_item: Option<Item>,
    uart_status: Option<(String, bool)>,
    i2c_status: Option<(String, bool)>,
}

const KEYS: &[KeyHelp] = &[
    KeyHelp::new("↑ ↓", "Move between controls"),
    KeyHelp::new("← →", "Change, or pick Apply and Revert"),
    KeyHelp::new("Enter", "Open the list, or activate"),
    KeyHelp::new("Space", "Toggle"),
    KeyHelp::new("Tab", "Sidebar, page, save bar"),
    KeyHelp::new("[ ]", "Back, forward"),
    KeyHelp::new("Esc", "Leave Settings"),
];

fn baud_label(b: u32) -> String {
    if b >= 1000 && b.is_multiple_of(1000) {
        format!("{}k", b / 1000)
    } else {
        b.to_string()
    }
}

impl InterfacesPage {
    pub fn new(data: &SettingsData) -> Self {
        let uart = data.uart.clone().unwrap_or_default();
        let i2c = data.i2c.clone().unwrap_or_default();
        Self {
            cursor: 0,
            uart: uart.clone(),
            i2c: i2c.clone(),
            uart_device: uart,
            i2c_device: i2c,
            uart_button: 1,
            i2c_button: 1,
            popup_item: None,
            uart_status: None,
            i2c_status: None,
        }
    }

    fn uart_dirty(&self) -> bool {
        self.uart != self.uart_device
    }

    fn i2c_dirty(&self) -> bool {
        self.i2c != self.i2c_device
    }

    /// The Console's pill: Active while the peripheral is up, Inactive when the
    /// stored config says enabled but it never came up, Disabled otherwise.
    fn pill(live: bool, config_enabled: bool) -> (&'static str, StatusTone) {
        if live {
            ("Active", StatusTone::Ok)
        } else if config_enabled {
            ("Inactive", StatusTone::Warning)
        } else {
            ("Disabled", StatusTone::Neutral)
        }
    }

    fn uart_tx_pins(&self, cx: &Cx<'_>) -> Vec<u8> {
        dspi_session::pins::candidates(cx.platform(), dspi_session::PinConstraint::UartTx)
            .into_iter()
            .filter(|p| {
                *p != self.uart.rx_pin
                    && (*p == self.uart.tx_pin
                        || cx.data.owner_of(cx.state, *p, "UART Control").is_none())
            })
            .collect()
    }

    fn uart_rx_pins(&self, cx: &Cx<'_>) -> Vec<u8> {
        dspi_session::pins::candidates(cx.platform(), dspi_session::PinConstraint::UartRx)
            .into_iter()
            .filter(|p| {
                *p != self.uart.tx_pin
                    && (*p == self.uart.rx_pin
                        || cx.data.owner_of(cx.state, *p, "UART Control").is_none())
            })
            .collect()
    }

    fn i2c_sda_pins(&self, cx: &Cx<'_>) -> Vec<u8> {
        dspi_session::pins::candidates(cx.platform(), dspi_session::PinConstraint::I2cSda)
            .into_iter()
            .filter(|p| {
                *p != self.i2c.scl_pin
                    && (*p == self.i2c.sda_pin
                        || cx.data.owner_of(cx.state, *p, "I2C Control").is_none())
            })
            .collect()
    }

    fn i2c_scl_pins(&self, cx: &Cx<'_>) -> Vec<u8> {
        dspi_session::pins::candidates(cx.platform(), dspi_session::PinConstraint::I2cScl)
            .into_iter()
            .filter(|p| {
                *p != self.i2c.sda_pin
                    && (*p == self.i2c.scl_pin
                        || cx.data.owner_of(cx.state, *p, "I2C Control").is_none())
            })
            .collect()
    }

    fn pin_row(
        item: Item,
        label: &str,
        detail: &str,
        pins: &[u8],
        current: u8,
    ) -> (Option<Item>, Row) {
        let mut choices: Vec<String> = pins.iter().map(|p| format!("GPIO {p}")).collect();
        let selected = match pins.iter().position(|p| *p == current) {
            Some(i) => i,
            None => {
                // Keep the current pin visible even when nothing else would
                // offer it, which is what the Console's picker does.
                choices.insert(0, format!("GPIO {current}"));
                0
            }
        };
        (
            Some(item),
            Row::Pick {
                label: label.into(),
                choices,
                selected,
                caption: Some(detail.into()),
                enabled: true,
            },
        )
    }

    fn apply_row(
        item: Item,
        dirty: bool,
        status: &Option<(String, bool)>,
        cursor: usize,
    ) -> Vec<(Option<Item>, Row)> {
        let mut rows = Vec::new();
        // A refusal is shown alongside "Unapplied changes": the draft is still
        // there to be corrected, and the reason is why it is.
        match status {
            Some((text, true)) => rows.push((None, Row::Status(text.clone(), true))),
            Some((text, false)) if !dirty => rows.push((None, Row::Status(text.clone(), false))),
            _ => {}
        }
        if dirty {
            rows.push((None, Row::note("Unapplied changes")));
        }
        rows.push((
            Some(item),
            Row::Buttons {
                label: String::new(),
                caption: None,
                buttons: vec!["Revert".into(), "Apply".into()],
                cursor,
                enabled: dirty,
            },
        ));
        rows
    }

    fn build(&self, cx: &Cx<'_>) -> Vec<(Option<Item>, Row)> {
        let iface = cx.data.iface.clone().unwrap_or_default();
        let device_uart = &self.uart_device;
        let device_i2c = &self.i2c_device;
        let mut rows: Vec<(Option<Item>, Row)> = Vec::new();

        if cx.feature("uart_control") {
            rows.push((None, Row::section("UART")));
            let (text, tone) = Self::pill(iface.uart_live, device_uart.enabled);
            rows.push((
                None,
                Row::Pill {
                    label: "Status".into(),
                    text: text.into(),
                    tone,
                    caption: None,
                },
            ));
            rows.push((
                Some(Item::UartEnable),
                Row::Toggle {
                    label: "Enable UART".into(),
                    on: self.uart.enabled,
                    caption: Some("Asynchronous 3.3V serial link, fixed 8N1 framing.".into()),
                    enabled: true,
                },
            ));
            if device_uart.enabled && !iface.uart_live {
                rows.push((
                    None,
                    Row::warning(
                        "Enabled in flash but not running",
                        "Its pins likely collide with the current output wiring. Reassign the \
                         conflicting pin or move this interface, then apply.",
                    ),
                ));
            }
            if self.uart.enabled {
                let tx = self.uart_tx_pins(cx);
                rows.push(Self::pin_row(
                    Item::UartTx,
                    "TX Pin",
                    "GPIO transmitting to the controller's RX.",
                    &tx,
                    self.uart.tx_pin,
                ));
                let rx = self.uart_rx_pins(cx);
                rows.push(Self::pin_row(
                    Item::UartRx,
                    "RX Pin",
                    "GPIO receiving from the controller's TX.",
                    &rx,
                    self.uart.rx_pin,
                ));
                rows.push((
                    Some(Item::UartBaud),
                    Row::Pick {
                        label: "Baud Rate".into(),
                        choices: BAUD_CHOICES.iter().map(|b| baud_label(*b)).collect(),
                        selected: BAUD_CHOICES
                            .iter()
                            .position(|b| *b == self.uart.baud)
                            .unwrap_or(4),
                        caption: Some("9600 - 1000000. Must match the controller.".into()),
                        enabled: true,
                    },
                ));
                rows.push((
                    Some(Item::UartNotify),
                    Row::Toggle {
                        label: "Push Notifications".into(),
                        on: self.uart.notify_enable,
                        caption: Some(
                            "Stream live parameter/preset/format changes to the controller \
                             (type-0x40 frames) instead of polling."
                                .into(),
                        ),
                        enabled: true,
                    },
                ));
            }
            rows.extend(Self::apply_row(
                Item::UartApply,
                self.uart_dirty(),
                &self.uart_status,
                self.uart_button,
            ));
        }

        if cx.feature("i2c_control") {
            rows.push((None, Row::Blank));
            rows.push((None, Row::section("I2C")));
            let (text, tone) = Self::pill(iface.i2c_live, device_i2c.enabled);
            rows.push((
                None,
                Row::Pill {
                    label: "Status".into(),
                    text: text.into(),
                    tone,
                    caption: None,
                },
            ));
            rows.push((
                Some(Item::I2cEnable),
                Row::Toggle {
                    label: "Enable I2C Target".into(),
                    on: self.i2c.enabled,
                    caption: Some(
                        "Device acts as an I2C slave; the controller is bus master. Poll-only \
                         (no async notifications)."
                            .into(),
                    ),
                    enabled: true,
                },
            ));
            if device_i2c.enabled && !iface.i2c_live {
                rows.push((
                    None,
                    Row::warning(
                        "Enabled in flash but not running",
                        "Its pins likely collide with the current output wiring. Reassign the \
                         conflicting pin or move this interface, then apply.",
                    ),
                ));
            }
            if self.i2c.enabled {
                let sda = self.i2c_sda_pins(cx);
                rows.push(Self::pin_row(
                    Item::I2cSda,
                    "SDA Pin",
                    "Serial data line (even GPIO).",
                    &sda,
                    self.i2c.sda_pin,
                ));
                let scl = self.i2c_scl_pins(cx);
                rows.push(Self::pin_row(
                    Item::I2cScl,
                    "SCL Pin",
                    "Serial clock line (next odd GPIO, same instance).",
                    &scl,
                    self.i2c.scl_pin,
                ));
                rows.push((
                    Some(Item::I2cAddress),
                    Row::Number {
                        label: format!("Target Address  0x{:02X}", self.i2c.address),
                        value: self.i2c.address as f64,
                        min: ADDR_MIN as f64,
                        max: ADDR_MAX as f64,
                        step: 1.0,
                        unit: String::new(),
                        decimals: 0,
                        caption: Some("7-bit address, 0x08 - 0x77.".into()),
                        enabled: true,
                    },
                ));
            }
            rows.extend(Self::apply_row(
                Item::I2cApply,
                self.i2c_dirty(),
                &self.i2c_status,
                self.i2c_button,
            ));
        }

        rows.push((None, Row::Blank));
        rows.push((None, Row::note(FOOTER)));
        if iface.proto_version != 0 {
            rows.push((
                None,
                Row::note(format!(
                    "External control protocol version {}.",
                    iface.proto_version
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

    fn apply(&mut self, item: Item, choice: usize, cx: &Cx<'_>) -> PageEvent {
        match item {
            Item::UartTx => {
                let pins = self.uart_tx_pins(cx);
                if let Some(p) = pins.get(choice) {
                    self.uart.tx_pin = *p;
                }
            }
            Item::UartRx => {
                let pins = self.uart_rx_pins(cx);
                if let Some(p) = pins.get(choice) {
                    self.uart.rx_pin = *p;
                }
            }
            Item::UartBaud => {
                if let Some(b) = BAUD_CHOICES.get(choice) {
                    self.uart.baud = *b;
                }
            }
            Item::I2cSda => {
                let pins = self.i2c_sda_pins(cx);
                if let Some(p) = pins.get(choice) {
                    self.i2c.sda_pin = *p;
                }
            }
            Item::I2cScl => {
                let pins = self.i2c_scl_pins(cx);
                if let Some(p) = pins.get(choice) {
                    self.i2c.scl_pin = *p;
                }
            }
            _ => {}
        }
        PageEvent::Handled
    }
}

impl SettingsPage for InterfacesPage {
    fn rows(&self, cx: &Cx<'_>) -> Vec<Row> {
        self.build(cx).into_iter().map(|(_, r)| r).collect()
    }

    fn act(&mut self, index: usize, action: Action, cx: &Cx<'_>) -> PageEvent {
        let Some(item) = self.item(index, cx) else {
            return PageEvent::Handled;
        };
        match (item, action) {
            (Item::UartEnable, Action::Toggled(on)) => {
                self.uart.enabled = on;
                PageEvent::Handled
            }
            (Item::UartNotify, Action::Toggled(on)) => {
                self.uart.notify_enable = on;
                PageEvent::Handled
            }
            (Item::I2cEnable, Action::Toggled(on)) => {
                self.i2c.enabled = on;
                PageEvent::Handled
            }
            (Item::I2cAddress, Action::Changed(v) | Action::Committed(v)) => {
                self.i2c.address = (v.round() as i64).clamp(ADDR_MIN as i64, ADDR_MAX as i64) as u8;
                PageEvent::Handled
            }
            (Item::I2cAddress, Action::Reset) => {
                self.i2c.address = I2cCtrlConfig::default().address;
                PageEvent::Handled
            }
            // The Apply row's two buttons: Revert on the left, Apply on the
            // right, so `←` and `→` pick and `Enter` runs.
            (Item::UartApply, Action::Selected(i)) => {
                self.uart_button = i.min(1);
                PageEvent::Handled
            }
            (Item::I2cApply, Action::Selected(i)) => {
                self.i2c_button = i.min(1);
                PageEvent::Handled
            }
            (Item::UartApply, Action::Open) => {
                if self.uart_button == 0 {
                    self.uart = self.uart_device.clone();
                    self.uart_status = None;
                    return PageEvent::Status("UART changes reverted".into());
                }
                self.uart_status = None;
                PageEvent::Session(super::iface_write(
                    TAG_UART,
                    "dev.uart",
                    Value::Bytes(self.uart.encode().to_vec()),
                    true,
                    std::rc::Rc::new(|code| status_message(code, "UART")),
                ))
            }
            (Item::I2cApply, Action::Open) => {
                if self.i2c_button == 0 {
                    self.i2c = self.i2c_device.clone();
                    self.i2c_status = None;
                    return PageEvent::Status("I2C changes reverted".into());
                }
                self.i2c_status = None;
                PageEvent::Session(super::iface_write(
                    TAG_I2C,
                    "dev.i2c",
                    Value::Bytes(self.i2c.encode().to_vec()),
                    false,
                    std::rc::Rc::new(|code| status_message(code, "I2C")),
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

    /// The device's own verdict on the configuration this section just sent.
    ///
    /// The draft only becomes "what the device holds" when the device says it
    /// took it; a refusal leaves the old configuration running, so the section
    /// stays dirty and the row says why.
    fn session_result(&mut self, tag: u32, reply: SessionReply, _cx: &Cx<'_>) -> PageEvent {
        let (uart, status) = match (tag, reply) {
            (TAG_UART, SessionReply::Ok(m)) => {
                self.uart_device = self.uart.clone();
                (true, (m, false))
            }
            (TAG_UART, SessionReply::Err(m)) => (true, (m, true)),
            (TAG_I2C, SessionReply::Ok(m)) => {
                self.i2c_device = self.i2c.clone();
                (false, (m, false))
            }
            (TAG_I2C, SessionReply::Err(m)) => (false, (m, true)),
            _ => return PageEvent::Handled,
        };
        let text = status.0.clone();
        if uart {
            self.uart_status = Some(status);
        } else {
            self.i2c_status = Some(status);
        }
        PageEvent::Status(text)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{data, frame, key, screen, state};
    use super::super::{AppConfig, Page, SettingsScreen};
    use super::*;
    use crate::shell::{Screen, ScreenEvent};
    use crossterm::event::KeyCode;

    #[test]
    fn both_interfaces_have_a_header_a_pill_and_a_toggle() {
        let (mut s, st) = screen(Page::Interfaces);
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            let f = frame(&mut s, &st, w, h);
            assert_eq!(f.lines().count(), h as usize);
            assert!(f.contains("UART"), "{w}x{h}: {f}");
            assert!(f.contains("Enable UART"), "{w}x{h}: {f}");
        }
        let f = frame(&mut s, &st, 120, 40);
        assert!(
            f.contains("Asynchronous 3.3V serial link, fixed 8N1 framing."),
            "{f}"
        );
        assert!(f.contains("Disabled"), "the status pill: {f}");
        assert!(f.contains("Enable I2C Target"), "{f}");
    }

    #[test]
    fn turning_uart_on_reveals_its_pins_and_marks_the_change_unapplied() {
        let (mut s, st) = screen(Page::Interfaces);
        s.handle(key(KeyCode::Tab), &st);
        s.handle(key(KeyCode::Char(' ')), &st);
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("TX Pin"), "{f}");
        assert!(f.contains("RX Pin"), "{f}");
        assert!(f.contains("Baud Rate"), "{f}");
        assert!(f.contains("Push Notifications"), "{f}");
        assert!(f.contains("Unapplied changes"), "{f}");
        assert!(f.contains("Apply"), "{f}");
    }

    /// Apply is one write of the whole 8-byte record, and the row reports what
    /// the device made of it rather than assuming.
    #[test]
    fn apply_writes_the_whole_configuration_and_reports_the_devices_answer() {
        use dspi_proto::generated::opcodes as op;
        use dspi_proto::packets::CtrlIfaceStatus;
        use dspi_transport::MockTransport;

        let iface = |last: u8| {
            CtrlIfaceStatus {
                uart_last_status: last,
                uart_live: last == 0,
                i2c_last_status: 0,
                i2c_live: false,
                proto_version: 2,
            }
            .encode()
            .to_vec()
        };
        let armed = |s: &mut SettingsScreen, st: &_| {
            s.handle(key(KeyCode::Tab), st);
            s.handle(key(KeyCode::Char(' ')), st);
            for _ in 0..5 {
                s.handle(key(KeyCode::Down), st);
            }
        };
        let session = |last: u8| {
            let t = MockTransport::new()
                .data(op::REQ_SET_UART_CONFIG, vec![])
                .data(
                    op::REQ_GET_UART_CONFIG,
                    UartCtrlConfig {
                        enabled: true,
                        ..UartCtrlConfig::default()
                    }
                    .encode()
                    .to_vec(),
                )
                .data(op::REQ_GET_CTRL_IFACE_STATUS, iface(last));
            let log = t.log_handle();
            let mut caps = crate::shell::fixture::caps();
            caps.features = vec![dspi_session::probe::Feature {
                name: "uart_control".into(),
                present: true,
                evidence: "0xF6 answered".into(),
            }];
            (
                dspi_session::Session::new(Box::new(t), caps).expect("session"),
                log,
            )
        };

        // Accepted: the record goes out whole and the section stops being dirty.
        let (mut s, st) = screen(Page::Interfaces);
        armed(&mut s, &st);
        let req = match s.handle(key(KeyCode::Enter), &st) {
            ScreenEvent::Session(r) => r,
            other => panic!("{other:?}"),
        };
        let (mut sess, log) = session(0);
        let reply = (req.run)(&mut sess);
        s.session_result(req.tag, reply, &st);
        let sent = log
            .lock()
            .unwrap()
            .iter()
            .find(|&e| e.opcode == op::REQ_SET_UART_CONFIG)
            .cloned()
            .expect("one config write");
        assert_eq!(
            sent.payload,
            UartCtrlConfig {
                enabled: true,
                tx_pin: 16,
                rx_pin: 17,
                notify_enable: false,
                baud: 115_200,
            }
            .encode()
            .to_vec()
        );
        let f = frame(&mut s, &st, 120, 40);
        assert!(
            f.contains("UART configuration applied and saved"),
            "the inline status row: {f}"
        );
        assert!(!f.contains("Unapplied changes"), "{f}");

        // Refused: the device kept its old configuration, so the draft stays
        // dirty and the row says which of the five reasons it was.
        let (mut s, st) = screen(Page::Interfaces);
        armed(&mut s, &st);
        let req = match s.handle(key(KeyCode::Enter), &st) {
            ScreenEvent::Session(r) => r,
            other => panic!("{other:?}"),
        };
        let (mut sess, _) = session(super::super::pin_status::PIN_IN_USE);
        let reply = (req.run)(&mut sess);
        assert_eq!(
            reply,
            crate::shell::SessionReply::Err(
                "A pin is already claimed by another output or interface".into()
            )
        );
        s.session_result(req.tag, reply, &st);
        let f = frame(&mut s, &st, 120, 40);
        assert!(
            f.contains("A pin is already claimed by another output or interface"),
            "{f}"
        );
        assert!(
            f.contains("Unapplied changes"),
            "still to be corrected: {f}"
        );
    }

    #[test]
    fn revert_puts_the_draft_back_to_what_the_device_holds() {
        let (mut s, st) = screen(Page::Interfaces);
        s.handle(key(KeyCode::Tab), &st);
        s.handle(key(KeyCode::Char(' ')), &st);
        for _ in 0..5 {
            s.handle(key(KeyCode::Down), &st);
        }
        s.handle(key(KeyCode::Left), &st);
        assert_eq!(
            s.handle(key(KeyCode::Enter), &st),
            ScreenEvent::Status("UART changes reverted".into())
        );
        let f = frame(&mut s, &st, 120, 40);
        assert!(!f.contains("Unapplied changes"), "{f}");
        assert!(!f.contains("TX Pin"), "the pins are hidden again: {f}");
        let _ = &st;
    }

    #[test]
    fn the_pin_pickers_carry_the_mux_constraint() {
        let st = state();
        let d = data();
        let cx = Cx {
            state: &st,
            data: &d,
            config: &AppConfig::default(),
            connected: true,
            global_dirty: false,
        };
        let p = InterfacesPage::new(&d);
        assert!(p.uart_tx_pins(&cx).iter().all(|x| x % 4 == 0));
        assert!(p.uart_rx_pins(&cx).iter().all(|x| x % 4 == 1));
        assert!(p.i2c_sda_pins(&cx).iter().all(|x| x % 2 == 0));
        assert!(p.i2c_scl_pins(&cx).iter().all(|x| x % 2 == 1));
        // GPIO 8 is Output 3 in the fixture, so it is not offered for TX.
        assert!(!p.uart_tx_pins(&cx).contains(&8));
    }

    #[test]
    fn an_enabled_but_dead_interface_says_why() {
        let st = state();
        let mut d = data();
        d.uart = Some(UartCtrlConfig {
            enabled: true,
            ..UartCtrlConfig::default()
        });
        let mut s = SettingsScreen::new(&st, d, AppConfig::default()).open(Page::Interfaces, &st);
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("Inactive"), "the pill: {f}");
        assert!(f.contains("Enabled in flash but not running"), "{f}");
        assert!(
            f.contains("pins likely collide with the current output wiring"),
            "{f}"
        );
    }

    #[test]
    fn the_footer_names_the_protocol_version() {
        let (mut s, st) = screen(Page::Interfaces);
        s.handle(key(KeyCode::Tab), &st);
        for _ in 0..8 {
            s.handle(key(KeyCode::Down), &st);
        }
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("External control protocol version 2."), "{f}");
    }

    #[test]
    fn every_status_string_is_the_consoles() {
        assert_eq!(
            status_message(0, "UART").0,
            "UART configuration applied and saved"
        );
        assert_eq!(
            status_message(1, "I2C").0,
            "A pin is out of range or lacks the required I2C mux function"
        );
        assert_eq!(
            status_message(2, "UART").0,
            "A pin is already claimed by another output or interface"
        );
        assert_eq!(
            status_message(5, "UART").0,
            "Baud rate is out of range (9600 - 1000000)"
        );
        assert_eq!(
            status_message(5, "I2C").0,
            "Address is out of range (0x08 - 0x77)"
        );
        assert!(status_message(1, "UART").1, "a failure is an error");
    }
}
