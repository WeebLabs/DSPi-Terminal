//! The output limiter on the output page: the Console's `OutputLimiterCell`
//! beside MUTE and its `OutputLimiterSettings` popover as an expanded section.
//!
//! The Console puts a gauge icon under the mute button: grey while off, accent
//! while on, orange while it is taking gain off. A click toggles it and a
//! right-click opens a popover with the threshold, the release, the link group
//! and the actions that reach across outputs (`OutputLimiterView.swift`,
//! `Components.swift:772-905`). Here the icon is a cell on the MUTE row that
//! takes focus: Space toggles, Enter opens the settings in place of the filter
//! list, and Esc puts the list back.
//!
//! Nothing here copies the firmware's link-group rules (limiter.c:143-194): a
//! write to one member moves the whole group, and the re-read that follows
//! every write (DESIGN section 11) brings the partners' new values in.

use crossterm::event::{KeyCode, KeyEvent};
use dspi_proto::generated::{limiter, ranges};
use dspi_session::DeviceState;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::panel::{self, Body, Header, Param, Row};
use super::{channel_name, number, output_channel};
use crate::shell::{Quick, ScreenEvent};
use crate::theme::{Glyphs, Theme};
use crate::widgets::{Action, KeyHelp, PopupList, StatusTone};

/// The probe name for `REQ_LIMITER` index `LIMITER_GET_STATUS` (limiter.h:20).
pub const FEATURE: &str = "output_limiter";

/// The reduction at which the Console's icon turns orange
/// (`OutputLimiterCell`, `let limiting = gr >= 0.05`).
pub const LIMITING_DB: f32 = 0.05;

/// The samples of latency every output carries while any limiter is on:
/// `LIMITER_DELAY` is two blocks (limiter.h:24-26).
pub const LATENCY_SAMPLES: u16 = 2 * limiter::LIMITER_BLOCK;

/// The highest link group; 0 is unlinked (limiter.h:34).
const LINK_MAX: u8 = limiter::LIMITER_LINK_GROUP_MAX as u8;

/// The Console's "All outputs" menu (`OutputLimiterView.swift`, settings).
pub const ALL_OUTPUTS_MENU: [&str; 3] = [
    "Link all stereo pairs",
    "Unlink all outputs",
    "Switch every limiter off",
];

/// The Console's action button (`OutputLimiterView.swift`, settings).
pub const COPY_TO_ALL: &str = "Copy to all outputs";

/// One sentence on the engage cost. The Console says nothing about it on
/// screen; the fact is the firmware's (survey-firmware-beta4 3.4, gotcha 14).
pub fn latency_caption() -> String {
    format!(
        "Turning on the first limiter adds {LATENCY_SAMPLES} samples of latency to every \
         output and briefly fades all outputs."
    )
}

/// The keys while the settings are open.
pub const KEYS: &[KeyHelp] = &[
    KeyHelp::new("↑ ↓", "Move"),
    KeyHelp::new("← →", "Adjust"),
    KeyHelp::new("Enter", "Edit or choose"),
    KeyHelp::new("Space", "On / off"),
    KeyHelp::new("Backspace", "Reset"),
    KeyHelp::new("Esc", "Close"),
];

/// Does this device have the limiter, and this output a record?
pub fn available(state: &DeviceState, output: usize) -> bool {
    panel::has_feature(state, FEATURE) && state.limiter(output).is_some()
}

/// Is any limiter on? The meter is only worth reading while one is.
pub fn any_on(state: &DeviceState) -> bool {
    state.limiters().iter().any(|l| l.enabled)
}

/// The gain reduction on one output, in dB: 0 while it is off or unread, as
/// the Console's cell reads it.
pub fn reduction_db(state: &DeviceState, output: usize) -> f32 {
    match (state.limiter(output), &state.limiter_meter) {
        (Some(l), Some(m)) if l.enabled => m.reduction_db(output),
        _ => 0.0,
    }
}

/// The indicator's three states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Indicator {
    Off,
    On,
    Limiting,
}

