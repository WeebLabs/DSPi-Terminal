//! The Subharmonic Synthesizer panel: `SubharmonicSynthView.swift` as a tool
//! panel.
//!
//! The Console lays the window out in two columns; a terminal panel is one,
//! so the left column (the band graph, its headroom cost and the three
//! levels) comes first and the right column (selectivity, the sub ceiling,
//! the LF boost and the outputs) follows it, in the Console's order.
//!
//! Three things on it are not in the bulk packet and are never notified:
//! solo, the headroom cost and the sub meters. The runner reads them while
//! this panel is on screen (`Live::poll_subharm`) and stores them on
//! `DeviceState`, where this panel draws them from like everything else.
//! Solo is also switched off when the panel closes, as the Console's window
//! does, because it mutes the program on the masked outputs and the firmware
//! keeps it across a preset load.

use crossterm::event::{KeyCode, KeyEvent};
use dspi_proto::generated::ranges::{
    SUBHARM_BOOST_MAX, SUBHARM_BOOST_MIN, SUBHARM_CEILING_MAX, SUBHARM_CEILING_MIN,
    SUBHARM_DEFAULT_BOOST, SUBHARM_DEFAULT_CEILING, SUBHARM_DEFAULT_DEPTH, SUBHARM_DEFAULT_HIGH,
    SUBHARM_DEFAULT_HOLD_MS, SUBHARM_DEFAULT_LOW, SUBHARM_DEPTH_MAX, SUBHARM_DEPTH_MIN,
    SUBHARM_HOLD_MAX, SUBHARM_HOLD_MIN, SUBHARM_LEVEL_MAX, SUBHARM_LEVEL_MIN,
};
use dspi_proto::generated::subharm::{
    SUBHARM_SELECT_ALL, SUBHARM_SELECT_PERCUSSIVE, SUBHARM_SELECT_SUSTAINED,
};
use dspi_session::DeviceState;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::number;
use super::panel::{self, Body, ChipMeter, ChipSpec, Header, PanelGraph, Param, Row};
use crate::shell::{Screen, ScreenEvent};
use crate::theme::{ChannelRole, Theme};
use crate::widgets::{Action, ChipState, KeyHelp, PopupList, StatusTone};

/// `subharmStartingPoints`, `SubharmonicSynthView.swift:81-86`: name,
/// detail, then the 24-36 Hz level, the 36-56 Hz level and the boost. None
/// uses the 56-80 Hz band, so applying one sends that band to its floor
/// (`:334-339`).
const STARTING_POINTS: [(&str, &str, f32, f32, f32); 4] = [
    ("Subwoofer feed", "Subtle added weight", -6.0, -6.0, 0.0),
    ("Club / large PA", "Lower two bands, full", 0.0, 0.0, 3.0),
    ("Thin recordings", "Add a missing bottom", 0.0, -6.0, 3.0),
    ("Cinema LFE", "Lowest octave only", 3.0, -12.0, 0.0),
];

/// The Console's outputs menu, `SubharmonicSynthView.swift:574-583`.
const OUTPUT_PRESETS: [&str; 3] = ["Sub only (recommended)", "All outputs", "None"];

/// The three bands: the sub each adds, the program range it is derived from
/// an octave above (`SubharmonicSynthView.swift:356-389`, the band edges of
/// subharm.h:17-22), and its path.
const BANDS: [(&str, &str, &str); 3] = [
    ("24 - 36 Hz", "48 - 72 Hz", "sub.low"),
    ("36 - 56 Hz", "72 - 112 Hz", "sub.high"),
    ("56 - 80 Hz", "112 - 160 Hz", "sub.top"),
];

/// The selectivity picker, `SubharmonicSynthView.swift:444-446`, in
/// `SUBHARM_SELECT_*` order (subharm.h:71-73), with the registry's names.
const MODES: [(&str, &str); 3] = [
    ("All material", "all"),
    ("Percussive", "percussive"),
    ("Sustained", "sustained"),
];

