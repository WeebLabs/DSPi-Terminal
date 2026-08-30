//! The channel sidebar and its footer block, after the Console's.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Widget;

use super::model::{ChannelItem, Selection, ShellModel, VolumeMode};
use crate::theme::{ColorDepth, Glyphs, Theme};
use crate::widgets::text::{fit_left, fit_right, truncate};
use crate::widgets::{LevelMeter, slider};

/// Which footer row has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FooterRow {
    Strip,
    Preset,
    Source,
    Volume,
}

impl FooterRow {
    pub fn next(self, has_source: bool) -> Self {
        match self {
            Self::Strip => Self::Preset,
            Self::Preset if has_source => Self::Source,
            Self::Preset | Self::Source => Self::Volume,
            Self::Volume => Self::Volume,
        }
    }
    pub fn prev(self, has_source: bool) -> Self {
        match self {
            Self::Strip | Self::Preset => Self::Strip,
            Self::Source => Self::Preset,
            Self::Volume if has_source => Self::Source,
            Self::Volume => Self::Preset,
        }
    }
}

/// The Console's user-volume taper: square-root power over [-60, 0] dB.
pub fn user_fraction(db: f64) -> f64 {
    let lin = ((db.clamp(-60.0, 0.0) + 60.0) / 60.0).clamp(0.0, 1.0);
    lin.sqrt()
}

/// The Console's master-volume taper: 0.1 dB steps to -10, 0.5 to -40, 1.0
/// to -128, over 248 units.
pub fn master_fraction(db: f64) -> f64 {
    let db = db.clamp(-128.0, 0.0);
    let units = if db >= -10.0 {
        (0.0 - db) / 0.1
    } else if db >= -40.0 {
        100.0 + (-10.0 - db) / 0.5
    } else {
        160.0 + (-40.0 - db)
    };
    1.0 - units / 248.0
}

/// One sidebar row: `▍name  ▓▓▓▓░░▏ IN1`.
fn draw_row(
    area: Rect,
    buf: &mut Buffer,
    item: &ChannelItem,
    selected: bool,
    cursor: bool,
    theme: &Theme,
) {
    let t = theme;
    let pill_w = item.descriptor.len() as u16;
    let meter_w = if area.width >= 22 { 8 } else { 6 };
    // bar, swatch, name, space, meter + clip, space, descriptor.
    let name_w = area
        .width
        .saturating_sub(1 + 1 + 1 + meter_w + 1 + 1 + pill_w) as usize;
    let hue = t.hue_for(item.role, selected);
    // Selection bar.
    let bar = if selected {
        if t.glyphs == Glyphs::Ascii {
            ">"
        } else {
            "▍"
        }
    } else {
        " "
    };
    buf.set_string(area.x, area.y, bar, Style::default().fg(t.accent));
    // The swatch is the one place a channel's hue always shows.
    let swatch = if t.glyphs == Glyphs::Ascii {
        "*"
    } else {
        "▪"
    };
    buf.set_string(area.x + 1, area.y, swatch, Style::default().fg(item.color));
    let name_style = if item.inactive {
        t.label()
    } else if cursor {
        t.focused()
    } else {
        Style::default().fg(hue)
    };
    buf.set_string(area.x + 2, area.y, fit_left(&item.name, name_w), name_style);
    let mx = area.x + 3 + name_w as u16;
    LevelMeter::new(item.level, hue, t)
        .peak(item.peak)
        .clipped(item.clipped)
        .inactive(item.inactive)
        .render(Rect::new(mx, area.y, meter_w + 1, 1), buf);
    let px = mx + meter_w + 2;
    let pill_style = if t.quiet() || !item.visible {
        t.label()
    } else if t.depth == ColorDepth::Mono {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        t.pill(item.color)
    };
    buf.set_string(px, area.y, &item.descriptor, pill_style);
}

/// One row of the flattened list.
enum Row<'a> {
    Header(&'static str),
    Blank,
    Channel {
        row: usize,
        item: &'a ChannelItem,
        selected: bool,
    },
}

fn rows(model: &ShellModel) -> Vec<Row<'_>> {
    let mut out = vec![Row::Header("INPUTS")];
    for (i, it) in model.inputs.iter().enumerate() {
        let selected = model.selection == Selection::Input(i)
            || model.linked_pairs.iter().any(|(a, b)| {
                (model.selection == Selection::Input(*a) && b == &i)
                    || (model.selection == Selection::Input(*b) && a == &i)
            });
        out.push(Row::Channel {
            row: i,
            item: it,
            selected,
        });
    }
    out.push(Row::Blank);
    out.push(Row::Header("OUTPUTS"));
    for (o, it) in model.outputs.iter().enumerate() {
        out.push(Row::Channel {
            row: model.inputs.len() + o,
            item: it,
            selected: model.selection == Selection::Output(o),
        });
    }
    out
}

