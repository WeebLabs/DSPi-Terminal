//! Settings > Display > Spectrum Analyser (`SpectrumSettingsTab`,
//! `DSPi_ConsoleApp.swift:1620-1782`).
//!
//! Two kinds of setting sit together, as in the Console. How the spectrum is
//! drawn is the Terminal's own business. The transform size, the averaging
//! and the peak decay belong to the device, but the analyser forgets them at
//! every power cycle, so they live in the settings file too and the runner
//! pushes them on connect and whenever they change here.
//!
//! The page is always listed. Without an analyser it says so in the Console's
//! words and still edits the file, so the values are ready for a firmware
//! that has one.

use crate::widgets::{Action, BannerKind, KeyHelp};

use super::config::Spectrum;
use super::{Cx, PageEvent, Row, SettingsPage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Strength,
    PeakHold,
    Smoothing,
    Floor,
    Ceiling,
    Transform,
    Averaging,
    PeakDecay,
}

const KEYS: &[KeyHelp] = &[
    KeyHelp::new("↑ ↓", "Move between controls"),
    KeyHelp::new("← →", "Change"),
    KeyHelp::new("Space", "Toggle"),
    KeyHelp::new("Tab", "Sidebar, page, save bar"),
    KeyHelp::new("[ ]", "Back, forward"),
    KeyHelp::new("Esc", "Leave Settings"),
];

#[derive(Default)]
pub struct SpectrumPage {
    cursor: usize,
}

/// The transform sizes this device offers, from its caps when they have been
/// read and from the protocol's own range before that, so the picker never
/// offers a size the device would refuse (`availableOrders`,
/// DSPi_ConsoleApp.swift:1628-1636).
fn orders(cx: &Cx<'_>) -> Vec<u8> {
    use dspi_session::rta::{ORDER_MAX, ORDER_MIN};
    match cx.data.rta {
        Some(c) if c.order_max >= c.order_min => (c.order_min..=c.order_max).collect(),
        _ => (ORDER_MIN..=ORDER_MAX).collect(),
    }
}

/// The order the picker shows: the stored one when offered, the device's
/// default otherwise, and the nearest offered size before the caps are read.
fn shown_order(s: &Spectrum, cx: &Cx<'_>) -> u8 {
    let offered = orders(cx);
    let (lo, hi) = (offered[0], offered[offered.len() - 1]);
    match (s.transform_order, cx.data.rta) {
        (Some(o), _) => o.clamp(lo, hi),
        (None, Some(c)) => c.order_default.clamp(lo, hi),
        (None, None) => hi,
    }
}

fn position<T: PartialEq>(values: &[T], v: &T) -> usize {
    values.iter().position(|x| x == v).unwrap_or(0)
}

fn pick(
    item: Item,
    label: &str,
    choices: Vec<String>,
    selected: usize,
    enabled: bool,
) -> (Option<Item>, Row) {
    (
        Some(item),
        Row::Pick {
            label: label.into(),
            choices,
            selected,
            caption: None,
            enabled,
        },
    )
}

