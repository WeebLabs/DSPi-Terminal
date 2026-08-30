//! The Console's dashboard: read-only cards, one per stereo pair.
//!
//! `DashboardView.swift`, in a terminal. A stereo card for the USB input pair,
//! a card per enabled S/PDIF output pair (single when only one half of the
//! pair is on), and a card for the PDM sub. Each card lists every band with
//! the Console's compact type code in the channel's colour.

use crossterm::event::{KeyCode, KeyEvent};
use dspi_proto::FilterType;
use dspi_proto::value::EqParamPacket;
use dspi_session::DeviceState;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Widget;

use super::{Shared, channel_name, trimmed, type_code};
use crate::shell::{Screen, ScreenEvent, Selection};
use crate::theme::{ChannelRole, Theme};
use crate::widgets::card::CardTitle;
use crate::widgets::text::{fit_left, fit_right};
use crate::widgets::{Card, KeyHelp};

/// One dashboard card: one or two channels side by side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardSpec {
    /// A heading drawn above the card, as the Console titles its input card.
    pub title: Option<&'static str>,
    /// Unified channel indices: one for a mono card, two for a stereo one.
    pub channels: Vec<usize>,
}

/// The cards the device's topology calls for, in the Console's order.
pub fn cards(state: &DeviceState) -> Vec<CardSpec> {
    let ni = state.caps.num_inputs as usize;
    let no = state.caps.num_outputs as usize;
    let mut out = Vec::new();
    if ni >= 2 {
        out.push(CardSpec {
            title: Some("STEREO INPUT (USB)"),
            channels: vec![0, 1],
        });
    }
    // The Console pairs every output but the sub, which is always mono.
    let pairs = no.saturating_sub(1) / 2;
    for pair in 0..pairs {
        let (l, r) = (pair * 2, pair * 2 + 1);
        let mut channels = Vec::new();
        if state.output(l).enabled {
            channels.push(ni + l);
        }
        if state.output(r).enabled {
            channels.push(ni + r);
        }
        if !channels.is_empty() {
            out.push(CardSpec {
                title: None,
                channels,
            });
        }
    }
    if no > 0 && state.output(no - 1).enabled {
        out.push(CardSpec {
            title: None,
            channels: vec![ni + no - 1],
        });
    }
    out
}

pub struct Overview {
    #[allow(dead_code)]
    shared: Shared,
    /// Which card the cursor is on.
    pub cursor: usize,
    /// First visible row of the stacked column.
    pub scroll: usize,
}

impl Overview {
    pub fn new(shared: Shared) -> Self {
        Self {
            shared,
            cursor: 0,
            scroll: 0,
        }
    }

    /// Rows one card occupies: a heading row (blank when the card has no
    /// title, so cards drawn abreast line up) and the box.
    fn card_rows(state: &DeviceState) -> u16 {
        state.caps.max_bands as u16 + 3
    }

    /// Two cards sit abreast once the detail region is as wide as the Roomy
    /// density gives it (DESIGN 8). The screen only sees its own rectangle,
    /// so it reads the width rather than the terminal's density.
    fn columns(area: Rect) -> usize {
        if area.width >= 120 { 2 } else { 1 }
    }
}

const KEYS: &[KeyHelp] = &[
    KeyHelp::new("↑ ↓", "Move between cards"),
    KeyHelp::new("Enter", "Select that channel"),
    KeyHelp::new("PgUp PgDn", "Scroll"),
];

impl Screen for Overview {
    fn title(&self) -> String {
        "Overview".into()
    }

    fn draw(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        state: &DeviceState,
        focused: bool,
    ) {
        if area.height == 0 || area.width < 20 {
            return;
        }
        let specs = cards(state);
        if specs.is_empty() {
            return;
        }
        self.cursor = self.cursor.min(specs.len() - 1);

        let cols = Self::columns(area);
        let step = Self::card_rows(state);
        let rows = specs.len().div_ceil(cols) as u16;
        let total = rows * step;

        // Keep the focused card in view.
        let focus_row = (self.cursor / cols) as u16;
        let visible_rows = (area.height / step).max(1);
        let mut first = (self.scroll as u16) / step;
        if focus_row < first {
            first = focus_row;
        } else if focus_row >= first + visible_rows {
            first = focus_row + 1 - visible_rows;
        }
        let max_first = rows.saturating_sub(visible_rows);
        first = first.min(max_first);
        self.scroll = (first * step) as usize;

        let col_w = area.width / cols as u16;
        for (i, spec) in specs.iter().enumerate() {
            let row = (i / cols) as u16;
            if row < first {
                continue;
            }
            let y = area.y + (row - first) * step;
            if y >= area.y + area.height {
                break;
            }
            let x = area.x + (i % cols) as u16 * col_w;
            let r = Rect::new(
                x,
                y,
                col_w.saturating_sub(1),
                step.min(area.y + area.height - y),
            );
            draw_card(r, buf, theme, state, spec, focused && i == self.cursor);
        }

        if total > area.height {
            let hint_x = area.x + area.width - 1;
            if first > 0 {
                buf.set_string(hint_x, area.y, "▲", theme.label());
            }
            if first < max_first {
                buf.set_string(hint_x, area.y + area.height - 1, "▼", theme.label());
            }
        }
    }

