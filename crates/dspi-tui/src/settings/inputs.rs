//! Settings > Inputs.
//!
//! The three input sections the Console puts on one page: S/PDIF (with the
//! optional receivers and LG Sound Sync), I2S (clock role, channel count and a
//! data pin per pair) and ADAT. Each section only appears when the device
//! answered its probe, because an absent feature is removed rather than dimmed.

use std::rc::Rc;

use dspi_proto::value::Value;

use crate::shell::{SessionReply, SessionRequest};
use crate::widgets::{Action, Button, Dialog, DialogOutcome, KeyHelp, PopupList, StatusTone};

use super::pin_status as st;
use super::{Cx, Explain, INPUT_RATES_HZ, PageEvent, Row, SettingsPage};

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

    /// How many S/PDIF receivers this device has, from the inventory count
    /// `REQ_GET_SPDIF_INPUT_CONFIG` answers (the Console's `spdifInputCount`).
    /// A firmware that predates the fourth input reports three and the extra
    /// choice never appears.
    fn spdif_inventory(cx: &Cx<'_>) -> usize {
        cx.data
            .spdif
            .as_ref()
            .map(|c| (c.count as usize).clamp(1, 4))
            .unwrap_or(4)
    }

    /// The most I2S input channels this part can take: one stereo pair on an
    /// RP2040, four on an RP2350 (the Console's `i2sMaxPairs * 2`).
    fn max_i2s_channels(cx: &Cx<'_>) -> usize {
        match cx.platform() {
            dspi_proto::Platform::Rp2350 => 8,
            _ => 2,
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
                    choices: (1..=Self::spdif_inventory(cx))
                        .map(|n| n.to_string())
                        .collect(),
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
                    choices: (1..=Self::max_i2s_channels(cx) / 2)
                        .map(|p| (p * 2).to_string())
                        .collect(),
                    selected: (pairs.max(1) - 1).min(Self::max_i2s_channels(cx) / 2 - 1),
                    caption: Some(format!(
                        "{pairs} stereo pair{} of 24-bit audio, sample-aligned",
                        if pairs == 1 { "" } else { "s" }
                    )),
                    // A stereo-only part has one choice, which the Console
                    // draws as a plain "2" rather than a picker.
                    enabled: cx.connected && Self::max_i2s_channels(cx) > 2,
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
            rows.push((
                None,
                Row::note(if Self::max_i2s_channels(cx) > 2 {
                    "Wire one ADC serial-data line per stereo pair to the GPIOs above; the shared \
                     bit clock and sample rate live in I2S Configuration. DSPi is the clock \
                     master and the pairs are sample-aligned. Save a preset to keep this wiring."
                } else {
                    "Wire the ADC's serial-data line to the GPIO above. The bit clock and sample \
                     rate live in I2S Configuration; DSPi is the clock master, so the source must \
                     follow."
                }),
            ));
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
    fn instance_steps(cx: &Cx<'_>, target: usize) -> Vec<(u8, bool)> {
        let mut out = Vec::new();
        if target >= 2 {
            for idx in 1..target {
                if !Self::optional_enabled(cx, idx) {
                    out.push((idx as u8, true));
                }
            }
        }
        for idx in (target.max(1)..4).rev() {
            if Self::optional_enabled(cx, idx) {
                out.push((idx as u8, false));
            }
        }
        out
    }

    /// The Console's sentence for one input write's `PIN_CONFIG_*` answer
    /// (`handleI2SInputStatus`, `handleAdatInputStatus`, `spdifInputRow`).
    fn explain(item: Item, name: String, pin: u8, owner: Option<String>) -> Rc<Explain> {
        Rc::new(move |code| match (item, code) {
            (Item::SpdifPin(idx), st::SUCCESS) => (
                format!("S/PDIF {} RX pin set to GPIO {pin}", idx + 1),
                false,
            ),
            (Item::SpdifPin(_), st::PIN_IN_USE) => (
                match &owner {
                    Some(o) => format!("GPIO {pin} is already assigned to {o}"),
                    None => format!("GPIO {pin} is already in use"),
                },
                true,
            ),
            (Item::SpdifPin(_), st::INVALID_PIN) => (
                format!("GPIO {pin} is not available on this platform"),
                true,
            ),
            (Item::SpdifPin(idx), _) => (format!("Failed to set S/PDIF {} RX pin", idx + 1), true),

            (Item::I2sChannels, st::SUCCESS) | (Item::I2sPin(_), st::SUCCESS) => {
                (name.clone(), false)
            }
            (Item::I2sChannels, _) | (Item::I2sPin(_), _) => (
                Self::i2s_input_failure(code, pin, owner.clone(), &name),
                true,
            ),

            (Item::AdatPin, st::SUCCESS) => (format!("ADAT input pin set to GPIO {pin}"), false),
            (Item::AdatPin, st::PIN_IN_USE) => (
                match &owner {
                    Some(o) => format!("GPIO {pin} is already assigned to {o}"),
                    None => "That pin is already in use".to_string(),
                },
                true,
            ),
            (Item::AdatPin, st::INVALID_PIN) => {
                (format!("GPIO {pin} isn't available on this device"), true)
            }
            (Item::AdatPin, st::INVALID_OUTPUT) => {
                ("ADAT input isn't supported on this device".into(), true)
            }
            (Item::AdatPin, _) => ("Failed to set ADAT input pin".into(), true),

            (Item::AdatEnable, st::SUCCESS) => (name.clone(), false),
            (Item::AdatEnable, st::PIN_IN_USE) => (
                "Switch to another input source before disabling ADAT input".into(),
                true,
            ),
            (Item::AdatEnable, st::INVALID_PIN) => (
                "Assign a valid data pin before enabling ADAT input".into(),
                true,
            ),
            (Item::AdatEnable, st::INVALID_OUTPUT) => {
                ("ADAT input isn't supported on this device".into(), true)
            }
            (Item::AdatEnable, _) => ("Failed to set ADAT input".into(), true),

            (_, st::SUCCESS) => (name.clone(), false),
            (_, _) => (format!("Failed to set {name}"), true),
        })
    }

    /// `handleI2SInputStatus`, whose four reasons the two I2S rows share.
    fn i2s_input_failure(code: u8, pin: u8, owner: Option<String>, label: &str) -> String {
        match code {
            st::PIN_IN_USE => match owner {
                Some(o) => format!("GPIO {pin} is already assigned to {o}"),
                None => "That pin is already in use".to_string(),
            },
            st::INVALID_PIN => format!("GPIO {pin} isn't available on this device"),
            st::INVALID_OUTPUT => "Multichannel I2S isn't supported on this device".to_string(),
            st::OUTPUT_ACTIVE => {
                "Can't change the bit clock while an I2S output is active".to_string()
            }
            _ => format!("Failed to set {label}"),
        }
    }

    fn write(path: &'static str, indices: Vec<u8>, value: Value, e: Rc<Explain>) -> PageEvent {
        PageEvent::IoSession(super::device_write(0, path, indices, value, e))
    }

    fn apply(&mut self, item: Item, choice: usize, cx: &Cx<'_>) -> PageEvent {
        self.status = None;
        match item {
            Item::Instances => {
                let target = choice + 1;
                let steps = Self::instance_steps(cx, target);
                if steps.is_empty() {
                    return PageEvent::Handled;
                }
                let pins: Vec<(u8, Option<String>)> = steps
                    .iter()
                    .map(|(idx, _)| {
                        let pin = Self::spdif_pin(cx, *idx as usize);
                        (
                            pin,
                            cx.data
                                .owner_of(cx.state, pin, &Self::spdif_owner(*idx as usize)),
                        )
                    })
                    .collect();
                PageEvent::IoSession(SessionRequest::new(0, move |session| {
                    for ((idx, on), (pin, owner)) in steps.iter().zip(pins.iter()) {
                        let out = session.write("in.spdif.enable", &[*idx], Value::Bool(*on));
                        let code = match out {
                            Err(e) => return SessionReply::Err(e.to_string()),
                            Ok(_) => session.last_write_status().unwrap_or(st::SUCCESS),
                        };
                        if code == st::SUCCESS {
                            continue;
                        }
                        return SessionReply::Err(if *on {
                            match owner {
                                Some(o) => format!(
                                    "Can't enable S/PDIF {}: GPIO {pin} is assigned to {o}",
                                    idx + 1
                                ),
                                None => format!(
                                    "Can't enable S/PDIF {}: GPIO {pin} is unavailable",
                                    idx + 1
                                ),
                            }
                        } else {
                            format!(
                                "Switch the input source away from S/PDIF {} before reducing the \
                                 input count",
                                idx + 1
                            )
                        });
                    }
                    SessionReply::Ok(format!("S/PDIF inputs set to {target}"))
                }))
            }
            Item::SpdifPin(idx) => {
                let pin = Self::spdif_pin(cx, idx);
                let owner_name = Self::spdif_owner(idx);
                let candidates = cx.free_pins(&owner_name, Some(pin));
                let Some(p) = candidates.get(choice).copied() else {
                    return PageEvent::Handled;
                };
                let held = cx.data.owner_of(cx.state, p, &owner_name);
                Self::write(
                    "in.spdif.pin",
                    vec![idx as u8],
                    Value::Int(p as i64),
                    Self::explain(item, String::new(), p, held),
                )
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
                let name = format!(
                    "I2S input set to {count} channels ({} pair{})",
                    count / 2,
                    if count == 2 { "" } else { "s" }
                );
                Self::write(
                    "in.i2s.channels",
                    Vec::new(),
                    Value::Int(count as i64),
                    Self::explain(item, name, 0, None),
                )
            }
            Item::I2sPin(pair) => {
                let pairs = Self::i2s_pins(cx).len();
                let pin = Self::i2s_pins(cx).get(pair).copied().unwrap_or(0);
                let owner_name = Self::i2s_owner(pair, pairs);
                let candidates = cx.free_pins(&owner_name, Some(pin));
                let Some(p) = candidates.get(choice).copied() else {
                    return PageEvent::Handled;
                };
                let held = cx.data.owner_of(cx.state, p, &owner_name);
                let name = format!("Serial Data {} set to GPIO {p}", pair + 1);
                Self::write(
                    "in.i2s.pin",
                    vec![pair as u8],
                    Value::Int(p as i64),
                    Self::explain(item, name, p, held),
                )
            }
            Item::AdatPin => {
                let pin = cx.state.input_config().and_then(|c| c.adat_input_pin);
                let candidates = cx.free_pins("ADAT Input", pin);
                let offset = usize::from(pin.is_none());
                if choice < offset {
                    return PageEvent::Handled;
                }
                let Some(p) = candidates.get(choice - offset).copied() else {
                    return PageEvent::Handled;
                };
                let held = cx.data.owner_of(cx.state, p, "ADAT Input");
                Self::write(
                    "in.adat.pin",
                    Vec::new(),
                    Value::Int(p as i64),
                    Self::explain(item, String::new(), p, held),
                )
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
                self.status = None;
                let name = format!("ADAT input {}", if on { "enabled" } else { "disabled" });
                Self::write(
                    "in.adat.enable",
                    Vec::new(),
                    Value::Bool(on),
                    Self::explain(item, name, 0, None),
                )
            }
            (Item::EnableAdatOutput, Action::Open) => {
                self.status = None;
                Self::write(
                    "adat.enable",
                    Vec::new(),
                    Value::Bool(true),
                    Self::explain(item, "ADAT output enabled".into(), 0, None),
                )
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
    use super::super::tests::{data, frame, key, screen, state};
    use super::super::{AppConfig, Page, SettingsScreen};
    use super::*;
    use crate::shell::{Screen, ScreenEvent};
    use crossterm::event::KeyCode;
    use dspi_transport::MockTransport;
    use dspi_transport::mock::Exchange;

    /// Run a page's write against a device answering `code`.
    fn answered(req: &SessionRequest, code: u8) -> (SessionReply, Vec<Exchange>) {
        let t = MockTransport::new().answering_everything(vec![code]);
        let log = t.log_handle();
        let mut caps = crate::shell::fixture::caps();
        caps.features = ["spdif_multi_input", "adat_input", "adat_output"]
            .into_iter()
            .map(|name| dspi_session::probe::Feature {
                name: name.into(),
                present: true,
                evidence: "answered".into(),
            })
            .collect();
        let mut session = dspi_session::Session::new(Box::new(t), caps).expect("session");
        let reply = (req.run)(&mut session);
        let sent = log.lock().unwrap().clone();
        (reply, sent)
    }

    fn request(ev: ScreenEvent) -> SessionRequest {
        match ev {
            ScreenEvent::Session(r) => r,
            other => panic!("{other:?}"),
        }
    }

    /// The `(index, on)` pairs an enable write put on the wire, in order.
    fn enables(sent: &[Exchange]) -> Vec<(u16, Vec<u8>)> {
        sent.iter()
            .filter(|e| e.opcode == dspi_proto::generated::opcodes::REQ_SET_SPDIF_INPUT_ENABLE)
            .map(|e| (e.value & 0xFF, vec![u8::from(e.value >> 8 != 0)]))
            .collect()
    }

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
        let req = request(s.handle(key(KeyCode::Right), &st));
        assert!(s.output_dirty());
        let (reply, sent) = answered(&req, st::SUCCESS);
        assert_eq!(enables(&sent), vec![(2u16, vec![1u8])]);
        assert_eq!(reply, SessionReply::Ok("S/PDIF inputs set to 3".into()));

        // And going down to one turns them off from the top.
        let (mut s, st) = screen(Page::Inputs);
        s.handle(key(KeyCode::Tab), &st);
        let req = request(s.handle(key(KeyCode::Left), &st));
        let (_, sent) = answered(&req, st::SUCCESS);
        assert_eq!(enables(&sent), vec![(1u16, vec![0u8])]);
    }

    /// A rejected enable leaves the count where it was, and the row says which
    /// input it could not bring up and why.
    #[test]
    fn a_refused_instance_change_names_the_input_and_the_pin() {
        let (mut s, st) = screen(Page::Inputs);
        s.handle(key(KeyCode::Tab), &st);
        let req = request(s.handle(key(KeyCode::Right), &st));
        let (reply, _) = answered(&req, st::PIN_IN_USE);
        assert_eq!(
            reply,
            SessionReply::Err("Can't enable S/PDIF 3: GPIO 22 is unavailable".into())
        );
        s.session_result(req.tag, reply, &st);
        let f = frame(&mut s, &st, 120, 120);
        assert!(f.contains("Can't enable S/PDIF 3"), "{f}");
        assert!(
            !f.contains("S/PDIF inputs set to"),
            "no optimistic row: {f}"
        );
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
        let req = request(s.popup_result(Some(0), &st));
        assert!(s.output_dirty());
        let (reply, sent) = answered(&req, st::SUCCESS);
        assert!(
            sent.iter()
                .any(|e| e.opcode == dspi_proto::generated::opcodes::REQ_SET_SPDIF_RX_PIN),
            "{sent:?}"
        );
        s.session_result(req.tag, reply, &st);
        let f = frame(&mut s, &st, 120, 120);
        assert!(f.contains("S/PDIF 1 RX pin set to GPIO"), "{f}");

        // The same move refused: the device kept its pin, and says so.
        let (mut s, st) = screen(Page::Inputs);
        s.handle(key(KeyCode::Tab), &st);
        s.handle(key(KeyCode::Down), &st);
        s.handle(key(KeyCode::Enter), &st);
        let req = request(s.popup_result(Some(0), &st));
        let (reply, _) = answered(&req, st::INVALID_PIN);
        s.session_result(req.tag, reply, &st);
        let f = frame(&mut s, &st, 120, 120);
        assert!(f.contains("is not available on this platform"), "{f}");
        assert!(!f.contains("RX pin set to GPIO"), "{f}");
    }

    /// The two lists the Console reads from the device: the S/PDIF inventory
    /// count and the platform's I2S pair limit. Offering counts the device
    /// will refuse is the working agreement's "nothing about device shape
    /// compiled in" the wrong way round.
    #[test]
    fn the_two_count_lists_come_from_the_device() {
        let st = state();
        let mut d = data();
        // A three-input firmware: the fourth choice never appears.
        d.spdif.as_mut().expect("spdif").count = 3;
        let cx = Cx {
            state: &st,
            data: &d,
            config: &AppConfig::default(),
            connected: true,
            global_dirty: false,
        };
        assert_eq!(InputsPage::spdif_inventory(&cx), 3);
        let mut s =
            SettingsScreen::new(&st, d.clone(), AppConfig::default()).open(Page::Inputs, &st);
        s.handle(key(KeyCode::Tab), &st);
        match s.handle(key(KeyCode::Enter), &st) {
            ScreenEvent::Popup(p) => assert_eq!(p.items, vec!["1", "2", "3"]),
            other => panic!("{other:?}"),
        }

        // Channels stride by two up to the part's own limit; a stereo-only
        // part gets one choice and no picker.
        assert_eq!(InputsPage::max_i2s_channels(&cx), 8, "RP2350");
        let mut small = st.clone();
        small.caps.platform = dspi_proto::Platform::Rp2040;
        let cx = Cx {
            state: &small,
            data: &d,
            config: &AppConfig::default(),
            connected: true,
            global_dirty: false,
        };
        assert_eq!(InputsPage::max_i2s_channels(&cx), 2, "RP2040");
        let rows = InputsPage::default().build(&cx);
        let channels = rows
            .iter()
            .find(|(i, _)| *i == Some(Item::I2sChannels))
            .map(|(_, r)| r.clone())
            .expect("a channels row");
        match channels {
            Row::Pick {
                choices, enabled, ..
            } => {
                assert_eq!(choices, vec!["2"]);
                assert!(!enabled, "one choice is not a picker");
            }
            other => panic!("{other:?}"),
        }
    }

    /// The Console's I2S Input footer, in both its wordings.
    #[test]
    fn the_i2s_input_section_carries_its_footer() {
        let (mut s, st) = screen(Page::Inputs);
        let f = frame(&mut s, &st, 120, 120);
        assert!(
            f.contains("Wire one ADC serial-data line per stereo pair"),
            "{f}"
        );
        assert!(f.contains("Save a preset to keep this wiring."), "{f}");

        let mut small = state();
        small.caps.platform = dspi_proto::Platform::Rp2040;
        let mut s =
            SettingsScreen::new(&small, data(), AppConfig::default()).open(Page::Inputs, &small);
        let f = frame(&mut s, &small, 120, 120);
        assert!(
            f.contains("Wire the ADC's serial-data line to the GPIO above."),
            "{f}"
        );
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
