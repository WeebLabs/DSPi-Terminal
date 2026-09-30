//! The Spectrum Analyser panel: the Console's analyser window
//! (`SpectrumAnalyserView.swift:1488-1902`) as a tool panel on `A`.
//!
//! The Console's window mirrors a channel chooser that lives in the response
//! graph's gear popover (`:820-960`). The Terminal has no gear, so the panel
//! carries the chooser itself: an Inputs / Outputs switch, one chip per
//! channel in its colour, the Console's summary and Clear. Beneath it the
//! picture as Curves, Bars or Both, and the window's status line at the foot.
//!
//! The engine that talks to the device ([`dspi_session::rta::RtaEngine`])
//! lives in the shared state, not here: the runner ticks it inside its own
//! loop and budget, and a later overlay or strip subscribes to the same
//! engine. The panel subscribes while it is open and has a channel chosen,
//! and releases on close, which is what makes the device stop the analyser.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crossterm::event::KeyEvent;
use dspi_proto::dsp;
use dspi_session::DeviceState;
use dspi_session::rta::{Bins, Request, RtaEngine, TAP_INPUT, TAP_OUTPUT, ViewId};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use super::Shared;
use super::panel::{self, Body, ChipSpec, Row};
use crate::graph::{Graph, GraphCurve, GraphSettings};
use crate::settings::config::Spectrum as Settings;
use crate::shell::{Screen, ScreenEvent};
use crate::theme::{ChannelRole, Glyphs, Theme};
use crate::widgets::text::{fit_centre, truncate, wrap};
use crate::widgets::{Action, ChipState, KeyHelp};

// ---------------------------------------------------------------------------
// What the panels share
// ---------------------------------------------------------------------------

/// The analyser's half of the shared state: the engine, the Settings page's
/// values, and the channels chosen, which outlive the panel as the Console's
/// dashboard selection outlives its window.
#[derive(Debug, Clone, Default)]
pub struct SpectrumState {
    pub engine: RtaEngine,
    pub settings: Settings,
    /// The chosen side and channels; `None` until someone chooses.
    pub selection: Option<Selection>,
    /// The side not showing, as it was left, so switching back restores it
    /// (`switchingSides`, SpectrumAnalyserView.swift:674-688).
    pub other_side: Option<Selection>,
}

impl SpectrumState {
    /// Take the Settings page's values, pushing the device's half on the
    /// next tick.
    pub fn adopt_settings(&mut self, s: &Settings) {
        self.settings = s.clone();
        self.engine.set_options(s.engine_options());
    }
}

/// Some channels at one tap: the device has one FFT engine listening at one
/// tap at a time, so inputs and outputs never mix (`RtaChannelSelection`,
/// SpectrumAnalyserView.swift:615-660).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub tap: u8,
    /// Channels at `tap` (input row, or output index), ascending.
    pub channels: Vec<u8>,
}

impl Selection {
    pub fn new(tap: u8, mut channels: Vec<u8>) -> Self {
        channels.sort_unstable();
        channels.dedup();
        Self { tap, channels }
    }

    pub fn mask(&self) -> u16 {
        self.channels
            .iter()
            .filter(|c| **c < 16)
            .fold(0, |m, c| m | 1 << c)
    }

    pub fn toggling(&self, channel: u8) -> Self {
        let mut channels = self.channels.clone();
        match channels.iter().position(|c| *c == channel) {
            Some(i) => {
                channels.remove(i);
            }
            None => channels.push(channel),
        }
        Self::new(self.tap, channels)
    }

    /// Only the channels in `live`, keeping the tap.
    pub fn restricted(&self, live: &[u8]) -> Self {
        Self::new(
            self.tap,
            self.channels
                .iter()
                .copied()
                .filter(|c| live.contains(c))
                .collect(),
        )
    }
}

/// The channels the analyser can show at `tap`: every live input row, or
/// every enabled output. Clamped to what the caps report, because a mask bit
/// for a channel the device lacks is a refused configuration (`rtaChannels`,
/// SpectrumAnalyserView.swift:691-699).
///
/// The live input rows are the device's: the active input count, and past
/// the stereo pair the rows the upmixer derives while it runs on one
/// (rta.c:95-106). The Console lists its own input count there; the
/// Terminal has no host-side count, and the device analyses no other rows.
pub fn channels(state: &DeviceState, engine: &RtaEngine, tap: u8) -> Vec<u8> {
    let reported = engine
        .caps()
        .map(|c| {
            if tap == TAP_INPUT {
                c.input_channels
            } else {
                c.output_channels
            }
        })
        .filter(|n| *n > 0)
        .unwrap_or(16)
        .min(16);
    if tap == TAP_INPUT {
        // Zero until the first meter read says otherwise.
        let live = match state.meters.active_inputs {
            0 => state.caps.num_inputs,
            n => n.saturating_add(super::matrix::derived_rows(state) as u8),
        };
        (0..live.min(state.caps.num_inputs).min(reported)).collect()
    } else {
        (0..state.caps.num_outputs.min(reported))
            .filter(|o| state.output(*o as usize).enabled)
            .collect()
    }
}

/// The selection on show: the chosen one, limited to channels live on this
/// device. Never chosen, or every chosen channel gone, falls back to the
/// first enabled output and then input 1; a chosen empty selection stays
/// empty (`dashboardRtaSelection`, SpectrumAnalyserView.swift:710-724).
pub fn selection(state: &DeviceState, s: &SpectrumState) -> Selection {
    if let Some(chosen) = &s.selection {
        if chosen.channels.is_empty() {
            return chosen.clone();
        }
        let live = chosen.restricted(&channels(state, &s.engine, chosen.tap));
        if !live.channels.is_empty() {
            return live;
        }
    }
    match channels(state, &s.engine, TAP_OUTPUT).first() {
        Some(first) => Selection::new(TAP_OUTPUT, vec![*first]),
        None => Selection::new(TAP_INPUT, vec![0]),
    }
}

/// A channel at a tap as the rest of the interface numbers it.
fn global_channel(state: &DeviceState, tap: u8, channel: u8) -> usize {
    if tap == TAP_INPUT {
        channel as usize
    } else {
        state.caps.num_inputs as usize + channel as usize
    }
}

fn channel_color(state: &DeviceState, theme: &Theme, tap: u8, channel: u8) -> Color {
    let ch = global_channel(state, tap, channel);
    theme.role_color(ChannelRole::of(
        ch as u8,
        state.caps.num_inputs,
        state.caps.num_outputs,
    ))
}

fn channel_label(state: &DeviceState, tap: u8, channel: u8) -> String {
    if tap == TAP_INPUT {
        // An upmixer row by what it carries, as the matrix names it.
        return super::matrix::row_name(state, channel as usize);
    }
    super::channel_name(state, global_channel(state, tap, channel))
}

/// The Console's summary under the chips (SpectrumAnalyserView.swift:936-942).
fn summary(sel: &Selection) -> String {
    match sel.channels.len() {
        0 => "Spectrum hidden".into(),
        1 => "1 channel".into(),
        n => format!("{n} channels"),
    }
}