    fn handle(&mut self, key: KeyEvent, state: &DeviceState) -> ScreenEvent {
        let specs = cards(state);
        if specs.is_empty() {
            return ScreenEvent::Unhandled;
        }
        let cols = Self::columns(Rect::new(0, 0, 120, 20));
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.cursor = self.cursor.saturating_sub(1);
                ScreenEvent::Handled
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.cursor = (self.cursor + 1).min(specs.len() - 1);
                ScreenEvent::Handled
            }
            KeyCode::PageUp => {
                self.cursor = self.cursor.saturating_sub(cols);
                ScreenEvent::Handled
            }
            KeyCode::PageDown => {
                self.cursor = (self.cursor + cols).min(specs.len() - 1);
                ScreenEvent::Handled
            }
            KeyCode::Enter => {
                let ch = specs[self.cursor].channels.first().copied().unwrap_or(0);
                let ni = state.caps.num_inputs as usize;
                ScreenEvent::Select(if ch < ni {
                    Selection::Input(ch)
                } else {
                    Selection::Output(ch - ni)
                })
            }
            _ => ScreenEvent::Unhandled,
        }
    }

    fn keys(&self) -> &'static [KeyHelp] {
        KEYS
    }
}

fn draw_card(
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    state: &DeviceState,
    spec: &CardSpec,
    focused: bool,
) {
    if area.height < 3 || area.width < 12 {
        return;
    }
    // Every card keeps a heading row whether or not it has a title, so cards
    // drawn side by side line up.
    if let Some(title) = spec.title {
        buf.set_string(area.x + 1, area.y, title, theme.section());
    }
    let body = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        (area.height - 1).min(state.caps.max_bands as u16 + 2),
    );
    if body.height < 3 {
        return;
    }

    // The Console's input card carries no delay (`showDelay: false`); the
    // output cards do.
    let ni = state.caps.num_inputs as usize;
    let details: Vec<String> = spec
        .channels
        .iter()
        .map(|ch| {
            if *ch < ni {
                String::new()
            } else {
                format!("Delay: {:.0} ms", delay_of(state, *ch))
            }
        })
        .collect();
    let names: Vec<String> = spec
        .channels
        .iter()
        .map(|ch| channel_name(state, *ch))
        .collect();
    let colors: Vec<Color> = spec
        .channels
        .iter()
        .map(|ch| {
            theme.role_color(ChannelRole::of(
                *ch as u8,
                state.caps.num_inputs,
                state.caps.num_outputs,
            ))
        })
        .collect();

    let left = CardTitle {
        name: &names[0],
        color: colors[0],
        detail: Some(&details[0]),
    };
    let card = if spec.channels.len() > 1 {
        Card::stereo(
            left,
            CardTitle {
                name: &names[1],
                color: colors[1],
                detail: Some(&details[1]),
            },
            theme,
        )
    } else {
        Card::single(&names[0], colors[0], theme).detail(&details[0])
    };
    let card = card.focused(focused);
    let inner = card.inner(body);
    card.render(body, buf);

    for (slot, ch) in spec.channels.iter().enumerate() {
        let Some(r) = inner.get(slot) else { continue };
        draw_bands(*r, buf, theme, state, *ch, colors[slot]);
    }
}

fn delay_of(state: &DeviceState, channel: usize) -> f32 {
    let ni = state.caps.num_inputs as usize;
    if channel >= ni {
        state.output(channel - ni).delay_ms
    } else {
        state.delay_ms(channel)
    }
}

fn draw_bands(
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    state: &DeviceState,
    channel: usize,
    color: Color,
) {
    let bands = state.bands(channel as u8);
    for (i, band) in bands.iter().enumerate() {
        let y = area.y + i as u16;
        if y >= area.y + area.height {
            break;
        }
        let active = band.filter_type != FilterType::Flat;
        buf.set_string(area.x, y, fit_right(&(i + 1).to_string(), 2), theme.label());
        let code = type_code(band.filter_type);
        buf.set_string(
            area.x + 3,
            y,
            fit_left(&code, 4),
            if active {
                Style::default().fg(color)
            } else {
                theme.label()
            },
        );
        let rest = area.width.saturating_sub(8) as usize;
        let text = if active {
            band_values(band)
        } else {
            "-".into()
        };
        buf.set_string(
            area.x + 8,
            y,
            fit_right(&text, rest),
            if active { theme.value() } else { theme.label() },
        );
    }
}

