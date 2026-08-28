//! The Test Signals panel: `TestSignalsView.swift` as a tool panel.
//!
//! Unlike every other panel here, the generator's configuration is not in the
//! bulk packet, so there is nothing in `DeviceState` to read it back from: the
//! panel owns the draft, exactly as the Console's view model does, and writes
//! it with `sig.config` whenever it changes. What the device tells us is the
//! run state, which arrives as `SIGGEN_STATE` notifications and lands in
//! `DeviceState::siggen_state`.
//!
//! Parameter ranges, defaults and timing models come from the type catalogue
//! below, which is the Console's own fallback table (`siggenTypeInfos`,
//! `TestSignalsView.swift:69-156`) transcribed from the firmware's `siggen.h`.
//! `SiggenCaps` carries the per-type descriptors on the wire but the probe
//! does not read them, so the caps supply only the counts and masks.

use crossterm::event::{KeyCode, KeyEvent};
use dspi_proto::packets::SiggenConfig;
use dspi_session::DeviceState;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::panel::{self, Body, ChipSpec, Header, Param, Row};
use crate::shell::{Screen, ScreenEvent};
use crate::theme::{ChannelRole, Theme};
use crate::widgets::{Action, ChipState, KeyHelp, StatusTone};

// ------------------------------------------------------------- the catalogue

/// `SIGGEN_TIMING_*`, siggen.h:158-160.
const CONTINUOUS: u8 = 0;
const SWEEP: u8 = 1;
const PATTERN: u8 = 2;

/// `SIGGEN_PARAM_*`, siggen.h:145-151.
const P_UNUSED: u8 = 0;
const P_FREQ: u8 = 1;
const P_MS: u8 = 2;
const P_CYCLES: u8 = 3;
const P_COUNT: u8 = 4;
const P_RATIO: u8 = 5;
const P_PATTERN: u8 = 6;

const ISP: u8 = 13;
const TONE_PAIR: u8 = 11;
const MULTITONE: u8 = 12;
const CHANNEL_ID: u8 = 14;
const WHITE: u8 = 2;
const PINK: u8 = 3;

/// One parameter slot: semantic, minimum, maximum, default.
#[derive(Debug, Clone, Copy)]
struct ParamDesc {
    semantic: u8,
    min: f32,
    max: f32,
    default: f32,
}

const UNUSED: ParamDesc = ParamDesc {
    semantic: P_UNUSED,
    min: 0.0,
    max: 0.0,
    default: 0.0,
};

const fn pd(semantic: u8, min: f32, max: f32, default: f32) -> ParamDesc {
    ParamDesc {
        semantic,
        min,
        max,
        default,
    }
}

