//! Choice rows: `‹ Medium ›` with a popup list, and the inline segmented
//! control `Slow │ Medium │ Fast`.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Clear, Widget};

use super::Action;
use super::text::{fit_left, truncate, wrap};
use crate::theme::Theme;

pub struct PickerRow<'a> {
    pub label: &'a str,
    pub choices: &'a [&'a str],
    pub selected: usize,
    pub caption: Option<&'a str>,
    pub theme: &'a Theme,
    pub focused: bool,
    pub enabled: bool,
}

impl<'a> PickerRow<'a> {
    pub fn new(label: &'a str, choices: &'a [&'a str], selected: usize, theme: &'a Theme) -> Self {
        Self {
            label,
            choices,
            selected,
            caption: None,
            theme,
            focused: false,
            enabled: true,
        }
    }
    pub fn caption(mut self, c: &'a str) -> Self {
        self.caption = Some(c);
        self
    }
    pub fn focused(mut self, f: bool) -> Self {
        self.focused = f;
        self
    }
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }

    pub fn height(&self, width: u16) -> u16 {
        1 + self
            .caption
            .map(|c| wrap(c, width.saturating_sub(2) as usize, 2).len() as u16)
            .unwrap_or(0)
    }

    /// `←`/`→` cycle without wrapping; `Enter` opens the list.
    pub fn handle(&self, key: KeyEvent) -> Option<Action> {
        if !self.enabled || self.choices.is_empty() {
            return None;
        }
        match key.code {
            KeyCode::Left | KeyCode::Char('h') => {
                Some(Action::Selected(self.selected.saturating_sub(1)))
            }
            KeyCode::Right | KeyCode::Char('l') => Some(Action::Selected(
                (self.selected + 1).min(self.choices.len() - 1),
            )),
            KeyCode::Enter | KeyCode::Char(' ') => Some(Action::Open),
            _ => None,
        }
    }
}

impl Widget for PickerRow<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 12 {
            return;
        }
        let t = self.theme;
        let current = self.choices.get(self.selected).copied().unwrap_or("");
        let value = format!("‹ {current} ›");
        let value_style = if !self.enabled {
            t.label()
        } else if self.focused {
            t.focused()
        } else {
            t.value()
        };
        buf.set_string(
            area.x,
            area.y,
            if self.focused { "▸" } else { " " },
            t.focused(),
        );
        let label = truncate(
            self.label,
            (area.width as usize).saturating_sub(value.len() + 3),
        );
        buf.set_string(
            area.x + 1,
            area.y,
            &label,
            if self.enabled {
                if self.focused { t.focused() } else { t.value() }
            } else {
                t.label()
            },
        );
        let vx = area.x + area.width - value.chars().count() as u16;
        buf.set_string(vx, area.y, &value, value_style);
        if let Some(c) = self.caption
            && area.height >= 2
        {
            for (i, line) in wrap(c, area.width as usize - 2, (area.height - 1) as usize)
                .iter()
                .enumerate()
            {
                buf.set_string(area.x + 1, area.y + 1 + i as u16, line, t.label());
            }
        }
    }
}

/// The inline segmented control. Every choice is visible; the selected one is
/// reverse video.
pub struct Segmented<'a> {
    pub label: &'a str,
    pub choices: &'a [&'a str],
    pub selected: usize,
    pub theme: &'a Theme,
    pub focused: bool,
    pub enabled: bool,
}

impl<'a> Segmented<'a> {
    pub fn new(label: &'a str, choices: &'a [&'a str], selected: usize, theme: &'a Theme) -> Self {
        Self {
            label,
            choices,
            selected,
            theme,
            focused: false,
            enabled: true,
        }
    }
    pub fn focused(mut self, f: bool) -> Self {
        self.focused = f;
        self
    }
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }
    pub fn handle(&self, key: KeyEvent) -> Option<Action> {
        PickerRow::new(self.label, self.choices, self.selected, self.theme)
            .enabled(self.enabled)
            .handle(key)
            .filter(|a| !matches!(a, Action::Open))
    }
}

