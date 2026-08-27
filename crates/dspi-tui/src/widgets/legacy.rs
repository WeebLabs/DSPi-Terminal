//! The two widgets that carry the application: the response graph and the meter.

use dspi_proto::dsp;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::theme::{Glyphs, Theme};

/// One channel's curve, ready to draw.
pub struct Curve<'a> {
    pub label: &'a str,
    pub color: Color,
    /// Magnitudes in dB, one per `dsp::POINTS`.
    pub points: &'a [f64],
    /// A dimmed curve reads as context rather than as the thing being edited.
    pub focused: bool,
}

/// A Bode plot, drawn with braille where the terminal allows it.
///
/// Braille gives each cell a 2x4 dot matrix, so a 60x12 area becomes a 120x48
/// plot grid. That is the difference between a curve you can read and a
/// staircase.
pub struct Bode<'a> {
    pub curves: Vec<Curve<'a>>,
    pub theme: &'a Theme,
    pub db_top: f64,
    pub db_bottom: f64,
    /// A vertical marker, e.g. the focused band's centre frequency.
    pub marker_hz: Option<f64>,
    /// A readout cursor, as an index into the curve.
    pub cursor: Option<usize>,
}

impl<'a> Bode<'a> {
    pub fn new(theme: &'a Theme) -> Self {
        Self {
            curves: Vec::new(),
            theme,
            db_top: 15.0,
            db_bottom: -15.0,
            marker_hz: None,
            cursor: None,
        }
    }

    pub fn curve(mut self, c: Curve<'a>) -> Self {
        self.curves.push(c);
        self
    }

    /// Cell column for a frequency, given the plot width.
    fn x_of(hz: f64, width: u16) -> f64 {
        let (lo, hi) = (dsp::MIN_HZ.log10(), dsp::MAX_HZ.log10());
        (hz.log10() - lo) / (hi - lo) * (width as f64 - 1.0)
    }
}

/// Braille cells encode their eight dots in a fixed, non-obvious bit order.
const BRAILLE_DOTS: [[u16; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];
const BRAILLE_BASE: u16 = 0x2800;

impl Widget for Bode<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 4 || area.height < 3 {
            return;
        }

        let span = (self.db_top - self.db_bottom).max(1.0);
        let to_row = |db: f64, rows: f64| -> f64 {
            let norm = (db - self.db_bottom) / span;
            (1.0 - norm.clamp(0.0, 1.0)) * (rows - 1.0)
        };

        // The zero line first, so curves draw over it.
        if self.db_bottom <= 0.0 && self.db_top >= 0.0 {
            let y = area.y + (to_row(0.0, area.height as f64).round() as u16).min(area.height - 1);
            for x in area.x..area.x + area.width {
                buf[(x, y)]
                    .set_symbol("─")
                    .set_style(Style::default().fg(self.theme.chrome));
            }
        }

        if let Some(hz) = self.marker_hz {
            let col = Bode::x_of(hz, area.width).round();
            if col >= 0.0 && col < area.width as f64 {
                let x = area.x + col as u16;
                for y in area.y..area.y + area.height {
                    buf[(x, y)]
                        .set_symbol("┆")
                        .set_style(Style::default().fg(self.theme.chrome));
                }
            }
        }

        // The cursor draws before the curves so a curve is never hidden by it.
        if let Some(i) = self.cursor {
            let t = i as f64 / (dsp::POINTS - 1) as f64;
            let x = area.x + ((t * (area.width - 1) as f64).round() as u16).min(area.width - 1);
            for y in area.y..area.y + area.height {
                buf[(x, y)]
                    .set_symbol("┊")
                    .set_style(Style::default().fg(self.theme.pending));
            }
        }

        match self.theme.glyphs {
            Glyphs::Braille => self.render_braille(area, buf, to_row),
            _ => self.render_coarse(area, buf, to_row),
        }
    }
}

