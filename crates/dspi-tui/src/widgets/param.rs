//! The parameter row: label, value, slider, caption. The Console's
//! `ValueField` plus `CustomSlider`, as one widget.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::widgets::Widget;

use super::text::{fit_left, truncate, with_unit, wrap};
use super::{Action, slider};
use crate::theme::Theme;

/// How a slider maps position to value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taper {
    Linear,
    /// Frequency and Q: equal slider travel is an equal ratio.
    Log,
}

/// Typed entry over a numeric value. One of these lives in the screen for
/// whichever row is armed; the row draws it when it is given one.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NumberEdit {
    pub text: String,
    /// False until the first keystroke, which replaces the seeded text.
    pub dirty: bool,
}

impl NumberEdit {
    pub fn start(value: f64, decimals: usize) -> Self {
        Self {
            text: format!("{value:.d$}", d = decimals),
            dirty: false,
        }
    }

    /// The typed value, if it parses.
    pub fn value(&self) -> Option<f64> {
        self.text.trim().replace(',', ".").parse().ok()
    }

    /// Handle a key while armed. Returns the committed value on Enter, `Closed`
    /// on Escape, and `None` for keys that only edited the text.
    pub fn handle(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Enter => self.value().map(Action::Committed),
            KeyCode::Esc => Some(Action::Closed),
            KeyCode::Backspace => {
                self.dirty = true;
                self.text.pop();
                None
            }
            KeyCode::Char(c) if c.is_ascii_digit() || c == '.' || c == '-' || c == ',' => {
                // The first keystroke replaces the seeded text, as a selected
                // field does.
                if !self.dirty {
                    self.text.clear();
                    self.dirty = true;
                }
                self.text.push(c);
                None
            }
            _ => None,
        }
    }
}

/// A parameter row.
pub struct ParamRow<'a> {
    pub label: &'a str,
    pub value: f64,
    pub min: f64,
    pub max: f64,
    /// The nudge step for `←`/`→`; `Shift` multiplies it by ten.
    pub step: f64,
    pub unit: &'a str,
    pub decimals: usize,
    pub taper: Taper,
    pub caption: Option<&'a str>,
    /// The colour of the filled track; a channel colour or the theme's `fg`.
    pub color: Color,
    pub theme: &'a Theme,
    pub focused: bool,
    pub enabled: bool,
    /// Typed entry in progress: the value cell shows the text instead.
    pub edit: Option<&'a NumberEdit>,
    /// Compact rows draw label, value and slider on one line and no caption.
    pub compact: bool,
    /// Labels on the slider ends, e.g. `Warm` and `Aggressive`.
    pub ends: Option<(&'a str, &'a str)>,
}

impl<'a> ParamRow<'a> {
    pub fn new(
        label: &'a str,
        value: f64,
        min: f64,
        max: f64,
        unit: &'a str,
        theme: &'a Theme,
    ) -> Self {
        Self {
            label,
            value,
            min,
            max,
            step: 1.0,
            unit,
            decimals: 1,
            taper: Taper::Linear,
            caption: None,
            color: theme.fg,
            theme,
            focused: false,
            enabled: true,
            edit: None,
            compact: false,
            ends: None,
        }
    }

    pub fn step(mut self, s: f64) -> Self {
        self.step = s;
        self
    }
    pub fn decimals(mut self, d: usize) -> Self {
        self.decimals = d;
        self
    }
    pub fn taper(mut self, t: Taper) -> Self {
        self.taper = t;
        self
    }
    pub fn caption(mut self, c: &'a str) -> Self {
        self.caption = Some(c);
        self
    }
    pub fn color(mut self, c: Color) -> Self {
        self.color = c;
        self
    }
    pub fn focused(mut self, f: bool) -> Self {
        self.focused = f;
        self
    }
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }
    pub fn edit(mut self, e: Option<&'a NumberEdit>) -> Self {
        self.edit = e;
        self
    }
    pub fn compact(mut self, c: bool) -> Self {
        self.compact = c;
        self
    }
    pub fn ends(mut self, lo: &'a str, hi: &'a str) -> Self {
        self.ends = Some((lo, hi));
        self
    }

    /// How many rows this row wants at a given width.
    pub fn height(&self, width: u16) -> u16 {
        if self.compact {
            return 1;
        }
        let cap = self
            .caption
            .map(|c| wrap(c, width.saturating_sub(2) as usize, 2).len() as u16)
            .unwrap_or(0);
        2 + cap
    }

    pub fn fraction(&self) -> f64 {
        fraction(self.value, self.min, self.max, self.taper)
    }

    /// The value after one nudge in `direction`, clamped.
    pub fn nudged(&self, direction: i32, coarse: bool) -> f64 {
        let mult = if coarse { 10.0 } else { 1.0 };
        let v = match self.taper {
            Taper::Linear => self.value + direction as f64 * self.step * mult,
            // A log field steps by ratio; `step` is the ratio per press, e.g.
            // 1.05 for frequency.
            Taper::Log => self.value * self.step.powf(direction as f64 * mult),
        };
        v.clamp(self.min, self.max)
    }

    /// Keys for a focused, unarmed row.
    pub fn handle(&self, key: KeyEvent) -> Option<Action> {
        if !self.enabled {
            return None;
        }
        let coarse = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Left | KeyCode::Char('h') => Some(Action::Changed(self.nudged(-1, coarse))),
            KeyCode::Right | KeyCode::Char('l') => Some(Action::Changed(self.nudged(1, coarse))),
            KeyCode::Char('-') => Some(Action::Changed(self.nudged(-1, coarse))),
            KeyCode::Char('+') | KeyCode::Char('=') => {
                Some(Action::Changed(self.nudged(1, coarse)))
            }
            KeyCode::Enter => Some(Action::Open),
            KeyCode::Backspace => Some(Action::Reset),
            _ => None,
        }
    }

    fn value_text(&self) -> String {
        match self.edit {
            Some(e) => format!("[{}]", e.text),
            None => with_unit(self.value, self.unit, self.decimals),
        }
    }
}