/// One signal type as the panel presents it.
#[derive(Debug, Clone, Copy)]
struct TypeInfo {
    id: u8,
    /// The short name under the tile.
    tile: &'static str,
    /// The name the transport line uses.
    display: &'static str,
    blurb: &'static str,
    labels: [&'static str; 4],
    timing: u8,
    params: [ParamDesc; 4],
}

const TYPES: [TypeInfo; 15] = [
    TypeInfo {
        id: 0,
        tile: "Sine",
        display: "Sine",
        blurb: "Pure tone, THD approx -139 dB",
        labels: ["Frequency", "", "", ""],
        timing: CONTINUOUS,
        params: [pd(P_FREQ, 1.0, 30000.0, 1000.0), UNUSED, UNUSED, UNUSED],
    },
    TypeInfo {
        id: 1,
        tile: "Square",
        display: "Square wave",
        blurb: "Band-limited (polyBLEP) square",
        labels: ["Frequency", "", "", ""],
        timing: CONTINUOUS,
        params: [pd(P_FREQ, 1.0, 30000.0, 100.0), UNUSED, UNUSED, UNUSED],
    },
    TypeInfo {
        id: 2,
        tile: "White",
        display: "White noise",
        blurb: "Uniform white noise",
        labels: ["", "", "", ""],
        timing: CONTINUOUS,
        params: [UNUSED, UNUSED, UNUSED, UNUSED],
    },
    TypeInfo {
        id: 3,
        tile: "Pink",
        display: "Pink noise",
        blurb: "-3 dB/oct, level-safe normalized",
        labels: ["", "", "", ""],
        timing: CONTINUOUS,
        params: [UNUSED, UNUSED, UNUSED, UNUSED],
    },
    TypeInfo {
        id: 4,
        tile: "Log Swp",
        display: "Log sweep",
        blurb: "Exponential sweep for room measurement",
        labels: ["Start", "End", "", ""],
        timing: SWEEP,
        params: [
            pd(P_FREQ, 1.0, 30000.0, 20.0),
            pd(P_FREQ, 1.0, 30000.0, 20000.0),
            UNUSED,
            UNUSED,
        ],
    },
    TypeInfo {
        id: 5,
        tile: "Lin Swp",
        display: "Linear sweep",
        blurb: "Linear frequency sweep",
        labels: ["Start", "End", "", ""],
        timing: SWEEP,
        params: [
            pd(P_FREQ, 1.0, 30000.0, 20.0),
            pd(P_FREQ, 1.0, 30000.0, 20000.0),
            UNUSED,
            UNUSED,
        ],
    },
    TypeInfo {
        id: 6,
        tile: "Step Swp",
        display: "Stepped sweep",
        blurb: "Discrete tones stepping up the band",
        labels: ["Start", "End", "Steps/octave", "Dwell"],
        timing: SWEEP,
        params: [
            pd(P_FREQ, 1.0, 30000.0, 20.0),
            pd(P_FREQ, 1.0, 30000.0, 20000.0),
            pd(P_COUNT, 1.0, 24.0, 3.0),
            pd(P_MS, 20.0, 10000.0, 250.0),
        ],
    },
    TypeInfo {
        id: 7,
        tile: "Impulse",
        display: "Impulse",
        blurb: "Single-sample unit impulses",
        labels: ["Period", "", "", ""],
        timing: PATTERN,
        params: [pd(P_MS, 10.0, 60000.0, 500.0), UNUSED, UNUSED, UNUSED],
    },
    TypeInfo {
        id: 8,
        tile: "Clicks",
        display: "Alternating clicks",
        blurb: "Clicks with alternating polarity",
        labels: ["Period", "", "", ""],
        timing: PATTERN,
        params: [pd(P_MS, 10.0, 60000.0, 500.0), UNUSED, UNUSED, UNUSED],
    },
    TypeInfo {
        id: 9,
        tile: "Polarity",
        display: "Polarity pulse",
        blurb: "Positive half-sine lobe per period",
        labels: ["Pulse width", "Period", "", ""],
        timing: PATTERN,
        params: [
            pd(P_MS, 1.0, 100.0, 5.0),
            pd(P_MS, 10.0, 60000.0, 500.0),
            UNUSED,
            UNUSED,
        ],
    },
    TypeInfo {
        id: 10,
        tile: "Burst",
        display: "Tone burst",
        blurb: "Sine bursts with raised-cosine edges",
        labels: ["Frequency", "On cycles", "Off cycles", "Edge cycles"],
        timing: PATTERN,
        params: [
            pd(P_FREQ, 1.0, 30000.0, 1000.0),
            pd(P_CYCLES, 1.0, 1000.0, 8.0),
            pd(P_CYCLES, 0.0, 1000.0, 8.0),
            pd(P_CYCLES, 0.0, 100.0, 2.0),
        ],
    },
    TypeInfo {
        id: 11,
        tile: "2-Tone",
        display: "Tone pair",
        blurb: "IMD test pair (SMPTE / CCIF)",
        labels: ["Tone 1", "Tone 2", "Ratio A1/A2", ""],
        timing: CONTINUOUS,
        params: [
            pd(P_FREQ, 1.0, 30000.0, 60.0),
            pd(P_FREQ, 1.0, 30000.0, 7000.0),
            pd(P_RATIO, 0.1, 10.0, 4.0),
            UNUSED,
        ],
    },
    TypeInfo {
        id: 12,
        tile: "Multi",
        display: "Multitone",
        blurb: "Log-spaced tones, Schroeder phases",
        labels: ["Tones", "Low", "High", ""],
        timing: CONTINUOUS,
        params: [
            pd(P_COUNT, 2.0, 16.0, 10.0),
            pd(P_FREQ, 1.0, 30000.0, 20.0),
            pd(P_FREQ, 1.0, 30000.0, 20000.0),
            UNUSED,
        ],
    },
    TypeInfo {
        id: 13,
        tile: "ISP",
        display: "ISP test",
        blurb: "Inter-sample-peak over patterns",
        labels: ["Pattern", "", "", ""],
        timing: CONTINUOUS,
        params: [pd(P_PATTERN, 0.0, 1.0, 0.0), UNUSED, UNUSED, UNUSED],
    },
    TypeInfo {
        id: 14,
        tile: "Chan ID",
        display: "Channel ID",
        blurb: "Counted pentatonic blips per channel",
        labels: ["Blip length", "", "", ""],
        timing: PATTERN,
        params: [pd(P_MS, 30.0, 1000.0, 120.0), UNUSED, UNUSED, UNUSED],
    },
];

fn type_info(id: u8) -> &'static TypeInfo {
    TYPES.iter().find(|t| t.id == id).unwrap_or(&TYPES[0])
}

/// The unit a parameter's semantic carries, `unitLabel`.
fn unit_of(semantic: u8) -> &'static str {
    match semantic {
        P_FREQ => "Hz",
        P_MS => "ms",
        P_CYCLES => "cyc",
        P_RATIO => "x",
        _ => "",
    }
}

const KEYS: &[KeyHelp] = &[
    KeyHelp::new("↑ ↓", "Move"),
    KeyHelp::new("← →", "Adjust"),
    KeyHelp::new("Enter", "Edit or choose"),
    KeyHelp::new("Space", "Start or stop"),
    KeyHelp::new("S", "Stop now"),
    KeyHelp::new("a", "All outputs"),
    KeyHelp::new("n", "No outputs"),
    KeyHelp::new("Backspace", "Reset"),
];

pub struct SignalsPanel {
    body: Body,
    /// The staged configuration. The generator's config is not in the bulk
    /// packet and there is no scalar readback for it, so the panel keeps it
    /// and writes it whole.
    pub draft: SiggenConfig,
    chip: usize,
    tile: usize,
    transport: usize,
    /// True once the draft has been seeded from the device's capabilities.
    seeded: bool,
}

impl Default for SignalsPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl SignalsPanel {
    pub fn new() -> Self {
        Self {
            // No master switch: the header carries the run state instead, so
            // focus starts on the first row.
            body: Body::headless(),
            draft: SiggenConfig {
                signal_type: 0,
                p1: TYPES[0].params[0].default,
                // The Console starts from whatever the device has staged; we
                // cannot read that back, so the panel opens at a measurement
                // level rather than at full scale.
                level_db: -20.0,
                ..SiggenConfig::default()
            },
            chip: 0,
            tile: 0,
            transport: 0,
            seeded: false,
        }
    }

    /// Fill in the parts of the draft that only the device can supply: which
    /// outputs exist, and therefore which ones a fresh draft plays on.
    fn seed(&mut self, state: &DeviceState) {
        if self.seeded {
            return;
        }
        self.seeded = true;
        self.draft.channel_mask = Self::valid_mask(state);
    }

    pub fn supported(state: &DeviceState) -> bool {
        panel::has_feature(state, "test_signals")
    }

