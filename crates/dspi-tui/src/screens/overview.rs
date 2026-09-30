//! The overview: a grid of small graphs, one per group of channels whose
//! curves are identical (DESIGN 12.2).
//!
//! The Console's `groupedChannels` rule decides the cells: two channels share
//! one when their responses are bit-identical after output gain is folded
//! in. Inputs and outputs never share a cell, because their summary lines
//! say different things. Each cell carries the member names in its title, a
//! plot in grey on a 0 dB rule, and one line: `5 bands · preamp -5.3 dB`,
//! `HP 80 Hz LR4 · -3.0 dB`, or `no filters`. The focused cell takes its
//! first channel's hue.

use crossterm::event::{KeyCode, KeyEvent};
use dspi_proto::{FilterType, dsp, xover};
use dspi_session::DeviceState;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, Widget};

use super::{Shared, channel_name, type_code};
use crate::graph::{Graph, GraphCurve, GraphSettings};
use crate::shell::{Screen, ScreenEvent, Selection};
use crate::theme::{ChannelRole, Glyphs, Theme};
use crate::widgets::KeyHelp;
use crate::widgets::text::{db, fit_centre, fit_left, hz, ms};

/// One cell of the grid.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    /// Unified channel indices, in channel order.
    pub channels: Vec<usize>,
    /// The shared response, with output gain folded in.
    pub magnitude: Vec<f64>,
    /// A constant line: drawn as the word `flat` rather than a curve.
    pub flat: bool,
    pub summary: String,
}

/// The bands the graph sees for a channel: PEQ, then the crossover on an
/// output, with the output trim as the gain offset.
fn bands_of(state: &DeviceState, channel: usize) -> (Vec<dsp::Band>, f64) {
    let ni = state.caps.num_inputs as usize;
    let band = |p: &dspi_proto::value::EqParamPacket| dsp::Band {
        filter_type: p.filter_type,
        freq: p.freq,
        q: p.q,
        gain_db: p.gain_db,
        bypass: p.bypass,
    };
    let mut bands: Vec<dsp::Band> = state.bands(channel as u8).iter().map(band).collect();
    if channel >= ni {
        bands.extend(state.xover_bands(channel as u8).iter().map(band));
        (bands, state.output(channel - ni).gain_db as f64)
    } else {
        (bands, 0.0)
    }
}

/// The channels the grid shows: every input, and every enabled output.
fn channels(state: &DeviceState) -> Vec<usize> {
    let ni = state.caps.num_inputs as usize;
    let no = state.caps.num_outputs as usize;
    (0..ni)
        .chain((0..no).filter(|o| state.output(*o).enabled).map(|o| ni + o))
        .collect()
}

fn bands_word(n: usize) -> String {
    if n == 1 {
        "1 band".into()
    } else {
        format!("{n} bands")
    }
}