impl Widget for Segmented<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 12 {
            return;
        }
        let t = self.theme;
        buf.set_string(
            area.x,
            area.y,
            if self.focused { "▸" } else { " " },
            t.focused(),
        );
        let total: usize = self.choices.iter().map(|c| c.len() + 2).sum::<usize>()
            + self.choices.len().saturating_sub(1);
        let label_w = (area.width as usize).saturating_sub(total + 3);
        buf.set_string(
            area.x + 1,
            area.y,
            fit_left(self.label, label_w),
            if !self.enabled {
                t.label()
            } else if self.focused {
                t.focused()
            } else {
                t.value()
            },
        );
        let mut x = area.x + area.width - total as u16;
        for (i, c) in self.choices.iter().enumerate() {
            let text = format!(" {c} ");
            let style = if i == self.selected && self.enabled {
                let color = if self.focused { t.accent } else { t.fg };
                t.pill(color)
            } else if self.enabled {
                t.value()
            } else {
                t.label()
            };
            buf.set_string(x, area.y, &text, style);
            x += text.len() as u16;
            if i + 1 < self.choices.len() {
                buf.set_string(x, area.y, "│", t.chrome_style());
                x += 1;
            }
        }
    }
}

/// A popup list, drawn over whatever is beneath it. The screen owns the
/// `PopupList` while it is open and forwards keys.
#[derive(Debug, Clone)]
pub struct PopupList {
    pub title: String,
    /// Items; an item starting with `#` is a group header and cannot be
    /// selected (the hierarchical filter-type menu flattens this way).
    pub items: Vec<String>,
    pub cursor: usize,
    pub scroll: usize,
    /// Optional search text; typing filters.
    pub filter: Option<String>,
}

impl PopupList {
    pub fn new(title: impl Into<String>, items: Vec<String>, selected: usize) -> Self {
        let mut p = Self {
            title: title.into(),
            items,
            cursor: selected,
            scroll: 0,
            filter: None,
        };
        p.skip_headers(1);
        p
    }

    pub fn searchable(mut self) -> Self {
        self.filter = Some(String::new());
        self
    }

    fn is_header(&self, i: usize) -> bool {
        self.items.get(i).is_some_and(|s| s.starts_with('#'))
    }

    fn visible(&self) -> Vec<usize> {
        match &self.filter {
            Some(f) if !f.is_empty() => {
                let f = f.to_lowercase();
                (0..self.items.len())
                    .filter(|&i| !self.is_header(i) && self.items[i].to_lowercase().contains(&f))
                    .collect()
            }
            _ => (0..self.items.len()).collect(),
        }
    }

    fn skip_headers(&mut self, dir: i32) {
        let n = self.items.len();
        if n == 0 {
            return;
        }
        let mut guard = 0;
        while self.is_header(self.cursor) && guard < n {
            self.cursor = if dir > 0 {
                (self.cursor + 1) % n
            } else {
                (self.cursor + n - 1) % n
            };
            guard += 1;
        }
    }

    fn step(&mut self, dir: i32) {
        let vis = self.visible();
        if vis.is_empty() {
            return;
        }
        let pos = vis.iter().position(|&i| i == self.cursor).unwrap_or(0);
        let next = if dir > 0 {
            (pos + 1).min(vis.len() - 1)
        } else {
            pos.saturating_sub(1)
        };
        self.cursor = vis[next];
        if self.is_header(self.cursor) {
            // Step past a header in the same direction; if we hit the end,
            // go back the other way.
            let before = self.cursor;
            self.skip_headers(dir);
            if self.is_header(self.cursor) || (dir > 0 && self.cursor < before) {
                self.cursor = before;
                self.skip_headers(-dir);
            }
        }
    }