    /// The outputs the generator will accept, from its own caps when the probe
    /// read them and from the channel count otherwise.
    fn valid_mask(state: &DeviceState) -> u16 {
        match &state.caps.siggen {
            Some(c) if c.valid_channel_mask != 0 => c.valid_channel_mask,
            _ => panel::all_outputs_mask(state),
        }
    }

    fn outputs(state: &DeviceState) -> usize {
        match &state.caps.siggen {
            Some(c) if c.output_channels > 0 => c.output_channels as usize,
            _ => state.caps.num_outputs as usize,
        }
    }

    fn info(&self) -> &'static TypeInfo {
        type_info(self.draft.signal_type)
    }

    /// `SIGGEN_STATE_*` as the Console labels it, plus the colour it uses.
    fn run_state(state: &DeviceState) -> (u8, &'static str, StatusTone) {
        let s = state.siggen_state.map(|(s, ..)| s).unwrap_or(0);
        match s {
            1 => (s, "Fading in", StatusTone::Ok),
            2 => (s, "Running", StatusTone::Ok),
            3 => (s, "Gap", StatusTone::Warning),
            4 => (s, "Fading out", StatusTone::Warning),
            _ => (s, "Idle", StatusTone::Neutral),
        }
    }

    fn running(state: &DeviceState) -> bool {
        Self::run_state(state).0 != 0
    }

    /// Why Start is refused, in the Console's words.
    fn blocker(&self, state: &DeviceState) -> Option<&'static str> {
        if !Self::supported(state) {
            return Some("Firmware has no signal generator");
        }
        if self.draft.channel_mask == 0 {
            return Some("Select at least one output");
        }
        if self.info().timing == SWEEP && self.draft.duration_ms == 0 {
            return Some("Sweep length must be greater than 0");
        }
        None
    }

    fn walking(&self) -> bool {
        self.draft.flags & SiggenConfig::FLAG_WALK != 0 || self.draft.signal_type == CHANNEL_ID
    }

    fn param(&self, i: usize) -> f32 {
        match i {
            0 => self.draft.p1,
            1 => self.draft.p2,
            2 => self.draft.p3,
            _ => self.draft.p4,
        }
    }

    fn set_param(&mut self, i: usize, v: f32) {
        let d = self.info().params[i];
        let v = v.clamp(d.min, if d.max == 0.0 { v } else { d.max });
        let v = if matches!(d.semantic, P_COUNT | P_CYCLES | P_PATTERN) {
            v.round()
        } else {
            v
        };
        match i {
            0 => self.draft.p1 = v,
            1 => self.draft.p2 = v,
            2 => self.draft.p3 = v,
            _ => self.draft.p4 = v,
        }
    }

    /// Switch signal type, taking the new type's defaults with it, as
    /// `selectType` does.
    fn select_type(&mut self, id: u8) {
        let t = type_info(id);
        self.draft.signal_type = id;
        self.draft.p1 = t.params[0].default;
        self.draft.p2 = t.params[1].default;
        self.draft.p3 = t.params[2].default;
        self.draft.p4 = t.params[3].default;
        self.draft.repeat_count = 0;
        self.draft.gap_ms = 0;
        self.draft.duration_ms = if t.timing == SWEEP { 5000 } else { 0 };
    }

    /// The whole draft as one `sig.config` line.
    pub fn config_command(&self) -> String {
        let f = |v: f32| super::number(v);
        let mut parts = vec![
            format!(
                "sig.config type={}",
                dspi_proto::packets::SIGGEN_TYPES
                    .iter()
                    .find(|(_, id)| *id == self.draft.signal_type)
                    .map(|(n, _)| *n)
                    .unwrap_or("sine")
            ),
            format!("channels=0x{:X}", self.draft.channel_mask),
            format!("invert=0x{:X}", self.draft.invert_mask),
            format!("level={}", f(self.draft.level_db)),
            format!("duration={}", self.draft.duration_ms),
            format!("repeat={}", self.draft.repeat_count),
            format!("gap={}", self.draft.gap_ms),
        ];
        if self.draft.flags != 0 {
            let mut names = Vec::new();
            for (name, bit) in [
                ("raw", SiggenConfig::FLAG_RAW),
                ("decorr", SiggenConfig::FLAG_DECORR),
                ("walk", SiggenConfig::FLAG_WALK),
            ] {
                if self.draft.flags & bit != 0 {
                    names.push(name);
                }
            }
            parts.push(format!("flags={}", names.join(",")));
        }
        for (i, name) in ["p1", "p2", "p3", "p4"].iter().enumerate() {
            if self.info().params[i].semantic != P_UNUSED {
                parts.push(format!("{name}={}", f(self.param(i))));
            }
        }
        parts.join(" ")
    }

    /// An edit stages the configuration; while the generator is running the
    /// firmware restarts with a fade, which is the Console's live re-apply.
    fn staged(&self) -> ScreenEvent {
        ScreenEvent::Command(self.config_command())
    }

    fn timing_rows(&self, rows: &mut Vec<Row>) {
        let seconds = self.draft.duration_ms as f64 / 1000.0;
        let repeat = |label: &str, caption: &str| {
            Row::Param(
                Param::new(label, self.draft.repeat_count as f64, 0.0, 65535.0, "")
                    .step(1.0)
                    .decimals(0)
                    .caption(caption),
            )
        };
        let gap = |label: &str| {
            Row::Param(
                Param::new(label, self.draft.gap_ms as f64, 0.0, 65535.0, "ms")
                    .step(50.0)
                    .decimals(0),
            )
        };
        rows.push(Row::Section {
            title: "Timing".into(),
            action: None,
        });
        match self.info().timing {
            SWEEP => {
                rows.push(Row::Param(
                    Param::new("Sweep length", seconds, 0.01, 600.0, "s")
                        .step(0.5)
                        .decimals(2),
                ));
                rows.push(repeat("Repeat", "0 = repeat forever"));
                rows.push(gap("Gap between sweeps"));
            }
            PATTERN => {
                rows.push(repeat(
                    "Repeat",
                    if self.draft.signal_type == CHANNEL_ID {
                        "Passes over the selected outputs. 0 = forever"
                    } else {
                        "Pattern periods. 0 = repeat forever"
                    },
                ));
                rows.push(gap("Extra gap per period"));
            }
            _ if self.walking() => {
                rows.push(Row::Param(
                    Param::new("Dwell per channel", seconds, 0.0, 600.0, "s")
                        .step(0.5)
                        .decimals(2)
                        .caption("0 = 2 s default"),
                ));
                rows.push(repeat(
                    "Passes",
                    "Full passes over the outputs. 0 = forever",
                ));
            }
            _ => {
                rows.push(Row::Param(
                    Param::new("Duration", seconds, 0.0, 600.0, "s")
                        .step(0.5)
                        .decimals(2)
                        .caption("0 = play until stopped"),
                ));
            }
        }
    }

    fn rows(&self, state: &DeviceState, theme: &Theme) -> Vec<Row> {
        let info = self.info();
        if !Self::supported(state) {
            return vec![
                Row::Banner {
                    title: "Signal generator not available".into(),
                    body: "The connected firmware does not include the onboard test signal \
                           generator. Update the DSPi firmware to use this tool."
                        .into(),
                },
                Row::Blank,
                self.transport_row(state),
            ];
        }

        let mut rows = vec![
            Row::Section {
                title: "Signal".into(),
                action: None,
            },
            Row::Tiles {
                labels: TYPES.iter().map(|t| t.tile.to_string()).collect(),
                selected: TYPES
                    .iter()
                    .position(|t| t.id == self.draft.signal_type)
                    .unwrap_or(0),
                cursor: self.tile,
                columns: 4,
            },
            Row::Caption(info.blurb.into()),
            Row::Blank,
            Row::Section {
                title: "Outputs".into(),
                action: Some("a All   n None".into()),
            },
            Row::Caption(
                "Space selects, Space again inverts polarity (ø). Dimmed outputs are disabled \
                 in the matrix mixer and stay silent."
                    .into(),
            ),
        ];
        let chips = (0..Self::outputs(state))
            .map(|o| {
                let bit = 1u16 << o;
                ChipSpec {
                    label: super::channel_name(state, state.caps.num_inputs as usize + o),
                    state: if self.draft.channel_mask & bit == 0 {
                        ChipState::Off
                    } else if self.draft.invert_mask & bit != 0 {
                        ChipState::Inverted
                    } else {
                        ChipState::On
                    },
                    color: theme.role_color(ChannelRole::of(
                        state.caps.num_inputs + o as u8,
                        state.caps.num_inputs,
                        state.caps.num_outputs,
                    )),
                    // A disabled output is still selectable, as it is in the
                    // Console; it just stays silent, which the caption says.
                    enabled: true,
                }
            })
            .collect();
        rows.push(Row::Chips {
            chips,
            cursor: self.chip,
            polarity: true,
        });

        rows.push(Row::Blank);
        rows.push(Row::Section {
            title: "Level".into(),
            action: None,
        });
        rows.push(Row::Param(
            Param::new("Level", self.draft.level_db as f64, -80.0, 0.0, "dBFS")
                .step(1.0)
                .decimals(1)
                .caption(
                    "Peak level in dBFS. Output trim, master volume and mute still apply \
                     downstream.",
                ),
        ));

        let used: Vec<usize> = (0..4)
            .filter(|i| info.params[*i].semantic != P_UNUSED)
            .collect();
        if !used.is_empty() {
            rows.push(Row::Blank);
            rows.push(Row::Section {
                title: "Parameters".into(),
                action: None,
            });
            if self.draft.signal_type == ISP {
                rows.push(Row::Segmented {
                    label: "Pattern".into(),
                    choices: vec!["fs/4 - +3.01 dBTP".into(), "fs/6 - +1.25 dBTP".into()],
                    selected: (self.draft.p1.round() as usize).min(1),
                    enabled: true,
                });
            } else {
                for i in used {
                    let d = info.params[i];
                    let whole = matches!(d.semantic, P_COUNT | P_CYCLES);
                    rows.push(Row::Param(
                        Param::new(
                            info.labels[i],
                            self.param(i) as f64,
                            d.min as f64,
                            d.max as f64,
                            unit_of(d.semantic),
                        )
                        .step(if d.semantic == P_RATIO { 0.1 } else { 1.0 })
                        .decimals(if whole {
                            0
                        } else if d.semantic == P_RATIO {
                            2
                        } else {
                            1
                        }),
                    ));
                }
            }
            if self.draft.signal_type == TONE_PAIR {
                rows.push(Row::Buttons {
                    label: "Presets:".into(),
                    buttons: vec!["SMPTE 60/7k".into(), "CCIF 19k/20k".into()],
                    cursor: self.transport,
                });
            }
            if self.draft.signal_type == MULTITONE
                && let Some(c) = &state.caps.siggen
                && c.multitone_max > 0
            {
                rows.push(Row::Caption(format!(
                    "Up to {} tones on this device.",
                    c.multitone_max
                )));
            }
        }

        rows.push(Row::Blank);
        self.timing_rows(&mut rows);

        rows.push(Row::Blank);
        rows.push(Row::Section {
            title: "Options".into(),
            action: None,
        });
        rows.push(Row::Toggle {
            label: "Bypass output EQ (RAW)".into(),
            on: self.draft.flags & SiggenConfig::FLAG_RAW != 0,
            caption: Some(
                "Skips crossover and PEQ on the selected outputs. Trim, master volume, mute \
                 and delay still apply."
                    .into(),
            ),
            enabled: true,
        });
        if matches!(self.draft.signal_type, WHITE | PINK) {
            rows.push(Row::Toggle {
                label: "Decorrelate channels".into(),
                on: self.draft.flags & SiggenConfig::FLAG_DECORR != 0,
                caption: Some("Independent noise per output instead of one copied signal.".into()),
                enabled: true,
            });
        }
        if self.draft.signal_type == CHANNEL_ID {
            rows.push(Row::Toggle {
                label: "Walk outputs one at a time".into(),
                on: true,
                caption: Some(
                    "Channel ID always walks: each output plays its channel number as counted \
                     blips."
                        .into(),
                ),
                enabled: false,
            });
        } else {
            rows.push(Row::Toggle {
                label: "Walk outputs one at a time".into(),
                on: self.draft.flags & SiggenConfig::FLAG_WALK != 0,
                caption: Some(
                    "Plays the selected outputs sequentially instead of together.".into(),
                ),
                enabled: true,
            });
        }

        rows.push(Row::Blank);
        rows.push(self.transport_row(state));
        rows
    }

    fn transport_row(&self, state: &DeviceState) -> Row {
        let (_, label, _) = Self::run_state(state);
        let running = Self::running(state);
        let blocker = self.blocker(state);
        let title = if running {
            let active = state
                .siggen_state
                .map(|(_, _, t, _)| type_info(t))
                .unwrap_or(self.info());
            format!("{label} · {}", active.display)
        } else if let Some(b) = blocker {
            b.to_string()
        } else {
            format!("Ready · {}", self.info().display)
        };
        let detail = if running {
            let mut parts = Vec::new();
            if let Some((_, _, _, channel)) = state.siggen_state
                && channel != 0xFF
            {
                parts.push(super::channel_name(
                    state,
                    state.caps.num_inputs as usize + channel as usize,
                ));
            }
            parts.push("edits apply live".into());
            parts.join(" · ")
        } else {
            match state.siggen_state.map(|(_, r, _, _)| r).unwrap_or(0) {
                1 => "Stopped".into(),
                2 => "Finished".into(),
                3 => "Stopped by preset load".into(),
                _ => {
                    let n = self.draft.channel_mask.count_ones();
                    format!(
                        "{n} output{} · peak {:.1} dBFS",
                        if n == 1 { "" } else { "s" },
                        self.draft.level_db
                    )
                }
            }
        };
        Row::Transport {
            title,
            detail,
            running,
            enabled: blocker.is_none(),
            cursor: self.transport.min(if running { 1 } else { 0 }),
        }
    }

    /// Start, stop, or say why Start is refused.
    fn transport(&mut self, state: &DeviceState, stop_now: bool) -> ScreenEvent {
        if stop_now {
            return ScreenEvent::Command("sig.control stop-now".into());
        }
        if Self::running(state) {
            return ScreenEvent::Command("sig.control stop".into());
        }
        match self.blocker(state) {
            Some(reason) => ScreenEvent::Status(reason.into()),
            // Stage the configuration and start it in one gesture, which is
            // what `siggenStart(with:)` does.
            None => ScreenEvent::Command(format!("{}\nsig.control start", self.config_command())),
        }
    }

    fn act(
        &mut self,
        rows: &[Row],
        index: usize,
        action: Action,
        state: &DeviceState,
    ) -> ScreenEvent {
        match (&rows[index], action) {
            (Row::Tiles { .. }, Action::Selected(i)) => {
                self.tile = i.min(TYPES.len() - 1);
                ScreenEvent::Handled
            }
            (Row::Tiles { .. }, Action::Button(i)) => {
                self.select_type(TYPES[i.min(TYPES.len() - 1)].id);
                self.staged()
            }
            (Row::Chips { .. }, Action::Selected(i)) => {
                self.chip = i;
                ScreenEvent::Handled
            }
            (Row::Chips { .. }, Action::Chip(i, next)) => {
                let bit = 1u16 << i;
                match next {
                    ChipState::On => {
                        self.draft.channel_mask |= bit;
                        self.draft.invert_mask &= !bit;
                    }
                    ChipState::Inverted => self.draft.invert_mask |= bit,
                    ChipState::Off => {
                        self.draft.channel_mask &= !bit;
                        self.draft.invert_mask &= !bit;
                    }
                }
                self.staged()
            }
            (Row::Segmented { .. }, Action::Selected(i)) => {
                self.draft.p1 = i.min(1) as f32;
                self.staged()
            }
            (Row::Buttons { .. }, Action::Selected(i)) => {
                self.transport = i;
                ScreenEvent::Handled
            }
            (Row::Buttons { .. }, Action::Button(i)) => {
                // `tonePairPresets`: SMPTE and CCIF.
                let (p1, p2, p3) = if i == 0 {
                    (60.0, 7000.0, 4.0)
                } else {
                    (19000.0, 20000.0, 1.0)
                };
                self.draft.p1 = p1;
                self.draft.p2 = p2;
                self.draft.p3 = p3;
                self.staged()
            }
            (Row::Toggle { label, .. }, Action::Toggled(on)) => {
                let bit = match label.as_str() {
                    "Bypass output EQ (RAW)" => SiggenConfig::FLAG_RAW,
                    "Decorrelate channels" => SiggenConfig::FLAG_DECORR,
                    _ => SiggenConfig::FLAG_WALK,
                };
                if on {
                    self.draft.flags |= bit;
                } else {
                    self.draft.flags &= !bit;
                }
                self.staged()
            }
            (Row::Transport { .. }, Action::Selected(i)) => {
                self.transport = i;
                ScreenEvent::Handled
            }
            (Row::Transport { running, .. }, Action::Button(i)) => {
                self.transport(state, *running && i == 1)
            }
            (Row::Param(p), Action::Changed(v)) => self.set_field(&p.label, v),
            (Row::Param(p), Action::Reset) => {
                let v = self.default_for(&p.label);
                self.set_field(&p.label, v)
            }
            _ => ScreenEvent::Handled,
        }
    }

    /// The default a Backspace puts back, from the type descriptor.
    fn default_for(&self, label: &str) -> f64 {
        match label {
            "Level" => -20.0,
            "Repeat" | "Passes" => 0.0,
            "Gap between sweeps" | "Extra gap per period" => 0.0,
            "Sweep length" => 5.0,
            "Duration" | "Dwell per channel" => 0.0,
            _ => self
                .info()
                .labels
                .iter()
                .position(|l| *l == label)
                .map(|i| self.info().params[i].default as f64)
                .unwrap_or(0.0),
        }
    }

    /// Put one edited value back into the draft, by the row's label.
    fn set_field(&mut self, label: &str, v: f64) -> ScreenEvent {
        match label {
            "Level" => self.draft.level_db = v.clamp(-80.0, 0.0) as f32,
            "Repeat" | "Passes" => self.draft.repeat_count = v.clamp(0.0, 65535.0) as u16,
            "Gap between sweeps" | "Extra gap per period" => {
                self.draft.gap_ms = v.clamp(0.0, 65535.0) as u16
            }
            "Sweep length" => self.draft.duration_ms = (v.max(0.01) * 1000.0) as u32,
            "Duration" | "Dwell per channel" => {
                self.draft.duration_ms = (v.max(0.0) * 1000.0) as u32
            }
            other => {
                if let Some(i) = self.info().labels.iter().position(|l| *l == other) {
                    self.set_param(i, v as f32);
                }
            }
        }
        self.staged()
    }
}