pub fn fraction(value: f64, min: f64, max: f64, taper: Taper) -> f64 {
    match taper {
        Taper::Linear => {
            if max <= min {
                0.0
            } else {
                ((value - min) / (max - min)).clamp(0.0, 1.0)
            }
        }
        Taper::Log => {
            let (lo, hi) = (min.max(1e-6).ln(), max.max(1e-6).ln());
            if hi <= lo {
                0.0
            } else {
                ((value.max(1e-6).ln() - lo) / (hi - lo)).clamp(0.0, 1.0)
            }
        }
    }
}

impl Widget for ParamRow<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 12 {
            return;
        }
        let t = self.theme;
        let label_style = if !self.enabled {
            t.label()
        } else if self.focused {
            t.focused()
        } else {
            t.value()
        };
        let value_style = if !self.enabled {
            t.label()
        } else if self.edit.is_some() {
            t.editing()
        } else {
            t.value()
        };
        let marker = if self.focused { "▸" } else { " " };
        let value = self.value_text();
        let w = area.width as usize;

        if self.compact {
            // `▸ Label  value  ━━━●━━━`
            let label_w = self.label.len().min(w / 3);
            let value_w = value.len();
            let x = area.x;
            buf.set_string(x, area.y, marker, t.focused());
            buf.set_string(x + 1, area.y, fit_left(self.label, label_w), label_style);
            buf.set_string(x + 2 + label_w as u16, area.y, &value, value_style);
            let sx = x + 3 + (label_w + value_w) as u16;
            let sw = (area.x + area.width).saturating_sub(sx);
            if sw >= 3 {
                let s = Rect::new(sx, area.y, sw, 1);
                if self.enabled {
                    slider::draw(s, buf, self.fraction(), self.color, self.focused, t);
                } else {
                    slider::draw_disabled(s, buf, t);
                }
            }
            return;
        }

        // Row 1: marker, label left, value right.
        buf.set_string(area.x, area.y, marker, t.focused());
        let label = truncate(self.label, w.saturating_sub(value.len() + 3));
        buf.set_string(area.x + 1, area.y, &label, label_style);
        buf.set_string(
            area.x + area.width - value.len() as u16,
            area.y,
            &value,
            value_style,
        );

        // Row 2: the slider, indented under the label.
        if area.height >= 2 {
            let s = Rect::new(area.x + 1, area.y + 1, area.width - 1, 1);
            match self.ends {
                Some((lo, hi)) if s.width > (lo.len() + hi.len() + 6) as u16 => {
                    buf.set_string(s.x, s.y, lo, t.label());
                    buf.set_string(s.x + s.width - hi.len() as u16, s.y, hi, t.label());
                    let inner = Rect::new(
                        s.x + lo.len() as u16 + 1,
                        s.y,
                        s.width - (lo.len() + hi.len() + 2) as u16,
                        1,
                    );
                    if self.enabled {
                        slider::draw(inner, buf, self.fraction(), self.color, self.focused, t);
                    } else {
                        slider::draw_disabled(inner, buf, t);
                    }
                }
                _ => {
                    if self.enabled {
                        slider::draw(s, buf, self.fraction(), self.color, self.focused, t);
                    } else {
                        slider::draw_disabled(s, buf, t);
                    }
                }
            }
        }

        // Rows 3..: the caption.
        if let Some(c) = self.caption
            && area.height >= 3
        {
            for (i, line) in wrap(c, w.saturating_sub(2), (area.height - 2) as usize)
                .iter()
                .enumerate()
            {
                buf.set_string(area.x + 1, area.y + 2 + i as u16, line, t.label());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{every_theme, key, render, shift};
    use super::*;
    use crate::theme::{ColorDepth, Glyphs};

    fn row<'a>(t: &'a Theme) -> ParamRow<'a> {
        ParamRow::new("Cutoff Frequency", 700.0, 500.0, 2000.0, "Hz", t)
            .decimals(0)
            .step(10.0)
            .caption("Simulates head shadow lowpass cutoff. Lower = more bass crossfeed. Typical: 650-700 Hz.")
    }

    #[test]
    fn a_row_is_label_value_slider_caption() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let s = render(row(&t).focused(true), 50, 4);
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(
            lines[0],
            "▸Cutoff Frequency                           700 Hz"
        );
        assert!(lines[1].starts_with(" ━━"), "{:?}", lines[1]);
        assert!(lines[1].contains('●'));
        assert!(lines[2].starts_with(" Simulates head shadow"));
        assert_eq!(lines.len(), 4);
        // Narrower, the caption runs out of its two lines and says so.
        let s = render(row(&t), 36, 4);
        assert!(s.lines().nth(3).unwrap().ends_with('…'), "{s}");
    }

    #[test]
    fn the_knob_sits_where_the_value_is() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let s = render(row(&t), 41, 2);
        // 700 of 500..2000 is 13.3 % of the way along a 40-cell track.
        let knob = s
            .lines()
            .nth(1)
            .unwrap()
            .chars()
            .position(|c| c == '●')
            .unwrap();
        assert_eq!(knob, 1 + 5);
    }

    #[test]
    fn nudges_step_and_clamp() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let r = row(&t);
        assert_eq!(r.handle(key(KeyCode::Right)), Some(Action::Changed(710.0)));
        assert_eq!(
            r.handle(shift(KeyCode::Right)),
            Some(Action::Changed(800.0))
        );
        let low = ParamRow::new("x", 500.0, 500.0, 2000.0, "Hz", &t).step(10.0);
        assert_eq!(low.handle(key(KeyCode::Left)), Some(Action::Changed(500.0)));
        assert_eq!(r.handle(key(KeyCode::Enter)), Some(Action::Open));
        assert_eq!(r.handle(key(KeyCode::Backspace)), Some(Action::Reset));
        assert_eq!(r.enabled(false).handle(key(KeyCode::Right)), None);
    }

    #[test]
    fn a_log_field_steps_by_ratio() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let r = ParamRow::new("Freq", 1000.0, 10.0, 20000.0, "Hz", &t)
            .taper(Taper::Log)
            .step(2.0);
        assert_eq!(r.handle(key(KeyCode::Right)), Some(Action::Changed(2000.0)));
        assert_eq!(r.handle(key(KeyCode::Left)), Some(Action::Changed(500.0)));
        assert!((fraction(1000.0, 10.0, 100000.0, Taper::Log) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn typing_replaces_the_seed_and_enter_commits() {
        let mut e = NumberEdit::start(700.0, 0);
        assert_eq!(e.text, "700");
        assert_eq!(e.handle(key(KeyCode::Char('6'))), None);
        assert_eq!(e.handle(key(KeyCode::Char('5'))), None);
        assert_eq!(e.handle(key(KeyCode::Char('0'))), None);
        assert_eq!(e.text, "650");
        assert_eq!(
            e.handle(key(KeyCode::Enter)),
            Some(Action::Committed(650.0))
        );
        let mut e = NumberEdit::start(1.5, 1);
        assert_eq!(e.handle(key(KeyCode::Esc)), Some(Action::Closed));
        let mut e = NumberEdit::start(1.0, 1);
        e.handle(key(KeyCode::Char('-')));
        e.handle(key(KeyCode::Char('8')));
        e.handle(key(KeyCode::Char(',')));
        e.handle(key(KeyCode::Char('6')));
        assert_eq!(e.value(), Some(-8.6));
    }

    #[test]
    fn an_armed_row_shows_the_text_in_brackets() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let e = NumberEdit::start(700.0, 0);
        let s = render(row(&t).focused(true).edit(Some(&e)), 50, 2);
        assert!(s.lines().next().unwrap().ends_with("[700]"));
    }

    #[test]
    fn a_compact_row_fits_one_line() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let r = ParamRow::new("Preamp", -5.3, -60.0, 10.0, "dB", &t).compact(true);
        let s = render(r, 40, 1);
        assert_eq!(s, " Preamp -5.3 dB ━━━━━━━━━━━━━━━━━━●━━━━━");
    }

    #[test]
    fn a_disabled_row_has_no_knob() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let s = render(row(&t).enabled(false), 50, 2);
        assert!(!s.contains('●'));
    }

    #[test]
    fn every_theme_renders_the_same_text() {
        let reference = render(row(&every_theme()[0]), 50, 4);
        for t in &every_theme()[1..3] {
            assert_eq!(render(row(t), 50, 4), reference);
        }
        // ASCII glyphs swap the track characters but keep the layout.
        let ascii = render(row(&every_theme()[3]), 50, 4);
        assert_eq!(ascii.lines().next(), reference.lines().next());
        assert!(ascii.lines().nth(1).unwrap().contains('O'));
    }
}
