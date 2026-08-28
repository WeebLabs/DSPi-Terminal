//! Settings > Graphing.
//!
//! The one page that touches no device at all: it edits the app's own settings
//! file (see [`config`](super::config)) and the live runner reads it back on
//! start. The Console's four sections, in its order, with its ranges and
//! steps.
//!
//! Two of the Console's controls are carried but have no terminal effect yet:
//! the line glow and the animation speed. They are kept rather than dropped so
//! the file round-trips and a phase that gives the curve a draw-in has
//! somewhere to read its timing from.

use crate::graph::GraphSettings;
use crate::widgets::{Action, KeyHelp};

use super::{AppConfig, Cx, PageEvent, Row, SettingsPage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Glow,
    Phase,
    Unwrap,
    LineWidth,
    Animation,
    FreqGrid,
    FreqLabels,
    DbGrid,
    DbLabels,
    Range,
    Center,
    MinFreq,
    MaxFreq,
    Popout,
    VolumeMode,
}

/// The Console's Min / Max Frequency menus.
const MIN_FREQS: [f64; 5] = GraphSettings::MIN_FREQS;
const MAX_FREQS: [f64; 3] = GraphSettings::MAX_FREQS;

pub struct GraphingPage {
    cursor: usize,
    /// The page edits a copy and hands the whole thing back on every change,
    /// which is what makes one write per keystroke safe: the file is small and
    /// the shell reports where it went.
    config: AppConfig,
}

const KEYS: &[KeyHelp] = &[
    KeyHelp::new("↑ ↓", "Move between controls"),
    KeyHelp::new("← →", "Change"),
    KeyHelp::new("Space", "Toggle"),
    KeyHelp::new("Tab", "Sidebar, page, save bar"),
    KeyHelp::new("[ ]", "Back, forward"),
    KeyHelp::new("Esc", "Leave Settings"),
];