impl Screen for SignalsPanel {
    fn title(&self) -> String {
        "Test Signals".into()
    }

    fn draw(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        state: &DeviceState,
        focused: bool,
    ) {
        self.seed(state);
        let rows = self.rows(state, theme);
        self.body.clamp(&rows);
        let (_, label, tone) = Self::run_state(state);
        let header =
            Header::new("Test Signals", "Onboard measurement signal generator").pill(label, tone);
        self.body.draw(area, buf, theme, &header, &rows, focused);
    }

    fn handle(&mut self, key: KeyEvent, state: &DeviceState) -> ScreenEvent {
        self.seed(state);
        // The transport keys work from anywhere in the panel, as the Console's
        // window-wide Space shortcut does. Every row here is fully operable
        // with Enter and the arrows, so Space is free to mean Start / Stop.
        match key.code {
            KeyCode::Char(' ') => return self.transport(state, false),
            KeyCode::Char('S') => return self.transport(state, true),
            _ => {}
        }
        if !Self::supported(state) {
            return ScreenEvent::Unhandled;
        }
        match key.code {
            KeyCode::Char('a') => {
                self.draft.channel_mask = Self::valid_mask(state);
                return self.staged();
            }
            KeyCode::Char('n') => {
                self.draft.channel_mask = 0;
                self.draft.invert_mask = 0;
                return self.staged();
            }
            _ => {}
        }
        let rows = self.rows(state, panel::key_theme());
        self.body.clamp(&rows);
        match panel::dispatch(&mut self.body, &rows, key) {
            panel::Step::Done(ev) => ev,
            // Headless: the header carries a state pill, not a switch.
            panel::Step::Header => ScreenEvent::Unhandled,
            panel::Step::Commit(i, v) => match &rows[i] {
                Row::Param(p) => {
                    let label = p.label.clone();
                    let v = v.clamp(p.min, p.max);
                    self.set_field(&label, v)
                }
                _ => ScreenEvent::Handled,
            },
            panel::Step::Row(i, action) => self.act(&rows, i, action, state),
        }
    }

