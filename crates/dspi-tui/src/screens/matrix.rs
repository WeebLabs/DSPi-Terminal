//! The Matrix Mixer: the Console's `MatrixMixerView` as a grid of crosspoints.
//!
//! One column per output, one row per input, and each output's own strip
//! (enable, gain, delay, mute) beneath. Everything is read from
//! [`DeviceState`] each frame, so a route made on the device or by another
//! host is on screen the next tick; every edit leaves as a command in the
//! shared grammar, which is also what the echo line shows.
//!
//! The one thing the panel decides for itself is the Core 1 interlock. PDM and
//! the per-output EQ workers cannot both run, and the firmware ignores an
//! enable that would break that in silence, so the panel does the Console's
//! own client-side check (`DSPViewModel.outputEnableWouldConflict`) from the
//! output-enable flags it already has, marks the cells that would collide, and
//! raises the Console's two confirmations before writing.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use dspi_session::DeviceState;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use super::{Shared, channel_name, clipboard, max_delay_ms, number, output_channel};
use crate::shell::{Screen, ScreenEvent};
use crate::theme::{ChannelRole, Glyphs, Theme};
use crate::widgets::text::{fit_centre, fit_left};
use crate::widgets::{Button, Dialog, DialogOutcome, KeyHelp, NumberEdit};

/// The pinned input-label column: a name and air after it. Nine cells at
/// the Normal pane, which is what lets nine output columns fit beside it;
/// sixteen once the pane is wide enough to spare them.
fn label_width(width: u16) -> u16 {
    if width >= 130 { 16 } else { 9 }
}

/// The narrowest output column: eight cells for `-60.0 INV`, the widest
/// value a gains line shows, and a gutter that doubles as the focus
/// bracket of the next column. Columns grow up to [`MAX_COL_W`] when the
/// pane has the room, so the grid is never more crowded than it has to be.
const MIN_COL_W: u16 = 9;
const MAX_COL_W: u16 = 18;

/// A trimmed decimal for the grammar, from an f64.
fn number_str(v: f64) -> String {
    number(v as f32)
}

/// The column width for a pane this wide showing `n_out` outputs.
fn col_width(width: u16, n_out: usize) -> u16 {
    (width.saturating_sub(label_width(width)) / n_out.max(1) as u16).clamp(MIN_COL_W, MAX_COL_W)
}
/// The Console's `CompactGainField` range, which is also the crosspoint's.
const GAIN_MIN: f64 = -60.0;
const GAIN_MAX: f64 = 12.0;
/// The Console's stereo matrix, before the 8-channel rows
/// (`BASE_MATRIX_INPUTS`).
const BASE_INPUTS: usize = 2;

/// A row of the grid. The four output rows follow the input rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Input(usize),
    Enable,
    Gain,
    Delay,
    Mute,
}

/// One drawn line of the body, which is the rows plus the chrome between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Line {
    Routing,
    Divider,
    Row(Row),
    /// The second line of an input: the crosspoint gains, under the connect
    /// dots of `Row(Row::Input(i))`.
    Gains(usize),
    /// The blank line after an input's gains, so the inputs sit evenly.
    Blank,
}

/// Which dialog the panel put on screen, so its answer comes back to the right
/// place.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pending {
    Clear,
    Rename(usize),
    /// Enabling this output needs the other side of Core 1 switched off first.
    Enable(usize),
}

/// What one cell needs to draw itself, so the cell painters stay short.
struct Paint<'a> {
    buf: &'a mut Buffer,
    theme: &'a Theme,
    state: &'a DeviceState,
    area: Rect,
}

pub struct MatrixPanel {
    shared: Shared,
    /// The reticle: a row and an output column.
    row: usize,
    col: usize,
    /// The leftmost visible output column.
    scroll: usize,
    /// The column and label widths of the last frame, from [`col_width`]
    /// and [`label_width`].
    col_w: u16,
    label_w: u16,
    edit: Option<NumberEdit>,
    pending: Option<Pending>,
}

const KEYS: &[KeyHelp] = &[
    KeyHelp::new("↑ ↓ ← →", "Move the reticle"),
    KeyHelp::new("Space", "Connect, enable, mute"),
    KeyHelp::new("Enter", "Edit a gain"),
    KeyHelp::new("i", "Invert"),
    KeyHelp::new("d", "Direct 1:1"),
    KeyHelp::new("D", "Clear"),
    KeyHelp::new("r", "Rename"),
    KeyHelp::new("y Y", "Copy, paste parameters"),
    KeyHelp::new("I", "Identify"),
];

// ---------------------------------------------------------------------------
// The Core 1 interlock, from the state the panel already has
// ---------------------------------------------------------------------------

/// The PDM sub, always the last output (`DSPViewModel.pdmOutputIndex`).
fn pdm_output(state: &DeviceState) -> Option<usize> {
    (state.caps.num_outputs as usize).checked_sub(1)
}

/// The outputs Core 1 runs EQ for: `CORE1_EQ_FIRST_OUTPUT` (2) up to the
/// output below PDM, from the discovered output count rather than a
/// compiled-in platform test.
fn eq_worker_outputs(state: &DeviceState) -> std::ops::RangeInclusive<usize> {
    const FIRST: usize = 2;
    match (state.caps.num_outputs as usize).checked_sub(2) {
        Some(last) if last >= FIRST => FIRST..=last,
        _ => std::ops::RangeInclusive::new(1, 0),
    }
}

/// `DSPViewModel.outputEnableWouldConflict`: would turning this output on take
/// the other side of Core 1 down?
fn would_conflict(state: &DeviceState, output: usize) -> bool {
    let Some(pdm) = pdm_output(state) else {
        return false;
    };
    let eq = eq_worker_outputs(state);
    if output == pdm {
        return eq.clone().any(|o| state.output(o).enabled);
    }
    if eq.contains(&output) {
        return state.output(pdm).enabled;
    }
    false
}

/// Eight-channel mode: the Console's `supports8chInput`, read off the
/// discovered input count rather than the platform name.
fn is_8ch(state: &DeviceState) -> bool {
    state.caps.num_inputs as usize > BASE_INPUTS
}

/// The colour of an output column, which is the sub's colour for the last
/// one: under the colour budget of DESIGN 12.3 only the focused column
/// takes its hue.
fn column_color(state: &DeviceState, theme: &Theme, output: usize, focused: bool) -> Color {
    theme.hue_for(
        ChannelRole::of(
            (state.caps.num_inputs as usize + output) as u8,
            state.caps.num_inputs,
            state.caps.num_outputs,
        ),
        focused,
    )
}

fn descriptor(state: &DeviceState, output: usize) -> String {
    ChannelRole::of(
        (state.caps.num_inputs as usize + output) as u8,
        state.caps.num_inputs,
        state.caps.num_outputs,
    )
    .descriptor(state.caps.num_outputs)
}

impl MatrixPanel {
    pub fn new(shared: Shared) -> Self {
        Self {
            shared,
            row: 0,
            col: 0,
            scroll: 0,
            col_w: MIN_COL_W,
            label_w: 9,
            edit: None,
            pending: None,
        }
    }

    fn inputs(&self, state: &DeviceState) -> usize {
        state.caps.num_inputs as usize
    }

    fn outputs(&self, state: &DeviceState) -> usize {
        state.caps.num_outputs as usize
    }

    fn rows(&self, state: &DeviceState) -> Vec<Row> {
        let mut v: Vec<Row> = (0..self.inputs(state)).map(Row::Input).collect();
        v.extend([Row::Enable, Row::Gain, Row::Delay, Row::Mute]);
        v
    }

    fn row_at(&self, state: &DeviceState) -> Row {
        let rows = self.rows(state);
        rows[self.row.min(rows.len() - 1)]
    }

    /// Every line the body draws, in order: the ROUTING band, two lines per
    /// input (the connect dots, then the gains, then a blank), a rule, then
    /// the output rows. The Console divides the inputs into stereo pairs and
    /// puts a crosspoint's dot and gain in one cell; here every input stands
    /// on its own and the dots and the gains are rows of their own, so a
    /// glance down a column reads the routing and a glance along a row the
    /// levels.
    fn lines(&self, state: &DeviceState) -> Vec<Line> {
        let n = self.inputs(state);
        let mut v = vec![Line::Routing];
        for i in 0..n {
            v.push(Line::Row(Row::Input(i)));
            v.push(Line::Gains(i));
            v.push(Line::Blank);
        }
        v.push(Line::Divider);
        v.extend([
            Line::Row(Row::Enable),
            Line::Row(Row::Gain),
            Line::Row(Row::Delay),
            Line::Row(Row::Mute),
        ]);
        v
    }