impl Bode<'_> {
    fn render_braille(&self, area: Rect, buf: &mut Buffer, to_row: impl Fn(f64, f64) -> f64) {
        let (dots_w, dots_h) = (area.width as usize * 2, area.height as usize * 4);

        for curve in &self.curves {
            // One accumulator per cell; braille cannot carry per-dot colour, so
            // overlapping curves resolve by draw order with the focused one last.
            let mut cells = vec![0u16; area.width as usize * area.height as usize];

            let mut prev: Option<(usize, usize)> = None;
            for dx in 0..dots_w {
                let t = dx as f64 / (dots_w - 1).max(1) as f64;
                let idx = (t * (dsp::POINTS - 1) as f64).round() as usize;
                let Some(db) = curve.points.get(idx.min(dsp::POINTS - 1)) else {
                    continue;
                };
                let dy = to_row(*db, dots_h as f64).round() as isize;
                if dy < 0 || dy >= dots_h as isize {
                    prev = None;
                    continue;
                }
                let dy = dy as usize;

                // Join to the previous point so a steep slope stays continuous
                // rather than breaking into disconnected dots.
                if let Some((px, py)) = prev
                    && px + 1 == dx
                {
                    let (lo, hi) = if py < dy { (py, dy) } else { (dy, py) };
                    for y in lo..=hi {
                        set_dot(&mut cells, area, dx, y);
                    }
                } else {
                    set_dot(&mut cells, area, dx, dy);
                }
                prev = Some((dx, dy));
            }

            let style = if curve.focused {
                Style::default().fg(curve.color)
            } else {
                Style::default().fg(self.theme.dim)
            };

            for (i, bits) in cells.iter().enumerate() {
                if *bits == 0 {
                    continue;
                }
                let x = area.x + (i % area.width as usize) as u16;
                let y = area.y + (i / area.width as usize) as u16;
                let ch = char::from_u32((BRAILLE_BASE | bits) as u32).unwrap_or('?');
                buf[(x, y)].set_char(ch).set_style(style);
            }
        }
    }

    /// Block fallback: one glyph per cell, so vertical resolution drops to eight
    /// steps per row instead of four dots. Coarser, but it works everywhere.
    fn render_coarse(&self, area: Rect, buf: &mut Buffer, to_row: impl Fn(f64, f64) -> f64) {
        let blocks = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

        for curve in &self.curves {
            let style = if curve.focused {
                Style::default().fg(curve.color)
            } else {
                Style::default().fg(self.theme.dim)
            };

            for x in 0..area.width {
                let t = x as f64 / (area.width - 1).max(1) as f64;
                let idx = (t * (dsp::POINTS - 1) as f64).round() as usize;
                let Some(db) = curve.points.get(idx.min(dsp::POINTS - 1)) else {
                    continue;
                };
                let row = to_row(*db, area.height as f64);
                let cell_y = row.floor().clamp(0.0, area.height as f64 - 1.0) as u16;
                let frac = 1.0 - (row - row.floor());
                let ch = if self.theme.glyphs == Glyphs::Ascii {
                    '*'
                } else {
                    blocks[((frac * 7.0).round() as usize).min(7)]
                };
                buf[(area.x + x, area.y + cell_y)]
                    .set_char(ch)
                    .set_style(style);
            }
        }
    }
}

fn set_dot(cells: &mut [u16], area: Rect, dx: usize, dy: usize) {
    let (cx, cy) = (dx / 2, dy / 4);
    if cx >= area.width as usize || cy >= area.height as usize {
        return;
    }
    cells[cy * area.width as usize + cx] |= BRAILLE_DOTS[dy % 4][dx % 2];
}

/// A "nice" tick step: 1, 2 or 5 times a power of ten.
///
/// Ticks a reader can do arithmetic with. Dividing the span into a fixed number
/// of equal parts would give steps like 4.3 dB, which are correct and useless.
fn nice_step(rough: f64) -> f64 {
    if rough <= 0.0 {
        return 1.0;
    }
    let decade = 10f64.powf(rough.log10().floor());
    let scaled = rough / decade;
    let step = if scaled <= 1.0 {
        1.0
    } else if scaled <= 2.0 {
        2.0
    } else if scaled <= 5.0 {
        5.0
    } else {
        10.0
    };
    step * decade
}

