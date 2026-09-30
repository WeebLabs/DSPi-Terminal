//! A boolean row: `● On` in green, `○ Off` dimmed, with a caption.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

use super::Action;
use super::text::{truncate, wrap};
use crate::theme::{Glyphs, Theme};

pub struct ToggleRow<'a> {
    pub label: &'a str,
    pub on: bool,
    pub caption: Option<&'a str>,
    pub theme: &'a Theme,
    pub focused: bool,
    pub enabled: bool,
}

impl<'a> ToggleRow<'a> {
    pub fn new(label: &'a str, on: bool, theme: &'a Theme) -> Self {
        Self {
            label,
            on,
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

    pub fn handle(&self, key: KeyEvent) -> Option<Action> {
        if !self.enabled {
            return None;
        }
        match key.code {
            KeyCode::Char(' ') | KeyCode::Enter => Some(Action::Toggled(!self.on)),
            KeyCode::Left => Some(Action::Toggled(false)),
            KeyCode::Right => Some(Action::Toggled(true)),
            _ => None,
        }
    }

    /// The `● On` / `○ Off` text, shared with header toggles.
    pub fn state_text(on: bool, glyphs: Glyphs) -> &'static str {
        match (on, glyphs) {
            (true, Glyphs::Ascii) => "[x] On",
            (false, Glyphs::Ascii) => "[ ] Off",
            (true, _) => "● On",
            (false, _) => "○ Off",
        }
    }
}

impl Widget for ToggleRow<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 10 {
            return;
        }
        let t = self.theme;
        let state = Self::state_text(self.on, t.glyphs);
        let state_style = if !self.enabled {
            t.label()
        } else if self.on {
            ratatui::style::Style::default().fg(t.ok)
        } else {
            t.label()
        };
        let label_style = if !self.enabled {
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
        let label = truncate(self.label, area.width as usize - state.len() - 3);
        buf.set_string(area.x + 1, area.y, &label, label_style);
        buf.set_string(
            area.x + area.width - state.len() as u16,
            area.y,
            state,
            state_style,
        );
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

#[cfg(test)]
mod tests {
    use super::super::testing::{key, render};
    use super::*;
    use crate::theme::{ColorDepth, Glyphs};

    #[test]
    fn a_toggle_shows_its_state_on_the_right() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let s = render(
            ToggleRow::new("Interaural Time Delay", true, &t)
                .caption("Simulates ~220 us path difference via all-pass filter"),
            60,
            2,
        );
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(
            lines[0],
            " Interaural Time Delay                                ● On"
        );
        assert_eq!(
            lines[1],
            " Simulates ~220 us path difference via all-pass filter"
        );
    }

    #[test]
    fn space_flips_and_arrows_set() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let r = ToggleRow::new("x", false, &t);
        assert_eq!(
            r.handle(key(KeyCode::Char(' '))),
            Some(Action::Toggled(true))
        );
        assert_eq!(r.handle(key(KeyCode::Left)), Some(Action::Toggled(false)));
        assert_eq!(r.enabled(false).handle(key(KeyCode::Enter)), None);
    }
}
