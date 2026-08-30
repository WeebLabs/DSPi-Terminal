//! The Stereo Upmixer panel: `UpmixerView.swift` as a tool panel.
//!
//! The only panel with live telemetry: the correlation and the three steering
//! gains come from `REQ_UPMIX_GET_STATUS`, which the runner polls once a
//! second into [`DeviceState::upmix_status`] because the notification
//! endpoint does not carry them. Without a reading the status line falls back
//! to what the configuration implies, which is what the Console shows before
//! its first fetch lands.
//!
//! Sections appear and disappear with the engines: an engine that is off has
//! nothing to configure, so its whole block goes, header and all.

use crossterm::event::KeyEvent;
use dspi_session::DeviceState;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::number;
use super::panel::{self, Body, Header, Param, Row};
use crate::shell::{Screen, ScreenEvent};
use crate::theme::Theme;
use crate::widgets::{Action, KeyHelp, StatusTone};

/// `UPMIX_CENTER_MODE_*`: the wire values in the Console's UI order, which is
/// Off first even though Off is 2 on the wire.
const CENTRE_MODES: [(&str, &str); 3] =
    [("Off", "off"), ("Sinner", "passive"), ("Logician", "logic")];
/// `UPMIX_SURROUND_MODE_*`: here the wire order and the UI order agree.
const SURROUND_MODES: [(&str, &str); 3] =
    [("Off", "off"), ("Sinner", "passive"), ("Logician", "logic")];

const CENTRE_OFF: u8 = 2;
const CENTRE_LOGIC: u8 = 1;
const SURROUND_OFF: u8 = 0;

const KEYS: &[KeyHelp] = &[
    KeyHelp::new("↑ ↓", "Move"),
    KeyHelp::new("← →", "Adjust"),
    KeyHelp::new("Enter", "Edit"),
    KeyHelp::new("Space", "Toggle"),
    KeyHelp::new("Backspace", "Reset"),
];

pub struct UpmixerPanel {
    body: Body,
}

impl Default for UpmixerPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl UpmixerPanel {
    pub fn new() -> Self {
        Self {
            body: Body::default(),
        }
    }

    /// The whole feature needs an RP2350 running wire format V25 or newer.
    pub fn supported(state: &DeviceState) -> bool {
        panel::has_feature(state, "upmixer")
    }

    /// The Console's `statusText`, from the telemetry when there is any and
    /// from the configuration when there is not.
    ///
    /// `UpmixerView.swift:203-217`: the connection is the first question asked,
    /// because every other answer would be a claim about a device that is not
    /// there. Its dot is `.secondary`, so the tone is neutral, not a warning.
    fn status(state: &DeviceState) -> (String, StatusTone) {
        if !state.connected {
            return ("No device connected".into(), StatusTone::Neutral);
        }
        let u = state.upmix();
        match &state.upmix_status {
            Some(s) if s.active => ("Active - processing audio".into(), StatusTone::Ok),
            Some(s) => (
                match s.parked_reason {
                    1 => "Idle: upmixer disabled",
                    2 => "Idle: input is not stereo",
                    3 => "Idle: sample rate above 48 kHz",
                    _ => "Idle",
                }
                .into(),
                StatusTone::Warning,
            ),
            // No reading yet: say what the configuration implies rather than
            // claiming a state the device has not reported.
            None if !u.enabled => ("Idle: upmixer disabled".into(), StatusTone::Warning),
            None => ("Idle".into(), StatusTone::Warning),
        }
    }