pub fn indicator(state: &DeviceState, output: usize) -> Indicator {
    match state.limiter(output) {
        Some(l) if l.enabled && reduction_db(state, output) >= LIMITING_DB => Indicator::Limiting,
        Some(l) if l.enabled => Indicator::On,
        _ => Indicator::Off,
    }
}

/// The on/off switch for one output.
pub fn toggle_command(state: &DeviceState, output: usize) -> String {
    let on = state.limiter(output).is_some_and(|l| l.enabled);
    format!("limit.on {output} {}", if on { "off" } else { "on" })
}

/// Draw the cell on the MUTE row: `LIMITER ● -1.0 dBFS  GR 0.0 dB`.
///
/// The colour carries the state, as the Console's icon does: grey off,
/// accent on, the warning colour while it is reducing.
#[allow(clippy::too_many_arguments)]
pub fn draw_indicator(
    x: u16,
    y: u16,
    right: u16,
    buf: &mut Buffer,
    theme: &Theme,
    state: &DeviceState,
    output: usize,
    focused: bool,
) {
    let Some(l) = state.limiter(output) else {
        return;
    };
    let mut put = |x: &mut u16, text: &str, style: Style| {
        if *x >= right {
            return;
        }
        let room = (right - *x) as usize;
        let text = crate::widgets::text::truncate(text, room);
        buf.set_string(*x, y, &text, style);
        *x += text.chars().count() as u16;
    };
    let mut cx = x;
    put(&mut cx, if focused { "▸" } else { " " }, theme.focused());
    put(
        &mut cx,
        "LIMITER",
        if focused {
            theme.focused()
        } else {
            theme.section()
        },
    );
    let state_now = indicator(state, output);
    let color = match state_now {
        Indicator::Off => theme.label(),
        Indicator::On => Style::default().fg(theme.accent),
        Indicator::Limiting => theme.warning_style(),
    };
    let dot = match (l.enabled, theme.glyphs == Glyphs::Ascii) {
        (true, true) => " * ",
        (false, true) => " o ",
        (true, false) => " ● ",
        (false, false) => " ○ ",
    };
    put(&mut cx, dot, color);
    if !l.enabled {
        put(&mut cx, "Off", theme.label());
        return;
    }
    put(
        &mut cx,
        &format!("{:.1} dBFS", l.threshold_db),
        theme.value(),
    );
    put(
        &mut cx,
        &format!("  GR {:.1} dB", reduction_db(state, output)),
        if state_now == Indicator::Limiting {
            theme.warning_style()
        } else {
            theme.label()
        },
    );
}

// ------------------------------------------------------------ across outputs

/// Outputs 1+2 in group 1, 3+4 in group 2 and so on, as far as the four
/// groups go, with the PDM sub left unlinked: the Console's
/// `stereoPairGroups`. The sub is the last output on both platforms
/// ([`panel::sub_output`]), so nothing here knows the platform.
pub fn stereo_pair_groups(state: &DeviceState) -> Vec<u8> {
    let sub = panel::sub_output(state);
    (0..state.caps.num_outputs as usize)
        .map(|o| {
            let pair = (o / 2 + 1) as u8;
            if o < sub && pair <= LINK_MAX { pair } else { 0 }
        })
        .collect()
}