/// The dB between labels: the smallest nice step whose labels neither crowd
/// each other nor take over the plot.
///
/// Two constraints, because either alone is wrong. A minimum row spacing keeps
/// labels off adjacent lines on a short plot; a maximum count stops a tall plot
/// from filling its gutter with numbers. Shared by [`db_axis`] and
/// [`db_axis_width`] so the gutter is always as wide as what goes in it.
fn tick_step(span: f64, furthest: f64, rows: f64) -> f64 {
    const MIN_ROWS_APART: f64 = 1.5;
    const MAX_LABELS: usize = 9;

    let mut step = nice_step(span / rows.max(1.0));
    // The ladder is 1-2-5 per decade, so this terminates well before the guard.
    for _ in 0..12 {
        let apart = step / span * (rows - 1.0).max(1.0);
        let labels = 2 * (furthest / step).floor() as usize + 1;
        if apart >= MIN_ROWS_APART && labels <= MAX_LABELS {
            break;
        }
        step = nice_step(step * 1.5);
    }
    step
}

/// dB labels for the y-axis, one line per plot row.
///
/// The zoom keys change the visible span, so a fixed scale would be wrong at
/// every setting but one. The step is chosen from the span, always includes
/// zero, and thins out rather than overprinting when the plot is short.
///
/// Returns exactly `height` lines, aligned to the same rows the plot uses.
pub fn db_axis(height: u16, db_top: f64, db_bottom: f64, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(""); height as usize];
    if height == 0 {
        return lines;
    }
    let span = (db_top - db_bottom).max(1.0);
    let rows = height as f64;
    let step = tick_step(span, db_top.abs().max(db_bottom.abs()), rows);

    let width = label_width(db_top, db_bottom, step);
    let mut taken: Vec<bool> = vec![false; height as usize];

    // From zero outwards, so the axis is symmetric and 0 is never the label
    // that gets dropped.
    let furthest = db_top.abs().max(db_bottom.abs());
    let mut ticks: Vec<f64> = vec![0.0];
    let mut v = step;
    while v <= furthest + 1e-9 {
        ticks.push(v);
        ticks.push(-v);
        v += step;
    }

    for db in ticks {
        if db > db_top + 1e-9 || db < db_bottom - 1e-9 {
            continue;
        }
        let norm = (db - db_bottom) / span;
        let row = ((1.0 - norm) * (rows - 1.0)).round() as usize;
        if row >= taken.len() || taken[row] {
            continue;
        }
        taken[row] = true;
        let text = format!("{:>width$} ", format_db(db), width = width);
        lines[row] = Line::from(Span::styled(
            text,
            Style::default().fg(if db == 0.0 { theme.chrome } else { theme.dim }),
        ));
    }
    lines
}

/// Width of the widest label the axis will draw, so the gutter is stable across
/// a redraw rather than shifting the plot sideways.
fn label_width(db_top: f64, db_bottom: f64, step: f64) -> usize {
    let furthest = db_top.abs().max(db_bottom.abs());
    let extreme = (furthest / step).floor() * step;
    format_db(extreme).len().max(format_db(-extreme).len())
}

fn format_db(db: f64) -> String {
    if db == 0.0 {
        "0".into()
    } else if (db.fract()).abs() < 1e-9 {
        format!("{db:+.0}")
    } else {
        format!("{db:+.1}")
    }
}

/// How wide a gutter [`db_axis`] needs for this span, including its trailing
/// space. Callers reserve this before laying the plot out.
pub fn db_axis_width(height: u16, db_top: f64, db_bottom: f64) -> u16 {
    let span = (db_top - db_bottom).max(1.0);
    let furthest = db_top.abs().max(db_bottom.abs());
    let step = tick_step(span, furthest, height as f64);
    label_width(db_top, db_bottom, step) as u16 + 1
}

/// Frequency labels for the x-axis, at the decades a listener thinks in.
pub fn frequency_axis(width: u16, theme: &Theme) -> Line<'static> {
    let marks: [(f64, &str); 7] = [
        (20.0, "20"),
        (100.0, "100"),
        (500.0, "500"),
        (1000.0, "1k"),
        (5000.0, "5k"),
        (10000.0, "10k"),
        (20000.0, "20k"),
    ];

    let mut row = vec![' '; width as usize];
    for (hz, label) in marks {
        let centre = Bode::x_of(hz, width).round() as isize;
        let start = centre - label.len() as isize / 2;
        for (i, c) in label.chars().enumerate() {
            let p = start + i as isize;
            if p >= 0 && (p as usize) < row.len() {
                row[p as usize] = c;
            }
        }
    }

    Line::from(Span::styled(
        row.into_iter().collect::<String>(),
        Style::default().fg(theme.dim),
    ))
}