    fn rows(&self, state: &DeviceState, theme: &Theme) -> Vec<Row> {
        if !Self::supported(state) {
            return vec![Row::Banner {
                title: "Requires an RP2350 device with firmware wire format V25 or newer.".into(),
                body: "The upmixer runs on stereo input at 48 kHz or below.".into(),
            }];
        }
        let u = state.upmix();
        let centre_off = u.center_mode == CENTRE_OFF;
        let surround_on = u.surround_mode != SURROUND_OFF;
        let (text, tone) = Self::status(state);

        let mut rows = vec![
            Row::Section {
                title: "Status".into(),
                action: None,
            },
            Row::Status { text, tone },
        ];
        if let Some(s) = state.upmix_status.as_ref().filter(|s| s.active) {
            let (centre, ls, rs) = s.gains();
            let corr = s.correlation();
            rows.push(Row::Gauge {
                label: "Correlation".into(),
                fraction: ((corr + 1.0) / 2.0) as f64,
                display: format!("{corr:+.2}"),
                color: theme.accent,
            });
            if !centre_off {
                rows.push(Row::Gauge {
                    label: "Centre gain".into(),
                    fraction: centre as f64,
                    display: format!("{:.0}%", centre * 100.0),
                    color: theme.ok,
                });
            }
            if surround_on {
                rows.push(Row::Gauge {
                    label: "Ls gain".into(),
                    fraction: ls as f64,
                    display: format!("{:.0}%", ls * 100.0),
                    color: theme.inputs[4],
                });
                rows.push(Row::Gauge {
                    label: "Rs gain".into(),
                    fraction: rs as f64,
                    display: format!("{:.0}%", rs * 100.0),
                    color: theme.inputs[5],
                });
            }
        }

        rows.push(Row::Blank);
        rows.push(Row::Section {
            title: "Engines".into(),
            action: None,
        });
        rows.push(Row::Segmented {
            label: "Centre".into(),
            choices: CENTRE_MODES.iter().map(|(n, _)| n.to_string()).collect(),
            selected: match u.center_mode {
                CENTRE_OFF => 0,
                0 => 1,
                _ => 2,
            },
            enabled: true,
        });
        rows.push(Row::Segmented {
            label: "Surround".into(),
            choices: SURROUND_MODES.iter().map(|(n, _)| n.to_string()).collect(),
            selected: u.surround_mode.min(2) as usize,
            enabled: true,
        });
        rows.push(Row::Caption(
            "Logician centre gates extraction on running L/R correlation; Logician surround \
             uses a Pro Logic II-style matrix decoder. Sinner modes are fixed (C = 0.7071(L+R), \
             surround = L-R) - a Hafler-style passive matrix like the one in the Schiit Syn."
                .into(),
        ));

        if !centre_off {
            rows.push(Row::Blank);
            rows.push(Row::Section {
                title: "Centre".into(),
                action: None,
            });
            rows.push(Row::Param(
                Param::new("Strength", u.strength as f64, 0.0, 100.0, "%")
                    .step(1.0)
                    .decimals(0)
                    .caption(
                        "Centre extraction strength; scales both the C output and how much \
                         centre energy is removed from L/R. In Sinner mode this is the fixed \
                         centre gain.",
                    ),
            ));
            rows.push(Row::Param(
                Param::new("Centre Width", u.center_width as f64, 0.0, 100.0, "%")
                    .step(1.0)
                    .decimals(0)
                    .caption(
                        "How much extracted centre stays in L/R. 0 = full removal (discrete \
                         centre); 100 = L/R untouched (expect combing if a real centre speaker \
                         plays).",
                    ),
            ));
            rows.push(Row::Param(
                Param::new("Presence", u.presence_db as f64, -12.0, 12.0, "dB")
                    .step(0.5)
                    .decimals(1)
                    .caption(
                        "Voice presence bell at 3 kHz (Q 0.6). Positive brings voices forward, \
                         negative pushes them back (Syn-style). Stored in 0.5 dB steps.",
                    ),
            ));
            if u.center_mode == CENTRE_LOGIC {
                rows.push(Row::Param(
                    Param::new("Correlation Threshold", u.threshold as f64, 0.0, 95.0, "%")
                        .step(1.0)
                        .decimals(0)
                        .caption(
                            "Correlation gate. Below this, nothing is extracted; above it, \
                             extraction scales up to full. Raise to extract only \
                             strongly-correlated content.",
                        ),
                ));
                rows.push(Row::Param(
                    Param::new("Attack", u.attack_ms as f64, 1.0, 500.0, "ms")
                        .step(1.0)
                        .decimals(0)
                        .caption("Centre gain rise time (Logician mode)."),
                ));
                rows.push(Row::Param(
                    Param::new("Release", u.release_ms as f64, 5.0, 2000.0, "ms")
                        .step(5.0)
                        .decimals(0)
                        .caption("Centre gain fall time (Logician mode)."),
                ));
                rows.push(Row::Param(
                    Param::new("Detector HPF", u.det_hpf_hz as f64, 20.0, 1000.0, "Hz")
                        .step(5.0)
                        .decimals(0)
                        .caption(
                            "Detector bass-cut corner. Content below this is ignored by the \
                             steering detector (the audio itself is not filtered) so bass does \
                             not pump the centre.",
                        ),
                ));
            }
        }

        if surround_on {
            rows.push(Row::Blank);
            rows.push(Row::Section {
                title: "Surround".into(),
                action: None,
            });
            rows.push(Row::Param(
                Param::new("Delay", u.sur_delay_ms as f64, 0.0, 20.0, "ms")
                    .step(0.5)
                    .decimals(1)
                    .caption(
                        "Haas delay on Ls/Rs (precedence effect). Rule of thumb ~1 ms per foot \
                         of listener distance.",
                    ),
            ));
            rows.push(Row::Param(
                Param::new("Band-limit HPF", u.sur_hpf_hz as f64, 20.0, 2000.0, "Hz")
                    .step(5.0)
                    .decimals(0)
                    .caption("Surround high-pass; keeps rumble out of the rears."),
            ));
            rows.push(Row::Param(
                Param::new("Band-limit LPF", u.sur_lpf_hz as f64, 1000.0, 20000.0, "Hz")
                    .step(100.0)
                    .decimals(0)
                    .caption(
                        "Surround low-pass. 7 kHz is the classic surround voicing; raise for \
                         full-band rears.",
                    ),
            ));
            rows.push(Row::Param(
                Param::new("Decorrelation", u.decorr as f64, 0.0, 100.0, "%")
                    .step(1.0)
                    .decimals(0)
                    .caption("Schroeder allpass decorrelator amount. 0 disables decorrelation."),
            ));
        }

        rows.push(Row::Blank);
        rows.push(Row::Section {
            title: "Routing".into(),
            action: None,
        });
        rows.push(Row::Caption(
            "The derived channels appear as matrix source rows: row 2 = Centre, row 3 = Left \
             Surround, row 4 = Right Surround. Open the Matrix Mixer to route them to your \
             output slots (a centre crosspoint gain of -3 dB is a safe start, since the centre \
             row can reach +3 dBFS)."
                .into(),
        ));
        rows
    }