    fn keys(&self) -> &'static [KeyHelp] {
        KEYS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::panel::testing;
    use crate::shell::Tool;
    use crate::widgets::testing::{key, shift};

    fn panel() -> (SignalsPanel, DeviceState) {
        let state = testing::state();
        let mut p = SignalsPanel::new();
        p.seed(&state);
        (p, state)
    }

    fn focus_of(p: &SignalsPanel, state: &DeviceState, want: fn(&Row) -> bool) -> usize {
        let rows = p.rows(state, panel::key_theme());
        panel::focus_rows(&rows)
            .iter()
            .position(|r| want(&rows[*r]))
            .expect("a row of that kind")
            + 1
    }

    #[test]
    fn the_panel_carries_the_grid_and_every_section() {
        let (mut p, state) = panel();
        let f = testing::draw(&mut p, &state, 100, 60);
        assert!(
            f.contains("Test Signals · Onboard measurement signal generator"),
            "{f}"
        );
        assert!(f.contains("Idle"), "the state pill: {f}");
        assert!(f.contains("SIGNAL"), "{f}");
        for tile in ["Sine", "Pink", "Log Swp", "2-Tone", "Chan ID"] {
            assert!(f.contains(tile), "the {tile} tile is missing:\n{f}");
        }
        assert!(
            f.contains("Pure tone, THD approx -139 dB"),
            "the blurb: {f}"
        );
        assert!(f.contains("OUTPUTS") && f.contains("a All"), "{f}");
        assert!(f.contains("LEVEL") && f.contains("-20.0 dBFS"), "{f}");
        assert!(f.contains("PARAMETERS") && f.contains("Frequency"), "{f}");
        assert!(f.contains("TIMING") && f.contains("Duration"), "{f}");
        assert!(
            f.contains("OPTIONS") && f.contains("Bypass output EQ (RAW)"),
            "{f}"
        );
        assert!(f.contains("Ready · Sine"), "the transport: {f}");
    }

