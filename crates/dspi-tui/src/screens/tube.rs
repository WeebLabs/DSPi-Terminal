//! The Tube Modeller panel: `TubeModellerView.swift` as a tool panel.
//!
//! The Console has a Basic and an Advanced layout; the terminal has one, the
//! Advanced rows, in the order the brief sets: the transfer curve, then
//! STAGE, CHARACTER, OUTPUT STAGE and OUTPUTS.
//!
//! The Console mirrors the firmware's type rules locally, loading a row's
//! four character values when a type is chosen and switching to Custom when
//! one of them is edited (`Commands.swift:1414-1550`). The terminal does
//! neither: it writes the one parameter and the re-read brings back whatever
//! the device made of it (`DESIGN.md` 11), so the rules live only in
//! `tube.c:115-167`.
//!
//! The transfer curve is the firmware's static waveshaper, taken from the
//! float kernel (`tube.c:228-236`, `:254-266`, `:296-297`, `:398-427`):
//! drive, bias, the two knees, the hardness blend, the rest-point offset, mix
//! and trim. What it leaves out has memory (sag, the 2.5 Hz DC blocker and
//! the output stage), so it is the stage's curve and not a frequency
//! response, exactly as the Console labels it. The RP2040's Q28 kernel clamps
//! its intermediates only far above full scale (`tube.c:449-514`), where the
//! drawn range ends anyway.

use crossterm::event::KeyEvent;
use dspi_proto::generated::ranges as r;
use dspi_proto::generated::tube as t;
use dspi_session::DeviceState;
use dspi_session::state::Tube;
use dspi_session::tube::{RECTIFIERS, TYPES, type_name, type_row};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::number;
use super::panel::{self, Body, ChipSpec, Header, PanelGraph, Param, Row};
use crate::shell::{Screen, ScreenEvent};
use crate::theme::{ChannelRole, Theme};
use crate::widgets::{Action, ChipState, KeyHelp, PopupList};

/// Name, detail, type, drive, rectifier, output stage, damping, resonance.
type StartingPoint = (&'static str, &'static str, u8, f32, u8, bool, f32, f32);

/// `tubeStartingPoints`, `TubeModellerView.swift:88-99`: name, detail, then
/// type, drive, rectifier, output stage, damping and resonance. Each also
/// sets mix 100 and trim 0 (`:463-464`).
const STARTING_POINTS: [StartingPoint; 5] = [
    (
        "Clean default",
        "12AX7, level-neutral, output stage on",
        1,
        r::TUBE_DEFAULT_DRIVE,
        1,
        true,
        r::TUBE_DEFAULT_XFMR_DAMPING,
        r::TUBE_DEFAULT_XFMR_RES,
    ),
    (
        "Warm hi-fi",
        "12AU7 line stage, tightly damped",
        5,
        -3.0,
        1,
        true,
        10.0,
        r::TUBE_DEFAULT_XFMR_RES,
    ),
    (
        "Single-ended sweetness",
        "300B, loose damping",
        16,
        3.0,
        1,
        true,
        2.0,
        r::TUBE_DEFAULT_XFMR_RES,
    ),
    (
        "Guitar-amp style",
        "12AX7 pushed, 5U4, loose damping",
        1,
        15.0,
        2,
        true,
        2.0,
        100.0,
    ),
    (
        "Push-pull power",
        "EL34 with the output stage on",
        12,
        0.0,
        1,
        true,
        6.0,
        r::TUBE_DEFAULT_XFMR_RES,
    ),
];

/// The Tube menu's three groups, `TubeModellerView.swift:539-547`, after
/// Custom.
const TYPE_GROUPS: [(&str, u8, u8); 3] = [
    ("Preamp triodes", 1, 8),
    ("Preamp pentodes", 9, 10),
    ("Power stages", 11, 16),
];

/// The Console's outputs menu, `TubeModellerView.swift:584-595`.
const OUTPUT_PRESETS: [&str; 3] = ["All outputs", "Exclude sub", "None"];

/// The registry's rectifier spellings, `tube.rectifier`'s choices.
const RECT_TOKENS: [&str; 4] = ["solid-state", "gz34", "5u4", "5y3"];

/// How many inputs the transfer curve is sampled at, the Console's 240 steps.
const CURVE_STEPS: usize = 240;

const KEYS: &[KeyHelp] = &[
    KeyHelp::new("↑ ↓", "Move"),
    KeyHelp::new("← →", "Adjust"),
    KeyHelp::new("Enter", "Edit or choose"),
    KeyHelp::new("Space", "Toggle"),
    KeyHelp::new("p", "Presets"),
    KeyHelp::new("Backspace", "Reset"),
];

/// The page command bar's words (DESIGN 13).
const VERBS: [&str; 14] = [
    "on",
    "off",
    "type",
    "drive",
    "mix",
    "trim",
    "bias",
    "asym",
    "hardness",
    "sag",
    "rect",
    "stage",
    "damping",
    "resonance",
];

// ------------------------------------------------------------------ shaper

/// The firmware's static waveshaper in f64, the Console's `TubeShaper`
/// (`TubeModellerView.swift:938-1015`), each line checked against `tube.c`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shaper {
    m: f64,
    b: f64,
    ratio_n: f64,
    c1: f64,
    c3: f64,
    c5: f64,
    s_p: f64,
    s_n: f64,
    dry_w: f64,
    wet_w: f64,
    v0: f64,
}

impl Shaper {
    /// The coefficients `tube_compute_coefficients` derives (tube.c:256-266,
    /// :294-297), from the parameters clamped as it clamps them.
    pub fn new(p: &Tube) -> Self {
        let c = |v: f32, lo: f32, hi: f32| v.clamp(lo, hi) as f64;
        let drive = c(p.drive_db, r::TUBE_DRIVE_MIN, r::TUBE_DRIVE_MAX);
        let bias = c(p.bias_pct, r::TUBE_BIAS_MIN, r::TUBE_BIAS_MAX);
        let asym = c(p.asym_db, r::TUBE_ASYM_MIN, r::TUBE_ASYM_MAX);
        let h = c(p.hardness_pct, r::TUBE_HARDNESS_MIN, r::TUBE_HARDNESS_MAX) / 100.0;
        let mix = c(p.mix_pct, r::TUBE_MIX_MIN, r::TUBE_MIX_MAX) / 100.0;
        let trim = c(p.trim_db, r::TUBE_TRIM_MIN, r::TUBE_TRIM_MAX);

        let m = 10f64.powf(drive / 20.0);
        let c1 = 1.5 + 0.375 * h;
        let kn = 10f64.powf(asym / 20.0);
        let mut s = Self {
            m,
            b: bias * 0.005,
            ratio_n: 1.0 / kn,
            c1,
            c3: -0.5 - 0.75 * h,
            c5: 0.375 * h,
            // The 1/m makeup: drive moves the knee, not the level.
            s_p: 1.0 / (c1 * m),
            s_n: kn / (c1 * m),
            dry_w: 1.0 - mix,
            wet_w: mix * 10f64.powf(trim / 20.0),
            v0: 0.0,
        };
        s.v0 = s.shape(0.0);
        s
    }

