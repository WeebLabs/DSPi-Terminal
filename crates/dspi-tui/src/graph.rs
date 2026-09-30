//! The response graph, following the Console's `GraphView.swift`.
//!
//! Log frequency across a settable window, linear dB about a settable centre,
//! an adaptive dB grid, and the phase of the principal curve on a right-hand
//! axis that scales with the vertical zoom. It draws the curves it is given,
//! in order, each in its own colour: the shell hands it one channel and, for
//! a linked pair, the partner underneath in grey (DESIGN 12); the tool panels
//! hand it their two or three series.
//!
//! The graph is render-only: it takes curves that the screen has already
//! computed with `dspi_proto::dsp`, and it answers questions (value at a
//! frequency) without owning any state.

use dspi_proto::dsp;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;

use crate::theme::{ColorDepth, Glyphs, Theme};

/// How strongly the grid is drawn: the Console's Grid Opacity slider
/// (0..200 %, `DSPi_ConsoleApp.swift:1864-1869`) in the three steps a
/// terminal cell can show. Off is 0 %, Dim is anything below 100 % (the
/// Console's 50 % default), and Normal is 100 % and up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GridStrength {
    Off,
    #[default]
    Dim,
    Normal,
}

impl GridStrength {
    pub const CHOICES: [&'static str; 3] = ["Off", "Dim", "Normal"];

    pub fn index(self) -> usize {
        match self {
            Self::Off => 0,
            Self::Dim => 1,
            Self::Normal => 2,
        }
    }

    pub fn from_index(i: usize) -> Self {
        match i {
            0 => Self::Off,
            2 => Self::Normal,
            _ => Self::Dim,
        }
    }
}

/// Settings > Graphing, with the Console's defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphSettings {
    pub min_hz: f64,
    pub max_hz: f64,
    pub db_center: f64,
    pub db_range: f64,
    pub show_phase: bool,
    pub unwrap_phase: bool,
    pub freq_grid: bool,
    pub freq_labels: bool,
    pub db_grid: bool,
    pub db_labels: bool,
    /// The cursor readout's frequency and its per-curve dB: the Console's
    /// Show Frequency Readout and Show Gain Readout.
    pub freq_readout: bool,
    pub gain_readout: bool,
    pub grid: GridStrength,
    /// The overview's cells per row: 0 is Auto, otherwise at most this many
    /// (the Console's Dashboard Layout).
    pub dashboard_cards: u8,
}

impl Default for GraphSettings {
    fn default() -> Self {
        Self {
            min_hz: 15.0,
            max_hz: 20_000.0,
            db_center: 0.0,
            db_range: 50.0,
            show_phase: false,
            unwrap_phase: false,
            freq_grid: true,
            freq_labels: true,
            db_grid: true,
            db_labels: true,
            freq_readout: true,
            gain_readout: true,
            grid: GridStrength::Dim,
            dashboard_cards: 0,
        }
    }
}

impl GraphSettings {
    pub const MIN_FREQS: [f64; 5] = [10.0, 15.0, 20.0, 50.0, 100.0];
    pub const MAX_FREQS: [f64; 3] = [5_000.0, 10_000.0, 20_000.0];

    pub fn db_top(&self) -> f64 {
        self.db_center + self.db_range / 2.0
    }
    pub fn db_bottom(&self) -> f64 {
        self.db_center - self.db_range / 2.0
    }

    /// The Console's scroll-wheel zoom: range clamped 10..100.
    pub fn zoom(&mut self, delta: f64) {
        self.db_range = (self.db_range + delta).clamp(10.0, 100.0);
    }

    /// The dB between grid lines and labels, by the Console's rule.
    pub fn db_step(&self) -> f64 {
        let span = self.db_range;
        if span <= 12.0 {
            1.0
        } else if span <= 30.0 {
            3.0
        } else if span <= 60.0 {
            5.0
        } else {
            10.0
        }
    }

    /// The phase axis half-span in degrees: 180 at the default range.
    pub fn phase_span(&self) -> f64 {
        180.0 * self.db_range / 50.0
    }

    fn x_frac(&self, hz: f64) -> f64 {
        let (lo, hi) = (self.min_hz.log10(), self.max_hz.log10());
        (hz.log10() - lo) / (hi - lo)
    }

    fn hz_at(&self, frac: f64) -> f64 {
        let (lo, hi) = (self.min_hz.log10(), self.max_hz.log10());
        10f64.powf(lo + frac * (hi - lo))
    }