    #[test]
    fn the_tiles_are_a_four_column_grid_of_every_type() {
        let (p, state) = panel();
        let rows = p.rows(&state, panel::key_theme());
        match rows.iter().find(|r| matches!(r, Row::Tiles { .. })) {
            Some(Row::Tiles {
                labels, columns, ..
            }) => {
                assert_eq!(labels.len(), 15, "siggen.h has 15 types");
                assert_eq!(*columns, 4);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn golden_frames_at_both_sizes() {
        let (_, state) = panel();
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            let f = testing::frame(Tool::Signals, Box::new(SignalsPanel::new()), &state, w, h);
            assert_eq!(f.lines().count(), h as usize, "{w}x{h}");
            assert!(f.contains("Test Signals"), "{w}x{h}:\n{f}");
            assert!(f.contains("G closes"), "{w}x{h}:\n{f}");
        }
    }

    #[test]
    fn an_unsupported_firmware_gets_the_notice_and_a_blocked_transport() {
        let state = crate::shell::fixture::state();
        let mut p = SignalsPanel::new();
        let f = testing::draw(&mut p, &state, 100, 40);
        assert!(f.contains("Signal generator not available"), "{f}");
        assert!(
            f.contains("does not include the onboard test signal"),
            "{f}"
        );
        assert!(!f.contains("SIGNAL"), "the body is gone: {f}");
        assert!(f.contains("Firmware has no signal generator"), "{f}");
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &state),
            ScreenEvent::Status("Firmware has no signal generator".into())
        );
    }

