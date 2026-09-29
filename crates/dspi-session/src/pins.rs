//! Who owns each GPIO.
//!
//! Every pin picker in the app asks the same question, and if each asked it
//! separately a pin claimed in one place would still look free in another. So
//! there is one map, built from the device's live configuration, and every
//! picker and every conflict message reads it.
//!
//! The rules are the firmware's own, not a tidier version of them, because the
//! point of the map is to predict what the device will accept:
//!
//! - A feature that is merely configured does not hold a pin. A disabled
//!   S/PDIF input, a disabled ADAT stream, a control interface kept down by a
//!   boot-time collision and a control-surface binding that failed to come up
//!   all hold nothing (survey 6.6, 6.7, 6.12).
//! - The I2S slave clock pair is claimed only in SPLIT clock-pin mode; in
//!   UNIFIED the pair is dormant and constrains nothing.
//! - LRCLK is always BCK + 1, on both roles.

use dspi_proto::Platform;
use dspi_proto::generated::opcodes as op;
use dspi_proto::packets::{CtrlIfaceStatus, DacHwMuteConfig, I2cCtrlConfig, UartCtrlConfig};
use dspi_proto::wire::InputConfig;

use crate::surfaces;
use crate::write::{Session, WriteError};

/// What a claimed GPIO is doing, for grouping on the pin overview.
///
/// Kept beside the claim itself so display code never has to infer a category
/// by matching on the owner's name. These are the Console's five roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PinRole {
    Output,
    Clock,
    Input,
    Control,
    Utility,
}

impl PinRole {
    pub fn label(self) -> &'static str {
        match self {
            PinRole::Output => "Output",
            PinRole::Clock => "Clock",
            PinRole::Input => "Input",
            PinRole::Control => "Control",
            PinRole::Utility => "Utility",
        }
    }
}

/// One GPIO and the feature holding it.
///
/// `owner` is the name the Console shows in its conflict messages, verbatim, so
/// the same pin reads the same way in both apps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinClaim {
    pub gpio: u8,
    pub role: PinRole,
    pub owner: String,
}

/// Which mux a picker needs the pin to carry.
///
/// The device does the authoritative same-instance validation on apply; this
/// narrows the list to pins that can possibly work, so a user is not offered a
/// choice that can only fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinConstraint {
    /// Every valid GPIO.
    Any,
    /// ADC-capable only, which a potentiometer needs: GPIO 26 to 28 on both
    /// platforms (control_surfaces.h:270).
    Adc,
    /// `pin % 4 == 0` (config.h:527).
    UartTx,
    /// `pin % 4 == 1`, same instance as TX (config.h:528).
    UartRx,
    /// Even, the SDA half of a mux pair (config.h:537).
    I2cSda,
    /// Odd, the SCL above its SDA (config.h:538).
    I2cScl,
    /// A `CLK_GPOUT`-capable pin, which is the only thing MCK can be driven
    /// from: GPIO 21 on RP2040, 13, 15 or 21 on RP2350 (survey 6.7).
    Mck,
}

/// Every GPIO this build will offer, mirroring the firmware's
/// `is_valid_gpio_pin()`: 0 to 22 and 26 to 28.
///
/// 23 to 25 are power and LED. RP2350 additionally allows GPIO 29, but the
/// list is shared with RP2040, whose highest is 28, so 29 is not offered.
pub fn valid_pins(_platform: Platform) -> Vec<u8> {
    (0..=22u8).chain(26..=28).collect()
}

/// The pins a picker may offer for one kind of function.
pub fn candidates(platform: Platform, constraint: PinConstraint) -> Vec<u8> {
    match constraint {
        PinConstraint::Mck => mck_pins(platform),
        PinConstraint::Adc => vec![26, 27, 28],
        other => valid_pins(platform)
            .into_iter()
            .filter(|p| match other {
                PinConstraint::UartTx => p % 4 == 0,
                PinConstraint::UartRx => p % 4 == 1,
                PinConstraint::I2cSda => p % 2 == 0,
                PinConstraint::I2cScl => p % 2 == 1,
                _ => true,
            })
            .collect(),
    }
}