/// One link write per output in ascending order, so each group's lowest
/// output keeps its settings and the rest adopt them (`setLimiterLinkGroups`,
/// Commands.swift:1703-1711; limiter.c:143-194).
fn link_commands(groups: &[u8]) -> String {
    groups
        .iter()
        .enumerate()
        .map(|(o, g)| format!("limit.link {o} {g}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn link_all_pairs(state: &DeviceState) -> String {
    link_commands(&stereo_pair_groups(state))
}

pub fn unlink_all(state: &DeviceState) -> String {
    link_commands(&vec![0; state.caps.num_outputs as usize])
}

/// One all-outputs SET (`LIMITER_ALL_OUTPUTS`, limiter.h:21), as
/// `setLimiterEnabledOnAll(false)` does.
pub const SWITCH_ALL_OFF: &str = "limit.on all off";

/// This output's threshold, release and on/off state on every output through
/// output 0xFF, the link groups left alone (`copyLimiterToAllOutputs`,
/// Commands.swift:1679-1692).
pub fn copy_to_all(state: &DeviceState, output: usize) -> Option<String> {
    let l = state.limiter(output)?;
    Some(format!(
        "limit.threshold all {}\nlimit.release all {}\nlimit.on all {}",
        number(l.threshold_db),
        number(l.release_ms),
        if l.enabled { "on" } else { "off" }
    ))
}

fn output_name(state: &DeviceState, output: usize) -> String {
    channel_name(state, output_channel(state, output))
}

/// The line under the link group: the Console's `linkSummary`.
pub fn link_summary(state: &DeviceState, output: usize) -> String {
    let group = state.limiter(output).map(|l| l.link_group).unwrap_or(0);
    if group == 0 {
        return "Not linked.".into();
    }
    let others: Vec<String> = state
        .limiters()
        .iter()
        .enumerate()
        .filter(|(o, l)| *o != output && l.link_group == group)
        .map(|(o, _)| output_name(state, o))
        .collect();
    match others.split_last() {
        None => format!("No other outputs in group {group}."),
        Some((last, [])) => format!("Linked with {last}."),
        Some((last, rest)) => format!("Linked with {} and {last}.", rest.join(", ")),
    }
}

// ---------------------------------------------------------- the page's `;`

/// The limiter's words on the output page's command bar: `limit on|off`,
/// `limit <dBFS>`, `release <ms>` and `link <0-4|off>`.
pub fn quick(state: &DeviceState, output: usize, verb: &str, args: &[&str]) -> Quick {
    use super::quick::number as num;
    let hint = |h: String| Quick {
        fallthrough: false,
        hint: h,
        ghost: None,
        commands: Vec::new(),
    };
    let one = |h: String, command: String| Quick {
        fallthrough: false,
        hint: h,
        ghost: None,
        commands: vec![command],
    };
    if !available(state, output) {
        return hint("this firmware has no output limiter".into());
    }
    let o = output;
    let arg = args.first().copied();
    match verb {
        "limit" => match arg {
            Some("on") => one("limiter on".into(), format!("limit.on {o} on")),
            Some("off") => one("limiter off".into(), format!("limit.on {o} off")),
            Some(t) => match num(t) {
                Some(db) => {
                    let db = db.clamp(
                        ranges::LIMITER_THRESHOLD_MIN as f64,
                        ranges::LIMITER_THRESHOLD_MAX as f64,
                    );
                    one(
                        format!("limiter threshold {db:.1} dBFS"),
                        format!("limit.threshold {o} {}", number(db as f32)),
                    )
                }
                None => hint("limit on · limit off · limit <dBFS>, -30 to 0".into()),
            },
            None => hint("limit on · limit off · limit <dBFS>, -30 to 0".into()),
        },
        "release" => match arg.and_then(num) {
            Some(ms) => {
                let ms = ms.clamp(
                    ranges::LIMITER_RELEASE_MIN as f64,
                    ranges::LIMITER_RELEASE_MAX as f64,
                );
                one(
                    format!("limiter release {ms:.0} ms"),
                    format!("limit.release {o} {}", number(ms as f32)),
                )
            }
            None => hint("release <ms>, 10 to 1000".into()),
        },
        _ => {
            let group = match arg {
                Some("off") => Some(0),
                Some(t) => t.parse::<u8>().ok().filter(|g| *g <= LINK_MAX),
                None => None,
            };
            match group {
                Some(0) => one("Not linked.".into(), format!("limit.link {o} 0")),
                Some(g) => one(format!("link group {g}"), format!("limit.link {o} {g}")),
                None => hint(format!("link <1-{LINK_MAX}> · link off")),
            }
        }
    }
}

// ------------------------------------------------------------ the settings

/// The settings, expanded under the page header in place of the filter list.
pub struct LimiterSettings {
    pub output: usize,
    body: Body,
    /// Which of the two action buttons the cursor is on.
    button: usize,
    /// The "All outputs" menu is open.
    menu: bool,
}

/// What the page should do with a key the settings had.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Event(ScreenEvent),
    Close,
}

impl LimiterSettings {
    pub fn new(output: usize) -> Self {
        Self {
            output,
            body: Body::default(),
            button: 0,
            menu: false,
        }
    }

    pub fn awaiting_menu(&self) -> bool {
        self.menu
    }

    fn rows(&self, state: &DeviceState) -> Vec<Row> {
        let Some(l) = state.limiter(self.output) else {
            return Vec::new();
        };
        // Greyed out and disabled while off, so an output without a limiter
        // never looks as if a ceiling were in force.
        let on = l.enabled;
        let mut rows = vec![
            Row::Param(
                Param::new(
                    "Threshold",
                    l.threshold_db as f64,
                    ranges::LIMITER_THRESHOLD_MIN as f64,
                    ranges::LIMITER_THRESHOLD_MAX as f64,
                    "dB",
                )
                .step(0.5)
                .decimals(1)
                .caption("The ceiling, in dBFS. No sample leaves this output above it.")
                .ends("-30 dBFS", "0 dBFS")
                .enabled(on),
            ),
            Row::Param(
                Param::new(
                    "Release",
                    l.release_ms as f64,
                    ranges::LIMITER_RELEASE_MIN as f64,
                    ranges::LIMITER_RELEASE_MAX as f64,
                    "ms",
                )
                .step(10.0)
                .decimals(0)
                .caption("How fast the gain recovers after a peak")
                .ends("10 ms", "1000 ms")
                .enabled(on),
            ),
            Row::Segmented {
                label: "Link group".into(),
                choices: (0..=LINK_MAX)
                    .map(|g| {
                        if g == 0 {
                            "Off".to_string()
                        } else {
                            g.to_string()
                        }
                    })
                    .collect(),
                selected: l.link_group.min(LINK_MAX) as usize,
                enabled: on,
            },
            Row::Caption(link_summary(state, self.output)),
            Row::Buttons {
                label: String::new(),
                buttons: vec![COPY_TO_ALL.into(), "All outputs ▾".into()],
                cursor: self.button,
            },
            Row::Caption(latency_caption()),
        ];
        // In INDEPENDENT mode the limiters are output configuration, saved
        // from Settings with the wiring rather than with the preset.
        if state.limiter_unsaved() {
            rows.push(Row::Status {
                text: "Output configuration unsaved; save it in Settings".into(),
                tone: StatusTone::Warning,
            });
        }
        rows
    }

    pub fn draw(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        state: &DeviceState,
        focused: bool,
    ) {
        let Some(l) = state.limiter(self.output) else {
            return;
        };
        let rows = self.rows(state);
        self.body.clamp(&rows);
        let title = format!("Output Limiter · {}", output_name(state, self.output));
        let header = Header::new(&title).toggle(l.enabled);
        self.body.draw(area, buf, theme, &header, &rows, focused);
        if !l.enabled {
            self.dim_buttons(area, buf, theme, &rows);
        }
    }

    /// The button row has no disabled look of its own, so while the limiter
    /// is off it is repainted in the label colour like the other settings.
    fn dim_buttons(&self, area: Rect, buf: &mut Buffer, theme: &Theme, rows: &[Row]) {
        let Some(at) = rows.iter().position(|r| matches!(r, Row::Buttons { .. })) else {
            return;
        };
        if at < self.body.first || area.height < 2 {
            return;
        }
        let y = area.y
            + 1
            + rows[self.body.first..at]
                .iter()
                .map(|r| r.height(area.width))
                .sum::<u16>();
        if y >= area.y + area.height {
            return;
        }
        for x in area.x + 1..area.x + area.width {
            buf[(x, y)].set_style(Style::reset().patch(theme.label()));
        }
    }

    pub fn handle(&mut self, key: KeyEvent, state: &DeviceState) -> Outcome {
        let Some(l) = state.limiter(self.output) else {
            return Outcome::Close;
        };
        if key.code == KeyCode::Esc && self.body.edit.is_none() {
            return Outcome::Close;
        }
        let o = self.output;
        // Space switches the limiter from anywhere in the settings, as the
        // Console's switch sits beside every control.
        if key.code == KeyCode::Char(' ') && self.body.edit.is_none() && self.body.focus != 0 {
            let on_buttons = matches!(self.body.row(&self.rows(state)), Some(Row::Buttons { .. }));
            if !on_buttons {
                return Outcome::Event(ScreenEvent::Command(toggle_command(state, o)));
            }
        }
        let rows = self.rows(state);
        self.body.clamp(&rows);
        let ev = match panel::dispatch(&mut self.body, &rows, key) {
            panel::Step::Done(ev) => ev,
            panel::Step::Header => ScreenEvent::Command(toggle_command(state, o)),
            panel::Step::Commit(i, v) => match &rows[i] {
                Row::Param(p) => param_command(o, &p.label, v.clamp(p.min, p.max)),
                _ => ScreenEvent::Handled,
            },
            panel::Step::Row(i, action) => match (&rows[i], action) {
                (Row::Param(p), Action::Changed(v)) => param_command(o, &p.label, v),
                (Row::Param(p), Action::Reset) => {
                    let v = if p.label == "Threshold" {
                        ranges::LIMITER_DEFAULT_THRESHOLD
                    } else {
                        ranges::LIMITER_DEFAULT_RELEASE
                    };
                    param_command(o, &p.label, v as f64)
                }
                (Row::Segmented { .. }, Action::Selected(g)) => {
                    ScreenEvent::Command(format!("limit.link {o} {g}"))
                }
                (Row::Buttons { .. }, Action::Selected(b)) => {
                    self.button = b;
                    ScreenEvent::Handled
                }
                // Disabled while off, with the rest of the settings.
                (Row::Buttons { .. }, Action::Button(_)) if !l.enabled => ScreenEvent::Handled,
                (Row::Buttons { .. }, Action::Button(0)) => copy_to_all(state, o)
                    .map(ScreenEvent::Command)
                    .unwrap_or(ScreenEvent::Handled),
                (Row::Buttons { .. }, Action::Button(_)) => {
                    self.menu = true;
                    ScreenEvent::Popup(PopupList::new(
                        "All outputs",
                        ALL_OUTPUTS_MENU.iter().map(|s| s.to_string()).collect(),
                        0,
                    ))
                }
                _ => ScreenEvent::Handled,
            },
        };
        Outcome::Event(ev)
    }

    pub fn popup_result(&mut self, choice: Option<usize>, state: &DeviceState) -> ScreenEvent {
        if !std::mem::take(&mut self.menu) {
            return ScreenEvent::Handled;
        }
        match choice {
            Some(0) => ScreenEvent::Command(link_all_pairs(state)),
            Some(1) => ScreenEvent::Command(unlink_all(state)),
            Some(2) => ScreenEvent::Command(SWITCH_ALL_OFF.into()),
            _ => ScreenEvent::Handled,
        }
    }
}

fn param_command(output: usize, label: &str, value: f64) -> ScreenEvent {
    let path = if label == "Threshold" {
        "limit.threshold"
    } else {
        "limit.release"
    };
    ScreenEvent::Command(format!("{path} {output} {}", number(value as f32)))
}

/// A device with the limiter, for the gallery and the tests: output 1 on at
/// the default ceiling and linked with output 2, the rest at the defaults.
pub fn demo_state(mut state: DeviceState) -> DeviceState {
    state.caps.features.push(dspi_session::probe::Feature {
        name: FEATURE.into(),
        present: true,
        evidence: "fixture".into(),
    });
    for o in 0..state.caps.num_outputs as usize {
        set(
            &mut state,
            o,
            dspi_session::state::LimiterOutput {
                enabled: o < 2,
                link_group: if o < 2 { 1 } else { 0 },
                threshold_db: ranges::LIMITER_DEFAULT_THRESHOLD,
                release_ms: ranges::LIMITER_DEFAULT_RELEASE,
            },
        );
    }
    state
}

/// Write one limiter record into the shadow (bulk_params.h:436-442).
pub fn set(state: &mut DeviceState, output: usize, l: dspi_session::state::LimiterOutput) {
    let base = dspi_proto::generated::OFF_LIMITER + 12 * output;
    let mut rec = [0u8; 12];
    rec[0] = l.enabled as u8;
    rec[1] = l.link_group;
    rec[4..8].copy_from_slice(&l.threshold_db.to_le_bytes());
    rec[8..12].copy_from_slice(&l.release_ms.to_le_bytes());
    state.bulk.patch(base, &rec);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::fixture;
    use crate::widgets::testing::key;
    use dspi_proto::packets::LimiterMeter;
    use dspi_session::state::LimiterOutput;

    fn state() -> DeviceState {
        demo_state(fixture::state())
    }

    fn rp2040() -> DeviceState {
        let mut s = fixture::state();
        s.caps.platform = dspi_proto::Platform::Rp2040;
        s.caps.num_inputs = 2;
        s.caps.num_outputs = 5;
        demo_state(s)
    }

    fn draw(p: &mut LimiterSettings, state: &DeviceState, w: u16, h: u16) -> String {
        let t = Theme::console(crate::theme::ColorDepth::TrueColor, Glyphs::Braille);
        crate::render_frame(w, h, |area, buf| p.draw(area, buf, &t, state, true))
    }

    #[test]
    fn the_indicator_has_three_states() {
        let mut s = state();
        assert_eq!(indicator(&s, 0), Indicator::On);
        assert_eq!(indicator(&s, 2), Indicator::Off);
        s.limiter_meter = Some(LimiterMeter {
            centi_db: vec![4, 320, 900, 0, 0, 0, 0, 0, 0],
        });
        assert_eq!(indicator(&s, 0), Indicator::On, "0.04 dB is not limiting");
        assert_eq!(indicator(&s, 1), Indicator::Limiting);
        assert_eq!(
            indicator(&s, 2),
            Indicator::Off,
            "an off limiter reads no reduction, whatever the meter says"
        );
        assert_eq!(reduction_db(&s, 1), 3.2);
    }

    #[test]
    fn stereo_pairs_link_in_fours_and_leave_the_sub_alone() {
        // RP2350: eight outputs in four pairs, the ninth is the PDM sub.
        assert_eq!(
            stereo_pair_groups(&state()),
            vec![1, 1, 2, 2, 3, 3, 4, 4, 0]
        );
        assert_eq!(
            link_all_pairs(&state()),
            "limit.link 0 1\nlimit.link 1 1\nlimit.link 2 2\nlimit.link 3 2\nlimit.link 4 3\n\
             limit.link 5 3\nlimit.link 6 4\nlimit.link 7 4\nlimit.link 8 0"
        );
        // RP2040: two pairs and the sub.
        assert_eq!(stereo_pair_groups(&rp2040()), vec![1, 1, 2, 2, 0]);
        assert_eq!(
            unlink_all(&rp2040()),
            "limit.link 0 0\nlimit.link 1 0\nlimit.link 2 0\nlimit.link 3 0\nlimit.link 4 0"
        );
    }

    #[test]
    fn the_link_summary_is_the_consoles() {
        let mut s = state();
        assert!(
            link_summary(&s, 0).starts_with("Linked with "),
            "{}",
            link_summary(&s, 0)
        );
        assert_eq!(link_summary(&s, 2), "Not linked.");
        let lone = LimiterOutput {
            link_group: 3,
            ..s.limiter(2).unwrap()
        };
        set(&mut s, 2, lone);
        assert_eq!(link_summary(&s, 2), "No other outputs in group 3.");
        set(&mut s, 3, lone);
        assert_eq!(link_summary(&s, 2), "Linked with OUT 4.");
        set(&mut s, 4, lone);
        set(&mut s, 5, lone);
        assert_eq!(link_summary(&s, 2), "Linked with OUT 4, OUT 5 and OUT 6.");
    }

    #[test]
    fn copy_to_all_writes_everything_but_the_link_group() {
        let mut s = state();
        set(
            &mut s,
            0,
            LimiterOutput {
                enabled: true,
                link_group: 1,
                threshold_db: -6.5,
                release_ms: 250.0,
            },
        );
        assert_eq!(
            copy_to_all(&s, 0).unwrap(),
            "limit.threshold all -6.5\nlimit.release all 250\nlimit.on all on"
        );
    }

    #[test]
    fn the_settings_carry_the_consoles_strings() {
        let s = state();
        let mut p = LimiterSettings::new(0);
        let f = draw(&mut p, &s, 80, 20);
        for want in [
            "Output Limiter",
            "● On",
            "Threshold",
            "-1.0 dB",
            "-30 dBFS",
            "0 dBFS",
            "Release",
            "100 ms",
            "Link group",
            "Off",
            "Linked with",
            "Copy to all outputs",
            "All outputs",
            "32 samples of latency",
        ] {
            assert!(f.contains(want), "{want:?} missing:\n{f}");
        }
    }

    /// The settings dim and stop answering while the limiter is off.
    #[test]
    fn the_settings_are_dimmed_and_disabled_while_off() {
        let s = state();
        let t = Theme::console(crate::theme::ColorDepth::TrueColor, Glyphs::Braille);
        let mut p = LimiterSettings::new(2);
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 20));
        p.draw(Rect::new(0, 0, 80, 20), &mut buf, &t, &s, true);
        let row = |needle: &str| {
            (0..20u16)
                .find(|y| {
                    (0..80u16)
                        .map(|x| buf[(x, *y)].symbol())
                        .collect::<String>()
                        .contains(needle)
                })
                .unwrap_or_else(|| panic!("{needle} not drawn"))
        };
        for needle in ["Threshold", "Link group", "Copy to all outputs"] {
            let y = row(needle);
            let x = (0..80u16)
                .find(|x| buf[(*x, y)].symbol() == needle.chars().next().unwrap().to_string())
                .unwrap();
            assert_eq!(
                buf[(x, y)].fg,
                t.dim,
                "{needle} is drawn in the disabled colour"
            );
        }
        // Down to the threshold: the arrows do nothing, nor does the menu.
        p.handle(key(KeyCode::Down), &s);
        assert_eq!(
            p.handle(key(KeyCode::Right), &s),
            Outcome::Event(ScreenEvent::Unhandled)
        );
        p.handle(key(KeyCode::Down), &s);
        p.handle(key(KeyCode::Down), &s);
        assert_eq!(
            p.handle(key(KeyCode::Right), &s),
            Outcome::Event(ScreenEvent::Unhandled),
            "the link group"
        );
        p.handle(key(KeyCode::Down), &s);
        assert_eq!(
            p.handle(key(KeyCode::Enter), &s),
            Outcome::Event(ScreenEvent::Handled),
            "Copy to all outputs"
        );
        // The switch still works.
        p.handle(key(KeyCode::Up), &s);
        p.handle(key(KeyCode::Up), &s);
        p.handle(key(KeyCode::Up), &s);
        p.handle(key(KeyCode::Up), &s);
        assert_eq!(
            p.handle(key(KeyCode::Enter), &s),
            Outcome::Event(ScreenEvent::Command("limit.on 2 on".into()))
        );
    }

    #[test]
    fn the_settings_write_threshold_release_and_link() {
        let s = state();
        let mut p = LimiterSettings::new(0);
        assert_eq!(
            p.handle(key(KeyCode::Enter), &s),
            Outcome::Event(ScreenEvent::Command("limit.on 0 off".into())),
            "the header switch"
        );
        p.handle(key(KeyCode::Down), &s);
        assert_eq!(
            p.handle(key(KeyCode::Left), &s),
            Outcome::Event(ScreenEvent::Command("limit.threshold 0 -1.5".into()))
        );
        assert_eq!(
            p.handle(key(KeyCode::Backspace), &s),
            Outcome::Event(ScreenEvent::Command("limit.threshold 0 -1".into()))
        );
        p.handle(key(KeyCode::Down), &s);
        assert_eq!(
            p.handle(key(KeyCode::Right), &s),
            Outcome::Event(ScreenEvent::Command("limit.release 0 110".into()))
        );
        // A typed release is clamped to the firmware's range.
        p.handle(key(KeyCode::Enter), &s);
        for c in "5000".chars() {
            p.handle(key(KeyCode::Char(c)), &s);
        }
        assert_eq!(
            p.handle(key(KeyCode::Enter), &s),
            Outcome::Event(ScreenEvent::Command("limit.release 0 1000".into()))
        );
        p.handle(key(KeyCode::Down), &s);
        assert_eq!(
            p.handle(key(KeyCode::Right), &s),
            Outcome::Event(ScreenEvent::Command("limit.link 0 2".into()))
        );
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &s),
            Outcome::Event(ScreenEvent::Command("limit.on 0 off".into())),
            "Space is the switch from anywhere"
        );
        p.handle(key(KeyCode::Down), &s);
        assert_eq!(
            p.handle(key(KeyCode::Enter), &s),
            Outcome::Event(ScreenEvent::Command(copy_to_all(&s, 0).unwrap()))
        );
        p.handle(key(KeyCode::Right), &s);
        match p.handle(key(KeyCode::Enter), &s) {
            Outcome::Event(ScreenEvent::Popup(list)) => {
                assert_eq!(list.items, ALL_OUTPUTS_MENU.map(String::from).to_vec())
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            p.popup_result(Some(2), &s),
            ScreenEvent::Command("limit.on all off".into())
        );
        assert_eq!(p.handle(key(KeyCode::Esc), &s), Outcome::Close);
    }

    #[test]
    fn every_advertised_key_is_handled() {
        let s = state();
        for help in KEYS {
            for k in crate::screens::tests::keys_for(help.key) {
                let mut done = false;
                for down in 0..5 {
                    let mut p = LimiterSettings::new(0);
                    for _ in 0..down {
                        p.handle(key(KeyCode::Down), &s);
                    }
                    let before = (p.body.focus, p.button);
                    let ev = p.handle(k, &s);
                    if ev != Outcome::Event(ScreenEvent::Unhandled)
                        || before != (p.body.focus, p.button)
                    {
                        done = true;
                    }
                }
                assert!(done, "{:?} does nothing (from {:?})", k.code, help.key);
            }
        }
    }

    #[test]
    fn the_command_bar_words() {
        let s = state();
        let q = |verb: &str, args: &[&str]| quick(&s, 0, verb, args);
        assert_eq!(q("limit", &["on"]).commands, vec!["limit.on 0 on"]);
        assert_eq!(q("limit", &["off"]).commands, vec!["limit.on 0 off"]);
        assert_eq!(q("limit", &["-3"]).commands, vec!["limit.threshold 0 -3"]);
        assert_eq!(
            q("limit", &["-40"]).commands,
            vec!["limit.threshold 0 -30"],
            "clamped"
        );
        assert_eq!(q("release", &["250"]).commands, vec!["limit.release 0 250"]);
        assert_eq!(q("link", &["2"]).commands, vec!["limit.link 0 2"]);
        assert_eq!(q("link", &["off"]).commands, vec!["limit.link 0 0"]);
        assert_eq!(q("link", &["0"]).commands, vec!["limit.link 0 0"]);
        for (verb, args) in [
            ("limit", &[][..]),
            ("limit", &["x"][..]),
            ("release", &[][..]),
            ("link", &["5"][..]),
        ] {
            let r = q(verb, args);
            assert!(r.commands.is_empty(), "{verb} {args:?}");
            assert!(!r.hint.is_empty());
        }
        let plain = fixture::state();
        assert!(
            quick(&plain, 0, "limit", &["on"]).commands.is_empty(),
            "no limiter, no write"
        );
    }
}