    /// How many output columns fit beside the pinned label column.
    fn visible_columns(&self, width: u16, n_out: usize) -> usize {
        (width.saturating_sub(label_width(width)) / col_width(width, n_out)).max(1) as usize
    }

    // -- Commands ----------------------------------------------------------

    /// One crosspoint write, carrying the two fields that are not changing.
    fn mix_command(
        &self,
        input: usize,
        output: usize,
        enabled: bool,
        gain: f32,
        invert: bool,
    ) -> String {
        let mut s = format!(
            "mix {input} {output} {} {}",
            if enabled { "on" } else { "off" },
            number(gain)
        );
        if invert {
            s.push_str(" inv");
        }
        s
    }

    /// The value under the reticle, for arming an edit.
    fn value_at(&self, state: &DeviceState) -> Option<f64> {
        match self.row_at(state) {
            Row::Input(i) => Some(state.crosspoint(i, self.col).gain_db as f64),
            Row::Gain => Some(state.output(self.col).gain_db as f64),
            Row::Delay => Some(state.output(self.col).delay_ms as f64),
            Row::Enable | Row::Mute => None,
        }
    }

    /// The command that writes `v` where the reticle is.
    fn write_at(&self, state: &DeviceState, v: f64) -> ScreenEvent {
        let o = self.col;
        match self.row_at(state) {
            Row::Input(i) => {
                let c = state.crosspoint(i, o);
                ScreenEvent::Command(self.mix_command(
                    i,
                    o,
                    c.enabled,
                    v.clamp(GAIN_MIN, GAIN_MAX) as f32,
                    c.phase_invert,
                ))
            }
            Row::Gain => ScreenEvent::Command(format!(
                "out.gain {o} {}",
                number(v.clamp(-60.0, 10.0) as f32)
            )),
            Row::Delay => ScreenEvent::Command(format!(
                "out.delay {o} {}",
                number(v.clamp(0.0, max_delay_ms(state)) as f32)
            )),
            Row::Enable | Row::Mute => ScreenEvent::Handled,
        }
    }

    /// The nudge one arrow press makes on the armed field. The Console's
    /// compact gain fields scroll in half decibels; delay follows the output
    /// page's tenth of a millisecond.
    fn nudge(&self, state: &DeviceState, dir: f64, coarse: bool) -> f64 {
        let mult = if coarse { 10.0 } else { 1.0 };
        let row = self.row_at(state);
        let (value, step) = match row {
            Row::Input(i) => (state.crosspoint(i, self.col).gain_db as f64, 0.5),
            Row::Gain => (state.output(self.col).gain_db as f64, 0.5),
            Row::Delay => (state.output(self.col).delay_ms as f64, 0.1),
            _ => (0.0, 0.0),
        };
        value + dir * step * mult
    }

    /// `DSPViewModel.applyDirectRouting`: free Core 1, put every crosspoint on
    /// the `i` to `i` diagonal, then enable the diagonal outputs.
    fn direct_routing(&self, state: &DeviceState) -> Vec<String> {
        let ni = self.inputs(state);
        let no = self.outputs(state);
        let n = ni.min(no);
        let pdm = pdm_output(state);
        let mut out = Vec::new();
        if let Some(pdm) = pdm
            && state.output(pdm).enabled
        {
            out.push(format!("out.enable {pdm} off"));
        }
        for i in 0..ni {
            for o in 0..no {
                let on = i == o && i < n;
                let c = state.crosspoint(i, o);
                if c.enabled != on || c.gain_db != 0.0 || c.phase_invert {
                    out.push(self.mix_command(i, o, on, 0.0, false));
                }
            }
        }
        for o in 0..n {
            if Some(o) != pdm && !state.output(o).enabled {
                out.push(format!("out.enable {o} on"));
            }
        }
        out
    }

    /// `DSPViewModel.clearAllRoutes`: disconnect every crosspoint, leaving the
    /// gains and the phase alone.
    fn clear_routes(&self, state: &DeviceState) -> Vec<String> {
        let mut out = Vec::new();
        for i in 0..self.inputs(state) {
            for o in 0..self.outputs(state) {
                let c = state.crosspoint(i, o);
                if c.enabled {
                    out.push(self.mix_command(i, o, false, c.gain_db, c.phase_invert));
                }
            }
        }
        out
    }

    /// `Session::enable_output_confirmed`'s order, as commands: free the other
    /// side of Core 1, then turn this output on.
    fn enable_confirmed(&self, state: &DeviceState, output: usize) -> Vec<String> {
        let mut out = Vec::new();
        if Some(output) == pdm_output(state) {
            for o in eq_worker_outputs(state) {
                out.push(format!("out.enable {o} off"));
            }
        } else if let Some(pdm) = pdm_output(state) {
            out.push(format!("out.enable {pdm} off"));
        }
        out.push(format!("out.enable {output} on"));
        out
    }

    /// The Console's two Core 1 alerts, verbatim
    /// (`MatrixMixerView.swift:192-217`).
    fn conflict_dialog(&self, state: &DeviceState, output: usize) -> Dialog {
        if Some(output) == pdm_output(state) {
            let eq = eq_worker_outputs(state);
            Dialog::confirm(
                "Warning",
                // 1-based for display, as the Console counts them.
                format!(
                    "Outputs {}-{} will be disabled. Are you sure?",
                    eq.start() + 1,
                    eq.end() + 1
                ),
                vec![Button::new("Enable PDM"), Button::new("Cancel")],
            )
        } else {
            Dialog::confirm(
                "Warning",
                "The PDM output will be disabled. Are you sure?",
                vec![Button::new("Disable PDM"), Button::new("Cancel")],
            )
        }
    }