fn nearest(values: &[f64], v: f64) -> usize {
    values
        .iter()
        .enumerate()
        .min_by(|a, b| {
            (a.1 - v)
                .abs()
                .partial_cmp(&(b.1 - v).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
        .unwrap_or(0)
}

fn hz_label(v: f64) -> String {
    if v >= 1000.0 {
        format!("{} kHz", v / 1000.0)
    } else {
        format!("{v:.0} Hz")
    }
}

impl GraphingPage {
    pub fn new(config: AppConfig) -> Self {
        Self { cursor: 0, config }
    }

    /// Take the settings the shell holds, after it has written them out.
    pub fn adopt(&mut self, config: AppConfig) {
        self.config = config;
    }

    fn build(&self) -> Vec<(Option<Item>, Row)> {
        let g = &self.config.graphing;
        let toggle = |item: Item, label: &str, on: bool, caption: Option<&str>, enabled: bool| {
            (
                Some(item),
                Row::Toggle {
                    label: label.into(),
                    on,
                    caption: caption.map(str::to_string),
                    enabled,
                },
            )
        };
        vec![
            (None, Row::section("Graph Appearance")),
            toggle(
                Item::Glow,
                "Graph Line Glow",
                g.line_glow,
                Some("Add a neon glow effect to frequency response curves"),
                true,
            ),
            toggle(
                Item::Phase,
                "Show Phase Response",
                g.show_phase,
                Some("Overlay the selected channel's phase (degrees) as a dotted line"),
                true,
            ),
            toggle(
                Item::Unwrap,
                "Unwrap Phase",
                g.unwrap_phase,
                Some("Show continuous phase instead of wrapping at ±180°"),
                g.show_phase,
            ),
            (None, Row::Blank),
            (None, Row::section("Response Curve")),
            (
                Some(Item::LineWidth),
                Row::Number {
                    label: "Line Width".into(),
                    value: g.line_width,
                    min: 1.0,
                    max: 4.0,
                    step: 0.5,
                    unit: "pt".into(),
                    decimals: 1,
                    caption: None,
                    enabled: true,
                },
            ),
            (
                Some(Item::Animation),
                Row::Number {
                    label: "Animation Speed".into(),
                    value: g.animation_speed,
                    min: 0.1,
                    max: 0.5,
                    step: 0.05,
                    unit: "s".into(),
                    decimals: 2,
                    caption: None,
                    enabled: true,
                },
            ),
            (None, Row::Blank),
            (None, Row::section("Scale & Grid")),
            toggle(
                Item::FreqGrid,
                "Show Frequency Grid",
                g.freq_grid,
                None,
                true,
            ),
            toggle(
                Item::FreqLabels,
                "Show Frequency Labels",
                g.freq_labels,
                None,
                true,
            ),
            toggle(Item::DbGrid, "Show dB Grid", g.db_grid, None, true),
            toggle(Item::DbLabels, "Show dB Labels", g.db_labels, None, true),
            (
                Some(Item::Range),
                Row::Number {
                    label: "Vertical Range".into(),
                    value: g.db_range,
                    min: 10.0,
                    max: 100.0,
                    step: 1.0,
                    unit: "dB".into(),
                    decimals: 0,
                    caption: None,
                    enabled: true,
                },
            ),
            (
                Some(Item::Center),
                Row::Number {
                    label: "Center".into(),
                    value: g.db_center,
                    min: -40.0,
                    max: 20.0,
                    step: 1.0,
                    unit: "dB".into(),
                    decimals: 0,
                    caption: Some(format!(
                        "{:+.0} to {:+.0} dB",
                        g.db_center + g.db_range / 2.0,
                        g.db_center - g.db_range / 2.0
                    )),
                    enabled: true,
                },
            ),
            (
                Some(Item::MinFreq),
                Row::Pick {
                    label: "Min Frequency".into(),
                    choices: MIN_FREQS.iter().map(|f| hz_label(*f)).collect(),
                    selected: nearest(&MIN_FREQS, g.min_hz),
                    caption: None,
                    enabled: true,
                },
            ),
            (
                Some(Item::MaxFreq),
                Row::Pick {
                    label: "Max Frequency".into(),
                    choices: MAX_FREQS.iter().map(|f| hz_label(*f)).collect(),
                    selected: nearest(&MAX_FREQS, g.max_hz),
                    caption: None,
                    enabled: true,
                },
            ),
            (None, Row::Blank),
            (None, Row::section("Pop-out Window")),
            toggle(
                Item::Popout,
                "Pop-out graph follows channel selection",
                g.popout_follows_selection,
                None,
                true,
            ),
            (None, Row::Blank),
            (None, Row::section("Volume")),
            (
                Some(Item::VolumeMode),
                Row::Pick {
                    label: "Slider".into(),
                    choices: super::config::VolumeChoice::CHOICES
                        .iter()
                        .map(|s| (*s).to_string())
                        .collect(),
                    selected: self.config.volume.mode.index(),
                    caption: Some(
                        "Which volume the sidebar's slider drives when the app starts.".into(),
                    ),
                    enabled: true,
                },
            ),
        ]
    }

    fn item(&self, index: usize) -> Option<Item> {
        self.build()
            .into_iter()
            .filter(|(_, r)| r.focusable())
            .nth(index)
            .and_then(|(i, _)| i)
    }
}

impl SettingsPage for GraphingPage {
    fn rows(&self, _cx: &Cx<'_>) -> Vec<Row> {
        self.build().into_iter().map(|(_, r)| r).collect()
    }

    fn act(&mut self, index: usize, action: Action, _cx: &Cx<'_>) -> PageEvent {
        let Some(item) = self.item(index) else {
            return PageEvent::Handled;
        };
        let mut c = self.config.clone();
        let g = &mut c.graphing;
        match (item, action) {
            (Item::Glow, Action::Toggled(v)) => g.line_glow = v,
            (Item::Phase, Action::Toggled(v)) => {
                g.show_phase = v;
                // The Console disables Unwrap Phase with the phase off; a
                // disabled row cannot be flipped, so nothing else is needed.
            }
            (Item::Unwrap, Action::Toggled(v)) => g.unwrap_phase = v,
            (Item::FreqGrid, Action::Toggled(v)) => g.freq_grid = v,
            (Item::FreqLabels, Action::Toggled(v)) => g.freq_labels = v,
            (Item::DbGrid, Action::Toggled(v)) => g.db_grid = v,
            (Item::DbLabels, Action::Toggled(v)) => g.db_labels = v,
            (Item::Popout, Action::Toggled(v)) => g.popout_follows_selection = v,
            (Item::LineWidth, Action::Changed(v) | Action::Committed(v)) => g.line_width = v,
            (Item::Animation, Action::Changed(v) | Action::Committed(v)) => g.animation_speed = v,
            (Item::Range, Action::Changed(v) | Action::Committed(v)) => g.db_range = v.round(),
            (Item::Center, Action::Changed(v) | Action::Committed(v)) => g.db_center = v.round(),
            (Item::MinFreq, Action::Selected(i)) => {
                g.min_hz = MIN_FREQS.get(i).copied().unwrap_or(g.min_hz)
            }
            (Item::MaxFreq, Action::Selected(i)) => {
                g.max_hz = MAX_FREQS.get(i).copied().unwrap_or(g.max_hz)
            }
            (Item::LineWidth | Item::Animation | Item::Range | Item::Center, Action::Reset) => {
                let d = super::config::Graphing::default();
                match item {
                    Item::LineWidth => g.line_width = d.line_width,
                    Item::Animation => g.animation_speed = d.animation_speed,
                    Item::Range => g.db_range = d.db_range,
                    _ => g.db_center = d.db_center,
                }
            }
            (Item::VolumeMode, Action::Selected(i)) => {
                c.volume.mode = super::config::VolumeChoice::from_index(i)
            }
            // `Enter` on a picker opens its list; the two here have three and
            // five entries, so cycling with the arrows is the whole gesture.
            _ => return PageEvent::Handled,
        }
        if c == self.config {
            return PageEvent::Handled;
        }
        self.config = c.clone();
        PageEvent::Config(Box::new(c))
    }

    fn cursor(&self) -> usize {
        self.cursor
    }

    fn set_cursor(&mut self, i: usize) {
        self.cursor = i;
    }

    fn keys(&self) -> &'static [KeyHelp] {
        KEYS
    }
}

#[cfg(test)]
mod tests {
    use super::super::Page;
    use super::super::tests::{frame, key, scratch, screen};
    use super::*;
    use crate::shell::{Screen, ScreenEvent};
    use crossterm::event::KeyCode;

    #[test]
    fn graphing_carries_the_consoles_four_sections() {
        let (mut s, st) = screen(Page::Graphing);
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            let f = frame(&mut s, &st, w, h);
            assert_eq!(f.lines().count(), h as usize);
            assert!(f.contains("GRAPH APPEARANCE"), "{w}x{h}: {f}");
            assert!(f.contains("Graph Line Glow"), "{w}x{h}: {f}");
            assert!(f.contains("Show Phase Response"), "{w}x{h}: {f}");
            assert!(f.contains("Unwrap Phase"), "{w}x{h}: {f}");
        }
        // Scrolling down reaches the rest.
        let (mut s, st) = screen(Page::Graphing);
        s.handle(key(KeyCode::Tab), &st);
        for _ in 0..12 {
            s.handle(key(KeyCode::Down), &st);
        }
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("SCALE & GRID"), "{f}");
        assert!(f.contains("Min Frequency"), "{f}");
        assert!(f.contains("Max Frequency"), "{f}");
    }

    #[test]
    fn unwrap_phase_is_disabled_until_the_phase_is_on() {
        let mut p = GraphingPage::new(AppConfig::default());
        let rows = p.build();
        let unwrap = rows
            .iter()
            .find(|(i, _)| *i == Some(Item::Unwrap))
            .map(|(_, r)| r)
            .unwrap();
        assert!(matches!(unwrap, Row::Toggle { enabled: false, .. }));
        assert!(matches!(rows[3].1, Row::Toggle { enabled: false, .. }));
        p.config.graphing.show_phase = true;
        assert!(matches!(p.build()[3].1, Row::Toggle { enabled: true, .. }));
    }

    #[test]
    fn a_change_writes_the_settings_file_and_the_graph_reads_it_back() {
        let path = scratch("graphing");
        let st = super::super::tests::state();
        let mut s = super::super::SettingsScreen::new(
            &st,
            super::super::tests::data(),
            AppConfig::default(),
        )
        .config_path(path.clone())
        .open(Page::Graphing, &st);
        s.handle(key(KeyCode::Tab), &st);
        s.handle(key(KeyCode::Down), &st);
        match s.handle(key(KeyCode::Char(' ')), &st) {
            ScreenEvent::Status(m) => assert!(m.starts_with("Saved "), "{m}"),
            other => panic!("{other:?}"),
        }
        let back = AppConfig::load_from(&path);
        assert!(back.graphing.show_phase);
        assert!(GraphSettings::from_config(&back.graphing).show_phase);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn the_frequency_pickers_offer_the_consoles_choices() {
        let p = GraphingPage::new(AppConfig::default());
        let rows = p.build();
        let min = rows
            .iter()
            .find(|(i, _)| *i == Some(Item::MinFreq))
            .map(|(_, r)| r)
            .unwrap();
        match min {
            Row::Pick {
                choices, selected, ..
            } => {
                assert_eq!(choices, &["10 Hz", "15 Hz", "20 Hz", "50 Hz", "100 Hz"]);
                assert_eq!(*selected, 1, "15 Hz is the default");
            }
            other => panic!("{other:?}"),
        }
        let max = rows
            .iter()
            .find(|(i, _)| *i == Some(Item::MaxFreq))
            .map(|(_, r)| r)
            .unwrap();
        match max {
            Row::Pick {
                choices, selected, ..
            } => {
                assert_eq!(choices, &["5 kHz", "10 kHz", "20 kHz"]);
                assert_eq!(*selected, 2);
            }
            other => panic!("{other:?}"),
        }
    }
}
