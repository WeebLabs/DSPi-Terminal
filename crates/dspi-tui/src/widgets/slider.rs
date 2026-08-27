//! The horizontal slider every parameter row draws.
//!
//! `━` for the track, `●` for the knob, the filled part in the value colour so
//! the position reads without finding the knob. Reverse video and `#` on a
//! terminal with no colour.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use crate::theme::{Glyphs, Theme};

/// Draw a slider into one row of `area`, with `fraction` in 0..1.
pub fn draw(
    area: Rect,
    buf: &mut Buffer,
    fraction: f64,
    color: Color,
    focused: bool,
    theme: &Theme,
) {
    if area.width < 3 || area.height == 0 {
        return;
    }
    let w = area.width as usize;
    let knob = ((fraction.clamp(0.0, 1.0)) * (w - 1) as f64).round() as usize;
    let (track, fill, knob_ch) = match theme.glyphs {
        Glyphs::Ascii => ("-", "=", "O"),
        _ => ("━", "━", "●"),
    };
    let knob_color = if focused { theme.accent } else { color };
    for i in 0..w {
        let x = area.x + i as u16;
        let cell = &mut buf[(x, area.y)];
        if i == knob {
            let mut st = Style::default().fg(knob_color);
            if theme.depth == crate::theme::ColorDepth::Mono {
                st = st.add_modifier(Modifier::REVERSED);
            }
            cell.set_symbol(knob_ch)
                .set_style(st.add_modifier(Modifier::BOLD));
        } else if i < knob {
            cell.set_symbol(fill).set_style(Style::default().fg(color));
        } else {
            cell.set_symbol(track)
                .set_style(Style::default().fg(theme.chrome));
        }
    }
}

/// A slider with no value: for a disabled row.
pub fn draw_disabled(area: Rect, buf: &mut Buffer, theme: &Theme) {
    if area.height == 0 {
        return;
    }
    let track = if theme.glyphs == Glyphs::Ascii {
        "-"
    } else {
        "━"
    };
    for i in 0..area.width {
        buf[(area.x + i, area.y)]
            .set_symbol(track)
            .set_style(Style::default().fg(theme.chrome_faint));
    }
}