    fn request_enable(&mut self, state: &DeviceState) -> ScreenEvent {
        let o = self.col;
        if state.output(o).enabled {
            // Disabling always lands.
            return ScreenEvent::Command(format!("out.enable {o} off"));
        }
        if would_conflict(state, o) {
            self.pending = Some(Pending::Enable(o));
            return ScreenEvent::Dialog(self.conflict_dialog(state, o));
        }
        ScreenEvent::Command(format!("out.enable {o} on"))
    }
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

impl MatrixPanel {
    /// The page grammar behind `;` (DESIGN 13). Routing reads as an arrow:
    /// `1 3 > 5` connects IN1 and IN3 to OUT5, `1 x all` disconnects IN1
    /// from everything. Lists take singles, ranges and `all` on both sides.
    fn quick_reply(&self, line: &str, state: &DeviceState) -> crate::shell::Quick {
        use super::quick::{channel_list, ghost, names, number, verb};
        use crate::shell::Quick;
        const VERBS: &[&str] = &["gain", "inv", "out", "direct", "clear"];
        const SUMMARY: &str = "1 3 > 5 connect · 1 x 3 disconnect · gain 1 3 -6 · inv 1 3 · out 5 mute · direct · clear";
        let ni = self.inputs(state);
        let no = self.outputs(state);
        let lower = line.to_ascii_lowercase();
        let tokens: Vec<&str> = lower.split_whitespace().collect();
        let hint = |h: &str| Quick {
            hint: h.to_string(),
            ghost: ghost(&lower, VERBS),
            commands: Vec::new(),
        };
        if tokens.is_empty() {
            return hint(SUMMARY);
        }

        // The arrow forms: `<inputs> > <outputs> [gain] [inv]`, `<inputs> x <outputs>`.
        if let Some(split) = tokens.iter().position(|t| *t == ">" || *t == "x") {
            let connect = tokens[split] == ">";
            let Some(ins) = channel_list(&tokens[..split], ni) else {
                return hint("inputs: 1 · 1 3 · 1-4 · all, then > or x");
            };
            let rest = &tokens[split + 1..];
            let inv = connect && rest.last() == Some(&"inv");
            let rest = if inv { &rest[..rest.len() - 1] } else { rest };
            // `1 > 3 4` is two outputs; a gain announces itself with a sign,
            // a decimal point or a value no output could be: `1 > 3 -6`.
            let (rest, gain) = match rest.last() {
                Some(t)
                    if connect
                        && rest.len() > 1
                        && number(t).is_some()
                        && super::quick::channel_token(t, no).is_none() =>
                {
                    (&rest[..rest.len() - 1], number(t))
                }
                _ => (rest, None),
            };
            let Some(outs) = channel_list(rest, no) else {
                return hint(if connect {
                    "outputs, then an optional gain and inv: 5 · 3 4 -6 inv · all"
                } else {
                    "outputs to disconnect: 5 · 3 4 · all"
                });
            };
            let mut commands = Vec::new();
            for &i in &ins {
                for &o in &outs {
                    let c = state.crosspoint(i, o);
                    let g = gain.map(|g| g as f32).unwrap_or(c.gain_db);
                    let inv = if connect {
                        inv || c.phase_invert
                    } else {
                        c.phase_invert
                    };
                    commands.push(self.mix_command(i, o, connect, g, inv));
                }
            }
            let mut h = format!(
                "{} {} {}",
                names("IN", &ins),
                if connect { "→" } else { "×" },
                names("OUT", &outs)
            );
            if let Some(g) = gain {
                h.push_str(&format!(" at {g:+.1} dB"));
            }
            if inv {
                h.push_str(" inverted");
            }
            h.push_str(&format!(
                " · {} crosspoint{} {}",
                commands.len(),
                if commands.len() == 1 { "" } else { "s" },
                if connect { "on" } else { "off" }
            ));
            return Quick {
                hint: h,
                ghost: None,
                commands,
            };
        }

        match verb(tokens[0], VERBS) {
            Some("gain") | Some("inv") => {
                let toggle = verb(tokens[0], VERBS) == Some("inv");
                let args = &tokens[1..];
                let (args, gain) = if toggle {
                    (args, None)
                } else {
                    match args.last().and_then(|t| number(t)) {
                        Some(g) if args.len() >= 3 => (&args[..args.len() - 1], Some(g)),
                        _ => (args, None),
                    }
                };
                let mid = args.len() / 2;
                let (ins, outs) = if args.len() >= 2 && args.len() % 2 == 0 {
                    (
                        channel_list(&args[..mid], ni),
                        channel_list(&args[mid..], no),
                    )
                } else {
                    (None, None)
                };
                match (ins, outs) {
                    (Some(ins), Some(outs)) if toggle || gain.is_some() => {
                        let g_db = gain.unwrap_or(0.0);
                        let mut commands = Vec::new();
                        for &i in &ins {
                            for &o in &outs {
                                let c = state.crosspoint(i, o);
                                commands.push(if toggle {
                                    self.mix_command(i, o, c.enabled, c.gain_db, !c.phase_invert)
                                } else {
                                    self.mix_command(i, o, c.enabled, g_db as f32, c.phase_invert)
                                });
                            }
                        }
                        let h = if toggle {
                            format!("invert {} → {}", names("IN", &ins), names("OUT", &outs))
                        } else {
                            format!(
                                "{} → {} gain {g_db:+.1} dB",
                                names("IN", &ins),
                                names("OUT", &outs)
                            )
                        };
                        Quick {
                            hint: h,
                            ghost: None,
                            commands,
                        }
                    }
                    _ if toggle => hint("inv <inputs> <outputs> · toggles polarity"),
                    _ => hint("gain <inputs> <outputs> <dB> · sets crosspoint gain"),
                }
            }
            Some("out") => {
                const FIELDS: &[&str] = &["gain", "delay", "mute", "unmute", "on", "off"];
                let args = &tokens[1..];
                let field_at = args.iter().position(|t| verb(t, FIELDS).is_some());
                let Some(fi) = field_at else {
                    return hint("out <outputs> gain <dB> · delay <ms> · mute · unmute · on · off");
                };
                let Some(outs) = channel_list(&args[..fi], no) else {
                    return hint("outputs first: out 3 · out 3-5 mute");
                };
                let field = verb(args[fi], FIELDS).unwrap();
                let value = args.get(fi + 1).and_then(|t| number(t));
                let mut commands = Vec::new();
                for &o in &outs {
                    match (field, value) {
                        ("gain", Some(v)) => commands
                            .push(format!("out.gain {o} {}", number_str(v.clamp(-60.0, 10.0)))),
                        ("delay", Some(v)) => commands.push(format!(
                            "out.delay {o} {}",
                            number_str(v.clamp(0.0, max_delay_ms(state)))
                        )),
                        ("mute", _) => commands.push(format!("out.mute {o} on")),
                        ("unmute", _) => commands.push(format!("out.mute {o} off")),
                        ("on", _) => commands.push(format!("out.enable {o} on")),
                        ("off", _) => commands.push(format!("out.enable {o} off")),
                        _ => {}
                    }
                }
                if commands.is_empty() {
                    return hint(match field {
                        "gain" => "out … gain <dB>",
                        _ => "out … delay <ms>",
                    });
                }
                Quick {
                    hint: format!(
                        "{} {}{}",
                        names("OUT", &outs),
                        field,
                        value.map(|v| format!(" {v}")).unwrap_or_default()
                    ),
                    ghost: None,
                    commands,
                }
            }
            Some("direct") => {
                let commands = self.direct_routing(state);
                if commands.is_empty() {
                    hint("direct 1:1 needs the 8-channel matrix")
                } else {
                    Quick {
                        hint: "Direct 1:1: INn → OUTn, everything else off".into(),
                        ghost: None,
                        commands,
                    }
                }
            }
            Some("clear") => {
                let commands = self.clear_routes(state);
                if commands.is_empty() {
                    hint("nothing to clear")
                } else {
                    Quick {
                        hint: format!("disconnect every crosspoint ({})", commands.len()),
                        ghost: None,
                        commands,
                    }
                }
            }
            _ => hint(SUMMARY),
        }
    }

    fn draw_headers(&self, p: &mut Paint, cols: &[usize]) {
        let (area, theme, state) = (p.area, p.theme, p.state);
        for (n, &o) in cols.iter().enumerate() {
            let x = area.x + self.label_w + n as u16 * self.col_w;
            let name = channel_name(state, output_channel(state, o));
            // `DESIGN.md` 7.7: "Column headers in the output colour". Both
            // header rows name the same column, so both take it.
            let color = column_color(state, theme, o, o == self.col);
            p.buf.set_string(
                x,
                area.y,
                fit_centre(&name, self.col_w as usize - 1),
                Style::default().fg(color),
            );
            let desc = descriptor(state, o);
            p.buf.set_string(
                x,
                area.y + 1,
                fit_centre(&desc, self.col_w as usize - 1),
                Style::default().fg(color),
            );
            // The Console outlines a column that would collide on Core 1 in
            // orange; the mark sits by the descriptor, once per column, rather
            // than in every cell beneath it.
            if would_conflict(state, o) {
                let start = x + (self.col_w - 1 - desc.chars().count() as u16) / 2;
                p.buf.set_string(
                    start + desc.chars().count() as u16 + 1,
                    area.y + 1,
                    "!",
                    theme.warning_style(),
                );
            }
        }
        // Which way the grid still has columns, since the label column is
        // pinned and the rest slides under it.
        let ascii = theme.glyphs == Glyphs::Ascii;
        if self.scroll > 0 {
            p.buf.set_string(
                area.x + self.label_w - 1,
                area.y + 1,
                if ascii { "<" } else { "‹" },
                theme.chrome_style(),
            );
        }
        if self.scroll + cols.len() < state.caps.num_outputs as usize {
            let x = area.x + self.label_w + cols.len() as u16 * self.col_w - 1;
            if x < area.x + area.width {
                p.buf.set_string(
                    x,
                    area.y + 1,
                    if ascii { ">" } else { "›" },
                    theme.chrome_style(),
                );
            }
        }
    }

    /// The `[` and `]` the kit puts round a focused cell, in the gutters
    /// either side of it.
    fn draw_brackets(&self, p: &mut Paint, x: u16, y: u16) {
        let (area, theme) = (p.area, p.theme);
        if x > area.x {
            p.buf.set_string(x - 1, y, "[", theme.focused());
        }
        if x + self.col_w - 1 < area.x + area.width {
            p.buf
                .set_string(x + self.col_w - 1, y, "]", theme.focused());
        }
    }