    #[test]
    fn choosing_a_type_takes_its_defaults_and_its_timing_model() {
        let (mut p, state) = panel();
        p.body.focus = focus_of(&p, &state, |r| matches!(r, Row::Tiles { .. }));
        // Down moves inside the grid rather than off it.
        p.handle(key(KeyCode::Down), &state);
        assert_eq!(p.tile, 4, "one row down in a four-wide grid");
        match p.handle(key(KeyCode::Enter), &state) {
            ScreenEvent::Command(c) => {
                assert!(c.starts_with("sig.config type=sweep-log"), "{c}");
                assert!(c.contains("duration=5000"), "a sweep gets 5 s: {c}");
                assert!(c.contains("p1=20") && c.contains("p2=20000"), "{c}");
            }
            other => panic!("{other:?}"),
        }
        let f = testing::draw(&mut p, &state, 100, 60);
        assert!(f.contains("Sweep length"), "the sweep timing block: {f}");
        assert!(f.contains("Gap between sweeps"), "{f}");
        assert!(f.contains("Exponential sweep for room measurement"), "{f}");
    }

    #[test]
    fn an_output_chip_cycles_off_on_and_inverted() {
        let (mut p, state) = panel();
        p.body.focus = focus_of(&p, &state, |r| matches!(r, Row::Chips { .. }));
        // Every output starts selected, so the first press inverts it.
        p.handle(key(KeyCode::Enter), &state);
        assert_eq!(p.draft.invert_mask, 0x001);
        p.handle(key(KeyCode::Enter), &state);
        assert_eq!(p.draft.invert_mask, 0);
        assert_eq!(p.draft.channel_mask, 0x1FE, "and now it is off");
        let f = testing::draw(&mut p, &state, 100, 60);
        assert!(f.contains("8 outputs"), "the transport detail: {f}");
    }