    fn path_for(label: &str) -> &'static str {
        match label {
            "Strength" => "up.strength",
            "Centre Width" => "up.width",
            "Presence" => "up.presence",
            "Correlation Threshold" => "up.threshold",
            "Attack" => "up.attack",
            "Release" => "up.release",
            "Detector HPF" => "up.det_hpf",
            "Delay" => "up.sur_delay",
            "Band-limit HPF" => "up.sur_hpf",
            "Band-limit LPF" => "up.sur_lpf",
            _ => "up.decorr",
        }
    }

    fn param_command(label: &str, value: f64) -> ScreenEvent {
        ScreenEvent::Command(format!(
            "{} {}",
            Self::path_for(label),
            number(value as f32)
        ))
    }

    fn act(&mut self, row: &Row, action: Action) -> ScreenEvent {
        match (row, action) {
            (Row::Segmented { label, .. }, Action::Selected(i)) => {
                let (path, table) = if label == "Centre" {
                    ("up.center_mode", &CENTRE_MODES)
                } else {
                    ("up.surround_mode", &SURROUND_MODES)
                };
                let token = table[i.min(table.len() - 1)].1;
                ScreenEvent::Command(format!("{path} {token}"))
            }
            (Row::Param(p), Action::Changed(v)) => Self::param_command(&p.label, v),
            (Row::Param(p), Action::Reset) => {
                // The upmixer's defaults, `upmix.h`, as the survey lists them.
                let v = match p.label.as_str() {
                    "Strength" => 70.0,
                    "Centre Width" => 0.0,
                    "Presence" => 0.0,
                    "Correlation Threshold" => 30.0,
                    "Attack" => 20.0,
                    "Release" => 200.0,
                    "Detector HPF" => 120.0,
                    "Delay" => 10.0,
                    "Band-limit HPF" => 100.0,
                    "Band-limit LPF" => 7000.0,
                    _ => 0.0,
                };
                Self::param_command(&p.label, v)
            }
            _ => ScreenEvent::Handled,
        }
    }
}