/// The `CLK_GPOUT`-capable pins, per platform.
pub fn mck_pins(platform: Platform) -> Vec<u8> {
    match platform {
        Platform::Rp2350 => vec![13, 15, 21],
        // RP2040's other GPOUT pins, 23 to 25, are board-reserved.
        _ => vec![21],
    }
}

/// The live pin ownership map.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PinMap {
    claims: Vec<PinClaim>,
    /// Whether any output slot is configured for I2S, which is what makes
    /// GPIO 15 unavailable for MCK on an RP2350: it is the LRCLK pin.
    any_slot_i2s: bool,
}

impl PinMap {
    /// The claim on `gpio`, or `None` if it is free.
    pub fn owner_of(&self, gpio: u8) -> Option<&PinClaim> {
        self.claims.iter().find(|c| c.gpio == gpio)
    }

    pub fn free(&self, gpio: u8) -> bool {
        self.owner_of(gpio).is_none()
    }

    /// Every claim, in pin order.
    pub fn claims(&self) -> &[PinClaim] {
        &self.claims
    }

    /// Every valid GPIO that nothing holds.
    pub fn free_pins(&self, platform: Platform) -> Vec<u8> {
        valid_pins(platform)
            .into_iter()
            .filter(|p| self.free(*p))
            .collect()
    }

    /// MCK candidates for this device, with GPIO 15 dropped while an output
    /// slot is I2S: it is that slot's LRCLK, so choosing it would always fail
    /// the firmware's own in-use check.
    pub fn mck_candidates(&self, platform: Platform) -> Vec<u8> {
        mck_pins(platform)
            .into_iter()
            .filter(|p| !(self.any_slot_i2s && *p == 15))
            .collect()
    }

    fn claim(&mut self, gpio: u8, role: PinRole, owner: impl Into<String>) {
        // First claim wins, which matches the order the firmware checks in and
        // keeps the message stable when two features share a pin illegally.
        if self.free(gpio) {
            self.claims.push(PinClaim {
                gpio,
                role,
                owner: owner.into(),
            });
        }
    }

    /// Read every pin-bearing setting and work out who owns what.
    ///
    /// The audio I/O pins all live in the bulk snapshot, so they cost one
    /// chunked read rather than a dozen round trips. The control interfaces and
    /// the control surfaces have no bulk representation and are read
    /// separately; a stall there means the firmware does not have the feature,
    /// which is information, not a failure.
    pub fn build(session: &mut Session) -> Result<PinMap, WriteError> {
        let bulk = session.snapshot()?;
        let mut map = PinMap::default();

        // --- physical outputs, section 8 (bulk_params.h:123-127) -----------
        if let Some(pins) = bulk.section("pins") {
            let count = pins[0] as usize;
            // The last entry is the PDM sub; the ones before it are the
            // S/PDIF and I2S slots.
            let slots = count.saturating_sub(1);
            for (i, gpio) in pins[1..].iter().take(count).enumerate() {
                if i < slots {
                    map.claim(*gpio, PinRole::Output, format!("Output {}", i + 1));
                } else {
                    map.claim(*gpio, PinRole::Output, "Subwoofer");
                }
            }
        }

        // --- I2S clocks, section 11 (bulk_params.h:154-167) ----------------
        if let Some(i2s) = bulk.section("i2s_config") {
            map.any_slot_i2s = i2s[..4].contains(&1);

            let bck = i2s[4];
            map.claim(bck, PinRole::Clock, "I2S BCK");
            map.claim(bck.wrapping_add(1), PinRole::Clock, "I2S LRCLK");

            // The slave pair is claimed only in SPLIT mode; in UNIFIED it is
            // dormant and constrains nothing.
            let split = dspi_proto::wire::decode_p1(i2s[8]) == Some(1);
            let slave_bck = i2s[9];
            if split && slave_bck != 0 {
                map.claim(slave_bck, PinRole::Clock, "I2S Slave BCK");
                map.claim(slave_bck.wrapping_add(1), PinRole::Clock, "I2S Slave LRCLK");
            }

            // MCK holds its pin only while it is enabled.
            if i2s[6] != 0 {
                map.claim(i2s[5], PinRole::Clock, "I2S MCK");
            }
        }

        // --- inputs, section 15 (bulk_params.h:203-233) --------------------
        if let Some(input) = bulk.input_config() {
            map.claim_inputs(&input);
        }

        // --- ADAT output, section 20 (bulk_params.h:328-332) ---------------
        if let Some(adat) = bulk.section("adat_config")
            && adat[0] != 0
        {
            map.claim(adat[1], PinRole::Output, "ADAT Output");
        }

        // --- DAC hardware mute, section 18 (bulk_params.h:286-294) ---------
        if let Some(dac) = bulk.section("dac_hw_mute")
            && let Ok(cfg) = DacHwMuteConfig::decode(dac)
            && cfg.enabled
            && cfg.pin != DacHwMuteConfig::PIN_NONE
        {
            map.claim(cfg.pin, PinRole::Utility, "DAC Mute");
        }

        map.claim_control_interfaces(session);
        map.claim_control_surfaces(session);

        map.claims.sort_by_key(|c| c.gpio);
        Ok(map)
    }