    #[test]
    fn all_and_none_are_the_consoles_two_buttons() {
        let (mut p, state) = panel();
        assert!(matches!(
            p.handle(key(KeyCode::Char('n')), &state),
            ScreenEvent::Command(_)
        ));
        assert_eq!(p.draft.channel_mask, 0);
        // With nothing selected, Start says why it will not.
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &state),
            ScreenEvent::Status("Select at least one output".into())
        );
        p.handle(key(KeyCode::Char('a')), &state);
        assert_eq!(p.draft.channel_mask, 0x1FF);
    }

    #[test]
    fn space_starts_and_stops_and_shift_s_stops_immediately() {
        let (mut p, mut state) = panel();
        match p.handle(key(KeyCode::Char(' ')), &state) {
            ScreenEvent::Command(c) => {
                assert!(c.starts_with("sig.config type=sine"), "{c}");
                assert!(c.ends_with("sig.control start"), "{c}");
            }
            other => panic!("{other:?}"),
        }
        state.siggen_state = Some((2, 0, 0, 0xFF));
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &state),
            ScreenEvent::Command("sig.control stop".into())
        );
        assert_eq!(
            p.handle(shift(KeyCode::Char('S')), &state),
            ScreenEvent::Command("sig.control stop-now".into())
        );
    }

    #[test]
    fn a_running_generator_shows_its_state_line() {
        let (mut p, mut state) = panel();
        state.siggen_state = Some((2, 0, 4, 2));
        let f = testing::draw(&mut p, &state, 100, 60);
        assert!(f.contains("Running"), "the pill: {f}");
        assert!(f.contains("Running · Log sweep"), "{f}");
        assert!(f.contains("edits apply live"), "{f}");
        assert!(f.contains("OUT 3"), "the walked output: {f}");
        // A stop reason is shown once it is idle again.
        state.siggen_state = Some((0, 3, 4, 0xFF));
        let f = testing::draw(&mut p, &state, 100, 60);
        assert!(f.contains("Stopped by preset load"), "{f}");
    }

    #[test]
    fn a_sweep_with_no_length_cannot_start() {
        let (mut p, state) = panel();
        p.select_type(4);
        p.draft.duration_ms = 0;
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &state),
            ScreenEvent::Status("Sweep length must be greater than 0".into())
        );
    }

    #[test]
    fn the_isp_type_gets_a_segmented_picker_and_the_tone_pair_gets_presets() {
        let (mut p, state) = panel();
        p.select_type(13);
        let f = testing::draw(&mut p, &state, 100, 60);
        assert!(f.contains("fs/4 - +3.01 dBTP"), "{f}");
        assert!(f.contains("fs/6 - +1.25 dBTP"), "{f}");
        p.body.focus = focus_of(&p, &state, |r| matches!(r, Row::Segmented { .. }));
        p.handle(key(KeyCode::Right), &state);
        assert_eq!(p.draft.p1, 1.0);

        p.select_type(11);
        let f = testing::draw(&mut p, &state, 100, 60);
        assert!(
            f.contains("SMPTE 60/7k") && f.contains("CCIF 19k/20k"),
            "{f}"
        );
        p.body.focus = focus_of(&p, &state, |r| matches!(r, Row::Buttons { .. }));
        p.handle(key(KeyCode::Right), &state);
        p.handle(key(KeyCode::Enter), &state);
        assert_eq!(
            (p.draft.p1, p.draft.p2, p.draft.p3),
            (19000.0, 20000.0, 1.0)
        );
    }

    #[test]
    fn channel_id_forces_the_walk_option_on() {
        let (mut p, state) = panel();
        p.select_type(14);
        let f = testing::draw(&mut p, &state, 100, 60);
        assert!(f.contains("Channel ID always walks"), "{f}");
        assert!(p.walking(), "the timing model follows the forced flag");
        assert!(
            f.contains("Passes over the selected outputs. 0 = forever"),
            "{f}"
        );
    }

    #[test]
    fn walking_a_continuous_signal_swaps_the_timing_block() {
        let (mut p, state) = panel();
        let f = testing::draw(&mut p, &state, 100, 60);
        assert!(f.contains("Duration"), "{f}");
        p.draft.flags |= SiggenConfig::FLAG_WALK;
        let f = testing::draw(&mut p, &state, 100, 60);
        assert!(f.contains("Dwell per channel"), "{f}");
        assert!(f.contains("0 = 2 s default"), "{f}");
        assert!(f.contains("Passes"), "{f}");
    }

    #[test]
    fn the_multitone_budget_comes_from_the_device() {
        let (mut p, state) = panel();
        p.select_type(12);
        let f = testing::draw(&mut p, &state, 100, 60);
        assert!(f.contains("Up to 16 tones on this device."), "{f}");
    }

    #[test]
    fn the_options_set_the_flags_the_firmware_defines() {
        let (mut p, state) = panel();
        p.select_type(2);
        let toggles: Vec<usize> = {
            let rows = p.rows(&state, panel::key_theme());
            panel::focus_rows(&rows)
                .iter()
                .enumerate()
                .filter(|(_, r)| matches!(rows[**r], Row::Toggle { .. }))
                .map(|(i, _)| i + 1)
                .collect()
        };
        assert_eq!(toggles.len(), 3, "noise adds the decorrelate option");
        p.body.focus = toggles[0];
        p.handle(key(KeyCode::Enter), &state);
        assert_eq!(p.draft.flags, SiggenConfig::FLAG_RAW);
        p.body.focus = toggles[1];
        p.handle(key(KeyCode::Enter), &state);
        assert_eq!(
            p.draft.flags,
            SiggenConfig::FLAG_RAW | SiggenConfig::FLAG_DECORR
        );
        p.body.focus = toggles[2];
        p.handle(key(KeyCode::Enter), &state);
        assert_eq!(
            p.draft.flags & SiggenConfig::FLAG_WALK,
            SiggenConfig::FLAG_WALK
        );
    }

    /// The whole point of the draft: what the panel writes has to be a line
    /// the command grammar can parse straight back into the same packet.
    #[test]
    fn the_config_line_round_trips_through_the_command_grammar() {
        let (mut p, state) = panel();
        p.select_type(6);
        p.draft.level_db = -12.5;
        p.draft.flags = SiggenConfig::FLAG_RAW | SiggenConfig::FLAG_WALK;
        p.draft.invert_mask = 0x0002;
        p.draft.repeat_count = 3;
        p.draft.gap_ms = 250;
        let line = p.config_command();
        let ctx = dspi_cmd::Context {
            channel_slugs: Vec::new(),
            num_inputs: state.caps.num_inputs,
            num_outputs: state.caps.num_outputs,
            max_bands: state.caps.max_bands,
        };
        let tokens = dspi_cmd::tokenize(&line);
        let refs: Vec<&str> = tokens.iter().map(String::as_str).collect();
        match dspi_cmd::parse(&refs, &ctx).expect("the panel's own line must parse") {
            dspi_cmd::Command::Set {
                path,
                value: dspi_proto::value::Value::Bytes(bytes),
                ..
            } => {
                assert_eq!(path, "sig.config");
                let back = SiggenConfig::decode(&bytes).expect("36 bytes");
                assert_eq!(back.signal_type, 6);
                assert_eq!(back.channel_mask, 0x1FF);
                assert_eq!(back.invert_mask, 0x0002);
                assert_eq!(back.flags, SiggenConfig::FLAG_RAW | SiggenConfig::FLAG_WALK);
                assert_eq!(back.level_db, -12.5);
                assert_eq!(back.duration_ms, 5000);
                assert_eq!(back.repeat_count, 3);
                assert_eq!(back.gap_ms, 250);
                assert_eq!(back.p3, 3.0);
                assert_eq!(back.p4, 250.0);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn every_advertised_key_is_handled_somewhere() {
        let (_, state) = panel();
        for help in KEYS {
            for k in crate::screens::tests::keys_for(help.key) {
                let mut hit = false;
                for focus in 1..14 {
                    let mut p = SignalsPanel::new();
                    p.seed(&state);
                    p.body.focus = focus;
                    let before = (p.body.focus, p.chip, p.tile, p.transport);
                    let ev = p.handle(k, &state);
                    let after = (p.body.focus, p.chip, p.tile, p.transport);
                    if ev != ScreenEvent::Unhandled || before != after {
                        hit = true;
                        break;
                    }
                }
                assert!(hit, "{:?} from {:?} is not bound", k.code, help.key);
            }
        }
    }
}