const KEYS: &[KeyHelp] = &[
    KeyHelp::new("↑ ↓", "Move"),
    KeyHelp::new("← →", "Adjust"),
    KeyHelp::new("Enter", "Edit or choose"),
    KeyHelp::new("Space", "Toggle"),
    KeyHelp::new("s", "Solo"),
    KeyHelp::new("p", "Presets"),
    KeyHelp::new("Backspace", "Reset"),
];

/// The page grammar's verbs (DESIGN 13).
const VERBS: &[&str] = &[
    "on", "off", "solo", "low", "high", "top", "boost", "ceiling", "select", "depth", "hold",
    "link",
];
const SUMMARY: &str = "on · off · solo · low -6 · high 0 · top off · boost 3 · ceiling -12 · \
                       select percussive · depth 75 · hold 150 · link on";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    Outputs,
    StartingPoint,
}

pub struct SubharmPanel {
    body: Body,
    chip: usize,
    pending: Option<Pending>,
}

impl Default for SubharmPanel {
    fn default() -> Self {
        Self::new()
    }
}

/// A band level as the Console shows it: the floor is "Off"
/// (`SubharmonicSynthView.swift:418`).
fn level_display(db: f32) -> Option<&'static str> {
    (db <= SUBHARM_LEVEL_MIN).then_some("Off")
}

/// The ceiling at full scale limits nothing and is "Off" (`:527`).
fn ceiling_display(db: f32) -> Option<&'static str> {
    (db >= SUBHARM_CEILING_MAX).then_some("Off")
}

impl SubharmPanel {
    pub fn new() -> Self {
        Self {
            body: Body::default(),
            chip: 0,
            pending: None,
        }
    }

    /// The Console gates the window on wire V29 and its V30 parts on V30;
    /// the Terminal talks only to V32, so the probe's feature is the one gate.
    pub fn supported(state: &DeviceState) -> bool {
        panel::has_feature(state, "subharmonic_synth")
    }

