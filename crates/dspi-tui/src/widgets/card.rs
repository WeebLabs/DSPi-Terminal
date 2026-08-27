//! A bordered card with a channel-coloured title, and the stereo variant whose
//! top border is split between two channels' colours (the terminal's version
//! of the Console's gradient stroke).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Widget;

use super::text::truncate;
use crate::theme::{Glyphs, Theme};

pub struct CardTitle<'a> {
    pub name: &'a str,
    pub color: Color,
    /// Right-aligned detail, e.g. `Delay: 0 ms`.
    pub detail: Option<&'a str>,
}

pub struct Card<'a> {
    pub left: CardTitle<'a>,
    /// A second title makes this a stereo card with a divider down the middle.
    pub right: Option<CardTitle<'a>>,
    pub theme: &'a Theme,
    pub focused: bool,
}

impl<'a> Card<'a> {
    pub fn single(name: &'a str, color: Color, theme: &'a Theme) -> Self {
        Self {
            left: CardTitle {
                name,
                color,
                detail: None,
            },
            right: None,
            theme,
            focused: false,
        }
    }
    pub fn stereo(left: CardTitle<'a>, right: CardTitle<'a>, theme: &'a Theme) -> Self {
        Self {
            left,
            right: Some(right),
            theme,
            focused: false,
        }
    }
    pub fn detail(mut self, d: &'a str) -> Self {
        self.left.detail = Some(d);
        self
    }
    pub fn focused(mut self, f: bool) -> Self {
        self.focused = f;
        self
    }

    /// The inner areas: one for a single card, two for a stereo card.
    pub fn inner(&self, area: Rect) -> Vec<Rect> {
        let inner = Rect::new(
            area.x + 1,
            area.y + 1,
            area.width.saturating_sub(2),
            area.height.saturating_sub(2),
        );
        match self.right {
            None => vec![inner],
            Some(_) => {
                let half = inner.width / 2;
                vec![
                    Rect::new(inner.x, inner.y, half.saturating_sub(1), inner.height),
                    Rect::new(
                        inner.x + half + 1,
                        inner.y,
                        inner.width - half - 1,
                        inner.height,
                    ),
                ]
            }
        }
    }

    fn glyphs(
        &self,
    ) -> (
        &'static str,
        &'static str,
        [&'static str; 4],
        &'static str,
        &'static str,
    ) {
        match self.theme.glyphs {
            Glyphs::Ascii => ("-", "|", ["+", "+", "+", "+"], "+", "+"),
            _ => ("─", "│", ["╭", "╮", "╰", "╯"], "┬", "┴"),
        }
    }

    fn title_text(t: &CardTitle, width: usize, theme: &Theme) -> (String, String) {
        let dot = if theme.glyphs == Glyphs::Ascii {
            "*"
        } else {
            "●"
        };
        let detail = t.detail.unwrap_or("");
        let name_w = width.saturating_sub(detail.len() + 5);
        (
            format!(" {dot} {} ", truncate(t.name, name_w)),
            detail.to_string(),
        )
    }
}

impl Widget for Card<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 6 || area.height < 2 {
            return;
        }
        let t = self.theme;
        let (h, v, corners, tee_top, tee_bottom) = self.glyphs();
        let border = if self.focused {
            Style::default().fg(t.accent)
        } else {
            t.chrome_style()
        };
        let (x0, y0, x1, y1) = (
            area.x,
            area.y,
            area.x + area.width - 1,
            area.y + area.height - 1,
        );

        // Sides and bottom in chrome.
        for y in y0 + 1..y1 {
            buf[(x0, y)].set_symbol(v).set_style(border);
            buf[(x1, y)].set_symbol(v).set_style(border);
        }
        for x in x0 + 1..x1 {
            buf[(x, y1)].set_symbol(h).set_style(border);
        }
        buf[(x0, y1)].set_symbol(corners[2]).set_style(border);
        buf[(x1, y1)].set_symbol(corners[3]).set_style(border);

