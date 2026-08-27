//! The Linkwitz Transform panel: the Console's popover, as a centred box.
//!
//! A Linkwitz Transform re-aligns a sealed woofer's rolloff from the driver's
//! `(f0, Q0)` to a target `(fp, Qp)`, and it buys that extension with real
//! low-frequency gain. A mistyped `fp` is tens of dB into a driver, so edits
//! are staged here and the resulting DC boost is shown before anything is
//! written, exactly as the Console does it.
//!
//! `Qp` is a readout rather than a field: it rides the wire in the 18-byte
//! form of the band packet, and the shared command grammar has no way to say
//! it. Everything else applies as one ordinary `eq` command.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use dspi_proto::value::EqParamPacket;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, BorderType, Borders, Clear, Widget};

use crate::theme::{Glyphs, Theme};
use crate::widgets::text::{q as q_text, wrap};
use crate::widgets::{Action, NumberEdit};

/// What a key did to the panel.
#[derive(Debug, Clone, PartialEq)]
pub enum PanelEvent {
    Handled,
    /// Escape, or Revert twice: the draft is dropped.
    Close,
    /// Apply: write this band.
    Apply(EqParamPacket),
}

/// The rows the cursor walks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    F0,
    Q0,
    Fp,
    Revert,
    Apply,
}

const ITEMS: [Item; 5] = [Item::F0, Item::Q0, Item::Fp, Item::Revert, Item::Apply];

pub const CAPTION: &str =
    "Re-align a sealed woofer's rolloff from the driver's (f0, Q0) to a target (fp, Qp).";
pub const BOOST_CAPTION: &str = "Real low-frequency gain (40 x log10(f0/fp)). It uses driver \
     excursion and amp headroom - reduce preamp or master volume to match.";
pub const QP_NOTE: &str =
    "Qp is shown only: the command grammar has no field for it, so it keeps its stored value.";

pub struct LinkwitzPanel {
    /// What the device has now.
    pub applied: EqParamPacket,
    /// What the panel would write.
    pub draft: EqParamPacket,
    cursor: usize,
    edit: Option<NumberEdit>,
}

impl LinkwitzPanel {
    pub fn new(band: EqParamPacket) -> Self {
        Self {
            applied: band,
            draft: band,
            cursor: 0,
            edit: None,
        }
    }

    /// The implied DC boost, `40 * log10(f0 / fp)` dB.
    pub fn dc_boost(&self) -> f64 {
        boost_of(&self.draft)
    }

    /// Whether the draft holds edits that are not on the device yet. Compared
    /// with tolerances because applying quantises.
    pub fn dirty(&self) -> bool {
        let d = &self.draft;
        let p = &self.applied;
        (d.freq - p.freq).abs() >= 0.05
            || (d.q - p.q).abs() >= 0.0005
            || (d.gain_db - p.gain_db).abs() >= 0.05
    }

    fn value(&self, item: Item) -> f64 {
        match item {
            Item::F0 => self.draft.freq as f64,
            Item::Q0 => self.draft.q as f64,
            Item::Fp => self.draft.gain_db as f64,
            _ => 0.0,
        }
    }

    fn set(&mut self, item: Item, v: f64) {
        match item {
            Item::F0 => self.draft.freq = v.clamp(10.0, 24000.0) as f32,
            Item::Q0 => self.draft.q = v.clamp(0.1, 20.0) as f32,
            Item::Fp => self.draft.gain_db = v.clamp(10.0, 24000.0) as f32,
            _ => {}
        }
    }

    /// The Console's steps: 1 Hz for a frequency, 0.01 for a Q.
    fn step(item: Item) -> f64 {
        match item {
            Item::Q0 => 0.01,
            _ => 1.0,
        }
    }

    fn text(&self, item: Item) -> String {
        match item {
            Item::F0 => format!("{:.0} Hz", self.draft.freq),
            Item::Q0 => q_text(self.draft.q as f64),
            Item::Fp => format!("{:.0} Hz", self.draft.gain_db),
            _ => String::new(),
        }
    }

