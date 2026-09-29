//! The caps-driven model behind Control Surfaces, Channel Groups and Macros.
//!
//! Nothing on the three Control pages is hardcoded. The device reports which
//! component types exist, which parameters each may drive, what units and
//! ranges those take, and which are unavailable on this platform; this module
//! turns those tables into the option lists the pages offer and the drafts they
//! encode.
//!
//! Three things live here and nowhere else:
//!
//! - **[`CsData`]**, everything the three pages read, taken in one pass when
//!   Settings opens. `DeviceState` carries none of it: bindings, names, IR
//!   commands, groups, macros, display config and pages are all their own
//!   opcodes, and a control-surface write pushes no notification, so a re-read
//!   is the only confirmation there is (survey-firmware 3.14).
//! - **The name tables**, copied verbatim from the Console's `typeName`,
//!   `nounName`, `actionName`, `pinDetail`, `invertTitle` and `invertDetail`
//!   (`DSPi_ConsoleApp.swift:6185-6620`). `packets.rs` has `CS_NOUNS` and its
//!   friends as command-line identifiers, which are not display names.
//! - **The link laws**, the small set of rules that keep a draft encodable:
//!   which actions a (type, noun) pair allows, which flags survive a noun or
//!   action change, which bands a group can address, and what a freshly added
//!   control starts as.
//!
//! The units are the fiddliest part. `dB`, `Q`, `percent` and `ms` are signed
//! 8.8 fixed point (1.0 = 256); `Hz` is a plain integer; and `Hz` and `Q` step
//! multiplicatively, so their step operand is in octaves rather than in the
//! unit itself (survey-firmware 3.2).

use std::collections::BTreeSet;

use dspi_proto::packets::CsTypeDesc;
use dspi_session::probe::ControlSurfaceCaps;
use dspi_session::surfaces::explain_status;
use dspi_session::surfaces::{
    CsBinding, CsDisplayCfg, CsDisplayPage, CsDisplayStatus, CsExtStatusPacket, CsGroup, CsMacro,
    CsMacroStep, CsNounDesc, CsStatusPacket, GPIO_UNUSED, IrCommand, Limits, Surfaces,
};
use dspi_session::{DeviceState, Session};

use super::Cx;
use crate::shell::SessionReply;

// ---------------------------------------------------------------------------
// The wire vocabulary, by name
// ---------------------------------------------------------------------------

/// `CsType` (control_surfaces.h:104-117).
pub mod ty {
    pub const NONE: u8 = 0;
    pub const BUTTON: u8 = 1;
    pub const SWITCH: u8 = 2;
    pub const POT: u8 = 3;
    pub const ENCODER: u8 = 4;
    pub const LED: u8 = 5;
    pub const LED_PWM: u8 = 6;
    pub const IR: u8 = 7;
    pub const DISPLAY: u8 = 8;
}

/// `CsAction` (control_surfaces.h:227-241).
pub mod act {
    pub const ADJUST: u8 = 0;
    pub const STEP: u8 = 1;
    pub const INC: u8 = 2;
    pub const DEC: u8 = 3;
    pub const TOGGLE: u8 = 4;
    pub const SET: u8 = 5;
    pub const FOLLOW: u8 = 6;
    pub const TRIGGER: u8 = 7;
    pub const IND_EQUALS: u8 = 8;
    pub const MOMENTARY: u8 = 9;
    pub const IND_ABOVE: u8 = 10;
    pub const IND_LEVEL: u8 = 11;
}

/// The nouns this module names by hand, because a rule keys off them.
pub mod noun {
    pub const USER_VOLUME: u8 = 0;
    pub const MASTER_VOLUME: u8 = 1;
    pub const USER_MUTE: u8 = 2;
    pub const LOUDNESS: u8 = 3;
    pub const CROSSFEED: u8 = 4;
    pub const LEVELLER: u8 = 5;
    pub const PRESET: u8 = 6;
    pub const INPUT_SOURCE: u8 = 7;
    pub const CLIP: u8 = 8;
    pub const EQ_BYPASS: u8 = 9;
    pub const LG_SYNC: u8 = 10;
    pub const CROSSFEED_PRESET: u8 = 11;
    pub const CROSSFEED_ITD: u8 = 12;
    pub const LEVELLER_AMOUNT: u8 = 13;
    pub const LEVELLER_SPEED: u8 = 14;
    pub const LEVELLER_LOOKAHEAD: u8 = 15;
    pub const PREAMP: u8 = 16;
    pub const OUTPUT_GAIN: u8 = 17;
    pub const OUTPUT_MUTE: u8 = 18;
    pub const OUTPUT_ENABLE: u8 = 19;
    pub const FILTER_FREQ: u8 = 20;
    pub const FILTER_GAIN: u8 = 21;
    pub const FILTER_Q: u8 = 22;
    pub const FILTER_TYPE: u8 = 23;
    pub const FILTER_BYPASS: u8 = 24;
    pub const SIGGEN: u8 = 25;
    pub const DAC_MUTE_TEST: u8 = 26;
    pub const CLIP_CH: u8 = 27;
    pub const LEVEL: u8 = 28;
    pub const SPDIF_LOCK: u8 = 29;
    pub const SAMPLE_RATE: u8 = 30;
    pub const USB_STREAMING: u8 = 31;
    pub const ADAT_ACTIVE: u8 = 32;
    pub const LG_PRESENT: u8 = 33;
    pub const LG_MUTED: u8 = 34;
    pub const UPMIX: u8 = 35;
    pub const UPMIX_CENTER_MODE: u8 = 36;
    pub const UPMIX_SURROUND_MODE: u8 = 37;
    pub const UPMIX_STRENGTH: u8 = 38;
    pub const UPMIX_WIDTH: u8 = 39;
    pub const UPMIX_PRESENCE: u8 = 40;
    pub const PSYBASS: u8 = 41;
    pub const PSYBASS_CUTOFF: u8 = 42;
    pub const PSYBASS_HARMONICS: u8 = 43;
    pub const PSYBASS_DRIVE: u8 = 44;
    pub const PSYBASS_CHARACTER: u8 = 45;
    pub const PSYBASS_ORIGINAL: u8 = 46;
    pub const OUTPUT_DELAY: u8 = 47;
    pub const PRESET_RELOAD: u8 = 48;
    pub const LOUDNESS_SPL: u8 = 49;
    pub const LOUDNESS_INTENSITY: u8 = 50;
    pub const INPUT_LEVEL_MAX: u8 = 51;
    pub const MACRO: u8 = 52;
    pub const CPU_LOAD: u8 = 53;
    pub const DISPLAY_PAGE: u8 = 54;
    pub const DISPLAY_EDIT: u8 = 55;
    pub const PAGE_VALUE: u8 = 56;
    // Caps v14 to v20 (control_surfaces.h:223-250). Named here so the picker
    // shows the Console's words; their categories and rules are phase B7.
    pub const SUBHARM: u8 = 57;
    pub const SUBHARM_LOW: u8 = 58;
    pub const SUBHARM_HIGH: u8 = 59;
    pub const SUBHARM_BOOST: u8 = 60;
    pub const SUBHARM_TOP: u8 = 61;
    pub const SUBHARM_SELECT: u8 = 62;
    pub const SUBHARM_DEPTH: u8 = 63;
    pub const SUBHARM_HOLD: u8 = 64;
    pub const SUBHARM_CEILING: u8 = 65;
    pub const SUBHARM_LINK: u8 = 66;
    pub const SUBHARM_SOLO: u8 = 67;
    pub const AUX: u8 = 68;
    pub const AUX_LEVEL: u8 = 69;
    pub const TUBE: u8 = 70;
    pub const TUBE_DRIVE: u8 = 71;
    pub const TUBE_TYPE: u8 = 72;
    pub const TUBE_MIX: u8 = 73;
    pub const LIMITER: u8 = 74;
    pub const LIMITER_THRESHOLD: u8 = 75;
    pub const LIMITER_RELEASE: u8 = 76;
    pub const LIMITER_LINK: u8 = 77;
    pub const LIMITER_GR: u8 = 78;
}

/// `CsBinding.flags` (control_surfaces.h:255-266). The byte is full.
pub mod flag {
    pub const INVERT: u8 = 0x01;
    pub const REVERSE: u8 = 0x02;
    pub const WRAP: u8 = 0x04;
    pub const ACCEL: u8 = 0x08;
    pub const REPEAT: u8 = 0x10;
    pub const GROUP: u8 = 0x20;
    pub const LINK_ABS: u8 = 0x40;
    pub const GROUP_ALL: u8 = 0x80;
}

/// `CsNounDesc.kind`.
pub mod kind {
    pub const CONTINUOUS: u8 = 0;
    pub const BOOL: u8 = 1;
    pub const ENUM: u8 = 2;
    /// Not a firmware value: `PAGE_VALUE` resolves its item at event time, so
    /// it carries no operands at all and reads as none of the three.
    pub const NONE: u8 = 255;
}

/// `CsNounDesc.unit`, from the protocol crate (control_surfaces.h:259-269).
pub mod unit {
    pub use dspi_proto::packets::cs_unit::*;
}

/// `CsNounDesc.target_kind`.
pub mod target {
    pub const NONE: u8 = 0;
    pub const INPUT_CH: u8 = 1;
    pub const OUTPUT_CH: u8 = 2;
    pub const DSP_CH: u8 = 3;
    pub const DSP_BAND: u8 = 4;
}

/// Highest per-LED brightness ceiling, as a percentage of full duty
/// (`CS_LED_BRIGHT_MAX`, caps v12). 0 on the wire means unset, which is full.
pub const LED_BRIGHT_MAX: u8 = 100;

/// The dwell floor the firmware enforces in either cycle mode, 0.1 s units.
pub const DISPLAY_MIN_DWELL: u16 = 10;

/// Longest indicator delay the u16 0.1 s field can express, in whole seconds.
pub const DELAY_MAX_SECONDS: u32 = u16::MAX as u32 / 10;

/// Longest macro step delay, in whole seconds; the field is 10 ms units.
pub const STEP_DELAY_MAX_SECONDS: f64 = u16::MAX as f64 / 100.0;

/// Actions a macro step may carry: the button repertoire minus MOMENTARY and
/// the indicators (`CS_MACRO_STEP_ACTIONS`).
pub const MACRO_STEP_ACTIONS: [u8; 5] = [act::SET, act::TOGGLE, act::INC, act::DEC, act::TRIGGER];

/// Button events, in wire order.
pub const EVENT_NAMES: [&str; 3] = ["Press", "Long press", "Double press"];

/// The three line alignments (`CS_DALIGN_*`); 3 is reserved.
pub const ALIGN_NAMES: [&str; 3] = ["Left", "Centre", "Right"];

/// The addresses the Console offers beside "Default".
pub const DISPLAY_ADDRESSES: [i16; 5] = [0x27, 0x3C, 0x3D, 0x3E, 0x3F];

/// True when a type only reports state rather than changing it.
pub fn is_indicator(t: u8) -> bool {
    t == ty::LED || t == ty::LED_PWM
}

/// `CS_ACT_BIT`: the caps masks are `1 << action`.
pub fn act_bit(action: u8) -> u16 {
    1u16 << action
}

// ---------------------------------------------------------------------------
// Display names, the Console's
// ---------------------------------------------------------------------------

/// The Console's `typeName` (`DSPi_ConsoleApp.swift:6185`).
pub fn type_name(t: u8) -> String {
    match t {
        ty::NONE => "None".into(),
        ty::BUTTON => "Push Button".into(),
        ty::SWITCH => "Toggle Switch".into(),
        ty::POT => "Potentiometer / Fader".into(),
        ty::ENCODER => "Rotary Encoder".into(),
        ty::LED => "Indicator LED".into(),
        ty::LED_PWM => "Dimmable LED".into(),
        ty::IR => "IR Remote".into(),
        ty::DISPLAY => "Display".into(),
        other => format!("Type {other}"),
    }
}