/// `rtaShortHz` (SpectrumAnalyserView.swift:21-23).
fn short_hz(hz: u16) -> String {
    if hz >= 1000 {
        format!("{}k", (hz as f64 / 1000.0).round() as u32)
    } else {
        hz.to_string()
    }
}

// ---------------------------------------------------------------------------
// Curves
// ---------------------------------------------------------------------------

/// One channel's band levels as a curve on the graph's frequency grid,
/// joined in log frequency between the bands that have a reading. Bands the
/// transform cannot resolve are left out rather than drawn at the floor, so
/// the curve starts where the measurement does (`bandPoints`,
/// GraphSpectrumOverlay.swift:344-400); outside the measured span the curve
/// is absent.
pub fn band_curve(levels: &[f64], centres: &[u16], populated: &dyn Fn(usize) -> bool) -> Vec<f64> {
    let points: Vec<(f64, f64)> = levels
        .iter()
        .zip(centres)
        .enumerate()
        .filter(|(i, (v, c))| populated(*i) && v.is_finite() && **c > 0)
        .map(|(_, (v, c))| ((*c as f64).log10(), *v))
        .collect();
    dsp::frequencies()
        .iter()
        .map(|f| interpolate(&points, f.log10()))
        .collect()
}

fn interpolate(points: &[(f64, f64)], x: f64) -> f64 {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return f64::NAN;
    };
    if x < first.0 || x > last.0 {
        return f64::NAN;
    }
    for w in points.windows(2) {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        if x <= x1 {
            let t = if x1 > x0 { (x - x0) / (x1 - x0) } else { 0.0 };
            return y0 + (y1 - y0) * t;
        }
    }
    last.1
}