    pub fn handle(&mut self, key: KeyEvent) -> PanelEvent {
        let item = ITEMS[self.cursor.min(ITEMS.len() - 1)];
        if self.edit.is_some() {
            if matches!(key.code, KeyCode::Left | KeyCode::Right) {
                let dir = if key.code == KeyCode::Left { -1.0 } else { 1.0 };
                let mult = if key.modifiers.contains(KeyModifiers::SHIFT) {
                    10.0
                } else {
                    1.0
                };
                let now = self
                    .edit
                    .as_ref()
                    .and_then(|e| e.value())
                    .unwrap_or(self.value(item));
                self.set(item, now + dir * Self::step(item) * mult);
                self.edit = Some(NumberEdit {
                    text: self.raw(item),
                    dirty: false,
                });
                return PanelEvent::Handled;
            }
            let edit = self.edit.as_mut().expect("armed");
            match edit.handle(key) {
                Some(Action::Committed(v)) => {
                    self.edit = None;
                    self.set(item, v);
                }
                Some(Action::Closed) => self.edit = None,
                _ => {}
            }
            return PanelEvent::Handled;
        }

        match key.code {
            KeyCode::Esc => PanelEvent::Close,
            KeyCode::Up | KeyCode::Char('k') => {
                self.cursor = self.cursor.saturating_sub(1);
                PanelEvent::Handled
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                self.cursor = (self.cursor + 1).min(ITEMS.len() - 1);
                PanelEvent::Handled
            }
            KeyCode::Left | KeyCode::Right if !matches!(item, Item::Revert | Item::Apply) => {
                let dir = if key.code == KeyCode::Left { -1.0 } else { 1.0 };
                let mult = if key.modifiers.contains(KeyModifiers::SHIFT) {
                    10.0
                } else {
                    1.0
                };
                let v = self.value(item) + dir * Self::step(item) * mult;
                self.set(item, v);
                PanelEvent::Handled
            }
            KeyCode::Enter => match item {
                Item::Revert => {
                    self.draft = self.applied;
                    PanelEvent::Handled
                }
                Item::Apply => {
                    if self.dirty() {
                        PanelEvent::Apply(self.draft)
                    } else {
                        PanelEvent::Close
                    }
                }
                other => {
                    self.edit = Some(NumberEdit {
                        text: self.raw(other),
                        dirty: false,
                    });
                    PanelEvent::Handled
                }
            },
            _ => PanelEvent::Handled,
        }
    }

    fn raw(&self, item: Item) -> String {
        match item {
            Item::Q0 => q_text(self.draft.q as f64),
            Item::F0 => format!("{:.0}", self.draft.freq),
            Item::Fp => format!("{:.0}", self.draft.gain_db),
            _ => String::new(),
        }
    }

    /// The box the panel wants, centred in `area`.
    pub fn size(&self, area: Rect) -> Rect {
        let w = 52u16.min(area.width.saturating_sub(2)).max(24);
        let h = 14u16.min(area.height).max(6);
        Rect::new(
            area.x + (area.width.saturating_sub(w)) / 2,
            area.y + (area.height.saturating_sub(h)) / 2,
            w,
            h,
        )
    }

