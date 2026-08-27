//! Level meters with peak hold and a clip cell, and the CPU meter.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;

use super::text::fit_left;
use crate::theme::{ColorDepth, Glyphs, Theme};

/// Peak-hold ballistics, owned by the screen per channel. The device sends
/// raw block peaks with no smoothing; the hold and decay are ours.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PeakHold {
    pub peak: f32,
    /// Seconds the hold has left before it starts to fall.
    pub hold: f32,
}

impl PeakHold {
    pub const HOLD_S: f32 = 1.0;
    /// Linear decay in level units per second once the hold expires.
    pub const DECAY_PER_S: f32 = 0.5;

    pub fn update(&mut self, level: f32, dt: f32) {
        if level >= self.peak {
            self.peak = level;
            self.hold = Self::HOLD_S;
        } else if self.hold > 0.0 {
            self.hold = (self.hold - dt).max(0.0);
        } else {
            self.peak = (self.peak - Self::DECAY_PER_S * dt).max(level);
        }
    }
}

/// A meter is read logarithmically; a linear bar spends most of its length on
/// the top 6 dB.
pub fn db(level: f32) -> f32 {
    if level <= 0.0001 {
        -60.0
    } else {
        20.0 * level.log10()
    }
}

pub fn fraction(level: f32) -> f32 {
    ((db(level) + 60.0) / 60.0).clamp(0.0, 1.0)
}

pub struct LevelMeter<'a> {
    pub label: Option<&'a str>,
    pub label_width: u16,
    /// Linear 0..1, as the device reports it.
    pub level: f32,
    pub peak: Option<f32>,
    pub clipped: bool,
    pub color: Color,
    pub theme: &'a Theme,
    /// Muted or disabled: drawn dim.
    pub inactive: bool,
    /// Show the dB readout after the bar.
    pub readout: bool,
}

impl<'a> LevelMeter<'a> {
    pub fn new(level: f32, color: Color, theme: &'a Theme) -> Self {
        Self {
            label: None,
            label_width: 0,
            level,
            peak: None,
            clipped: false,
            color,
            theme,
            inactive: false,
            readout: false,
        }
    }
    pub fn label(mut self, l: &'a str, width: u16) -> Self {
        self.label = Some(l);
        self.label_width = width;
        self
    }
    pub fn peak(mut self, p: f32) -> Self {
        self.peak = Some(p);
        self
    }
    pub fn clipped(mut self, c: bool) -> Self {
        self.clipped = c;
        self
    }
    pub fn inactive(mut self, i: bool) -> Self {
        self.inactive = i;
        self
    }
    pub fn readout(mut self, r: bool) -> Self {
        self.readout = r;
        self
    }
}

impl Widget for LevelMeter<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < self.label_width + 4 {
            return;
        }
        let t = self.theme;
        let y = area.y;
        let mut x = area.x;
        if let Some(l) = self.label {
            buf.set_string(
                x,
                y,
                fit_left(l, self.label_width as usize),
                if self.inactive { t.label() } else { t.value() },
            );
            x += self.label_width;
        }
        let readout = if self.readout {
            format!(" {:>3.0}", db(self.level))
        } else {
            String::new()
        };
        // One cell for the clip zone, the rest for the bar.
        let bar_w = (area.x + area.width).saturating_sub(x + 1 + readout.len() as u16);
        if bar_w == 0 {
            return;
        }
        let (fill, empty, peak_ch, clip_ch) = match t.glyphs {
            Glyphs::Ascii => ("#", ".", "|", "!"),
            _ => ("▓", "░", "▌", "▏"),
        };
        let filled = (fraction(self.level) * bar_w as f32).round() as u16;
        let peak_cell = self
            .peak
            .map(|p| ((fraction(p) * bar_w as f32).round() as u16).min(bar_w.saturating_sub(1)));
        let fill_style = if self.inactive {
            t.label()
        } else {
            Style::default().fg(self.color)
        };
        for i in 0..bar_w {
            let cell = &mut buf[(x + i, y)];
            if Some(i) == peak_cell && i >= filled {
                cell.set_symbol(peak_ch).set_style(fill_style);
            } else if i < filled {
                cell.set_symbol(fill).set_style(fill_style);
            } else {
                cell.set_symbol(empty)
                    .set_style(Style::default().fg(t.chrome_faint));
            }
        }
        let clip_x = x + bar_w;
        let clip_style = if self.clipped {
            t.alarm().add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.chrome_faint)
        };
        let clip_sym = if self.clipped && t.depth == ColorDepth::Mono {
            "!"
        } else {
            clip_ch
        };
        buf[(clip_x, y)].set_symbol(clip_sym).set_style(clip_style);
        if self.readout {
            buf.set_string(clip_x + 1, y, &readout, t.label());
        }
    }
}