    /// The dots line of an input: `●` where it feeds the column, `○` where
    /// it does not, centred in the column like the MUTE row's.
    fn draw_connect(
        &self,
        p: &mut Paint,
        x: u16,
        y: u16,
        input: usize,
        output: usize,
        focused: bool,
    ) {
        let (theme, state) = (p.theme, p.state);
        let c = state.crosspoint(input, output);
        // The Console desaturates a disabled output's whole column.
        let live = state.output(output).enabled;
        let ascii = theme.glyphs == Glyphs::Ascii;
        let dot = match (c.enabled, ascii) {
            (true, true) => "*",
            (false, true) => "o",
            (true, false) => "●",
            (false, false) => "○",
        };
        let style = if live && c.enabled {
            Style::default().fg(column_color(state, theme, output, output == self.col))
        } else if focused {
            theme.focused()
        } else {
            theme.label()
        };
        p.buf.set_string(x + (self.col_w - 1) / 2, y, dot, style);
        if focused {
            self.draw_brackets(p, x, y);
        }
    }

    /// The gains line of an input: the crosspoint gain under each connected
    /// dot, `INV` after it when inverted, nothing under a `○`.
    fn draw_gain(&self, p: &mut Paint, x: u16, y: u16, input: usize, output: usize, focused: bool) {
        let (theme, state) = (p.theme, p.state);
        let c = state.crosspoint(input, output);
        let live = state.output(output).enabled;
        let w = self.col_w as usize - 1;
        let armed = focused && self.edit.is_some();
        let style = if armed {
            theme.editing()
        } else if focused {
            theme.focused()
        } else if live {
            theme.value()
        } else {
            theme.label()
        };
        // Cells are bare numbers whose last digit sits under the dot, so a
        // column of gains lines up on its decimal point. The unit appears
        // after the value in the focused cell alone; an inverted cell has
        // `INV` there instead, one cell left so the pair fits the column.
        let dot = (w / 2) as u16;
        if let Some(e) = self.edit.as_ref().filter(|_| focused) {
            let text = format!("[{}]", e.text);
            let len = text.chars().count();
            let start = x + (w.saturating_sub(len) / 2) as u16;
            p.buf.set_string(start, y, &text, style);
        } else if c.enabled {
            let number = format!("{:.1}", c.gain_db);
            let len = number.chars().count() as u16;
            let end = if c.phase_invert { dot - 1 } else { dot };
            p.buf
                .set_string(x + end + 1 - len.min(end + 1), y, &number, style);
            if c.phase_invert {
                let style = if live {
                    theme.warning_style()
                } else {
                    theme.label()
                };
                p.buf.set_string(x + dot + 1, y, "INV", style);
            } else if focused {
                p.buf.set_string(x + dot + 1, y, " dB", style);
            }
        }
        if focused {
            self.draw_brackets(p, x, y);
        }
    }

    fn draw_output_cell(
        &self,
        p: &mut Paint,
        x: u16,
        y: u16,
        row: Row,
        output: usize,
        focused: bool,
    ) {
        let (theme, state) = (p.theme, p.state);
        let out = state.output(output);
        let live = out.enabled;
        let ascii = theme.glyphs == Glyphs::Ascii;
        let w = self.col_w as usize - 1;
        match row {
            Row::Enable => {
                let text = match (out.enabled, ascii) {
                    (true, true) => "on",
                    (false, true) => "off",
                    (true, false) => "⏻ on",
                    (false, false) => "⏻ off",
                };
                let style = if focused {
                    theme.focused()
                } else if out.enabled {
                    Style::default().fg(theme.accent)
                } else if would_conflict(state, output) {
                    theme.warning_style()
                } else {
                    theme.label()
                };
                p.buf.set_string(x, y, fit_centre(text, w), style);
            }
            Row::Gain | Row::Delay => {
                let armed = focused && self.edit.is_some();
                let unit = if row == Row::Gain { " dB" } else { " ms" };
                let text = match (self.edit.as_ref().filter(|_| focused), row) {
                    (Some(e), _) => format!("[{}]", e.text),
                    (None, Row::Gain) => format!("{:.1}", out.gain_db),
                    (None, _) => format!("{:.1}", out.delay_ms),
                };
                let style = if armed {
                    theme.editing()
                } else if focused {
                    theme.focused()
                } else if live {
                    theme.value()
                } else {
                    theme.label()
                };
                if armed {
                    p.buf.set_string(x, y, fit_centre(&text, w), style);
                } else {
                    // Right-aligned under the dot, like the gains lines, with
                    // the unit after the value in the focused cell alone.
                    let dot = (w / 2) as u16;
                    let len = text.chars().count() as u16;
                    p.buf
                        .set_string(x + dot + 1 - len.min(dot + 1), y, &text, style);
                    if focused {
                        p.buf.set_string(x + dot + 1, y, unit, style);
                    }
                }
            }
            Row::Mute => {
                let glyph = match (out.mute, ascii) {
                    (true, true) => "*",
                    (false, true) => "o",
                    (true, false) => "●",
                    (false, false) => "○",
                };
                let style = if !live {
                    theme.label()
                } else if out.mute {
                    theme.pill(theme.danger)
                } else if focused {
                    theme.focused()
                } else {
                    theme.label()
                };
                p.buf.set_string(x, y, " ".repeat(w), theme.value());
                p.buf.set_string(x + (self.col_w - 1) / 2, y, glyph, style);
            }
            Row::Input(_) => {}
        }
        if focused {
            self.draw_brackets(p, x, y);
        }
    }

    fn draw_label(&self, p: &mut Paint, y: u16, row: Row, here: bool) {
        let (area, theme, state) = (p.area, p.theme, p.state);
        let x = area.x;
        match row {
            Row::Input(i) => {
                let color = theme.hue_for(ChannelRole::Input(i as u8), here);
                let name = channel_name(state, i);
                let text = fit_left(&name, self.label_w as usize - 2);
                p.buf.set_string(x + 1, y, text, Style::default().fg(color));
            }
            Row::Enable => p.buf.set_string(x + 1, y, "ENABLE", theme.section()),
            Row::Gain => p.buf.set_string(x + 1, y, "GAIN", theme.section()),
            Row::Delay => p.buf.set_string(x + 1, y, "DELAY", theme.section()),
            Row::Mute => p.buf.set_string(x + 1, y, "MUTE", theme.section()),
        }
    }

    /// The ROUTING band. The Console puts its Direct 1:1 and Clear buttons
    /// here; they are the `d` and `D` keys, on the key line, so the band
    /// carries only its name.
    fn draw_routing(&self, p: &mut Paint, y: u16) {
        let (area, theme) = (p.area, p.theme);
        p.buf.set_string(area.x + 1, y, "ROUTING", theme.section());
    }
}

impl Screen for MatrixPanel {
    fn title(&self) -> String {
        "Matrix Mixer".into()
    }

    fn quick(&self, line: &str, state: &DeviceState) -> Option<crate::shell::Quick> {
        Some(self.quick_reply(line, state))
    }

    fn draw(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        state: &DeviceState,
        focused: bool,
    ) {
        if area.height < 4 || area.width < label_width(area.width) + MIN_COL_W {
            return;
        }
        let n_out = self.outputs(state);
        self.col_w = col_width(area.width, n_out);
        self.label_w = label_width(area.width);
        let per_screen = self.visible_columns(area.width, n_out).min(n_out.max(1));
        self.col = self.col.min(n_out.saturating_sub(1));
        // Keep the reticle's column on screen; the label column is pinned and
        // the rest slides under it.
        self.scroll = self
            .scroll
            .min(n_out.saturating_sub(per_screen))
            .min(self.col)
            .max((self.col + 1).saturating_sub(per_screen));
        let cols: Vec<usize> = (self.scroll..(self.scroll + per_screen).min(n_out)).collect();
        let grid_w = self.label_w + cols.len() as u16 * self.col_w;

        let here = self.row_at(state);
        let lines = self.lines(state);
        let body_h = area.height.saturating_sub(2) as usize;
        let cursor_line = lines
            .iter()
            .position(|l| *l == Line::Row(here))
            .unwrap_or(0);
        // An input's gains line travels with its dots line.
        let last_needed = cursor_line + usize::from(matches!(here, Row::Input(_)));
        let first = if lines.len() <= body_h {
            0
        } else {
            last_needed
                .saturating_sub(body_h - 1)
                .min(lines.len() - body_h)
        };

        let mut p = Paint {
            buf,
            theme,
            state,
            area,
        };
        self.draw_headers(&mut p, &cols);

        for (n, line) in lines.iter().skip(first).take(body_h).enumerate() {
            let y = area.y + 2 + n as u16;
            match line {
                Line::Routing => self.draw_routing(&mut p, y),
                Line::Blank => {}
                Line::Divider => {
                    let w = (grid_w - self.label_w).min(area.width.saturating_sub(self.label_w))
                        as usize;
                    let bar = if theme.glyphs == Glyphs::Ascii {
                        "-".repeat(w)
                    } else {
                        "─".repeat(w)
                    };
                    p.buf
                        .set_string(area.x + self.label_w, y, bar, theme.chrome_style());
                }
                Line::Row(row) => {
                    let on_row = focused && *row == here;
                    if on_row {
                        let caret = if theme.glyphs == Glyphs::Ascii {
                            ">"
                        } else {
                            "▸"
                        };
                        p.buf.set_string(area.x, y, caret, theme.focused());
                    }
                    self.draw_label(&mut p, y, *row, on_row);
                    for (i, &o) in cols.iter().enumerate() {
                        let x = area.x + self.label_w + i as u16 * self.col_w;
                        let cell = on_row && o == self.col;
                        match row {
                            Row::Input(input) => self.draw_connect(&mut p, x, y, *input, o, cell),
                            other => self.draw_output_cell(&mut p, x, y, *other, o, cell),
                        }
                    }
                }
                Line::Gains(input) => {
                    let on_row = focused && Row::Input(*input) == here;
                    for (i, &o) in cols.iter().enumerate() {
                        let x = area.x + self.label_w + i as u16 * self.col_w;
                        let cell = on_row && o == self.col;
                        self.draw_gain(&mut p, x, y, *input, o, cell);
                    }
                }
            }
        }
    }

