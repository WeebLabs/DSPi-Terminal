//! The GPIO overview grid from Settings > Overview.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;

use super::Action;
use crate::theme::Theme;

/// The Console's five pin roles, each with a tint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinRole {
    Output,
    Clock,
    Input,
    Control,
    Utility,
}

impl PinRole {
    pub fn name(self) -> &'static str {
        match self {
            Self::Output => "Outputs",
            Self::Clock => "Clocks",
            Self::Input => "Inputs",
            Self::Control => "Control",
            Self::Utility => "Other",
        }
    }

    pub fn color(self, theme: &Theme) -> Color {
        match self {
            Self::Output => theme.accent,
            Self::Clock => theme.danger,
            Self::Input => theme.ok,
            Self::Control => theme.inputs[4],
            Self::Utility => theme.inputs[7],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinCell {
    pub gpio: u8,
    /// The owner label, e.g. `SPDIF 1 out`, when claimed.
    pub owner: Option<(PinRole, String)>,
}

pub struct PinGrid<'a> {
    pub pins: &'a [PinCell],
    pub theme: &'a Theme,
    pub focused: bool,
    pub cursor: usize,
    pub columns: u16,
}

impl<'a> PinGrid<'a> {
    pub fn new(pins: &'a [PinCell], theme: &'a Theme) -> Self {
        Self {
            pins,
            theme,
            focused: false,
            cursor: 0,
            columns: 9,
        }
    }
    pub fn focused(mut self, f: bool, cursor: usize) -> Self {
        self.focused = f;
        self.cursor = cursor;
        self
    }

    pub fn height(&self) -> u16 {
        (self.pins.len() as u16).div_ceil(self.columns)
    }

    pub fn handle(&self, key: KeyEvent) -> Option<Action> {
        let n = self.pins.len();
        if n == 0 {
            return None;
        }
        let c = self.columns as usize;
        match key.code {
            KeyCode::Left => Some(Action::Selected(self.cursor.saturating_sub(1))),
            KeyCode::Right => Some(Action::Selected((self.cursor + 1).min(n - 1))),
            KeyCode::Up => Some(Action::Selected(self.cursor.saturating_sub(c))),
            KeyCode::Down => Some(Action::Selected((self.cursor + c).min(n - 1))),
            _ => None,
        }
    }

    /// The echo-line text for the pin under the cursor.
    pub fn describe(&self) -> String {
        match self.pins.get(self.cursor) {
            Some(PinCell {
                gpio,
                owner: Some((role, label)),
            }) => format!("GP{gpio}: {label} ({})", role.name()),
            Some(PinCell { gpio, owner: None }) => format!("GP{gpio} - available"),
            None => String::new(),
        }
    }
}

impl Widget for PinGrid<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 6 {
            return;
        }
        let t = self.theme;
        let cell_w = 5u16;
        for (i, p) in self.pins.iter().enumerate() {
            let col = (i as u16) % self.columns;
            let row = (i as u16) / self.columns;
            if row >= area.height || (col + 1) * cell_w > area.width {
                continue;
            }
            let x = area.x + col * cell_w;
            let y = area.y + row;
            let text = format!("GP{:<2}", p.gpio);
            let mut style = match &p.owner {
                Some((role, _)) => t.pill(role.color(t)),
                None => Style::default().fg(t.dim),
            };
            if self.focused && i == self.cursor {
                style = style.add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
            }
            buf.set_string(x + 1, y, &text, style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{key, render, render_buf};
    use super::*;
    use crate::theme::{ColorDepth, Glyphs};

    fn pins() -> Vec<PinCell> {
        (0..12)
            .map(|g| PinCell {
                gpio: g,
                owner: if g == 6 {
                    Some((PinRole::Output, "SPDIF 1 out".into()))
                } else {
                    None
                },
            })
            .collect()
    }

    #[test]
    fn the_grid_wraps_at_nine_columns_and_tints_claimed_pins() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let p = pins();
        let s = render(PinGrid::new(&p, &t), 50, 2);
        assert_eq!(
            s.lines().next().unwrap(),
            " GP0  GP1  GP2  GP3  GP4  GP5  GP6  GP7  GP8"
        );
        assert_eq!(s.lines().nth(1).unwrap(), " GP9  GP10 GP11");
        let b = render_buf(PinGrid::new(&p, &t), 50, 2);
        assert!(b[(31, 0)].modifier.contains(Modifier::REVERSED));
        assert_eq!(b[(31, 0)].fg, t.accent);
    }

    #[test]
    fn arrows_move_in_two_dimensions_and_describe_the_pin() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let p = pins();
        let g = PinGrid::new(&p, &t).focused(true, 6);
        assert_eq!(g.handle(key(KeyCode::Down)), Some(Action::Selected(11)));
        assert_eq!(g.describe(), "GP6: SPDIF 1 out (Outputs)");
        assert_eq!(
            PinGrid::new(&p, &t).focused(true, 0).describe(),
            "GP0 - available"
        );
    }
}
