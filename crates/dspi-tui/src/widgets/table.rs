//! A simple table with fixed columns, a focused row and cell, and scroll hints.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Widget;

use super::text::{fit_left, fit_right};
use crate::theme::Theme;

#[derive(Debug, Clone)]
pub struct Column {
    pub title: &'static str,
    pub width: u16,
    pub right: bool,
}

impl Column {
    pub const fn left(title: &'static str, width: u16) -> Self {
        Self {
            title,
            width,
            right: false,
        }
    }
    pub const fn right(title: &'static str, width: u16) -> Self {
        Self {
            title,
            width,
            right: true,
        }
    }
}

/// One cell: text plus an optional colour override.
#[derive(Debug, Clone, Default)]
pub struct Cell {
    pub text: String,
    pub color: Option<Color>,
    pub dim: bool,
}

impl From<String> for Cell {
    fn from(text: String) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }
}
impl From<&str> for Cell {
    fn from(text: &str) -> Self {
        Self {
            text: text.into(),
            ..Default::default()
        }
    }
}

impl Cell {
    pub fn colored(text: impl Into<String>, color: Color) -> Self {
        Self {
            text: text.into(),
            color: Some(color),
            dim: false,
        }
    }
    pub fn dim(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            color: None,
            dim: true,
        }
    }
}

pub struct Table<'a> {
    pub columns: &'a [Column],
    pub rows: Vec<Vec<Cell>>,
    pub theme: &'a Theme,
    pub focused: bool,
    pub cursor: (usize, usize),
    /// Which cell is armed for typing, if any: shown bracketed.
    pub armed: Option<(usize, usize)>,
    pub scroll: usize,
    /// Draw the header row.
    pub header: bool,
}

impl<'a> Table<'a> {
    pub fn new(columns: &'a [Column], theme: &'a Theme) -> Self {
        Self {
            columns,
            rows: Vec::new(),
            theme,
            focused: false,
            cursor: (0, 0),
            armed: None,
            scroll: 0,
            header: true,
        }
    }
    pub fn rows(mut self, rows: Vec<Vec<Cell>>) -> Self {
        self.rows = rows;
        self
    }
    pub fn focused(mut self, f: bool, cursor: (usize, usize)) -> Self {
        self.focused = f;
        self.cursor = cursor;
        self
    }
    pub fn armed(mut self, a: Option<(usize, usize)>) -> Self {
        self.armed = a;
        self
    }
    pub fn scroll(mut self, s: usize) -> Self {
        self.scroll = s;
        self
    }
    pub fn header(mut self, h: bool) -> Self {
        self.header = h;
        self
    }

    /// The scroll offset that keeps `row` visible in `height` body rows.
    pub fn scroll_to(row: usize, current: usize, height: usize) -> usize {
        if height == 0 {
            return 0;
        }
        if row < current {
            row
        } else if row >= current + height {
            row + 1 - height
        } else {
            current
        }
    }
}

impl Widget for Table<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 4 {
            return;
        }
        let t = self.theme;
        let mut y = area.y;
        // Column x positions, fitting what is available; the last column is
        // clipped rather than the first.
        let mut xs = Vec::new();
        let mut x = area.x + 2;
        for c in self.columns {
            xs.push((x, c.width.min((area.x + area.width).saturating_sub(x))));
            x += c.width + 1;
        }
        if self.header {
            for (c, (cx, cw)) in self.columns.iter().zip(&xs) {
                if *cw == 0 {
                    continue;
                }
                let text = if c.right {
                    fit_right(&c.title.to_uppercase(), *cw as usize)
                } else {
                    fit_left(&c.title.to_uppercase(), *cw as usize)
                };
                buf.set_string(*cx, y, text, t.section());
            }
            y += 1;
        }
        let body_h = (area.y + area.height).saturating_sub(y) as usize;
        for (ri, row) in self.rows.iter().enumerate().skip(self.scroll).take(body_h) {
            let is_cursor_row = self.focused && ri == self.cursor.0;
            if is_cursor_row {
                buf.set_string(area.x, y, "▸", t.focused());
            }
            for (ci, (cell, (cx, cw))) in row.iter().zip(&xs).enumerate() {
                if *cw == 0 {
                    continue;
                }
                let right = self.columns[ci].right;
                let mut text = cell.text.clone();
                let armed = self.armed == Some((ri, ci));
                if armed {
                    text = format!("[{text}]");
                }
                let text = if right {
                    fit_right(&text, *cw as usize)
                } else {
                    fit_left(&text, *cw as usize)
                };
                let style = if armed {
                    t.editing()
                } else if is_cursor_row && ci == self.cursor.1 {
                    t.focused()
                } else if cell.dim {
                    t.label()
                } else if let Some(c) = cell.color {
                    Style::default().fg(c)
                } else {
                    t.value()
                };
                buf.set_string(*cx, y, text, style);
            }
            y += 1;
        }
        // Scroll hints in the right margin.
        let hint_x = area.x + area.width - 1;
        if self.scroll > 0 {
            let hy = area.y + u16::from(self.header);
            buf.set_string(hint_x, hy, "▲", t.label());
        }
        if self.scroll + body_h < self.rows.len() {
            buf.set_string(hint_x, area.y + area.height - 1, "▼", t.label());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::render;
    use super::*;
    use crate::theme::{ColorDepth, Glyphs};

    const COLS: &[Column] = &[
        Column::right("#", 2),
        Column::left("Type", 12),
        Column::right("Freq", 8),
        Column::right("Gain", 8),
    ];

    #[test]
    fn a_table_lines_its_columns_up_and_marks_the_cursor() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let rows = vec![
            vec![
                "1".into(),
                "Peaking".into(),
                "64 Hz".into(),
                "-6.2 dB".into(),
            ],
            vec![
                "2".into(),
                "Low Shelf".into(),
                "105 Hz".into(),
                "+8.8 dB".into(),
            ],
        ];
        let s = render(
            Table::new(COLS, &t)
                .rows(rows)
                .focused(true, (1, 2))
                .armed(Some((1, 2))),
            40,
            3,
        );
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines[0], "   # TYPE             FREQ     GAIN");
        assert_eq!(lines[1], "   1 Peaking         64 Hz  -6.2 dB");
        assert_eq!(lines[2], "▸  2 Low Shelf    [105 Hz]  +8.8 dB");
    }

    #[test]
    fn scrolling_keeps_the_cursor_visible_and_hints_the_overflow() {
        assert_eq!(Table::scroll_to(0, 3, 5), 0);
        assert_eq!(Table::scroll_to(7, 0, 5), 3);
        assert_eq!(Table::scroll_to(4, 2, 5), 2);
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let rows: Vec<Vec<Cell>> = (1..=10)
            .map(|i| vec![i.to_string().into(), "Off".into(), "".into(), "".into()])
            .collect();
        let s = render(Table::new(COLS, &t).rows(rows).scroll(3), 40, 4);
        assert!(s.lines().nth(1).unwrap().ends_with('▲'));
        assert!(s.lines().nth(3).unwrap().ends_with('▼'));
    }
}