impl SpectrumPage {
    fn build(&self, cx: &Cx<'_>) -> Vec<(Option<Item>, Row)> {
        let s = &cx.config.spectrum;
        let toggle = |item: Item, label: &str, on: bool, caption: &str| {
            (
                Some(item),
                Row::Toggle {
                    label: label.into(),
                    on,
                    caption: Some(caption.into()),
                    enabled: true,
                },
            )
        };
        let orders = orders(cx);
        let mut rows = vec![
            (None, Row::section("Display")),
            (
                Some(Item::Strength),
                Row::Number {
                    label: "Spectrum Strength".into(),
                    value: s.strength_pct as f64,
                    min: 30.0,
                    max: 100.0,
                    step: 5.0,
                    unit: "%".into(),
                    decimals: 0,
                    caption: Some(
                        "How far the fill comes forward behind the curves when the spectrum is \
                         drawn on the response graph."
                            .into(),
                    ),
                    enabled: true,
                },
            ),
            toggle(
                Item::PeakHold,
                "Peak Hold",
                s.peak_hold,
                "A cap above each band marking its recent maximum",
            ),
            toggle(
                Item::Smoothing,
                "Smoothing",
                s.smoothing,
                "Glides the spectrum between device frames the way the peak meters glide \
                 between polls. It matters most with several channels selected, where one \
                 channel refreshes only every few hundred milliseconds and the picture would \
                 otherwise step.",
            ),
            // The Console's footer names the graph's gear; here the channels
            // are chosen in the panel itself.
            (
                None,
                Row::note(
                    "Choose which channels to show, and whether to draw them as curves, bars, \
                     or both, in the Spectrum Analyser panel (A). The analyser only runs while \
                     something is watching it, so with the panel closed the device stops it and \
                     spends nothing.",
                ),
            ),
            (None, Row::Blank),
            (None, Row::section("Vertical Scale")),
            pick(
                Item::Floor,
                "Floor",
                Spectrum::FLOORS.iter().map(|f| format!("{f} dB")).collect(),
                position(&Spectrum::FLOORS, &s.floor_db),
                true,
            ),
            pick(
                Item::Ceiling,
                "Ceiling",
                Spectrum::CEILINGS
                    .iter()
                    .map(|c| {
                        if *c == 0 {
                            "0 dBFS".to_string()
                        } else {
                            format!("{c:+} dBFS")
                        }
                    })
                    .collect(),
                position(&Spectrum::CEILINGS, &s.ceiling_db),
                true,
            ),
            (
                None,
                Row::note(
                    "Six decibels of headroom above full scale is the default because \
                     upmix-derived rows and hot EQ can legitimately exceed 0 dBFS.",
                ),
            ),
            (None, Row::Blank),
            (None, Row::section("Engine")),
            (
                Some(Item::Transform),
                Row::Pick {
                    label: "Transform Size".into(),
                    choices: orders
                        .iter()
                        .map(|o| format!("{} points", 1u32 << o))
                        .collect(),
                    selected: position(&orders, &shown_order(s, cx)),
                    caption: Some(
                        "More points resolve lower frequencies, but a frame takes longer to \
                         fill, so each channel refreshes less often."
                            .into(),
                    ),
                    enabled: true,
                },
            ),
            pick(
                Item::Averaging,
                "Averaging",
                Spectrum::AVERAGING_MS
                    .iter()
                    .map(|ms| match ms {
                        0 => "Off".to_string(),
                        1000 => "1 s".into(),
                        3000 => "3 s".into(),
                        ms => format!("{ms} ms"),
                    })
                    .collect(),
                position(&Spectrum::AVERAGING_MS, &s.averaging_ms),
                true,
            ),
            pick(
                Item::PeakDecay,
                "Peak Decay",
                Spectrum::PEAK_DECAYS
                    .iter()
                    .map(|d| {
                        if *d == 0 {
                            "Off".to_string()
                        } else {
                            format!("{d} dB/s")
                        }
                    })
                    .collect(),
                position(&Spectrum::PEAK_DECAYS, &s.peak_decay_db_s),
                s.peak_hold,
            ),
        ];
        match cx.data.rta {
            Some(c) => rows.push((
                None,
                Row::note(format!(
                    "This device reports {} dB of usable range and transforms up to {} points.",
                    c.dynamic_range_db,
                    1u32 << c.order_max
                )),
            )),
            None if cx.connected => rows.push((
                None,
                Row::Banner(
                    BannerKind::Warning,
                    "Spectrum analyser unavailable".into(),
                    "The connected firmware has no spectrum analyser, so these settings have \
                     nothing to apply to until it is updated."
                        .into(),
                ),
            )),
            None => {}
        }
        rows
    }

    fn item(&self, index: usize, cx: &Cx<'_>) -> Option<Item> {
        self.build(cx)
            .into_iter()
            .filter(|(_, r)| r.focusable())
            .nth(index)
            .and_then(|(i, _)| i)
    }
}

impl SettingsPage for SpectrumPage {
    fn rows(&self, cx: &Cx<'_>) -> Vec<Row> {
        self.build(cx).into_iter().map(|(_, r)| r).collect()
    }