    /// `shaper_f` (tube.c:228-236) after the drive and bias of the kernel
    /// (tube.c:398): the stage's output for input `x`, offset included.
    fn shape(&self, x: f64) -> f64 {
        let mut t = self.m * x + self.b;
        if t < 0.0 {
            t *= self.ratio_n;
        }
        let t = t.clamp(-1.0, 1.0);
        let t2 = t * t;
        let p = t * (self.c1 + t2 * (self.c3 + t2 * self.c5));
        p * if t >= 0.0 { self.s_p } else { self.s_n }
    }

    /// What leaves the module for input `x`: the wet path less its rest
    /// point, blended with the dry input (tube.c:405, :427).
    pub fn output(&self, x: f64) -> f64 {
        self.dry_w * x + self.wet_w * (self.shape(x) - self.v0)
    }

    /// The inputs at which each half reaches its knee, negative then
    /// positive, when that is inside a full-scale swing: `m x + b = 1` and
    /// `(m x + b) ratio_n = -1`.
    pub fn knees(&self) -> (Option<f64>, Option<f64>) {
        let pos = (1.0 - self.b) / self.m;
        let neg = (-1.0 / self.ratio_n - self.b) / self.m;
        (
            (neg > -1.0).then_some(neg.max(-1.0)),
            (pos < 1.0).then_some(pos.min(1.0)),
        )
    }

    /// The curve at [`CURVE_STEPS`] + 1 inputs from -1 to +1.
    pub fn curve(&self) -> Vec<f64> {
        (0..=CURVE_STEPS)
            .map(|i| self.output(-1.0 + 2.0 * i as f64 / CURVE_STEPS as f64))
            .collect()
    }

    /// The second and third harmonic of a full-scale sine, in dB against the
    /// fundamental, or -120 when absent. A 256-point DFT at the three bins is
    /// exact for a memoryless curve (`TubeShaper.harmonics`).
    pub fn harmonics(&self) -> (f64, f64) {
        let n = 256;
        let (mut re, mut im) = ([0.0f64; 3], [0.0f64; 3]);
        for i in 0..n {
            let phase = 2.0 * std::f64::consts::PI * i as f64 / n as f64;
            let y = self.output(phase.sin());
            for k in 0..3 {
                re[k] += y * ((k + 1) as f64 * phase).cos();
                im[k] += y * ((k + 1) as f64 * phase).sin();
            }
        }
        let mag: Vec<f64> = (0..3).map(|k| re[k].hypot(im[k])).collect();
        let rel = |a: f64| {
            if mag[0] <= 1e-12 || a <= mag[0] * 1e-6 {
                -120.0
            } else {
                20.0 * (a / mag[0]).log10()
            }
        };
        (rel(mag[1]), rel(mag[2]))
    }
}

/// The output stage's lift at the resonance and at the top, in dB: a source
/// impedance of Zn/df against a speaker rising to `Z_PEAK` and `Z_HF` times
/// nominal (tube.c:279-284, tube.h:87-88; `xfmrLift`,
/// `TubeModellerView.swift:748-753`).
pub fn xfmr_lift(damping: f32) -> (f64, f64) {
    let df = damping.max(r::TUBE_XFMR_DAMPING_MIN) as f64;
    let lift = |z: f64| 20.0 * (z * (df + 1.0) / (z * df + 1.0)).log10();
    (
        lift(r::TUBE_XFMR_Z_PEAK_RATIO as f64),
        lift(r::TUBE_XFMR_Z_HF_RATIO as f64),
    )
}

/// The rectifier's summary line, `rectifierSummary`
/// (`TubeModellerView.swift:732-740`).
pub fn rectifier_summary(rectifier: u8) -> String {
    match RECTIFIERS.get(rectifier as usize) {
        Some(row) if rectifier as u16 != t::TUBE_RECT_SOLID_STATE => format!(
            "Sag depth x{:.1}, {:.0} ms attack, {:.0} ms release.",
            row.depth_scale, row.attack_ms, row.release_ms
        ),
        _ => "No sag: the supply holds up however hard the stage is driven.".into(),
    }
}

/// The Tube row's caption, `tubeTypeCaption` (`TubeModellerView.swift:
/// 567-575`): the style, and for a push-pull row with the output stage off,
/// the suggestion to turn it on.
pub fn type_caption(p: &Tube) -> String {
    match type_row(p.tube_type) {
        None => "Character controls as set, no tube row applied.".into(),
        Some(row) if row.push_pull && !p.xfmr_enabled => {
            format!("{}. Meant for use with the output stage on.", row.style)
        }
        Some(row) => format!("{}.", row.style),
    }
}

/// The Tube menu: Custom, then each group under its header, as a popup's
/// items beside the type each one chooses (`None` for a header).
pub fn type_menu() -> Vec<(String, Option<u8>)> {
    let mut items = vec![("Custom".to_string(), Some(t::TUBE_TYPE_CUSTOM as u8))];
    for (group, lo, hi) in TYPE_GROUPS {
        items.push((format!("#{group}"), None));
        items.extend((lo..=hi).map(|n| (type_name(n), Some(n))));
    }
    items
}

fn harmonic(db: f64) -> String {
    if db <= -100.0 {
        "none".into()
    } else {
        format!("{db:.0} dB")
    }
}

// ------------------------------------------------------------------- panel

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    Outputs,
    StartingPoint,
    Type,
}

pub struct TubePanel {
    body: Body,
    chip: usize,
    pending: Option<Pending>,
}

impl Default for TubePanel {
    fn default() -> Self {
        Self::new()
    }
}

impl TubePanel {
    pub fn new() -> Self {
        Self {
            body: Body::default(),
            chip: 0,
            pending: None,
        }
    }

    /// The feature ships in wire format V31; the Console hides the body and
    /// shows an upgrade note on anything older (`TubeModellerView.swift:
    /// 123-125`). The probe reports it as `tube_preamp`.
    pub fn supported(state: &DeviceState) -> bool {
        panel::has_feature(state, "tube_preamp")
    }