    fn handle(&mut self, key: KeyEvent, state: &DeviceState) -> ScreenEvent {
        let coarse = key.modifiers.contains(KeyModifiers::SHIFT);
        if self.edit.is_some() {
            return self.handle_edit(key, state, coarse);
        }
        let rows = self.rows(state).len();
        let outputs = self.outputs(state);
        match key.code {
            KeyCode::Up => {
                self.row = self.row.saturating_sub(1);
                ScreenEvent::Handled
            }
            KeyCode::Down => {
                self.row = (self.row + 1).min(rows - 1);
                ScreenEvent::Handled
            }
            KeyCode::Left => {
                self.col = self.col.saturating_sub(1);
                ScreenEvent::Handled
            }
            KeyCode::Right => {
                self.col = (self.col + 1).min(outputs.saturating_sub(1));
                ScreenEvent::Handled
            }
            KeyCode::Char(' ') => self.activate(state, false),
            KeyCode::Enter => self.activate(state, true),
            KeyCode::Char('i') => match self.row_at(state) {
                Row::Input(i) => {
                    let c = state.crosspoint(i, self.col);
                    ScreenEvent::Command(self.mix_command(
                        i,
                        self.col,
                        c.enabled,
                        c.gain_db,
                        !c.phase_invert,
                    ))
                }
                _ => ScreenEvent::Handled,
            },
            KeyCode::Char('d') => {
                let cmds = if is_8ch(state) {
                    self.direct_routing(state)
                } else {
                    Vec::new()
                };
                if cmds.is_empty() {
                    ScreenEvent::Handled
                } else {
                    ScreenEvent::Command(cmds.join("\n"))
                }
            }
            KeyCode::Char('D') => {
                if !is_8ch(state) {
                    return ScreenEvent::Handled;
                }
                self.pending = Some(Pending::Clear);
                ScreenEvent::Dialog(Dialog::confirm(
                    "Clear",
                    "Disconnect every crosspoint",
                    vec![Button::destructive("Clear"), Button::new("Cancel")],
                ))
            }
            KeyCode::Char('r') => {
                let channel = output_channel(state, self.col);
                self.pending = Some(Pending::Rename(channel));
                ScreenEvent::Dialog(Dialog::text(
                    "Rename",
                    "",
                    state.channel_name(channel),
                    "Name",
                ))
            }
            KeyCode::Char('y') => {
                let clip = clipboard::copy(state, output_channel(state, self.col));
                let msg = clipboard::copied_message(&clip);
                self.shared.borrow_mut().clipboard = Some(clip);
                ScreenEvent::Status(msg)
            }
            KeyCode::Char('Y') => {
                let clip = self.shared.borrow().clipboard.clone();
                let Some(clip) = clip else {
                    return ScreenEvent::Status("Nothing to paste".into());
                };
                let channel = output_channel(state, self.col);
                let cmds = clipboard::paste_commands(&clip, state, channel, None);
                ScreenEvent::Command(cmds.join("\n"))
            }
            // The same blip melody the sidebar's `i` plays, on the output the
            // reticle is over.
            KeyCode::Char('I') => match super::identify_command(state, self.col) {
                Some(cmds) => ScreenEvent::Command(cmds),
                None => ScreenEvent::Status("Firmware has no signal generator".into()),
            },
            _ => ScreenEvent::Unhandled,
        }
    }

    fn keys(&self) -> &'static [KeyHelp] {
        KEYS
    }

    fn dialog_result(&mut self, outcome: DialogOutcome, state: &DeviceState) -> ScreenEvent {
        let Some(pending) = self.pending.take() else {
            return ScreenEvent::Handled;
        };
        match (pending, outcome) {
            (Pending::Clear, DialogOutcome::Button(0)) => {
                let cmds = self.clear_routes(state);
                if cmds.is_empty() {
                    ScreenEvent::Handled
                } else {
                    ScreenEvent::Command(cmds.join("\n"))
                }
            }
            (Pending::Rename(channel), DialogOutcome::Text(name)) => {
                let name = name.trim().to_string();
                if name.is_empty() {
                    ScreenEvent::Handled
                } else {
                    ScreenEvent::Command(format!("ch.name {channel} {name}"))
                }
            }
            (Pending::Enable(o), DialogOutcome::Button(0)) => {
                ScreenEvent::Command(self.enable_confirmed(state, o).join("\n"))
            }
            _ => ScreenEvent::Handled,
        }
    }
}

impl MatrixPanel {
    /// `Space` and `Enter`: `Enter` always arms a value, `Space` toggles what
    /// can be toggled and arms what cannot.
    fn activate(&mut self, state: &DeviceState, enter: bool) -> ScreenEvent {
        match self.row_at(state) {
            Row::Input(i) if !enter => {
                let c = state.crosspoint(i, self.col);
                ScreenEvent::Command(self.mix_command(
                    i,
                    self.col,
                    !c.enabled,
                    c.gain_db,
                    c.phase_invert,
                ))
            }
            Row::Input(_) | Row::Gain | Row::Delay => self.arm(state),
            Row::Enable => self.request_enable(state),
            Row::Mute => {
                let o = self.col;
                let muted = state.output(o).mute;
                ScreenEvent::Command(format!("out.mute {o} {}", if muted { "off" } else { "on" }))
            }
        }
    }

    fn arm(&mut self, state: &DeviceState) -> ScreenEvent {
        if let Some(v) = self.value_at(state) {
            self.edit = Some(NumberEdit::start(v, 1));
        }
        ScreenEvent::Handled
    }