    /// S/PDIF and I2S input pins.
    ///
    /// A disabled optional S/PDIF input's pin is a stored preference, not a
    /// claim (survey 6.6). Note which enable mask this is: the bulk packet
    /// carries `spdif_rx_enabled_ext`, whose bit 0 is S/PDIF **2**, one bit
    /// apart from the mask `REQ_GET_SPDIF_INPUT_CONFIG` answers with. See
    /// docs/firmware-notes.md section 14.
    fn claim_inputs(&mut self, input: &InputConfig) {
        let ext_mask = input.spdif_rx_enabled_ext.unwrap_or(0);
        let any_optional = ext_mask != 0;

        self.claim(
            input.spdif_rx_pin,
            PinRole::Input,
            if any_optional {
                "S/PDIF 1 RX"
            } else {
                "S/PDIF RX"
            },
        );
        for (i, pin) in input.spdif_rx_pin_ext.iter().enumerate() {
            if ext_mask & (1u8 << i) != 0
                && let Some(gpio) = pin
            {
                self.claim(*gpio, PinRole::Input, format!("S/PDIF {} RX", i + 2));
            }
        }

        // I2S RX: only the pairs the active channel count actually uses. A
        // placeholder pin on an inactive pair does not lock a GPIO out of
        // another function (the firmware's own two-tier rule).
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
            self.claim(input.i2s_rx_pin, PinRole::Input, label(0));
        }
        for (i, pin) in input.i2s_rx_pin_ext.iter().enumerate() {
            if i + 1 < pairs
                && let Some(gpio) = pin
            {
                self.claim(*gpio, PinRole::Input, label(i + 1));
            }
        }