    fn act(&mut self, index: usize, action: Action, cx: &Cx<'_>) -> PageEvent {
        let Some(item) = self.item(index, cx) else {
            return PageEvent::Handled;
        };
        let mut c = cx.config.clone();
        let s = &mut c.spectrum;
        match (item, action) {
            (Item::Strength, Action::Changed(v) | Action::Committed(v)) => {
                // The Console's slider steps in fives.
                s.strength_pct = ((v / 5.0).round() * 5.0).clamp(30.0, 100.0) as u8;
            }
            (Item::Strength, Action::Reset) => s.strength_pct = Spectrum::default().strength_pct,
            (Item::PeakHold, Action::Toggled(v)) => s.peak_hold = v,
            (Item::Smoothing, Action::Toggled(v)) => s.smoothing = v,
            (Item::Floor, Action::Selected(i)) => {
                s.floor_db = Spectrum::FLOORS.get(i).copied().unwrap_or(s.floor_db)
            }
            (Item::Ceiling, Action::Selected(i)) => {
                s.ceiling_db = Spectrum::CEILINGS.get(i).copied().unwrap_or(s.ceiling_db)
            }
            (Item::Transform, Action::Selected(i)) => {
                if let Some(o) = orders(cx).get(i) {
                    s.transform_order = Some(*o);
                }
            }
            (Item::Averaging, Action::Selected(i)) => {
                s.averaging_ms = Spectrum::AVERAGING_MS
                    .get(i)
                    .copied()
                    .unwrap_or(s.averaging_ms)
            }
            (Item::PeakDecay, Action::Selected(i)) => {
                s.peak_decay_db_s = Spectrum::PEAK_DECAYS
                    .get(i)
                    .copied()
                    .unwrap_or(s.peak_decay_db_s)
            }
            _ => return PageEvent::Handled,
        }
        if c == *cx.config {
            return PageEvent::Handled;
        }
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
    use super::super::tests::{data, frame, key, scratch, screen, state};
    use super::super::{Page, SettingsData};
    use super::*;
    use crate::settings::{AppConfig, SettingsScreen};
    use crate::shell::{Screen, ScreenEvent};
    use crossterm::event::KeyCode;

    fn with_caps() -> SettingsData {
        let mut d = data();
        d.rta = Some(crate::screens::spectrum::demo::caps());
        d
    }

    #[test]
    fn golden_frames_at_both_sizes() {
        let st = state();
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            let mut s = SettingsScreen::new(&st, with_caps(), AppConfig::default())
                .open(Page::Spectrum, &st);
            let f = frame(&mut s, &st, w, h);
            assert_eq!(f.lines().count(), h as usize);
            assert!(f.contains("DISPLAY"), "{w}x{h}:\n{f}");
            assert!(f.contains("Spectrum Strength"), "{w}x{h}:\n{f}");
            assert!(f.contains("100%"), "{w}x{h}:\n{f}");
            assert!(f.contains("Peak Hold"), "{w}x{h}:\n{f}");
            assert!(f.contains("Smoothing"), "{w}x{h}:\n{f}");
        }
        // The rest is further down.
        let mut s =
            SettingsScreen::new(&st, with_caps(), AppConfig::default()).open(Page::Spectrum, &st);
        s.handle(key(KeyCode::Tab), &st);
        for _ in 0..7 {
            s.handle(key(KeyCode::Down), &st);
        }
        let f = frame(&mut s, &st, 120, 40);
        for text in [
            "VERTICAL SCALE",
            "-90 dB",
            "+6 dBFS",
            "ENGINE",
            "1024 points",
            "300 ms",
            "12 dB/s",
            "This device reports 120 dB of usable range and transforms up to 1024 points.",
        ] {
            assert!(f.contains(text), "{text}:\n{f}");
        }
    }