/// The summary line for a channel (DESIGN 12.2): what is set, and nothing
/// that is not, so a cell does not spend its one line on `+0.0 dB`.
fn summary(state: &DeviceState, channel: usize, flat: bool) -> String {
    let ni = state.caps.num_inputs as usize;
    let active = |p: &dspi_proto::value::EqParamPacket| p.filter_type != FilterType::Flat;
    let peq = state
        .bands(channel as u8)
        .iter()
        .filter(|p| active(p))
        .count();
    let mut parts: Vec<String> = Vec::new();
    if channel < ni {
        if flat {
            parts.push("no filters".into());
        } else {
            parts.push(bands_word(peq));
        }
        let preamp = state.preamp_db(channel) as f64;
        if preamp != 0.0 {
            parts.push(format!("preamp {}", db(preamp)));
        }
    } else {
        let out = state.output(channel - ni);
        let xover: Vec<String> = state
            .xover_bands(channel as u8)
            .iter()
            .filter(|p| active(p))
            .map(|p| {
                let f = hz(p.freq as f64);
                match xover::meta(p.filter_type.to_raw()) {
                    Some(m) => format!(
                        "{} {f} {}{}",
                        if m.high_pass { "HP" } else { "LP" },
                        m.family.short().to_uppercase(),
                        m.order
                    ),
                    // A plain pass in a crossover slot keeps the crossover's
                    // LP / HP: only PEQ bands read as cuts (DSPMath.swift:170-172).
                    None => match p.filter_type {
                        FilterType::LowPass => format!("LP {f}"),
                        FilterType::HighPass => format!("HP {f}"),
                        FilterType::LowPass1 => format!("LP1 {f}"),
                        FilterType::HighPass1 => format!("HP1 {f}"),
                        other => format!("{} {f}", type_code(other)),
                    },
                }
            })
            .collect();
        if xover.is_empty() && peq == 0 {
            parts.push("no filters".into());
        } else {
            parts.extend(xover);
            if peq > 0 {
                parts.push(bands_word(peq));
            }
        }
        let trim = out.gain_db as f64;
        if trim != 0.0 {
            parts.push(db(trim));
        }
    }
    let delay = if channel < ni {
        state.delay_ms(channel)
    } else {
        state.output(channel - ni).delay_ms
    };
    if delay != 0.0 {
        parts.push(ms(delay as f64));
    }
    parts.join(" · ")
}

pub struct Overview {
    #[allow(dead_code)]
    shared: Shared,
    /// Which cell the cursor is on.
    pub cursor: usize,
    /// The first row of cells drawn.
    pub scroll: usize,
    /// The last frame's column count, so the arrows know the grid's shape.
    cols: usize,
    /// Responses by channel, kept between frames: a curve is only recomputed
    /// when its bands or gain change.
    cache: Vec<(Vec<dsp::Band>, f64, Vec<f64>)>,
}

impl Overview {
    pub fn new(shared: Shared) -> Self {
        Self {
            shared,
            cursor: 0,
            scroll: 0,
            cols: 3,
            cache: Vec::new(),
        }
    }

    /// The grid's cells for the device's current state, in channel order.
    ///
    /// None without a device: the Console's dashboard renders no cards then,
    /// since they would only repeat the last device's channels
    /// (`DashboardView.swift:86-91`).
    pub fn cells(&mut self, state: &DeviceState) -> Vec<Cell> {
        if !state.connected {
            return Vec::new();
        }
        let ni = state.caps.num_inputs as usize;
        let mut out: Vec<Cell> = Vec::new();
        for ch in channels(state) {
            let (bands, gain) = bands_of(state, ch);
            if self.cache.len() <= ch {
                self.cache
                    .resize(ch + 1, (Vec::new(), f64::NAN, Vec::new()));
            }
            let slot = &mut self.cache[ch];
            if slot.0 != bands || slot.1.to_bits() != gain.to_bits() {
                *slot = (bands.clone(), gain, dsp::curve(&bands, gain));
            }
            let magnitude = &slot.2;
            let same_role = |c: &Cell| (c.channels[0] < ni) == (ch < ni);
            let identical = |c: &Cell| {
                c.magnitude.len() == magnitude.len()
                    && c.magnitude
                        .iter()
                        .zip(magnitude)
                        .all(|(a, b)| a.to_bits() == b.to_bits())
            };
            if let Some(cell) = out.iter_mut().find(|c| same_role(c) && identical(c)) {
                cell.channels.push(ch);
                continue;
            }
            let flat = magnitude
                .iter()
                .all(|v| v.to_bits() == magnitude[0].to_bits());
            out.push(Cell {
                channels: vec![ch],
                magnitude: magnitude.clone(),
                flat,
                summary: summary(state, ch, flat),
            });
        }
        out
    }

    /// The narrowest cell that still reads: a 20-column cell keeps the
    /// summary (`HP 80 Hz LR4 · 2 bands`) and an 18-column plot.
    const MIN_W: u16 = 20;
    /// The shortest cell: three plot rows between the title and the summary.
    const MIN_H: u16 = 6;
    /// The tallest cell worth drawing; beyond ten plot rows a cell is a
    /// graph, and the channel page has one of those.
    const MAX_H: u16 = 13;