/// The scroll offset (in flattened rows) that keeps the channel at `cursor`
/// visible in `height` rows, given the current offset.
pub fn scroll_for(model: &ShellModel, cursor: usize, height: u16, current: usize) -> usize {
    let rows = rows(model);
    let Some(pos) = rows
        .iter()
        .position(|r| matches!(r, Row::Channel { row, .. } if *row == cursor))
    else {
        return 0;
    };
    let h = height as usize;
    if h == 0 {
        return 0;
    }
    // Keep the header above the first input in view when the cursor is at
    // the top, and never scroll past the end.
    let max = rows.len().saturating_sub(h);
    let s = if pos < current {
        pos.saturating_sub(1)
    } else if pos >= current + h {
        pos + 1 - h
    } else {
        current
    };
    s.min(max)
}

/// Draw the INPUTS / OUTPUTS list. `cursor` is the focused channel row when
/// the list has focus; `scroll` is the first flattened row shown.
pub fn draw_list(
    area: Rect,
    buf: &mut Buffer,
    model: &ShellModel,
    theme: &Theme,
    focused: bool,
    cursor: usize,
    scroll: usize,
) {
    if area.height == 0 {
        return;
    }
    let t = theme;
    let all = rows(model);
    let mut y = area.y;
    for r in all.iter().skip(scroll) {
        if y >= area.y + area.height {
            break;
        }
        match r {
            Row::Header(h) => buf.set_string(area.x + 1, y, h, t.section()),
            Row::Blank => {}
            Row::Channel {
                row,
                item,
                selected,
            } => {
                draw_row(
                    Rect::new(area.x, y, area.width, 1),
                    buf,
                    item,
                    *selected,
                    focused && cursor == *row,
                    t,
                );
            }
        }
        y += 1;
    }
    // Scroll hints sit in the border column to the right of the list, so
    // they never cover a pill.
    let hint_x = area.x + area.width;
    if scroll > 0 {
        buf.set_string(hint_x, area.y, "▲", t.label());
    }
    if scroll + (area.height as usize) < all.len() {
        buf.set_string(hint_x, area.y + area.height - 1, "▼", t.label());
    }
}