/// The Console's `nounName` (`:6253`). Three nouns read differently on an
/// indicator, where the LED shows the state and a button does the clearing.
pub fn noun_name(n: u8, for_type: u8) -> String {
    if !is_indicator(for_type) {
        match n {
            noun::CLIP => return "Clear Clipping".into(),
            noun::DAC_MUTE_TEST => return "Test DAC Mute".into(),
            noun::PRESET_RELOAD => return "Reload Preset".into(),
            _ => {}
        }
    }
    match n {
        noun::USER_VOLUME => "Volume".into(),
        noun::MASTER_VOLUME => "Master Volume".into(),
        noun::USER_MUTE => "Mute".into(),
        noun::LOUDNESS => "Loudness".into(),
        noun::CROSSFEED => "Crossfeed".into(),
        noun::LEVELLER => "Volume Leveller".into(),
        noun::PRESET => "Preset".into(),
        noun::INPUT_SOURCE => "Input Source".into(),
        noun::CLIP => "Clipping".into(),
        noun::EQ_BYPASS => "EQ Bypass".into(),
        noun::LG_SYNC => "LG Sound Sync".into(),
        noun::CROSSFEED_PRESET => "Crossfeed Preset".into(),
        noun::CROSSFEED_ITD => "Crossfeed ITD".into(),
        noun::LEVELLER_AMOUNT => "Leveller Amount".into(),
        noun::LEVELLER_SPEED => "Leveller Speed".into(),
        noun::LEVELLER_LOOKAHEAD => "Leveller Lookahead".into(),
        noun::PREAMP => "Input Preamp".into(),
        noun::OUTPUT_GAIN => "Output Gain".into(),
        noun::OUTPUT_MUTE => "Output Mute".into(),
        noun::OUTPUT_ENABLE => "Output Enable".into(),
        noun::FILTER_FREQ => "Filter Frequency".into(),
        noun::FILTER_GAIN => "Filter Gain".into(),
        noun::FILTER_Q => "Filter Q".into(),
        noun::FILTER_TYPE => "Filter Type".into(),
        noun::FILTER_BYPASS => "Filter Bypass".into(),
        noun::SIGGEN => "Signal Generator".into(),
        noun::DAC_MUTE_TEST => "DAC Mute Test".into(),
        noun::CLIP_CH => "Channel Clipping".into(),
        noun::LEVEL => "Channel Level".into(),
        noun::SPDIF_LOCK => "S/PDIF Lock".into(),
        noun::SAMPLE_RATE => "Sample Rate".into(),
        noun::USB_STREAMING => "USB Streaming".into(),
        noun::ADAT_ACTIVE => "ADAT Active".into(),
        noun::LG_PRESENT => "LG Source Present".into(),
        noun::LG_MUTED => "LG Muted".into(),
        noun::UPMIX => "Upmixer".into(),
        noun::UPMIX_CENTER_MODE => "Upmixer Centre Mode".into(),
        noun::UPMIX_SURROUND_MODE => "Upmixer Surround Mode".into(),
        noun::UPMIX_STRENGTH => "Upmixer Strength".into(),
        noun::UPMIX_WIDTH => "Upmixer Width".into(),
        noun::UPMIX_PRESENCE => "Upmixer Presence".into(),
        noun::PSYBASS => "Psychoacoustic Bass".into(),
        noun::PSYBASS_CUTOFF => "Psych Bass Cutoff Frequency".into(),
        noun::PSYBASS_HARMONICS => "Psych Bass Harmonics".into(),
        noun::PSYBASS_DRIVE => "Psych Bass Drive".into(),
        noun::PSYBASS_CHARACTER => "Psych Bass Character".into(),
        noun::PSYBASS_ORIGINAL => "Psych Bass Original Level".into(),
        noun::OUTPUT_DELAY => "Output Delay".into(),
        noun::PRESET_RELOAD => "Preset Reload".into(),
        noun::LOUDNESS_SPL => "Loudness Reference SPL".into(),
        noun::LOUDNESS_INTENSITY => "Loudness Intensity".into(),
        noun::INPUT_LEVEL_MAX => "Input Signal Level".into(),
        noun::MACRO => {
            if is_indicator(for_type) {
                "Running Macro".into()
            } else {
                "Macro".into()
            }
        }
        noun::CPU_LOAD => "CPU Load".into(),
        noun::DISPLAY_PAGE => "Show Page".into(),
        noun::DISPLAY_EDIT => "Allow Editing".into(),
        noun::PAGE_VALUE => "Browse/Adjust".into(),
        noun::SUBHARM => "Subharmonic Synthesizer".into(),
        noun::SUBHARM_LOW => "Subharm 24-36 Hz Level".into(),
        noun::SUBHARM_HIGH => "Subharm 36-56 Hz Level".into(),
        noun::SUBHARM_TOP => "Subharm 56-80 Hz Level".into(),
        noun::SUBHARM_BOOST => "Subharm LF Boost".into(),
        noun::SUBHARM_SELECT => "Subharm Selectivity".into(),
        noun::SUBHARM_DEPTH => "Subharm Selectivity Depth".into(),
        noun::SUBHARM_HOLD => "Subharm Selectivity Hold".into(),
        noun::SUBHARM_CEILING => "Subharm Sub Ceiling".into(),
        noun::SUBHARM_LINK => "Subharm Pair Link".into(),
        noun::SUBHARM_SOLO => "Subharm Solo".into(),
        noun::AUX => "Aux Switch".into(),
        noun::AUX_LEVEL => "Aux Level".into(),
        noun::TUBE => "Tube Modeller".into(),
        noun::TUBE_DRIVE => "Tube Drive".into(),
        noun::TUBE_TYPE => "Tube Type".into(),
        noun::TUBE_MIX => "Tube Mix".into(),
        // The Console has no limiter nouns yet; these follow its pattern
        // (PLAN-beta4 decision 6).
        noun::LIMITER => "Limiter".into(),
        noun::LIMITER_THRESHOLD => "Limiter Threshold".into(),
        noun::LIMITER_RELEASE => "Limiter Release".into(),
        noun::LIMITER_LINK => "Limiter Link".into(),
        noun::LIMITER_GR => "Limiter Gain Reduction".into(),
        other => format!("Parameter {other}"),
    }
}

/// The Console's `actionName` (`:6323`). `Browse/Adjust` names a direction
/// rather than a list position, because unarmed it moves a page.
pub fn action_name(action: u8, n: u8, is_enum: bool) -> String {
    if n == noun::PAGE_VALUE {
        if action == act::INC {
            return "Up".into();
        }
        if action == act::DEC {
            return "Down".into();
        }
    }
    match action {
        act::ADJUST => "Adjust".into(),
        act::STEP => "Step".into(),
        act::INC => if is_enum { "Next" } else { "Increase" }.into(),
        act::DEC => if is_enum { "Previous" } else { "Decrease" }.into(),
        act::TOGGLE => "Toggle".into(),
        act::SET => "Set value".into(),
        act::FOLLOW => "Follow position".into(),
        act::TRIGGER => "Trigger".into(),
        act::IND_EQUALS => "Indicate".into(),
        act::MOMENTARY => "Hold".into(),
        act::IND_ABOVE => "Indicate above".into(),
        act::IND_LEVEL => "Show level".into(),
        other => format!("Action {other}"),
    }
}

/// The Console's `pinDetail` (`:6586`).
pub fn pin_detail(t: u8) -> &'static str {
    match t {
        ty::POT => "ADC pin (GPIO 26, 27, or 28), wiper to the pin.",
        ty::LED | ty::LED_PWM => "Output pin driving the LED.",
        ty::IR => "GPIO wired to the receiver module's OUT (VCC to 3V3, GND to GND).",
        _ => "Wired between this GPIO and GND.",
    }
}

/// The Console's `invertTitle` (`:6595`).
pub fn invert_title(t: u8) -> &'static str {
    match t {
        ty::LED | ty::LED_PWM => "Active-Low LED",
        ty::POT | ty::ENCODER => "Pull-Down Wiring",
        ty::IR => "Idle-Low Receiver",
        _ => "Active-High Wiring",
    }
}

/// The Console's `invertDetail` (`:6604`).
pub fn invert_detail(t: u8) -> &'static str {
    match t {
        ty::LED => "Drive the pin low to light the LED (LED wired to 3V3 through a resistor).",
        ty::LED_PWM => "Invert the PWM duty for an LED wired to 3V3 through a resistor.",
        ty::POT | ty::ENCODER => {
            "Wire the common terminal to 3V3 instead of GND (internal pull-down)."
        }
        ty::IR => {
            "The receiver idles low and pulls high on a mark; default is the usual idle-high, \
             active-low module."
        }
        _ => "Component wired to 3V3 with the internal pull-down; default is to GND with pull-up.",
    }
}

/// The Console's `boolLabel`: the two states a boolean noun reads as.
pub fn bool_label(n: u8, on: bool) -> &'static str {
    match n {
        noun::CLIP | noun::CLIP_CH => {
            if on {
                "Clipping"
            } else {
                "Not clipping"
            }
        }
        noun::USER_MUTE | noun::OUTPUT_MUTE | noun::LG_MUTED => {
            if on {
                "Muted"
            } else {
                "Unmuted"
            }
        }
        noun::SPDIF_LOCK => {
            if on {
                "Locked"
            } else {
                "Unlocked"
            }
        }
        noun::USB_STREAMING | noun::ADAT_ACTIVE | noun::SIGGEN => {
            if on {
                "Active"
            } else {
                "Idle"
            }
        }
        noun::LG_PRESENT => {
            if on {
                "Present"
            } else {
                "Absent"
            }
        }
        _ => {
            if on {
                "On"
            } else {
                "Off"
            }
        }
    }
}

/// The Console's `csDisplayModelName` (`Constants.swift:754`).
pub fn display_model_name(model: u8) -> String {
    match model {
        1 => "LCD 16x2 (HD44780)".into(),
        2 => "LCD 20x4 (HD44780)".into(),
        3 => "Character OLED 16x2".into(),
        4 => "Character OLED 20x2".into(),
        5 => "Character OLED 20x4".into(),
        6 => "OLED 128x64 (SSD1306)".into(),
        7 => "OLED 128x32 (SSD1306)".into(),
        8 => "OLED 128x64 (SH1106)".into(),
        other => format!("Model {other}"),
    }
}

/// The address a model answers on when the binding stores 0.
pub fn display_default_address(model: u8) -> u8 {
    if model == 1 || model == 2 { 0x27 } else { 0x3C }
}

/// The graphic OLEDs, the only models with a large font to render.
pub fn display_is_graphic(model: u8) -> bool {
    model >= 6
}

/// The Console's `csIrProtocolName`.
pub fn ir_protocol_name(p: u8) -> &'static str {
    match p {
        1 => "NEC",
        2 => "RC5",
        3 => "RC6",
        4 => "Generic",
        _ => "None",
    }
}

/// The Console's `bandName`: PEQ bands 0-9, crossover bands 20-23.
pub fn band_name(band: u8) -> String {
    if (20..=23).contains(&band) {
        format!("Crossover {}", band - 19)
    } else {
        format!("Band {}", band as u16 + 1)
    }
}

// ---------------------------------------------------------------------------
// Units
// ---------------------------------------------------------------------------

/// True when a unit encodes value and range as signed 8.8 fixed point.
pub fn unit_is_fixed_point(u: u8) -> bool {
    dspi_proto::packets::cs_unit_is_fixed_point(u)
}

/// True when a unit steps multiplicatively, so its step operand is in octaves.
/// `MS_LOG` (caps v20) is one: plain integer ms stepped in octaves.
pub fn unit_is_log(u: u8) -> bool {
    dspi_proto::packets::cs_unit_is_log(u)
}

pub fn unit_symbol(u: u8) -> &'static str {
    match u {
        unit::DB => "dB",
        unit::HZ => "Hz",
        unit::Q => "Q",
        unit::PERCENT => "%",
        unit::MS | unit::MS_LOG => "ms",
        _ => "",
    }
}

/// Decimals a unit's field shows: Hz and percent whole, Q and ms fine.
pub fn unit_decimals(u: u8) -> usize {
    match u {
        unit::HZ | unit::PERCENT | unit::MS_LOG => 0,
        unit::Q | unit::MS => 2,
        _ => 1,
    }
}

/// One nudge of a unit's field (the Console's `unitScrollStep`).
pub fn unit_scroll_step(u: u8) -> f64 {
    match u {
        unit::HZ | unit::MS_LOG => 10.0,
        unit::Q => 0.1,
        unit::PERCENT => 1.0,
        unit::MS => 0.1,
        _ => 0.5,
    }
}

/// The smallest step a step field may carry; 8.8 resolves 1/256.
pub fn unit_min_step(u: u8) -> f64 {
    if u == unit::MS { 0.01 } else { 0.1 }
}

/// The firmware's own step when `CsBinding.step` is 0: one unit for the linear
/// units, 1/12 octave for the log ones, 0.1 ms for delay.
pub fn default_step(u: u8) -> f64 {
    match u {
        unit::HZ | unit::Q | unit::MS_LOG => 1.0 / 12.0,
        unit::MS => 0.1,
        _ => 1.0,
    }
}

fn clamp_i16(v: f64) -> i16 {
    v.round().clamp(i16::MIN as f64, i16::MAX as f64) as i16
}

pub fn decode_value(q: i16, u: u8) -> f64 {
    if unit_is_fixed_point(u) {
        q as f64 / 256.0
    } else {
        q as f64
    }
}

pub fn encode_value(v: f64, u: u8) -> i16 {
    clamp_i16(if unit_is_fixed_point(u) { v * 256.0 } else { v })
}

/// A step operand: continuous units carry it as 8.8 (dB, octaves, percent);
/// the plain unit carries a position count.
pub fn decode_step(q: i16, u: u8) -> f64 {
    if u == unit::NONE {
        q as f64
    } else {
        q as f64 / 256.0
    }
}

pub fn encode_step(v: f64, u: u8) -> i16 {
    clamp_i16(if u == unit::NONE { v } else { v * 256.0 })
}

/// A value with its unit, the Console's `fmtUnit`.
pub fn fmt_unit(v: f64, u: u8) -> String {
    match u {
        unit::HZ => format!("{v:.0} Hz"),
        unit::Q => format!("Q {v:.2}"),
        unit::PERCENT => format!("{v:.0} %"),
        unit::DB => format!("{v:.1} dB"),
        unit::MS => format!("{v:.2} ms"),
        unit::MS_LOG => format!("{v:.0} ms"),
        _ => format!("{v:.0}"),
    }
}