    /// Columns of cells, and rows per cell, for `n` cells in this region:
    /// the largest cells that let every cell fit, so a full device is seen
    /// whole wherever the pane has room for it (DESIGN 12.7). When even the
    /// smallest cells will not fit, the grid takes the most columns it can
    /// at the smallest height and scrolls.
    fn shape(area: Rect, n: usize) -> (usize, u16) {
        let n = n.max(1);
        let mut best: Option<(usize, u16, u16)> = None;
        let mut widest_cols = 1;
        for cols in 1..=8usize {
            let w = area.width / cols as u16;
            if w < Self::MIN_W {
                break;
            }
            widest_cols = cols;
            let rows_needed = n.div_ceil(cols) as u16;
            let h = (area.height / rows_needed.max(1)).min(Self::MAX_H);
            if h < Self::MIN_H {
                continue;
            }
            if best.is_none_or(|(_, bh, bw)| (h, w) > (bh, bw)) {
                best = Some((cols, h, w));
            }
        }
        match best {
            Some((cols, h, _)) => (cols, h),
            None => (widest_cols, Self::MIN_H),
        }
    }
}

const KEYS: &[KeyHelp] = &[
    KeyHelp::new("↑ ↓ ← →", "Move between cells"),
    KeyHelp::new("Enter", "Select that channel"),
    KeyHelp::new("1-9", "Select the nth cell"),
    KeyHelp::new("PgUp PgDn", "Scroll"),
];

impl Screen for Overview {
    fn title(&self) -> String {
        "Overview".into()
    }

    fn draw(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        state: &DeviceState,
        focused: bool,
    ) {
        if area.height < 4 || area.width < 20 {
            return;
        }
        let cells = self.cells(state);
        if cells.is_empty() {
            return;
        }
        self.cursor = self.cursor.min(cells.len() - 1);
        let (cols, step) = Self::shape(area, cells.len());
        self.cols = cols;
        let rows = cells.len().div_ceil(cols) as u16;

        // Keep the focused cell in view.
        let focus_row = (self.cursor / cols) as u16;
        let visible_rows = (area.height / step).max(1);
        let mut first = self.scroll as u16;
        if focus_row < first {
            first = focus_row;
        } else if focus_row >= first + visible_rows {
            first = focus_row + 1 - visible_rows;
        }
        let max_first = rows.saturating_sub(visible_rows);
        first = first.min(max_first);
        self.scroll = first as usize;

        // The y-range follows the main graph so cells compare (DESIGN 12.2).
        let settings = GraphSettings {
            freq_grid: false,
            freq_labels: false,
            db_labels: false,
            show_phase: false,
            ..self.shared.borrow().graph.clone()
        };
        let col_w = area.width / cols as u16;
        for (i, cell) in cells.iter().enumerate() {
            let row = (i / cols) as u16;
            if row < first {
                continue;
            }
            let y = area.y + (row - first) * step;
            if y >= area.y + area.height {
                break;
            }
            let x = area.x + (i % cols) as u16 * col_w;
            let r = Rect::new(
                x,
                y,
                col_w.saturating_sub(1),
                step.min(area.y + area.height - y),
            );
            draw_cell(
                r,
                buf,
                theme,
                state,
                cell,
                &settings,
                focused && i == self.cursor,
            );
        }

        if rows > visible_rows {
            let hint_x = area.x + area.width - 1;
            if first > 0 {
                buf.set_string(hint_x, area.y, "▲", theme.label());
            }
            if first < max_first {
                buf.set_string(hint_x, area.y + area.height - 1, "▼", theme.label());
            }
        }
    }