    pub fn handle(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Up | KeyCode::Char('k')
                if self
                    .filter
                    .as_deref()
                    .is_none_or(|_| key.code == KeyCode::Up) =>
            {
                self.step(-1);
                None
            }
            KeyCode::Down | KeyCode::Char('j')
                if self
                    .filter
                    .as_deref()
                    .is_none_or(|_| key.code == KeyCode::Down) =>
            {
                self.step(1);
                None
            }
            KeyCode::Home => {
                self.cursor = 0;
                self.skip_headers(1);
                None
            }
            KeyCode::End => {
                self.cursor = self.items.len().saturating_sub(1);
                self.skip_headers(-1);
                None
            }
            KeyCode::Enter => {
                if self.items.is_empty() || self.is_header(self.cursor) {
                    None
                } else {
                    Some(Action::Selected(self.cursor))
                }
            }
            KeyCode::Esc => Some(Action::Closed),
            KeyCode::Backspace => {
                if let Some(f) = &mut self.filter {
                    f.pop();
                    self.reset_cursor();
                }
                None
            }
            KeyCode::Char(c) => {
                if let Some(f) = &mut self.filter {
                    f.push(c);
                    self.reset_cursor();
                }
                None
            }
            _ => None,
        }
    }

    fn reset_cursor(&mut self) {
        let vis = self.visible();
        if !vis.contains(&self.cursor) {
            self.cursor = vis.first().copied().unwrap_or(0);
        }
    }

    /// The size the popup wants, capped to what is available.
    pub fn size(&self, max_w: u16, max_h: u16) -> (u16, u16) {
        let widest = self
            .items
            .iter()
            .map(|s| s.trim_start_matches('#').len())
            .chain(std::iter::once(self.title.len()))
            .max()
            .unwrap_or(10);
        let w = (widest as u16 + 6).min(max_w).max(12);
        let rows = self.visible().len() as u16 + 2 + u16::from(self.filter.is_some());
        (w, rows.min(max_h).max(3))
    }
}

impl Widget for &PopupList {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // The theme is not carried by the popup; it draws in the terminal's
        // default with reverse video for the cursor, which reads in every
        // depth. Screens wrap it in a themed block via `PopupList::draw`.
        Clear.render(area, buf);
        let inner = Rect::new(
            area.x + 1,
            area.y + 1,
            area.width.saturating_sub(2),
            area.height.saturating_sub(2),
        );
        let vis = self.visible();
        let mut y = inner.y;
        if let Some(f) = &self.filter {
            buf.set_string(
                inner.x,
                y,
                fit_left(&format!("/{f}"), inner.width as usize),
                Style::default(),
            );
            y += 1;
        }
        let rows = (inner.y + inner.height).saturating_sub(y) as usize;
        let pos = vis.iter().position(|&i| i == self.cursor).unwrap_or(0);
        let start = pos
            .saturating_sub(rows.saturating_sub(1))
            .min(self.scroll.max(pos.saturating_sub(rows - 1)));
        let start = if pos < start { pos } else { start };
        for (row, &i) in vis.iter().enumerate().skip(start).take(rows) {
            let _ = row;
            let text = &self.items[i];
            let (text, style) = if let Some(h) = text.strip_prefix('#') {
                (h.to_string(), Style::default().add_modifier(Modifier::DIM))
            } else if i == self.cursor {
                (
                    format!("▸ {text}"),
                    Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
                )
            } else {
                (format!("  {text}"), Style::default())
            };
            buf.set_string(inner.x, y, fit_left(&text, inner.width as usize), style);
            y += 1;
        }
    }
}

