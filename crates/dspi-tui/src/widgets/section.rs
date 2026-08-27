//! The Console's uppercase section label, with an optional action on the right.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

use super::text::{fit_right, truncate};
use crate::theme::Theme;

pub struct SectionHeader<'a> {
    pub title: &'a str,
    /// Drawn right-aligned, e.g. `Presets ▾`.
    pub action: Option<&'a str>,
    pub theme: &'a Theme,
    /// Whether the action is the focused element.
    pub focused: bool,
}

impl<'a> SectionHeader<'a> {
    pub fn new(title: &'a str, theme: &'a Theme) -> Self {
        Self {
            title,
            action: None,
            theme,
            focused: false,
        }
    }

    pub fn action(mut self, a: &'a str) -> Self {
        self.action = Some(a);
        self
    }

    pub fn focused(mut self, f: bool) -> Self {
        self.focused = f;
        self
    }
}

impl Widget for SectionHeader<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 2 {
            return;
        }
        let title = truncate(&self.title.to_uppercase(), area.width as usize - 1);
        buf.set_string(area.x + 1, area.y, &title, self.theme.section());
        if let Some(a) = self.action {
            let w = (area.width as usize).saturating_sub(title.len() + 3);
            if w >= 3 {
                let s = fit_right(a, w.min(a.len()));
                let x = area.x + area.width - s.len() as u16 - 1;
                let style = if self.focused {
                    self.theme.focused()
                } else {
                    self.theme.label()
                };
                buf.set_string(x, area.y, &s, style);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::render;
    use super::*;
    use crate::theme::{ColorDepth, Glyphs};

    #[test]
    fn a_header_is_uppercase_with_its_action_on_the_right() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let s = render(
            SectionHeader::new("Output pairs", &t).action("Presets ▾"),
            40,
            1,
        );
        assert_eq!(s, " OUTPUT PAIRS               Presets ▾");
    }
}