    fn handle(&mut self, key: KeyEvent, state: &DeviceState) -> ScreenEvent {
        let cells = self.cells(state);
        if cells.is_empty() {
            return ScreenEvent::Unhandled;
        }
        let last = cells.len() - 1;
        let cols = self.cols.max(1);
        let select = |cell: &Cell| {
            let ch = cell.channels[0];
            let ni = state.caps.num_inputs as usize;
            ScreenEvent::Select(if ch < ni {
                Selection::Input(ch)
            } else {
                Selection::Output(ch - ni)
            })
        };
        match key.code {
            KeyCode::Left | KeyCode::Char('h') => {
                self.cursor = self.cursor.saturating_sub(1);
                ScreenEvent::Handled
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.cursor = (self.cursor + 1).min(last);
                ScreenEvent::Handled
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.cursor = self.cursor.saturating_sub(cols);
                ScreenEvent::Handled
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.cursor + cols <= last {
                    self.cursor += cols;
                }
                ScreenEvent::Handled
            }
            KeyCode::PageUp => {
                self.cursor = self.cursor.saturating_sub(cols * 3);
                ScreenEvent::Handled
            }
            KeyCode::PageDown => {
                self.cursor = (self.cursor + cols * 3).min(last);
                ScreenEvent::Handled
            }
            KeyCode::Enter => select(&cells[self.cursor]),
            KeyCode::Char(d @ '1'..='9') => {
                let i = d as usize - '1' as usize;
                match cells.get(i) {
                    Some(cell) => {
                        self.cursor = i;
                        select(cell)
                    }
                    None => ScreenEvent::Handled,
                }
            }
            _ => ScreenEvent::Unhandled,
        }
    }

    fn keys(&self) -> &'static [KeyHelp] {
        KEYS
    }
}