        // The top border carries the channel colour: the whole width for a
        // single card, split at the divider for a stereo card.
        let split = self.right.as_ref().map(|_| x0 + 1 + (area.width - 2) / 2);
        for x in x0..=x1 {
            let color = match split {
                Some(s) if x > s => self.right.as_ref().map(|r| r.color).unwrap_or(t.chrome),
                _ => self.left.color,
            };
            let sym = if x == x0 {
                corners[0]
            } else if x == x1 {
                corners[1]
            } else {
                h
            };
            buf[(x, y0)]
                .set_symbol(sym)
                .set_style(Style::default().fg(color));
        }
        if let Some(s) = split {
            buf[(s, y0)].set_symbol(tee_top).set_style(border);
            buf[(s, y1)].set_symbol(tee_bottom).set_style(border);
            for y in y0 + 1..y1 {
                buf[(s, y)].set_symbol(v).set_style(border);
            }
        }

        // Titles on the top border.
        let panels: Vec<(Rect, &CardTitle)> = match &self.right {
            None => vec![(Rect::new(x0 + 1, y0, area.width - 2, 1), &self.left)],
            Some(r) => {
                let s = split.unwrap();
                vec![
                    (Rect::new(x0 + 1, y0, s - x0 - 1, 1), &self.left),
                    (Rect::new(s + 1, y0, x1 - s - 1, 1), r),
                ]
            }
        };
        for (r, title) in panels {
            let (name, detail) = Self::title_text(title, r.width as usize, t);
            buf.set_string(
                r.x,
                r.y,
                &name,
                Style::default()
                    .fg(title.color)
                    .add_modifier(ratatui::style::Modifier::BOLD),
            );
            if !detail.is_empty() && r.width as usize > name.chars().count() + detail.len() + 2 {
                buf.set_string(
                    r.x + r.width - detail.len() as u16 - 2,
                    r.y,
                    format!(" {detail} "),
                    t.label(),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{render, render_buf};
    use super::*;
    use crate::theme::ColorDepth;

    #[test]
    fn a_stereo_card_splits_its_top_border_between_two_colours() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let card = Card::stereo(
            CardTitle {
                name: "FL",
                color: t.inputs[0],
                detail: Some("Delay: 0 ms"),
            },
            CardTitle {
                name: "FR",
                color: t.inputs[1],
                detail: None,
            },
            &t,
        );
        let inner = card.inner(Rect::new(0, 0, 60, 4));
        assert_eq!(inner.len(), 2);
        let b = render_buf(card, 60, 4);
        assert_eq!(b[(1, 0)].fg, t.inputs[0]);
        assert_eq!(b[(58, 0)].fg, t.inputs[1]);
        let s = render(
            Card::stereo(
                CardTitle {
                    name: "FL",
                    color: t.inputs[0],
                    detail: Some("Delay: 0 ms"),
                },
                CardTitle {
                    name: "FR",
                    color: t.inputs[1],
                    detail: None,
                },
                &t,
            ),
            60,
            3,
        );
        let top = s.lines().next().unwrap();
        assert_eq!(
            top,
            "╭ ● FL ────────── Delay: 0 ms ┬ ● FR ──────────────────────╮"
        );
        assert!(top.starts_with("╭ ● FL "), "{top}");
        assert!(top.contains("┬ ● FR "), "{top}");
        assert!(top.contains(" Delay: 0 ms "), "{top}");
    }

    #[test]
    fn a_single_card_has_one_inner_area() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let c = Card::single("Sub", t.sub, &t).detail("Delay: 2.5 ms");
        assert_eq!(
            c.inner(Rect::new(0, 0, 30, 5)),
            vec![Rect::new(1, 1, 28, 3)]
        );
        let s = render(c, 30, 3);
        assert_eq!(s.lines().next().unwrap(), "╭ ● Sub ────── Delay: 2.5 ms ╮");
    }
}