impl PopupList {
    /// Draw the popup with the theme's border and title at `area`.
    pub fn draw(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        use ratatui::widgets::{Block, BorderType, Borders};
        Clear.render(area, buf);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme.accent))
            .title(format!(" {} ", self.title))
            .title_style(theme.title());
        block.render(area, buf);
        // Reuse the inner renderer, which expects to draw its own border
        // area; give it the same rect so the inner offsets line up.
        let inner = Rect::new(
            area.x + 1,
            area.y + 1,
            area.width.saturating_sub(2),
            area.height.saturating_sub(2),
        );
        let vis = self.visible();
        let mut y = inner.y;
        if let Some(f) = &self.filter {
            buf.set_string(
                inner.x,
                y,
                fit_left(&format!("/{f}"), inner.width as usize),
                theme.value(),
            );
            y += 1;
        }
        let rows = (inner.y + inner.height).saturating_sub(y) as usize;
        if rows == 0 {
            return;
        }
        let pos = vis.iter().position(|&i| i == self.cursor).unwrap_or(0);
        let start = pos.saturating_sub(rows - 1);
        for &i in vis.iter().skip(start).take(rows) {
            let text = &self.items[i];
            let (text, style) = if let Some(h) = text.strip_prefix('#') {
                (h.to_uppercase(), theme.section())
            } else if i == self.cursor {
                (format!("▸ {text}"), theme.pill(theme.accent))
            } else {
                (format!("  {text}"), theme.value())
            };
            buf.set_string(inner.x, y, fit_left(&text, inner.width as usize), style);
            y += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{key, render};
    use super::*;
    use crate::theme::{ColorDepth, Glyphs};

    const SPEEDS: &[&str] = &["Slow", "Medium", "Fast"];

    #[test]
    fn a_picker_shows_the_current_choice_in_chevrons() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let s = render(PickerRow::new("Speed", SPEEDS, 1, &t), 40, 1);
        assert_eq!(s, " Speed                        ‹ Medium ›");
    }

    #[test]
    fn arrows_cycle_without_wrapping_and_enter_opens() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let p = PickerRow::new("Speed", SPEEDS, 2, &t);
        assert_eq!(p.handle(key(KeyCode::Right)), Some(Action::Selected(2)));
        assert_eq!(p.handle(key(KeyCode::Left)), Some(Action::Selected(1)));
        assert_eq!(p.handle(key(KeyCode::Enter)), Some(Action::Open));
    }

    #[test]
    fn a_segmented_control_shows_every_choice() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let s = render(Segmented::new("Speed", SPEEDS, 1, &t), 40, 1);
        assert_eq!(s, " Speed             Slow │ Medium │ Fast");
    }

    #[test]
    fn a_popup_skips_group_headers_and_filters() {
        let items: Vec<String> = ["#Shelves", "Low Shelf", "High Shelf", "#Passes", "Low Pass"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut p = PopupList::new("Type", items.clone(), 0);
        assert_eq!(p.cursor, 1, "the cursor lands on the first real item");
        p.handle(key(KeyCode::Down));
        assert_eq!(p.cursor, 2);
        p.handle(key(KeyCode::Down));
        assert_eq!(p.cursor, 4, "stepped over the header");
        p.handle(key(KeyCode::Down));
        assert_eq!(p.cursor, 4, "no wrap at the end");
        assert_eq!(p.handle(key(KeyCode::Enter)), Some(Action::Selected(4)));
        assert_eq!(p.handle(key(KeyCode::Esc)), Some(Action::Closed));

        let mut p = PopupList::new("Type", items, 0).searchable();
        p.handle(key(KeyCode::Char('h')));
        p.handle(key(KeyCode::Char('i')));
        assert_eq!(p.visible(), vec![2]);
        assert_eq!(p.cursor, 2);
    }

    #[test]
    fn a_popup_draws_with_the_cursor_marked() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let p = PopupList::new("Speed", SPEEDS.iter().map(|s| s.to_string()).collect(), 1);
        let (w, h) = p.size(40, 10);
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        term.draw(|f| p.draw(Rect::new(0, 0, w, h), f.buffer_mut(), &t))
            .unwrap();
        let buf = term.backend().buffer();
        let row = |y: u16| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert!(row(0).contains(" Speed "));
        assert!(row(2).contains("▸ Medium"), "{}", row(2));
    }
}