    /// Move a cursor frequency by one fortieth of the window.
    pub fn step_cursor(&self, hz: f64, direction: i32) -> f64 {
        let f = (self.x_frac(hz) + direction as f64 / 40.0).clamp(0.0, 1.0);
        self.hz_at(f)
    }
}

/// One channel's response, computed by the screen.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphCurve {
    pub descriptor: String,
    pub color: Color,
    /// dB at `dsp::frequencies()`, with any output gain already folded in as
    /// a constant offset, which is what lets two outputs at different trims
    /// stay separate when grouping.
    pub magnitude: Vec<f64>,
    /// Degrees at the same frequencies, wrapped to +/-180.
    pub phase: Option<Vec<f64>>,
    /// The principal curve: bold in mono, and the one whose phase is drawn.
    pub selected: bool,
}

/// Interpolate a 201-point log-spaced curve at any frequency.
pub fn value_at(points: &[f64], hz: f64) -> f64 {
    if points.is_empty() {
        return 0.0;
    }
    let (lo, hi) = (dsp::MIN_HZ.log10(), dsp::MAX_HZ.log10());
    let pos = ((hz.log10() - lo) / (hi - lo) * (points.len() - 1) as f64)
        .clamp(0.0, (points.len() - 1) as f64);
    let i = pos.floor() as usize;
    let frac = pos - i as f64;
    let a = points[i];
    let b = points[(i + 1).min(points.len() - 1)];
    a + (b - a) * frac
}

pub struct Graph<'a> {
    pub curves: &'a [GraphCurve],
    pub settings: &'a GraphSettings,
    pub theme: &'a Theme,
    /// A readout cursor, in Hz.
    pub cursor: Option<f64>,
    /// The focused band's centre, in Hz.
    pub marker_hz: Option<f64>,
    /// Draw the plot with the accent border colour.
    pub focused: bool,
}

impl<'a> Graph<'a> {
    pub fn new(curves: &'a [GraphCurve], settings: &'a GraphSettings, theme: &'a Theme) -> Self {
        Self {
            curves,
            settings,
            theme,
            cursor: None,
            marker_hz: None,
            focused: false,
        }
    }
    pub fn cursor(mut self, hz: Option<f64>) -> Self {
        self.cursor = hz;
        self
    }
    pub fn marker(mut self, hz: Option<f64>) -> Self {
        self.marker_hz = hz;
        self
    }

    /// The readout for the cursor: every curve's dB at that frequency.
    pub fn readout(&self, hz: f64) -> Vec<(String, f64)> {
        self.curves
            .iter()
            .map(|c| (c.descriptor.clone(), value_at(&c.magnitude, hz)))
            .collect()
    }

    /// The cursor readout as the status line shows it, `1000 Hz  IN1 +6.0`,
    /// with the frequency and the levels each left out when Graphing turns
    /// them off. None when both are off.
    pub fn readout_text(&self, hz: f64) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        if self.settings.freq_readout {
            parts.push(format!("{hz:.0} Hz"));
        }
        if self.settings.gain_readout {
            parts.extend(
                self.readout(hz)
                    .iter()
                    .map(|(d, db)| format!("{d} {db:+.1}")),
            );
        }
        (!parts.is_empty()).then(|| parts.join("  "))
    }

    fn db_label(db: f64) -> String {
        if db == 0.0 {
            "0".into()
        } else {
            format!("{db:+.0}")
        }
    }

    /// Gutter width for the dB labels: the widest label plus a space.
    fn gutter(&self) -> u16 {
        if !self.settings.db_labels {
            return 0;
        }
        let s = self.settings;
        let w = [s.db_top(), s.db_bottom()]
            .iter()
            .map(|d| Self::db_label(*d).len())
            .max()
            .unwrap_or(3);
        w as u16 + 1
    }

    fn phase_gutter(&self) -> u16 {
        if self.settings.show_phase { 5 } else { 0 }
    }

    /// The plot rectangle inside `area`, after gutters and the label row.
    pub fn plot_area(&self, area: Rect) -> Rect {
        let g = self.gutter();
        let pg = self.phase_gutter();
        let bottom = u16::from(self.settings.freq_labels);
        Rect::new(
            area.x + g,
            area.y,
            area.width.saturating_sub(g + pg),
            area.height.saturating_sub(bottom),
        )
    }
}