    fn handle_edit(&mut self, key: KeyEvent, state: &DeviceState, coarse: bool) -> ScreenEvent {
        if matches!(key.code, KeyCode::Left | KeyCode::Right) {
            let dir = if key.code == KeyCode::Left { -1.0 } else { 1.0 };
            let v = self.nudge(state, dir, coarse);
            let ev = self.write_at(state, v);
            self.edit = Some(NumberEdit::start(v, 1));
            return ev;
        }
        let Some(edit) = self.edit.as_mut() else {
            return ScreenEvent::Handled;
        };
        match edit.handle(key) {
            Some(crate::widgets::Action::Committed(v)) => {
                self.edit = None;
                self.write_at(state, v)
            }
            Some(crate::widgets::Action::Closed) => {
                self.edit = None;
                ScreenEvent::Handled
            }
            _ => ScreenEvent::Handled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::{Overview, shared};
    use crate::shell::{Focus, Shell, Tool, fixture};
    use crate::theme::ColorDepth;
    use crate::widgets::testing::key;
    use dspi_proto::generated::SECTIONS;
    use dspi_proto::wire::BulkPacket;

    fn theme() -> Theme {
        Theme::console(ColorDepth::TrueColor, Glyphs::Braille)
    }

    fn section(name: &str) -> usize {
        SECTIONS
            .iter()
            .find(|(n, _, _)| *n == name)
            .map(|(_, o, _)| *o)
            .expect("section")
    }

    fn edited(f: impl FnOnce(&mut [u8])) -> DeviceState {
        let mut s = fixture::state();
        let mut b = s.bulk.as_bytes().to_vec();
        f(&mut b);
        s.replace_bulk(BulkPacket::decode(b).expect("packet"));
        s
    }

    /// The fixture with some outputs switched off.
    fn outputs_off(off: &[usize]) -> DeviceState {
        let base = section("outputs");
        edited(|b| {
            for o in off {
                b[base + o * 12] = 0;
            }
        })
    }

    /// The fixture with one more crosspoint connected.
    fn connected(input: usize, output: usize) -> DeviceState {
        let base = section("crosspoints");
        edited(|b| b[base + (input * 9 + output) * 8] = 1)
    }

    /// A two-input device: the Console's stereo matrix.
    fn stereo() -> DeviceState {
        let mut s = fixture::state();
        s.caps.num_inputs = 2;
        s
    }

    fn panel() -> MatrixPanel {
        MatrixPanel::new(shared())
    }

    fn draw(p: &mut MatrixPanel, state: &DeviceState, w: u16, h: u16) -> ratatui::buffer::Buffer {
        let t = theme();
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        term.draw(|f| p.draw(f.area(), f.buffer_mut(), &t, state, true))
            .unwrap();
        term.backend().buffer().clone()
    }

    fn text(buf: &ratatui::buffer::Buffer) -> String {
        let a = buf.area();
        (0..a.height)
            .map(|y| {
                (0..a.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The panel in the shell's tool region, which is where a person meets it.
    fn shell() -> Shell {
        let t = theme();
        let model = fixture::rp2350(&t);
        let mut s = Shell::new(model, t, Box::new(Overview::new(shared())));
        s.open_tool(Tool::Matrix, Box::new(panel()));
        s.focus = Focus::Screen;
        s
    }

    fn frame(s: &mut Shell, w: u16, h: u16) -> String {
        let state = fixture::state();
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        term.draw(|f| s.draw(f.area(), f.buffer_mut(), &state))
            .unwrap();
        text(term.backend().buffer())
    }

    // -- Golden frames -----------------------------------------------------

    #[test]
    fn the_panel_fills_the_pane_at_every_size() {
        let mut s = shell();
        for (w, h) in [(80u16, 24u16), (120, 40), (200, 60)] {
            let f = frame(&mut s, w, h);
            assert_eq!(f.lines().count(), h as usize, "{w}x{h}");
            assert!(f.contains("Matrix Mixer"), "{w}x{h}: {f}");
            assert!(f.contains("INPUTS"), "the sidebar is still there: {f}");
            for want in ["ROUTING", "OUT1", "FL"] {
                assert!(f.contains(want), "{w}x{h} has no {want}:\n{f}");
            }
            // Two lines per input: at 80x24 the output rows are below the
            // fold and the reticle scrolls them in; from 120x40 they show.
            if h >= 40 {
                for want in ["ENABLE", "GAIN", "DELAY", "MUTE"] {
                    assert!(f.contains(want), "{w}x{h} has no {want}:\n{f}");
                }
            }
            // The routing actions are the `d` and `D` keys on the key line,
            // not text in the ROUTING band.
            let band = f.lines().find(|l| l.contains("ROUTING")).expect("ROUTING");
            assert!(!band.contains("Direct 1:1"), "{band}");
        }
    }

    #[test]
    fn every_column_fits_at_the_wide_density_and_scrolls_below_it() {
        // The pane a tool panel gets at the Wide density, and at the reference
        // layout. The panel is drawn alone because the sidebar has its own
        // OUT9 row and its own guillemets.
        let state = fixture::state();
        let wide = text(&draw(&mut panel(), &state, 170, 20));
        for o in 1..=9 {
            assert!(
                wide.contains(&format!("OUT{o}")),
                "OUT{o} is missing:\n{wide}"
            );
        }
        assert!(
            !wide.contains('\u{203a}'),
            "nothing left to scroll to:\n{wide}"
        );

        // The Normal pane holds all nine as well, now that a column is nine
        // cells; only the Compact pane, 56 wide, has to scroll.
        let normal = text(&draw(&mut panel(), &state, 90, 20));
        assert!(normal.contains("OUT9"), "all nine:\n{normal}");
        let compact = text(&draw(&mut panel(), &state, 56, 20));
        assert!(
            !compact.contains("OUT9"),
            "the last columns wait:\n{compact}"
        );
        assert!(
            compact.contains('\u{203a}'),
            "and the frame says so:\n{compact}"
        );
    }

    #[test]
    fn the_label_column_stays_pinned_while_the_grid_scrolls() {
        let mut p = panel();
        let state = fixture::state();
        for _ in 0..8 {
            p.handle(key(KeyCode::Right), &state);
        }
        let f = text(&draw(&mut p, &state, 56, 40));
        assert!(
            f.contains("OUT9"),
            "the reticle's column is on screen:\n{f}"
        );
        assert!(
            !f.contains("OUT1 "),
            "the first columns have scrolled away:\n{f}"
        );
        assert!(
            f.contains("FL") && f.contains("SR"),
            "the labels are pinned:\n{f}"
        );
        assert!(f.contains('‹'), "and the frame says so:\n{f}");
    }

    // -- The grid ----------------------------------------------------------

    #[test]
    fn a_crosspoint_shows_its_state_its_gain_and_its_phase() {
        let state = connected(0, 3);
        let mut p = panel();
        let f = text(&draw(&mut p, &state, 120, 20));
        let fl = f.lines().nth(3).expect("the FL dots row");
        assert!(fl.contains('●') && fl.contains('○'), "{fl}");
        assert!(!fl.contains("0.0"), "gains are on the next line: {fl}");
        let gains = f.lines().nth(4).expect("the FL gains row");
        // The reticle is on FL/OUT1, so that cell alone carries its unit.
        assert!(gains.contains("0.0 dB") && gains.contains('['), "{gains}");
        assert_eq!(gains.matches("dB").count(), 1, "{gains}");
        let fr = f.lines().nth(7).expect("the FR gains row");
        assert!(fr.contains("0.0") && !fr.contains("dB"), "{fr}");
        assert!(
            !gains.contains("+0.0 dB") && !f.contains("-5.3 dB"),
            "no input trim in the matrix; it lives on the input page: {f}"
        );
    }

    /// D52: `DESIGN.md` 7.7 says "Column headers in the output colour". Only
    /// the descriptor row had it; the name row above was drawn as a caption.
    #[test]
    fn both_header_rows_are_in_the_columns_output_colour() {
        let t = theme();
        let f = draw(&mut panel(), &fixture::state(), 200, 20);
        let col_w = col_width(200, 9);
        for (col, channel) in [(0usize, 8u8), (2, 10), (8, 16)] {
            let x = label_width(200) + col as u16 * col_w + 2;
            let want = t.channel_of(channel, 8, 9);
            assert_eq!(f[(x, 0)].fg, want, "the name row of column {col}");
            assert_eq!(f[(x, 1)].fg, want, "the descriptor row of column {col}");
        }
    }

    #[test]
    fn a_disabled_outputs_cells_draw_dim() {
        let t = theme();
        // The fixture connects input 2 to output 2, so the cell has a colour to
        // lose. Line order: headers, ROUTING, then three lines per input, so
        // FC's dots are on line 9.
        let col_w = col_width(120, 9);
        let label_w = label_width(120);
        let (x, y) = (label_w + 2 * col_w + (col_w - 1) / 2, 9u16);
        let live = draw(&mut panel(), &fixture::state(), 120, 34);
        assert_eq!(live[(x, y)].symbol(), "●");
        assert_eq!(
            live[(x, y)].fg,
            t.channel_of(10, 8, 9),
            "the output's colour"
        );

        let off = draw(&mut panel(), &outputs_off(&[2]), 120, 34);
        assert_eq!(off[(x, y)].symbol(), "●", "still connected");
        assert_eq!(off[(x, y)].fg, t.dim, "but the whole column is dim");
        // And so is the output's own strip beneath it: the GAIN row is line
        // 29, after twenty-four input lines and the rule.
        let x = label_w + 2 * col_w;
        let gain: Vec<_> = (0..col_w - 1)
            .map(|dx| &off[(x + dx, 29)])
            .filter(|c| c.symbol() != " ")
            .map(|c| c.fg)
            .collect();
        assert!(
            !gain.is_empty() && gain.iter().all(|c| *c == t.dim),
            "the GAIN row: {gain:?}"
        );
    }

    /// D51: the interlock is symmetric, so the marker is too. Marking only
    /// PDM said that enabling an EQ-worker output while PDM runs was free,
    /// when the ENABLE row was already colouring both sides orange.
    #[test]
    fn a_core_one_collision_marks_both_sides_of_the_interlock() {
        // The mark sits in the descriptor row, once per column.
        let count = |f: &str| {
            f.lines()
                .nth(1)
                .map(|l| l.matches(" !").count())
                .unwrap_or(0)
        };

        // Every output is on in the fixture, so every column on both sides of
        // the interlock is in collision: PDM and the six EQ workers.
        let f = text(&draw(&mut panel(), &fixture::state(), 200, 20));
        assert_eq!(count(&f), 7, "PDM plus outputs 3 to 8:\n{f}");
        assert!(!f.contains("○!"), "no marks in the cells:\n{f}");

        // With the EQ workers off, PDM is free, but each of them would still
        // collide with the PDM output that is still running.
        let clear = outputs_off(&[2, 3, 4, 5, 6, 7]);
        let f = text(&draw(&mut panel(), &clear, 200, 20));
        assert_eq!(count(&f), 6, "the six EQ workers, not PDM:\n{f}");

        // With PDM off as well, nothing is in collision with anything.
        let clear = outputs_off(&[2, 3, 4, 5, 6, 7, 8]);
        let f = text(&draw(&mut panel(), &clear, 200, 20));
        assert_eq!(count(&f), 0, "{f}");
    }

    #[test]
    fn inputs_stand_alone_and_one_rule_parts_them_from_the_outputs() {
        let f = text(&draw(&mut panel(), &fixture::state(), 120, 36));
        let dividers = f.lines().filter(|l| l.contains("──")).count();
        assert_eq!(dividers, 1, "one rule above ENABLE:\n{f}");
        let lines: Vec<&str> = f.lines().collect();
        assert!(lines[3].starts_with("▸FL") && lines[4].contains("0.0 dB"));
        assert!(
            lines[4].starts_with("        "),
            "the gains line has no label: {f}"
        );
        assert!(lines[5].is_empty(), "a blank line after the gains:\n{f}");
        assert!(lines[6].starts_with(" FR"), "no pair divider:\n{f}");
        assert!(lines[9].starts_with(" FC"), "{f}");
        let s = text(&draw(&mut panel(), &stereo(), 120, 36));
        let dividers = s.lines().filter(|l| l.contains("──")).count();
        assert_eq!(dividers, 1, "the same rule in stereo:\n{s}");
    }

    #[test]
    fn the_quick_grammar_routes_sets_gains_and_toggles() {
        let state = fixture::state();
        let p = panel();
        let q = |line: &str| p.quick_reply(line, &state);

        let route = q("1 2 > 3 4");
        assert_eq!(
            route.commands,
            vec![
                "mix 0 2 on 0",
                "mix 0 3 on 0",
                "mix 1 2 on 0",
                "mix 1 3 on 0"
            ],
            "{route:?}"
        );
        assert!(route.hint.contains("IN1 IN2 → OUT3 OUT4"), "{}", route.hint);

        let gained = q("1 > 3 -6 inv");
        assert_eq!(gained.commands, vec!["mix 0 2 on -6 inv"]);

        let cut = q("1-4 x all");
        assert_eq!(cut.commands.len(), 4 * 9);
        assert!(cut.commands[0].contains(" off "), "{:?}", cut.commands[0]);

        // The fixture connects IN1→OUT1; gain keeps the enable, inv toggles.
        let g = q("gain 1 1 -6");
        assert_eq!(g.commands, vec!["mix 0 0 on -6"]);
        let i = q("inv 1 1");
        assert_eq!(i.commands, vec!["mix 0 0 on 0 inv"]);

        let m = q("out 3-4 mute");
        assert_eq!(m.commands, vec!["out.mute 2 on", "out.mute 3 on"]);
        let e = q("out 5 off");
        assert_eq!(e.commands, vec!["out.enable 4 off"]);
        let d = q("out 9 delay 2.5");
        assert_eq!(d.commands, vec!["out.delay 8 2.5"]);

        assert!(!q("direct").commands.is_empty());
        assert!(!q("clear").commands.is_empty());

        // Incomplete lines hint and run nothing.
        for partial in ["", "1 2", "1 >", "gain 1", "out", "out 3 gain"] {
            let r = q(partial);
            assert!(r.commands.is_empty(), "{partial:?} ran {:?}", r.commands);
            assert!(!r.hint.is_empty(), "{partial:?} has no hint");
        }
        assert_eq!(q("g").ghost.as_deref(), Some("ain"));
    }

    #[test]
    fn columns_widen_with_the_pane() {
        assert_eq!(col_width(90, 9), 9, "120x40's pane: nine columns of nine");
        assert_eq!(label_width(90), 9);
        assert_eq!(
            col_width(166, 9),
            16,
            "200x60's pane: sixteen, all nine fit"
        );
        assert_eq!(col_width(400, 9), MAX_COL_W, "capped");
        assert_eq!(col_width(90, 5), 16, "five outputs get wider columns");
        // Nine columns fit the Normal pane beside the label column.
        let f = text(&draw(&mut panel(), &fixture::state(), 90, 30));
        let head = f.lines().nth(1).unwrap();
        assert!(head.contains("OUT1") && head.contains("OUT9"), "{head}");
        assert!(!f.contains('›'), "nothing to scroll to: {f}");
    }

    // -- Routing -----------------------------------------------------------

    #[test]
    fn direct_one_to_one_routes_the_diagonal_and_frees_core_one() {
        let state = connected(0, 3);
        let mut p = panel();
        let ScreenEvent::Command(c) = p.handle(key(KeyCode::Char('d')), &state) else {
            panic!("Direct 1:1 wrote nothing");
        };
        let lines: Vec<&str> = c.lines().collect();
        // The PDM sub goes first, so the EQ workers are free to come up.
        assert_eq!(lines[0], "out.enable 8 off");
        // The stray route is taken down; the diagonal is already right, so
        // nothing else is written.
        assert_eq!(lines[1], "mix 0 3 off 0");
        assert_eq!(lines.len(), 2, "{lines:?}");
    }

    #[test]
    fn direct_one_to_one_does_nothing_on_a_two_input_device() {
        let mut p = panel();
        assert_eq!(
            p.handle(key(KeyCode::Char('d')), &stereo()),
            ScreenEvent::Handled,
            "the Console only offers it in 8-channel mode"
        );
        assert_eq!(
            p.handle(key(KeyCode::Char('D')), &stereo()),
            ScreenEvent::Handled
        );
    }

    #[test]
    fn clear_confirms_then_disconnects_every_crosspoint() {
        let state = fixture::state();
        let mut p = panel();
        let ScreenEvent::Dialog(d) = p.handle(key(KeyCode::Char('D')), &state) else {
            panic!("Clear asks first");
        };
        assert_eq!(d.body, "Disconnect every crosspoint");
        assert!(d.buttons[0].destructive);
        let ScreenEvent::Command(c) = p.dialog_result(DialogOutcome::Button(0), &state) else {
            panic!("Clear wrote nothing");
        };
        // The fixture's eight diagonal routes, gains and phase left alone.
        assert_eq!(c.lines().count(), 8, "{c}");
        assert_eq!(c.lines().next().unwrap(), "mix 0 0 off 0");
        // Cancel writes nothing.
        p.handle(key(KeyCode::Char('D')), &state);
        assert_eq!(
            p.dialog_result(DialogOutcome::Cancelled, &state),
            ScreenEvent::Handled
        );
    }

    // -- Crosspoint and strip edits ---------------------------------------

    #[test]
    fn space_connects_and_i_inverts_a_crosspoint() {
        let state = fixture::state();
        let mut p = panel();
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &state),
            ScreenEvent::Command("mix 0 0 off 0".into())
        );
        assert_eq!(
            p.handle(key(KeyCode::Char('i')), &state),
            ScreenEvent::Command("mix 0 0 on 0 inv".into())
        );
    }

    #[test]
    fn enter_arms_a_gain_and_the_arrows_nudge_it_by_half_a_decibel() {
        let state = fixture::state();
        let mut p = panel();
        p.handle(key(KeyCode::Enter), &state);
        assert!(p.edit.is_some(), "armed");
        assert_eq!(
            p.handle(key(KeyCode::Left), &state),
            ScreenEvent::Command("mix 0 0 on -0.5".into())
        );
        // Typing over the armed field commits on Enter.
        for c in "-6".chars() {
            p.handle(key(KeyCode::Char(c)), &state);
        }
        assert_eq!(
            p.handle(key(KeyCode::Enter), &state),
            ScreenEvent::Command("mix 0 0 on -6".into())
        );
        assert!(p.edit.is_none());
    }

    #[test]
    fn the_output_rows_write_enable_gain_delay_and_mute() {
        let state = fixture::state();
        let mut p = panel();
        for _ in 0..8 {
            p.handle(key(KeyCode::Down), &state);
        }
        assert_eq!(p.row_at(&state), Row::Enable);
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &state),
            ScreenEvent::Command("out.enable 0 off".into())
        );
        p.handle(key(KeyCode::Down), &state);
        p.handle(key(KeyCode::Enter), &state);
        assert_eq!(
            p.handle(key(KeyCode::Right), &state),
            ScreenEvent::Command("out.gain 0 0.5".into())
        );
        p.handle(key(KeyCode::Esc), &state);
        p.handle(key(KeyCode::Down), &state);
        p.handle(key(KeyCode::Enter), &state);
        assert_eq!(
            p.handle(key(KeyCode::Right), &state),
            ScreenEvent::Command("out.delay 0 0.1".into())
        );
        p.handle(key(KeyCode::Esc), &state);
        p.handle(key(KeyCode::Down), &state);
        assert_eq!(p.row_at(&state), Row::Mute);
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &state),
            ScreenEvent::Command("out.mute 0 on".into())
        );
    }

