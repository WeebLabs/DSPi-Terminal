//! Settings > Overview: the pin map.
//!
//! Read-only, and deliberately so: the pin pickers on the other pages each
//! answer "is this one free?", and this answers "what does the whole header
//! look like right now". Only *held* pins are listed, which is the Console's
//! rule and the firmware's: a feature that is configured but switched off
//! reserves nothing, so a disabled optional S/PDIF input, a stopped ADAT
//! stream and a control interface that never came up appear nowhere.
//!
//! [`claims_from_state`] is the fallback the page draws with before Settings
//! has been able to ask the device: everything the bulk snapshot alone can
//! prove. It is deliberately a subset of `PinMap::build`, which additionally
//! reads the control interfaces and the control-surface bindings; those need a
//! session, and a page never has one.

use dspi_session::DeviceState;
use dspi_session::pins::{PinClaim, PinRole as ClaimRole, valid_pins};

use crate::widgets::{Action, KeyHelp, PinCell, PinRole};

use super::{Cx, PageEvent, Row, SettingsPage};

/// The claims the bulk snapshot alone proves: physical outputs, the I2S
/// clocks, the audio inputs, the ADAT output and the DAC mute.
///
/// The rules are the firmware's, copied from `pins.rs` so the two cannot
/// disagree about whether a pin is held: a disabled feature holds nothing, the
/// slave clock pair is claimed only in split mode, and LRCLK is always BCK + 1.
pub fn claims_from_state(state: &DeviceState) -> Vec<PinClaim> {
    let mut claims: Vec<PinClaim> = Vec::new();
    let mut claim = |gpio: u8, role: ClaimRole, owner: &str| {
        if !claims.iter().any(|c: &PinClaim| c.gpio == gpio) {
            claims.push(PinClaim {
                gpio,
                role,
                owner: owner.to_string(),
            });
        }
    };

    let pins = state.output_pins();
    let slots = pins.len().saturating_sub(1);
    for (i, gpio) in pins.iter().enumerate() {
        if i < slots {
            claim(*gpio, ClaimRole::Output, &format!("Output {}", i + 1));
        } else {
            claim(*gpio, ClaimRole::Output, "Subwoofer");
        }
    }

    let i2s = state.i2s();
    claim(i2s.bck_pin, ClaimRole::Clock, "I2S BCK");
    claim(i2s.bck_pin.wrapping_add(1), ClaimRole::Clock, "I2S LRCLK");
    if i2s.clock_pin_mode == Some(1) && i2s.bck_pin_slave != 0 {
        claim(i2s.bck_pin_slave, ClaimRole::Clock, "I2S Slave BCK");
        claim(
            i2s.bck_pin_slave.wrapping_add(1),
            ClaimRole::Clock,
            "I2S Slave LRCLK",
        );
    }
    if i2s.mck_enabled {
        claim(i2s.mck_pin, ClaimRole::Clock, "I2S MCK");
    }

    if let Some(input) = state.input_config() {
        let ext = input.spdif_rx_enabled_ext.unwrap_or(0);
        claim(
            input.spdif_rx_pin,
            ClaimRole::Input,
            if ext != 0 { "S/PDIF 1 RX" } else { "S/PDIF RX" },
        );
        for (i, pin) in input.spdif_rx_pin_ext.iter().enumerate() {
            if ext & (1u8 << i) != 0
                && let Some(g) = pin
            {
                claim(*g, ClaimRole::Input, &format!("S/PDIF {} RX", i + 2));
            }
        }
        let pairs = input.i2s_input_channels.unwrap_or(2).max(2) as usize / 2;
        let multi = pairs > 1;
        let label = |pair: usize| {
            if multi {
                format!("I2S RX {}", pair + 1)
            } else {
                "I2S RX".to_string()
            }
        };
        if pairs >= 1 {
            claim(input.i2s_rx_pin, ClaimRole::Input, &label(0));
        }
        for (i, pin) in input.i2s_rx_pin_ext.iter().enumerate() {
            if i + 1 < pairs
                && let Some(g) = pin
            {
                claim(*g, ClaimRole::Input, &label(i + 1));
            }
        }
        if input.adat_input_enabled == Some(true)
            && let Some(pin) = input.adat_input_pin
        {
            claim(pin, ClaimRole::Input, "ADAT Input");
        }
    }

    let (adat_on, adat_pin) = state.adat_output();
    if adat_on {
        claim(adat_pin, ClaimRole::Output, "ADAT Output");
    }

    let dac = state.dac_hw_mute();
    if dac.enabled && dac.pin != dspi_proto::packets::DacHwMuteConfig::PIN_NONE {
        claim(dac.pin, ClaimRole::Utility, "DAC Mute");
    }

    claims.sort_by_key(|c| c.gpio);
    claims
}