/// A horizontal level meter with a dedicated clip zone.
///
/// The clip indicator is driven by the device's sticky latch, not by the bar
/// reaching full scale: a single sample over is what matters, and it would be
/// invisible in a bar that has already fallen back.
pub struct Meter<'a> {
    pub label: &'a str,
    /// Peak as a linear 0..1 fraction, as the device reports it.
    pub level: f32,
    pub clipped: bool,
    pub color: Color,
    pub theme: &'a Theme,
    pub label_width: u16,
}

impl Meter<'_> {
    /// A meter is read logarithmically; a linear bar spends most of its length
    /// on the top 6 dB and tells you nothing about a quiet signal.
    pub fn db(level: f32) -> f32 {
        if level <= 0.0001 {
            -60.0
        } else {
            20.0 * level.log10()
        }
    }

    fn fraction(level: f32) -> f32 {
        ((Self::db(level) + 60.0) / 60.0).clamp(0.0, 1.0)
    }
}

impl Widget for Meter<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < self.label_width + 4 {
            return;
        }
        let y = area.y;

        buf.set_string(
            area.x,
            y,
            format!("{:<w$}", self.label, w = self.label_width as usize),
            Style::default().fg(self.theme.dim),
        );

        // Reserve the last two columns: one for the clip flag, one for the value.
        let bar_x = area.x + self.label_width;
        let readout = format!("{:>5.0}", Self::db(self.level));
        let bar_w = area
            .width
            .saturating_sub(self.label_width + readout.len() as u16 + 2);
        if bar_w == 0 {
            return;
        }

        let filled = (Self::fraction(self.level) * bar_w as f32).round() as u16;
        for i in 0..bar_w {
            let (ch, style) = if i < filled {
                ("▓", Style::default().fg(self.color))
            } else {
                ("░", Style::default().fg(self.theme.chrome))
            };
            buf[(bar_x + i, y)].set_symbol(ch).set_style(style);
        }

        buf.set_string(
            bar_x + bar_w,
            y,
            &readout,
            Style::default().fg(self.theme.dim),
        );

        let flag_x = bar_x + bar_w + readout.len() as u16 + 1;
        if flag_x < area.x + area.width {
            let (ch, style) = if self.clipped {
                ("▌", self.theme.alarm())
            } else {
                (" ", Style::default())
            };
            buf[(flag_x, y)].set_symbol(ch).set_style(style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    fn render_to(width: u16, height: u16, f: impl FnOnce(Rect, &mut Buffer)) -> Buffer {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        f(area, &mut buf);
        buf
    }

    fn as_text(buf: &Buffer) -> String {
        let area = *buf.area();
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_flat_curve_draws_along_the_zero_line() {
        let theme = Theme::dark(crate::theme::ColorDepth::TrueColor, Glyphs::Braille);
        let points = vec![0.0f64; dsp::POINTS];
        let buf = render_to(40, 9, |area, buf| {
            Bode::new(&theme)
                .curve(Curve {
                    label: "test",
                    color: theme.fg,
                    points: &points,
                    focused: true,
                })
                .render(area, buf);
        });
        let text = as_text(&buf);
        let rows: Vec<&str> = text.lines().collect();
        // The curve should sit in the middle, not at an edge.
        let drawn: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.chars().any(|c| c as u32 >= 0x2800 && c as u32 <= 0x28FF))
            .map(|(i, _)| i)
            .collect();
        assert!(!drawn.is_empty(), "nothing was drawn:\n{text}");
        assert!(
            drawn.iter().all(|r| *r > 0 && *r < rows.len() - 1),
            "a 0 dB curve should sit mid-plot, drew on rows {drawn:?}"
        );
    }

    // ------------------------------------------------------- the dB scale

    fn axis_labels(height: u16, range: f64) -> Vec<(usize, String)> {
        let theme = Theme::dark(crate::theme::ColorDepth::TrueColor, Glyphs::Braille);
        db_axis(height, range / 2.0, -range / 2.0, &theme)
            .iter()
            .enumerate()
            .map(|(i, l)| {
                (
                    i,
                    l.spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>()
                        .trim()
                        .to_string(),
                )
            })
            .filter(|(_, t)| !t.is_empty())
            .collect()
    }

    /// The whole point: the zoom keys change the span, so a scale that did not
    /// follow would be wrong at every setting but one.
    #[test]
    fn the_scale_follows_the_zoom() {
        let tight = axis_labels(20, 10.0);
        let wide = axis_labels(20, 100.0);
        assert_ne!(tight, wide, "the scale did not change with the span");

        // The topmost label is the highest tick inside the span, not the span
        // itself: at ±5 dB the step is 2, so it reads +4 rather than +5.
        let top = |v: &[(usize, String)]| v.first().unwrap().1.clone();
        assert_eq!(top(&tight), "+4");
        assert_eq!(top(&wide), "+40");
    }

    /// Every label has to name a value the plot is actually showing, or it
    /// invites the user to read off a number that is not on screen.
    #[test]
    fn no_label_falls_outside_the_visible_span() {
        for range in [10.0f64, 15.0, 30.0, 55.0, 100.0] {
            for height in [6u16, 9, 14, 30] {
                for (_, text) in axis_labels(height, range) {
                    let db: f64 = text.parse().expect(&text);
                    assert!(
                        db.abs() <= range / 2.0 + 1e-9,
                        "{db} is outside ±{} at height {height}",
                        range / 2.0
                    );
                }
            }
        }
    }

    /// Zero is the line the eye returns to, so it is never the label that gets
    /// dropped when the plot is short.
    #[test]
    fn zero_is_always_labelled() {
        for range in [10.0f64, 15.0, 30.0, 55.0, 100.0] {
            for height in [4u16, 6, 9, 14, 30] {
                let labels = axis_labels(height, range);
                assert!(
                    labels.iter().any(|(_, t)| t == "0"),
                    "no zero at ±{} over {height} rows: {labels:?}",
                    range / 2.0
                );
            }
        }
    }

    /// A label on the wrong row is worse than none: it misreads the curve.
    /// The zero label must land on the row the plot draws its zero line.
    #[test]
    fn the_labels_line_up_with_the_plot() {
        let theme = Theme::dark(crate::theme::ColorDepth::TrueColor, Glyphs::Braille);
        for range in [10.0f64, 30.0, 100.0] {
            for height in [6u16, 9, 14, 25] {
                let buf = render_to(40, height, |area, buf| {
                    Bode {
                        curves: Vec::new(),
                        theme: &theme,
                        db_top: range / 2.0,
                        db_bottom: -range / 2.0,
                        marker_hz: None,
                        cursor: None,
                    }
                    .render(area, buf);
                });
                let plot_zero = as_text(&buf)
                    .lines()
                    .position(|l| l.starts_with('─'))
                    .expect("no zero line");
                let label_zero = axis_labels(height, range)
                    .into_iter()
                    .find(|(_, t)| t == "0")
                    .expect("no zero label")
                    .0;
                assert_eq!(
                    label_zero,
                    plot_zero,
                    "zero label and zero line disagree at ±{} over {height} rows",
                    range / 2.0
                );
            }
        }
    }

    /// The gutter is reserved before the labels are drawn, so if it is too
    /// narrow the numbers are silently clipped.
    #[test]
    fn the_gutter_fits_the_widest_label() {
        for range in [10.0f64, 15.0, 30.0, 55.0, 100.0] {
            for height in [6u16, 9, 14, 30] {
                let (top, bottom) = (range / 2.0, -range / 2.0);
                let reserved = db_axis_width(height, top, bottom);
                let widest = axis_labels(height, range)
                    .iter()
                    .map(|(_, t)| t.chars().count())
                    .max()
                    .unwrap_or(0);
                assert!(
                    reserved as usize > widest,
                    "gutter {reserved} cannot hold a {widest}-char label at ±{top}"
                );
            }
        }
    }

    /// One line per row, or the labels drift out of step with the plot.
    #[test]
    fn the_axis_returns_a_line_per_row() {
        let theme = Theme::dark(crate::theme::ColorDepth::TrueColor, Glyphs::Braille);
        for height in [0u16, 1, 5, 40] {
            assert_eq!(db_axis(height, 15.0, -15.0, &theme).len(), height as usize);
        }
    }

    /// Labels a reader can do arithmetic with. Splitting the span into equal
    /// parts would give steps like 4.3 dB, which are correct and useless.
    #[test]
    fn steps_are_round_numbers() {
        for range in [10.0f64, 15.0, 30.0, 55.0, 100.0] {
            let labels = axis_labels(20, range);
            let values: Vec<f64> = labels.iter().map(|(_, t)| t.parse().unwrap()).collect();
            for pair in values.windows(2) {
                let step = (pair[0] - pair[1]).abs();
                let decade = 10f64.powf(step.log10().floor());
                let scaled = step / decade;
                assert!(
                    [1.0, 2.0, 5.0].iter().any(|n| (scaled - n).abs() < 1e-9),
                    "{step} is not a 1-2-5 step (from {values:?})"
                );
            }
        }
    }

    #[test]
    fn a_boosted_curve_sits_above_a_cut_one() {
        let theme = Theme::dark(crate::theme::ColorDepth::TrueColor, Glyphs::Braille);
        let row_of = |db: f64| -> usize {
            let points = vec![db; dsp::POINTS];
            let buf = render_to(40, 9, |area, buf| {
                Bode::new(&theme)
                    .curve(Curve {
                        label: "t",
                        color: theme.fg,
                        points: &points,
                        focused: true,
                    })
                    .render(area, buf);
            });
            as_text(&buf)
                .lines()
                .position(|r| r.chars().any(|c| (0x2800..=0x28FF).contains(&(c as u32))))
                .unwrap()
        };
        // Rows count downward, so a boost has the smaller index.
        assert!(row_of(10.0) < row_of(-10.0));
    }

    #[test]
    fn the_graph_declines_to_draw_in_a_hopeless_space() {
        let theme = Theme::default();
        let points = vec![0.0f64; dsp::POINTS];
        let buf = render_to(3, 2, |area, buf| {
            Bode::new(&theme)
                .curve(Curve {
                    label: "t",
                    color: theme.fg,
                    points: &points,
                    focused: true,
                })
                .render(area, buf);
        });
        assert!(as_text(&buf).trim().is_empty());
    }

    #[test]
    fn the_block_fallback_still_draws_something() {
        let theme = Theme::dark(crate::theme::ColorDepth::Ansi16, Glyphs::Blocks);
        let points = vec![6.0f64; dsp::POINTS];
        let buf = render_to(40, 9, |area, buf| {
            Bode::new(&theme)
                .curve(Curve {
                    label: "t",
                    color: theme.fg,
                    points: &points,
                    focused: true,
                })
                .render(area, buf);
        });
        let text = as_text(&buf);
        assert!(
            text.chars().any(|c| "▁▂▃▄▅▆▇█".contains(c)),
            "no block glyphs drawn:\n{text}"
        );
    }

    #[test]
    fn the_frequency_axis_lands_its_labels_in_order() {
        let theme = Theme::default();
        let line = frequency_axis(60, &theme);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let pos = |s: &str| text.find(s).unwrap_or(usize::MAX);
        assert!(pos("20") < pos("100"));
        assert!(pos("100") < pos("1k"));
        assert!(pos("1k") < pos("10k"));
    }

    #[test]
    fn meter_scale_is_logarithmic_with_a_sixty_db_floor() {
        assert_eq!(Meter::db(1.0), 0.0);
        assert!((Meter::db(0.5) + 6.02).abs() < 0.05);
        assert_eq!(Meter::db(0.0), -60.0);
        assert_eq!(Meter::fraction(1.0), 1.0);
        assert_eq!(Meter::fraction(0.0), 0.0);
    }

    #[test]
    fn a_meter_shows_its_label_level_and_readout() {
        let theme = Theme::default();
        let buf = render_to(30, 1, |area, buf| {
            Meter {
                label: "In 1",
                level: 0.5,
                clipped: false,
                color: theme.fg,
                theme: &theme,
                label_width: 6,
            }
            .render(area, buf);
        });
        let text = as_text(&buf);
        assert!(text.starts_with("In 1"), "{text}");
        assert!(text.contains('▓'), "no filled bar: {text}");
        assert!(text.contains("-6"), "no readout: {text}");
    }

    /// The clip flag is latched by the device, so it must show even when the bar
    /// has already fallen back to a low level.
    #[test]
    fn the_clip_flag_is_independent_of_the_current_level() {
        let theme = Theme::default();
        let buf = render_to(30, 1, |area, buf| {
            Meter {
                label: "Sub",
                level: 0.01,
                clipped: true,
                color: theme.fg,
                theme: &theme,
                label_width: 6,
            }
            .render(area, buf);
        });
        assert!(as_text(&buf).contains('▌'), "clip flag missing");
    }

    #[test]
    fn a_meter_too_narrow_to_be_useful_draws_nothing() {
        let theme = Theme::default();
        let buf = render_to(6, 1, |area, buf| {
            Meter {
                label: "In 1",
                level: 0.5,
                clipped: false,
                color: theme.fg,
                theme: &theme,
                label_width: 6,
            }
            .render(area, buf);
        });
        assert!(as_text(&buf).trim().is_empty());
    }
}

/// A meter sized to sit inline beside a channel name.
///
/// The full [`Meter`] carries a label and a numeric readout; in a channel list
/// the name is already there and the number would crowd out the bar, so this
/// draws the bar and the clip flag only. The level is still logarithmic: a
/// linear bar spends most of its length on the top 6 dB.
pub struct InlineMeter<'a> {
    pub level: f32,
    pub clipped: bool,
    pub color: Color,
    pub theme: &'a Theme,
}

