//! Status pills (` Active `) and banners (`▲ Adjust only with audio stopped`).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

use super::text::wrap;
use crate::theme::{Glyphs, Theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusTone {
    /// Green: live, applied, connected.
    Ok,
    /// Orange: inactive, pending, unsaved.
    Warning,
    /// Red: error, muted, clipping.
    Danger,
    /// Grey: disabled, not applicable.
    Neutral,
}

pub struct StatusPill<'a> {
    pub text: &'a str,
    pub tone: StatusTone,
    pub theme: &'a Theme,
}

impl<'a> StatusPill<'a> {
    pub fn new(text: &'a str, tone: StatusTone, theme: &'a Theme) -> Self {
        Self { text, tone, theme }
    }

    pub fn width(&self) -> u16 {
        self.text.len() as u16 + 2
    }

    pub fn style(&self) -> Style {
        let t = self.theme;
        let color = match self.tone {
            StatusTone::Ok => t.ok,
            StatusTone::Warning => t.warning,
            StatusTone::Danger => t.danger,
            StatusTone::Neutral => t.dim,
        };
        t.pill(color)
    }
}

impl Widget for StatusPill<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 3 {
            return;
        }
        let text = format!(" {} ", self.text);
        let text: String = text.chars().take(area.width as usize).collect();
        buf.set_string(area.x, area.y, &text, self.style());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BannerKind {
    Warning,
    Info,
    Error,
}

/// A two-line notice: a glyph and a title, then a wrapped body.
pub struct Banner<'a> {
    pub kind: BannerKind,
    pub title: &'a str,
    pub body: Option<&'a str>,
    pub theme: &'a Theme,
}

impl<'a> Banner<'a> {
    pub fn warning(title: &'a str, theme: &'a Theme) -> Self {
        Self {
            kind: BannerKind::Warning,
            title,
            body: None,
            theme,
        }
    }
    pub fn info(title: &'a str, theme: &'a Theme) -> Self {
        Self {
            kind: BannerKind::Info,
            title,
            body: None,
            theme,
        }
    }
    pub fn error(title: &'a str, theme: &'a Theme) -> Self {
        Self {
            kind: BannerKind::Error,
            title,
            body: None,
            theme,
        }
    }
    pub fn body(mut self, b: &'a str) -> Self {
        self.body = Some(b);
        self
    }

    pub fn height(&self, width: u16) -> u16 {
        1 + self
            .body
            .map(|b| wrap(b, width.saturating_sub(3) as usize, 2).len() as u16)
            .unwrap_or(0)
    }
}

impl Widget for Banner<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 6 {
            return;
        }
        let t = self.theme;
        let (glyph, style) = match (self.kind, t.glyphs) {
            (BannerKind::Warning, Glyphs::Ascii) => ("!", t.warning_style()),
            (BannerKind::Warning, _) => ("▲", t.warning_style()),
            (BannerKind::Info, Glyphs::Ascii) => ("i", t.label()),
            (BannerKind::Info, _) => ("ⓘ", t.label()),
            (BannerKind::Error, Glyphs::Ascii) => ("x", Style::default().fg(t.danger)),
            (BannerKind::Error, _) => ("✖", Style::default().fg(t.danger)),
        };
        buf.set_string(area.x + 1, area.y, glyph, style);
        buf.set_string(
            area.x + 3,
            area.y,
            self.title,
            style.add_modifier(ratatui::style::Modifier::BOLD),
        );
        if let Some(b) = self.body {
            for (i, line) in wrap(b, area.width as usize - 3, (area.height - 1) as usize)
                .iter()
                .enumerate()
            {
                buf.set_string(area.x + 3, area.y + 1 + i as u16, line, t.label());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{render, render_buf};
    use super::*;
    use crate::theme::ColorDepth;
    use ratatui::style::Modifier;

    #[test]
    fn a_pill_is_reverse_video_in_its_tone() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let b = render_buf(StatusPill::new("Active", StatusTone::Ok, &t), 12, 1);
        assert!(b[(0, 0)].modifier.contains(Modifier::REVERSED));
        assert_eq!(b[(0, 0)].fg, t.ok);
        assert_eq!(
            render(StatusPill::new("Active", StatusTone::Ok, &t), 12, 1),
            " Active"
        );
    }

    #[test]
    fn a_banner_is_a_glyph_a_title_and_a_wrapped_body() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let s = render(
            Banner::warning("Adjust only with audio stopped", &t)
                .body("Changing these settings while audio is playing can send a loud pop or full-level transient."),
            50,
            3,
        );
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines[0], " ▲ Adjust only with audio stopped");
        assert!(lines[1].starts_with("   Changing these settings"));
        assert_eq!(lines.len(), 3);
    }
}
