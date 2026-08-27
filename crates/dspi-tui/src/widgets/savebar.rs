//! The Settings save bar: shown only while something is unsaved.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::widgets::Widget;

use super::text::truncate;
use crate::theme::{Glyphs, Theme};

pub struct SaveBar<'a> {
    pub subtitle: &'a str,
    pub busy: bool,
    pub theme: &'a Theme,
    /// 0 = Revert focused, 1 = Save focused, None = neither.
    pub focus: Option<usize>,
    pub save_enabled: bool,
    pub revert_enabled: bool,
}

impl<'a> SaveBar<'a> {
    pub const CS_ONLY: &'static str =
        "Your controls are live now; saving keeps them across a reboot.";
    pub const FLASH: &'static str = "Saving writes these settings to the device's flash.";

    pub fn new(subtitle: &'a str, theme: &'a Theme) -> Self {
        Self {
            subtitle,
            busy: false,
            theme,
            focus: None,
            save_enabled: true,
            revert_enabled: true,
        }
    }
    pub fn busy(mut self, b: bool) -> Self {
        self.busy = b;
        self
    }
    pub fn focus(mut self, f: Option<usize>) -> Self {
        self.focus = f;
        self
    }
    pub fn enabled(mut self, revert: bool, save: bool) -> Self {
        self.revert_enabled = revert;
        self.save_enabled = save;
        self
    }
}

impl Widget for SaveBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 30 {
            return;
        }
        let t = self.theme;
        let glyph = if t.glyphs == Glyphs::Ascii {
            "*"
        } else {
            "●"
        };
        let head = format!("{glyph} Unsaved changes");
        buf.set_string(
            area.x + 1,
            area.y,
            &head,
            t.warning_style().add_modifier(Modifier::BOLD),
        );
        let buttons = ["Revert", "Save"];
        let bw: usize = buttons.iter().map(|b| b.len() + 4).sum();
        let head_w = head.chars().count();
        let sub_w = (area.width as usize).saturating_sub(head_w + bw + 6);
        let sub = if self.busy {
            "Saving..."
        } else {
            self.subtitle
        };
        buf.set_string(
            area.x + 2 + head_w as u16,
            area.y,
            truncate(sub, sub_w),
            t.label(),
        );
        let mut x = area.x + area.width - bw as u16;
        for (i, b) in buttons.iter().enumerate() {
            let enabled = if i == 0 {
                self.revert_enabled
            } else {
                self.save_enabled
            } && !self.busy;
            let text = if i == 1 {
                format!("[{b}]")
            } else {
                format!(" {b} ")
            };
            let style = if !enabled {
                t.label()
            } else if self.focus == Some(i) {
                t.pill(t.accent).add_modifier(Modifier::BOLD)
            } else if i == 1 {
                t.focused()
            } else {
                t.value()
            };
            buf.set_string(x, area.y, &text, style);
            x += text.len() as u16 + 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::render;
    use super::*;
    use crate::theme::ColorDepth;

    #[test]
    fn the_save_bar_reads_left_to_right() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let s = render(SaveBar::new(SaveBar::FLASH, &t), 100, 1);
        assert_eq!(
            s,
            " ● Unsaved changes Saving writes these settings to the device's flash.             Revert   [Save]"
        );
    }
}