/// One channel from both products: the bands below the top of the bass
/// bank, where the bins are too coarse to resolve a third-octave, and the
/// bins above it, crossfaded over the next three bands so there is no step
/// (`blend`, GraphSpectrumOverlay.swift:397-438). Where only one product has
/// data it is used alone.
pub fn blend(bands: &[f64], bins: &Bins, centres: &[u16], bass_bands: usize) -> Vec<f64> {
    let (lo, hi) = if bass_bands > 0 && bass_bands <= centres.len() {
        (
            (centres[bass_bands - 1] as f64).log10(),
            (centres[(bass_bands + 2).min(centres.len() - 1)] as f64).log10(),
        )
    } else {
        (f64::NEG_INFINITY, f64::NEG_INFINITY)
    };
    dsp::frequencies()
        .iter()
        .zip(bands)
        .map(|(f, band)| {
            let bin = bins.level_at(*f);
            match (band.is_finite(), bin) {
                (true, Some(b)) => {
                    let w = if hi > lo {
                        ((f.log10() - lo) / (hi - lo)).clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                    band + (b - band) * w
                }
                (true, None) => *band,
                (false, Some(b)) => b,
                (false, None) => f64::NAN,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Smoothing between frames
// ---------------------------------------------------------------------------

/// Glides the picture between device frames, one pole per value, stepped by
/// real elapsed time (`RtaBarSmoother`, SpectrumAnalyserView.swift:128-190).
/// The fall is slower than the rise, and a peak cap never eases upward. A
/// new identity (another channel, another band count) snaps rather than
/// sliding across from the old picture.
#[derive(Debug, Default)]
struct Glide {
    series: HashMap<(u8, u8, u8), (Vec<f64>, Instant)>,
}

impl Glide {
    fn step(
        &mut self,
        key: (u8, u8, u8),
        target: Vec<f64>,
        now: Instant,
        rise: Duration,
        fall: Duration,
    ) -> Vec<f64> {
        let entry = self.series.entry(key).or_insert((Vec::new(), now));
        if entry.0.len() != target.len() {
            *entry = (target.clone(), now);
            return target;
        }
        let dt = now.saturating_duration_since(entry.1).as_secs_f64();
        if dt <= 0.0 {
            return entry.0.clone();
        }
        entry.1 = now;
        let k = |tau: Duration| {
            if tau.is_zero() {
                1.0
            } else {
                1.0 - (-dt / tau.as_secs_f64()).exp()
            }
        };
        let (rise_k, fall_k) = (k(rise), k(fall));
        for (v, t) in entry.0.iter_mut().zip(&target) {
            if !v.is_finite() || !t.is_finite() {
                *v = *t;
            } else {
                *v += (t - *v) * if t > v { rise_k } else { fall_k };
            }
        }
        entry.0.clone()
    }
}

/// The fall time for the Smoothing setting: about one refresh interval, so a
/// bar is still moving when the next frame lands, clamped at both ends
/// (`rtaFallTau` with the Console's 0.35, SpectrumAnalyserView.swift:
/// 201-210 and DSPi_ConsoleApp.swift:118).
fn fall_tau(refresh: Duration) -> Duration {
    Duration::from_secs_f64((refresh.as_secs_f64() * 0.35).clamp(0.035, 0.40))
}

// ---------------------------------------------------------------------------
// The panel
// ---------------------------------------------------------------------------

/// How the window draws the spectrum (`DisplayMode`,
/// SpectrumAnalyserView.swift:1576-1581).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Curves,
    Bars,
    Both,
}

const MODES: [&str; 3] = ["Curves", "Bars", "Both"];

const KEYS: &[KeyHelp] = &[
    KeyHelp::new("↑ ↓", "Move"),
    KeyHelp::new("← →", "Choose"),
    KeyHelp::new("Space", "Show or hide a channel"),
    KeyHelp::new("Enter", "Clear"),
];

/// One channel's picture, gathered for drawing.
struct Trace {
    color: Color,
    name: String,
    avg: Vec<f64>,
    peak: Option<Vec<f64>>,
    /// The curve on the graph's frequency grid.
    curve: Vec<f64>,
    peak_curve: Option<Vec<f64>>,
}

pub struct SpectrumPanel {
    shared: Shared,
    body: Body,
    chip: usize,
    mode: Mode,
    view: Option<ViewId>,
    glide: Glide,
}

impl SpectrumPanel {
    pub fn new(shared: Shared) -> Self {
        Self {
            shared,
            body: Body::headless(),
            chip: 0,
            mode: Mode::Curves,
            view: None,
            glide: Glide::default(),
        }
    }

    /// Open and start watching at once, so the next tick already reads.
    pub fn open(shared: Shared, state: &DeviceState) -> Self {
        let mut p = Self::new(shared);
        p.sync(state);
        p
    }

    pub fn mode(mut self, mode: Mode) -> Self {
        self.mode = mode;
        self
    }

    /// Keep the engine's subscription in step with what is shown: watching
    /// while a channel is chosen, released when none is.
    pub fn sync(&mut self, state: &DeviceState) {
        let Ok(mut app) = self.shared.try_borrow_mut() else {
            return;
        };
        let sel = selection(state, &app.spectrum);
        let engine = &mut app.spectrum.engine;
        if sel.channels.is_empty() || !engine.supported() || !state.connected {
            if let Some(id) = self.view.take() {
                engine.release(id);
            }
            return;
        }
        let request = Request {
            tap: sel.tap,
            mask: sel.mask(),
            wants_bins: sel.channels.len() == 1,
        };
        match self.view {
            Some(id) => engine.update(id, request),
            None => self.view = Some(engine.subscribe(request)),
        }
    }

    fn supported(&self, state: &DeviceState) -> bool {
        state.connected && self.shared.borrow().spectrum.engine.supported()
    }

    fn rows(&self, state: &DeviceState, theme: &Theme) -> Vec<Row> {
        let app = self.shared.borrow();
        let sel = selection(state, &app.spectrum);
        let available = channels(state, &app.spectrum.engine, sel.tap);
        let mut rows = vec![Row::Segmented {
            label: "Spectrum".into(),
            choices: vec!["Inputs".into(), "Outputs".into()],
            selected: usize::from(sel.tap == TAP_OUTPUT),
            enabled: true,
        }];
        if available.is_empty() {
            rows.push(Row::Caption(
                if sel.tap == TAP_INPUT {
                    "No active inputs."
                } else {
                    "No enabled outputs."
                }
                .into(),
            ));
        } else {
            rows.push(Row::Chips {
                chips: available
                    .iter()
                    .map(|ch| ChipSpec {
                        label: channel_label(state, sel.tap, *ch),
                        state: if sel.channels.contains(ch) {
                            ChipState::On
                        } else {
                            ChipState::Off
                        },
                        color: channel_color(state, theme, sel.tap, *ch),
                        enabled: true,
                        dimmed: false,
                    })
                    .collect(),
                cursor: self.chip.min(available.len() - 1),
                polarity: false,
            });
        }
        rows.push(Row::Buttons {
            label: summary(&sel),
            buttons: if sel.channels.is_empty() {
                Vec::new()
            } else {
                vec!["Clear".into()]
            },
            cursor: 0,
        });
        rows.push(Row::Segmented {
            label: "Display".into(),
            choices: MODES.iter().map(|m| m.to_string()).collect(),
            selected: self.mode as usize,
            enabled: true,
        });
        rows
    }

    fn act(&mut self, row: &Row, action: Action, state: &DeviceState) -> ScreenEvent {
        let mut app = self.shared.borrow_mut();
        let sel = selection(state, &app.spectrum);
        let available = channels(state, &app.spectrum.engine, sel.tap);
        let s = &mut app.spectrum;
        match (row, action) {
            (Row::Segmented { label, .. }, Action::Selected(i)) if label == "Display" => {
                self.mode = [Mode::Curves, Mode::Bars, Mode::Both][i.min(2)];
            }
            (Row::Segmented { .. }, Action::Selected(i)) => {
                // Choosing a side brings back what was checked there when it
                // was last left, or nothing.
                let tap = if i == 0 { TAP_INPUT } else { TAP_OUTPUT };
                if tap != sel.tap {
                    let restored = s
                        .other_side
                        .take()
                        .filter(|o| o.tap == tap)
                        .unwrap_or_else(|| Selection::new(tap, Vec::new()));
                    s.other_side = Some(s.selection.take().unwrap_or(sel));
                    s.selection = Some(restored);
                    self.chip = 0;
                }
            }
            (Row::Chips { .. }, Action::Selected(i)) => self.chip = i,
            (Row::Chips { .. }, Action::Chip(i, _)) => {
                if let Some(ch) = available.get(i) {
                    s.selection = Some(sel.toggling(*ch));
                }
            }
            (Row::Buttons { buttons, .. }, Action::Button(_)) if !buttons.is_empty() => {
                s.selection = Some(Selection::new(sel.tap, Vec::new()));
            }
            _ => {}
        }
        drop(app);
        self.sync(state);
        ScreenEvent::Handled
    }

    /// Every chosen channel's picture, glided when Smoothing is on.
    fn traces(&mut self, state: &DeviceState, theme: &Theme, now: Instant) -> Vec<Trace> {
        let app = self.shared.borrow();
        let s = &app.spectrum;
        let engine = &s.engine;
        let sel = selection(state, s);
        let single = sel.channels.len() == 1;
        let populated = |i: usize| engine.band_populated(i);
        let fall = fall_tau(engine.refresh_interval());
        let rise = fall.mul_f64(0.4);
        let mut out = Vec::new();
        for ch in &sel.channels {
            let Some(frame) = engine.frame(sel.tap, *ch) else {
                out.push(Trace {
                    color: channel_color(state, theme, sel.tap, *ch),
                    name: channel_label(state, sel.tap, *ch),
                    avg: Vec::new(),
                    peak: None,
                    curve: Vec::new(),
                    peak_curve: None,
                });
                continue;
            };
            let avg: Vec<f64> = frame
                .avg_levels()
                .iter()
                .map(|v| engine.level_db(*v))
                .collect();
            let peak: Vec<f64> = frame
                .peak_levels()
                .iter()
                .map(|v| engine.level_db(*v))
                .collect();
            let (avg, peak) = if s.settings.smoothing {
                (
                    self.glide.step((sel.tap, *ch, 0), avg, now, rise, fall),
                    self.glide
                        .step((sel.tap, *ch, 1), peak, now, Duration::ZERO, fall),
                )
            } else {
                (avg, peak)
            };
            let bands = band_curve(&avg, engine.centres(), &populated);
            let curve = match engine.bins(sel.tap, *ch).filter(|_| single) {
                Some(bins) => {
                    let blended = blend(&bands, bins, engine.centres(), engine.bass_bands());
                    if s.settings.smoothing {
                        self.glide.step((sel.tap, *ch, 2), blended, now, rise, fall)
                    } else {
                        blended
                    }
                }
                None => bands,
            };
            let peak_on = s.settings.peak_hold;
            out.push(Trace {
                color: channel_color(state, theme, sel.tap, *ch),
                name: channel_label(state, sel.tap, *ch),
                peak_curve: peak_on.then(|| band_curve(&peak, engine.centres(), &populated)),
                peak: peak_on.then_some(peak),
                avg,
                curve,
            });
        }
        out
    }

    fn draw_curves(&self, area: Rect, buf: &mut Buffer, theme: &Theme, traces: &[Trace]) {
        let app = self.shared.borrow();
        let s = &app.spectrum.settings;
        let settings = GraphSettings {
            min_hz: app.graph.min_hz,
            max_hz: app.graph.max_hz,
            db_center: (s.floor_db + s.ceiling_db) as f64 / 2.0,
            db_range: (s.ceiling_db - s.floor_db) as f64,
            show_phase: false,
            unwrap_phase: false,
            freq_grid: true,
            freq_labels: true,
            db_grid: true,
            db_labels: true,
            grid: app.graph.grid,
            ..GraphSettings::default()
        };
        // Peak contours first, in grey, so every channel's own curve sits on
        // top of them.
        let mut curves: Vec<GraphCurve> = traces
            .iter()
            .filter_map(|t| t.peak_curve.as_ref())
            .map(|p| GraphCurve {
                descriptor: String::new(),
                color: theme.dim,
                magnitude: p.clone(),
                phase: None,
                selected: false,
            })
            .collect();
        curves.extend(
            traces
                .iter()
                .filter(|t| !t.curve.is_empty())
                .map(|t| GraphCurve {
                    descriptor: t.name.clone(),
                    color: t.color,
                    magnitude: t.curve.clone(),
                    phase: None,
                    selected: true,
                }),
        );
        let graph = Graph::new(&curves, &settings, theme);
        let plot = graph.plot_area(area);
        ratatui::widgets::Widget::render(graph, area, buf);
        // A cell has no opacity: a strength in the lower half of the
        // Console's slider draws the curves dim instead.
        if s.strength_pct < Settings::DIM_BELOW {
            for y in plot.y..plot.y + plot.height {
                for x in plot.x..plot.x + plot.width {
                    let cell = &mut buf[(x, y)];
                    if cell
                        .symbol()
                        .chars()
                        .next()
                        .is_some_and(|c| ('\u{2800}'..='\u{28FF}').contains(&c))
                    {
                        cell.modifier.insert(Modifier::DIM);
                    }
                }
            }
        }
    }

    fn draw_bars(&self, area: Rect, buf: &mut Buffer, theme: &Theme, traces: &[Trace]) {
        let n = traces.len();
        if n == 0 || area.height < 2 || area.width < 8 {
            return;
        }
        // The fewest columns that give every cell a title and four rows of
        // bars; failing that, as many columns as the Console allows.
        let fits = |c: usize| {
            let rows = n.div_ceil(c) as u16;
            (area.height / rows, area.width / c as u16)
        };
        let cols = (1..=n.min(4))
            .find(|c| fits(*c).0 >= 5 && fits(*c).1 >= 16)
            .unwrap_or(n.min(4));
        let (cell_h, cell_w) = fits(cols);
        let app = self.shared.borrow();
        let s = &app.spectrum;
        for (i, t) in traces.iter().enumerate() {
            let (r, c) = ((i / cols) as u16, (i % cols) as u16);
            if cell_h < 2 || (r + 1) * cell_h > area.height {
                break;
            }
            let cell = Rect::new(
                area.x + c * cell_w,
                area.y + r * cell_h,
                cell_w.saturating_sub(u16::from(c + 1 < cols as u16)),
                cell_h,
            );
            draw_bar_cell(cell, buf, theme, t, s, cell_h >= 6);
        }
    }

    fn draw_status(&self, area: Rect, buf: &mut Buffer, theme: &Theme, shows_bars: bool) {
        if area.height == 0 {
            return;
        }
        let app = self.shared.borrow();
        let engine = &app.spectrum.engine;
        let st = engine.status().copied().unwrap_or_default();
        let running = engine.running();
        let ascii = theme.glyphs == Glyphs::Ascii;
        let dot = match (running, ascii) {
            (true, true) => "*",
            (false, true) => "o",
            (true, false) => "●",
            (false, false) => "○",
        };
        buf.set_string(
            area.x + 1,
            area.y,
            dot,
            Style::default().fg(if running { theme.ok } else { theme.dim }),
        );
        let mut parts = vec![
            if running { "Running" } else { "Idle" }.to_string(),
            engine.refresh_description(),
        ];
        if st.frames_per_s > 0 {
            parts.push(format!("{} frames/s", st.frames_per_s));
        }
        if st.last_frame_us > 0 {
            parts.push(format!(
                "transform {} {}",
                st.last_frame_us,
                if ascii { "us" } else { "µs" }
            ));
        }
        if st.busy_us_per_s > 0 {
            // Ten thousand microseconds a second is one percent.
            parts.push(format!(
                "main loop {:.1}%",
                st.busy_us_per_s as f64 / 10_000.0
            ));
        }
        if st.bass_busy_us_per_s > 0 {
            let saturated = st.bass_busy_us_per_s == u16::MAX;
            parts.push(format!(
                "bass {}{:.2}%",
                match (saturated, ascii) {
                    (false, _) => "",
                    (true, true) => ">=",
                    (true, false) => "≥",
                },
                st.bass_busy_us_per_s as f64 / 10_000.0
            ));
        }
        // Whole items only: a telemetry figure cut in half reads as another
        // number. What does not fit the first line starts the second.
        let room = area.width.saturating_sub(3) as usize;
        let mut lines = [String::new(), String::new()];
        let mut row = 0;
        for p in parts {
            let next = if lines[row].is_empty() {
                p.clone()
            } else {
                format!("{}  {p}", lines[row])
            };
            if next.chars().count() <= room {
                lines[row] = next;
            } else if row == 0 && area.height >= 2 {
                row = 1;
                lines[1] = p;
            } else {
                break;
            }
        }
        let [line, rest] = lines;
        buf.set_string(area.x + 3, area.y, &line, theme.label());

        // The notice: a refusal, the remedy for shaded bands, or the range
        // this device can be trusted over (SpectrumAnalyserView.swift:1874-1885).
        let caps = engine.caps().copied().unwrap_or_default();
        let low_band = (shows_bars && running && engine.fft_order() < caps.order_max)
            .then(|| engine.lowest_measurable_centre())
            .flatten();
        let (notice, style) = if engine.rejected() {
            (
                "Device refused this configuration".to_string(),
                theme.warning_style(),
            )
        } else if let Some(hz) = low_band {
            (
                format!(
                    "shaded bands below {} Hz need a larger transform size in Settings",
                    short_hz(hz)
                ),
                theme.warning_style(),
            )
        } else if caps.dynamic_range_db > 0 {
            (
                format!(
                    "{}/{} dB range",
                    caps.dynamic_range_db, caps.bass_dynamic_range_db
                ),
                theme.label(),
            )
        } else {
            (String::new(), theme.label())
        };
        if notice.is_empty() {
            if area.height >= 2 {
                buf.set_string(area.x + 3, area.y + 1, &rest, theme.label());
            }
            return;
        }
        if area.height >= 2 {
            // The notice keeps the right of the second line; telemetry that
            // spilled over gives way to it when both will not fit.
            let w = notice.chars().count() as u16;
            let spill = rest.chars().count() as u16;
            if spill > 0 && 3 + spill + 2 + w <= area.width {
                buf.set_string(area.x + 3, area.y + 1, &rest, theme.label());
                buf.set_string(area.x + area.width - w - 1, area.y + 1, &notice, style);
            } else {
                buf.set_string(area.x + 3, area.y + 1, truncate(&notice, room), style);
            }
        } else {
            let w = notice.chars().count() as u16;
            let used = 3 + line.chars().count() as u16 + 2;
            if used + w <= area.width {
                buf.set_string(area.x + area.width - w, area.y, &notice, style);
            }
        }
    }

    fn draw_notice(area: Rect, buf: &mut Buffer, theme: &Theme, title: &str, body: &str) {
        if area.height == 0 {
            return;
        }
        let mid = area.y + area.height.saturating_sub(2) / 2;
        buf.set_string(
            area.x,
            mid,
            fit_centre(title, area.width as usize),
            theme.title(),
        );
        for (i, line) in wrap(body, area.width.saturating_sub(4) as usize, 3)
            .iter()
            .enumerate()
        {
            let y = mid + 1 + i as u16;
            if y >= area.y + area.height {
                break;
            }
            buf.set_string(
                area.x,
                y,
                fit_centre(line, area.width as usize),
                theme.label(),
            );
        }
    }
}

impl Drop for SpectrumPanel {
    /// Closing the panel releases its subscription; when it was the last,
    /// the engine stops the device on the next tick.
    fn drop(&mut self) {
        if let Some(id) = self.view.take()
            && let Ok(mut app) = self.shared.try_borrow_mut()
        {
            app.spectrum.engine.release(id);
        }
    }
}

/// One channel's third-octave bars: a title, equal-width bars on the
/// device's own band centres, the peak hold as a cap, and shading where the
/// transform has no bin (`RtaBandsView`, SpectrumAnalyserView.swift:236-420).
fn draw_bar_cell(
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    t: &Trace,
    s: &SpectrumState,
    labels: bool,
) {
    if area.height < 2 || area.width < 4 {
        return;
    }
    let ascii = theme.glyphs == Glyphs::Ascii;
    buf.set_string(
        area.x,
        area.y,
        if ascii { "*" } else { "●" },
        Style::default().fg(t.color),
    );
    buf.set_string(
        area.x + 2,
        area.y,
        truncate(&t.name, area.width.saturating_sub(3) as usize),
        theme.section(),
    );
    let plot_h = area.height - 1 - u16::from(labels);
    let plot = Rect::new(area.x, area.y + 1, area.width, plot_h);
    let n = t.avg.len();
    if n == 0 || plot_h == 0 {
        return;
    }
    let (floor, ceil) = (s.settings.floor_db as f64, s.settings.ceiling_db as f64);
    let norm = |db: f64| ((db - floor) / (ceil - floor).max(1.0)).clamp(0.0, 1.0);
    let engine = &s.engine;
    let w = plot.width as usize;
    // A wide cell spreads the bands across its width, band b from column
    // b w / n; with two columns a band or more, every bar is the same width
    // with a gap after it, and with fewer the bars touch rather than gap at
    // random. A narrow cell shows in each column the loudest band that lands
    // in it, so a tone is kept rather than averaged away.
    let first_col = |b: usize| (b * w).div_ceil(n);
    let bar_w = if w / n >= 2 { w / n - 1 } else { w };
    const BLOCKS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    for x in 0..w {
        let (b0, b1) = if w >= n {
            let b = x * n / w;
            if x - first_col(b) >= bar_w {
                continue;
            }
            (b, b + 1)
        } else {
            (x * n / w, ((x + 1) * n / w).max(x * n / w + 1))
        };
        let bands = b0..b1.min(n);
        let shaded = bands.clone().all(|b| !engine.band_populated(b));
        let px = plot.x + x as u16;
        if shaded {
            for y in 0..plot_h {
                buf[(px, plot.y + y)]
                    .set_symbol(if ascii { "." } else { "░" })
                    .set_style(Style::default().fg(theme.chrome_faint));
            }
            continue;
        }
        let level = bands.clone().map(|b| t.avg[b]).fold(f64::MIN, f64::max);
        let eighths = (norm(level) * plot_h as f64 * 8.0).round() as u16;
        for r in 0..plot_h {
            let fill = eighths.saturating_sub(r * 8).min(8);
            if fill == 0 {
                break;
            }
            let sym = match (ascii, fill) {
                (true, 4..) => "#",
                (true, _) => continue,
                (false, _) => BLOCKS[(fill - 1) as usize],
            };
            buf[(px, plot.y + plot_h - 1 - r)]
                .set_symbol(sym)
                .set_style(Style::default().fg(t.color));
        }
        if let Some(peak) = &t.peak {
            let p = bands
                .map(|b| peak.get(b).copied().unwrap_or(f64::MIN))
                .fold(f64::MIN, f64::max);
            let row = ((norm(p) * plot_h as f64).ceil() as u16).min(plot_h);
            // A cap only where it clears the bar.
            if row > 0 && row * 8 > eighths + 4 {
                buf[(px, plot.y + plot_h - row)]
                    .set_symbol(if ascii { "-" } else { "▔" })
                    .set_style(Style::default().fg(t.color));
            }
        }
    }
    if labels {
        let y = plot.y + plot_h;
        let centres = engine.centres();
        let mut last_end = 0u16;
        for (hz, label) in [(100u16, "100"), (1000, "1k"), (10_000, "10k")] {
            let Some(b) = centres.iter().position(|c| *c == hz).filter(|b| *b < n) else {
                continue;
            };
            let x = if w / n >= 2 {
                first_col(b) + bar_w / 2
            } else {
                first_col(b)
            } as u16;
            let start = plot.x + x.saturating_sub(label.len() as u16 / 2);
            if start < last_end || start + label.len() as u16 > plot.x + plot.width {
                continue;
            }
            buf.set_string(start, y, label, theme.label());
            last_end = start + label.len() as u16 + 1;
        }
    }
}

impl Screen for SpectrumPanel {
    fn title(&self) -> String {
        "Spectrum Analyser".into()
    }

    fn draw(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        state: &DeviceState,
        focused: bool,
    ) {
        if area.height == 0 {
            return;
        }
        self.sync(state);
        if !self.supported(state) {
            panel::draw_unsupported(
                area,
                buf,
                theme,
                "Spectrum analyser unavailable",
                if state.connected {
                    "The connected firmware does not provide a compatible analyser. Update the \
                     firmware to use it."
                } else {
                    "Connect a DSPi to use the analyser."
                },
            );
            return;
        }
        let rows = self.rows(state, theme);
        self.body.clamp(&rows);
        let heights: Vec<u16> = rows.iter().map(|r| r.height(area.width)).collect();
        let controls_h = heights.iter().sum::<u16>().min(area.height);
        let focus_row = self.body.focused_row(&rows);
        panel::draw_rows(
            Rect::new(area.x, area.y, area.width, controls_h),
            buf,
            theme,
            &rows,
            0,
            focus_row,
            focused,
            None,
        );
        let status_h = if area.height >= controls_h + 10 { 2 } else { 1 };
        let top = area.y + controls_h + 1;
        let bottom = (area.y + area.height).saturating_sub(status_h);
        if bottom <= top {
            return;
        }
        let display = Rect::new(area.x + 1, top, area.width.saturating_sub(2), bottom - top);
        let sel = selection(state, &self.shared.borrow().spectrum);
        let shows_bars = !sel.channels.is_empty() && self.mode != Mode::Curves;
        if sel.channels.is_empty() {
            Self::draw_notice(
                display,
                buf,
                theme,
                "No channels selected",
                "Choose channels above to see their spectrum. The analyser runs only while \
                 something is watching it.",
            );
        } else {
            let traces = self.traces(state, theme, Instant::now());
            match self.mode {
                Mode::Curves => self.draw_curves(display, buf, theme, &traces),
                Mode::Bars => self.draw_bars(display, buf, theme, &traces),
                Mode::Both => {
                    let upper = display.height / 2;
                    self.draw_curves(
                        Rect::new(display.x, display.y, display.width, upper),
                        buf,
                        theme,
                        &traces,
                    );
                    self.draw_bars(
                        Rect::new(
                            display.x,
                            display.y + upper,
                            display.width,
                            display.height - upper,
                        ),
                        buf,
                        theme,
                        &traces,
                    );
                }
            }
        }
        self.draw_status(
            Rect::new(area.x, bottom, area.width, status_h),
            buf,
            theme,
            shows_bars,
        );
    }

    fn handle(&mut self, key: KeyEvent, state: &DeviceState) -> ScreenEvent {
        if !self.supported(state) {
            return ScreenEvent::Unhandled;
        }
        let rows = self.rows(state, panel::key_theme());
        self.body.clamp(&rows);
        match panel::dispatch(&mut self.body, &rows, key) {
            panel::Step::Done(ev) => ev,
            panel::Step::Row(i, action) => self.act(&rows[i], action, state),
            panel::Step::Header | panel::Step::Commit(..) => ScreenEvent::Handled,
        }
    }

    fn keys(&self) -> &'static [KeyHelp] {
        KEYS
    }
}

// ---------------------------------------------------------------------------
// A picture without a device
// ---------------------------------------------------------------------------

/// Fill the shared analyser with an RP2350's caps and a plausible picture,
/// for the gallery and the golden frames: a pink-ish slope with a bump in
/// the bass on every channel at `tap`, bins for the first, and a running
/// status.
pub mod demo {
    use std::time::Instant;

    use dspi_proto::packets::{RtaBandFrame, RtaBinFrame, RtaBinHeader, RtaCaps, RtaStatus};
    use dspi_session::rta::TAP_OUTPUT;

    use super::{Selection, Shared};

    /// `rta_band_centre_tab` (rta_tables.h:818-822).
    pub const CENTRES: [u16; 37] = [
        10, 13, 16, 20, 25, 32, 40, 50, 63, 80, 100, 125, 160, 200, 250, 315, 400, 500, 630, 800,
        1000, 1250, 1600, 2000, 2500, 3150, 4000, 5000, 6300, 8000, 10000, 12500, 16000, 20000,
        25000, 31500, 40000,
    ];

    /// The RP2350's caps (survey-firmware-beta4 3.2).
    pub fn caps() -> RtaCaps {
        RtaCaps {
            input_channels: 8,
            output_channels: 9,
            order_min: 8,
            order_max: 10,
            order_default: 10,
            bass_bands: 14,
            max_bands: 37,
            level_zero: 243,
            dynamic_range_db: 120,
            idle_timeout_ms: 5000,
            max_bin_frame: 529,
            bass_dynamic_range_db: 70,
        }
    }

    /// A level byte for `db` dBFS (rta_fft.h:44-45).
    fn level(db: f64) -> u8 {
        (243.0 + db * 2.0).round().clamp(0.0, 255.0) as u8
    }

    /// Channel `ch`'s band picture: -18 dBFS in the bass falling 3 dB an
    /// octave, a little different per channel.
    pub fn frame(ch: u8) -> RtaBandFrame {
        let mut avg = [0u8; RtaBandFrame::MAX_BANDS];
        let mut peak = [0u8; RtaBandFrame::MAX_BANDS];
        for (i, (a, p)) in avg.iter_mut().zip(peak.iter_mut()).enumerate() {
            let octaves = (i as f64 - 8.0) / 3.0;
            let bump = if (5..=10).contains(&i) { 6.0 } else { 0.0 };
            let db = -24.0 - 3.0 * octaves.max(0.0) + bump - 2.0 * ch as f64;
            *a = level(db);
            *p = level(db + 6.0);
        }
        RtaBandFrame {
            channel: ch,
            seq: 1,
            n_bands: 34,
            age_ms: 12,
            avg,
            peak,
        }
    }

    /// The same shape at 1024 points, with a tone at 1 kHz.
    pub fn bins(ch: u8) -> RtaBinFrame {
        let levels = (0..512)
            .map(|k| {
                let hz = k as f64 * 48_000.0 / 1024.0;
                let octaves = (hz.max(20.0) / 100.0).log2();
                let tone = if (21..=22).contains(&k) { 30.0 } else { 0.0 };
                level(-30.0 - 3.0 * octaves.max(0.0) + tone)
            })
            .collect();
        RtaBinFrame {
            header: RtaBinHeader {
                channel: ch,
                seq: 1,
                fft_order: 10,
                sample_rate_hz: 48_000,
                n_bins: 512,
            },
            levels,
        }
    }

    pub fn status(live: u8) -> RtaStatus {
        RtaStatus {
            state: 1,
            tap: TAP_OUTPUT,
            channel: 0,
            live_count: live,
            live_mask: (1u16 << live) - 1,
            frames_per_s: 47,
            busy_us_per_s: 43_000,
            last_frame_us: 850,
            idle_ms: 20,
            sample_rate_hz: 48_000,
            first_band: 0,
            bass_busy_us_per_s: 1_200,
        }
    }

    /// Load the analyser with `channels` chosen at `tap`, each with a
    /// picture, and bins for a one-channel choice.
    pub fn load(shared: &Shared, tap: u8, channels: &[u8]) {
        let mut app = shared.borrow_mut();
        let s = &mut app.spectrum;
        s.engine.load(caps(), CENTRES.to_vec());
        let now = Instant::now();
        s.engine
            .accept_frames(tap, channels.iter().map(|c| frame(*c)).collect(), now);
        if let [one] = channels {
            s.engine.accept_bins(tap, &bins(*one), now);
        }
        s.engine.accept_status(status(channels.len().max(1) as u8));
        s.selection = Some(Selection::new(tap, channels.to_vec()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::panel::testing;
    use crate::shell::Tool;
    use crate::widgets::testing::key;
    use crossterm::event::KeyCode;

    fn shared_with(tap: u8, channels: &[u8]) -> Shared {
        let shared = crate::screens::shared();
        demo::load(&shared, tap, channels);
        shared
    }

    fn frame_of(shared: &Shared, state: &DeviceState, w: u16, h: u16) -> String {
        testing::frame(
            Tool::Spectrum,
            Box::new(SpectrumPanel::new(shared.clone())),
            state,
            w,
            h,
        )
    }

    fn both_sizes(shared: &Shared, state: &DeviceState, check: impl Fn(&str, u16, u16)) {
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            let f = frame_of(shared, state, w, h);
            assert_eq!(f.lines().count(), h as usize, "{w}x{h}");
            assert!(f.contains("Spectrum Analyser"), "{w}x{h}:\n{f}");
            assert!(f.contains("A closes"), "{w}x{h}:\n{f}");
            check(&f, w, h);
        }
    }

    #[test]
    fn golden_frames_unavailable() {
        let state = testing::state();
        // A firmware without the analyser: caps never read.
        let shared = crate::screens::shared();
        both_sizes(&shared, &state, |f, w, h| {
            assert!(f.contains("Spectrum analyser unavailable"), "{w}x{h}:\n{f}");
            assert!(f.contains("does not provide a compatible"), "{w}x{h}:\n{f}");
            assert!(!f.contains("Outputs"), "no controls: {f}");
        });
        let mut gone = state.clone();
        gone.connected = false;
        let shared = shared_with(TAP_OUTPUT, &[0]);
        let f = frame_of(&shared, &gone, 120, 40);
        assert!(f.contains("Connect a DSPi to use the analyser."), "{f}");
    }

    #[test]
    fn golden_frames_no_channels() {
        let state = testing::state();
        let shared = shared_with(TAP_OUTPUT, &[]);
        both_sizes(&shared, &state, |f, w, h| {
            assert!(f.contains("No channels selected"), "{w}x{h}:\n{f}");
            assert!(f.contains("Spectrum hidden"), "{w}x{h}:\n{f}");
            let summary = f.lines().find(|l| l.contains("Spectrum hidden")).unwrap();
            assert!(!summary.contains("Clear"), "nothing to clear: {summary}");
            assert!(f.contains("Inputs") && f.contains("Outputs"), "{f}");
        });
        // Nothing chosen means nothing watched.
        let mut p = SpectrumPanel::new(shared.clone());
        p.sync(&state);
        assert!(!shared.borrow().spectrum.engine.watching());
    }

    #[test]
    fn golden_frames_curves_one_channel() {
        let state = testing::state();
        let shared = shared_with(TAP_OUTPUT, &[0]);
        both_sizes(&shared, &state, |f, w, h| {
            assert!(f.contains("1 channel"), "{w}x{h}:\n{f}");
            assert!(f.contains("Clear"), "{f}");
            assert!(f.contains("Curves"), "{f}");
            assert!(f.contains("-40"), "the dB axis: {f}");
            assert!(f.contains("1k"), "the frequency axis: {f}");
            assert!(
                f.chars().any(|c| ('\u{2801}'..='\u{28FF}').contains(&c)),
                "a curve: {w}x{h}:\n{f}"
            );
            assert!(f.contains("Running"), "{f}");
            assert!(f.contains("1 channel, each refreshed every 21 ms"), "{f}");
        });
        let f = frame_of(&shared, &state, 120, 40);
        assert!(f.contains("47 frames/s"), "{f}");
        assert!(f.contains("transform 850 µs"), "{f}");
        assert!(f.contains("main loop 4.3%"), "{f}");
        assert!(f.contains("bass 0.12%"), "{f}");
        assert!(f.contains("120/70 dB range"), "{f}");
    }

    #[test]
    fn golden_frames_curves_several_channels() {
        let state = testing::state();
        let shared = shared_with(TAP_OUTPUT, &[0, 1, 8]);
        both_sizes(&shared, &state, |f, w, h| {
            assert!(f.contains("3 channels"), "{w}x{h}:\n{f}");
            assert!(
                f.chars().any(|c| ('\u{2801}'..='\u{28FF}').contains(&c)),
                "curves: {w}x{h}:\n{f}"
            );
        });
        // Band curves only: several channels rotate, so no bins are wanted.
        let mut p = SpectrumPanel::new(shared.clone());
        p.sync(&state);
        let want = shared.borrow().spectrum.engine.wanted().unwrap();
        assert_eq!(want.channel_mask, 0b1_0000_0011);
    }

    #[test]
    fn golden_frames_bars() {
        let state = testing::state();
        let shared = shared_with(TAP_OUTPUT, &[0, 1]);
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            let f = testing::frame(
                Tool::Spectrum,
                Box::new(SpectrumPanel::new(shared.clone()).mode(Mode::Bars)),
                &state,
                w,
                h,
            );
            assert_eq!(f.lines().count(), h as usize);
            assert!(f.contains('█'), "bars: {w}x{h}:\n{f}");
            assert!(f.contains("2 channels"), "{f}");
            // Each cell names its channel.
            let names = [0usize, 1].map(|o| channel_name_of(&state, TAP_OUTPUT, o as u8));
            for n in names {
                assert!(f.contains(&n), "{n} in {w}x{h}:\n{f}");
            }
        }
    }

    fn channel_name_of(state: &DeviceState, tap: u8, ch: u8) -> String {
        channel_label(state, tap, ch)
    }

    #[test]
    fn both_draws_the_curves_over_the_bars() {
        let state = testing::state();
        let shared = shared_with(TAP_OUTPUT, &[0]);
        let f = testing::frame(
            Tool::Spectrum,
            Box::new(SpectrumPanel::new(shared.clone()).mode(Mode::Both)),
            &state,
            120,
            40,
        );
        assert!(f.contains('█'), "{f}");
        assert!(
            f.chars().any(|c| ('\u{2801}'..='\u{28FF}').contains(&c)),
            "{f}"
        );
    }

    #[test]
    fn a_refusal_and_the_small_transform_hint_use_the_consoles_words() {
        let state = testing::state();
        let shared = shared_with(TAP_OUTPUT, &[0]);
        {
            let mut app = shared.borrow_mut();
            let mut settings = app.spectrum.settings.clone();
            settings.transform_order = Some(8);
            app.spectrum.adopt_settings(&settings);
            let mut s = demo::status(1);
            s.first_band = 15;
            app.spectrum.engine.accept_status(s);
        }
        let f = testing::frame(
            Tool::Spectrum,
            Box::new(SpectrumPanel::new(shared.clone()).mode(Mode::Bars)),
            &state,
            120,
            40,
        );
        assert!(
            f.contains("shaded bands below 315 Hz need a larger transform size in Settings"),
            "{f}"
        );
        assert!(f.contains('░'), "the shaded bands: {f}");
    }

    #[test]
    fn choosing_channels_and_sides_drives_the_engine() {
        let state = testing::state();
        let shared = shared_with(TAP_OUTPUT, &[0]);
        let mut p = SpectrumPanel::new(shared.clone());
        p.sync(&state);
        let wanted = || shared.borrow().spectrum.engine.wanted().unwrap();
        assert_eq!((wanted().tap, wanted().channel_mask), (TAP_OUTPUT, 1));

        // The chips: move right and show output 2 as well.
        p.body.focus = 2;
        p.handle(key(KeyCode::Right), &state);
        assert_eq!(p.chip, 1);
        p.handle(key(KeyCode::Char(' ')), &state);
        assert_eq!(wanted().channel_mask, 0b11);

        // Inputs: nothing chosen there yet, so nothing is watched.
        p.body.focus = 1;
        p.handle(key(KeyCode::Left), &state);
        let sel = selection(&state, &shared.borrow().spectrum);
        assert_eq!(sel, Selection::new(TAP_INPUT, vec![]));
        assert!(!shared.borrow().spectrum.engine.watching());
        // And back: the outputs come back as they were left.
        p.handle(key(KeyCode::Right), &state);
        assert_eq!(wanted().channel_mask, 0b11);

        // Clear.
        p.body.focus = 3;
        p.handle(key(KeyCode::Enter), &state);
        assert_eq!(
            selection(&state, &shared.borrow().spectrum).channels,
            Vec::<u8>::new()
        );
        assert!(!shared.borrow().spectrum.engine.watching());

        // The display switch.
        p.body.focus = 4;
        p.handle(key(KeyCode::Right), &state);
        assert_eq!(p.mode, Mode::Bars);
    }

    /// `A` is a global tool key (PLAN-beta4 decision 2): it opens the panel
    /// from the overview and closes it again from the panel itself.
    #[test]
    fn a_opens_and_closes_the_panel() {
        use crate::shell::{Placeholder, Shell, ShellEvent};
        let state = testing::state();
        let theme = crate::theme::Theme::console(
            crate::theme::ColorDepth::TrueColor,
            crate::theme::Glyphs::Braille,
        );
        let model = crate::shell::fixture::rp2350(&theme);
        let mut shell = Shell::new(model, theme, Box::new(Placeholder::new("Overview", "")));
        let a = crossterm::event::KeyEvent::new(
            KeyCode::Char('A'),
            crossterm::event::KeyModifiers::SHIFT,
        );
        assert_eq!(
            shell.handle(a, &state),
            vec![ShellEvent::OpenTool(Tool::Spectrum)]
        );
        let shared = shared_with(TAP_OUTPUT, &[0]);
        shell.open_tool(Tool::Spectrum, Box::new(SpectrumPanel::new(shared)));
        assert_eq!(shell.handle(a, &state), vec![ShellEvent::CloseTool]);
    }

    #[test]
    fn closing_the_panel_releases_the_analyser() {
        let state = testing::state();
        let shared = shared_with(TAP_OUTPUT, &[0]);
        let p = SpectrumPanel::open(shared.clone(), &state);
        assert!(shared.borrow().spectrum.engine.watching());
        drop(p);
        assert!(!shared.borrow().spectrum.engine.watching());
    }

    /// The input chips are the rows the device is analysing: the active
    /// inputs, and the upmixer's derived rows while it runs on the stereo
    /// pair (rta.c:95-106).
    #[test]
    fn the_input_chips_are_the_live_rows() {
        let mut state = testing::state();
        let shared = crate::screens::shared();
        demo::load(&shared, TAP_INPUT, &[0]);
        let engine = &shared.borrow().spectrum.engine;
        let n = |state: &DeviceState| channels(state, engine, TAP_INPUT).len();
        state.meters.active_inputs = 0;
        assert_eq!(n(&state), 8, "unknown: every input");
        state.meters.active_inputs = 4;
        assert_eq!(n(&state), 4);
        state.meters.active_inputs = 2;
        assert_eq!(n(&state), 2, "the upmixer is off");
        state.caps.features.push(dspi_session::probe::Feature {
            name: "upmixer".into(),
            present: true,
            evidence: "test".into(),
        });
        let o = dspi_proto::generated::SECTIONS
            .iter()
            .find(|(n, _, _)| *n == "upmix")
            .map(|(_, o, _)| *o)
            .unwrap();
        state.bulk.patch(o, &[1]);
        assert!(state.upmix().enabled);
        let rows = 2 + super::super::matrix::derived_rows(&state);
        assert!(rows == 3 || rows == 5, "{rows}");
        assert_eq!(n(&state), rows, "the derived rows are live");
        assert_eq!(channel_label(&state, TAP_INPUT, 2), "C");
    }

    #[test]
    fn the_default_is_the_first_enabled_output() {
        let state = testing::state();
        let shared = crate::screens::shared();
        demo::load(&shared, TAP_OUTPUT, &[]);
        shared.borrow_mut().spectrum.selection = None;
        let sel = selection(&state, &shared.borrow().spectrum);
        let first = channels(&state, &shared.borrow().spectrum.engine, TAP_OUTPUT)[0];
        assert_eq!(sel, Selection::new(TAP_OUTPUT, vec![first]));
    }

    #[test]
    fn a_band_curve_leaves_out_what_was_not_measured() {
        let levels: Vec<f64> = (0..34).map(|i| -20.0 - i as f64).collect();
        let curve = band_curve(&levels, &demo::CENTRES, &|i| i >= 14);
        let f = dsp::frequencies();
        let at = |hz: f64| {
            let i = f.iter().position(|x| *x >= hz).unwrap();
            curve[i]
        };
        assert!(at(100.0).is_nan(), "below the first measured band");
        assert!(at(1000.0).is_finite());
        // 1 kHz is band 20: -40 dBFS, give or take the grid.
        assert!((at(1000.0) - -40.0).abs() < 1.0, "{}", at(1000.0));
    }

    #[test]
    fn one_channel_blends_bands_into_bins_above_the_bass_bank() {
        let bands = vec![-20.0; dsp::POINTS];
        let bins = Bins {
            channel: 0,
            fft_order: 10,
            sample_rate_hz: 48_000,
            levels_db: vec![-60.0; 512],
            smoothed_db: vec![-60.0; 512],
        };
        let c = blend(&bands, &bins, &demo::CENTRES, 14);
        let f = dsp::frequencies();
        let at = |hz: f64| c[f.iter().position(|x| *x >= hz).unwrap()];
        assert_eq!(at(20.0), -20.0, "below bin 1 the bands alone");
        assert!((at(100.0) - -20.0).abs() < 1e-9, "the bass bank");
        assert!((at(2000.0) - -60.0).abs() < 1e-9, "the bins");
        let mid = at(300.0);
        assert!(mid < -20.0 && mid > -60.0, "crossfaded: {mid}");
    }

    #[test]
    fn a_glide_snaps_first_and_falls_slower_than_it_rises() {
        let mut g = Glide::default();
        let t0 = Instant::now();
        let rise = Duration::from_millis(40);
        let fall = Duration::from_millis(100);
        assert_eq!(g.step((1, 0, 0), vec![-60.0], t0, rise, fall), vec![-60.0]);
        let up = g.step(
            (1, 0, 0),
            vec![0.0],
            t0 + Duration::from_millis(40),
            rise,
            fall,
        )[0];
        let mut h = Glide::default();
        h.step((1, 0, 0), vec![0.0], t0, rise, fall);
        let down = h.step(
            (1, 0, 0),
            vec![-60.0],
            t0 + Duration::from_millis(40),
            rise,
            fall,
        )[0];
        assert!(up > -60.0 && up < 0.0);
        assert!((up - -60.0).abs() > (down - 0.0).abs(), "{up} {down}");
    }

    #[test]
    fn every_advertised_key_is_handled_somewhere() {
        let state = testing::state();
        let shared = shared_with(TAP_OUTPUT, &[0]);
        for help in KEYS {
            for k in crate::screens::tests::keys_for(help.key) {
                let mut hit = false;
                for focus in 1..6 {
                    let mut p = SpectrumPanel::new(shared.clone());
                    p.body.focus = focus;
                    let before = (p.body.focus, p.chip, p.mode);
                    let ev = p.handle(k, &state);
                    if ev != ScreenEvent::Unhandled || (p.body.focus, p.chip, p.mode) != before {
                        hit = true;
                        break;
                    }
                }
                assert!(hit, "{:?} from {:?} is not bound", k.code, help.key);
            }
        }
    }
}