        // ADAT input holds its GPIO only while enabled, like the output.
        if input.adat_input_enabled == Some(true)
            && let Some(pin) = input.adat_input_pin
        {
            self.claim(pin, PinRole::Input, "ADAT Input");
        }
    }

    /// UART and I2C hold their pins only while they are actually up.
    ///
    /// A stored-but-enabled config whose pins collided at boot is kept down by
    /// the firmware and holds nothing, which is why this reads the status as
    /// well as the config (survey 6.12).
    fn claim_control_interfaces(&mut self, session: &mut Session) {
        let status = session
            .with_transport(|t| t.control_in(op::REQ_GET_CTRL_IFACE_STATUS, 0, 8))
            .ok()
            .and_then(|d| CtrlIfaceStatus::decode(&d).ok())
            .unwrap_or_default();

        if status.uart_live
            && let Ok(cfg) = session
                .with_transport(|t| t.control_in(op::REQ_GET_UART_CONFIG, 0, 8))
                .map_err(|_| ())
                .and_then(|d| UartCtrlConfig::decode(&d).map_err(|_| ()))
        {
            self.claim(cfg.tx_pin, PinRole::Control, "UART Control");
            self.claim(cfg.rx_pin, PinRole::Control, "UART Control");
        }

        if status.i2c_live
            && let Ok(cfg) = session
                .with_transport(|t| t.control_in(op::REQ_GET_I2C_CONFIG, 0, 8))
                .map_err(|_| ())
                .and_then(|d| I2cCtrlConfig::decode(&d).map_err(|_| ()))
        {
            self.claim(cfg.sda_pin, PinRole::Control, "I2C Control");
            self.claim(cfg.scl_pin, PinRole::Control, "I2C Control");
        }
    }

    /// A binding holds its GPIOs only while it is live.
    ///
    /// A cleared binding, or one the firmware kept down because its pins
    /// collided at boot, holds nothing; `active_mask` is the firmware's own
    /// answer to that and is what this reads.
    fn claim_control_surfaces(&mut self, session: &mut Session) {
        let Some(caps) = session.capabilities().cs.clone() else {
            return;
        };
        let Ok(status) = session
            .with_transport(|t| surfaces::read_status(t, caps.max_bindings, caps.max_ir_commands))
        else {
            return;
        };

        for slot in 0..caps.max_bindings {
            if !status.is_slot_active(slot) {
                continue;
            }
            let Ok(binding) = session.with_transport(|t| surfaces::read_binding(t, slot)) else {
                continue;
            };
            if binding.is_empty() {
                continue;
            }
            // A display's two pins are SDA then SCL, and an encoder's are its
            // two channels; both come back from the same accessor.
            for gpio in binding.pins() {
                self.claim(
                    gpio,
                    PinRole::Control,
                    format!("Control Surface {}", slot + 1),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::{Capabilities, ChannelInfo, ControlSurfaceCaps};
    use dspi_proto::generated;
    use dspi_proto::packets::{CsBinding, CsStatusPacket, GPIO_UNUSED};
    use dspi_transport::MockTransport;

    /// The bulk sections this module reads by name. A section that is renamed
    /// or dropped should fail here rather than quietly stop claiming pins.
    const SECTIONS_READ: [&str; 5] = [
        "pins",
        "i2s_config",
        "input_config",
        "dac_hw_mute",
        "adat_config",
    ];

    fn caps(platform: Platform) -> Capabilities {
        let (num_inputs, num_outputs) = match platform {
            Platform::Rp2350 => (8u8, 9u8),
            _ => (2, 5),
        };
        Capabilities {
            serial: "MOCK".into(),
            platform,
            firmware: "1.1.6".into(),
            wire_format: dspi_proto::generated::wire::WIRE_FORMAT_VERSION as u8,
            num_channels: num_inputs + num_outputs,
            num_inputs,
            num_outputs,
            max_bands: 10,
            band_storage: 12,
            channels: (0..num_inputs + num_outputs)
                .map(|i| ChannelInfo {
                    index: i,
                    name: format!("ch{i}"),
                    slug: format!("ch{i}"),
                    is_output: i >= num_inputs,
                })
                .collect(),
            features: Vec::new(),
            cs: Some(ControlSurfaceCaps {
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
            }),
            siggen: None,
            active_preset: Some(0),
        }
    }

    /// A bulk packet with the pin-bearing sections filled in.
    struct Device {
        bytes: Vec<u8>,
    }

    impl Device {
        fn new() -> Self {
            let mut bytes = vec![0u8; generated::BULK_SIZE];
            bytes[0] = generated::wire::WIRE_FORMAT_VERSION as u8;
            bytes[1] = 1; // RP2350
            bytes[2] = 17;
            bytes[3] = 9;
            bytes[4] = 8;
            bytes[5] = 12;
            let len = (generated::BULK_SIZE as u16).to_le_bytes();
            bytes[6..8].copy_from_slice(&len);
            Self { bytes }
        }

        fn section(&mut self, name: &str) -> &mut [u8] {
            let (_, off, len) = generated::SECTIONS
                .iter()
                .find(|(n, _, _)| *n == name)
                .copied()
                .expect("section");
            &mut self.bytes[off..off + len]
        }

        /// Five physical outputs: four slots and the PDM sub.
        fn with_outputs(mut self, pins: [u8; 5]) -> Self {
            let s = self.section("pins");
            s[0] = 5;
            s[1..6].copy_from_slice(&pins);
            self
        }

        fn with_i2s(mut self, bck: u8, mck: Option<u8>, slave: Option<(u8, bool)>) -> Self {
            let s = self.section("i2s_config");
            s[4] = bck;
            if let Some(pin) = mck {
                s[5] = pin;
                s[6] = 1;
            }
            if let Some((pin, split)) = slave {
                s[8] = if split { 2 } else { 1 }; // clock_pin_mode_p1
                s[9] = pin;
            }
            self
        }

        fn with_i2s_slot_type(mut self, slot: usize, is_i2s: bool) -> Self {
            self.section("i2s_config")[slot] = is_i2s as u8;
            self
        }

        fn with_input(
            mut self,
            spdif: u8,
            spdif_ext: [u8; 3],
            ext_enabled: u8,
            i2s_rx: u8,
            i2s_rx_ext: [u8; 3],
            channels: u8,
        ) -> Self {
            let s = self.section("input_config");
            s[1] = spdif;
            s[2] = i2s_rx;
            s[4] = channels;
            s[5..8].copy_from_slice(&i2s_rx_ext);
            s[8..11].copy_from_slice(&spdif_ext);
            s[11] = ext_enabled + 1; // stored plus one
            self
        }

        fn with_adat_out(mut self, pin: u8, enabled: bool) -> Self {
            let s = self.section("adat_config");
            s[0] = enabled as u8;
            s[1] = pin;
            self
        }

        fn with_adat_in(mut self, pin: u8, enabled: bool) -> Self {
            let s = self.section("input_config");
            s[13] = pin;
            s[14] = enabled as u8 + 1; // stored plus one
            self
        }

        fn with_dac_mute(mut self, pin: u8, enabled: bool) -> Self {
            let s = self.section("dac_hw_mute");
            s[0] = enabled as u8;
            s[2] = pin;
            self
        }

        fn transport(self) -> MockTransport {
            MockTransport::new()
                .window(op::REQ_GET_ALL_PARAMS_CHUNK, self.bytes)
                // Both control interfaces down, no live bindings, unless a
                // test overrides these.
                .data(op::REQ_GET_CTRL_IFACE_STATUS, vec![0u8; 8])
                .data(op::REQ_GET_CS_STATUS, vec![0u8; 41])
        }
    }

    fn session_from(t: MockTransport, platform: Platform) -> Session {
        Session::new(Box::new(t), caps(platform)).expect("session")
    }

    fn map_of(t: MockTransport, platform: Platform) -> PinMap {
        PinMap::build(&mut session_from(t, platform)).expect("pin map")
    }

    #[test]
    fn the_valid_pins_are_the_firmwares_own_list() {
        let pins = valid_pins(Platform::Rp2350);
        assert_eq!(pins.len(), 26);
        assert!(pins.contains(&0) && pins.contains(&22));
        // 23 to 25 are power and the LED.
        assert!(!pins.contains(&23) && !pins.contains(&24) && !pins.contains(&25));
        assert!(pins.contains(&26) && pins.contains(&28));
        assert!(!pins.contains(&29), "not offered, since RP2040 has no 29");
    }

    #[test]
    fn the_sections_this_module_reads_all_exist() {
        for name in SECTIONS_READ {
            assert!(
                generated::SECTIONS.iter().any(|(n, _, _)| *n == name),
                "the bulk section `{name}` this module reads is gone"
            );
        }
    }

    #[test]
    fn output_pins_are_claimed_with_the_sub_last() {
        let map = map_of(
            Device::new().with_outputs([2, 3, 6, 7, 10]).transport(),
            Platform::Rp2350,
        );
        assert_eq!(map.owner_of(2).unwrap().owner, "Output 1");
        assert_eq!(map.owner_of(7).unwrap().owner, "Output 4");
        assert_eq!(map.owner_of(10).unwrap().owner, "Subwoofer");
        assert_eq!(map.owner_of(2).unwrap().role, PinRole::Output);
    }

    /// LRCLK is BCK + 1 and is never configured separately, so a map that only
    /// claimed BCK would offer the LRCLK pin to something else.
    #[test]
    fn the_clock_pair_claims_both_halves() {
        let map = map_of(
            Device::new().with_i2s(14, None, None).transport(),
            Platform::Rp2350,
        );
        assert_eq!(map.owner_of(14).unwrap().owner, "I2S BCK");
        assert_eq!(map.owner_of(15).unwrap().owner, "I2S LRCLK");
        assert_eq!(map.owner_of(15).unwrap().role, PinRole::Clock);
    }

    /// In SPLIT mode both pairs are claimed at once, which is the whole reason
    /// the mode exists; in UNIFIED the slave pair is dormant.
    #[test]
    fn the_slave_pair_is_claimed_only_in_split_mode() {
        let unified = map_of(
            Device::new()
                .with_i2s(14, None, Some((26, false)))
                .transport(),
            Platform::Rp2350,
        );
        assert!(unified.free(26), "a dormant slave pair constrains nothing");
        assert!(unified.free(27));

        let split = map_of(
            Device::new()
                .with_i2s(14, None, Some((26, true)))
                .transport(),
            Platform::Rp2350,
        );
        assert_eq!(split.owner_of(26).unwrap().owner, "I2S Slave BCK");
        assert_eq!(split.owner_of(27).unwrap().owner, "I2S Slave LRCLK");
        assert_eq!(split.owner_of(14).unwrap().owner, "I2S BCK");
    }

    #[test]
    fn mck_holds_its_pin_only_while_enabled() {
        let on = map_of(
            Device::new().with_i2s(14, Some(13), None).transport(),
            Platform::Rp2350,
        );
        assert_eq!(on.owner_of(13).unwrap().owner, "I2S MCK");

        let mut dev = Device::new().with_i2s(14, Some(13), None);
        dev.section("i2s_config")[6] = 0; // disabled
        assert!(map_of(dev.transport(), Platform::Rp2350).free(13));
    }

    /// A disabled optional input's pin is a stored preference, not a claim.
    #[test]
    fn a_disabled_spdif_input_holds_no_pin() {
        // ext mask bit 0 is S/PDIF 2, so 0b010 enables S/PDIF 3 alone.
        let map = map_of(
            Device::new()
                .with_input(5, [20, 21, 22], 0b010, 1, [2, 3, 4], 2)
                .transport(),
            Platform::Rp2350,
        );
        assert_eq!(map.owner_of(5).unwrap().owner, "S/PDIF 1 RX");
        assert!(map.free(20), "S/PDIF 2 is disabled");
        assert_eq!(map.owner_of(21).unwrap().owner, "S/PDIF 3 RX");
        assert!(map.free(22), "S/PDIF 4 is disabled");
    }

    #[test]
    fn a_lone_spdif_input_reads_without_a_number() {
        let map = map_of(
            Device::new()
                .with_input(5, [20, 21, 22], 0, 1, [2, 3, 4], 2)
                .transport(),
            Platform::Rp2350,
        );
        assert_eq!(map.owner_of(5).unwrap().owner, "S/PDIF RX");
    }

    /// Only the pairs the active channel count uses are claimed: a placeholder
    /// pin on an inactive pair must not lock a GPIO out of another function.
    #[test]
    fn i2s_rx_claims_only_the_active_pairs() {
        let stereo = map_of(
            Device::new()
                .with_i2s(14, None, None)
                .with_input(5, [0, 0, 0], 0, 1, [2, 3, 4], 2)
                .transport(),
            Platform::Rp2350,
        );
        assert_eq!(stereo.owner_of(1).unwrap().owner, "I2S RX");
        assert!(stereo.free(2), "pair 2 is not active in stereo mode");

        let eight = map_of(
            Device::new()
                .with_i2s(14, None, None)
                .with_input(5, [0, 0, 0], 0, 1, [2, 3, 4], 8)
                .transport(),
            Platform::Rp2350,
        );
        assert_eq!(eight.owner_of(1).unwrap().owner, "I2S RX 1");
        assert_eq!(eight.owner_of(4).unwrap().owner, "I2S RX 4");
    }

    #[test]
    fn adat_holds_its_pins_only_while_enabled() {
        let off = map_of(
            Device::new()
                .with_adat_out(12, false)
                .with_adat_in(9, false)
                .transport(),
            Platform::Rp2350,
        );
        assert!(off.free(12) && off.free(9));

        let on = map_of(
            Device::new()
                .with_adat_out(12, true)
                .with_adat_in(9, true)
                .transport(),
            Platform::Rp2350,
        );
        assert_eq!(on.owner_of(12).unwrap().owner, "ADAT Output");
        assert_eq!(on.owner_of(9).unwrap().owner, "ADAT Input");
        assert_eq!(on.owner_of(9).unwrap().role, PinRole::Input);
    }

    #[test]
    fn the_dac_mute_pin_is_claimed_only_while_enabled() {
        let off = map_of(
            Device::new().with_dac_mute(11, false).transport(),
            Platform::Rp2350,
        );
        assert!(off.free(11));

        let on = map_of(
            Device::new().with_dac_mute(11, true).transport(),
            Platform::Rp2350,
        );
        assert_eq!(on.owner_of(11).unwrap().owner, "DAC Mute");
        assert_eq!(on.owner_of(11).unwrap().role, PinRole::Utility);
    }

    /// A control interface that is configured but not up holds nothing: the
    /// firmware kept it down, so its pins really are free.
    #[test]
    fn the_control_interfaces_are_claimed_only_while_live() {
        let cfg_uart = UartCtrlConfig {
            enabled: true,
            tx_pin: 16,
            rx_pin: 17,
            notify_enable: false,
            baud: 115_200,
        };
        let cfg_i2c = I2cCtrlConfig {
            enabled: true,
            sda_pin: 18,
            scl_pin: 19,
            address: 0x42,
        };

        let down = Device::new()
            .transport()
            .data(op::REQ_GET_UART_CONFIG, cfg_uart.encode().to_vec())
            .data(op::REQ_GET_I2C_CONFIG, cfg_i2c.encode().to_vec());
        let map = map_of(down, Platform::Rp2350);
        assert!(map.free(16) && map.free(17) && map.free(18) && map.free(19));

        let live = CtrlIfaceStatus {
            uart_last_status: 0,
            uart_live: true,
            i2c_last_status: 0,
            i2c_live: true,
            proto_version: 1,
        };
        let up = Device::new()
            .transport()
            .data(op::REQ_GET_CTRL_IFACE_STATUS, live.encode().to_vec())
            .data(op::REQ_GET_UART_CONFIG, cfg_uart.encode().to_vec())
            .data(op::REQ_GET_I2C_CONFIG, cfg_i2c.encode().to_vec());
        let map = map_of(up, Platform::Rp2350);
        assert_eq!(map.owner_of(16).unwrap().owner, "UART Control");
        assert_eq!(map.owner_of(17).unwrap().owner, "UART Control");
        assert_eq!(map.owner_of(18).unwrap().owner, "I2C Control");
        assert_eq!(map.owner_of(19).unwrap().role, PinRole::Control);
    }

    /// Only a live binding holds its GPIOs. One that failed to come up at boot
    /// is visible in `slot_status` and holds nothing.
    #[test]
    fn a_control_surface_binding_claims_its_pins_while_live() {
        let encoder = CsBinding {
            component: 4,
            noun: 1,
            action: 1,
            gpio: [20, 21],
            ..Default::default()
        };
        let mut status = CsStatusPacket {
            max_bindings: 16,
            active_mask: 0b1,
            slot_status: vec![0; 16],
            ir_cmd_status: vec![0; 16],
            ..Default::default()
        };

        let t = Device::new()
            .transport()
            .data(op::REQ_GET_CS_STATUS, status.encode())
            .data(op::REQ_GET_CS_BINDING, encoder.encode().to_vec());
        let map = map_of(t, Platform::Rp2350);
        assert_eq!(map.owner_of(20).unwrap().owner, "Control Surface 1");
        assert_eq!(map.owner_of(21).unwrap().owner, "Control Surface 1");

        // The same binding, not live: its pins are free.
        status.active_mask = 0;
        let t = Device::new()
            .transport()
            .data(op::REQ_GET_CS_STATUS, status.encode())
            .data(op::REQ_GET_CS_BINDING, encoder.encode().to_vec());
        let map = map_of(t, Platform::Rp2350);
        assert!(map.free(20) && map.free(21));
    }

    /// A display binding takes two pins, SDA then SCL, exactly like an
    /// encoder's two channels.
    #[test]
    fn a_display_binding_claims_both_of_its_pins() {
        let display = CsBinding {
            component: 8,
            gpio: [2, 3],
            index: 6,
            ..Default::default()
        };
        let status = CsStatusPacket {
            max_bindings: 16,
            active_mask: 0b1,
            slot_status: vec![0; 16],
            ir_cmd_status: vec![0; 16],
            ..Default::default()
        };
        let t = Device::new()
            .transport()
            .data(op::REQ_GET_CS_STATUS, status.encode())
            .data(op::REQ_GET_CS_BINDING, display.encode().to_vec());
        let map = map_of(t, Platform::Rp2350);
        assert_eq!(map.owner_of(2).unwrap().owner, "Control Surface 1");
        assert_eq!(map.owner_of(3).unwrap().owner, "Control Surface 1");
    }

    /// A single-pin binding writes 0xFF in its second slot, which is not a
    /// GPIO and must never be claimed.
    #[test]
    fn an_unused_second_pin_is_not_a_claim() {
        let button = CsBinding {
            component: 1,
            gpio: [16, GPIO_UNUSED],
            ..Default::default()
        };
        assert_eq!(button.pins(), vec![16]);
    }

    #[test]
    fn free_reports_the_pins_nothing_holds() {
        let map = map_of(
            Device::new()
                .with_outputs([2, 3, 6, 7, 10])
                .with_i2s(14, None, None)
                .with_input(5, [0, 0, 0], 0, 1, [0, 0, 0], 2)
                .transport(),
            Platform::Rp2350,
        );
        assert!(!map.free(2) && !map.free(14) && !map.free(15));
        assert!(map.free(0) && map.free(28));
        let free = map.free_pins(Platform::Rp2350);
        assert!(!free.contains(&2));
        assert!(free.contains(&28));
    }

    #[test]
    fn the_claims_come_back_in_pin_order() {
        let map = map_of(
            Device::new()
                .with_outputs([10, 3, 6, 7, 2])
                .with_i2s(14, None, None)
                .transport(),
            Platform::Rp2350,
        );
        let order: Vec<u8> = map.claims().iter().map(|c| c.gpio).collect();
        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(order, sorted);
    }

    // ------------------------------------------------------------ candidates

    #[test]
    fn a_pot_is_offered_only_analogue_pins() {
        assert_eq!(
            candidates(Platform::Rp2350, PinConstraint::Adc),
            vec![26, 27, 28]
        );
    }

    /// The mux rules, which are what make a pin possible at all: TX is
    /// `pin % 4 == 0`, RX the next one up, SDA even and SCL odd.
    #[test]
    fn the_control_interface_pins_follow_their_mux_rules() {
        let tx = candidates(Platform::Rp2350, PinConstraint::UartTx);
        assert!(tx.iter().all(|p| p % 4 == 0));
        assert!(tx.contains(&16), "the default TX pin must be offerable");

        let rx = candidates(Platform::Rp2350, PinConstraint::UartRx);
        assert!(rx.iter().all(|p| p % 4 == 1));
        assert!(rx.contains(&17));

        let sda = candidates(Platform::Rp2350, PinConstraint::I2cSda);
        assert!(sda.iter().all(|p| p % 2 == 0));
        assert!(sda.contains(&18));

        let scl = candidates(Platform::Rp2350, PinConstraint::I2cScl);
        assert!(scl.iter().all(|p| p % 2 == 1));
        assert!(scl.contains(&19));
    }

    /// MCK is driven from a hardware clock output, so only the CLK_GPOUT pins
    /// can carry it, and there is exactly one on an RP2040.
    #[test]
    fn mck_is_offered_only_clock_capable_pins() {
        assert_eq!(mck_pins(Platform::Rp2040), vec![21]);
        assert_eq!(mck_pins(Platform::Rp2350), vec![13, 15, 21]);
        assert_eq!(
            candidates(Platform::Rp2350, PinConstraint::Mck),
            vec![13, 15, 21]
        );
    }

    /// GPIO 15 is `clk_gpout1` and also the LRCLK of an I2S slot, so while a
    /// slot is I2S it can only fail the firmware's in-use check.
    #[test]
    fn mck_drops_gpio_fifteen_while_a_slot_is_i2s() {
        let plain = map_of(Device::new().transport(), Platform::Rp2350);
        assert_eq!(plain.mck_candidates(Platform::Rp2350), vec![13, 15, 21]);

        let i2s = map_of(
            Device::new().with_i2s_slot_type(0, true).transport(),
            Platform::Rp2350,
        );
        assert_eq!(i2s.mck_candidates(Platform::Rp2350), vec![13, 21]);
        // An RP2040 has one candidate either way.
        assert_eq!(i2s.mck_candidates(Platform::Rp2040), vec![21]);
    }
}