/// The footer block: divider, quick strip, preset, source, volume, CPU.
pub fn draw_footer(
    area: Rect,
    buf: &mut Buffer,
    model: &ShellModel,
    theme: &Theme,
    focus: Option<FooterRow>,
    strip_cursor: usize,
) {
    if area.height < 2 {
        return;
    }
    let t = theme;
    let w = area.width as usize;
    let mut y = area.y;
    let divider = if t.glyphs == Glyphs::Ascii {
        "-"
    } else {
        "─"
    };
    buf.set_string(area.x, y, divider.repeat(w), t.chrome_style());
    y += 1;

    // The quick strip: `M X● L○ V○ P● T , b○`. When that does not fit, the
    // toggles close up against each other; their dots keep them readable.
    let texts: Vec<(String, Option<bool>)> = model
        .strip
        .iter()
        .map(|item| {
            let dot = match (item.state, t.glyphs) {
                (Some(true), Glyphs::Ascii) => "*",
                (Some(false), Glyphs::Ascii) => "o",
                (Some(true), _) => "●",
                (Some(false), _) => "○",
                (None, _) => "",
            };
            (format!("{}{}", item.key, dot), item.state)
        })
        .collect();
    let full: usize = texts
        .iter()
        .map(|(s, _)| s.chars().count() + 1)
        .sum::<usize>()
        + 1;
    let tight = full > w;
    let mut x = area.x + 1;
    for (i, (text, state)) in texts.iter().enumerate() {
        let base = match state {
            Some(true) => Style::default().fg(t.ok),
            Some(false) => t.label(),
            None => t.value(),
        };
        let style = if focus == Some(FooterRow::Strip) && i == strip_cursor {
            t.pill(t.accent).add_modifier(Modifier::BOLD)
        } else {
            base
        };
        let tw = text.chars().count() as u16;
        if x + tw > area.x + area.width {
            break;
        }
        buf.set_string(x, y, text, style);
        let next_is_toggle = texts.get(i + 1).is_some_and(|(_, st)| st.is_some());
        let gap = if tight && state.is_some() && next_is_toggle {
            0
        } else {
            1
        };
        x += tw + gap;
    }
    y += 1;

    let picker = |buf: &mut Buffer, y: u16, label: &str, value: &str, focused: bool| {
        let label_style = if focused { t.focused() } else { t.label() };
        buf.set_string(area.x + 1, y, fit_left(label, 7), label_style);
        let vw = w.saturating_sub(9);
        let text = format!("‹{}›", truncate(value, vw.saturating_sub(2)));
        buf.set_string(
            area.x + 8,
            y,
            &text,
            if focused { t.focused() } else { t.value() },
        );
    };
    if y < area.y + area.height {
        let label = if model.preset_dirty {
            format!("{}*", model.preset_label)
        } else {
            model.preset_label.clone()
        };
        picker(buf, y, "Preset", &label, focus == Some(FooterRow::Preset));
        y += 1;
    }
    if y < area.y + area.height {
        if let Some((choices, i)) = &model.source {
            picker(
                buf,
                y,
                "Source",
                choices.get(*i).map(String::as_str).unwrap_or(""),
                focus == Some(FooterRow::Source),
            );
        }
        y += 1;
    }
    if y + 1 < area.y + area.height {
        let vf = focus == Some(FooterRow::Volume);
        let mode = match model.volume_mode {
            VolumeMode::User => "User",
            VolumeMode::Master => "Master",
        };
        let readout = if model.volume_mode == VolumeMode::Master && model.volume_db <= -128.0 {
            "-inf".to_string()
        } else {
            format!("{:.1} dB", model.volume_db)
        };
        let (label, mode) = if w >= 21 {
            ("Volume", mode)
        } else if w >= 18 {
            ("Vol", mode)
        } else {
            ("Vol", &mode[..1])
        };
        buf.set_string(
            area.x + 1,
            y,
            label,
            if vf { t.focused() } else { t.label() },
        );
        buf.set_string(
            area.x + 2 + label.len() as u16,
            y,
            mode,
            if vf { t.focused() } else { t.value() },
        );
        buf.set_string(
            area.x + area.width - readout.len() as u16 - 1,
            y,
            &readout,
            t.value(),
        );
        y += 1;
        let frac = match model.volume_mode {
            VolumeMode::User => user_fraction(model.volume_db),
            VolumeMode::Master => master_fraction(model.volume_db),
        };
        let color = if model.volume_mode == VolumeMode::Master {
            t.danger
        } else {
            t.fg
        };
        slider::draw(
            Rect::new(area.x + 1, y, area.width - 2, 1),
            buf,
            frac,
            color,
            vf,
            t,
        );
        y += 1;
    }
    if y < area.y + area.height {
        // `C0 ▓░░ 31% C1 ▓▓░ 74%` in the space a sidebar has.
        // Each meter is label, space, bar, space, a four-wide percentage and
        // a gap, so the bar takes whatever the width leaves.
        let sep: &str = if w >= 24 { " " } else { "" };
        let cells = ((w.saturating_sub(1)) / 2)
            .saturating_sub(7 + sep.len())
            .clamp(2, 8) as u16;
        let (f, e) = if t.glyphs == Glyphs::Ascii {
            ("#", ".")
        } else {
            ("▓", "░")
        };
        let mut x = area.x + 1;
        for (label, pct) in [("C0", model.cpu.0), ("C1", model.cpu.1)] {
            buf.set_string(x, y, label, t.label());
            x += 3;
            let filled = ((pct.min(100) as f32 / 100.0) * cells as f32).round() as u16;
            let color = if pct > 90 { t.danger } else { t.accent };
            for i in 0..cells {
                let cell = &mut buf[(x + i, y)];
                if i < filled {
                    cell.set_symbol(f).set_style(Style::default().fg(color));
                } else {
                    cell.set_symbol(e)
                        .set_style(Style::default().fg(t.chrome_faint));
                }
            }
            x += cells;
            buf.set_string(
                x,
                y,
                format!("{sep}{}", fit_right(&format!("{pct}%"), 4)),
                t.value(),
            );
            x += 5 + sep.len() as u16;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tapers_match_the_consoles() {
        assert!((user_fraction(0.0) - 1.0).abs() < 1e-9);
        assert!((user_fraction(-60.0)).abs() < 1e-9);
        assert!(
            (user_fraction(-45.0) - 0.5).abs() < 1e-9,
            "sqrt taper: -45 dB is halfway"
        );
        assert!((master_fraction(0.0) - 1.0).abs() < 1e-9);
        assert!((master_fraction(-128.0)).abs() < 1e-9);
        // -10 dB is 100 of 248 units from the top.
        assert!((master_fraction(-10.0) - (1.0 - 100.0 / 248.0)).abs() < 1e-9);
        assert!((master_fraction(-40.0) - (1.0 - 160.0 / 248.0)).abs() < 1e-9);
    }

    #[test]
    fn footer_focus_skips_the_source_row_when_there_is_none() {
        assert_eq!(FooterRow::Preset.next(false), FooterRow::Volume);
        assert_eq!(FooterRow::Preset.next(true), FooterRow::Source);
        assert_eq!(FooterRow::Volume.prev(false), FooterRow::Preset);
        assert_eq!(FooterRow::Strip.prev(true), FooterRow::Strip);
    }
}
