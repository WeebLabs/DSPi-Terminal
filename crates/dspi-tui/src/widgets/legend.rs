//! The legend row of channel pills under the graph, and the sidebar pill.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;

use super::Action;
use crate::theme::{ColorDepth, Glyphs, Theme};

#[derive(Debug, Clone)]
pub struct LegendPill {
    pub descriptor: String,
    pub color: Color,
    pub visible: bool,
    /// This channel's curve is identical to an earlier one's; marked `=`.
    pub grouped: bool,
}

/// One pill: `● IN1` in colour when visible, `○ IN1` dim when hidden.
pub fn pill_text(p: &LegendPill, glyphs: Glyphs) -> String {
    let dot = match (p.visible, glyphs) {
        (true, Glyphs::Ascii) => "*",
        (false, Glyphs::Ascii) => "o",
        (true, _) => "●",
        (false, _) => "○",
    };
    let eq = if p.grouped { "=" } else { "" };
    format!("{dot} {}{eq}", p.descriptor)
}

pub fn pill_style(p: &LegendPill, theme: &Theme) -> Style {
    if !p.visible {
        theme.label()
    } else if theme.depth == ColorDepth::Mono {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(p.color)
    }
}

pub struct LegendRow<'a> {
    pub pills: &'a [LegendPill],
    pub theme: &'a Theme,
    pub focused: bool,
    pub cursor: usize,
    /// Trailing item, e.g. `φ phase` when the overlay is on.
    pub trailing: Option<&'a str>,
}

impl<'a> LegendRow<'a> {
    pub fn new(pills: &'a [LegendPill], theme: &'a Theme) -> Self {
        Self {
            pills,
            theme,
            focused: false,
            cursor: 0,
            trailing: None,
        }
    }
    pub fn focused(mut self, f: bool, cursor: usize) -> Self {
        self.focused = f;
        self.cursor = cursor;
        self
    }
    pub fn trailing(mut self, t: &'a str) -> Self {
        self.trailing = Some(t);
        self
    }

    /// Rows needed to show every pill at `width`: one or two.
    pub fn rows_needed(pills: &[LegendPill], width: u16, glyphs: Glyphs) -> u16 {
        let total: u16 = pills
            .iter()
            .map(|p| pill_text(p, glyphs).chars().count() as u16 + 2)
            .sum::<u16>()
            + 1;
        if total > width { 2 } else { 1 }
    }

    pub fn handle(&self, key: KeyEvent) -> Option<Action> {
        let n = self.pills.len();
        if n == 0 {
            return None;
        }
        match key.code {
            KeyCode::Left | KeyCode::Char('h') => {
                Some(Action::Selected(self.cursor.saturating_sub(1)))
            }
            KeyCode::Right | KeyCode::Char('l') => {
                Some(Action::Selected((self.cursor + 1).min(n - 1)))
            }
            KeyCode::Char(' ') | KeyCode::Enter => Some(Action::Chip(
                self.cursor,
                if self.pills[self.cursor].visible {
                    super::ChipState::Off
                } else {
                    super::ChipState::On
                },
            )),
            _ => None,
        }
    }
}

impl Widget for LegendRow<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }
        let t = self.theme;
        let mut x = area.x + 1;
        let mut y = area.y;
        for (i, p) in self.pills.iter().enumerate() {
            let text = pill_text(p, t.glyphs);
            let w = text.chars().count() as u16;
            if x + w > area.x + area.width {
                // Wrap onto the next row when there is one.
                if y + 1 < area.y + area.height {
                    y += 1;
                    x = area.x + 1;
                } else {
                    break;
                }
            }
            let mut style = pill_style(p, t);
            if self.focused && i == self.cursor {
                style = style.add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
            }
            buf.set_string(x, y, &text, style);
            x += w + 2;
        }
        if let Some(tr) = self.trailing {
            let w = tr.chars().count() as u16;
            if area.x + area.width > x + w {
                buf.set_string(area.x + area.width - w - 1, area.y, tr, t.label());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{key, render};
    use super::*;

    fn pills(t: &Theme) -> Vec<LegendPill> {
        vec![
            LegendPill {
                descriptor: "IN1".into(),
                color: t.inputs[0],
                visible: true,
                grouped: false,
            },
            LegendPill {
                descriptor: "IN2".into(),
                color: t.inputs[1],
                visible: true,
                grouped: true,
            },
            LegendPill {
                descriptor: "OUT1".into(),
                color: t.outputs[0],
                visible: false,
                grouped: false,
            },
        ]
    }

    #[test]
    fn pills_show_visibility_and_grouping() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let p = pills(&t);
        let s = render(LegendRow::new(&p, &t).trailing("φ phase"), 40, 1);
        assert_eq!(s, " ● IN1  ● IN2=  ○ OUT1          φ phase");
    }

    #[test]
    fn space_toggles_the_pill_under_the_cursor() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let p = pills(&t);
        let r = LegendRow::new(&p, &t).focused(true, 2);
        assert_eq!(
            r.handle(key(KeyCode::Char(' '))),
            Some(Action::Chip(2, super::super::ChipState::On))
        );
        assert_eq!(r.handle(key(KeyCode::Right)), Some(Action::Selected(2)));
    }
}