/// The kit's tint role for a session claim role.
fn tint(role: ClaimRole) -> PinRole {
    match role {
        ClaimRole::Output => PinRole::Output,
        ClaimRole::Clock => PinRole::Clock,
        ClaimRole::Input => PinRole::Input,
        ClaimRole::Control => PinRole::Control,
        ClaimRole::Utility => PinRole::Utility,
    }
}

/// The Console's section order: Outputs, Clocks, Inputs, Control, Other.
const ROLE_ORDER: [ClaimRole; 5] = [
    ClaimRole::Output,
    ClaimRole::Clock,
    ClaimRole::Input,
    ClaimRole::Control,
    ClaimRole::Utility,
];

#[derive(Debug, Default)]
pub struct OverviewPage {
    cursor: usize,
    grid_cursor: usize,
}

const KEYS: &[KeyHelp] = &[
    KeyHelp::new("← →", "Walk the pin map"),
    KeyHelp::new("↑ ↓", "Scroll"),
    KeyHelp::new("Tab", "Sidebar, page, save bar"),
    KeyHelp::new("[ ]", "Back, forward"),
    KeyHelp::new("Esc", "Leave Settings"),
];

impl OverviewPage {
    fn cells(&self, cx: &Cx<'_>) -> Vec<PinCell> {
        let claims = cx.data.pin_claims(cx.state);
        valid_pins(cx.platform())
            .into_iter()
            .map(|gpio| PinCell {
                gpio,
                owner: claims
                    .iter()
                    .find(|c| c.gpio == gpio)
                    .map(|c| (tint(c.role), c.owner.clone())),
            })
            .collect()
    }
}

impl SettingsPage for OverviewPage {
    fn rows(&self, cx: &Cx<'_>) -> Vec<Row> {
        if !cx.connected {
            return vec![
                Row::pair("No Device Connected", String::new()),
                Row::note(
                    "Pin assignments live on the device. Connect a DSPi to see which GPIOs are \
                     in use.",
                ),
            ];
        }
        let claims = cx.data.pin_claims(cx.state);
        if claims.is_empty() {
            return vec![Row::note("No GPIOs are currently claimed.")];
        }
        let total = valid_pins(cx.platform()).len();
        let free = total.saturating_sub(claims.len());
        let mut rows = vec![
            Row::pair(
                format!("{} of {total} GPIOs in use", claims.len()),
                if free == 0 {
                    "none free".to_string()
                } else {
                    format!("{free} free")
                },
            ),
            Row::Grid {
                cells: self.cells(cx),
                cursor: self.grid_cursor,
            },
        ];
        for role in ROLE_ORDER {
            let rows_for: Vec<&PinClaim> = claims.iter().filter(|c| c.role == role).collect();
            if rows_for.is_empty() {
                continue;
            }
            rows.push(Row::Blank);
            rows.push(Row::section(tint(role).name()));
            for c in rows_for {
                rows.push(Row::pair(format!("GP{}", c.gpio), c.owner.clone()));
            }
        }
        rows
    }

