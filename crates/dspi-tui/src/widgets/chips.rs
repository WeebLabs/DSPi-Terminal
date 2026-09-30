//! A row of toggleable chips: `[1] [2] [3]` for output masks, with the
//! test-signal variant's third state for inverted polarity.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;

use super::Action;
use crate::theme::{ColorDepth, Glyphs, Theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChipState {
    Off,
    On,
    /// Selected with inverted polarity (Signal Generator only).
    Inverted,
}

pub struct Chip<'a> {
    pub label: &'a str,
    pub state: ChipState,
    /// The chip's own colour when on; a channel colour, usually.
    pub color: Color,
    /// A disabled output: drawn dim and not toggleable.
    pub enabled: bool,
}

pub struct ChipRow<'a> {
    pub chips: Vec<Chip<'a>>,
    pub theme: &'a Theme,
    pub focused: bool,
    pub cursor: usize,
    /// Whether `Space` cycles through `Inverted` (Signal Generator) or just
    /// flips.
    pub polarity: bool,
}

impl<'a> ChipRow<'a> {
    pub fn new(theme: &'a Theme) -> Self {
        Self {
            chips: Vec::new(),
            theme,
            focused: false,
            cursor: 0,
            polarity: false,
        }
    }
    pub fn chip(mut self, label: &'a str, state: ChipState, color: Color) -> Self {
        self.chips.push(Chip {
            label,
            state,
            color,
            enabled: true,
        });
        self
    }
    pub fn disabled_chip(mut self, label: &'a str, state: ChipState, color: Color) -> Self {
        self.chips.push(Chip {
            label,
            state,
            color,
            enabled: false,
        });
        self
    }
    pub fn focused(mut self, f: bool, cursor: usize) -> Self {
        self.focused = f;
        self.cursor = cursor;
        self
    }
    pub fn polarity(mut self, p: bool) -> Self {
        self.polarity = p;
        self
    }

    pub fn handle(&self, key: KeyEvent) -> Option<Action> {
        let n = self.chips.len();
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
            KeyCode::Char(' ') | KeyCode::Enter => {
                let c = &self.chips[self.cursor];
                if !c.enabled {
                    return None;
                }
                let next = match (c.state, self.polarity) {
                    (ChipState::Off, _) => ChipState::On,
                    (ChipState::On, true) => ChipState::Inverted,
                    (ChipState::On, false) => ChipState::Off,
                    (ChipState::Inverted, _) => ChipState::Off,
                };
                Some(Action::Chip(self.cursor, next))
            }
            KeyCode::Char('p') => Some(Action::Open),
            _ => None,
        }
    }

    /// Columns needed to draw every chip.
    pub fn width(&self) -> u16 {
        self.chips
            .iter()
            .map(|c| c.label.len() as u16 + 3)
            .sum::<u16>()
            .saturating_sub(1)
    }
}