    pub fn draw(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let r = self.size(area);
        Clear.render(r, buf);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(if theme.glyphs == Glyphs::Ascii {
                BorderType::Plain
            } else {
                BorderType::Rounded
            })
            .border_style(Style::default().fg(theme.accent))
            .title(" Linkwitz Transform ")
            .title_style(theme.title());
        let inner = block.inner(r);
        block.render(r, buf);
        if inner.height < 4 {
            return;
        }
        let w = inner.width as usize;
        let mut y = inner.y;
        let bottom = inner.y + inner.height;
        let line = |buf: &mut Buffer, y: &mut u16, text: &str, style: Style| {
            if *y < bottom {
                buf.set_string(inner.x, *y, crate::widgets::text::truncate(text, w), style);
                *y += 1;
            }
        };

        for l in wrap(CAPTION, w, 2) {
            line(buf, &mut y, &l, theme.label());
        }
        y += 1;

        let focused = |i: usize| self.cursor == i;
        let mark = |i: usize| if focused(i) { "▸" } else { " " };
        let field = |i: usize, armed: bool| {
            if armed {
                theme.editing()
            } else if focused(i) {
                theme.focused()
            } else {
                theme.value()
            }
        };
        let armed_text = |i: usize, fallback: String| match (&self.edit, focused(i)) {
            (Some(e), true) => format!("[{}]", e.text),
            _ => fallback,
        };

        if y < bottom {
            buf.set_string(inner.x, y, "Driver", theme.section());
            buf.set_string(inner.x + 8, y, format!("{}f0", mark(0)), theme.label());
            buf.set_string(
                inner.x + 11,
                y,
                armed_text(0, self.text(Item::F0)),
                field(0, self.edit.is_some() && focused(0)),
            );
            buf.set_string(inner.x + 24, y, format!("{}Q0", mark(1)), theme.label());
            buf.set_string(
                inner.x + 27,
                y,
                armed_text(1, self.text(Item::Q0)),
                field(1, self.edit.is_some() && focused(1)),
            );
            y += 1;
        }
        if y < bottom {
            buf.set_string(inner.x, y, "Target", theme.section());
            buf.set_string(inner.x + 8, y, format!("{}fp", mark(2)), theme.label());
            buf.set_string(
                inner.x + 11,
                y,
                armed_text(2, self.text(Item::Fp)),
                field(2, self.edit.is_some() && focused(2)),
            );
            buf.set_string(inner.x + 24, y, " Qp", theme.label());
            buf.set_string(
                inner.x + 27,
                y,
                q_text(self.draft.qp.unwrap_or(0.707) as f64),
                theme.label(),
            );
            y += 1;
        }
        y += 1;

        let boost = self.dc_boost();
        if y < bottom {
            buf.set_string(inner.x, y, "DC boost", theme.section());
            let text = format!("{boost:+.1} dB");
            let style = if boost > 15.0 {
                theme.warning_style()
            } else {
                theme.value()
            };
            buf.set_string(inner.x + 10, y, &text, style);
            if boost > 15.0 {
                let glyph = if theme.glyphs == Glyphs::Ascii {
                    "!"
                } else {
                    "▲"
                };
                buf.set_string(
                    inner.x + 11 + text.len() as u16,
                    y,
                    glyph,
                    theme.warning_style(),
                );
            }
            y += 1;
        }
        for l in wrap(BOOST_CAPTION, w, 2) {
            line(buf, &mut y, &l, theme.label());
        }
        for l in wrap(QP_NOTE, w, 2) {
            line(buf, &mut y, &l, theme.label());
        }

        // Status and buttons on the last row.
        let by = bottom - 1;
        let (status, style) = if self.dirty() {
            ("Not applied yet", theme.warning_style())
        } else {
            ("Applied", theme.label())
        };
        buf.set_string(inner.x, by, status, style);
        let buttons = [(3usize, "Revert"), (4, "Apply")];
        let total: u16 = buttons.iter().map(|(_, b)| b.len() as u16 + 4).sum();
        let mut x = inner.x + inner.width.saturating_sub(total);
        for (i, label) in buttons {
            let text = format!(" {label} ");
            let style = if focused(i) {
                theme.pill(theme.accent)
            } else if self.dirty() {
                theme.value()
            } else {
                theme.label()
            };
            buf.set_string(x, by, &text, style);
            x += text.len() as u16 + 2;
        }
    }
}