const BRAILLE_DOTS: [[u16; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];
const BRAILLE_BASE: u16 = 0x2800;

impl Widget for Graph<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let plot = self.plot_area(area);
        if plot.width < 4 || plot.height < 2 {
            return;
        }
        let t = self.theme;
        let s = self.settings;
        let (top, bottom) = (s.db_top(), s.db_bottom());
        let span = (top - bottom).max(1.0);
        let rows = plot.height as f64;
        let to_row = |db: f64, rows: f64| -> f64 {
            (1.0 - ((db - bottom) / span).clamp(0.0, 1.0)) * (rows - 1.0)
        };
        let x_of = |hz: f64| -> Option<u16> {
            let f = s.x_frac(hz);
            if !(0.0..=1.0).contains(&f) {
                return None;
            }
            Some(plot.x + ((f * (plot.width - 1) as f64).round() as u16).min(plot.width - 1))
        };

        // Grid first, so the curves draw over it. Dim is the quiet look
        // (DESIGN 12); Normal lifts each line one step.
        let (grid_major, grid_minor) = match s.grid {
            GridStrength::Normal => (t.dim, t.chrome),
            _ => (t.chrome, t.chrome_faint),
        };
        let grid_on = s.grid != GridStrength::Off;
        if s.db_grid && grid_on {
            let step = s.db_step();
            // A grid line on every row is no grid at all; below two rows per
            // step only the zero line is drawn.
            let sparse = rows / (span / step) < 1.5;
            let mut db = (bottom / step).ceil() * step;
            while db <= top + 1e-9 {
                if sparse && db != 0.0 {
                    db += step;
                    continue;
                }
                let y = plot.y + (to_row(db, rows).round() as u16).min(plot.height - 1);
                let (sym, style) = if db == 0.0 {
                    (
                        if t.glyphs == Glyphs::Ascii {
                            "-"
                        } else {
                            "─"
                        },
                        Style::default().fg(grid_major),
                    )
                } else {
                    (
                        if t.glyphs == Glyphs::Ascii {
                            "."
                        } else {
                            "┈"
                        },
                        Style::default().fg(grid_minor),
                    )
                };
                for x in plot.x..plot.x + plot.width {
                    buf[(x, y)].set_symbol(sym).set_style(style);
                }
                db += step;
            }
        }
        if s.freq_grid && grid_on {
            let major = [100.0, 1_000.0, 10_000.0];
            // The Console draws every minor decade line at 6 % white, which
            // is nearly invisible. A terminal cell is not, so only the 2 and
            // 5 lines are drawn, and only when the plot is wide enough that
            // they do not crowd the majors.
            let minor: Vec<f64> = [2.0, 5.0]
                .iter()
                .flat_map(|m| [10.0, 100.0, 1_000.0].map(|d| m * d))
                .chain(std::iter::once(20_000.0))
                .collect();
            let wide = plot.width >= 100;
            for hz in minor.iter().filter(|_| wide).chain(major.iter()) {
                let Some(x) = x_of(*hz) else { continue };
                let is_major = major.contains(hz);
                let style = Style::default().fg(if is_major { grid_major } else { grid_minor });
                let sym = match (is_major, t.glyphs) {
                    (_, Glyphs::Ascii) => ":",
                    (true, _) => "┆",
                    (false, _) => "╎",
                };
                for y in plot.y..plot.y + plot.height {
                    let cell = &mut buf[(x, y)];
                    if cell.symbol() == " " || cell.symbol() == "┈" || cell.symbol() == "." {
                        cell.set_symbol(sym).set_style(style);
                    }
                }
            }
        }

        // Band marker and cursor.
        if let Some(hz) = self.marker_hz
            && let Some(x) = x_of(hz)
        {
            for y in plot.y..plot.y + plot.height {
                buf[(x, y)]
                    .set_symbol("┆")
                    .set_style(Style::default().fg(t.dim));
            }
        }
        if let Some(hz) = self.cursor
            && let Some(x) = x_of(hz)
        {
            for y in plot.y..plot.y + plot.height {
                buf[(x, y)]
                    .set_symbol("┊")
                    .set_style(Style::default().fg(t.warning));
            }
        }

        // Curves in the order given, so the last one lands on top.
        for c in self.curves {
            let sample = |frac: f64| -> f64 { value_at(&c.magnitude, s.hz_at(frac)) };
            let style = if t.depth == ColorDepth::Mono {
                Style::default().add_modifier(if c.selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                })
            } else {
                Style::default().fg(c.color)
            };
            draw_curve(plot, buf, t, &sample, &to_row, false, style);
        }

        // Phase of the principal (or first) curve, dotted, on its own axis;
        // drawn last so it is never hidden.
        if s.show_phase {
            let half = s.phase_span();
            let target = self
                .curves
                .iter()
                .find(|c| c.selected)
                .or_else(|| self.curves.first());
            if let Some(c) = target
                && let Some(ph) = &c.phase
            {
                let phase_points: Vec<f64> = if s.unwrap_phase {
                    dsp::unwrap_phase(ph)
                } else {
                    ph.clone()
                };
                let sample = |frac: f64| -> f64 { value_at(&phase_points, s.hz_at(frac)) };
                let to_row_phase = |deg: f64, rows: f64| -> f64 {
                    (1.0 - ((deg + half) / (2.0 * half)).clamp(0.0, 1.0)) * (rows - 1.0)
                };
                draw_curve(
                    plot,
                    buf,
                    t,
                    &sample,
                    &to_row_phase,
                    true,
                    Style::default().fg(t.fg),
                );
            }
            let gx = plot.x + plot.width;
            let labels = [
                (0u16, format!("{:+.0}", half)),
                (plot.height - 1, format!("{:+.0}", -half)),
            ];
            for (y, l) in labels {
                buf.set_string(gx + 1, plot.y + y, format!("{l:>4}"), t.label());
            }
            let mid = plot.y + (to_row(s.db_center, rows).round() as u16).min(plot.height - 1);
            buf.set_string(gx + 1, mid, "   0", t.label());
        }

        // dB labels in the left gutter.
        if s.db_labels {
            let g = self.gutter() as usize;
            // A short plot cannot label every step; double the step until
            // labels are at least two rows apart.
            let mut step = s.db_step();
            while rows / (span / step) < 2.0 && step < span {
                step *= 2.0;
            }
            let mut taken = vec![false; plot.height as usize];
            let mut db = (bottom / step).ceil() * step;
            while db <= top + 1e-9 {
                let row = to_row(db, rows).round() as usize;
                if row < taken.len() && !taken[row] {
                    taken[row] = true;
                    let text = format!("{:>w$} ", Self::db_label(db), w = g - 1);
                    let style = if db == 0.0 {
                        Style::default().fg(t.chrome)
                    } else {
                        t.label()
                    };
                    buf.set_string(area.x, plot.y + row as u16, &text, style);
                }
                db += step;
            }
        }

        // Frequency labels along the bottom.
        if s.freq_labels && area.height > plot.height {
            let y = plot.y + plot.height;
            let marks: [(f64, &str); 10] = [
                (20.0, "20"),
                (50.0, "50"),
                (100.0, "100"),
                (200.0, "200"),
                (500.0, "500"),
                (1000.0, "1k"),
                (2000.0, "2k"),
                (5000.0, "5k"),
                (10_000.0, "10k"),
                (20_000.0, "20k"),
            ];
            let mut last_end: i32 = -1;
            for (hz, label) in marks {
                let Some(x) = x_of(hz) else { continue };
                let start = (x as i32 - label.len() as i32 / 2).max(plot.x as i32);
                let start = start.min((plot.x + plot.width) as i32 - label.len() as i32);
                if start <= last_end {
                    continue;
                }
                buf.set_string(start as u16, y, label, t.label());
                last_end = start + label.len() as i32;
            }
        }
    }
}