impl Screen for UpmixerPanel {
    fn quick(&self, line: &str, _state: &DeviceState) -> Option<crate::shell::Quick> {
        Some(super::quick::toggle(line, "up.on", "Upmixer"))
    }

    fn title(&self) -> String {
        "Stereo Upmixer".into()
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
        let header = Header::new("Derive Centre and Surround from stereo")
            .toggle(state.upmix().enabled)
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
                "up.on {}",
                if state.upmix().enabled { "off" } else { "on" }
            )),
            panel::Step::Commit(i, v) => match &rows[i] {
                Row::Param(p) => Self::param_command(&p.label, v.clamp(p.min, p.max)),
                _ => ScreenEvent::Handled,
            },
            panel::Step::Row(i, action) => self.act(&rows[i], action),
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
    use crate::widgets::testing::key;
    use crossterm::event::KeyCode;
    use dspi_proto::packets::UpmixStatus;

    fn section(name: &str) -> usize {
        dspi_proto::generated::SECTIONS
            .iter()
            .find(|(n, _, _)| *n == name)
            .map(|(_, o, _)| *o)
            .expect("section")
    }

    /// The upmixer on, with the Logician centre and the Sinner surround, so
    /// every conditional section is present.
    fn panel() -> (UpmixerPanel, DeviceState) {
        let mut state = testing::state();
        let u = section("upmix");
        state.bulk.patch(u, &[1, 1, 1, 0]);
        state.bulk.patch(u + 4, &70.0f32.to_le_bytes());
        state.bulk.patch(u + 8, &0.0f32.to_le_bytes());
        state.bulk.patch(u + 12, &30.0f32.to_le_bytes());
        state.bulk.patch(u + 16, &20.0f32.to_le_bytes());
        state.bulk.patch(u + 20, &200.0f32.to_le_bytes());
        state.bulk.patch(u + 24, &120.0f32.to_le_bytes());
        state.bulk.patch(u + 28, &10.0f32.to_le_bytes());
        state.bulk.patch(u + 32, &100.0f32.to_le_bytes());
        state.bulk.patch(u + 36, &7000.0f32.to_le_bytes());
        state.bulk.patch(u + 40, &0.0f32.to_le_bytes());
        (UpmixerPanel::new(), state)
    }

    #[test]
    fn the_panel_carries_the_status_engines_and_both_parameter_blocks() {
        let (mut p, state) = panel();
        let f = testing::draw(&mut p, &state, 100, 60);
        assert!(f.contains("Derive Centre and Surround from stereo"), "{f}");
        assert!(f.contains("STATUS"), "{f}");
        assert!(f.contains("ENGINES"), "{f}");
        assert!(f.contains("Sinner") && f.contains("Logician"), "{f}");
        assert!(f.contains("CENTRE") && f.contains("Strength"), "{f}");
        assert!(
            f.contains("Correlation Threshold"),
            "the Logician block: {f}"
        );
        assert!(f.contains("SURROUND") && f.contains("Decorrelation"), "{f}");
        assert!(f.contains("ROUTING"), "{f}");
        assert!(f.contains("row 2 = Centre"), "the routing note: {f}");
    }

    #[test]
    fn an_engine_that_is_off_takes_its_whole_block_with_it() {
        let (mut p, mut state) = panel();
        let u = section("upmix");
        // Centre off (wire 2), surround off (wire 0).
        state.bulk.patch(u + 1, &[2, 0]);
        let f = testing::draw(&mut p, &state, 100, 60);
        assert!(!f.contains("Strength"), "{f}");
        assert!(!f.contains("Decorrelation"), "{f}");
        assert!(f.contains("ENGINES"), "the engines stay: {f}");
        // A Sinner centre keeps Strength but drops the Logician steering.
        state.bulk.patch(u + 1, &[0, 0]);
        let f = testing::draw(&mut p, &state, 100, 60);
        assert!(f.contains("Strength") && f.contains("Presence"), "{f}");
        assert!(!f.contains("Correlation Threshold"), "{f}");
    }

    #[test]
    fn the_live_gauges_appear_only_with_a_reading_that_says_active() {
        let (mut p, mut state) = panel();
        let f = testing::draw(&mut p, &state, 100, 60);
        assert!(!f.contains("Ls gain"), "no telemetry yet: {f}");
        state.upmix_status = Some(UpmixStatus {
            active: true,
            parked_reason: 0,
            // 0.5 correlation, and three steering gains.
            corr_q14: 8192,
            balance_q14: 0,
            center_gain_q15: 16384,
            ls_gain_q15: 8192,
            rs_gain_q15: 24576,
        });
        let f = testing::draw(&mut p, &state, 100, 60);
        assert!(f.contains("Active - processing audio"), "{f}");
        assert!(f.contains("Correlation") && f.contains("+0.50"), "{f}");
        assert!(f.contains("Centre gain") && f.contains("50%"), "{f}");
        assert!(f.contains("Ls gain") && f.contains("Rs gain"), "{f}");
    }

    #[test]
    fn a_parked_reading_says_why_in_the_consoles_words() {
        let (mut p, mut state) = panel();
        for (reason, text) in [
            (1u8, "Idle: upmixer disabled"),
            (2, "Idle: input is not stereo"),
            (3, "Idle: sample rate above 48 kHz"),
        ] {
            state.upmix_status = Some(UpmixStatus {
                active: false,
                parked_reason: reason,
                ..UpmixStatus::default()
            });
            let f = testing::draw(&mut p, &state, 100, 60);
            assert!(f.contains(text), "{reason}: {f}");
        }
    }

    /// D48 asked whether all four gauges should show whenever the status is
    /// active. They should not: `UpmixerView.swift:186-197` gates Centre gain
    /// on `!centreOff` and the two surround gains on `surroundOn`, exactly as
    /// here. Only Correlation is unconditional.
    #[test]
    fn a_gauge_goes_with_the_engine_that_feeds_it() {
        let (mut p, mut state) = panel();
        state.upmix_status = Some(UpmixStatus {
            active: true,
            parked_reason: 0,
            corr_q14: 8192,
            balance_q14: 0,
            center_gain_q15: 16384,
            ls_gain_q15: 8192,
            rs_gain_q15: 24576,
        });
        let f = testing::draw(&mut p, &state, 100, 60);
        for want in ["Correlation", "Centre gain", "Ls gain", "Rs gain"] {
            assert!(f.contains(want), "{want} with both engines on:\n{f}");
        }

        // Centre off (wire 2), surround off (wire 0).
        let u = section("upmix");
        state.bulk.patch(u + 1, &[2, 0]);
        let f = testing::draw(&mut p, &state, 100, 60);
        assert!(f.contains("Correlation"), "always shown: {f}");
        for gone in ["Centre gain", "Ls gain", "Rs gain"] {
            assert!(!f.contains(gone), "{gone} has no engine behind it:\n{f}");
        }
    }

    /// D9: `UpmixerView.swift:204` asks about the connection before anything
    /// else, so a device that has gone away never reads as "Idle" or, worse, as
    /// "Active - processing audio" off a stale reading.
    #[test]
    fn a_missing_device_says_so_before_it_says_anything_else() {
        let (mut p, mut state) = panel();
        state.upmix_status = Some(UpmixStatus {
            active: true,
            parked_reason: 0,
            corr_q14: 8192,
            balance_q14: 0,
            center_gain_q15: 16384,
            ls_gain_q15: 8192,
            rs_gain_q15: 24576,
        });
        let f = testing::draw(&mut p, &state, 100, 60);
        assert!(f.contains("Active - processing audio"), "{f}");

        state.connected = false;
        let f = testing::draw(&mut p, &state, 100, 60);
        assert!(f.contains("No device connected"), "{f}");
        assert!(!f.contains("Active - processing audio"), "{f}");
        assert_eq!(
            UpmixerPanel::status(&state).1,
            StatusTone::Neutral,
            "the Console's dot is .secondary here, not orange"
        );
    }

    #[test]
    fn an_unsupported_device_gets_the_consoles_two_lines() {
        let state = crate::shell::fixture::state();
        let mut p = UpmixerPanel::new();
        let f = testing::draw(&mut p, &state, 100, 40);
        assert!(
            f.contains("Requires an RP2350 device with firmware wire format V25 or newer."),
            "{f}"
        );
        assert!(
            f.contains("The upmixer runs on stereo input at 48 kHz or below."),
            "{f}"
        );
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &state),
            ScreenEvent::Unhandled
        );
    }

    #[test]
    fn golden_frames_at_both_sizes() {
        let (_, state) = panel();
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            let f = testing::frame(Tool::Upmixer, Box::new(UpmixerPanel::new()), &state, w, h);
            assert_eq!(f.lines().count(), h as usize, "{w}x{h}");
            assert!(f.contains("Stereo Upmixer"), "{w}x{h}:\n{f}");
            assert!(f.contains("U closes"), "{w}x{h}:\n{f}");
        }
    }

    #[test]
    fn the_engines_write_the_wire_names_not_the_ui_order() {
        let (mut p, state) = panel();
        let rows = p.rows(&state, panel::key_theme());
        let segs: Vec<usize> = panel::focus_rows(&rows)
            .iter()
            .enumerate()
            .filter(|(_, r)| matches!(rows[**r], Row::Segmented { .. }))
            .map(|(i, _)| i + 1)
            .collect();
        p.body.focus = segs[0];
        // Centre sits on Logician (index 2); left is Sinner, which is wire 0.
        assert_eq!(
            p.handle(key(KeyCode::Left), &state),
            ScreenEvent::Command("up.center_mode passive".into())
        );
        p.body.focus = segs[1];
        assert_eq!(
            p.handle(key(KeyCode::Right), &state),
            ScreenEvent::Command("up.surround_mode logic".into())
        );
    }

    #[test]
    fn the_parameters_write_their_registry_paths() {
        let (mut p, state) = panel();
        let rows = p.rows(&state, panel::key_theme());
        let params: Vec<usize> = panel::focus_rows(&rows)
            .iter()
            .enumerate()
            .filter(|(_, r)| matches!(rows[**r], Row::Param(_)))
            .map(|(i, _)| i + 1)
            .collect();
        p.body.focus = params[0];
        assert_eq!(
            p.handle(key(KeyCode::Right), &state),
            ScreenEvent::Command("up.strength 71".into())
        );
        p.body.focus = params[2];
        assert_eq!(
            p.handle(key(KeyCode::Right), &state),
            ScreenEvent::Command("up.presence 0.5".into())
        );
        p.body.focus = params[6];
        assert_eq!(
            p.handle(key(KeyCode::Right), &state),
            ScreenEvent::Command("up.det_hpf 125".into())
        );
        p.body.focus = *params.last().unwrap();
        assert_eq!(
            p.handle(key(KeyCode::Right), &state),
            ScreenEvent::Command("up.decorr 1".into())
        );
    }

    #[test]
    fn every_advertised_key_is_handled_somewhere() {
        let (_, state) = panel();
        for help in KEYS {
            for k in crate::screens::tests::keys_for(help.key) {
                let mut hit = false;
                for focus in 0..8 {
                    let mut p = UpmixerPanel::new();
                    p.body.focus = focus;
                    let before = p.body.focus;
                    let ev = p.handle(k, &state);
                    if ev != ScreenEvent::Unhandled || p.body.focus != before {
                        hit = true;
                        break;
                    }
                }
                assert!(hit, "{:?} from {:?} is not bound", k.code, help.key);
            }
        }
    }
}