/// The Console's dashboard row: frequency, gain where the type uses one, and Q
/// for peaking bands alone.
pub fn band_values(band: &EqParamPacket) -> String {
    let mut s = format!("{:.0} Hz", band.freq);
    if band.filter_type.uses_gain() {
        s.push_str(&format!("  {} dB", trimmed(band.gain_db as f64, 2, true)));
    }
    if band.filter_type == FilterType::Peaking {
        s.push_str(&format!("  {} Q", trimmed(band.q as f64, 3, false)));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::shared;
    use crate::shell::fixture;
    use crate::theme::{ColorDepth, Glyphs};
    use crate::widgets::testing::key;

    fn draw(w: u16, h: u16) -> String {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let state = fixture::state();
        let mut s = Overview::new(shared());
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        term.draw(|f| s.draw(f.area(), f.buffer_mut(), &t, &state, true))
            .unwrap();
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
    fn the_fixture_makes_one_input_card_four_output_pairs_and_the_sub() {
        let specs = cards(&fixture::state());
        assert_eq!(specs.len(), 6, "{specs:?}");
        assert_eq!(specs[0].title, Some("STEREO INPUT (USB)"));
        assert_eq!(specs[0].channels, vec![0, 1]);
        assert_eq!(specs[1].channels, vec![8, 9]);
        assert_eq!(specs[5].channels, vec![16], "the sub is mono");
    }

    #[test]
    fn a_disabled_half_leaves_a_single_card_and_a_disabled_pair_none() {
        let mut state = fixture::state();
        let (_, outs, _) = dspi_proto::generated::SECTIONS
            .iter()
            .find(|(n, _, _)| *n == "outputs")
            .copied()
            .unwrap();
        // Turn OUT4 off: its pair becomes a single card.
        state.bulk.patch(outs + 3 * 12, &[0]);
        let specs = cards(&state);
        assert_eq!(specs[2].channels, vec![10], "only OUT3 survives");
        // And with both halves off the pair is gone.
        state.bulk.patch(outs + 2 * 12, &[0]);
        let specs = cards(&state);
        assert_eq!(specs.len(), 5);
    }

    #[test]
    fn the_reference_frame_shows_the_console_strings() {
        let f = draw(94, 21);
        assert!(f.contains("STEREO INPUT (USB)"), "{f}");
        assert!(f.contains("● FL"), "{f}");
        assert!(f.contains("Delay: 0 ms"), "{f}");
        // The fixture's first band is a low shelf at 105 Hz, +8.8 dB.
        assert!(f.contains("LS"), "{f}");
        assert!(f.contains("105 Hz  +8.8 dB"), "{f}");
        // A peaking band shows its Q as well.
        assert!(f.contains("2856 Hz  -8.6 dB  3.58 Q"), "{f}");
        // Unset bands read as a dash.
        assert!(f.contains("OFF"), "{f}");
    }

    #[test]
    fn the_minimum_frame_still_draws_a_card() {
        let f = draw(56, 10);
        assert!(f.contains("STEREO INPUT (USB)"), "{f}");
        assert!(f.contains("FL"), "{f}");
        assert!(f.lines().count() <= 10);
    }

    #[test]
    fn wide_terminals_put_two_cards_abreast() {
        let f = draw(140, 30);
        let first = f.lines().next().unwrap();
        assert!(first.contains("STEREO INPUT (USB)"), "{first}");
        // The second card's header sits on the same row as the first card's.
        let abreast = f
            .lines()
            .any(|l| l.contains("● FL") && l.contains("● OUT L"));
        assert!(abreast, "two cards abreast:\n{f}");
    }

    #[test]
    fn arrows_walk_the_cards_and_enter_names_the_channel() {
        let state = fixture::state();
        let mut s = Overview::new(shared());
        assert_eq!(s.handle(key(KeyCode::Down), &state), ScreenEvent::Handled);
        assert_eq!(s.cursor, 1);
        assert_eq!(
            s.handle(key(KeyCode::Enter), &state),
            ScreenEvent::Select(Selection::Output(0))
        );
        assert_eq!(
            s.handle(key(KeyCode::PageDown), &state),
            ScreenEvent::Handled
        );
        assert_eq!(s.handle(key(KeyCode::PageUp), &state), ScreenEvent::Handled);
        for _ in 0..20 {
            s.handle(key(KeyCode::Down), &state);
        }
        assert_eq!(s.cursor, 5, "clamped to the last card");
        assert_eq!(
            s.handle(key(KeyCode::Char('z')), &state),
            ScreenEvent::Unhandled
        );
    }

    #[test]
    fn every_advertised_key_is_handled() {
        let state = fixture::state();
        for help in KEYS {
            for k in crate::screens::tests::keys_for(help.key) {
                let mut s = Overview::new(shared());
                s.cursor = 2;
                let before = s.cursor;
                let ev = s.handle(k, &state);
                assert!(
                    ev != ScreenEvent::Unhandled || s.cursor != before,
                    "{:?} does nothing (from {:?})",
                    k.code,
                    help.key
                );
            }
        }
    }
}