/// Rasterise one curve into braille (or the coarse fallback). `sample` maps
/// a fraction of the width to a value and `to_row` a value to a row of dots
/// (or cells), so a panel graph with its own axes can reuse it.
pub(crate) fn draw_curve(
    plot: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    sample: &dyn Fn(f64) -> f64,
    to_row: &dyn Fn(f64, f64) -> f64,
    dotted: bool,
    style: Style,
) {
    match theme.glyphs {
        Glyphs::Braille => {
            let (dots_w, dots_h) = (plot.width as usize * 2, plot.height as usize * 4);
            let mut cells = vec![0u16; plot.width as usize * plot.height as usize];
            let mut prev: Option<(usize, usize)> = None;
            for dx in 0..dots_w {
                let frac = dx as f64 / (dots_w - 1).max(1) as f64;
                let v = sample(frac);
                let dy = to_row(v, dots_h as f64).round() as isize;
                if dy < 0 || dy >= dots_h as isize || !v.is_finite() {
                    prev = None;
                    continue;
                }
                let dy = dy as usize;
                // The phase overlay is dashed 6 on, 4 off so it reads apart
                // from the magnitude curve.
                if dotted && dx % 10 >= 6 {
                    prev = None;
                    continue;
                }
                if let Some((px, py)) = prev
                    && px + 1 == dx
                {
                    let (lo, hi) = if py < dy { (py, dy) } else { (dy, py) };
                    for y in lo..=hi {
                        set_dot(&mut cells, plot, dx, y);
                    }
                } else {
                    set_dot(&mut cells, plot, dx, dy);
                }
                prev = Some((dx, dy));
            }
            for (i, bits) in cells.iter().enumerate() {
                if *bits == 0 {
                    continue;
                }
                let x = plot.x + (i % plot.width as usize) as u16;
                let y = plot.y + (i / plot.width as usize) as u16;
                let ch = char::from_u32((BRAILLE_BASE | bits) as u32).unwrap_or('?');
                buf[(x, y)].set_char(ch).set_style(style);
            }
        }
        _ => {
            let blocks = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
            for x in 0..plot.width {
                if dotted && x % 5 >= 3 {
                    continue;
                }
                let frac = x as f64 / (plot.width - 1).max(1) as f64;
                let v = sample(frac);
                if !v.is_finite() {
                    continue;
                }
                let row = to_row(v, plot.height as f64);
                let cell_y = row.floor().clamp(0.0, plot.height as f64 - 1.0) as u16;
                let fr = 1.0 - (row - row.floor());
                let ch = if theme.glyphs == Glyphs::Ascii {
                    '*'
                } else {
                    blocks[((fr * 7.0).round() as usize).min(7)]
                };
                buf[(plot.x + x, plot.y + cell_y)]
                    .set_char(ch)
                    .set_style(style);
            }
        }
    }
}