    fn rows(&self, state: &DeviceState, theme: &Theme) -> Vec<Row> {
        if !Self::supported(state) {
            return vec![Row::Banner {
                title: "Requires firmware with wire format V31 or newer.".into(),
                body: "Update the DSPi firmware to use the Tube Modeller.".into(),
            }];
        }
        let p = state.tube();
        let shaper = Shaper::new(&p);
        let (second, third) = shaper.harmonics();
        let mut rows = vec![
            Row::Menu {
                title: "Transfer curve".into(),
                action: "Apply preset ▾".into(),
            },
            Row::Graph(if p.enabled {
                PanelGraph::Transfer {
                    output: shaper.curve(),
                    knees: shaper.knees(),
                }
            } else {
                PanelGraph::Disabled
            }),
            Row::Section {
                title: "At full scale".into(),
                action: Some(format!(
                    "2nd {}   3rd {}",
                    harmonic(second),
                    harmonic(third)
                )),
            },
            Row::Blank,
            Row::Section {
                title: "Stage".into(),
                action: None,
            },
            Row::Buttons {
                label: "Tube".into(),
                buttons: vec![format!("{} ▾", type_name(p.tube_type))],
                cursor: 0,
            },
            Row::Caption(type_caption(&p)),
            Row::Param(
                Param::new(
                    "Drive",
                    p.drive_db as f64,
                    r::TUBE_DRIVE_MIN as f64,
                    r::TUBE_DRIVE_MAX as f64,
                    "dB",
                )
                .step(0.5)
                .decimals(1),
            ),
            Row::Param(
                Param::new(
                    "Mix",
                    p.mix_pct as f64,
                    r::TUBE_MIX_MIN as f64,
                    r::TUBE_MIX_MAX as f64,
                    "%",
                )
                .step(1.0)
                .decimals(0)
                .ends("Dry", "All tube"),
            ),
            Row::Param(
                Param::new(
                    "Output Trim",
                    p.trim_db as f64,
                    r::TUBE_TRIM_MIN as f64,
                    r::TUBE_TRIM_MAX as f64,
                    "dB",
                )
                .step(0.5)
                .decimals(1),
            ),
            Row::Blank,
            Row::Section {
                title: "Character".into(),
                // `TubeModellerView.swift:645`.
                action: Some(
                    type_row(p.tube_type).map_or("custom".into(), |r| format!("from {}", r.name)),
                ),
            },
            Row::Param(
                Param::new(
                    "Bias",
                    p.bias_pct as f64,
                    r::TUBE_BIAS_MIN as f64,
                    r::TUBE_BIAS_MAX as f64,
                    "%",
                )
                .step(1.0)
                .decimals(0),
            ),
            Row::Param(
                Param::new(
                    "Asymmetry",
                    p.asym_db as f64,
                    r::TUBE_ASYM_MIN as f64,
                    r::TUBE_ASYM_MAX as f64,
                    "dB",
                )
                .step(0.5)
                .decimals(1),
            ),
            Row::Param(
                Param::new(
                    "Knee Hardness",
                    p.hardness_pct as f64,
                    r::TUBE_HARDNESS_MIN as f64,
                    r::TUBE_HARDNESS_MAX as f64,
                    "%",
                )
                .step(1.0)
                .decimals(0),
            ),
            // Solid state switches sag off, so the Console dims the row but
            // leaves it live (`TubeModellerView.swift:700`).
            Row::Param(
                Param::new(
                    "Sag",
                    p.sag_pct as f64,
                    r::TUBE_SAG_MIN as f64,
                    r::TUBE_SAG_MAX as f64,
                    "%",
                )
                .step(1.0)
                .decimals(0)
                .dimmed(p.rectifier as u16 == t::TUBE_RECT_SOLID_STATE),
            ),
            Row::Segmented {
                label: "Rectifier".into(),
                choices: RECTIFIERS
                    .iter()
                    .map(|r| r.short_name().to_string())
                    .collect(),
                selected: (p.rectifier as usize).min(RECTIFIERS.len() - 1),
                enabled: true,
            },
            Row::Caption(rectifier_summary(p.rectifier)),
            Row::Blank,
            Row::Section {
                title: "Output stage".into(),
                action: None,
            },
            Row::Toggle {
                label: "Enabled".into(),
                on: p.xfmr_enabled,
                caption: Some("A valve amplifier's loose grip on the speaker.".into()),
                enabled: true,
            },
        ];
        // The firmware runs none of the stage while it is off, so the Console
        // hides its rows rather than showing them doing nothing (`TubeModellerView.swift:775-778`).
        if p.xfmr_enabled {
            let (bell, top) = xfmr_lift(p.xfmr_damping);
            rows.push(Row::Param(
                Param::new(
                    "Damping Factor",
                    p.xfmr_damping as f64,
                    r::TUBE_XFMR_DAMPING_MIN as f64,
                    r::TUBE_XFMR_DAMPING_MAX as f64,
                    "",
                )
                .step(0.5)
                .decimals(1)
                .ends("1 (loose)", "20 (tight)"),
            ));
            rows.push(Row::Caption(format!(
                "+{bell:.1} dB at resonance, +{top:.1} dB at the top."
            )));
            rows.push(Row::Param(
                Param::new(
                    "Speaker Resonance",
                    p.xfmr_res_hz as f64,
                    r::TUBE_XFMR_RES_MIN as f64,
                    r::TUBE_XFMR_RES_MAX as f64,
                    "Hz",
                )
                .step(1.0)
                .decimals(0)
                .ends("30 Hz", "150 Hz"),
            ));
        }
        rows.push(Row::Blank);
        rows.push(Row::Section {
            title: "Outputs".into(),
            action: Some("Presets ▾".into()),
        });
        let chips = (0..state.caps.num_outputs as usize)
            .map(|o| ChipSpec {
                label: (o + 1).to_string(),
                state: if p.output_mask & (1 << o) != 0 {
                    ChipState::On
                } else {
                    ChipState::Off
                },
                color: theme.role_color(ChannelRole::of(
                    state.caps.num_inputs + o as u8,
                    state.caps.num_inputs,
                    state.caps.num_outputs,
                )),
                enabled: true,
                dimmed: false,
            })
            .collect();
        rows.push(Row::Chips {
            chips,
            cursor: self.chip,
            polarity: false,
        });
        rows.push(Row::Caption(
            "Tube runs before the crossover and the per-output EQ, where a real preamp sits: \
             a sub output saturates the full-band program and then low-passes the result."
                .into(),
        ));
        rows
    }