/// `40 * log10(f0 / fp)`, the Console's `linkwitzDCBoostDB`.
pub fn boost_of(p: &EqParamPacket) -> f64 {
    if p.gain_db <= 0.0 || p.freq <= 0.0 {
        return 0.0;
    }
    40.0 * (p.freq as f64 / p.gain_db as f64).log10()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ColorDepth, Glyphs};
    use crate::widgets::testing::key;
    use dspi_proto::FilterType;

    fn band(f0: f32, q0: f32, fp: f32) -> EqParamPacket {
        EqParamPacket {
            channel: 16,
            band: 0,
            filter_type: FilterType::LinkwitzTransform,
            bypass: false,
            freq: f0,
            q: q0,
            gain_db: fp,
            qp: Some(0.707),
        }
    }

    fn draw(p: &LinkwitzPanel, w: u16, h: u16) -> String {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        term.draw(|f| p.draw(f.area(), f.buffer_mut(), &t)).unwrap();
        let buf = term.backend().buffer();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_dc_boost_is_forty_log_ten_of_the_ratio() {
        let p = LinkwitzPanel::new(band(40.0, 0.5, 25.0));
        assert!((p.dc_boost() - 8.16).abs() < 0.01, "{}", p.dc_boost());
        // An octave of extension is 12 dB.
        let p = LinkwitzPanel::new(band(40.0, 0.5, 20.0));
        assert!((p.dc_boost() - 12.04).abs() < 0.01);
    }

    #[test]
    fn the_panel_shows_the_consoles_words_and_both_alignments() {
        let p = LinkwitzPanel::new(band(40.0, 0.5, 25.0));
        let f = draw(&p, 56, 16);
        assert!(f.contains(" Linkwitz Transform "), "{f}");
        assert!(f.contains("Re-align a sealed woofer's rolloff"), "{f}");
        assert!(
            f.contains("Driver") && f.contains("f0") && f.contains("40 Hz"),
            "{f}"
        );
        assert!(
            f.contains("Target") && f.contains("fp") && f.contains("25 Hz"),
            "{f}"
        );
        assert!(f.contains("Qp") && f.contains("0.707"), "{f}");
        assert!(f.contains("DC boost") && f.contains("+8.2 dB"), "{f}");
        assert!(f.contains("Applied"), "{f}");
        assert!(f.contains("Revert") && f.contains("Apply"), "{f}");
    }

    #[test]
    fn a_boost_over_fifteen_db_is_marked() {
        let p = LinkwitzPanel::new(band(40.0, 0.5, 15.0));
        assert!(p.dc_boost() > 15.0);
        let f = draw(&p, 56, 16);
        assert!(f.contains("▲"), "{f}");
    }

    #[test]
    fn edits_are_staged_and_revert_puts_them_back() {
        let mut p = LinkwitzPanel::new(band(40.0, 0.5, 25.0));
        assert!(!p.dirty());
        // The cursor starts on f0; one press is one hertz.
        p.handle(key(KeyCode::Right));
        assert_eq!(p.draft.freq, 41.0);
        assert!(p.dirty());
        let f = draw(&p, 56, 16);
        assert!(f.contains("Not applied yet"), "{f}");
        // Revert is the fourth item.
        for _ in 0..3 {
            p.handle(key(KeyCode::Down));
        }
        p.handle(key(KeyCode::Enter));
        assert!(!p.dirty());
        assert_eq!(p.draft.freq, 40.0);
    }

    #[test]
    fn apply_hands_back_the_band_and_escape_drops_the_draft() {
        let mut p = LinkwitzPanel::new(band(40.0, 0.5, 25.0));
        p.handle(key(KeyCode::Right));
        for _ in 0..4 {
            p.handle(key(KeyCode::Down));
        }
        match p.handle(key(KeyCode::Enter)) {
            PanelEvent::Apply(b) => assert_eq!(b.freq, 41.0),
            other => panic!("{other:?}"),
        }
        let mut p = LinkwitzPanel::new(band(40.0, 0.5, 25.0));
        assert_eq!(p.handle(key(KeyCode::Esc)), PanelEvent::Close);
    }

    #[test]
    fn a_field_can_be_typed_as_well_as_nudged() {
        let mut p = LinkwitzPanel::new(band(40.0, 0.5, 25.0));
        p.handle(key(KeyCode::Enter));
        for c in "32".chars() {
            p.handle(key(KeyCode::Char(c)));
        }
        p.handle(key(KeyCode::Enter));
        assert_eq!(p.draft.freq, 32.0);
        // Q0 steps by a hundredth.
        p.handle(key(KeyCode::Down));
        p.handle(key(KeyCode::Right));
        assert!((p.draft.q - 0.51).abs() < 1e-4, "{}", p.draft.q);
    }
}