/// `C0 ▓▓▓░░░░░ 31%`: eight cells, red above 90 %.
pub struct CpuMeter<'a> {
    pub label: &'a str,
    pub percent: u8,
    pub theme: &'a Theme,
}

impl<'a> CpuMeter<'a> {
    pub fn new(label: &'a str, percent: u8, theme: &'a Theme) -> Self {
        Self {
            label,
            percent,
            theme,
        }
    }
    pub const WIDTH: u16 = 3 + 8 + 5;
}

impl Widget for CpuMeter<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < Self::WIDTH {
            return;
        }
        let t = self.theme;
        let (fill, empty) = match t.glyphs {
            Glyphs::Ascii => ("#", "."),
            _ => ("▓", "░"),
        };
        buf.set_string(area.x, area.y, self.label, t.label());
        let x = area.x + self.label.len() as u16 + 1;
        let filled = ((self.percent.min(100) as f32 / 100.0) * 8.0).round() as u16;
        let color = if self.percent > 90 {
            t.danger
        } else {
            t.accent
        };
        for i in 0..8 {
            let cell = &mut buf[(x + i, area.y)];
            if i < filled {
                cell.set_symbol(fill).set_style(Style::default().fg(color));
            } else {
                cell.set_symbol(empty)
                    .set_style(Style::default().fg(t.chrome_faint));
            }
        }
        buf.set_string(x + 9, area.y, format!("{:>3}%", self.percent), t.value());
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{render, render_buf};
    use super::*;

    #[test]
    fn a_meter_is_log_scaled_with_a_clip_cell() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        // -20 dB is two thirds of the way along.
        let s = render(LevelMeter::new(0.1, t.inputs[0], &t).label("FL", 4), 16, 1);
        assert_eq!(s, "FL  ▓▓▓▓▓▓▓░░░░▏");
        let b = render_buf(
            LevelMeter::new(0.1, t.inputs[0], &t)
                .label("FL", 4)
                .clipped(true),
            16,
            1,
        );
        assert!(b[(15, 0)].modifier.contains(Modifier::REVERSED));
        // -6 dB is nine tenths of the way along an eleven-cell bar.
        assert_eq!(
            render(LevelMeter::new(0.0, t.inputs[0], &t).peak(0.5), 12, 1),
            "░░░░░░░░░░▌▏"
        );
    }

    #[test]
    fn peak_hold_holds_then_falls() {
        let mut p = PeakHold::default();
        p.update(0.8, 0.05);
        assert_eq!(p.peak, 0.8);
        p.update(0.2, 0.5);
        assert_eq!(p.peak, 0.8, "still holding");
        p.update(0.2, 0.6);
        assert_eq!(p.hold, 0.0);
        p.update(0.2, 0.2);
        assert!(p.peak < 0.8 && p.peak > 0.2, "{}", p.peak);
        p.update(0.9, 0.0);
        assert_eq!(p.peak, 0.9, "a new peak re-arms the hold");
    }

    #[test]
    fn the_cpu_meter_turns_red_over_ninety() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        assert_eq!(
            render(CpuMeter::new("C0", 31, &t), 20, 1),
            "C0 ▓▓░░░░░░  31%"
        );
        let b = render_buf(CpuMeter::new("C1", 95, &t), 20, 1);
        assert_eq!(b[(3, 0)].fg, t.danger);
    }
}