    fn path_for(label: &str) -> &'static str {
        match label {
            "Drive" => "tube.drive",
            "Mix" => "tube.mix",
            "Output Trim" => "tube.trim",
            "Bias" => "tube.bias",
            "Asymmetry" => "tube.asym",
            "Knee Hardness" => "tube.hardness",
            "Sag" => "tube.sag",
            "Damping Factor" => "tube.damping",
            _ => "tube.resonance",
        }
    }

    /// The header's defaults (tube.h:61-72), for Backspace.
    fn default_for(label: &str) -> f32 {
        match label {
            "Drive" => r::TUBE_DEFAULT_DRIVE,
            "Mix" => r::TUBE_DEFAULT_MIX,
            "Output Trim" => r::TUBE_DEFAULT_TRIM,
            "Bias" => r::TUBE_DEFAULT_BIAS,
            "Asymmetry" => r::TUBE_DEFAULT_ASYM,
            "Knee Hardness" => r::TUBE_DEFAULT_HARDNESS,
            "Sag" => r::TUBE_DEFAULT_SAG,
            "Damping Factor" => r::TUBE_DEFAULT_XFMR_DAMPING,
            _ => r::TUBE_DEFAULT_XFMR_RES,
        }
    }

    fn param_command(label: &str, value: f64) -> ScreenEvent {
        ScreenEvent::Command(format!(
            "{} {}",
            Self::path_for(label),
            number(value as f32)
        ))
    }

    fn act(&mut self, row: &Row, action: Action, state: &DeviceState) -> ScreenEvent {
        match (row, action) {
            (Row::Menu { .. }, Action::Open) => {
                self.pending = Some(Pending::StartingPoint);
                ScreenEvent::Popup(PopupList::new(
                    "Apply preset",
                    STARTING_POINTS
                        .iter()
                        .map(|(n, d, ..)| format!("{n} - {d}"))
                        .collect(),
                    0,
                ))
            }
            (Row::Buttons { .. }, Action::Button(_)) => {
                self.pending = Some(Pending::Type);
                let menu = type_menu();
                let current = state.tube().tube_type;
                let at = menu
                    .iter()
                    .position(|(_, n)| *n == Some(current))
                    .unwrap_or(0);
                ScreenEvent::Popup(PopupList::new(
                    "Tube",
                    menu.into_iter().map(|(label, _)| label).collect(),
                    at,
                ))
            }
            (Row::Segmented { .. }, Action::Selected(i)) => ScreenEvent::Command(format!(
                "tube.rectifier {}",
                RECT_TOKENS[i.min(RECT_TOKENS.len() - 1)]
            )),
            (Row::Toggle { .. }, Action::Toggled(next)) => {
                ScreenEvent::Command(format!("tube.xfmr {}", if next { "on" } else { "off" }))
            }
            (Row::Chips { .. }, Action::Selected(i)) => {
                self.chip = i;
                ScreenEvent::Handled
            }
            (Row::Chips { .. }, Action::Chip(i, next)) => {
                let bit = 1u16 << i;
                let mask = state.tube().output_mask;
                let mask = if next == ChipState::On {
                    mask | bit
                } else {
                    mask & !bit
                };
                ScreenEvent::Command(format!("tube.mask 0x{mask:X}"))
            }
            (Row::Chips { .. }, Action::Open) => {
                self.pending = Some(Pending::Outputs);
                ScreenEvent::Popup(PopupList::new(
                    "Outputs",
                    OUTPUT_PRESETS.iter().map(|s| s.to_string()).collect(),
                    0,
                ))
            }
            (Row::Param(p), Action::Changed(v)) => Self::param_command(&p.label, v),
            (Row::Param(p), Action::Reset) => {
                Self::param_command(&p.label, Self::default_for(&p.label) as f64)
            }
            _ => ScreenEvent::Handled,
        }
    }

    /// The Console's three output presets. Exclude sub clears the PDM
    /// output's bit, the last output slot on either platform.
    fn output_preset(state: &DeviceState, i: usize) -> u16 {
        let all = panel::all_outputs_mask(state);
        match i {
            0 => all,
            1 => all & !(1u16 << panel::sub_output(state)),
            _ => 0x0000,
        }
    }

    /// The lines a starting point writes, in the Console's order
    /// (`TubeModellerView.swift:455-465`).
    fn starting_point(i: usize) -> String {
        let (_, _, ty, drive, rect, xfmr, damping, res) =
            STARTING_POINTS[i.min(STARTING_POINTS.len() - 1)];
        [
            format!("tube.type {ty}"),
            format!("tube.drive {}", number(drive)),
            format!("tube.rectifier {}", RECT_TOKENS[rect as usize]),
            format!("tube.damping {}", number(damping)),
            format!("tube.resonance {}", number(res)),
            format!("tube.xfmr {}", if xfmr { "on" } else { "off" }),
            format!("tube.mix {}", number(r::TUBE_MIX_MAX)),
            "tube.trim 0".to_string(),
        ]
        .join("\n")
    }

    /// The page grammar: `on`, `off`, `stage on`, `type 12ax7`, `rect gz34`,
    /// and a verb and a number for each slider.
    fn grammar(line: &str, state: &DeviceState) -> crate::shell::Quick {
        use super::quick::{ghost, number as num, verb};
        let tokens: Vec<String> = line
            .split_whitespace()
            .map(|s| s.to_ascii_lowercase())
            .collect();
        let quick = |hint: String, commands: Vec<String>| crate::shell::Quick {
            hint,
            ghost: ghost(line, &VERBS),
            commands,
            fallthrough: false,
        };
        let Some(first) = tokens.first() else {
            return quick(
                "on · off · type · drive · mix · trim · bias · asym · hardness · sag · rect · \
                 stage · damping · resonance"
                    .into(),
                Vec::new(),
            );
        };
        let Some(v) = verb(first, &VERBS) else {
            return crate::shell::Quick {
                fallthrough: true,
                ..quick(
                    "not a Tube Modeller word · runs the : grammar".into(),
                    Vec::new(),
                )
            };
        };
        let arg = tokens.get(1).map(String::as_str);
        let p = state.tube();
        match v {
            "on" | "off" => quick(format!("Tube Modeller {v}"), vec![format!("tube.on {v}")]),
            "stage" => match arg.and_then(|a| verb(a, &["on", "off"])) {
                Some(s) => quick(format!("Output stage {s}"), vec![format!("tube.xfmr {s}")]),
                None => quick(
                    format!(
                        "stage on · off (now {})",
                        if p.xfmr_enabled { "on" } else { "off" }
                    ),
                    Vec::new(),
                ),
            },
            "type" => match arg.and_then(parse_type) {
                Some(n) => quick(
                    format!("Tube {}", type_name(n)),
                    vec![format!("tube.type {n}")],
                ),
                None => quick(
                    format!(
                        "type custom · 12ax7 · el34 · 0 to {} (now {})",
                        t::TUBE_TYPE_MAX,
                        type_name(p.tube_type)
                    ),
                    Vec::new(),
                ),
            },
            "rect" => match arg.and_then(parse_rect) {
                Some(n) => quick(
                    format!("Rectifier {}", RECTIFIERS[n as usize].name),
                    vec![format!("tube.rectifier {}", RECT_TOKENS[n as usize])],
                ),
                None => quick("rect solid · gz34 · 5u4 · 5y3".into(), Vec::new()),
            },
            slider => {
                let (label, path, lo, hi, unit, now) = match slider {
                    "drive" => (
                        "Drive",
                        "tube.drive",
                        r::TUBE_DRIVE_MIN,
                        r::TUBE_DRIVE_MAX,
                        " dB",
                        p.drive_db,
                    ),
                    "mix" => (
                        "Mix",
                        "tube.mix",
                        r::TUBE_MIX_MIN,
                        r::TUBE_MIX_MAX,
                        "%",
                        p.mix_pct,
                    ),
                    "trim" => (
                        "Output Trim",
                        "tube.trim",
                        r::TUBE_TRIM_MIN,
                        r::TUBE_TRIM_MAX,
                        " dB",
                        p.trim_db,
                    ),
                    "bias" => (
                        "Bias",
                        "tube.bias",
                        r::TUBE_BIAS_MIN,
                        r::TUBE_BIAS_MAX,
                        "%",
                        p.bias_pct,
                    ),
                    "asym" => (
                        "Asymmetry",
                        "tube.asym",
                        r::TUBE_ASYM_MIN,
                        r::TUBE_ASYM_MAX,
                        " dB",
                        p.asym_db,
                    ),
                    "hardness" => (
                        "Knee Hardness",
                        "tube.hardness",
                        r::TUBE_HARDNESS_MIN,
                        r::TUBE_HARDNESS_MAX,
                        "%",
                        p.hardness_pct,
                    ),
                    "sag" => (
                        "Sag",
                        "tube.sag",
                        r::TUBE_SAG_MIN,
                        r::TUBE_SAG_MAX,
                        "%",
                        p.sag_pct,
                    ),
                    "damping" => (
                        "Damping Factor",
                        "tube.damping",
                        r::TUBE_XFMR_DAMPING_MIN,
                        r::TUBE_XFMR_DAMPING_MAX,
                        "",
                        p.xfmr_damping,
                    ),
                    _ => (
                        "Speaker Resonance",
                        "tube.resonance",
                        r::TUBE_XFMR_RES_MIN,
                        r::TUBE_XFMR_RES_MAX,
                        " Hz",
                        p.xfmr_res_hz,
                    ),
                };
                match arg.and_then(num) {
                    Some(x) if (lo as f64..=hi as f64).contains(&x) => quick(
                        format!("{label} {}{unit}", number(x as f32)),
                        vec![format!("{path} {}", number(x as f32))],
                    ),
                    _ => quick(
                        format!(
                            "{v} {} to {}{unit} (now {}{unit})",
                            number(lo),
                            number(hi),
                            number(now)
                        ),
                        Vec::new(),
                    ),
                }
            }
        }
    }
}