impl Widget for InlineMeter<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 3 {
            return;
        }

        // One column is reserved for the clip flag, which must stay visible even
        // when the bar has fallen back: the latch is what matters, not the
        // current level.
        let bar_w = area.width - 1;
        let filled = (Meter::fraction(self.level) * bar_w as f32).round() as u16;

        for i in 0..bar_w {
            let (ch, style) = if i < filled {
                ("▓", Style::default().fg(self.color))
            } else {
                ("░", Style::default().fg(self.theme.chrome))
            };
            buf[(area.x + i, area.y)].set_symbol(ch).set_style(style);
        }

        let (ch, style) = if self.clipped {
            ("▌", self.theme.alarm())
        } else {
            (" ", Style::default())
        };
        buf[(area.x + bar_w, area.y)]
            .set_symbol(ch)
            .set_style(style);
    }
}

#[cfg(test)]
mod inline_meter_tests {
    use super::*;
    use crate::theme::ColorDepth;

    fn render(level: f32, clipped: bool, width: u16) -> String {
        let theme = Theme::dark(ColorDepth::TrueColor, Glyphs::Braille);
        let area = Rect::new(0, 0, width, 1);
        let mut buf = Buffer::empty(area);
        InlineMeter {
            level,
            clipped,
            color: theme.fg,
            theme: &theme,
        }
        .render(area, &mut buf);
        (0..width).map(|x| buf[(x, 0)].symbol()).collect()
    }

    #[test]
    fn the_bar_fills_with_level() {
        let quiet = render(0.001, false, 12);
        let loud = render(1.0, false, 12);
        assert!(quiet.matches('▓').count() < loud.matches('▓').count());
        assert!(loud.starts_with('▓'));
    }

    #[test]
    fn silence_draws_an_empty_bar_rather_than_nothing() {
        let s = render(0.0, false, 12);
        assert!(s.contains('░'), "an empty channel still needs a bar: {s}");
        assert!(!s.contains('▓'));
    }

    /// The clip latch is what matters, not the current level, so the flag must
    /// show even on a channel that has fallen silent since.
    #[test]
    fn the_clip_flag_survives_a_quiet_bar() {
        assert!(render(0.0, true, 12).contains('▌'));
        assert!(!render(1.0, false, 12).contains('▌'));
    }

    #[test]
    fn a_full_bar_still_leaves_room_for_the_flag() {
        let s = render(1.0, true, 12);
        assert!(s.ends_with('▌'), "the flag was overwritten: {s}");
    }

    #[test]
    fn a_hopeless_width_draws_nothing_rather_than_a_stub() {
        assert_eq!(render(1.0, false, 2).trim(), "");
    }
}