/// Indicator delays are 0.1 s units; macro step delays are 10 ms units, ten
/// times finer (firmware-notes 24). Mixing them up is a factor of ten with
/// nothing to notice it by, so each has its own pair.
pub fn encode_delay(seconds: u32) -> u16 {
    (seconds.min(DELAY_MAX_SECONDS) * 10) as u16
}

pub fn decode_delay(raw: u16) -> u32 {
    raw as u32 / 10
}

pub fn encode_step_delay(seconds: f64) -> u16 {
    let units = (seconds * 100.0).round();
    // NaN is not greater than zero and not less than it either, so the test is
    // written to reject it rather than to fall through to the cast.
    if units.is_nan() || units <= 0.0 {
        return 0;
    }
    units.min(u16::MAX as f64) as u16
}

pub fn decode_step_delay(raw: u16) -> f64 {
    raw as f64 / 100.0
}

/// A delay in the words its fields use: whole minutes and seconds.
pub fn fmt_delay(raw: u16) -> String {
    let total = decode_delay(raw);
    let (m, s) = (total / 60, total % 60);
    if m == 0 {
        format!("{s} s")
    } else if s == 0 {
        format!("{m} min")
    } else {
        format!("{m} min {s} s")
    }
}

/// Seconds in the most readable unit, for the macro summaries.
pub fn fmt_delay_seconds(s: f64) -> String {
    if s < 60.0 {
        return format!("{s:.2} s");
    }
    let mins = s / 60.0;
    if (mins - mins.round()).abs() < f64::EPSILON {
        format!("{mins:.0} min")
    } else {
        format!("{mins:.1} min")
    }
}

// ---------------------------------------------------------------------------
// What the pages read
// ---------------------------------------------------------------------------

/// The capability header, as this crate needs it.
///
/// A copy rather than [`ControlSurfaceCaps`] itself, because `SettingsData`
/// compares and defaults its fields and the probe's struct does neither. The
/// two are kept in step by [`From`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CsCaps {
    pub caps_version: u8,
    pub max_bindings: u8,
    pub type_count: u8,
    pub noun_count: u8,
    pub max_ir_commands: u8,
    pub max_groups: u8,
    pub max_macros: u8,
    pub max_macro_steps: u8,
    pub max_pages: u8,
    pub display_models: u8,
    pub types: Vec<CsTypeDesc>,
}

impl From<&ControlSurfaceCaps> for CsCaps {
    fn from(c: &ControlSurfaceCaps) -> Self {
        Self {
            caps_version: c.caps_version,
            max_bindings: c.max_bindings,
            type_count: c.type_count,
            noun_count: c.noun_count,
            max_ir_commands: c.max_ir_commands,
            max_groups: c.max_groups,
            max_macros: c.max_macros,
            max_macro_steps: c.max_macro_steps,
            max_pages: c.max_pages,
            display_models: c.display_models,
            types: c.types.clone(),
        }
    }
}

/// Every control-surface record the three Control pages show.
///
/// Read once when Settings opens. Sixteen bindings, sixteen names, sixteen IR
/// commands, eight groups, eight macros, the display config and its pages, plus
/// the three status packets: about seventy round trips, which is why it happens
/// on open rather than per frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CsData {
    pub caps: CsCaps,
    pub nouns: Vec<CsNounDesc>,
    pub bindings: Vec<CsBinding>,
    pub names: Vec<String>,
    pub ir: Vec<IrCommand>,
    pub groups: Vec<CsGroup>,
    pub macros: Vec<CsMacro>,
    pub display_cfg: CsDisplayCfg,
    /// From the display config reply; published nowhere else.
    pub max_pages: u8,
    pub model_count: u8,
    pub pages: Vec<CsDisplayPage>,
    pub status: CsStatusPacket,
    pub ext: CsExtStatusPacket,
    pub display_status: CsDisplayStatus,
}

impl CsData {
    /// Read the lot, tolerating a firmware that lacks any part of it.
    ///
    /// A stall is not an error here: caps v9 added groups and macros and v10
    /// the display, so an older device answers nothing for those and the pages
    /// simply have none to show.
    pub fn read(session: &mut Session) -> Option<Self> {
        let probed = session.capabilities().cs.clone()?;
        let limits = Limits::from(&probed);
        let caps = CsCaps::from(&probed);
        session
            .with_transport(|t| {
                let mut s = Surfaces::new(t, limits);
                let nouns = (0..caps.noun_count)
                    .map(|n| s.read_noun(n).unwrap_or_default())
                    .collect();
                let bindings = (0..caps.max_bindings)
                    .map(|slot| s.read_binding(slot).unwrap_or_default())
                    .collect();
                let names = (0..caps.max_bindings)
                    .map(|slot| s.read_name(slot).unwrap_or_default())
                    .collect();
                let ir = (0..caps.max_ir_commands)
                    .map(|sub| s.read_ir_command(sub).unwrap_or_default())
                    .collect();
                let groups = (0..caps.max_groups)
                    .map(|g| s.read_group(g).unwrap_or_default())
                    .collect();
                let macros = (0..caps.max_macros)
                    .map(|m| s.read_macro(m).unwrap_or_default())
                    .collect();
                let reply = s.read_display_cfg().ok();
                let max_pages = reply.as_ref().map(|r| r.max_pages).unwrap_or(0);
                let model_count = reply.as_ref().map(|r| r.model_count).unwrap_or(0);
                let display_cfg = reply.map(|r| r.cfg).unwrap_or_default();
                let pages = (0..max_pages)
                    .map(|p| s.read_display_page(p).unwrap_or_default())
                    .collect();
                Ok(CsData {
                    nouns,
                    bindings,
                    names,
                    ir,
                    groups,
                    macros,
                    display_cfg,
                    max_pages,
                    model_count,
                    pages,
                    status: s.read_status().unwrap_or_default(),
                    ext: s.read_ext_status().unwrap_or_default(),
                    display_status: s.read_display_status().unwrap_or_default(),
                    caps,
                })
            })
            .ok()
    }

    pub fn slot_count(&self) -> usize {
        self.bindings.len()
    }

    pub fn noun_desc(&self, n: u8) -> Option<&CsNounDesc> {
        self.nouns.get(n as usize)
    }

    pub fn type_desc(&self, t: u8) -> Option<&dspi_proto::packets::CsTypeDesc> {
        self.caps.types.get(t as usize)
    }

    pub fn binding(&self, slot: usize) -> CsBinding {
        self.bindings.get(slot).cloned().unwrap_or_default()
    }

    pub fn name(&self, slot: usize) -> String {
        self.names.get(slot).cloned().unwrap_or_default()
    }

    pub fn group_name(&self, g: usize) -> String {
        match self.groups.get(g) {
            Some(x) if !x.name.is_empty() => x.name.clone(),
            _ => format!("Group {}", g + 1),
        }
    }

    pub fn macro_name(&self, m: usize) -> String {
        match self.macros.get(m) {
            Some(x) if !x.name.is_empty() => x.name.clone(),
            _ => format!("Macro {}", m + 1),
        }
    }

    /// Configurable component types (every type but NONE), from the caps table.
    pub fn real_types(&self) -> Vec<u8> {
        if self.caps.type_count <= 1 {
            return Vec::new();
        }
        (1..self.caps.type_count).collect()
    }

    /// How many channels a group kind addresses, taken from a noun that
    /// targets that space so the count stays device-served.
    pub fn group_channel_count(&self, kind: u8) -> u8 {
        for nd in self.nouns.iter().filter(|n| n.is_available()) {
            if nd.target_count == 0 {
                continue;
            }
            if nd.target_kind == kind {
                return nd.target_count;
            }
            if kind == target::DSP_CH && nd.target_kind == target::DSP_BAND {
                return nd.target_count;
            }
        }
        0
    }
}

// ---------------------------------------------------------------------------
// Running a write against the device
// ---------------------------------------------------------------------------