/// A type by number, `custom`, or any unambiguous start of one of a row's
/// names (`12ax7`, `ecc83`, `el3`).
fn parse_type(tok: &str) -> Option<u8> {
    if let Ok(n) = tok.parse::<u8>() {
        return (n as u16 <= t::TUBE_TYPE_MAX).then_some(n);
    }
    if "custom".starts_with(tok) {
        return Some(t::TUBE_TYPE_CUSTOM as u8);
    }
    let hits: Vec<u8> = TYPES
        .iter()
        .enumerate()
        .filter(|(_, row)| {
            row.name
                .split(" / ")
                .any(|alias| alias.to_ascii_lowercase().starts_with(tok))
        })
        .map(|(i, _)| i as u8 + 1)
        .collect();
    match hits.as_slice() {
        [one] => Some(*one),
        _ => None,
    }
}

/// A rectifier by number or by an unambiguous start of its name.
fn parse_rect(tok: &str) -> Option<u8> {
    if let Ok(n) = tok.parse::<u8>() {
        return (n as u16 <= t::TUBE_RECT_MAX).then_some(n);
    }
    let names = ["solid-state", "gz34", "5ar4", "5u4", "5y3"];
    super::quick::verb(tok, &names).map(|n| match n {
        "solid-state" => 0,
        "gz34" | "5ar4" => 1,
        "5u4" => 2,
        _ => 3,
    })
}

impl Screen for TubePanel {
    fn quick(&self, line: &str, state: &DeviceState) -> Option<crate::shell::Quick> {
        Some(Self::grammar(line, state))
    }

    fn title(&self) -> String {
        "Tube Modeller".into()
    }

    fn draw(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        state: &DeviceState,
        focused: bool,
    ) {
        let rows = self.rows(state, theme);
        self.body.clamp(&rows);
        let header = Header::new(
            "Valve-style harmonic colour, supply sag and a tube amplifier's output stage",
        )
        .toggle(state.tube().enabled)
        .enabled(Self::supported(state));
        self.body.draw(area, buf, theme, &header, &rows, focused);
    }

    fn handle(&mut self, key: KeyEvent, state: &DeviceState) -> ScreenEvent {
        if !Self::supported(state) {
            return ScreenEvent::Unhandled;
        }
        let rows = self.rows(state, panel::key_theme());
        self.body.clamp(&rows);
        match panel::dispatch(&mut self.body, &rows, key) {
            panel::Step::Done(ev) => ev,
            panel::Step::Header => ScreenEvent::Command(format!(
                "tube.on {}",
                if state.tube().enabled { "off" } else { "on" }
            )),
            panel::Step::Commit(i, v) => match &rows[i] {
                Row::Param(p) => Self::param_command(&p.label, v.clamp(p.min, p.max)),
                _ => ScreenEvent::Handled,
            },
            panel::Step::Row(i, action) => self.act(&rows[i], action, state),
        }
    }