impl Widget for ChipRow<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }
        let t = self.theme;
        let texts: Vec<String> = self
            .chips
            .iter()
            .map(|c| {
                let mark = match (c.state, t.depth, t.glyphs) {
                    (ChipState::Inverted, _, Glyphs::Ascii) => "o",
                    (ChipState::Inverted, _, _) => "ø",
                    (ChipState::On, ColorDepth::Mono, Glyphs::Ascii) => "x",
                    (ChipState::On, ColorDepth::Mono, _) => "■",
                    (ChipState::Off, ColorDepth::Mono, _) => " ",
                    _ => "",
                };
                format!("[{mark}{}]", c.label)
            })
            .collect();
        // When the row overflows, show a window that keeps the cursor chip
        // in view, with `‹` and `›` marking what is hidden either side.
        let widths: Vec<u16> = texts.iter().map(|s| s.chars().count() as u16 + 1).collect();
        let total: u16 = widths.iter().sum();
        let avail = area.width.saturating_sub(2);
        let mut start = 0usize;
        if total > avail && self.focused {
            let mut span: u16 = widths[..=self.cursor.min(widths.len() - 1)].iter().sum();
            while span > avail.saturating_sub(2) && start < self.cursor {
                span -= widths[start];
                start += 1;
            }
        }
        let mut x = area.x + 1;
        if start > 0 {
            buf.set_string(area.x, area.y, "‹", t.label());
        }
        for (i, c) in self.chips.iter().enumerate().skip(start) {
            let text = &texts[i];
            if x + text.chars().count() as u16 > area.x + area.width.saturating_sub(1) {
                buf.set_string(area.x + area.width - 1, area.y, "›", t.label());
                break;
            }
            let base = if !c.enabled {
                t.label()
            } else {
                match c.state {
                    ChipState::Off => Style::default().fg(t.dim),
                    // DESIGN 12.3: a chip is reverse video in `fg`; the
                    // channel hue is the console theme's alone.
                    ChipState::On => t.pill(if t.quiet() { t.fg } else { c.color }),
                    ChipState::Inverted => t.pill(t.warning),
                }
            };
            let style = if self.focused && i == self.cursor {
                base.add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                base
            };
            buf.set_string(x, area.y, text, style);
            x += text.chars().count() as u16 + 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{key, render, render_buf};
    use super::*;
    use crate::theme::Glyphs;

    fn row(t: &Theme) -> ChipRow<'_> {
        ChipRow::new(t)
            .chip("1", ChipState::On, t.outputs[0])
            .chip("2", ChipState::Off, t.outputs[1])
            .chip("3", ChipState::Inverted, t.outputs[2])
            .disabled_chip("4", ChipState::Off, t.outputs[3])
    }

    #[test]
    fn chips_draw_as_bracketed_labels() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        assert_eq!(render(row(&t), 30, 1), " [1] [2] [ø3] [4]");
        let m = Theme::mono(Glyphs::Ascii);
        assert_eq!(render(row(&m), 30, 1), " [x1] [ 2] [o3] [ 4]");
    }

    #[test]
    fn an_overflowing_row_scrolls_to_keep_the_cursor_in_view() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let labels: Vec<String> = (1..=9).map(|n| format!("OUT {n}")).collect();
        let mut row = ChipRow::new(&t);
        for l in &labels {
            row = row.chip(l, ChipState::Off, t.outputs[0]);
        }
        let s = render(row.focused(true, 8), 30, 1);
        assert!(s.starts_with('‹'), "{s}");
        assert!(s.contains("[OUT 9]"), "the cursor chip is shown: {s}");
        let mut row = ChipRow::new(&t);
        for l in &labels {
            row = row.chip(l, ChipState::Off, t.outputs[0]);
        }
        let s = render(row.focused(true, 0), 30, 1);
        assert!(s.starts_with(" [OUT 1]") && s.ends_with('›'), "{s}");
    }

    #[test]
    fn a_selected_chip_is_reverse_video_in_its_colour() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let b = render_buf(row(&t), 30, 1);
        assert!(b[(1, 0)].modifier.contains(Modifier::REVERSED));
        assert_eq!(b[(1, 0)].fg, t.outputs[0]);
        assert!(!b[(5, 0)].modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn space_toggles_and_polarity_adds_a_third_state() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let r = row(&t).focused(true, 0);
        assert_eq!(
            r.handle(key(KeyCode::Char(' '))),
            Some(Action::Chip(0, ChipState::Off))
        );
        let r = row(&t).focused(true, 0).polarity(true);
        assert_eq!(
            r.handle(key(KeyCode::Char(' '))),
            Some(Action::Chip(0, ChipState::Inverted))
        );
        let r = row(&t).focused(true, 3);
        assert_eq!(
            r.handle(key(KeyCode::Char(' '))),
            None,
            "a disabled chip does not toggle"
        );
        assert_eq!(r.handle(key(KeyCode::Right)), Some(Action::Selected(3)));
        assert_eq!(r.handle(key(KeyCode::Char('p'))), Some(Action::Open));
    }
}