    fn act(&mut self, _index: usize, action: Action, cx: &Cx<'_>) -> PageEvent {
        if let Action::Selected(i) = action {
            self.grid_cursor = i;
            let cells = self.cells(cx);
            return match cells.get(i) {
                Some(PinCell {
                    gpio,
                    owner: Some((role, label)),
                }) => PageEvent::Status(format!("GP{gpio}: {label} ({})", role.name())),
                Some(PinCell { gpio, owner: None }) => {
                    PageEvent::Status(format!("GP{gpio} - available"))
                }
                None => PageEvent::Handled,
            };
        }
        PageEvent::Handled
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
}

#[cfg(test)]
mod tests {
    use super::super::Page;
    use super::super::tests::{data, frame, key, screen, state};
    use super::*;
    use crate::shell::{Screen, ScreenEvent};
    use crossterm::event::KeyCode;

    #[test]
    fn the_overview_summarises_and_maps_every_gpio() {
        let (mut s, st) = screen(Page::Overview);
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            let f = frame(&mut s, &st, w, h);
            assert_eq!(f.lines().count(), h as usize);
            assert!(f.contains("GPIOs in use"), "{w}x{h}: {f}");
            assert!(f.contains("GP0"), "the map: {w}x{h}\n{f}");
        }
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("free"), "{f}");
        assert!(f.contains("OUTPUTS"), "{f}");
        assert!(f.contains("CLOCKS"), "{f}");
        assert!(f.contains("INPUTS"), "{f}");
    }

    #[test]
    fn the_claims_are_the_firmwares_rules() {
        let st = state();
        let claims = claims_from_state(&st);
        let owner = |g: u8| {
            claims
                .iter()
                .find(|c| c.gpio == g)
                .map(|c| c.owner.clone())
                .unwrap_or_default()
        };
        assert_eq!(owner(6), "Output 1");
        assert_eq!(owner(10), "Subwoofer");
        assert_eq!(owner(14), "I2S BCK");
        assert_eq!(owner(15), "I2S LRCLK");
        assert_eq!(owner(13), "I2S MCK");
        assert_eq!(owner(5), "S/PDIF 1 RX");
        assert_eq!(owner(21), "S/PDIF 2 RX");
        assert_eq!(owner(12), "ADAT Output");
        assert_eq!(owner(11), "DAC Mute");
        // An unclaimed pin is unclaimed: GPIO 0 is nobody's.
        assert!(claims.iter().all(|c| c.gpio != 0));
    }

    #[test]
    fn a_claimed_pin_is_marked_in_the_map_and_named_on_the_echo_line() {
        let mut st = state();
        st.caps.features.clear();
        let cx = Cx {
            state: &st,
            data: &data(),
            config: &super::super::AppConfig::default(),
            connected: true,
            global_dirty: false,
        };
        let page = OverviewPage::default();
        let cells = page.cells(&cx);
        let cell = cells.iter().find(|c| c.gpio == 6).expect("GP6");
        assert_eq!(
            cell.owner,
            Some((PinRole::Output, "Output 1".to_string())),
            "a claimed pin carries its owner and role"
        );
        assert!(cells.iter().find(|c| c.gpio == 0).unwrap().owner.is_none());

        // And walking the grid says who holds the pin under the cursor.
        let (mut s, st) = screen(Page::Overview);
        s.handle(key(KeyCode::Tab), &st);
        let mut last = ScreenEvent::Handled;
        for _ in 0..6 {
            last = s.handle(key(KeyCode::Right), &st);
        }
        assert_eq!(
            last,
            ScreenEvent::Status("GP6: Output 1 (Outputs)".into()),
            "the echo line names the owner"
        );
    }

    #[test]
    fn a_disconnected_overview_says_where_the_pins_live() {
        let st = state();
        let mut s =
            super::super::SettingsScreen::new(&st, data(), super::super::AppConfig::default())
                .connected(false)
                .open(Page::Overview, &st);
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("No Device Connected"), "{f}");
        assert!(f.contains("Pin assignments live on the device."), "{f}");
    }
}
