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
use crate::widgets::text::{fit_centre, fit_left, fit_right};
use crate::widgets::{Button, Dialog, DialogOutcome, KeyHelp, NumberEdit};

/// The pinned input-label column: a name, its trim, and air between them.
const LABEL_W: u16 = 18;
/// The narrowest output column: the connect dot, the gain, `INV`, and a
/// gutter that doubles as the focus bracket of the next column. Columns
/// grow up to [`MAX_COL_W`] when the pane has the room, so the grid is
/// never more crowded than it has to be.
const MIN_COL_W: u16 = 13;
const MAX_COL_W: u16 = 18;

/// The column width for a pane this wide showing `n_out` outputs.
fn col_width(width: u16, n_out: usize) -> u16 {
    (width.saturating_sub(LABEL_W) / n_out.max(1) as u16).clamp(MIN_COL_W, MAX_COL_W)
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
    /// The reticle: a row, an output column, and whether it sits on the input
    /// trim in the pinned label column instead.
    row: usize,
    col: usize,
    trim: bool,
    /// The leftmost visible output column.
    scroll: usize,
    /// The column width of the last frame, from [`col_width`].
    col_w: u16,
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
            trim: false,
            scroll: 0,
            col_w: MIN_COL_W,
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

    /// Every line the body draws, in order: the ROUTING band, one row per
    /// input, a rule, then the output rows. The Console divides the inputs
    /// into stereo pairs; here every input stands on its own, since the
    /// pairing said nothing the names do not.
    fn lines(&self, state: &DeviceState) -> Vec<Line> {
        let n = self.inputs(state);
        let mut v = vec![Line::Routing];
        for i in 0..n {
            v.push(Line::Row(Row::Input(i)));
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
        (width.saturating_sub(LABEL_W) / col_width(width, n_out)).max(1) as usize
    }

    /// Is the trim field reachable from where the reticle is?
    fn trim_available(&self, state: &DeviceState) -> bool {
        is_8ch(state) && matches!(self.row_at(state), Row::Input(_))
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
        if self.trim {
            let Row::Input(i) = self.row_at(state) else {
                return None;
            };
            return Some(state.preamp_db(i) as f64);
        }
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
        if self.trim {
            let Row::Input(i) = self.row_at(state) else {
                return ScreenEvent::Handled;
            };
            let v = v.clamp(GAIN_MIN, GAIN_MAX);
            return ScreenEvent::Command(format!("pre {i} {}", number(v as f32)));
        }
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
        let (value, step) = match (self.trim, row) {
            (true, Row::Input(i)) => (state.preamp_db(i) as f64, 0.5),
            (_, Row::Input(i)) => (state.crosspoint(i, self.col).gain_db as f64, 0.5),
            (_, Row::Gain) => (state.output(self.col).gain_db as f64, 0.5),
            (_, Row::Delay) => (state.output(self.col).delay_ms as f64, 0.1),
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
    fn draw_headers(&self, p: &mut Paint, cols: &[usize]) {
        let (area, theme, state) = (p.area, p.theme, p.state);
        for (n, &o) in cols.iter().enumerate() {
            let x = area.x + LABEL_W + n as u16 * self.col_w;
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
                area.x + LABEL_W - 1,
                area.y + 1,
                if ascii { "<" } else { "‹" },
                theme.chrome_style(),
            );
        }
        if self.scroll + cols.len() < state.caps.num_outputs as usize {
            let x = area.x + LABEL_W + cols.len() as u16 * self.col_w - 1;
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

    fn draw_crosspoint(
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
        let dot_style = if live && c.enabled {
            Style::default().fg(column_color(state, theme, output, output == self.col))
        } else {
            theme.label()
        };
        p.buf.set_string(x, y, dot, dot_style);

        match self.edit.as_ref().filter(|_| focused) {
            Some(e) => {
                let text = fit_right(&format!("[{}]", e.text), 9);
                p.buf.set_string(x + 2, y, text, theme.editing());
            }
            None => {
                // Connected cells right-align so the decimals line up down the
                // column; a disconnected one has no number to line up.
                let (gain, style) = if c.enabled {
                    (fit_right(&format!("{:.1}", c.gain_db), 5), theme.value())
                } else {
                    (fit_centre("-", 5), theme.label())
                };
                let style = if !live {
                    theme.label()
                } else if focused {
                    theme.focused()
                } else {
                    style
                };
                p.buf.set_string(x + 2, y, gain, style);
                if c.phase_invert {
                    let style = if live {
                        theme.warning_style()
                    } else {
                        theme.label()
                    };
                    p.buf.set_string(x + 8, y, "INV", style);
                }
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
                let text = match (self.edit.as_ref().filter(|_| focused), row) {
                    (Some(e), _) => format!("[{}]", e.text),
                    (None, Row::Gain) => format!("{:.1} dB", out.gain_db),
                    (None, _) => format!("{:.1} ms", out.delay_ms),
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
                p.buf.set_string(x, y, fit_centre(&text, w), style);
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
                if !is_8ch(state) {
                    let text = fit_left(&name, LABEL_W as usize - 2);
                    p.buf.set_string(x + 1, y, text, Style::default().fg(color));
                    return;
                }
                p.buf
                    .set_string(x + 1, y, fit_left(&name, 7), Style::default().fg(color));
                // The Console's per-input trim, bound to `preampDB[input]`.
                let focused = here && self.trim;
                let text = match self.edit.as_ref().filter(|_| focused) {
                    Some(e) => format!("[{}]", e.text),
                    None => format!("{:+.1} dB", state.preamp_db(i)),
                };
                let style = if focused && self.edit.is_some() {
                    theme.editing()
                } else if focused {
                    theme.focused()
                } else {
                    theme.value()
                };
                p.buf.set_string(x + 8, y, fit_right(&text, 8), style);
            }
            Row::Enable => p.buf.set_string(x + 1, y, "ENABLE", theme.section()),
            Row::Gain => p.buf.set_string(x + 1, y, "GAIN", theme.section()),
            Row::Delay => p.buf.set_string(x + 1, y, "DELAY", theme.section()),
            Row::Mute => p.buf.set_string(x + 1, y, "MUTE", theme.section()),
        }
    }

    fn draw_routing(&self, p: &mut Paint, y: u16, grid_w: u16) {
        let (area, theme, state) = (p.area, p.theme, p.state);
        p.buf.set_string(area.x + 1, y, "ROUTING", theme.section());
        if !is_8ch(state) {
            return;
        }
        // The Console puts Direct 1:1 and Clear in the ROUTING band, and only
        // in 8-channel mode: an 8-channel stream is silent until routes exist.
        let actions = "Direct 1:1   Clear";
        let w = actions.chars().count() as u16;
        if grid_w > LABEL_W + w + 2 {
            p.buf
                .set_string(area.x + grid_w - w - 1, y, actions, theme.value());
        }
    }
}

impl Screen for MatrixPanel {
    fn title(&self) -> String {
        "Matrix Mixer".into()
    }

    fn draw(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        state: &DeviceState,
        focused: bool,
    ) {
        if area.height < 4 || area.width < LABEL_W + MIN_COL_W {
            return;
        }
        let n_out = self.outputs(state);
        self.col_w = col_width(area.width, n_out);
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
        let grid_w = LABEL_W + cols.len() as u16 * self.col_w;

        let here = self.row_at(state);
        let lines = self.lines(state);
        let body_h = area.height.saturating_sub(2) as usize;
        let cursor_line = lines
            .iter()
            .position(|l| *l == Line::Row(here))
            .unwrap_or(0);
        let first = if lines.len() <= body_h {
            0
        } else {
            cursor_line
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
                Line::Routing => self.draw_routing(&mut p, y, grid_w),
                Line::Divider => {
                    let w = (grid_w - LABEL_W).min(area.width.saturating_sub(LABEL_W)) as usize;
                    let bar = if theme.glyphs == Glyphs::Ascii {
                        "-".repeat(w)
                    } else {
                        "─".repeat(w)
                    };
                    p.buf
                        .set_string(area.x + LABEL_W, y, bar, theme.chrome_style());
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
                        let x = area.x + LABEL_W + i as u16 * self.col_w;
                        let cell = on_row && !self.trim && o == self.col;
                        match row {
                            Row::Input(input) => {
                                self.draw_crosspoint(&mut p, x, y, *input, o, cell)
                            }
                            other => self.draw_output_cell(&mut p, x, y, *other, o, cell),
                        }
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
                self.trim &= self.trim_available(state);
                ScreenEvent::Handled
            }
            KeyCode::Down => {
                self.row = (self.row + 1).min(rows - 1);
                self.trim &= self.trim_available(state);
                ScreenEvent::Handled
            }
            KeyCode::Left => {
                if self.trim {
                    // Already at the pinned column.
                } else if self.col > 0 {
                    self.col -= 1;
                } else if self.trim_available(state) {
                    self.trim = true;
                }
                ScreenEvent::Handled
            }
            KeyCode::Right => {
                if self.trim {
                    self.trim = false;
                } else {
                    self.col = (self.col + 1).min(outputs.saturating_sub(1));
                }
                ScreenEvent::Handled
            }
            KeyCode::Char(' ') => self.activate(state, false),
            KeyCode::Enter => self.activate(state, true),
            KeyCode::Char('i') => match self.row_at(state) {
                Row::Input(i) if !self.trim => {
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
        if self.trim {
            return self.arm(state);
        }
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
            for want in ["ROUTING", "OUT1", "FL", "ENABLE", "GAIN", "DELAY", "MUTE"] {
                assert!(f.contains(want), "{w}x{h} has no {want}:\n{f}");
            }
            // The Console's two routing actions, which only 8-channel mode has.
            assert!(
                f.contains("Direct 1:1") && f.contains("Clear"),
                "{w}x{h}: {f}"
            );
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

        let normal = text(&draw(&mut panel(), &state, 94, 20));
        assert!(!normal.contains("OUT9"), "the last columns wait:\n{normal}");
        assert!(
            normal.contains('\u{203a}'),
            "and the frame says so:\n{normal}"
        );
    }

    #[test]
    fn the_label_column_stays_pinned_while_the_grid_scrolls() {
        let mut p = panel();
        let state = fixture::state();
        for _ in 0..8 {
            p.handle(key(KeyCode::Right), &state);
        }
        let f = text(&draw(&mut p, &state, 94, 20));
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
        let fl = f.lines().nth(3).expect("the FL row");
        assert!(fl.contains('●') && fl.contains("0.0"), "{fl}");
        assert!(fl.contains('○') && fl.contains('-'), "{fl}");
        // The trim only exists in 8-channel mode; the stereo matrix has none.
        assert!(fl.contains("+0.0 dB"), "the input trim: {fl}");
        let stereo_frame = text(&draw(&mut panel(), &stereo(), 120, 20));
        assert!(!stereo_frame.contains("+0.0 dB"), "{stereo_frame}");
    }

    /// D52: `DESIGN.md` 7.7 says "Column headers in the output colour". Only
    /// the descriptor row had it; the name row above was drawn as a caption.
    #[test]
    fn both_header_rows_are_in_the_columns_output_colour() {
        let t = theme();
        let f = draw(&mut panel(), &fixture::state(), 200, 20);
        let col_w = col_width(200, 9);
        for (col, channel) in [(0usize, 8u8), (2, 10), (8, 16)] {
            let x = LABEL_W + col as u16 * col_w + 2;
            let want = t.channel_of(channel, 8, 9);
            assert_eq!(f[(x, 0)].fg, want, "the name row of column {col}");
            assert_eq!(f[(x, 1)].fg, want, "the descriptor row of column {col}");
        }
    }

    #[test]
    fn a_disabled_outputs_cells_draw_dim() {
        let t = theme();
        // The fixture connects input 2 to output 2, so the cell has a colour to
        // lose. Row order: headers, ROUTING, FL, FR, FC.
        let col_w = col_width(120, 9);
        let (x, y) = (LABEL_W + 2 * col_w, 5u16);
        let live = draw(&mut panel(), &fixture::state(), 120, 20);
        assert_eq!(live[(x, y)].symbol(), "●");
        assert_eq!(
            live[(x, y)].fg,
            t.channel_of(10, 8, 9),
            "the output's colour"
        );

        let off = draw(&mut panel(), &outputs_off(&[2]), 120, 20);
        assert_eq!(off[(x, y)].symbol(), "●", "still connected");
        assert_eq!(off[(x, y)].fg, t.dim, "but the whole column is dim");
        // And so is the output's own strip beneath it.
        let gain: Vec<_> = (0..col_w - 1).map(|dx| off[(x + dx, 13)].fg).collect();
        assert!(gain.iter().all(|c| *c == t.dim), "the GAIN row: {gain:?}");
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
        let f = text(&draw(&mut panel(), &fixture::state(), 120, 24));
        let dividers = f.lines().filter(|l| l.contains("──")).count();
        assert_eq!(dividers, 1, "one rule above ENABLE:\n{f}");
        let lines: Vec<&str> = f.lines().collect();
        assert!(lines[3].starts_with("▸FL") && lines[4].starts_with(" FR"));
        assert!(lines[5].starts_with(" FC"), "no pair divider:\n{f}");
        let s = text(&draw(&mut panel(), &stereo(), 120, 24));
        let dividers = s.lines().filter(|l| l.contains("──")).count();
        assert_eq!(dividers, 1, "the same rule in stereo:\n{s}");
    }

    #[test]
    fn columns_widen_with_the_pane() {
        assert_eq!(col_width(90, 9), MIN_COL_W, "120x40's pane: the minimum");
        assert_eq!(
            col_width(166, 9),
            16,
            "200x60's pane: sixteen, all nine fit"
        );
        assert_eq!(col_width(400, 9), MAX_COL_W, "capped");
        assert_eq!(col_width(90, 5), 14, "five outputs get wider columns");
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
    fn the_input_trim_lives_in_the_pinned_column() {
        let state = fixture::state();
        let mut p = panel();
        p.handle(key(KeyCode::Left), &state);
        assert!(p.trim, "left of the first column is the trim");
        p.handle(key(KeyCode::Enter), &state);
        assert_eq!(
            p.handle(key(KeyCode::Right), &state),
            ScreenEvent::Command("pre 0 0.5".into())
        );
        // The output rows have no trim, so the reticle leaves it behind.
        p.handle(key(KeyCode::Esc), &state);
        for _ in 0..8 {
            p.handle(key(KeyCode::Down), &state);
        }
        assert!(!p.trim);
        // And the stereo matrix never has one.
        let mut p = panel();
        p.handle(key(KeyCode::Left), &stereo());
        assert!(!p.trim);
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
                for (row, col, trim) in [(0usize, 0usize, false), (0, 0, true), (8, 8, false)] {
                    let mut p = panel();
                    p.row = row;
                    p.col = col;
                    p.trim = trim;
                    let before = (p.row, p.col, p.trim, p.edit.clone());
                    let ev = p.handle(k, &state);
                    let after = (p.row, p.col, p.trim, p.edit.clone());
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