    fn keys(&self) -> &'static [KeyHelp] {
        KEYS
    }

    fn popup_result(&mut self, choice: Option<usize>, state: &DeviceState) -> ScreenEvent {
        match (self.pending.take(), choice) {
            (Some(Pending::Outputs), Some(i)) => {
                ScreenEvent::Command(format!("tube.mask 0x{:X}", Self::output_preset(state, i)))
            }
            (Some(Pending::StartingPoint), Some(i)) => {
                ScreenEvent::Command(Self::starting_point(i))
            }
            (Some(Pending::Type), Some(i)) => match type_menu().get(i) {
                Some((_, Some(n))) => ScreenEvent::Command(format!("tube.type {n}")),
                _ => ScreenEvent::Handled,
            },
            _ => ScreenEvent::Handled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::panel::testing;
    use crate::shell::{Tool, fixture};
    use crate::widgets::testing::key;
    use crossterm::event::KeyCode;

    fn section(name: &str) -> usize {
        dspi_proto::generated::SECTIONS
            .iter()
            .find(|(n, _, _)| *n == name)
            .map(|(_, o, _)| *o)
            .expect("section")
    }

    /// The RP2350 fixture with the tube at its power-on values.
    fn state(enabled: bool) -> DeviceState {
        let mut s = fixture::state();
        fixture::tube(&mut s, enabled);
        s
    }

    /// The RP2040's shape: two inputs and five outputs, the sub the fifth.
    fn rp2040(enabled: bool) -> DeviceState {
        let mut s = state(enabled);
        s.caps.platform = dspi_proto::Platform::Rp2040;
        s.caps.num_inputs = 2;
        s.caps.num_outputs = 5;
        s
    }

    fn set_type(s: &mut DeviceState, n: u8) {
        s.bulk.patch(section("tube") + 1, &[n]);
    }
    fn set_rect(s: &mut DeviceState, n: u8) {
        s.bulk.patch(section("tube") + 2, &[n]);
    }
    fn set_stage(s: &mut DeviceState, on: bool) {
        s.bulk.patch(section("tube") + 3, &[u8::from(on)]);
    }
    fn set_f32(s: &mut DeviceState, at: usize, v: f32) {
        s.bulk.patch(section("tube") + at, &v.to_le_bytes());
    }

    /// Put the cursor on the first row that matches.
    fn focus(p: &mut TubePanel, s: &DeviceState, pick: impl Fn(&Row) -> bool) {
        let rows = p.rows(s, panel::key_theme());
        p.body.focus = panel::focus_rows(&rows)
            .iter()
            .position(|r| pick(&rows[*r]))
            .expect("a matching row")
            + 1;
    }

    fn frames(s: &DeviceState) -> Vec<(u16, u16, String)> {
        [(120u16, 40u16), (80, 24)]
            .into_iter()
            .map(|(w, h)| {
                let f = testing::frame(Tool::Tube, Box::new(TubePanel::new()), s, w, h);
                assert_eq!(f.lines().count(), h as usize, "{w}x{h}");
                assert!(f.contains("Tube Modeller"), "{w}x{h}:\n{f}");
                assert!(f.contains("D closes"), "{w}x{h}:\n{f}");
                (w, h, f)
            })
            .collect()
    }

    /// The panel alone, tall enough to hold every row.
    fn whole(s: &DeviceState) -> String {
        testing::draw(&mut TubePanel::new(), s, 100, 80)
    }

    #[test]
    fn golden_frames_disabled() {
        let s = state(false);
        for (w, h, f) in frames(&s) {
            assert!(f.contains("○ Off"), "{w}x{h}:\n{f}");
            assert!(f.contains("TRANSFER CURVE"), "{w}x{h}:\n{f}");
            assert!(f.contains("Disabled"), "{w}x{h}:\n{f}");
            assert!(!f.contains("0 dBFS"), "no curve while off: {f}");
        }
    }

    #[test]
    fn golden_frames_enabled_with_the_12ax7() {
        let s = state(true);
        for (w, h, f) in frames(&s) {
            assert!(f.contains("● On"), "{w}x{h}:\n{f}");
            assert!(f.contains("0 dBFS") && f.contains("out"), "{w}x{h}:\n{f}");
            assert!(f.contains("Apply preset ▾"), "{w}x{h}:\n{f}");
        }
        let (_, _, f) = &frames(&s)[0];
        assert!(f.contains("AT FULL SCALE"), "{f}");
        assert!(f.contains("2nd -39 dB   3rd -44 dB"), "{f}");
        assert!(f.contains("Tube  12AX7 / ECC83 ▾"), "{f}");
        assert!(f.contains("High-gain preamp triode."), "{f}");
        assert!(f.contains("from 12AX7 / ECC83"), "{f}");
        let all = whole(&s);
        for text in [
            "Valve-style harmonic colour, supply sag and a tube amplifier's output stage",
            "STAGE",
            "Drive",
            "-12.0 dB",
            "Dry",
            "All tube",
            "Output Trim",
            "CHARACTER",
            "Knee Hardness",
            "Solid state",
            "GZ34",
            "5U4",
            "5Y3",
            "Sag depth x0.6, 5 ms attack, 120 ms release.",
            "OUTPUT STAGE",
            "A valve amplifier's loose grip on the speaker.",
            "Damping Factor",
            "1 (loose)",
            "20 (tight)",
            "+2.5 dB at resonance, +1.6 dB at the top.",
            "Speaker Resonance",
            "95 Hz",
            "OUTPUTS",
            "Presets ▾",
        ] {
            assert!(all.contains(text), "{text:?}:\n{all}");
        }
    }

    #[test]
    fn golden_frames_custom() {
        let mut s = state(true);
        set_type(&mut s, 0);
        set_f32(&mut s, 12, -20.0);
        for (w, h, f) in frames(&s) {
            assert!(f.contains("Custom ▾"), "{w}x{h}:\n{f}");
        }
        let all = whole(&s);
        assert!(
            all.contains("Character controls as set, no tube row applied."),
            "{all}"
        );
        assert!(all.contains("custom"), "the character label: {all}");
        assert!(all.contains("-20%"), "{all}");
    }

    #[test]
    fn golden_frames_output_stage_off() {
        let mut s = state(true);
        set_type(&mut s, 12);
        set_stage(&mut s, false);
        for (w, h, f) in frames(&s) {
            assert!(f.contains("EL34 ▾"), "{w}x{h}:\n{f}");
        }
        let all = whole(&s);
        assert!(
            all.contains(
                "Push-pull power, mid crunch, deep sag. Meant for use with the output stage on."
            ),
            "{all}"
        );
        assert!(!all.contains("Damping Factor"), "{all}");
    }

    #[test]
    fn golden_frames_solid_state_rectifier() {
        let mut s = state(true);
        set_rect(&mut s, 0);
        for (_, _, f) in frames(&s) {
            assert!(f.contains("TRANSFER CURVE"), "{f}");
        }
        let all = whole(&s);
        assert!(
            all.contains("No sag: the supply holds up however hard the stage is driven."),
            "{all}"
        );
    }

    #[test]
    fn the_type_menu_is_custom_then_the_consoles_three_groups() {
        let menu = type_menu();
        let labels: Vec<&str> = menu.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels[0], "Custom");
        assert_eq!(labels[1], "#Preamp triodes");
        assert_eq!(labels[2], "12AX7 / ECC83");
        assert_eq!(labels[9], "6DJ8 / ECC88 / 6922");
        assert_eq!(labels[10], "#Preamp pentodes");
        assert_eq!(labels[11], "EF86 / 6267");
        assert_eq!(labels[13], "#Power stages");
        assert_eq!(labels[14], "EL84 / 6BQ5");
        assert_eq!(labels[19], "300B / 2A3");
        assert_eq!(menu.len(), 1 + 3 + t::TUBE_TYPE_MAX as usize);
        let types: Vec<u8> = menu.iter().filter_map(|(_, n)| *n).collect();
        assert_eq!(types, (0..=t::TUBE_TYPE_MAX as u8).collect::<Vec<_>>());

        // The popup opens on the current type and answers with its number.
        let s = state(true);
        let mut p = TubePanel::new();
        focus(&mut p, &s, |r| matches!(r, Row::Buttons { .. }));
        match p.handle(key(KeyCode::Enter), &s) {
            ScreenEvent::Popup(list) => {
                assert_eq!(list.title, "Tube");
                assert_eq!(list.cursor, 2, "on the 12AX7");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            p.popup_result(Some(15), &s),
            ScreenEvent::Command("tube.type 12".into()),
            "EL34, and nothing else: the device loads the row"
        );
    }

    #[test]
    fn sag_is_dimmed_but_live_with_a_solid_state_rectifier() {
        let s = state(true);
        let mut solid = state(true);
        set_rect(&mut solid, 0);
        let lit = testing::fg_of(&mut TubePanel::new(), &s, 100, 80, "Sag ");
        let dim = testing::fg_of(&mut TubePanel::new(), &solid, 100, 80, "Sag ");
        let bias = testing::fg_of(&mut TubePanel::new(), &solid, 100, 80, "Bias");
        assert_ne!(lit, dim, "dimmed with solid state");
        assert_eq!(lit, bias, "and only Sag");
        let mut p = TubePanel::new();
        focus(
            &mut p,
            &solid,
            |r| matches!(r, Row::Param(p) if p.label == "Sag"),
        );
        assert_eq!(
            p.handle(key(KeyCode::Right), &solid),
            ScreenEvent::Command("tube.sag 16".into())
        );
    }

    #[test]
    fn the_output_stage_rows_hide_while_it_is_off() {
        let mut s = state(true);
        let labels = |s: &DeviceState| -> Vec<String> {
            TubePanel::new()
                .rows(s, panel::key_theme())
                .iter()
                .filter_map(|r| match r {
                    Row::Param(p) => Some(p.label.clone()),
                    Row::Caption(c) if c.contains("at resonance") => Some(c.clone()),
                    _ => None,
                })
                .collect()
        };
        let on = labels(&s);
        assert!(on.iter().any(|l| l == "Damping Factor"));
        assert!(on.iter().any(|l| l == "Speaker Resonance"));
        assert!(on.iter().any(|l| l.contains("at resonance")));
        set_stage(&mut s, false);
        let off = labels(&s);
        assert!(!off.iter().any(|l| l == "Damping Factor"), "{off:?}");
        assert!(!off.iter().any(|l| l == "Speaker Resonance"), "{off:?}");
        assert!(!off.iter().any(|l| l.contains("at resonance")), "{off:?}");
        // The toggle stays, and turns it back on.
        let mut p = TubePanel::new();
        focus(&mut p, &s, |r| matches!(r, Row::Toggle { .. }));
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &s),
            ScreenEvent::Command("tube.xfmr on".into())
        );
    }

    #[test]
    fn exclude_sub_clears_the_pdm_output_on_both_platforms() {
        for (s, all, no_sub) in [(state(true), 0x1FF, 0xFF), (rp2040(true), 0x1F, 0x0F)] {
            let mut p = TubePanel::new();
            focus(&mut p, &s, |r| matches!(r, Row::Chips { .. }));
            match p.handle(key(KeyCode::Char('p')), &s) {
                ScreenEvent::Popup(list) => {
                    assert_eq!(list.items, vec!["All outputs", "Exclude sub", "None"])
                }
                other => panic!("{other:?}"),
            }
            assert_eq!(
                p.popup_result(Some(1), &s),
                ScreenEvent::Command(format!("tube.mask 0x{no_sub:X}"))
            );
            p.pending = Some(Pending::Outputs);
            assert_eq!(
                p.popup_result(Some(0), &s),
                ScreenEvent::Command(format!("tube.mask 0x{all:X}"))
            );
            p.pending = Some(Pending::Outputs);
            assert_eq!(
                p.popup_result(Some(2), &s),
                ScreenEvent::Command("tube.mask 0x0".into())
            );
            let chips = p
                .rows(&s, panel::key_theme())
                .into_iter()
                .find_map(|r| match r {
                    Row::Chips { chips, .. } => Some(chips.len()),
                    _ => None,
                });
            assert_eq!(chips, Some(s.caps.num_outputs as usize));
        }
    }

    #[test]
    fn a_starting_point_writes_the_consoles_eight_lines() {
        let s = state(true);
        let mut p = TubePanel::new();
        focus(&mut p, &s, |r| matches!(r, Row::Menu { .. }));
        match p.handle(key(KeyCode::Enter), &s) {
            ScreenEvent::Popup(list) => {
                assert_eq!(list.items.len(), 5);
                assert_eq!(
                    list.items[3],
                    "Guitar-amp style - 12AX7 pushed, 5U4, loose damping"
                );
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            p.popup_result(Some(3), &s),
            ScreenEvent::Command(
                "tube.type 1\ntube.drive 15\ntube.rectifier 5u4\ntube.damping 2\n\
                 tube.resonance 100\ntube.xfmr on\ntube.mix 100\ntube.trim 0"
                    .into()
            )
        );
    }

    #[test]
    fn every_line_the_panel_writes_parses_in_the_shared_grammar() {
        let s = state(true);
        let ctx = dspi_cmd::Context {
            channel_slugs: Vec::new(),
            num_inputs: s.caps.num_inputs,
            num_outputs: s.caps.num_outputs,
            max_bands: s.caps.max_bands,
        };
        let mut lines: Vec<String> = (0..STARTING_POINTS.len())
            .flat_map(|i| {
                TubePanel::starting_point(i)
                    .lines()
                    .map(String::from)
                    .collect::<Vec<_>>()
            })
            .collect();
        lines.extend(RECT_TOKENS.iter().map(|t| format!("tube.rectifier {t}")));
        lines.push("tube.mask 0x1FF".into());
        lines.push("tube.on on".into());
        for label in [
            "Drive",
            "Mix",
            "Output Trim",
            "Bias",
            "Asymmetry",
            "Knee Hardness",
            "Sag",
            "Damping Factor",
            "Speaker Resonance",
        ] {
            let ScreenEvent::Command(line) =
                TubePanel::param_command(label, TubePanel::default_for(label) as f64)
            else {
                unreachable!()
            };
            lines.push(line);
        }
        for line in lines {
            let tokens = dspi_cmd::tokenize(&line);
            let refs: Vec<&str> = tokens.iter().map(String::as_str).collect();
            dspi_cmd::parse(&refs, &ctx).unwrap_or_else(|e| panic!("{line}: {e}"));
        }
    }

    #[test]
    fn the_command_bar_speaks_the_panels_words() {
        let s = state(true);
        let p = TubePanel::new();
        let q = |line: &str| p.quick(line, &s).unwrap();
        assert_eq!(q("on").commands, vec!["tube.on on"]);
        assert_eq!(q("off").commands, vec!["tube.on off"]);
        assert_eq!(q("type 12ax7").commands, vec!["tube.type 1"]);
        assert_eq!(q("type el34").commands, vec!["tube.type 12"]);
        assert_eq!(q("ty ecc88").commands, vec!["tube.type 8"]);
        assert_eq!(q("type custom").commands, vec!["tube.type 0"]);
        assert_eq!(q("type 16").commands, vec!["tube.type 16"]);
        assert_eq!(q("dri -6").commands, vec!["tube.drive -6"]);
        assert_eq!(q("mix 50").commands, vec!["tube.mix 50"]);
        assert_eq!(q("trim -1.5").commands, vec!["tube.trim -1.5"]);
        assert_eq!(q("bias -20").commands, vec!["tube.bias -20"]);
        assert_eq!(q("asym 4").commands, vec!["tube.asym 4"]);
        assert_eq!(q("hard 70").commands, vec!["tube.hardness 70"]);
        assert_eq!(q("sag 30").commands, vec!["tube.sag 30"]);
        assert_eq!(q("rect solid").commands, vec!["tube.rectifier solid-state"]);
        assert_eq!(q("rect 5ar4").commands, vec!["tube.rectifier gz34"]);
        assert_eq!(q("rect 3").commands, vec!["tube.rectifier 5y3"]);
        assert_eq!(q("stage off").commands, vec!["tube.xfmr off"]);
        assert_eq!(q("damp 8").commands, vec!["tube.damping 8"]);
        assert_eq!(q("res 80").commands, vec!["tube.resonance 80"]);
        assert_eq!(q("type el34").hint, "Tube EL34");
        // Incomplete, out of range or ambiguous lines run nothing, and say
        // why.
        for partial in [
            "", "type", "type 12a", "type 17", "drive", "drive 30", "mix -1", "rect", "rect x",
            "stage", "s", "d", "r", "t",
        ] {
            let r = q(partial);
            assert!(r.commands.is_empty(), "{partial:?} ran {:?}", r.commands);
            assert!(!r.hint.is_empty(), "{partial:?}");
        }
        assert_eq!(q("drive 30").hint, "drive -30 to 24 dB (now -12 dB)");
        assert_eq!(q("hard").ghost.as_deref(), Some("ness"));
        let other = q("eq in.1 3 peak 1k -2");
        assert!(other.fallthrough && other.commands.is_empty());
    }

    /// The curve is the firmware's, so it has to keep the firmware's own
    /// promises: unity small-signal gain at every drive (tube.c:254-255), a
    /// rest point at zero, and the header's figures for the defaults and the
    /// output stage (tube.h:52-53, :62).
    #[test]
    fn the_shaper_keeps_the_firmwares_promises() {
        let s = state(true);
        let mut p = s.tube();
        // Unbiased: bias moves the operating point off the centre of the curve.
        p.bias_pct = 0.0;
        for drive in [-30.0, -12.0, 0.0, 24.0] {
            p.drive_db = drive;
            let sh = Shaper::new(&p);
            assert!(sh.output(0.0).abs() < 1e-12, "rest point at {drive}");
            let slope = (sh.output(1e-6) - sh.output(-1e-6)) / 2e-6;
            assert!((slope - 1.0).abs() < 1e-3, "unity gain at {drive}: {slope}");
        }

        // tube.h:62: the 12AX7 row at -12 dB drive gives about 0.23 % THD at
        // -12 dBFS.
        let sh = Shaper::new(&s.tube());
        let amp = 10f64.powf(-12.0 / 20.0);
        let n = 1024;
        let mut bins = [0.0f64; 8];
        for (k, bin) in bins.iter_mut().enumerate() {
            let (mut re, mut im) = (0.0, 0.0);
            for i in 0..n {
                let ph = 2.0 * std::f64::consts::PI * i as f64 / n as f64;
                let y = sh.output(amp * ph.sin());
                re += y * ((k + 1) as f64 * ph).cos();
                im += y * ((k + 1) as f64 * ph).sin();
            }
            *bin = re.hypot(im);
        }
        let thd = bins[1..].iter().map(|b| b * b).sum::<f64>().sqrt() / bins[0] * 100.0;
        assert!((0.2..0.26).contains(&thd), "THD {thd:.3} %");

        let (bell, top) = xfmr_lift(1.0);
        assert_eq!(format!("{bell:.1} {top:.1}"), "4.1 2.5");
        assert_eq!(format!("{:.1}", xfmr_lift(20.0).0), "0.3");

        // Pushed hard, both knees fall inside the swing and are marked.
        p = s.tube();
        p.drive_db = 15.0;
        let (neg, pos) = Shaper::new(&p).knees();
        assert!(neg.is_some() && pos.is_some());
        assert_eq!(Shaper::new(&s.tube()).knees(), (None, None));
    }

    #[test]
    fn an_old_firmware_gets_the_consoles_banner_instead_of_the_body() {
        let s = fixture::state();
        let mut p = TubePanel::new();
        let f = testing::draw(&mut p, &s, 100, 30);
        assert!(
            f.contains("Requires firmware with wire format V31 or newer."),
            "{f}"
        );
        assert!(
            f.contains("Update the DSPi firmware to use the Tube Modeller."),
            "{f}"
        );
        assert!(!f.contains("TRANSFER CURVE"), "{f}");
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &s),
            ScreenEvent::Unhandled
        );
    }

    #[test]
    fn the_header_switch_and_the_rows_write_their_parameters() {
        let s = state(true);
        let mut p = TubePanel::new();
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &s),
            ScreenEvent::Command("tube.on off".into())
        );
        focus(
            &mut p,
            &s,
            |r| matches!(r, Row::Param(p) if p.label == "Drive"),
        );
        assert_eq!(
            p.handle(key(KeyCode::Right), &s),
            ScreenEvent::Command("tube.drive -11.5".into())
        );
        assert_eq!(
            p.handle(key(KeyCode::Backspace), &s),
            ScreenEvent::Command("tube.drive -12".into())
        );
        focus(&mut p, &s, |r| matches!(r, Row::Segmented { .. }));
        assert_eq!(
            p.handle(key(KeyCode::Right), &s),
            ScreenEvent::Command("tube.rectifier 5u4".into())
        );
        focus(&mut p, &s, |r| matches!(r, Row::Chips { .. }));
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &s),
            ScreenEvent::Command("tube.mask 0xFFFE".into())
        );
    }

    #[test]
    fn every_advertised_key_is_handled_somewhere() {
        let s = state(true);
        for help in KEYS {
            for k in crate::screens::tests::keys_for(help.key) {
                let mut hit = false;
                for focus in 0..20 {
                    let mut p = TubePanel::new();
                    p.body.focus = focus;
                    let before = (p.body.focus, p.chip);
                    let ev = p.handle(k, &s);
                    if ev != ScreenEvent::Unhandled || (p.body.focus, p.chip) != before {
                        hit = true;
                        break;
                    }
                }
                assert!(hit, "{:?} from {:?} is not bound", k.code, help.key);
            }
        }
    }
}