    #[test]
    fn without_an_analyser_the_page_says_so_in_the_consoles_words() {
        let st = state();
        let (mut s, _) = screen(Page::Spectrum);
        s.handle(key(KeyCode::Tab), &st);
        for _ in 0..7 {
            s.handle(key(KeyCode::Down), &st);
        }
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("Spectrum analyser unavailable"), "{f}");
        assert!(
            f.contains("The connected firmware has no spectrum analyser"),
            "{f}"
        );
        // The sizes come from the protocol's own range meanwhile.
        let page = SpectrumPage::default();
        let d = data();
        let cfg = AppConfig::default();
        let cx = Cx {
            state: &st,
            data: &d,
            config: &cfg,
            connected: true,
            global_dirty: false,
        };
        assert_eq!(orders(&cx), vec![8, 9, 10]);
        assert!(page.rows(&cx).iter().any(|r| matches!(
            r,
            Row::Pick { label, selected: 2, .. } if label == "Transform Size"
        )));
    }

    #[test]
    fn the_pickers_offer_the_consoles_choices_and_defaults() {
        let st = state();
        let d = with_caps();
        let cfg = AppConfig::default();
        let cx = Cx {
            state: &st,
            data: &d,
            config: &cfg,
            connected: true,
            global_dirty: false,
        };
        let rows = SpectrumPage::default().rows(&cx);
        let pick = |name: &str| {
            rows.iter()
                .find_map(|r| match r {
                    Row::Pick {
                        label,
                        choices,
                        selected,
                        enabled,
                        ..
                    } if label == name => Some((choices.clone(), *selected, *enabled)),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{name}"))
        };
        assert_eq!(
            pick("Floor"),
            (
                vec!["-60 dB".into(), "-90 dB".into(), "-120 dB".into()],
                1,
                true
            )
        );
        assert_eq!(
            pick("Ceiling"),
            (
                vec!["0 dBFS".into(), "+6 dBFS".into(), "+12 dBFS".into()],
                1,
                true
            )
        );
        assert_eq!(
            pick("Transform Size").0,
            vec!["256 points", "512 points", "1024 points"]
        );
        assert_eq!(pick("Transform Size").1, 2, "the caps default");
        assert_eq!(
            pick("Averaging"),
            (
                ["Off", "50 ms", "125 ms", "300 ms", "1 s", "3 s"]
                    .map(String::from)
                    .to_vec(),
                3,
                true
            )
        );
        assert_eq!(
            pick("Peak Decay"),
            (
                ["Off", "4 dB/s", "12 dB/s", "30 dB/s"]
                    .map(String::from)
                    .to_vec(),
                2,
                true
            )
        );

        // Peak Decay is disabled without Peak Hold.
        let mut off = AppConfig::default();
        off.spectrum.peak_hold = false;
        let cx = Cx { config: &off, ..cx };
        let rows = SpectrumPage::default().rows(&cx);
        assert!(rows.iter().any(|r| matches!(
            r,
            Row::Pick { label, enabled: false, .. } if label == "Peak Decay"
        )));
    }

    #[test]
    fn a_change_is_saved_and_handed_to_the_runner() {
        let path = scratch("spectrum");
        let st = state();
        let seen = std::rc::Rc::new(std::cell::RefCell::new(None));
        let sink = seen.clone();
        let mut s = SettingsScreen::new(&st, with_caps(), AppConfig::default())
            .config_path(path.clone())
            .on_config(move |c: &AppConfig| *sink.borrow_mut() = Some(c.spectrum.clone()))
            .open(Page::Spectrum, &st);
        s.handle(key(KeyCode::Tab), &st);
        // Strength is first: one step down is 95 %.
        match s.handle(key(KeyCode::Left), &st) {
            ScreenEvent::Status(m) => assert!(m.starts_with("Saved "), "{m}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(AppConfig::load_from(&path).spectrum.strength_pct, 95);
        assert_eq!(seen.borrow().as_ref().unwrap().strength_pct, 95);
        // Transform Size, the sixth control: 512 points.
        for _ in 0..5 {
            s.handle(key(KeyCode::Down), &st);
        }
        s.handle(key(KeyCode::Left), &st);
        let back = AppConfig::load_from(&path).spectrum;
        assert_eq!(back.transform_order, Some(9));
        assert_eq!(back.engine_options().fft_order, Some(9));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