    /// The line under the selectivity picker (`:492-498`).
    fn selectivity_summary(mode: u8) -> &'static str {
        match mode as u16 {
            SUBHARM_SELECT_PERCUSSIVE => {
                "A short sub burst after each attack - extends kicks, not the bass line."
            }
            SUBHARM_SELECT_SUSTAINED => {
                "The sub opens once a band has been ringing - extends bass notes, not kicks."
            }
            _ => "Every band signal is treated alike.",
        }
    }

    fn rows(&self, state: &DeviceState, theme: &Theme) -> Vec<Row> {
        if !Self::supported(state) {
            return vec![Row::Banner {
                title: "Requires firmware with wire format V29 or newer.".into(),
                body: "Update the DSPi firmware to use the Subharmonic Synthesizer.".into(),
            }];
        }
        let s = state.subharm();
        let levels = [s.low_db, s.high_db, s.top_db];
        let headroom = state.subharm_headroom_db;
        let mut rows = vec![
            Row::Menu {
                title: "Bands".into(),
                action: "Apply preset ▾".into(),
            },
            Row::Graph(if s.enabled {
                PanelGraph::Subharm {
                    levels: levels.map(|l| l as f64),
                    boost: s.boost_db as f64,
                    ceiling: s.ceiling_db as f64,
                }
            } else {
                PanelGraph::Disabled
            }),
            // `headroomReadout`, `:313-328`. Unknown until the runner's
            // first read, which is not the same as none.
            Row::Readout {
                title: "Headroom cost".into(),
                value: match headroom {
                    Some(db) if db > 0.0 => format!("{db:+.1} dB"),
                    Some(_) => "none".into(),
                    None => "-".into(),
                },
                tone: if headroom.is_some_and(|db| db > 0.0) {
                    StatusTone::Warning
                } else {
                    StatusTone::Neutral
                },
            },
            Row::Blank,
            Row::Section {
                title: "Levels".into(),
                action: None,
            },
        ];
        for ((title, source, _), db) in BANDS.iter().zip(levels) {
            rows.push(Row::Param(
                Param::new(
                    title,
                    db as f64,
                    SUBHARM_LEVEL_MIN as f64,
                    SUBHARM_LEVEL_MAX as f64,
                    "dB",
                )
                .step(0.5)
                .decimals(1)
                .caption(&format!("Derived from {source}"))
                .ends("Off", &format!("{:+.0} dB", SUBHARM_LEVEL_MAX))
                .display(level_display(db)),
            ));
        }

        rows.push(Row::Blank);
        rows.push(Row::Section {
            title: "Selectivity".into(),
            action: None,
        });
        rows.push(Row::Segmented {
            label: String::new(),
            choices: MODES.iter().map(|(c, _)| c.to_string()).collect(),
            selected: (s.select_mode as usize).min(MODES.len() - 1),
            enabled: true,
        });
        rows.push(Row::Caption(
            Self::selectivity_summary(s.select_mode).into(),
        ));
        // Depth and hold do nothing in "all material", so the Console hides
        // them there rather than showing them doing nothing (`:459-487`).
        if s.select_mode as u16 != SUBHARM_SELECT_ALL {
            rows.push(Row::Param(
                Param::new(
                    "Depth",
                    s.select_depth_pct as f64,
                    SUBHARM_DEPTH_MIN as f64,
                    SUBHARM_DEPTH_MAX as f64,
                    "%",
                )
                .step(5.0)
                .decimals(0),
            ));
            rows.push(Row::Param(
                Param::new(
                    "Hold",
                    s.select_hold_ms as f64,
                    SUBHARM_HOLD_MIN as f64,
                    SUBHARM_HOLD_MAX as f64,
                    "ms",
                )
                .step(10.0)
                .decimals(0),
            ));
        }

        rows.push(Row::Blank);
        rows.push(Row::Section {
            title: "Sub ceiling".into(),
            action: None,
        });
        rows.push(Row::Param(
            Param::new(
                "Threshold",
                s.ceiling_db as f64,
                SUBHARM_CEILING_MIN as f64,
                SUBHARM_CEILING_MAX as f64,
                "dB",
            )
            .step(1.0)
            .decimals(0)
            .ends("-40 dBFS", "Off")
            .display(ceiling_display(s.ceiling_db)),
        ));

        rows.push(Row::Blank);
        rows.push(Row::Section {
            title: "LF boost".into(),
            action: None,
        });
        rows.push(Row::Param(
            Param::new(
                "70 Hz bell",
                s.boost_db as f64,
                SUBHARM_BOOST_MIN as f64,
                SUBHARM_BOOST_MAX as f64,
                "dB",
            )
            .step(0.5)
            .decimals(1)
            .ends("Off", &format!("{:+.0} dB", SUBHARM_BOOST_MAX)),
        ));

        rows.push(Row::Blank);
        rows.push(Row::Section {
            title: "Outputs".into(),
            action: Some("Presets ▾".into()),
        });
        let outputs = state.caps.num_outputs as usize;
        let on = |o: usize| s.output_mask & (1 << o) != 0;
        rows.push(Row::Chips {
            chips: (0..outputs)
                .map(|o| ChipSpec {
                    label: (o + 1).to_string(),
                    state: if on(o) { ChipState::On } else { ChipState::Off },
                    color: theme.role_color(ChannelRole::of(
                        state.caps.num_inputs + o as u8,
                        state.caps.num_inputs,
                        state.caps.num_outputs,
                    )),
                    enabled: true,
                    dimmed: false,
                })
                .collect(),
            cursor: self.chip,
            polarity: false,
        });
        // The bar under each chip is the synthesized sub alone, and a
        // masked-off output is never metered, so its bar stays empty
        // (`SubharmChipsPane`, `:724-729`; `SubharmLiveMeterBar`).
        rows.push(Row::Meters(
            (0..outputs)
                .map(|o| ChipMeter {
                    label: (o + 1).to_string(),
                    fraction: match (&state.subharm_meter, on(o) && s.enabled) {
                        (Some(m), true) => m.peak(o) as f64,
                        _ => 0.0,
                    },
                    on: on(o),
                })
                .collect(),
        ));
        // The chips' tooltip (`:609`), where `DESIGN.md` 7.8 puts a caption.
        rows.push(Row::Caption(
            "Select the outputs that can actually play 24 to 80 Hz. Subharm runs before the \
             crossover, so a satellite with a highpass loses the sub again - mask it off and \
             save the CPU instead."
                .into(),
        ));
        rows.push(Row::Toggle {
            label: "Link output pairs".into(),
            on: s.link_pairs,
            caption: Some("One sub per pair, from its mono sum.".into()),
            enabled: true,
        });
        rows
    }

    fn path_for(label: &str) -> &'static str {
        if let Some((_, _, path)) = BANDS.iter().find(|(title, ..)| *title == label) {
            return path;
        }
        match label {
            "Depth" => "sub.depth",
            "Hold" => "sub.hold",
            "Threshold" => "sub.ceiling",
            _ => "sub.boost",
        }
    }

    fn param_command(label: &str, value: f64) -> ScreenEvent {
        ScreenEvent::Command(format!(
            "{} {}",
            Self::path_for(label),
            number(value as f32)
        ))
    }

    /// The firmware's defaults, subharm.h:85-94; the Console's are the same
    /// (`ToolParameters.swift:174-192`).
    fn default_for(label: &str) -> f32 {
        match label {
            "24 - 36 Hz" => SUBHARM_DEFAULT_LOW,
            "36 - 56 Hz" => SUBHARM_DEFAULT_HIGH,
            // SUBHARM_DEFAULT_TOP is SUBHARM_LEVEL_MIN: the third band ships
            // off (subharm.h:87).
            "56 - 80 Hz" => SUBHARM_LEVEL_MIN,
            "Depth" => SUBHARM_DEFAULT_DEPTH,
            "Hold" => SUBHARM_DEFAULT_HOLD_MS,
            "Threshold" => SUBHARM_DEFAULT_CEILING,
            _ => SUBHARM_DEFAULT_BOOST,
        }
    }

    /// Solo is only offered while the module runs (`:242`).
    fn solo(state: &DeviceState) -> ScreenEvent {
        if !state.subharm().enabled {
            return ScreenEvent::Status("Solo needs the Subharmonic Synthesizer on".into());
        }
        let on = state.subharm_solo.unwrap_or(false);
        ScreenEvent::Command(format!("sub.solo {}", if on { "off" } else { "on" }))
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
            (Row::Chips { .. }, Action::Selected(i)) => {
                self.chip = i;
                ScreenEvent::Handled
            }
            (Row::Chips { .. }, Action::Chip(i, next)) => {
                let bit = 1u16 << i;
                let mask = state.subharm().output_mask;
                let mask = if next == ChipState::On {
                    mask | bit
                } else {
                    mask & !bit
                };
                ScreenEvent::Command(format!("sub.mask 0x{mask:X}"))
            }
            (Row::Chips { .. }, Action::Open) => {
                self.pending = Some(Pending::Outputs);
                ScreenEvent::Popup(PopupList::new(
                    "Presets",
                    OUTPUT_PRESETS.iter().map(|s| s.to_string()).collect(),
                    0,
                ))
            }
            (Row::Segmented { .. }, Action::Selected(i)) => {
                ScreenEvent::Command(format!("sub.select {}", MODES[i.min(MODES.len() - 1)].1))
            }
            (Row::Toggle { .. }, Action::Toggled(next)) => {
                ScreenEvent::Command(format!("sub.link {}", if next { "on" } else { "off" }))
            }
            (Row::Param(p), Action::Changed(v)) => Self::param_command(&p.label, v),
            (Row::Param(p), Action::Reset) => {
                Self::param_command(&p.label, Self::default_for(&p.label) as f64)
            }
            _ => ScreenEvent::Handled,
        }
    }

    /// The page grammar (DESIGN 13): one word per control, each taking the
    /// value its row would.
    fn quick_reply(&self, line: &str, state: &DeviceState) -> crate::shell::Quick {
        use super::quick::{ghost, number as num, verb};
        use crate::shell::Quick;
        let lower = line.to_ascii_lowercase();
        let tokens: Vec<&str> = lower.split_whitespace().collect();
        let hint = |h: &str| Quick {
            fallthrough: false,
            hint: h.to_string(),
            ghost: ghost(&lower, VERBS),
            commands: Vec::new(),
        };
        let one = |hint: String, command: String| Quick {
            fallthrough: false,
            hint,
            ghost: None,
            commands: vec![command],
        };
        let Some(&first) = tokens.first() else {
            return hint(SUMMARY);
        };
        let arg = tokens.get(1).copied();
        let switch = |arg: Option<&str>, now: bool| match arg {
            None => Some(!now),
            Some(a) => match verb(a, &["on", "off"]) {
                Some("on") => Some(true),
                Some("off") => Some(false),
                _ => None,
            },
        };
        let s = state.subharm();
        // A level: a number in the band's range, or `off` for the floor.
        let level = |path: &str, name: &str| match arg {
            Some("off") => one(format!("{name} off"), format!("{path} -30")),
            Some(t) => match num(t) {
                Some(db) => {
                    let db = db.clamp(SUBHARM_LEVEL_MIN as f64, SUBHARM_LEVEL_MAX as f64);
                    if db <= SUBHARM_LEVEL_MIN as f64 {
                        one(format!("{name} off"), format!("{path} -30"))
                    } else {
                        one(
                            format!("{name} {db:+.1} dB"),
                            format!("{path} {}", number(db as f32)),
                        )
                    }
                }
                None => hint(&format!("{name}: -30 (off) to +12 dB, or off")),
            },
            None => hint(&format!("{name}: -30 (off) to +12 dB, or off")),
        };
        let ranged = |path: &str, what: &str, lo: f32, hi: f32, unit: &str| match arg.and_then(num)
        {
            Some(v) => {
                let v = v.clamp(lo as f64, hi as f64);
                one(
                    format!("{what} {} {unit}", number(v as f32)),
                    format!("{path} {}", number(v as f32)),
                )
            }
            None => hint(&format!("{what}: {} to {} {unit}", number(lo), number(hi))),
        };
        match verb(first, VERBS) {
            Some("on") => one("Subharmonic Synthesizer on".into(), "sub.on on".into()),
            Some("off") => one("Subharmonic Synthesizer off".into(), "sub.on off".into()),
            Some("solo") => {
                if !s.enabled {
                    return hint("solo needs the synthesizer on");
                }
                match switch(arg, state.subharm_solo.unwrap_or(false)) {
                    Some(v) => one(
                        format!("solo {}", if v { "on" } else { "off" }),
                        format!("sub.solo {}", if v { "on" } else { "off" }),
                    ),
                    None => hint("solo [on · off]"),
                }
            }
            Some("low") => level("sub.low", "24 - 36 Hz"),
            Some("high") => level("sub.high", "36 - 56 Hz"),
            Some("top") => level("sub.top", "56 - 80 Hz"),
            Some("boost") => match arg {
                Some("off") => one("70 Hz bell off".into(), "sub.boost 0".into()),
                _ => ranged(
                    "sub.boost",
                    "70 Hz bell",
                    SUBHARM_BOOST_MIN,
                    SUBHARM_BOOST_MAX,
                    "dB",
                ),
            },
            Some("ceiling") => match arg {
                Some("off") => one("sub ceiling off".into(), "sub.ceiling 0".into()),
                Some(t) if num(t).is_some_and(|v| v >= SUBHARM_CEILING_MAX as f64) => {
                    one("sub ceiling off".into(), "sub.ceiling 0".into())
                }
                _ => ranged(
                    "sub.ceiling",
                    "sub ceiling",
                    SUBHARM_CEILING_MIN,
                    SUBHARM_CEILING_MAX,
                    "dBFS",
                ),
            },
            Some("select") => {
                let names: Vec<&str> = MODES.iter().map(|(_, n)| *n).collect();
                match arg.and_then(|a| verb(a, &names)) {
                    Some(m) => {
                        let label = MODES.iter().find(|(_, n)| *n == m).unwrap().0;
                        one(format!("selectivity: {label}"), format!("sub.select {m}"))
                    }
                    None => hint("select all · percussive · sustained"),
                }
            }
            Some("depth") => ranged(
                "sub.depth",
                "depth",
                SUBHARM_DEPTH_MIN,
                SUBHARM_DEPTH_MAX,
                "%",
            ),
            Some("hold") => ranged("sub.hold", "hold", SUBHARM_HOLD_MIN, SUBHARM_HOLD_MAX, "ms"),
            Some("link") => match switch(arg, s.link_pairs) {
                Some(v) => one(
                    format!("link output pairs {}", if v { "on" } else { "off" }),
                    format!("sub.link {}", if v { "on" } else { "off" }),
                ),
                None => hint("link [on · off]"),
            },
            _ => Quick {
                fallthrough: true,
                hint: SUMMARY.to_string(),
                ghost: ghost(&lower, VERBS),
                commands: Vec::new(),
            },
        }
    }
}