fn set_dot(cells: &mut [u16], plot: Rect, dx: usize, dy: usize) {
    let (cx, cy) = (dx / 2, dy / 4);
    if cx >= plot.width as usize || cy >= plot.height as usize {
        return;
    }
    cells[cy * plot.width as usize + cx] |= BRAILLE_DOTS[dy % 4][dx % 2];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::testing::{render, render_buf};
    use dspi_proto::FilterType;

    fn flat(name: &str, color: Color) -> GraphCurve {
        GraphCurve {
            descriptor: name.into(),
            color,
            magnitude: vec![0.0; dsp::POINTS],
            phase: Some(vec![0.0; dsp::POINTS]),
            selected: false,
        }
    }

    fn tuned(name: &str, color: Color, gain: f64) -> GraphCurve {
        let bands = [dsp::Band {
            filter_type: FilterType::Peaking,
            freq: 1000.0,
            q: 1.0,
            gain_db: gain as f32,
            bypass: false,
        }];
        GraphCurve {
            descriptor: name.into(),
            color,
            magnitude: dsp::curve(&bands, 0.0),
            phase: Some(dsp::phase_curve(&bands)),
            selected: false,
        }
    }

    #[test]
    fn a_flat_curve_sits_on_the_zero_line_with_labels_around_it() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let curves = vec![flat("IN1", t.inputs[0])];
        let s = GraphSettings::default();
        let out = render(Graph::new(&curves, &s, &t), 60, 12);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 12);
        // Eleven plot rows cannot label every 5 dB; the labels thin to 10.
        assert!(out.contains("+20 ") && !out.contains("+25 "), "{out}");
        assert!(
            lines[11].contains("100") && lines[11].contains("1k") && lines[11].contains("10k"),
            "{:?}",
            lines[11]
        );
        // Zero is the middle row, drawn as a rule, and the flat curve's dots
        // sit on it.
        let zero = lines.iter().position(|l| l.starts_with("  0 ")).unwrap();
        assert_eq!(zero, 5);
        assert!(
            lines[zero]
                .chars()
                .any(|c| ('\u{2800}'..='\u{28FF}').contains(&c)),
            "{:?}",
            lines[zero]
        );
    }

    #[test]
    fn the_db_step_follows_the_consoles_rule() {
        let mut s = GraphSettings::default();
        assert_eq!(s.db_step(), 5.0);
        s.db_range = 12.0;
        assert_eq!(s.db_step(), 1.0);
        s.db_range = 30.0;
        assert_eq!(s.db_step(), 3.0);
        s.db_range = 100.0;
        assert_eq!(s.db_step(), 10.0);
        s.zoom(50.0);
        assert_eq!(s.db_range, 100.0, "clamped");
        assert_eq!(s.phase_span(), 360.0);
    }

    #[test]
    fn values_interpolate_between_the_curve_points() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let c = tuned("IN1", t.inputs[0], 6.0);
        let at_peak = value_at(&c.magnitude, 1000.0);
        assert!((at_peak - 6.0).abs() < 0.2, "{at_peak}");
        assert!(value_at(&c.magnitude, 20.0).abs() < 0.2);
        let s = GraphSettings::default();
        let g = Graph::new(std::slice::from_ref(&c), &s, &t);
        assert_eq!(g.readout(1000.0).len(), 1);
        assert!((s.step_cursor(1000.0, 1) - 1000.0) > 0.0);
    }

    #[test]
    fn the_last_curve_draws_over_the_one_before_it() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let mut curves = vec![tuned("IN2", t.dim, 6.0), tuned("IN1", t.inputs[0], 6.0)];
        curves[1].selected = true;
        let s = GraphSettings::default();
        let buf = render_buf(Graph::new(&curves, &s, &t), 60, 12);
        let mut under = 0;
        let mut over = 0;
        for y in 0..11 {
            for x in 4..60 {
                let cell = &buf[(x, y)];
                if cell.fg == t.dim {
                    under += 1;
                } else if cell.fg == t.inputs[0] {
                    over += 1;
                }
            }
        }
        assert!(over > 0 && under == 0, "{under} under, {over} over");
        assert_eq!(
            Graph::new(&curves, &s, &t).readout(1000.0).len(),
            2,
            "the readout names both"
        );
    }

    #[test]
    fn the_phase_axis_appears_on_the_right_when_enabled() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let curves = vec![tuned("IN1", t.inputs[0], 6.0)];
        let s = GraphSettings {
            show_phase: true,
            ..Default::default()
        };
        let out = render(Graph::new(&curves, &s, &t), 60, 12);
        assert!(out.lines().next().unwrap().ends_with("+180"), "{out}");
        assert!(out.lines().nth(10).unwrap().ends_with("-180"), "{out}");
    }

    #[test]
    fn the_readout_leaves_out_what_graphing_turns_off() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let curves = vec![tuned("IN1", t.inputs[0], 6.0)];
        let mut s = GraphSettings::default();
        let text = |s: &GraphSettings| Graph::new(&curves, s, &t).readout_text(1000.0);
        assert_eq!(text(&s).as_deref(), Some("1000 Hz  IN1 +6.0"));
        s.gain_readout = false;
        assert_eq!(text(&s).as_deref(), Some("1000 Hz"));
        s.freq_readout = false;
        assert_eq!(text(&s), None);
        s.gain_readout = true;
        assert_eq!(text(&s).as_deref(), Some("IN1 +6.0"));
    }

    #[test]
    fn the_grid_strength_picks_the_grid_colours() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let curves: Vec<GraphCurve> = Vec::new();
        let colours = |grid: GridStrength| {
            let s = GraphSettings {
                grid,
                ..Default::default()
            };
            // Wide and tall enough for the minor lines both ways.
            let buf = render_buf(Graph::new(&curves, &s, &t), 120, 40);
            let mut seen = Vec::new();
            for y in 0..39 {
                for x in 4..120 {
                    let c = &buf[(x, y)];
                    if c.symbol() != " " && !seen.contains(&c.fg) {
                        seen.push(c.fg);
                    }
                }
            }
            seen
        };
        assert!(colours(GridStrength::Off).is_empty(), "no grid at all");
        let dim = colours(GridStrength::Dim);
        assert!(
            dim.contains(&t.chrome) && dim.contains(&t.chrome_faint),
            "{dim:?}"
        );
        let normal = colours(GridStrength::Normal);
        assert!(
            normal.contains(&t.dim) && normal.contains(&t.chrome),
            "{normal:?}"
        );
        assert!(!normal.contains(&t.chrome_faint), "{normal:?}");
    }

    #[test]
    fn a_narrow_window_shows_only_its_labels() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let curves = vec![flat("IN1", t.inputs[0])];
        let s = GraphSettings {
            min_hz: 100.0,
            max_hz: 5000.0,
            ..Default::default()
        };
        let out = render(Graph::new(&curves, &s, &t), 60, 8);
        let bottom = out.lines().last().unwrap();
        assert!(
            bottom.contains("100") && bottom.contains("5k") && !bottom.contains("20k"),
            "{bottom}"
        );
    }
}