/// Run one control-surface write and say what the device made of it.
///
/// Every control-surface SET is a deferred, live-only preview: the transfer
/// returns before the main loop has validated it, so the outcome only exists
/// once `REQ_GET_CS_STATUS` reports it under this write's own slot tag.
/// [`Surfaces::write_binding`] and its siblings wait that out, absorbing
/// `PENDING` and retrying `BUSY`, so what arrives here is the settled answer.
///
/// The closure may make several writes; returning the first that did not
/// succeed reports that reason rather than the last write's.
pub(crate) fn run(
    session: &mut Session,
    f: impl FnOnce(&mut Surfaces<'_>) -> dspi_session::surfaces::Result<CsStatusPacket>,
) -> SessionReply {
    let limits = session
        .capabilities()
        .cs
        .as_ref()
        .map(Limits::from)
        .unwrap_or_default();
    let out = session.with_transport(|t| {
        let mut s = Surfaces::new(t, limits);
        Ok(f(&mut s))
    });
    match out {
        Ok(Ok(st)) if st.last_status == dspi_session::surfaces::status::SUCCESS => {
            SessionReply::Ok("Applied".into())
        }
        Ok(Ok(st)) => SessionReply::Err(capitalise(&explain_status(st.last_status))),
        Ok(Err(e)) => SessionReply::Err(e.to_string()),
        Err(e) => SessionReply::Err(e.to_string()),
    }
}

/// The same again, for a write that answers a bare status code rather than a
/// status packet: firing a macro is immediate, not deferred.
pub(crate) fn run_code(
    session: &mut Session,
    ok: &str,
    f: impl FnOnce(&mut Surfaces<'_>) -> dspi_session::surfaces::Result<u8>,
) -> SessionReply {
    let limits = session
        .capabilities()
        .cs
        .as_ref()
        .map(Limits::from)
        .unwrap_or_default();
    let out = session.with_transport(|t| {
        let mut s = Surfaces::new(t, limits);
        Ok(f(&mut s))
    });
    match out {
        Ok(Ok(code)) if code == dspi_session::surfaces::status::SUCCESS => {
            SessionReply::Ok(ok.into())
        }
        Ok(Ok(code)) => SessionReply::Err(capitalise(&explain_status(code))),
        Ok(Err(e)) => SessionReply::Err(e.to_string()),
        Err(e) => SessionReply::Err(e.to_string()),
    }
}

/// The Console's placeholder while the capability tables are missing: either
/// the device is still being read, or there is no device to read. Every picker
/// on the three Control pages is built from device-served tables, so without
/// them a page can only explain itself.
pub const AWAITING: &str = "Reading control-surface capabilities from the device...";
pub const DISCONNECTED_TITLE: &str = "No Device Connected";
pub const DISCONNECTED_BODY: &str = "Control surfaces are stored on the device. Connect a DSPi to \
                                     view and configure its wired controls.";

pub(crate) fn placeholder_rows(cx: &Cx<'_>) -> Vec<super::Row> {
    if cx.connected {
        vec![super::Row::note(AWAITING)]
    } else {
        vec![empty_state(DISCONNECTED_TITLE, DISCONNECTED_BODY)]
    }
}

/// The Console's centred empty state, as a terminal can draw one: a title and
/// its explanation, in the kit's info banner rather than a section header,
/// which is uppercased and would shout it.
pub(crate) fn empty_state(title: &str, body: &str) -> super::Row {
    super::Row::Banner(
        crate::widgets::BannerKind::Info,
        title.to_string(),
        body.to_string(),
    )
}

/// `explain_status` answers a lowercase clause, which reads well after "Not
/// running:" and badly as a sentence of its own.
pub fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Why a stored-but-enabled binding is not running, from its slot health code.
pub fn inactive_reason(code: u8) -> String {
    format!(
        "Not running: {}. Reassign the conflicting pin, then apply.",
        explain_status(code)
    )
}

// ---------------------------------------------------------------------------
// Names that need the rest of the device
// ---------------------------------------------------------------------------

/// The Console's input-source list, shared with the sidebar so a firmware that
/// grows the enum still reads correctly.
pub const INPUT_SOURCES: [&str; 7] = [
    "USB", "S/PDIF", "I2S", "ADAT", "S/PDIF 2", "S/PDIF 3", "S/PDIF 4",
];

/// The Console's `targetName`: a channel address in the noun's own space.
pub fn target_name(state: &DeviceState, nd: &CsNounDesc, t: u8) -> String {
    let ch = match nd.target_kind {
        target::OUTPUT_CH => t as usize + state.caps.num_inputs as usize,
        _ => t as usize,
    };
    let name = state.channel_name(ch);
    if name.is_empty() {
        format!("Channel {}", ch + 1)
    } else {
        name
    }
}

/// The Console's `enumValueLabel`: what one position of an enum noun reads as.
pub(crate) fn enum_value_label(cx: &Cx<'_>, cs: &CsData, n: u8, value: i32) -> String {
    match n {
        noun::MACRO => cs.macro_name(value.max(0) as usize),
        noun::PRESET => {
            let name = cx
                .data
                .preset_names
                .get(value.max(0) as usize)
                .cloned()
                .unwrap_or_default();
            if name.is_empty() {
                format!("Preset {}", value + 1)
            } else {
                format!("Preset {} - {name}", value + 1)
            }
        }
        noun::INPUT_SOURCE => INPUT_SOURCES
            .get(value.max(0) as usize)
            .map(|s| (*s).to_string())
            .unwrap_or_else(|| format!("Source {value}")),
        noun::SAMPLE_RATE => ["44.1 kHz", "48 kHz", "96 kHz"]
            .get(value.max(0) as usize)
            .map(|s| (*s).to_string())
            .unwrap_or_else(|| format!("Rate {value}")),
        noun::LEVELLER_SPEED => ["Slow", "Medium", "Fast"]
            .get(value.max(0) as usize)
            .map(|s| (*s).to_string())
            .unwrap_or_else(|| format!("Speed {value}")),
        noun::CROSSFEED_PRESET => ["Default", "Chu Moy", "Meier", "Custom"]
            .get(value.max(0) as usize)
            .map(|s| (*s).to_string())
            .unwrap_or_else(|| format!("Preset {value}")),
        // Caps v5 appended Off as value 2; the surround enum puts Off first.
        noun::UPMIX_CENTER_MODE => ["Sinner", "Logician", "Off"]
            .get(value.max(0) as usize)
            .map(|s| (*s).to_string())
            .unwrap_or_else(|| format!("Mode {value}")),
        noun::UPMIX_SURROUND_MODE => ["Off", "Sinner", "Logician"]
            .get(value.max(0) as usize)
            .map(|s| (*s).to_string())
            .unwrap_or_else(|| format!("Mode {value}")),
        // PEQ passes read as cuts (`DSPi_ConsoleApp.swift:7426-7428`).
        noun::FILTER_TYPE => [
            "Flat",
            "Peaking",
            "Low Shelf",
            "High Shelf",
            "High Cut",
            "Low Cut",
            "Notch",
            "All Pass",
            "All Pass (1st)",
            "Low Shelf (1st)",
            "High Shelf (1st)",
        ]
        .get(value.max(0) as usize)
        .map(|s| (*s).to_string())
        .unwrap_or_else(|| format!("Type {value}")),
        _ => value.to_string(),
    }
}

// ---------------------------------------------------------------------------
// The caps-driven option lists
// ---------------------------------------------------------------------------

/// Nouns a component type can drive: a non-empty action intersection, and a
/// target space this device actually has.
///
/// A noun whose caps `actions` mask is zero is unavailable on this platform
/// (ADAT on an RP2040, say) and intersects with nothing, so it never appears. A
/// noun that addresses a channel space with no channels in it is unavailable
/// the other way round: there is nowhere for it to point, and the picker under
/// it would be empty.
pub fn valid_nouns(cs: &CsData, t: u8) -> Vec<u8> {
    let Some(td) = cs.type_desc(t) else {
        return Vec::new();
    };
    (0..cs.nouns.len() as u8)
        .filter(|n| {
            let nd = &cs.nouns[*n as usize];
            nd.actions & td.actions != 0 && (nd.target_kind == target::NONE || nd.target_count > 0)
        })
        .collect()
}

/// Legal actions for a (type, noun) pair: the AND of their two masks.
pub fn valid_actions(cs: &CsData, t: u8, n: u8) -> Vec<u8> {
    let (Some(td), Some(nd)) = (cs.type_desc(t), cs.noun_desc(n)) else {
        return Vec::new();
    };
    let eff = td.actions & nd.actions;
    (0..16u8).filter(|a| eff & act_bit(*a) != 0).collect()
}

/// A sensible default action for a freshly picked (type, noun).
pub fn default_action(cs: &CsData, t: u8, n: u8) -> u8 {
    let avail = valid_actions(cs, t, n);
    if avail.is_empty() {
        return 0;
    }
    let pref: &[u8] = match t {
        ty::POT => &[act::ADJUST],
        ty::ENCODER => &[act::STEP],
        ty::SWITCH => &[act::FOLLOW],
        ty::LED => &[act::IND_EQUALS, act::IND_ABOVE],
        ty::LED_PWM => &[act::IND_LEVEL, act::IND_ABOVE, act::IND_EQUALS],
        // A button and a remote key share one action repertoire.
        ty::BUTTON | ty::IR => &[
            act::TOGGLE,
            act::TRIGGER,
            act::INC,
            act::SET,
            act::DEC,
            act::MOMENTARY,
        ],
        _ => &[],
    };
    for a in pref {
        if avail.contains(a) {
            return *a;
        }
    }
    avail[0]
}

/// One submenu of the parameter picker.
pub struct NounCategory {
    pub name: &'static str,
    pub nouns: &'static [u8],
}

/// The Console's `nounCategories`, in its order. A noun not named here (a
/// future firmware's addition) still appears, under "Other".
pub const NOUN_CATEGORIES: &[NounCategory] = &[
    NounCategory {
        name: "Volume & Mute",
        nouns: &[noun::USER_VOLUME, noun::MASTER_VOLUME, noun::USER_MUTE],
    },
    NounCategory {
        name: "Loudness",
        nouns: &[noun::LOUDNESS, noun::LOUDNESS_SPL, noun::LOUDNESS_INTENSITY],
    },
    NounCategory {
        name: "Crossfeed",
        nouns: &[noun::CROSSFEED, noun::CROSSFEED_PRESET, noun::CROSSFEED_ITD],
    },
    NounCategory {
        name: "Volume Leveller",
        nouns: &[
            noun::LEVELLER,
            noun::LEVELLER_AMOUNT,
            noun::LEVELLER_SPEED,
            noun::LEVELLER_LOOKAHEAD,
        ],
    },
    NounCategory {
        name: "Psychoacoustic Bass",
        nouns: &[
            noun::PSYBASS,
            noun::PSYBASS_CUTOFF,
            noun::PSYBASS_HARMONICS,
            noun::PSYBASS_DRIVE,
            noun::PSYBASS_CHARACTER,
            noun::PSYBASS_ORIGINAL,
        ],
    },
    NounCategory {
        name: "Upmixer",
        nouns: &[
            noun::UPMIX,
            noun::UPMIX_CENTER_MODE,
            noun::UPMIX_SURROUND_MODE,
            noun::UPMIX_STRENGTH,
            noun::UPMIX_WIDTH,
            noun::UPMIX_PRESENCE,
        ],
    },
    NounCategory {
        name: "Input & Presets",
        nouns: &[
            noun::PRESET,
            noun::PRESET_RELOAD,
            noun::INPUT_SOURCE,
            noun::LG_SYNC,
        ],
    },
    NounCategory {
        name: "Channels",
        nouns: &[
            noun::PREAMP,
            noun::OUTPUT_GAIN,
            noun::OUTPUT_MUTE,
            noun::OUTPUT_ENABLE,
            noun::OUTPUT_DELAY,
        ],
    },
    NounCategory {
        name: "Filters",
        nouns: &[
            noun::EQ_BYPASS,
            noun::FILTER_FREQ,
            noun::FILTER_GAIN,
            noun::FILTER_Q,
            noun::FILTER_TYPE,
            noun::FILTER_BYPASS,
        ],
    },
    NounCategory {
        name: "Tools",
        nouns: &[noun::MACRO, noun::SIGGEN, noun::DAC_MUTE_TEST, noun::CLIP],
    },
    NounCategory {
        name: "Display",
        nouns: &[noun::DISPLAY_PAGE, noun::PAGE_VALUE, noun::DISPLAY_EDIT],
    },
    NounCategory {
        name: "Status",
        nouns: &[
            noun::CPU_LOAD,
            noun::CLIP_CH,
            noun::LEVEL,
            noun::INPUT_LEVEL_MAX,
            noun::SPDIF_LOCK,
            noun::SAMPLE_RATE,
            noun::USB_STREAMING,
            noun::ADAT_ACTIVE,
            noun::LG_PRESENT,
            noun::LG_MUTED,
        ],
    },
];

/// The valid nouns for a type, bucketed into the Console's categories, empty
/// categories dropped and anything unclaimed collected under "Other".
///
/// The terminal has no submenus, so the popup is one flat list with the
/// category as a `dim` header, which DESIGN 6.4 calls the hierarchical variant.
pub fn noun_groups(cs: &CsData, t: u8) -> Vec<(&'static str, Vec<u8>)> {
    let valid: BTreeSet<u8> = valid_nouns(cs, t).into_iter().collect();
    let mut used: BTreeSet<u8> = BTreeSet::new();
    let mut out: Vec<(&'static str, Vec<u8>)> = Vec::new();
    for cat in NOUN_CATEGORIES {
        let ns: Vec<u8> = cat
            .nouns
            .iter()
            .copied()
            .filter(|n| valid.contains(n))
            .collect();
        if ns.is_empty() {
            continue;
        }
        used.extend(ns.iter().copied());
        out.push((cat.name, ns));
    }
    let others: Vec<u8> = valid
        .iter()
        .copied()
        .filter(|n| !used.contains(n))
        .collect();
    if !others.is_empty() {
        out.push(("Other", others));
    }
    out
}

/// The picker's flat list: every valid noun in category order, and the header
/// each entry sits under.
pub fn noun_choices(cs: &CsData, t: u8) -> Vec<(&'static str, u8)> {
    noun_groups(cs, t)
        .into_iter()
        .flat_map(|(cat, ns)| ns.into_iter().map(move |n| (cat, n)))
        .collect()
}

/// Group slots whose channel space matches what a noun targets. A band noun
/// addresses DSP channels, so it takes a DSP_CH group.
pub fn compatible_groups(cs: &CsData, n: u8) -> Vec<u8> {
    let Some(nd) = cs.noun_desc(n) else {
        return Vec::new();
    };
    if !nd.is_targeted() {
        return Vec::new();
    }
    let wanted = if nd.target_kind == target::DSP_BAND {
        target::DSP_CH
    } else {
        nd.target_kind
    };
    (0..cs.groups.len() as u8)
        .filter(|g| {
            let grp = &cs.groups[*g as usize];
            grp.is_configured() && grp.target_kind == wanted
        })
        .collect()
}

/// "Front Pair (2 ch)" for the group half of a target picker.
pub fn group_menu_label(cs: &CsData, g: u8) -> String {
    let name = cs.group_name(g as usize);
    match cs.groups.get(g as usize) {
        Some(grp) if grp.is_configured() => format!("{name} ({} ch)", grp.members().len()),
        _ => format!("{name} (empty)"),
    }
}

/// The merged Channels / Groups list a target picker offers, and where the
/// current target sits in it.
///
/// Channels and groups are one question - what does this affect - so they are
/// one picker; `target` meaning a channel or a group index depending on
/// `CS_FLAG_GROUP` is a wire detail the selection carries instead. A group that
/// has since been emptied or re-kinded still shows, or the picker would
/// silently retarget the binding to a channel behind the user's back.
pub(crate) fn target_choices(
    cx: &Cx<'_>,
    cs: &CsData,
    nd: &CsNounDesc,
    usable: &[u8],
    target: u8,
    grouped: bool,
) -> (Vec<String>, usize) {
    let mut choices: Vec<String> = (0..nd.target_count)
        .map(|t| target_name(cx.state, nd, t))
        .collect();
    let channels = choices.len();
    let mut groups: Vec<u8> = usable.to_vec();
    if grouped && !groups.contains(&target) {
        groups.push(target);
    }
    // The Console sections the menu into Channels and Groups; a terminal popup
    // is one flat list, so the group half says so on each row instead.
    choices.extend(
        groups
            .iter()
            .map(|g| format!("Group: {}", group_menu_label(cs, *g))),
    );
    let selected = if grouped {
        channels + groups.iter().position(|g| *g == target).unwrap_or(0)
    } else {
        (target as usize).min(channels.saturating_sub(1))
    };
    (choices, selected)
}

/// Where a choice from [`target_choices`] lands: a channel, or a group.
pub(crate) fn target_choice(
    nd: &CsNounDesc,
    usable: &[u8],
    target: u8,
    grouped: bool,
    choice: usize,
) -> (bool, u8) {
    let channels = nd.target_count as usize;
    if choice < channels {
        return (false, choice as u8);
    }
    let mut groups: Vec<u8> = usable.to_vec();
    if grouped && !groups.contains(&target) {
        groups.push(target);
    }
    match groups.get(choice - channels) {
        Some(g) => (true, *g),
        None => (false, 0),
    }
}

/// Valid filter bands on one DSP channel: every PEQ band the device serves,
/// plus the four crossover bands for FILTER_FREQ and FILTER_BYPASS on an
/// output.
///
/// The PEQ count is `caps.max_bands`, which the device answers; the `20..=23`
/// crossover range beside it is a frozen firmware constant.
pub fn bands_on_channel(state: &DeviceState, n: u8, dsp_channel: u8) -> Vec<u8> {
    let mut bands: Vec<u8> = (0..state.caps.max_bands).collect();
    let is_output = dsp_channel >= state.caps.num_inputs;
    if is_output && (n == noun::FILTER_FREQ || n == noun::FILTER_BYPASS) {
        bands.extend([20, 21, 22, 23]);
    }
    bands
}

/// Bands offerable for a target that may be a group. The firmware needs the
/// band to be valid for every member, so a mixed group offers only what they
/// share; picking a crossover band on one would just fail validation.
pub fn band_options(cs: &CsData, state: &DeviceState, n: u8, t: u8, grouped: bool) -> Vec<u8> {
    if !grouped {
        return bands_on_channel(state, n, t);
    }
    let members = cs
        .groups
        .get(t as usize)
        .map(|g| g.members())
        .unwrap_or_default();
    if members.is_empty() {
        return (0..state.caps.max_bands).collect();
    }
    let mut common: Option<BTreeSet<u8>> = None;
    for m in members {
        let s: BTreeSet<u8> = bands_on_channel(state, n, m).into_iter().collect();
        common = Some(match common {
            Some(c) => c.intersection(&s).copied().collect(),
            None => s,
        });
    }
    common.unwrap_or_default().into_iter().collect()
}

// ---------------------------------------------------------------------------
// Drafts that stay encodable
// ---------------------------------------------------------------------------

/// Whether the firmware accepts on/off delays on this (type, action) pair: an
/// LED following a boolean condition, and nothing else. A delay left anywhere
/// else is rejected outright, so the editor clears the fields on the way out.
pub fn delays_allowed(t: u8, action: u8) -> bool {
    is_indicator(t) && (action == act::IND_EQUALS || action == act::IND_ABOVE)
}

/// The kind a noun's operands follow. `PAGE_VALUE` resolves its item at event
/// time and must carry no operands at all, which is neither of the three real
/// kinds.
pub fn operand_kind(cs: &CsData, n: u8) -> u8 {
    if n == noun::PAGE_VALUE {
        return kind::NONE;
    }
    cs.noun_desc(n).map(|d| d.kind).unwrap_or(kind::BOOL)
}

pub fn noun_unit(cs: &CsData, n: u8) -> u8 {
    cs.noun_desc(n).map(|d| d.unit).unwrap_or(unit::DB)
}

/// A noun's full natural-unit range.
pub fn noun_range(cs: &CsData, n: u8) -> (f64, f64) {
    let nd = cs.noun_desc(n).copied().unwrap_or_default();
    let u = nd.unit;
    (decode_value(nd.min_q, u), decode_value(nd.max_q, u))
}

/// Reset value, step and range for a binding's action and noun kind.
pub fn default_operands(cs: &CsData, b: &CsBinding) -> CsBinding {
    let mut b = b.clone();
    let nd = cs.noun_desc(b.noun).copied().unwrap_or_default();
    let k = operand_kind(cs, b.noun);
    let u = nd.unit;
    b.value = 0;
    b.step = 0;
    b.range_min = 0;
    b.range_max = 0;
    if !delays_allowed(b.component, b.action) {
        b.on_delay = 0;
        b.off_delay = 0;
    }
    if b.component != ty::LED_PWM {
        b.base_bright = 0;
    }
    match b.action {
        act::STEP | act::INC | act::DEC => {
            if k == kind::ENUM {
                b.step = 1;
            }
        }
        act::SET | act::MOMENTARY => {
            if k == kind::CONTINUOUS {
                b.value = nd.max_q;
            } else if k == kind::BOOL {
                b.value = 1;
            }
        }
        act::IND_EQUALS => {
            if k == kind::BOOL {
                b.value = 1;
            }
        }
        act::IND_ABOVE => {
            // A quarter of the way up the range: -45 dB on a -60..0 meter.
            if k == kind::CONTINUOUS {
                let (lo, hi) = (decode_value(nd.min_q, u), decode_value(nd.max_q, u));
                b.value = encode_value(lo + 0.25 * (hi - lo), u);
            }
        }
        _ => {}
    }
    if b.noun == noun::PAGE_VALUE {
        b.value = 0;
        b.step = 0;
        b.range_min = 0;
        b.range_max = 0;
    }
    sanitize_group_flags(cs, &b)
}

/// Bring the group flags back into the set the firmware accepts after a noun or
/// action edit. The whole binding is rejected if any of the three bits is set
/// where it does not belong, so a stale bit would break Apply with a message
/// pointing at nothing the user can see.
pub fn sanitize_group_flags(cs: &CsData, b: &CsBinding) -> CsBinding {
    let mut b = b.clone();
    if b.flags & flag::GROUP == 0 {
        b.flags &= !(flag::LINK_ABS | flag::GROUP_ALL);
        return b;
    }
    let usable = compatible_groups(cs, b.noun);
    if usable.is_empty() || b.action == act::TRIGGER {
        b.flags &= !(flag::GROUP | flag::LINK_ABS | flag::GROUP_ALL);
        b.target = 0;
        return b;
    }
    if !usable.contains(&b.target) {
        b.target = usable[0];
    }
    let k = operand_kind(cs, b.noun);
    if !(b.action == act::ADJUST && k == kind::CONTINUOUS) {
        b.flags &= !flag::LINK_ABS;
    }
    if !(b.action == act::IND_EQUALS || b.action == act::IND_ABOVE) {
        b.flags &= !flag::GROUP_ALL;
    }
    b
}

/// The slot holding the IR receiver, counting drafts, if any. One per device.
pub fn container_slot(drafts: &[CsBinding], live: &CsData, want: u8) -> Option<usize> {
    (0..drafts.len()).find(|s| drafts[*s].component == want || live.binding(*s).component == want)
}

/// ADC-capable GPIOs, the only pins a potentiometer may occupy. GPIO 29 is the
/// VSYS monitor and is not offered.
pub const ADC_PINS: [u8; 3] = [26, 27, 28];

/// Every GPIO a control may claim.
pub(crate) fn all_pins(cx: &Cx<'_>) -> Vec<u8> {
    dspi_session::pins::valid_pins(cx.platform())
}

/// The owner label `PinMap` gives a control-surface slot.
pub fn slot_owner(slot: usize) -> String {
    format!("Control Surface {}", slot + 1)
}

/// GPIOs a pin picker may offer: mux-free pins (ADC only for a pot), the
/// current pin always kept visible, and the sibling encoder pin excluded so the
/// two channels cannot collide.
pub(crate) fn pin_candidates(
    cx: &Cx<'_>,
    cs: &CsData,
    drafts: &[CsBinding],
    slot: usize,
    is_second: bool,
) -> Vec<u8> {
    let b = &drafts[slot];
    let td = cs.type_desc(b.component).copied().unwrap_or_default();
    let adc = td.pin_class == dspi_proto::packets::CsTypeDesc::PINCLASS_ADC;
    let two_pin = td.pin_count >= 2;
    let base: Vec<u8> = if adc { ADC_PINS.to_vec() } else { all_pins(cx) };
    let sibling = two_pin.then(|| if is_second { b.gpio[0] } else { b.gpio[1] });
    let current = if is_second { b.gpio[1] } else { b.gpio[0] };
    let owner = slot_owner(slot);
    base.into_iter()
        .filter(|p| {
            if Some(*p) == sibling {
                return false;
            }
            if *p == current {
                return true;
            }
            if cx.data.owner_of(cx.state, *p, &owner).is_none() {
                return true;
            }
            // A button may share a GPIO whose only other owners are buttons:
            // one binding per gesture (survey-firmware 3.3).
            b.component == ty::BUTTON && button_shareable(cs, *p, slot)
        })
        .collect()
}

/// True when every live binding on this pin other than `excluding` is a button.
fn button_shareable(cs: &CsData, pin: u8, excluding: usize) -> bool {
    let mut saw_button = false;
    for (s, b) in cs.bindings.iter().enumerate() {
        if s == excluding || !cs.status.is_slot_active(s as u8) {
            continue;
        }
        if !b.pins().contains(&pin) {
            continue;
        }
        if b.component != ty::BUTTON {
            return false;
        }
        saw_button = true;
    }
    saw_button
}

/// Legal SDA pins for the display: the mux pairs GPIOs so bit 0 picks SDA
/// (even) or SCL (odd), so SDA is always even and its odd neighbour is SCL.
pub(crate) fn i2c_sda_candidates(cx: &Cx<'_>, drafts: &[CsBinding], slot: usize) -> Vec<u8> {
    let valid: BTreeSet<u8> = all_pins(cx).into_iter().collect();
    // The display masters one I2C instance; the control interface holds the
    // other while it is live, and the firmware rejects an overlap outright.
    let blocked = if cx.data.iface.as_ref().is_some_and(|s| s.i2c_live) {
        cx.data.i2c.as_ref().map(|c| (c.sda_pin >> 1) & 1)
    } else {
        None
    };
    let owner = slot_owner(slot);
    valid
        .iter()
        .copied()
        .filter(|sda| {
            if Some((sda >> 1) & 1) == blocked {
                return false;
            }
            if sda % 2 != 0 || !valid.contains(&(sda + 1)) {
                return false;
            }
            if drafts[slot].component == ty::DISPLAY && *sda == drafts[slot].gpio[0] {
                return true;
            }
            cx.data.owner_of(cx.state, *sda, &owner).is_none()
                && cx.data.owner_of(cx.state, sda + 1, &owner).is_none()
        })
        .collect()
}

/// The first free legal SDA/SCL pair, falling back to the lowest legal pair.
pub(crate) fn free_i2c_pair(cx: &Cx<'_>, drafts: &[CsBinding], slot: usize) -> (u8, u8) {
    if let Some(sda) = i2c_sda_candidates(cx, drafts, slot).first() {
        return (*sda, sda + 1);
    }
    let sda = all_pins(cx).into_iter().find(|p| p % 2 == 0).unwrap_or(0);
    (sda, sda + 1)
}

/// Build a fresh binding when a slot gains or changes its component type.
pub(crate) fn make_binding(
    cx: &Cx<'_>,
    cs: &CsData,
    drafts: &[CsBinding],
    slot: usize,
    t: u8,
) -> CsBinding {
    if t == ty::NONE {
        return CsBinding::default();
    }
    let owner = slot_owner(slot);
    let free = |adc: bool| -> Vec<u8> {
        let base: Vec<u8> = if adc { ADC_PINS.to_vec() } else { all_pins(cx) };
        base.into_iter()
            .filter(|p| cx.data.owner_of(cx.state, *p, &owner).is_none())
            .collect()
    };
    // The IR receiver is a container: it carries only its pin and sense; noun,
    // action and every operand must be zero.
    if t == ty::IR {
        return CsBinding {
            component: ty::IR,
            gpio: [free(false).first().copied().unwrap_or(0), GPIO_UNUSED],
            ..Default::default()
        };
    }
    // So is the display: SDA, SCL, a model in `index` and an address in
    // `value`. SDA must be even and SCL the odd pin above it.
    if t == ty::DISPLAY {
        let (sda, scl) = free_i2c_pair(cx, drafts, slot);
        return CsBinding {
            component: ty::DISPLAY,
            gpio: [sda, scl],
            index: 6, // SSD1306 128x64, the Console's seed
            value: 0, // the model's conventional address
            ..Default::default()
        };
    }
    let td = cs.type_desc(t).copied().unwrap_or_default();
    let adc = td.pin_class == dspi_proto::packets::CsTypeDesc::PINCLASS_ADC;
    let two_pin = td.pin_count >= 2;
    let pins = free(adc);
    let n = valid_nouns(cs, t)
        .first()
        .copied()
        .unwrap_or(noun::MASTER_VOLUME);
    let gpio0 = pins
        .first()
        .copied()
        .unwrap_or(if adc { ADC_PINS[0] } else { 0 });
    let gpio1 = if two_pin {
        pins.get(1)
            .copied()
            .unwrap_or(if gpio0 == 0 { 1 } else { 0 })
    } else {
        GPIO_UNUSED
    };
    let b = CsBinding {
        component: t,
        noun: n,
        action: default_action(cs, t, n),
        gpio: [gpio0, gpio1],
        ..Default::default()
    };
    default_operands(cs, &b)
}

/// Retarget a binding after its noun changed: the new noun may address a
/// different space, or nothing at all.
pub fn set_noun(cs: &CsData, b: &CsBinding, n: u8) -> CsBinding {
    let mut b = b.clone();
    b.noun = n;
    b.target = 0;
    b.index = 0;
    let acts = valid_actions(cs, b.component, n);
    if !acts.contains(&b.action) {
        b.action = default_action(cs, b.component, n);
    }
    default_operands(cs, &b)
}

// ---------------------------------------------------------------------------
// Macro steps
// ---------------------------------------------------------------------------

/// Nouns a macro step can drive: those accepting at least one step action.
/// MACRO and PAGE_VALUE are rejected as step nouns by the firmware.
pub fn macro_step_nouns(cs: &CsData) -> Vec<u8> {
    let mask = MACRO_STEP_ACTIONS.iter().fold(0u16, |m, a| m | act_bit(*a));
    (0..cs.nouns.len() as u8)
        .filter(|n| {
            *n != noun::MACRO && *n != noun::PAGE_VALUE && cs.nouns[*n as usize].actions & mask != 0
        })
        .collect()
}

pub fn macro_step_actions(cs: &CsData, n: u8) -> Vec<u8> {
    let Some(nd) = cs.noun_desc(n) else {
        return Vec::new();
    };
    MACRO_STEP_ACTIONS
        .iter()
        .copied()
        .filter(|a| nd.actions & act_bit(*a) != 0)
        .collect()
}

/// Reset a step's operands for its action and noun kind, the step-shaped
/// mirror of [`default_operands`].
pub fn default_step_operands(cs: &CsData, step: &CsMacroStep) -> CsMacroStep {
    let mut st = step.clone();
    let nd = cs.noun_desc(st.noun).copied().unwrap_or_default();
    let k = operand_kind(cs, st.noun);
    st.value = 0;
    st.step = 0;
    let is_enum_step = k == kind::ENUM && (st.action == act::INC || st.action == act::DEC);
    if !is_enum_step {
        st.flags &= !flag::WRAP;
    }
    if st.action == act::SET {
        if k == kind::CONTINUOUS {
            st.value = nd.max_q;
        } else if k == kind::BOOL {
            st.value = 1;
        }
    }
    // A trigger is never grouped, the same rule bindings follow.
    if st.action == act::TRIGGER || !nd.is_targeted() {
        st.flags &= !flag::GROUP;
        st.target = 0;
    }
    st
}

/// A fresh step: the first noun that accepts a step action, already valid.
pub fn default_macro_step(cs: &CsData) -> CsMacroStep {
    let n = macro_step_nouns(cs)
        .first()
        .copied()
        .unwrap_or(noun::USER_MUTE);
    let st = CsMacroStep {
        noun: n,
        action: macro_step_actions(cs, n)
            .first()
            .copied()
            .unwrap_or(act::SET),
        ..Default::default()
    };
    default_step_operands(cs, &st)
}

// ---------------------------------------------------------------------------
// Display pages
// ---------------------------------------------------------------------------

/// Nouns a page can show: anything the platform has, minus the three display
/// nouns themselves, which the firmware rejects as page nouns.
pub fn display_page_nouns(cs: &CsData) -> Vec<u8> {
    (0..cs.nouns.len() as u8)
        .filter(|n| {
            cs.nouns[*n as usize].is_available()
                && *n != noun::DISPLAY_PAGE
                && *n != noun::DISPLAY_EDIT
                && *n != noun::PAGE_VALUE
        })
        .collect()
}

/// Whether a page may carry a level bar (caps v13). The bar plots the value
/// inside the noun's own range, so it needs one: the firmware refuses the flag
/// on every switch and mode, and on any continuous noun with an empty range.
pub fn page_bar_allowed(cs: &CsData, n: u8) -> bool {
    let Some(nd) = cs.noun_desc(n) else {
        return false;
    };
    nd.kind == kind::CONTINUOUS && decode_value(nd.max_q, nd.unit) > decode_value(nd.min_q, nd.unit)
}

/// "Out 3 Gain": the label the panel itself renders.
pub fn display_page_summary(state: &DeviceState, cs: &CsData, page: &CsDisplayPage) -> String {
    let name = noun_name(page.noun, ty::NONE);
    let Some(nd) = cs.noun_desc(page.noun) else {
        return name;
    };
    if !nd.is_targeted() {
        return name;
    }
    let where_ = if page.flags & dspi_proto::packets::page_flags::GROUP != 0 {
        cs.group_name(page.target as usize)
    } else {
        target_name(state, nd, page.target)
    };
    format!("{where_} {name}")
}

// ---------------------------------------------------------------------------
// Plain-language summaries
// ---------------------------------------------------------------------------

/// "Press" / "Long-press" / "Double-press" for a button binding's gesture.
fn press_word(b: &CsBinding) -> &'static str {
    if b.component != ty::BUTTON {
        return "Press";
    }
    match b.event {
        1 => "Long-press",
        2 => "Double-press",
        _ => "Press",
    }
}

/// " (Output 1, Band 3)" for a targeted binding's summary. A grouped binding
/// names the group instead: `target` means something else entirely in that
/// state.
fn target_suffix(state: &DeviceState, cs: &CsData, b: &CsBinding) -> String {
    let Some(nd) = cs.noun_desc(b.noun) else {
        return String::new();
    };
    if !nd.is_targeted() {
        return String::new();
    }
    let mut s = if b.flags & flag::GROUP != 0 {
        format!(" ({}", cs.group_name(b.target as usize))
    } else {
        format!(" ({}", target_name(state, nd, b.target))
    };
    if nd.has_band() {
        s += &format!(", {}", band_name(b.index));
    }
    s + ")"
}

/// The Console's `verbPhrase`: one line saying what a control does, plus its
/// condition timing and brightness cap when it carries any. A collapsed card
/// shows only this.
pub fn verb_phrase(state: &DeviceState, cs: &CsData, b: &CsBinding) -> String {
    let mut phrase = action_phrase(state, cs, b);
    if b.on_delay != 0 || b.off_delay != 0 {
        let mut parts: Vec<String> = Vec::new();
        if b.on_delay != 0 {
            parts.push(format!("{} on", fmt_delay(b.on_delay)));
        }
        if b.off_delay != 0 {
            parts.push(format!("{} off", fmt_delay(b.off_delay)));
        }
        phrase += &format!(" Delayed {}.", parts.join(", "));
    }
    if b.base_bright != 0 && b.base_bright < LED_BRIGHT_MAX {
        phrase += &format!(" Up to {}% bright.", b.base_bright);
    }
    phrase
}

fn action_phrase(state: &DeviceState, cs: &CsData, b: &CsBinding) -> String {
    if b.component == ty::IR {
        return "Receives commands from an IR remote.".into();
    }
    if b.component == ty::DISPLAY {
        let addr = if b.value == 0 {
            display_default_address(b.index)
        } else {
            b.value as u8
        };
        return format!(
            "{} on I2C, address 0x{addr:X}.",
            display_model_name(b.index)
        );
    }
    if b.noun == noun::PAGE_VALUE {
        return page_value_phrase(cs, b);
    }
    if b.noun == noun::DISPLAY_PAGE {
        return display_page_phrase(b);
    }
    let n = noun_name(b.noun, b.component) + &target_suffix(state, cs, b);
    let is_enum = operand_kind(cs, b.noun) == kind::ENUM;
    let press = press_word(b);
    match b.action {
        act::ADJUST => format!("Turn to set {n}."),
        act::STEP => format!("Turn to step {n}."),
        act::INC => {
            if is_enum {
                format!("{press} to select the next {n}.")
            } else {
                format!("{press} to raise {n}.")
            }
        }
        act::DEC => {
            if is_enum {
                format!("{press} to select the previous {n}.")
            } else {
                format!("{press} to lower {n}.")
            }
        }
        act::TOGGLE => format!("{press} to toggle {n}."),
        act::SET => format!("{press} to set {n}."),
        act::MOMENTARY => format!("Hold to engage {n}; releases when let go."),
        act::FOLLOW => format!("{n} follows the switch position."),
        // The noun name carries the verb for a trigger ("Clear Clipping").
        act::TRIGGER => format!("{press} to {}.", n.to_lowercase()),
        act::IND_EQUALS => format!("Lights to indicate {n}."),
        act::IND_ABOVE => format!("Lights when {n} is above a level."),
        act::IND_LEVEL => format!("Brightness follows {n}."),
        _ => String::new(),
    }
}

/// The page noun names the panel, not a list position, so the generic enum
/// wording ("select the next Show Page") would read as nonsense.
fn display_page_phrase(b: &CsBinding) -> String {
    let press = press_word(b);
    match b.action {
        act::STEP => "Turn to move through the pages on screen.".into(),
        act::INC => format!("{press} to show the next page."),
        act::DEC => format!("{press} to show the previous page."),
        act::SET => format!("{press} to show a set page."),
        act::IND_EQUALS => "Lights while a set page is on screen.".into(),
        _ => String::new(),
    }
}

/// Browse/Adjust does two jobs, and which one a control gets depends on the
/// panel's own gate: with "Arm Before Editing" off it only ever adjusts.
fn page_value_phrase(cs: &CsData, b: &CsBinding) -> String {
    let gated = cs.display_cfg.flags & dspi_proto::packets::display_flags::EDIT_GATED != 0;
    let press = press_word(b);
    match b.action {
        act::STEP => if gated {
            "Turn to move through pages, or to adjust the shown value once editing is armed."
        } else {
            "Turn to adjust the shown value."
        }
        .into(),
        act::INC => {
            if gated {
                format!(
                    "{press} for the next page, or to raise the shown value once editing is armed."
                )
            } else {
                format!("{press} to raise the shown value.")
            }
        }
        act::DEC => {
            if gated {
                format!(
                    "{press} for the previous page, or to lower the shown value once editing is armed."
                )
            } else {
                format!("{press} to lower the shown value.")
            }
        }
        // The firmware silently no-ops a toggle on anything but an on/off item.
        act::TOGGLE => {
            if gated {
                format!(
                    "{press} to toggle the shown value, once editing is armed. Only acts on a page \
                     showing an on/off setting."
                )
            } else {
                format!(
                    "{press} to toggle the shown value. Only acts on a page showing an on/off \
                     setting."
                )
            }
        }
        _ => String::new(),
    }
}

/// The Console's `irVerbPhrase`, for a collapsed remote-button card.
pub fn ir_verb_phrase(state: &DeviceState, cs: &CsData, c: &IrCommand) -> String {
    if c.noun == noun::DISPLAY_PAGE {
        return match c.action {
            act::INC => "Show the next page".into(),
            act::DEC => "Show the previous page".into(),
            act::SET => "Show a set page".into(),
            _ => "Show Page".into(),
        };
    }
    if c.noun == noun::PAGE_VALUE {
        let gated = cs.display_cfg.flags & dspi_proto::packets::display_flags::EDIT_GATED != 0;
        return match c.action {
            act::INC => if gated {
                "Next page, or value up when armed"
            } else {
                "Shown value up"
            }
            .into(),
            act::DEC => if gated {
                "Previous page, or value down when armed"
            } else {
                "Shown value down"
            }
            .into(),
            act::TOGGLE => "Toggle the shown value (on/off pages only)".into(),
            _ => "Browse/Adjust".into(),
        };
    }
    let mut n = noun_name(c.noun, ty::IR);
    if let Some(nd) = cs.noun_desc(c.noun)
        && nd.is_targeted()
    {
        n += &if c.flags & flag::GROUP != 0 {
            format!(" ({})", cs.group_name(c.target as usize))
        } else if nd.has_band() {
            format!(
                " ({}, {})",
                target_name(state, nd, c.target),
                band_name(c.index)
            )
        } else {
            format!(" ({})", target_name(state, nd, c.target))
        };
    }
    let is_enum = operand_kind(cs, c.noun) == kind::ENUM;
    match c.action {
        act::INC => {
            if is_enum {
                format!("Next {n}")
            } else {
                format!("Raise {n}")
            }
        }
        act::DEC => {
            if is_enum {
                format!("Previous {n}")
            } else {
                format!("Lower {n}")
            }
        }
        act::TOGGLE => format!("Toggle {n}"),
        act::SET => format!("Set {n}"),
        act::MOMENTARY => format!("Hold {n}"),
        _ => n,
    }
}

/// One macro step in words, for its row header.
pub fn macro_step_summary(state: &DeviceState, cs: &CsData, st: &CsMacroStep) -> String {
    let n = noun_name(st.noun, ty::BUTTON);
    let t = match cs.noun_desc(st.noun) {
        Some(nd) if nd.is_targeted() => {
            if st.flags & flag::GROUP != 0 {
                format!(" ({})", cs.group_name(st.target as usize))
            } else {
                format!(" ({})", target_name(state, nd, st.target))
            }
        }
        _ => String::new(),
    };
    let verb = match st.action {
        act::SET => format!("set {n}{t}"),
        act::TOGGLE => format!("toggle {n}{t}"),
        act::INC => format!("raise {n}{t}"),
        act::DEC => format!("lower {n}{t}"),
        act::TRIGGER => n.to_lowercase(),
        _ => n,
    };
    if st.pre_delay == 0 {
        let mut c = verb.chars();
        match c.next() {
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            None => verb,
        }
    } else {
        format!(
            "After {}, {verb}",
            fmt_delay_seconds(decode_step_delay(st.pre_delay))
        )
    }
}

/// "4 outputs - Out 1, Out 2, ..." for a collapsed group card.
pub fn group_summary(state: &DeviceState, group: &CsGroup) -> String {
    if !group.is_configured() {
        return "No channels selected.".into();
    }
    let names: Vec<String> = group
        .members()
        .into_iter()
        .map(|ch| group_channel_name(state, group.target_kind, ch))
        .collect();
    let head = names.iter().take(4).cloned().collect::<Vec<_>>().join(", ");
    if names.len() > 4 {
        format!("{head} +{} more", names.len() - 4)
    } else {
        head
    }
}

/// A channel's name inside a group's own numbering.
pub fn group_channel_name(state: &DeviceState, kind: u8, ch: u8) -> String {
    let nd = CsNounDesc {
        target_kind: kind,
        target_count: 255,
        ..Default::default()
    };
    target_name(state, &nd, ch)
}

/// "3 steps, 1.5 s total" for a collapsed macro card.
pub fn macro_summary(m: &CsMacro, running: Option<u8>) -> String {
    if let Some(step) = running {
        return format!("Running - step {} of {}.", step as u16 + 1, m.step_count);
    }
    let n = m.step_count as usize;
    if n == 0 {
        return "No steps yet.".into();
    }
    let total: f64 = m
        .active_steps()
        .iter()
        .map(|s| decode_step_delay(s.pre_delay))
        .sum();
    let base = format!("{n} step{}", if n == 1 { "" } else { "s" });
    if total > 0.0 {
        format!("{base}, {} total", fmt_delay_seconds(total))
    } else {
        base
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A device with control surfaces, for the gallery and the golden tests.
pub mod demo {
    use super::*;
    use dspi_proto::packets::{CsTypeDesc, page_flags};

    /// The v1.1.6 type table (control_surfaces.h), so every picker is exercised.
    pub fn caps() -> ControlSurfaceCaps {
        // Action masks per type, as the firmware publishes them.
        let t = |actions: u16, pin_count: u8, pin_class: u8| CsTypeDesc {
            actions,
            pin_count,
            pin_class,
        };
        let bit = act_bit;
        let button = bit(act::INC)
            | bit(act::DEC)
            | bit(act::TOGGLE)
            | bit(act::SET)
            | bit(act::TRIGGER)
            | bit(act::MOMENTARY);
        ControlSurfaceCaps {
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
            types: vec![
                t(0, 0, 0),
                t(button, 1, 0),
                t(bit(act::FOLLOW), 1, 0),
                t(bit(act::ADJUST), 1, CsTypeDesc::PINCLASS_ADC),
                t(bit(act::STEP), 2, 0),
                t(bit(act::IND_EQUALS) | bit(act::IND_ABOVE), 1, 0),
                t(
                    bit(act::IND_EQUALS) | bit(act::IND_ABOVE) | bit(act::IND_LEVEL),
                    1,
                    0,
                ),
                t(button, 1, 0),
                t(0, 2, 0),
            ],
        }
    }

    /// A noun table shaped like the firmware's: available nouns with their
    /// kinds, units, ranges and target spaces, and one deliberately absent.
    pub fn nouns() -> Vec<CsNounDesc> {
        let bit = act_bit;
        let cont = bit(act::ADJUST) | bit(act::STEP) | bit(act::INC) | bit(act::DEC);
        let ind = bit(act::IND_ABOVE) | bit(act::IND_LEVEL);
        let boolean = bit(act::TOGGLE) | bit(act::SET) | bit(act::FOLLOW) | bit(act::IND_EQUALS);
        let enums = bit(act::STEP) | bit(act::INC) | bit(act::DEC) | bit(act::SET);
        let mut v = vec![CsNounDesc::default(); 57];
        let mut set = |n: u8, d: CsNounDesc| v[n as usize] = d;
        set(
            noun::USER_VOLUME,
            CsNounDesc {
                kind: kind::CONTINUOUS,
                actions: cont | ind,
                min_q: -60 * 256,
                max_q: 0,
                unit: unit::DB,
                ..Default::default()
            },
        );
        set(
            noun::MASTER_VOLUME,
            CsNounDesc {
                kind: kind::CONTINUOUS,
                actions: cont | ind,
                min_q: -80 * 256,
                max_q: 0,
                unit: unit::DB,
                ..Default::default()
            },
        );
        set(
            noun::USER_MUTE,
            CsNounDesc {
                kind: kind::BOOL,
                actions: boolean,
                ..Default::default()
            },
        );
        set(
            noun::PRESET,
            CsNounDesc {
                kind: kind::ENUM,
                enum_count: 10,
                actions: enums | bit(act::IND_EQUALS),
                max_q: 9,
                ..Default::default()
            },
        );
        set(
            noun::INPUT_SOURCE,
            CsNounDesc {
                kind: kind::ENUM,
                enum_count: 4,
                actions: enums,
                max_q: 3,
                ..Default::default()
            },
        );
        set(
            noun::CLIP,
            CsNounDesc {
                kind: kind::BOOL,
                actions: bit(act::TRIGGER) | bit(act::IND_EQUALS),
                ..Default::default()
            },
        );
        set(
            noun::PREAMP,
            CsNounDesc {
                kind: kind::CONTINUOUS,
                actions: cont,
                min_q: -40 * 256,
                max_q: 20 * 256,
                unit: unit::DB,
                target_kind: target::INPUT_CH,
                target_count: 8,
                ..Default::default()
            },
        );
        set(
            noun::OUTPUT_GAIN,
            CsNounDesc {
                kind: kind::CONTINUOUS,
                actions: cont | ind,
                min_q: -60 * 256,
                max_q: 20 * 256,
                unit: unit::DB,
                target_kind: target::OUTPUT_CH,
                target_count: 8,
                ..Default::default()
            },
        );
        set(
            noun::OUTPUT_MUTE,
            CsNounDesc {
                kind: kind::BOOL,
                actions: boolean,
                target_kind: target::OUTPUT_CH,
                target_count: 8,
                ..Default::default()
            },
        );
        set(
            noun::FILTER_FREQ,
            CsNounDesc {
                kind: kind::CONTINUOUS,
                actions: cont,
                min_q: 20,
                max_q: 20000,
                unit: unit::HZ,
                target_kind: target::DSP_BAND,
                target_count: 17,
                ..Default::default()
            },
        );
        set(
            noun::LEVEL,
            CsNounDesc {
                kind: kind::CONTINUOUS,
                actions: ind,
                min_q: -60 * 256,
                max_q: 0,
                unit: unit::DB,
                target_kind: target::DSP_CH,
                target_count: 17,
                ..Default::default()
            },
        );
        set(
            noun::MACRO,
            CsNounDesc {
                kind: kind::ENUM,
                enum_count: 8,
                actions: bit(act::SET) | bit(act::IND_EQUALS),
                max_q: 7,
                ..Default::default()
            },
        );
        set(
            noun::CPU_LOAD,
            CsNounDesc {
                kind: kind::CONTINUOUS,
                actions: ind,
                max_q: 100 * 256,
                unit: unit::PERCENT,
                ..Default::default()
            },
        );
        set(
            noun::DISPLAY_PAGE,
            CsNounDesc {
                kind: kind::ENUM,
                enum_count: 16,
                actions: enums | bit(act::IND_EQUALS),
                max_q: 15,
                ..Default::default()
            },
        );
        set(
            noun::DISPLAY_EDIT,
            CsNounDesc {
                kind: kind::BOOL,
                actions: boolean,
                ..Default::default()
            },
        );
        set(
            noun::PAGE_VALUE,
            CsNounDesc {
                kind: kind::ENUM,
                enum_count: 1,
                actions: bit(act::STEP) | bit(act::INC) | bit(act::DEC) | bit(act::TOGGLE),
                ..Default::default()
            },
        );
        // ADAT_ACTIVE is left with a zero action mask: the firmware's way of
        // saying the platform does not have it.
        v
    }

    /// A device holding one of nearly everything: an encoder, a button, a pot,
    /// an LED, an IR receiver with a learned key, a display with two pages, two
    /// groups and a macro.
    pub fn data() -> CsData {
        let caps = caps();
        let mut bindings = vec![CsBinding::default(); caps.max_bindings as usize];
        bindings[0] = CsBinding {
            component: ty::ENCODER,
            noun: noun::USER_VOLUME,
            action: act::STEP,
            flags: flag::ACCEL,
            gpio: [10, 11],
            ..Default::default()
        };
        bindings[1] = CsBinding {
            component: ty::BUTTON,
            noun: noun::USER_MUTE,
            action: act::TOGGLE,
            gpio: [16, GPIO_UNUSED],
            value: 1,
            ..Default::default()
        };
        bindings[2] = CsBinding {
            component: ty::POT,
            noun: noun::OUTPUT_GAIN,
            action: act::ADJUST,
            gpio: [26, GPIO_UNUSED],
            target: 1,
            ..Default::default()
        };
        bindings[3] = CsBinding {
            component: ty::IR,
            gpio: [15, GPIO_UNUSED],
            ..Default::default()
        };
        bindings[4] = CsBinding {
            component: ty::DISPLAY,
            gpio: [2, 3],
            index: 6,
            ..Default::default()
        };
        let mut names = vec![String::new(); caps.max_bindings as usize];
        names[0] = "Volume Knob".into();
        let mut ir = vec![IrCommand::default(); caps.max_ir_commands as usize];
        ir[0] = IrCommand {
            noun: noun::USER_VOLUME,
            action: act::INC,
            protocol: 1,
            code: 0x20DF_40BF,
            ..Default::default()
        };
        let mut groups = vec![CsGroup::default(); caps.max_groups as usize];
        groups[0] = CsGroup {
            target_kind: target::OUTPUT_CH,
            member_mask: 0b0011,
            name: "Front Pair".into(),
        };
        let mut macros = vec![CsMacro::default(); caps.max_macros as usize];
        macros[0] = CsMacro {
            name: "Night".into(),
            step_count: 2,
            steps: {
                let mut s = vec![CsMacroStep::default(); caps.max_macro_steps as usize];
                s[0] = CsMacroStep {
                    noun: noun::INPUT_SOURCE,
                    action: act::SET,
                    value: 1,
                    ..Default::default()
                };
                s[1] = CsMacroStep {
                    noun: noun::PRESET,
                    action: act::SET,
                    value: 2,
                    pre_delay: 50,
                    ..Default::default()
                };
                s
            },
        };
        let mut pages = vec![CsDisplayPage::default(); caps.max_pages as usize];
        pages[0] = CsDisplayPage {
            noun: noun::USER_VOLUME,
            target: 0,
            index: 0,
            flags: page_flags::ACTIVE | page_flags::LARGE,
        };
        pages[1] = CsDisplayPage {
            noun: noun::PRESET,
            target: 0,
            index: 0,
            flags: page_flags::ACTIVE,
        };
        let status = CsStatusPacket {
            last_status: 0,
            last_slot: 0,
            max_bindings: caps.max_bindings,
            dirty: false,
            active_mask: 0b0001_1111,
            slot_status: vec![0; caps.max_bindings as usize],
            ir_active_mask: 0b0001,
            ir_learn_state: 0,
            ir_cmd_status: vec![0; caps.max_ir_commands as usize],
        };
        CsData {
            nouns: nouns(),
            bindings,
            names,
            ir,
            groups,
            macros,
            display_cfg: CsDisplayCfg {
                mode: 1,
                home_page: 0,
                dwell: 30,
                overlay_hold: 20,
                brightness: 0,
                flags: dspi_proto::packets::display_flags::EDIT_GATED,
                edit_timeout: 100,
            },
            max_pages: caps.max_pages,
            model_count: caps.display_models,
            pages,
            status,
            ext: CsExtStatusPacket {
                max_groups: caps.max_groups,
                max_macros: caps.max_macros,
                max_macro_steps: caps.max_macro_steps,
                macro_running: CsExtStatusPacket::MACRO_NONE,
                ..Default::default()
            },
            display_status: CsDisplayStatus {
                init_state: 2,
                current_page: 0,
                model: 6,
                ..Default::default()
            },
            caps: CsCaps::from(&caps),
        }
    }

    /// The Settings demo device, with the control-surface capabilities the
    /// three Control pages need.
    pub fn state() -> DeviceState {
        let mut s = super::super::demo::state();
        s.caps.cs = Some(caps());
        s
    }

    /// The Settings demo data, with the control surfaces filled in.
    pub fn settings_data() -> super::super::SettingsData {
        super::super::SettingsData {
            cs: Some(data()),
            ..super::super::demo::data()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The PEQ band count is device-served (`caps.max_bands`, probe.rs:184);
    /// only the four crossover bands beside it are a frozen firmware constant.
    #[test]
    fn the_band_picker_is_sized_by_the_device() {
        let mut st = demo::state();
        st.caps.max_bands = 6;
        // An input has PEQ bands only.
        assert_eq!(
            bands_on_channel(&st, noun::FILTER_FREQ, 0),
            (0..6).collect::<Vec<u8>>()
        );
        // An output adds the crossover range for the two nouns that take it.
        let out = st.caps.num_inputs;
        assert_eq!(
            bands_on_channel(&st, noun::FILTER_FREQ, out),
            vec![0, 1, 2, 3, 4, 5, 20, 21, 22, 23]
        );
        assert_eq!(
            bands_on_channel(&st, noun::FILTER_GAIN, out),
            (0..6).collect::<Vec<u8>>()
        );

        // A group with no members falls back to the same served count.
        let cs = demo::data();
        assert_eq!(
            band_options(&cs, &st, noun::FILTER_FREQ, 7, true),
            (0..6).collect::<Vec<u8>>()
        );

        st.caps.max_bands = 16;
        assert_eq!(bands_on_channel(&st, noun::FILTER_GAIN, 0).len(), 16);
    }

    #[test]
    fn the_name_tables_are_the_consoles() {
        assert_eq!(type_name(ty::POT), "Potentiometer / Fader");
        assert_eq!(type_name(ty::LED_PWM), "Dimmable LED");
        assert_eq!(noun_name(noun::USER_VOLUME, ty::ENCODER), "Volume");
        assert_eq!(
            noun_name(noun::PSYBASS_CUTOFF, ty::POT),
            "Psych Bass Cutoff Frequency"
        );
        assert_eq!(noun_name(noun::PAGE_VALUE, ty::BUTTON), "Browse/Adjust");
        // Three nouns read differently on an indicator.
        assert_eq!(noun_name(noun::CLIP, ty::BUTTON), "Clear Clipping");
        assert_eq!(noun_name(noun::CLIP, ty::LED), "Clipping");
        assert_eq!(noun_name(noun::MACRO, ty::LED), "Running Macro");
        assert_eq!(noun_name(noun::MACRO, ty::BUTTON), "Macro");
        // Test Signal was renamed (`DSPi_ConsoleApp.swift:7134`).
        assert_eq!(noun_name(noun::SIGGEN, ty::BUTTON), "Signal Generator");
        // And the filter types read as cuts (`DSPi_ConsoleApp.swift:7426-7428`).
        let state = demo::state();
        let data = demo::settings_data();
        let config = crate::settings::AppConfig::default();
        let cx = Cx {
            state: &state,
            data: &data,
            config: &config,
            connected: true,
            global_dirty: false,
        };
        let cs = data.cs.as_ref().expect("demo control surfaces");
        let types: Vec<String> = (0..11)
            .map(|v| enum_value_label(&cx, cs, noun::FILTER_TYPE, v))
            .collect();
        assert_eq!(types[4], "High Cut");
        assert_eq!(types[5], "Low Cut");
        assert!(
            !types
                .iter()
                .any(|t| t.contains("Pass") && !t.starts_with("All"))
        );
        assert_eq!(action_name(act::INC, noun::PRESET, true), "Next");
        assert_eq!(action_name(act::INC, noun::USER_VOLUME, false), "Increase");
        assert_eq!(action_name(act::INC, noun::PAGE_VALUE, true), "Up");
        assert_eq!(
            action_name(act::IND_LEVEL, noun::LEVEL, false),
            "Show level"
        );
        assert_eq!(invert_title(ty::LED), "Active-Low LED");
        assert_eq!(invert_title(ty::ENCODER), "Pull-Down Wiring");
        assert_eq!(invert_title(ty::IR), "Idle-Low Receiver");
        assert_eq!(invert_title(ty::BUTTON), "Active-High Wiring");
        assert!(pin_detail(ty::POT).starts_with("ADC pin (GPIO 26, 27, or 28)"));
    }

    /// The 8.8 units and the plain ones encode differently, and the log units
    /// carry their step in octaves. Getting either wrong is a silent factor of
    /// 256 on the wire.
    #[test]
    fn the_unit_encodings_follow_the_wire() {
        assert_eq!(encode_value(-6.0, unit::DB), -1536);
        assert_eq!(decode_value(-1536, unit::DB), -6.0);
        assert_eq!(encode_value(2000.0, unit::HZ), 2000);
        assert_eq!(decode_value(2000, unit::HZ), 2000.0);
        assert_eq!(encode_value(0.707, unit::Q), 181);
        assert_eq!(encode_value(50.0, unit::PERCENT), 12800);
        assert_eq!(encode_value(1.5, unit::MS), 384);
        // A step in the plain unit is a position count, not 8.8.
        assert_eq!(encode_step(3.0, unit::NONE), 3);
        assert_eq!(encode_step(1.0, unit::DB), 256);
        // Hz and Q step multiplicatively; their step is octaves in 8.8.
        assert!(unit_is_log(unit::HZ) && unit_is_log(unit::Q));
        assert!(!unit_is_log(unit::DB) && !unit_is_log(unit::MS));
        assert_eq!(default_step(unit::HZ), 1.0 / 12.0);
        assert_eq!(default_step(unit::MS), 0.1);
        // Caps v20's MS_LOG: plain ms like Hz, octave steps like Hz
        // (control_surfaces.h:268-269), so 1000 ms fits where 8.8 stops at 127.
        assert_eq!(encode_value(1000.0, unit::MS_LOG), 1000);
        assert_eq!(decode_value(10, unit::MS_LOG), 10.0);
        assert!(unit_is_log(unit::MS_LOG));
        assert_eq!(decode_step(256, unit::MS_LOG), 1.0, "one octave");
        assert_eq!(fmt_unit(200.0, unit::MS_LOG), "200 ms");
        assert_eq!(default_step(unit::DB), 1.0);
        // Saturating, not wrapping.
        assert_eq!(encode_value(1e6, unit::DB), i16::MAX);
    }

    /// The two delay scales are ten times apart (firmware-notes 24): one
    /// second is 10 in a binding and 100 in a macro step.
    #[test]
    fn the_two_delay_scales_stay_apart() {
        assert_eq!(encode_delay(1), 10);
        assert_eq!(encode_step_delay(1.0), 100);
        assert_eq!(decode_delay(600), 60);
        assert_eq!(decode_step_delay(150), 1.5);
        assert_eq!(fmt_delay(encode_delay(90)), "1 min 30 s");
        assert_eq!(fmt_delay(encode_delay(120)), "2 min");
        assert_eq!(fmt_delay(0), "0 s");
        // Both saturate rather than wrap.
        assert_eq!(encode_delay(99_999), 65_530);
        assert_eq!(encode_step_delay(1e9), u16::MAX);
    }

    #[test]
    fn an_unavailable_noun_is_never_offered() {
        let cs = demo::data();
        // ADAT_ACTIVE has a zero action mask in the fixture.
        assert!(!cs.nouns[noun::ADAT_ACTIVE as usize].is_available());
        for t in cs.real_types() {
            assert!(
                !valid_nouns(&cs, t).contains(&noun::ADAT_ACTIVE),
                "type {t} offered an unavailable noun"
            );
        }
    }

    /// A type may only do what both it and the noun allow. A switch can follow
    /// a boolean and nothing else; a pot cannot toggle.
    #[test]
    fn only_the_intersection_of_the_two_action_masks_is_offered() {
        let cs = demo::data();
        assert_eq!(
            valid_actions(&cs, ty::SWITCH, noun::USER_MUTE),
            vec![act::FOLLOW]
        );
        assert!(valid_actions(&cs, ty::POT, noun::USER_MUTE).is_empty());
        assert_eq!(
            valid_actions(&cs, ty::POT, noun::USER_VOLUME),
            vec![act::ADJUST]
        );
        assert_eq!(
            valid_actions(&cs, ty::ENCODER, noun::USER_VOLUME),
            vec![act::STEP]
        );
        // The Console's preferred default for each shape of control.
        assert_eq!(default_action(&cs, ty::POT, noun::USER_VOLUME), act::ADJUST);
        assert_eq!(
            default_action(&cs, ty::ENCODER, noun::USER_VOLUME),
            act::STEP
        );
        assert_eq!(
            default_action(&cs, ty::BUTTON, noun::USER_MUTE),
            act::TOGGLE
        );
        assert_eq!(default_action(&cs, ty::BUTTON, noun::CLIP), act::TRIGGER);
    }

    #[test]
    fn the_parameter_picker_is_grouped_in_the_consoles_order() {
        let cs = demo::data();
        let groups = noun_groups(&cs, ty::BUTTON);
        let names: Vec<&str> = groups.iter().map(|(n, _)| *n).collect();
        assert_eq!(names.first(), Some(&"Volume & Mute"));
        assert!(names.contains(&"Input & Presets"));
        // Every offered noun appears exactly once.
        let flat = noun_choices(&cs, ty::BUTTON);
        let mut seen: BTreeSet<u8> = BTreeSet::new();
        for (_, n) in &flat {
            assert!(seen.insert(*n), "noun {n} listed twice");
        }
        assert_eq!(seen.len(), valid_nouns(&cs, ty::BUTTON).len());
    }

    /// A crossover band only exists on an output, and a group only offers the
    /// bands every member shares.
    #[test]
    fn the_band_list_follows_the_channel_and_the_group() {
        let cs = demo::data();
        let st = demo::state();
        let inputs = st.caps.num_inputs;
        assert_eq!(
            bands_on_channel(&st, noun::FILTER_FREQ, 0).len(),
            10,
            "an input has no crossover"
        );
        assert_eq!(
            bands_on_channel(&st, noun::FILTER_FREQ, inputs).len(),
            14,
            "an output adds the four crossover bands"
        );
        assert_eq!(
            bands_on_channel(&st, noun::FILTER_GAIN, inputs).len(),
            10,
            "only frequency and bypass reach a crossover"
        );
        // A group with an input and an output shares only the PEQ bands.
        let mut cs2 = cs.clone();
        cs2.groups[1] = CsGroup {
            target_kind: target::DSP_CH,
            member_mask: 1 | (1 << inputs),
            name: "Mixed".into(),
        };
        assert_eq!(
            band_options(&cs2, &st, noun::FILTER_FREQ, 1, true).len(),
            10
        );
    }

    /// The three group flags are rejected outright where they do not belong,
    /// so a noun or action change has to clear them.
    #[test]
    fn the_group_flags_are_swept_after_a_noun_change() {
        let cs = demo::data();
        let b = CsBinding {
            component: ty::POT,
            noun: noun::OUTPUT_GAIN,
            action: act::ADJUST,
            flags: flag::GROUP | flag::LINK_ABS,
            target: 0,
            ..Default::default()
        };
        // Still legal: an ADJUST on a continuous noun with a matching group.
        let kept = sanitize_group_flags(&cs, &b);
        assert_eq!(kept.flags & flag::LINK_ABS, flag::LINK_ABS);
        // A noun with no compatible group drops GROUP and its two modifiers.
        let moved = set_noun(&cs, &b, noun::USER_VOLUME);
        assert_eq!(moved.flags & (flag::GROUP | flag::LINK_ABS), 0);
        assert_eq!(moved.target, 0, "the group index is not a channel");
    }

    /// Delays are legal only on an LED following a boolean condition; left
    /// anywhere else the firmware rejects the whole binding.
    #[test]
    fn the_delays_are_cleared_when_they_stop_being_legal() {
        let cs = demo::data();
        assert!(delays_allowed(ty::LED, act::IND_EQUALS));
        assert!(delays_allowed(ty::LED_PWM, act::IND_ABOVE));
        assert!(!delays_allowed(ty::LED_PWM, act::IND_LEVEL));
        assert!(!delays_allowed(ty::BUTTON, act::TOGGLE));
        let b = CsBinding {
            component: ty::LED,
            noun: noun::USER_MUTE,
            action: act::IND_EQUALS,
            on_delay: 50,
            off_delay: 100,
            ..Default::default()
        };
        let kept = default_operands(&cs, &b);
        assert_eq!((kept.on_delay, kept.off_delay), (50, 100));
        let moved = default_operands(
            &cs,
            &CsBinding {
                action: act::IND_LEVEL,
                ..b
            },
        );
        assert_eq!((moved.on_delay, moved.off_delay), (0, 0));
    }

    /// `PAGE_VALUE` resolves its item at event time; the firmware rejects the
    /// binding if any operand is set, and it reads as a one-value enum which
    /// would otherwise pick up a step of 1.
    #[test]
    fn browse_adjust_carries_no_operands() {
        let cs = demo::data();
        let b = default_operands(
            &cs,
            &CsBinding {
                component: ty::ENCODER,
                noun: noun::PAGE_VALUE,
                action: act::STEP,
                ..Default::default()
            },
        );
        assert_eq!((b.value, b.step, b.range_min, b.range_max), (0, 0, 0, 0));
        assert_eq!(operand_kind(&cs, noun::PAGE_VALUE), kind::NONE);
    }

    #[test]
    fn a_macro_step_never_offers_macro_or_browse_adjust() {
        let cs = demo::data();
        let nouns = macro_step_nouns(&cs);
        assert!(!nouns.contains(&noun::MACRO));
        assert!(!nouns.contains(&noun::PAGE_VALUE));
        assert!(nouns.contains(&noun::USER_MUTE));
        // Only the five step actions, and only those the noun accepts.
        for n in &nouns {
            for a in macro_step_actions(&cs, *n) {
                assert!(MACRO_STEP_ACTIONS.contains(&a), "noun {n} action {a}");
            }
        }
    }

    /// A bar plots the value inside the noun's range, so a switch or a mode has
    /// nowhere to draw it; the firmware answers INVALID_PAGE.
    #[test]
    fn a_level_bar_needs_a_range() {
        let cs = demo::data();
        assert!(page_bar_allowed(&cs, noun::USER_VOLUME));
        assert!(!page_bar_allowed(&cs, noun::USER_MUTE));
        assert!(!page_bar_allowed(&cs, noun::PRESET));
        let pages = display_page_nouns(&cs);
        assert!(!pages.contains(&noun::DISPLAY_PAGE));
        assert!(!pages.contains(&noun::PAGE_VALUE));
        assert!(!pages.contains(&noun::DISPLAY_EDIT));
        assert!(pages.contains(&noun::MACRO), "a macro page is allowed");
    }

    #[test]
    fn the_summaries_read_as_sentences() {
        let cs = demo::data();
        let st = demo::state();
        assert_eq!(
            verb_phrase(&st, &cs, &cs.bindings[0]),
            "Turn to step Volume."
        );
        assert_eq!(
            verb_phrase(&st, &cs, &cs.bindings[1]),
            "Press to toggle Mute."
        );
        assert!(verb_phrase(&st, &cs, &cs.bindings[3]).starts_with("Receives commands from an IR"));
        assert!(
            verb_phrase(&st, &cs, &cs.bindings[4])
                .contains("OLED 128x64 (SSD1306) on I2C, address 0x3C")
        );
        assert_eq!(macro_summary(&cs.macros[0], None), "2 steps, 0.50 s total");
        assert_eq!(
            macro_summary(&cs.macros[0], Some(1)),
            "Running - step 2 of 2."
        );
        assert!(group_summary(&st, &cs.groups[0]).contains(','));
    }

    #[test]
    fn a_pot_is_only_ever_offered_an_adc_pin() {
        assert_eq!(ADC_PINS, [26, 27, 28]);
        let cs = demo::data();
        let td = cs.type_desc(ty::POT).unwrap();
        assert_eq!(td.pin_class, dspi_proto::packets::CsTypeDesc::PINCLASS_ADC);
        assert_eq!(cs.type_desc(ty::ENCODER).unwrap().pin_count, 2);
    }
}