    #[test]
    fn a_typed_delay_is_clamped_to_the_platforms_maximum() {
        let state = fixture::state();
        let mut p = panel();
        for _ in 0..10 {
            p.handle(key(KeyCode::Down), &state);
        }
        assert_eq!(p.row_at(&state), Row::Delay);
        p.handle(key(KeyCode::Enter), &state);
        for c in "200".chars() {
            p.handle(key(KeyCode::Char(c)), &state);
        }
        assert_eq!(
            p.handle(key(KeyCode::Enter), &state),
            ScreenEvent::Command("out.delay 0 85".into())
        );
    }

    // -- The Core 1 interlock ---------------------------------------------

    #[test]
    fn enabling_pdm_raises_the_consoles_first_alert() {
        let state = outputs_off(&[8]);
        let mut p = panel();
        for _ in 0..8 {
            p.handle(key(KeyCode::Down), &state);
        }
        for _ in 0..8 {
            p.handle(key(KeyCode::Right), &state);
        }
        let ScreenEvent::Dialog(d) = p.handle(key(KeyCode::Char(' ')), &state) else {
            panic!("no alert");
        };
        assert_eq!(d.title, "Warning");
        assert_eq!(d.body, "Outputs 3-8 will be disabled. Are you sure?");
        assert_eq!(d.buttons[0].label, "Enable PDM");
        assert_eq!(d.buttons[1].label, "Cancel");
        let ScreenEvent::Command(c) = p.dialog_result(DialogOutcome::Button(0), &state) else {
            panic!("no write");
        };
        let lines: Vec<&str> = c.lines().collect();
        assert_eq!(lines.first(), Some(&"out.enable 2 off"));
        assert_eq!(lines.last(), Some(&"out.enable 8 on"));
        assert_eq!(lines.len(), 7, "six EQ workers, then PDM: {lines:?}");
    }