fn draw_cell(
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    state: &DeviceState,
    cell: &Cell,
    settings: &GraphSettings,
    focused: bool,
) {
    if area.height < 3 || area.width < 8 {
        return;
    }
    let t = theme;
    let first = cell.channels[0];
    let role = ChannelRole::of(first as u8, state.caps.num_inputs, state.caps.num_outputs);
    // Colour on demand: the focused cell alone takes its channel's hue.
    let hue = t.hue_for(role, focused);
    let names: Vec<String> = cell
        .channels
        .iter()
        .map(|ch| channel_name(state, *ch))
        .collect();
    let title = fit_left(&names.join(" "), area.width.saturating_sub(4) as usize);
    let title_style = if focused {
        Style::default().fg(hue).add_modifier(Modifier::BOLD)
    } else {
        t.section()
    };
    let border = if focused {
        Style::default().fg(hue)
    } else {
        t.chrome_style()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(if t.glyphs == Glyphs::Ascii {
            BorderType::Plain
        } else {
            BorderType::Rounded
        })
        .border_style(border)
        .title(format!(" {} ", title.trim_end()))
        .title_style(title_style);
    let inner = block.inner(area);
    block.render(area, buf);
    if inner.height < 2 || inner.width < 4 {
        return;
    }

    // The summary gets one row, or two when it will not fit and the plot can
    // spare a row: a narrow cell says `HP 60 Hz LR4` over `2 bands` rather
    // than `HP 60 Hz LR4 · 2 b…`.
    let text_w = inner.width.saturating_sub(1) as usize;
    let lines = summary_lines(&cell.summary, text_w);
    let summary_rows = if lines.len() > 1 && inner.height >= 5 {
        2
    } else {
        1
    };
    let plot = Rect::new(inner.x, inner.y, inner.width, inner.height - summary_rows);
    if cell.flat {
        // A constant line says nothing a word cannot say better.
        buf.set_string(
            plot.x,
            plot.y + plot.height / 2,
            fit_centre("flat", plot.width as usize),
            t.label(),
        );
    } else {
        let curve = [GraphCurve {
            descriptor: names[0].clone(),
            color: if focused { hue } else { t.dim },
            magnitude: cell.magnitude.clone(),
            phase: None,
            selected: focused,
        }];
        Graph::new(&curve, settings, t).render(plot, buf);
    }
    // With one row and a summary that needs two, the whole line is shown
    // cut rather than its first half shown whole.
    let shown: Vec<&str> = if summary_rows == 1 {
        vec![cell.summary.as_str()]
    } else {
        lines.iter().map(String::as_str).collect()
    };
    for (i, line) in shown.iter().enumerate() {
        buf.set_string(
            inner.x + 1,
            inner.y + inner.height - summary_rows + i as u16,
            fit_left(line, text_w),
            t.label(),
        );
    }
}

/// The summary as it fits `width`: one line when it does, else its ` · `
/// parts packed onto two.
fn summary_lines(summary: &str, width: usize) -> Vec<String> {
    if summary.chars().count() <= width {
        return vec![summary.to_string()];
    }
    let mut lines: Vec<String> = Vec::new();
    for part in summary.split(" · ") {
        match lines.last_mut() {
            Some(last) if last.chars().count() + 3 + part.chars().count() <= width => {
                last.push_str(" · ");
                last.push_str(part);
            }
            _ => lines.push(part.to_string()),
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::shared;
    use crate::shell::fixture;
    use crate::theme::{ColorDepth, Glyphs};
    use crate::widgets::testing::key;

    fn draw(w: u16, h: u16) -> String {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let state = fixture::state();
        let mut s = Overview::new(shared());
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        term.draw(|f| s.draw(f.area(), f.buffer_mut(), &t, &state, true))
            .unwrap();
        let buf = term.backend().buffer();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn identical_curves_share_a_cell_and_roles_never_mix() {
        let state = fixture::state();
        let cells = Overview::new(shared()).cells(&state);
        let members: Vec<Vec<usize>> = cells.iter().map(|c| c.channels.clone()).collect();
        assert_eq!(
            members,
            vec![
                vec![0, 1],
                vec![2, 3, 4, 5, 6, 7],
                vec![8, 9],
                vec![10, 11, 12, 13, 14, 15],
                vec![16],
            ],
            "{cells:?}"
        );
        assert!(!cells[0].flat && cells[1].flat && !cells[2].flat);
        assert_eq!(cells[0].summary, "5 bands");
        assert_eq!(cells[1].summary, "no filters");
        assert_eq!(cells[2].summary, "HP 80 Hz");
        assert_eq!(cells[3].summary, "no filters");
        assert_eq!(cells[4].summary, "LP 80 Hz · -3.0 dB");
    }

    #[test]
    fn the_busy_fixture_fills_the_grid_and_draws_its_crossovers() {
        let state = fixture::busy_state();
        let cells = Overview::new(shared()).cells(&state);
        let members: Vec<Vec<usize>> = cells.iter().map(|c| c.channels.clone()).collect();
        assert_eq!(
            members,
            vec![
                vec![0, 1],
                vec![2],
                vec![3],
                vec![4, 5],
                vec![6, 7],
                vec![8, 9],
                vec![10, 11],
                vec![13, 14, 15],
                vec![16],
            ],
            "OUT 5 is off and every tuning is its own cell: {members:?}"
        );
        let summaries: Vec<&str> = cells.iter().map(|c| c.summary.as_str()).collect();
        assert_eq!(
            summaries,
            vec![
                "5 bands · preamp -5.3 dB",
                "3 bands · preamp -5.3 dB",
                "2 bands · preamp -5.3 dB",
                "4 bands · preamp -5.3 dB",
                "no filters · preamp -5.3 dB",
                "HP 80 Hz LR4 · 2 bands",
                "HP 100 Hz BW2 · -2.0 dB",
                "no filters",
                "LP 80 Hz LR4 · -3.0 dB · 2.5 ms",
            ]
        );
        // A crossover-typed band shapes the curve, so none of those cells
        // is flat.
        assert!(!cells[5].flat && !cells[6].flat && !cells[8].flat);
        assert!(cells[4].flat && cells[7].flat);
    }

    #[test]
    fn a_crossover_and_a_delay_read_in_the_short_form() {
        let mut state = fixture::state();
        let (_, xo, _) = dspi_proto::generated::SECTIONS
            .iter()
            .find(|(n, _, _)| *n == "crossovers")
            .copied()
            .unwrap();
        // The sub's low pass becomes LR4 (raw 34).
        state.bulk.patch(xo + 16 * 4 * 16, &[34]);
        let (_, outs, _) = dspi_proto::generated::SECTIONS
            .iter()
            .find(|(n, _, _)| *n == "outputs")
            .copied()
            .unwrap();
        state.bulk.patch(outs + 8 * 12 + 8, &2.5f32.to_le_bytes());
        let cells = Overview::new(shared()).cells(&state);
        let sub = cells.iter().find(|c| c.channels == vec![16]).unwrap();
        assert_eq!(sub.summary, "LP 80 Hz LR4 · -3.0 dB · 2.5 ms");
    }

    #[test]
    fn a_disabled_output_leaves_the_grid() {
        let mut state = fixture::state();
        let (_, outs, _) = dspi_proto::generated::SECTIONS
            .iter()
            .find(|(n, _, _)| *n == "outputs")
            .copied()
            .unwrap();
        state.bulk.patch(outs + 3 * 12, &[0]);
        let cells = Overview::new(shared()).cells(&state);
        assert!(cells.iter().all(|c| !c.channels.contains(&11)), "{cells:?}");
        assert_eq!(cells.len(), 5);
    }

    #[test]
    fn the_reference_frame_draws_titled_cells_with_summaries() {
        let f = draw(94, 35);
        assert!(f.contains("╭ FL FR "), "{f}");
        assert!(f.contains("╭ FC LFE BL BR SL SR "), "{f}");
        assert!(f.contains("╭ OUT L OUT R "), "{f}");
        assert!(f.contains("╭ Sub "), "{f}");
        assert!(f.contains("5 bands"), "{f}");
        assert!(f.contains("flat"), "{f}");
        assert!(f.contains("LP 80 Hz · -3.0 dB"), "{f}");
        // Five cells in 94x35: three columns of the tallest cells, so the
        // second row of cells starts on row thirteen.
        let lines: Vec<&str> = f.lines().collect();
        assert!(lines[0].matches('╭').count() == 3, "{:?}", lines[0]);
        assert!(lines[13].starts_with("╭"), "{:?}", lines[13]);
        // A cell's curve is braille.
        assert!(
            f.chars().any(|c| ('\u{2800}'..='\u{28FF}').contains(&c)),
            "{f}"
        );
    }

    #[test]
    fn the_compact_frame_fits_two_columns_of_short_cells() {
        let f = draw(54, 19);
        let lines: Vec<&str> = f.lines().collect();
        assert_eq!(lines[0].matches('╭').count(), 2, "{:?}", lines[0]);
        assert!(lines[6].starts_with("╭"), "{:?}", lines[6]);
        assert!(
            lines[12].starts_with("╭"),
            "three rows fit: {:?}",
            lines[12]
        );
        assert!(f.contains("FL FR"), "{f}");
        assert!(!f.contains("▼"), "everything fits: {f}");
        let f = draw(54, 10);
        assert!(f.contains("▼"), "more rows below: {f}");
    }

    #[test]
    fn the_grid_sizes_its_cells_so_every_channel_fits_where_it_can() {
        // Seventeen distinct channels at the Normal density's 94x35: four
        // columns of seven-row cells, five rows, all on one screen.
        assert_eq!(Overview::shape(Rect::new(0, 0, 94, 35), 17), (4, 7));
        // At Wide's 170x55: five columns, thirteen-row cells.
        assert_eq!(Overview::shape(Rect::new(0, 0, 170, 55), 17), (5, 13));
        // Five cells get the tallest cells three abreast.
        assert_eq!(Overview::shape(Rect::new(0, 0, 94, 35), 5), (3, 13));
        // Compact's 54x19 cannot hold seventeen: two columns, the smallest
        // cells, and the grid scrolls.
        assert_eq!(Overview::shape(Rect::new(0, 0, 54, 19), 17), (2, 6));
        let state = fixture::full_state();
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let mut s = Overview::new(shared());
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(94, 35)).unwrap();
        term.draw(|f| s.draw(f.area(), f.buffer_mut(), &t, &state, true))
            .unwrap();
        let buf = term.backend().buffer();
        let f: String = (0..35)
            .map(|y| (0..94).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        for name in ["FL", "SR", "OUT 5", "Sub"] {
            assert!(
                f.contains(&format!("╭ {name} ")),
                "{name} is on screen:\n{f}"
            );
        }
        assert!(!f.contains('▼'), "nothing left to scroll to:\n{f}");
        assert!(f.contains(" 1 band "), "singular: {f}");
    }

    #[test]
    fn a_narrow_cell_wraps_its_summary_at_the_separators() {
        assert_eq!(summary_lines("5 bands", 19), vec!["5 bands"]);
        assert_eq!(
            summary_lines("HP 60 Hz LR4 · 2 bands", 19),
            vec!["HP 60 Hz LR4", "2 bands"]
        );
        assert_eq!(
            summary_lines("LP 80 Hz LR4 · -3.0 dB · 2.5 ms", 19),
            vec!["LP 80 Hz LR4", "-3.0 dB · 2.5 ms"]
        );
        // Seventeen cells at 94x35 are 22 wide: the summaries wrap whole.
        let state = fixture::full_state();
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let mut s = Overview::new(shared());
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(94, 35)).unwrap();
        term.draw(|f| s.draw(f.area(), f.buffer_mut(), &t, &state, true))
            .unwrap();
        let buf = term.backend().buffer();
        let f: String = (0..35)
            .map(|y| (0..94).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(f.contains(" preamp -5.3 dB "), "{f}");
        assert!(!f.contains("…│"), "no summary is cut: {f}");
    }

    #[test]
    fn arrows_walk_the_grid_and_enter_or_a_digit_names_the_channel() {
        let state = fixture::state();
        let mut s = Overview::new(shared());
        assert_eq!(s.handle(key(KeyCode::Right), &state), ScreenEvent::Handled);
        assert_eq!(s.cursor, 1);
        assert_eq!(s.handle(key(KeyCode::Down), &state), ScreenEvent::Handled);
        assert_eq!(s.cursor, 4, "down a row of three");
        assert_eq!(
            s.handle(key(KeyCode::Enter), &state),
            ScreenEvent::Select(Selection::Output(8))
        );
        assert_eq!(s.handle(key(KeyCode::Up), &state), ScreenEvent::Handled);
        assert_eq!(s.cursor, 1);
        assert_eq!(
            s.handle(key(KeyCode::Char('3')), &state),
            ScreenEvent::Select(Selection::Output(0))
        );
        assert_eq!(s.cursor, 2);
        assert_eq!(
            s.handle(key(KeyCode::Char('9')), &state),
            ScreenEvent::Handled,
            "no ninth cell"
        );
        for _ in 0..20 {
            s.handle(key(KeyCode::Right), &state);
        }
        assert_eq!(s.cursor, 4, "clamped to the last cell");
        assert_eq!(
            s.handle(key(KeyCode::Char('z')), &state),
            ScreenEvent::Unhandled
        );
    }

    #[test]
    fn every_advertised_key_is_handled() {
        let state = fixture::state();
        for help in KEYS {
            for k in crate::screens::tests::keys_for(help.key) {
                let mut s = Overview::new(shared());
                s.cursor = 2;
                let before = s.cursor;
                let ev = s.handle(k, &state);
                assert!(
                    ev != ScreenEvent::Unhandled || s.cursor != before,
                    "{:?} does nothing (from {:?})",
                    k.code,
                    help.key
                );
            }
        }
    }
}