impl Screen for SubharmPanel {
    fn quick(&self, line: &str, state: &DeviceState) -> Option<crate::shell::Quick> {
        Some(self.quick_reply(line, state))
    }

    fn title(&self) -> String {
        "Subharmonic Synthesizer".into()
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
        let supported = Self::supported(state);
        let enabled = state.subharm().enabled;
        let mut header =
            Header::new("Generates a subharmonic at half the frequency of the source's bass")
                .toggle(enabled)
                .enabled(supported);
        if supported {
            // The latching SOLO button: orange while on, disabled unless the
            // module is (`soloButton`, `:229-248`).
            let style = if state.subharm_solo == Some(true) {
                theme.pill(theme.warning)
            } else if enabled {
                theme.label()
            } else {
                Style::default().fg(theme.chrome_faint)
            };
            header = header.badge(" SOLO ", style);
        }
        self.body.draw(area, buf, theme, &header, &rows, focused);
    }

    fn handle(&mut self, key: KeyEvent, state: &DeviceState) -> ScreenEvent {
        if !Self::supported(state) {
            return ScreenEvent::Unhandled;
        }
        let rows = self.rows(state, panel::key_theme());
        self.body.clamp(&rows);
        // `s` is solo wherever the cursor is, unless a field is being typed.
        if key.code == KeyCode::Char('s') && self.body.edit.is_none() {
            return Self::solo(state);
        }
        match panel::dispatch(&mut self.body, &rows, key) {
            panel::Step::Done(ev) => ev,
            panel::Step::Header => ScreenEvent::Command(format!(
                "sub.on {}",
                if state.subharm().enabled { "off" } else { "on" }
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
                let mask = match i {
                    // The PDM sub's bit (`subOnlyMask`, `:122-124`).
                    0 => 1u16 << panel::sub_output(state),
                    1 => panel::all_outputs_mask(state),
                    _ => 0x0000,
                };
                ScreenEvent::Command(format!("sub.mask 0x{mask:X}"))
            }
            (Some(Pending::StartingPoint), Some(i)) => {
                let (_, _, low, high, boost) = STARTING_POINTS[i.min(STARTING_POINTS.len() - 1)];
                ScreenEvent::Command(
                    [
                        format!("sub.low {}", number(low)),
                        format!("sub.high {}", number(high)),
                        format!("sub.boost {}", number(boost)),
                        format!("sub.top {}", number(SUBHARM_LEVEL_MIN)),
                    ]
                    .join("\n"),
                )
            }
            _ => ScreenEvent::Handled,
        }
    }
}