    #[test]
    fn enabling_an_eq_worker_raises_the_consoles_second_alert() {
        let state = outputs_off(&[3]);
        let mut p = panel();
        for _ in 0..8 {
            p.handle(key(KeyCode::Down), &state);
        }
        for _ in 0..3 {
            p.handle(key(KeyCode::Right), &state);
        }
        let ScreenEvent::Dialog(d) = p.handle(key(KeyCode::Char(' ')), &state) else {
            panic!("no alert");
        };
        assert_eq!(d.title, "Warning");
        assert_eq!(d.body, "The PDM output will be disabled. Are you sure?");
        assert_eq!(d.buttons[0].label, "Disable PDM");
        assert_eq!(
            p.dialog_result(DialogOutcome::Button(0), &state),
            ScreenEvent::Command("out.enable 8 off\nout.enable 3 on".into())
        );
    }

    #[test]
    fn an_enable_with_nothing_to_collide_with_is_written_straight_out() {
        // PDM off and no EQ workers on: turning output 3 on takes nobody down.
        let state = outputs_off(&[2, 3, 4, 5, 6, 7, 8]);
        let mut p = panel();
        for _ in 0..8 {
            p.handle(key(KeyCode::Down), &state);
        }
        for _ in 0..3 {
            p.handle(key(KeyCode::Right), &state);
        }
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &state),
            ScreenEvent::Command("out.enable 3 on".into())
        );
    }

    // -- Column actions ----------------------------------------------------

    #[test]
    fn r_renames_the_columns_output() {
        let state = fixture::state();
        let mut p = panel();
        let ScreenEvent::Dialog(d) = p.handle(key(KeyCode::Char('r')), &state) else {
            panic!("no rename dialog");
        };
        assert_eq!(d.title, "Rename");
        assert_eq!(
            p.dialog_result(DialogOutcome::Text("Left ".into()), &state),
            ScreenEvent::Command("ch.name 8 Left".into())
        );
        // An empty name leaves the channel alone, as the Console does.
        p.handle(key(KeyCode::Char('r')), &state);
        assert_eq!(
            p.dialog_result(DialogOutcome::Text("  ".into()), &state),
            ScreenEvent::Handled
        );
    }

    #[test]
    fn y_and_shift_y_move_a_columns_parameters_through_the_clipboard() {
        let state = fixture::state();
        let bench = shared();
        let mut p = MatrixPanel::new(bench.clone());
        assert_eq!(
            p.handle(key(KeyCode::Char('Y')), &state),
            ScreenEvent::Status("Nothing to paste".into())
        );
        assert_eq!(
            p.handle(key(KeyCode::Char('y')), &state),
            ScreenEvent::Status("Copied OUT L parameters".into())
        );
        assert!(bench.borrow().clipboard.is_some());
        p.handle(key(KeyCode::Right), &state);
        let ScreenEvent::Command(c) = p.handle(key(KeyCode::Char('Y')), &state) else {
            panic!("nothing pasted");
        };
        assert!(c.contains("out.gain 1 0"), "{c}");
        assert!(c.contains("eq out.2 20 highpass 80 0.707 0"), "{c}");
    }

    /// D23: `I` was a placeholder long after the signal generator arrived. It
    /// plays the same blip melody the sidebar's `i` does, on the column the
    /// reticle is over.
    #[test]
    fn identify_plays_the_channel_id_tone_on_the_column_under_the_reticle() {
        let mut p = panel();
        let state = crate::screens::panel::testing::state();
        p.col = 2;
        match p.handle(key(KeyCode::Char('I')), &state) {
            ScreenEvent::Command(c) => {
                assert!(c.starts_with("sig.config type=channel-id"), "{c}");
                assert!(c.contains("channels=0x4"), "the third output: {c}");
                assert!(c.contains("flags=walk") && c.contains("p1=120"), "{c}");
                assert!(c.ends_with("sig.control start"), "{c}");
            }
            other => panic!("{other:?}"),
        }
        // A firmware with no generator says so rather than writing nothing.
        assert_eq!(
            p.handle(key(KeyCode::Char('I')), &fixture::state()),
            ScreenEvent::Status("Firmware has no signal generator".into())
        );
    }

    // -- Keys --------------------------------------------------------------

    #[test]
    fn every_advertised_key_is_handled() {
        let state = fixture::state();
        for help in KEYS {
            for k in crate::screens::tests::keys_for(help.key) {
                let mut done = false;
                for (row, col) in [(0usize, 0usize), (8, 8)] {
                    let mut p = panel();
                    p.row = row;
                    p.col = col;
                    let before = (p.row, p.col, p.edit.clone());
                    let ev = p.handle(k, &state);
                    let after = (p.row, p.col, p.edit.clone());
                    if ev != ScreenEvent::Unhandled || before != after {
                        done = true;
                    }
                }
                assert!(done, "{:?} does nothing (from {:?})", k.code, help.key);
            }
        }
    }

    #[test]
    fn the_graph_keys_still_reach_the_shell() {
        let state = fixture::state();
        let mut p = panel();
        for c in ['g', 'p', 'b', 'c', '=', '+', '-'] {
            assert_eq!(
                p.handle(key(KeyCode::Char(c)), &state),
                ScreenEvent::Unhandled,
                "{c} belongs to the shell"
            );
        }
    }
}
