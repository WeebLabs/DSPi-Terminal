//! The tool-panel template: the shape every DSP window in `DESIGN.md` 7.8 and
//! the Signal Generator panel in 7.9 are built from.
//!
//! A panel is a pinned header (title, subtitle and the master toggle, which is
//! always first in focus order) over a single scrolling column of rows. A panel
//! declares that column afresh every frame as a `Vec<Row>` read from
//! `DeviceState`, so a change made anywhere on the device is on screen the next
//! tick, and this module measures it, scrolls it, draws it and routes keys into
//! the widget each row is made of. When the device does not have the feature,
//! the whole column is replaced by the Console's unsupported banner.
//!
//! Rows own their strings rather than borrowing, because they are rebuilt from
//! the device state each frame and the borrow would tie the panel's own state
//! to the frame it was drawn in.

use crossterm::event::{KeyCode, KeyEvent};
use dspi_session::DeviceState;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;

use crate::graph::{Graph, GraphCurve, GraphSettings};
use crate::theme::{Glyphs, Theme};
use crate::widgets::chips::Chip;
use crate::widgets::text::{fit_left, truncate, wrap};
use crate::widgets::{
    Action, Banner, ChipRow, ChipState, NumberEdit, ParamRow, SectionHeader, Segmented, StatusPill,
    StatusTone, Taper, ToggleRow,
};

/// The height every panel graph is given: `DESIGN.md` 7.8's eight rows.
pub const GRAPH_ROWS: u16 = 8;

/// A theme for key handling only.
///
/// `Screen::handle` is not given the theme, and a widget's `handle` needs one
/// only because the widget is built before it answers. Which key does what
/// never depends on the palette, so one shared instance serves every panel and
/// no theme is rebuilt on a keystroke.
pub fn key_theme() -> &'static Theme {
    static THEME: std::sync::OnceLock<Theme> = std::sync::OnceLock::new();
    THEME.get_or_init(|| Theme::console(crate::theme::ColorDepth::TrueColor, Glyphs::Braille))
}

/// Is a device feature present, as `probe` reported it?
///
/// A feature the probe never asked about is absent, which is the same answer
/// the Console gives when its version gate fails: the panel shows its banner
/// rather than offering controls that would stall.
pub fn has_feature(state: &DeviceState, name: &str) -> bool {
    state
        .caps
        .features
        .iter()
        .any(|f| f.name == name && f.present)
}

/// A mask with one bit per output this device has.
pub fn all_outputs_mask(state: &DeviceState) -> u16 {
    let n = state.caps.num_outputs as u32;
    if n >= 16 {
        0xFFFF
    } else {
        (1u32 << n) as u16 - 1
    }
}

/// The PDM sub's output index: the last output slot, which is how the Console
/// numbers it (OUT9 on RP2350, OUT5 on RP2040).
pub fn sub_output(state: &DeviceState) -> usize {
    (state.caps.num_outputs as usize).saturating_sub(1)
}

// ---------------------------------------------------------------- row model

/// One parameter row, as data.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub label: String,
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub unit: String,
    pub decimals: usize,
    pub step: f64,
    pub taper: Taper,
    pub caption: Option<String>,
    pub ends: Option<(String, String)>,
    pub enabled: bool,
    /// Drawn as inactive but still editable, which is the Console's
    /// `.opacity(0.5)` on a section whose controls stay live: the crossfeed
    /// parameters outside the Custom voicing, where an edit switches the
    /// voicing rather than being refused (`CrossfeedView.swift:301`).
    pub dimmed: bool,
    /// Text shown instead of the number: the Console's `displayOverride`.
    pub display: Option<String>,
}

impl Param {
    pub fn new(label: &str, value: f64, min: f64, max: f64, unit: &str) -> Self {
        Self {
            label: label.into(),
            value,
            min,
            max,
            unit: unit.into(),
            decimals: 1,
            step: 1.0,
            taper: Taper::Linear,
            caption: None,
            ends: None,
            enabled: true,
            dimmed: false,
            display: None,
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
    pub fn caption(mut self, c: &str) -> Self {
        self.caption = Some(c.into());
        self
    }
    pub fn ends(mut self, lo: &str, hi: &str) -> Self {
        self.ends = Some((lo.into(), hi.into()));
        self
    }
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }
    pub fn dimmed(mut self, d: bool) -> Self {
        self.dimmed = d;
        self
    }
    pub fn display(mut self, d: Option<&str>) -> Self {
        self.display = d.map(str::to_string);
        self
    }

    /// The row as it is drawn. A dimmed parameter borrows the disabled
    /// styling and nothing else: `handle` still goes through [`Self::widget`],
    /// so the keys keep working.
    fn drawn<'a>(&'a self, theme: &'a Theme) -> ParamRow<'a> {
        self.widget(theme).enabled(self.enabled && !self.dimmed)
    }

    fn widget<'a>(&'a self, theme: &'a Theme) -> ParamRow<'a> {
        let mut r = ParamRow::new(
            &self.label,
            self.value,
            self.min,
            self.max,
            &self.unit,
            theme,
        )
        .step(self.step)
        .decimals(self.decimals)
        .taper(self.taper)
        .enabled(self.enabled)
        .display(self.display.as_deref());
        if let Some(c) = &self.caption {
            r = r.caption(c);
        }
        if let Some((lo, hi)) = &self.ends {
            r = r.ends(lo, hi);
        }
        r
    }
}

/// One chip in a chip row.
#[derive(Debug, Clone, PartialEq)]
pub struct ChipSpec {
    pub label: String,
    pub state: ChipState,
    pub color: Color,
    pub enabled: bool,
    /// Drawn dim but still selectable, the Console's
    /// `.opacity(matrixEnabled ? 1.0 : 0.4)` on a chip whose button is live
    /// (`TestSignalsView.swift:557`).
    pub dimmed: bool,
}

/// What a panel's graph draws.
#[derive(Debug, Clone, PartialEq)]
pub enum PanelGraph {
    /// dB against log frequency, auto-scaled to the data the way the Console's
    /// curve views are: at least a 10 dB window, with 20 % padding.
    Curves {
        series: Vec<(String, Color, Vec<f64>)>,
        /// Drawn top right, one per line: the Console's little legend or badge.
        legend: Vec<(String, Color)>,
    },
    /// The psychoacoustic-bass schematic: the original band up to `fc` and the
    /// synthesised harmonics from `fc` to `4fc`, with both marked.
    Bars {
        fc: f64,
        original: f64,
        harmonics: f64,
    },
    /// The Tube Modeller's static transfer curve: output against input over
    /// one full-scale swing, as `TubeTransferView` draws it.
    Transfer {
        /// The output at evenly spaced inputs from -1 to +1.
        output: Vec<f64>,
        /// The inputs past which the stage clips, negative then positive,
        /// when they fall inside the swing.
        knees: (Option<f64>, Option<f64>),
    },
    /// The subharmonic synthesizer's three derived bands and its LF boost
    /// bell, over the Console's 16 to 250 Hz and -42 to +18 dB frame. The
    /// levels are the three band settings in dB, `SUBHARM_LEVEL_MIN` for a
    /// band that is off; the ceiling is in dBFS, 0 for off.
    Subharm {
        levels: [f64; 3],
        boost: f64,
        ceiling: f64,
    },
    /// The master toggle is off, so the Console draws the word instead.
    Disabled,
}

/// One meter under a chip: the chip's label (so the meter lines up with
/// it), the level from 0 to 1, and whether the chip is on.
#[derive(Debug, Clone, PartialEq)]
pub struct ChipMeter {
    pub label: String,
    pub fraction: f64,
    pub on: bool,
}

/// One row of a panel's scrolling column.
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    /// An uppercase section label with an optional right-aligned action.
    Section {
        title: String,
        action: Option<String>,
    },
    /// A section label whose action is the focusable control, for the panels
    /// whose only control in that section is a menu.
    Menu {
        title: String,
        action: String,
    },
    Caption(String),
    Blank,
    Param(Param),
    Toggle {
        label: String,
        on: bool,
        caption: Option<String>,
        enabled: bool,
    },
    Segmented {
        label: String,
        choices: Vec<String>,
        selected: usize,
        enabled: bool,
    },
    /// One option of a radio group: a disc, a name and a detail line.
    Radio {
        label: String,
        detail: String,
        on: bool,
    },
    Chips {
        chips: Vec<ChipSpec>,
        cursor: usize,
        polarity: bool,
    },
    /// A labelled row of push buttons, for the Console's inline preset buttons.
    Buttons {
        label: String,
        buttons: Vec<String>,
        cursor: usize,
    },
    Graph(PanelGraph),
    /// A read-only bar with a label and a formatted value: the upmixer's
    /// telemetry.
    Gauge {
        label: String,
        fraction: f64,
        display: String,
        color: Color,
    },
    /// A dot and a line of text.
    Status {
        text: String,
        tone: StatusTone,
    },
    /// The signal grid: fixed-width cells laid out in a grid.
    Tiles {
        labels: Vec<String>,
        selected: usize,
        cursor: usize,
        columns: usize,
    },
    /// The transport row: a status line and a Start / Stop button pair.
    Transport {
        title: String,
        detail: String,
        running: bool,
        enabled: bool,
        cursor: usize,
    },
    Banner {
        title: String,
        body: String,
    },
    /// A section label with a reading at its right, in a tone: the
    /// subharmonic synthesizer's HEADROOM COST.
    Readout {
        title: String,
        value: String,
        tone: StatusTone,
    },
    /// A thin meter under each chip of the chip row above it.
    Meters(Vec<ChipMeter>),
}

/// The width a tile takes, including its gap: eight columns of label, the two
/// brackets, and a space. Eight is the longest name in the signal catalogue.
const TILE_WIDTH: u16 = 11;

/// The column a one-line radio row's detail starts in, measured from the end
/// of its disc. `DESIGN.md` 7.8 aligns `Jan Meier` and the three shorter names
/// on the same edge, and `Jan Meier` is the longest of them.
const RADIO_LABEL_W: usize = 12;

/// Whether a radio row's detail fits beside its label rather than under it.
///
/// Measured against the widest disc, `(*)` in ASCII, so the answer does not
/// change with the glyph set: `height` has no theme to ask.
fn radio_on_one_line(width: u16, detail: &str) -> bool {
    2 + 3 + RADIO_LABEL_W + detail.chars().count() <= width as usize
}

impl Row {
    /// Can the keyboard land here?
    pub fn focusable(&self) -> bool {
        matches!(
            self,
            Row::Menu { .. }
                | Row::Param(_)
                | Row::Toggle { .. }
                | Row::Segmented { .. }
                | Row::Radio { .. }
                | Row::Chips { .. }
                | Row::Buttons { .. }
                | Row::Tiles { .. }
                | Row::Transport { .. }
        )
    }

    /// How many rows this wants at a given width.
    pub fn height(&self, width: u16) -> u16 {
        let caption = |c: &Option<String>| {
            c.as_ref()
                .map(|c| wrap(c, width.saturating_sub(2) as usize, 2).len() as u16)
                .unwrap_or(0)
        };
        match self {
            Row::Section { .. } | Row::Menu { .. } | Row::Blank => 1,
            Row::Caption(c) => wrap(c, width.saturating_sub(2) as usize, 3).len() as u16,
            Row::Param(p) => {
                if width < 40 {
                    // Narrow: the caption goes first, then the slider, as
                    // `DESIGN.md` 8 says.
                    1
                } else {
                    2 + caption(&p.caption)
                }
            }
            Row::Toggle { caption: c, .. } => 1 + if width < 40 { 0 } else { caption(c) },
            Row::Segmented { .. } => 1,
            Row::Radio { detail, .. } => {
                if radio_on_one_line(width, detail) {
                    1
                } else {
                    2
                }
            }
            Row::Chips { .. } => 1,
            Row::Buttons { .. } => 1,
            Row::Graph(_) => GRAPH_ROWS,
            Row::Gauge { .. } => 1,
            Row::Status { .. } => 1,
            Row::Tiles {
                labels, columns, ..
            } => {
                let cols = grid_columns(*columns, width);
                labels.len().div_ceil(cols) as u16
            }
            Row::Transport { .. } => 2,
            Row::Banner { body, .. } => {
                1 + wrap(body, width.saturating_sub(3) as usize, 3).len() as u16
            }
            Row::Readout { .. } | Row::Meters(_) => 1,
        }
    }

    /// The keys this row's widget handles.
    pub fn handle(&self, key: KeyEvent, theme: &Theme) -> Option<Action> {
        match self {
            Row::Param(p) => p.widget(theme).handle(key),
            Row::Toggle { on, enabled, .. } => {
                ToggleRow::new("", *on, theme).enabled(*enabled).handle(key)
            }
            Row::Segmented {
                choices,
                selected,
                enabled,
                ..
            } => {
                let refs: Vec<&str> = choices.iter().map(String::as_str).collect();
                Segmented::new("", &refs, *selected, theme)
                    .enabled(*enabled)
                    .handle(key)
            }
            Row::Radio { .. } => match key.code {
                KeyCode::Enter | KeyCode::Char(' ') => Some(Action::Toggled(true)),
                _ => None,
            },
            Row::Chips {
                chips,
                cursor,
                polarity,
            } => {
                let mut row = ChipRow::new(theme);
                for c in chips {
                    row.chips.push(Chip {
                        label: &c.label,
                        state: c.state,
                        color: c.color,
                        enabled: c.enabled,
                    });
                }
                row.focused(true, *cursor).polarity(*polarity).handle(key)
            }
            Row::Menu { .. } => match key.code {
                KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Char('p') => Some(Action::Open),
                _ => None,
            },
            Row::Buttons {
                buttons, cursor, ..
            } => match key.code {
                KeyCode::Left => Some(Action::Selected(cursor.saturating_sub(1))),
                KeyCode::Right => {
                    Some(Action::Selected((cursor + 1).min(buttons.len().max(1) - 1)))
                }
                KeyCode::Enter | KeyCode::Char(' ') => Some(Action::Button(*cursor)),
                _ => None,
            },
            Row::Tiles {
                labels,
                cursor,
                columns,
                ..
            } => {
                let cols = (*columns).max(1);
                let n = labels.len();
                if n == 0 {
                    return None;
                }
                let moved = match key.code {
                    KeyCode::Left => cursor.saturating_sub(1),
                    KeyCode::Right => (cursor + 1).min(n - 1),
                    KeyCode::Up => cursor.saturating_sub(cols),
                    KeyCode::Down => (cursor + cols).min(n - 1),
                    KeyCode::Enter | KeyCode::Char(' ') => {
                        return Some(Action::Button(*cursor));
                    }
                    _ => return None,
                };
                Some(Action::Selected(moved))
            }
            Row::Transport {
                running,
                enabled,
                cursor,
                ..
            } => match key.code {
                KeyCode::Left => Some(Action::Selected(cursor.saturating_sub(1))),
                KeyCode::Right => Some(Action::Selected((cursor + 1).min(1))),
                KeyCode::Enter | KeyCode::Char(' ') if *enabled || *running => {
                    Some(Action::Button(*cursor))
                }
                _ => None,
            },
            _ => None,
        }
    }
}

/// How many tile columns fit; the declared count, narrowed when the pane is.
fn grid_columns(want: usize, width: u16) -> usize {
    let fits = (width.saturating_sub(1) / TILE_WIDTH).max(1) as usize;
    want.min(fits).max(1)
}

/// The indices of the rows the keyboard can land on.
pub fn focus_rows(rows: &[Row]) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter(|(_, r)| r.focusable())
        .map(|(i, _)| i)
        .collect()
}

/// The first row to draw so that `focus` is fully visible, given where the
/// column was scrolled to last frame.
///
/// Scrolling by whole rows rather than by lines keeps a three-line parameter
/// row from being sliced in half at the top of the pane, which is what the
/// Console's scroll view does too.
pub fn window(heights: &[u16], viewport: u16, first: usize, focus: Option<usize>) -> usize {
    if heights.is_empty() {
        return 0;
    }
    let mut first = first.min(heights.len() - 1);
    let Some(focus) = focus else { return first };
    if focus < first {
        return focus;
    }
    loop {
        let used: u16 = heights[first..=focus.min(heights.len() - 1)].iter().sum();
        if used <= viewport || first >= focus {
            return first;
        }
        first += 1;
    }
}

// --------------------------------------------------------------- panel body

/// The cursor, the scroll position and the armed field: everything a panel
/// keeps between frames. The rows themselves are rebuilt from the device each
/// frame, so this is all the state a tool panel has.
///
/// Focus index 0 is always the header toggle, which `DESIGN.md` 7.8 puts first
/// in focus order; 1 and up index the focusable rows in order.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Body {
    pub focus: usize,
    pub first: usize,
    pub edit: Option<NumberEdit>,
    /// Set when the panel has no header toggle, so focus starts on the rows.
    pub headless: bool,
}

impl Body {
    pub fn headless() -> Self {
        Self {
            focus: 1,
            headless: true,
            ..Self::default()
        }
    }

    /// The row the cursor is on, if it is not on the header.
    pub fn focused_row(&self, rows: &[Row]) -> Option<usize> {
        if self.focus == 0 {
            return None;
        }
        focus_rows(rows).get(self.focus - 1).copied()
    }

    /// The focusable row the cursor is on.
    pub fn row<'a>(&self, rows: &'a [Row]) -> Option<&'a Row> {
        self.focused_row(rows).and_then(|i| rows.get(i))
    }

    /// Move the cursor, clamped. Returns false when it was already at the end,
    /// which is how a panel knows to hand the key back to the shell.
    pub fn step(&mut self, dir: i32, rows: &[Row]) -> bool {
        // Index 0 is the header and 1 up the rows, with or without a header
        // drawn, so the last row is the row count either way.
        let highest = focus_rows(rows).len();
        let lowest = usize::from(self.headless);
        if highest < lowest {
            return false;
        }
        let next = (self.focus as i32 + dir).clamp(lowest as i32, highest as i32) as usize;
        let moved = next != self.focus;
        self.focus = next;
        if moved {
            self.edit = None;
        }
        moved
    }

    /// Keep the cursor on a row that still exists after the panel's shape
    /// changed (an engine switched off, a signal type with fewer parameters).
    pub fn clamp(&mut self, rows: &[Row]) {
        let highest = focus_rows(rows).len();
        let lowest = usize::from(self.headless);
        self.focus = self.focus.clamp(lowest, highest.max(lowest));
    }

    /// Draw the header and the column beneath it.
    pub fn draw(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        header: &Header<'_>,
        rows: &[Row],
        focused: bool,
    ) {
        if area.height == 0 {
            return;
        }
        draw_header(
            area,
            buf,
            theme,
            header,
            focused && self.focus == 0 && !self.headless,
        );
        if area.height < 2 {
            return;
        }
        let body = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
        let heights: Vec<u16> = rows.iter().map(|r| r.height(body.width)).collect();
        let focus_row = self.focused_row(rows);
        self.first = window(&heights, body.height, self.first, focus_row);
        draw_rows(
            body,
            buf,
            theme,
            rows,
            self.first,
            focus_row,
            focused,
            self.edit.as_ref(),
        );
    }
}

/// What is left for a panel to do after the shared key handling has run.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Nothing: the cursor moved, an edit was cancelled, or the key was not
    /// for this panel.
    Done(crate::shell::ScreenEvent),
    /// The header's master toggle was pressed.
    Header,
    /// The focused row answered, by row index.
    Row(usize, Action),
    /// A typed value was committed on the parameter at this row index.
    Commit(usize, f64),
}

/// The half of every panel's key handling that is the same: an armed field
/// first, then the up and down walk, then the header toggle, then the focused
/// row's own widget. What comes back is only the part a panel has to decide
/// for itself, which is which command the change is.
pub fn dispatch(body: &mut Body, rows: &[Row], key: KeyEvent) -> Step {
    use crate::shell::ScreenEvent;

    if let Some(edit) = body.edit.as_mut() {
        return match edit.handle(key) {
            Some(Action::Committed(v)) => {
                body.edit = None;
                match body.focused_row(rows) {
                    Some(i) => Step::Commit(i, v),
                    None => Step::Done(ScreenEvent::Handled),
                }
            }
            Some(Action::Closed) => {
                body.edit = None;
                Step::Done(ScreenEvent::Handled)
            }
            _ => Step::Done(ScreenEvent::Handled),
        };
    }

    if matches!(key.code, KeyCode::Up | KeyCode::Down) {
        let dir = if key.code == KeyCode::Up { -1 } else { 1 };
        // A tile grid walks in two dimensions, so it keeps the vertical keys
        // until its own cursor has nowhere left to go, and only then does the
        // panel's cursor leave the grid.
        if let Some(i) = body.focused_row(rows)
            && let Row::Tiles { cursor, .. } = &rows[i]
            && let Some(Action::Selected(next)) = rows[i].handle(key, key_theme())
            && next != *cursor
        {
            return Step::Row(i, Action::Selected(next));
        }
        body.step(dir, rows);
        return Step::Done(ScreenEvent::Handled);
    }

    if body.focus == 0 {
        return match key.code {
            KeyCode::Enter | KeyCode::Char(' ') => Step::Header,
            _ => Step::Done(ScreenEvent::Unhandled),
        };
    }

    let Some(i) = body.focused_row(rows) else {
        return Step::Done(ScreenEvent::Unhandled);
    };
    match rows[i].handle(key, key_theme()) {
        // Enter on a parameter arms it for typing; every other row's Open is
        // the panel's business.
        Some(Action::Open) => {
            if let Row::Param(p) = &rows[i] {
                body.edit = Some(NumberEdit::start(p.value, p.decimals));
                return Step::Done(ScreenEvent::Handled);
            }
            Step::Row(i, Action::Open)
        }
        Some(a) => Step::Row(i, a),
        None => Step::Done(ScreenEvent::Unhandled),
    }
}

// ------------------------------------------------------------------ drawing

/// A panel's fixed header: the Console's window title, its subtitle and the
/// master toggle.
pub struct Header<'a> {
    /// The Console's window subtitle. The panel's name is on the pane
    /// border already, so the header does not say it again.
    pub subtitle: &'a str,
    /// `None` for a panel with no master switch.
    pub toggle: Option<bool>,
    pub enabled: bool,
    /// A state pill instead of a switch, for the Signal Generator, whose header
    /// carries the generator's run state rather than a control.
    pub pill: Option<(String, StatusTone)>,
    /// A second, smaller control drawn just left of the switch, in the style
    /// the panel gives it: the subharmonic synthesizer's `SOLO`.
    pub badge: Option<(String, Style)>,
}

impl<'a> Header<'a> {
    pub fn new(subtitle: &'a str) -> Self {
        Self {
            subtitle,
            toggle: None,
            enabled: true,
            pill: None,
            badge: None,
        }
    }
    pub fn toggle(mut self, on: bool) -> Self {
        self.toggle = Some(on);
        self
    }
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }
    pub fn pill(mut self, text: impl Into<String>, tone: StatusTone) -> Self {
        self.pill = Some((text.into(), tone));
        self
    }
    pub fn badge(mut self, text: impl Into<String>, style: Style) -> Self {
        self.badge = Some((text.into(), style));
        self
    }
}

pub fn draw_header(area: Rect, buf: &mut Buffer, theme: &Theme, h: &Header<'_>, focused: bool) {
    if area.height == 0 {
        return;
    }
    let state = h
        .toggle
        .map(|on| ToggleRow::state_text(on, theme.glyphs))
        .unwrap_or("");
    let right = state.chars().count()
        + h.pill
            .as_ref()
            .map(|(t, _)| t.chars().count() + 2)
            .unwrap_or(0);
    let badge = h.badge.as_ref().map(|(t, _)| t.chars().count() + 1);
    let reserve = right + 2 + badge.unwrap_or(0);
    let title = truncate(
        h.subtitle,
        (area.width as usize).saturating_sub(reserve + 1),
    );
    buf.set_string(
        area.x,
        area.y,
        if focused { "▸" } else { " " },
        theme.focused(),
    );
    buf.set_string(
        area.x + 1,
        area.y,
        &title,
        if focused {
            theme.focused()
        } else {
            theme.label()
        },
    );
    if h.toggle.is_some() {
        let style = if !h.enabled {
            theme.label()
        } else if h.toggle == Some(true) {
            Style::default().fg(theme.ok)
        } else {
            theme.label()
        };
        buf.set_string(
            area.x + area.width - state.chars().count() as u16,
            area.y,
            state,
            style,
        );
    }
    if let Some((text, tone)) = &h.pill {
        draw_pill(area, buf, theme, text, *tone);
    }
    if let Some((text, style)) = &h.badge {
        let x = (area.x + area.width).saturating_sub((right + 1 + text.chars().count()) as u16);
        if x > area.x + 1 {
            buf.set_string(x, area.y, text, *style);
        }
    }
}

/// Draw the scrolling column, returning the row index it started at so the
/// panel can keep its place.
#[allow(clippy::too_many_arguments)]
pub fn draw_rows(
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    rows: &[Row],
    first: usize,
    focus_row: Option<usize>,
    focused: bool,
    edit: Option<&NumberEdit>,
) {
    let mut y = area.y;
    let bottom = area.y + area.height;
    for (i, row) in rows.iter().enumerate().skip(first) {
        let h = row.height(area.width);
        if y >= bottom {
            break;
        }
        let visible = h.min(bottom - y);
        let r = Rect::new(area.x, y, area.width, visible);
        let here = focused && focus_row == Some(i);
        draw_row(r, buf, theme, row, here, if here { edit } else { None });
        y += h;
    }
}

fn draw_row(
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    row: &Row,
    focused: bool,
    edit: Option<&NumberEdit>,
) {
    if area.height == 0 || area.width < 8 {
        return;
    }
    match row {
        Row::Blank => {}
        Row::Section { title, action } => {
            let mut h = SectionHeader::new(title, theme);
            if let Some(a) = action {
                h = h.action(a);
            }
            h.render(area, buf);
        }
        Row::Menu { title, action } => {
            SectionHeader::new(title, theme)
                .action(action)
                .focused(focused)
                .render(area, buf);
            if focused {
                buf.set_string(area.x, area.y, "▸", theme.focused());
            }
        }
        Row::Caption(c) => {
            for (i, line) in wrap(
                c,
                area.width.saturating_sub(2) as usize,
                area.height as usize,
            )
            .iter()
            .enumerate()
            {
                buf.set_string(area.x + 1, area.y + i as u16, line, theme.label());
            }
        }
        Row::Param(p) => {
            p.drawn(theme)
                .focused(focused)
                .edit(edit)
                .compact(area.width < 40)
                .render(area, buf);
        }
        Row::Toggle {
            label,
            on,
            caption,
            enabled,
        } => {
            let mut t = ToggleRow::new(label, *on, theme)
                .focused(focused)
                .enabled(*enabled);
            if let Some(c) = caption.as_deref().filter(|_| area.height > 1) {
                t = t.caption(c);
            }
            t.render(area, buf);
        }
        Row::Segmented {
            label,
            choices,
            selected,
            enabled,
        } => {
            let refs: Vec<&str> = choices.iter().map(String::as_str).collect();
            Segmented::new(label, &refs, *selected, theme)
                .focused(focused)
                .enabled(*enabled)
                .render(area, buf);
        }
        Row::Radio {
            label, detail, on, ..
        } => {
            let disc = match (*on, theme.glyphs) {
                (true, Glyphs::Ascii) => "(*)",
                (false, Glyphs::Ascii) => "( )",
                (true, _) => "●",
                (false, _) => "○",
            };
            buf.set_string(
                area.x,
                area.y,
                if focused { "▸" } else { " " },
                theme.focused(),
            );
            let style = if *on {
                Style::default().fg(theme.accent)
            } else {
                theme.label()
            };
            buf.set_string(area.x + 1, area.y, disc, style);
            let lx = area.x + 2 + disc.chars().count() as u16;
            let label_style = if focused {
                theme.focused()
            } else {
                theme.value()
            };
            let room = area.width.saturating_sub(lx - area.x) as usize;
            if area.height == 1 {
                // `DESIGN.md` 7.8: `● Default    700 Hz / 4.5 dB - Balanced,
                // most popular`, on one row, with the details in a column.
                buf.set_string(lx, area.y, fit_left(label, RADIO_LABEL_W), label_style);
                let dx = lx + RADIO_LABEL_W as u16;
                buf.set_string(
                    dx,
                    area.y,
                    truncate(detail, room.saturating_sub(RADIO_LABEL_W)),
                    theme.label(),
                );
            } else {
                buf.set_string(lx, area.y, truncate(label, room), label_style);
                buf.set_string(lx, area.y + 1, truncate(detail, room), theme.label());
            }
        }
        Row::Chips {
            chips,
            cursor,
            polarity,
        } => {
            let mut r = ChipRow::new(theme);
            for c in chips {
                r.chips.push(Chip {
                    label: &c.label,
                    state: c.state,
                    color: c.color,
                    // Drawing only: `handle` below keeps a dimmed chip live.
                    enabled: c.enabled && !c.dimmed,
                });
            }
            r.focused(focused, *cursor)
                .polarity(*polarity)
                .render(area, buf);
        }
        Row::Buttons {
            label,
            buttons,
            cursor,
        } => {
            buf.set_string(
                area.x,
                area.y,
                if focused { "▸" } else { " " },
                theme.focused(),
            );
            buf.set_string(area.x + 1, area.y, label, theme.label());
            let mut x = area.x + 2 + label.chars().count() as u16;
            for (i, b) in buttons.iter().enumerate() {
                let text = format!(" {b} ");
                if x + text.chars().count() as u16 > area.x + area.width {
                    break;
                }
                let style = if focused && i == *cursor {
                    theme.pill(theme.accent)
                } else {
                    theme.value()
                };
                buf.set_string(x, area.y, &text, style);
                x += text.chars().count() as u16 + 1;
            }
        }
        Row::Graph(g) => draw_graph(area, buf, theme, g),
        Row::Gauge {
            label,
            fraction,
            display,
            color,
        } => {
            buf.set_string(area.x + 1, area.y, fit_left(label, 14), theme.label());
            let bx = area.x + 16;
            let bw = area
                .width
                .saturating_sub(16 + display.chars().count() as u16 + 2);
            if bw >= 4 {
                let filled = (fraction.clamp(0.0, 1.0) * bw as f64).round() as u16;
                for i in 0..bw {
                    let (sym, style) = if i < filled {
                        ("▓", Style::default().fg(*color))
                    } else {
                        ("░", Style::default().fg(theme.chrome_faint))
                    };
                    buf[(bx + i, area.y)].set_symbol(sym).set_style(style);
                }
            }
            buf.set_string(
                area.x + area.width - display.chars().count() as u16,
                area.y,
                display,
                theme.value(),
            );
        }
        Row::Status { text, tone } => {
            let (dot, color) = match tone {
                StatusTone::Ok => ("●", theme.ok),
                StatusTone::Warning => ("●", theme.warning),
                StatusTone::Danger => ("●", theme.danger),
                StatusTone::Neutral => ("○", theme.dim),
            };
            let dot = if theme.glyphs == Glyphs::Ascii {
                "*"
            } else {
                dot
            };
            buf.set_string(area.x + 1, area.y, dot, Style::default().fg(color));
            buf.set_string(
                area.x + 3,
                area.y,
                truncate(text, area.width.saturating_sub(4) as usize),
                if *tone == StatusTone::Ok {
                    theme.value()
                } else {
                    theme.label()
                },
            );
        }
        Row::Tiles {
            labels,
            selected,
            cursor,
            columns,
        } => {
            let cols = grid_columns(*columns, area.width);
            for (i, label) in labels.iter().enumerate() {
                let (r, c) = (i / cols, i % cols);
                if r as u16 >= area.height {
                    break;
                }
                let x = area.x + 1 + c as u16 * TILE_WIDTH;
                if x + TILE_WIDTH > area.x + area.width + 1 {
                    continue;
                }
                let text = format!("[{}]", fit_left(label, (TILE_WIDTH - 3) as usize));
                let mut style = if i == *selected {
                    theme.pill(theme.accent)
                } else {
                    theme.value()
                };
                if focused && i == *cursor {
                    style = style.add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
                }
                buf.set_string(x, area.y + r as u16, &text, style);
            }
        }
        Row::Transport {
            title,
            detail,
            running,
            enabled,
            cursor,
        } => {
            buf.set_string(
                area.x,
                area.y,
                if focused { "▸" } else { " " },
                theme.focused(),
            );
            let buttons: Vec<(&str, Style)> = if *running {
                vec![
                    ("Stop", theme.pill(theme.danger)),
                    ("Stop now", theme.value()),
                ]
            } else if *enabled {
                vec![("Start", theme.pill(theme.ok))]
            } else {
                vec![("Start", theme.label())]
            };
            let width: u16 = buttons
                .iter()
                .map(|(b, _)| b.chars().count() as u16 + 3)
                .sum();
            buf.set_string(
                area.x + 1,
                area.y,
                truncate(
                    title,
                    (area.width as usize).saturating_sub(width as usize + 2),
                ),
                if focused {
                    theme.focused()
                } else {
                    theme.value()
                },
            );
            let mut x = area.x + area.width - width;
            for (i, (label, style)) in buttons.iter().enumerate() {
                let text = format!(" {label} ");
                let style = if focused && i == *cursor {
                    style.add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
                } else {
                    *style
                };
                buf.set_string(x, area.y, &text, style);
                x += text.chars().count() as u16 + 1;
            }
            if area.height > 1 {
                buf.set_string(
                    area.x + 1,
                    area.y + 1,
                    truncate(detail, area.width.saturating_sub(2) as usize),
                    theme.label(),
                );
            }
        }
        Row::Banner { title, body } => {
            Banner::warning(title, theme).body(body).render(area, buf);
        }
        Row::Readout { title, value, tone } => {
            SectionHeader::new(title, theme).render(area, buf);
            let w = value.chars().count() as u16;
            if w + title.chars().count() as u16 + 4 <= area.width {
                let style = match tone {
                    StatusTone::Ok => Style::default().fg(theme.ok),
                    StatusTone::Warning => Style::default().fg(theme.warning),
                    StatusTone::Danger => Style::default().fg(theme.danger),
                    StatusTone::Neutral => theme.label(),
                };
                buf.set_string(area.x + area.width - w - 1, area.y, value, style);
            }
        }
        Row::Meters(meters) => draw_chip_meters(area, buf, theme, meters),
    }
}

/// Meters under a chip row, one per chip and as wide as it, laid out the way
/// `ChipRow` lays out its chips so each sits under its own.
fn draw_chip_meters(area: Rect, buf: &mut Buffer, theme: &Theme, meters: &[ChipMeter]) {
    let eighths = ['▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];
    let mut x = area.x + 1;
    for m in meters {
        // `ChipRow` draws `[label]`, with a mark before the label in mono.
        let w = m.label.chars().count() as u16
            + 2
            + u16::from(theme.depth == crate::theme::ColorDepth::Mono);
        if x + w > area.x + area.width {
            break;
        }
        let fill = Style::default().fg(if m.on { theme.accent } else { theme.dim });
        let track = Style::default().fg(theme.chrome_faint);
        let total = (m.fraction.clamp(0.0, 1.0) * w as f64 * 8.0).round() as u16;
        for i in 0..w {
            let here = total.saturating_sub(i * 8).min(8);
            let (sym, style) = match (here, theme.glyphs) {
                (0, Glyphs::Ascii) => ('.', track),
                (0, _) => ('░', track),
                (_, Glyphs::Ascii) => ('#', fill),
                (n, _) => (eighths[n as usize - 1], fill),
            };
            buf[(x + i, area.y)].set_char(sym).set_style(style);
        }
        x += w + 1;
    }
}

fn draw_graph(area: Rect, buf: &mut Buffer, theme: &Theme, g: &PanelGraph) {
    match g {
        PanelGraph::Disabled => {
            let text = "Disabled";
            let x = area.x + area.width.saturating_sub(text.len() as u16) / 2;
            buf.set_string(x, area.y + area.height / 2, text, theme.label());
        }
        PanelGraph::Curves { series, legend } => {
            let (mut lo, mut hi) = (f64::MAX, f64::MIN);
            for (_, _, points) in series {
                for v in points {
                    lo = lo.min(*v);
                    hi = hi.max(*v);
                }
            }
            if !lo.is_finite() || !hi.is_finite() {
                return;
            }
            // The Console's domain rule: at least a 10 dB window, then 20 %
            // padding above and below.
            let visual = (hi - lo).max(10.0);
            let pad = visual * 0.2;
            let (bottom, top) = (lo - pad, hi + pad);
            let settings = GraphSettings {
                min_hz: 20.0,
                max_hz: 20_000.0,
                db_center: (top + bottom) / 2.0,
                db_range: top - bottom,
                show_phase: false,
                unwrap_phase: false,
                freq_grid: true,
                freq_labels: true,
                db_grid: true,
                db_labels: true,
            };
            let curves: Vec<GraphCurve> = series
                .iter()
                .map(|(name, color, points)| GraphCurve {
                    descriptor: name.clone(),
                    color: *color,
                    magnitude: points.clone(),
                    phase: None,
                    selected: false,
                })
                .collect();
            Graph::new(&curves, &settings, theme).render(area, buf);
            for (i, (label, color)) in legend.iter().enumerate() {
                let text = format!("● {label}");
                let w = text.chars().count() as u16;
                if i as u16 >= area.height || w + 2 > area.width {
                    break;
                }
                buf.set_string(
                    area.x + area.width - w - 1,
                    area.y + i as u16,
                    &text,
                    Style::default().fg(*color),
                );
            }
        }
        PanelGraph::Bars {
            fc,
            original,
            harmonics,
        } => draw_bars(area, buf, theme, *fc, *original, *harmonics),
        PanelGraph::Transfer { output, knees } => draw_transfer(area, buf, theme, output, *knees),
        PanelGraph::Subharm {
            levels,
            boost,
            ceiling,
        } => draw_subharm(area, buf, theme, levels, *boost, *ceiling),
    }
}

/// The subharmonic synthesizer's bands, in `SubharmBandView`'s frame: 16 to
/// 250 Hz on a log axis, -42 to +18 dB, the dB marks hugging the empty left
/// edge and the frequency marks at 20, 50, 100 and 200.
///
/// Where the Console draws each band as a flat block at its level, each band
/// here is the shape the firmware's filters give it
/// ([`crate::curves::subharm_sub_db`]), filled in eighths of a row; where two
/// overlap, the louder is drawn. The LF boost bell is the one curve, and the
/// sub ceiling a dashed line across the sub range only, since it limits the
/// sub and not the program.
fn draw_subharm(
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    levels: &[f64; 3],
    boost: f64,
    ceiling: f64,
) {
    use crate::curves::{subharm_bell_db, subharm_sub_db};
    use dspi_proto::generated::ranges::{SUBHARM_CEILING_MAX, SUBHARM_LEVEL_MIN};
    const MIN_HZ: f64 = 16.0;
    const MAX_HZ: f64 = 250.0;
    const DB_MIN: f64 = -42.0;
    const DB_MAX: f64 = 18.0;
    let plot_h = area.height.saturating_sub(1);
    if plot_h < 2 || area.width < 16 {
        return;
    }
    let plot = Rect::new(area.x, area.y, area.width, plot_h);
    let hz_at = |frac: f64| MIN_HZ * (MAX_HZ / MIN_HZ).powf(frac);
    let x_of = |hz: f64| -> u16 {
        let f = (hz.clamp(MIN_HZ, MAX_HZ) / MIN_HZ).ln() / (MAX_HZ / MIN_HZ).ln();
        area.x + ((f * (area.width - 1) as f64).round() as u16).min(area.width - 1)
    };
    // Rows down from the top of the plot, continuous.
    let y_of = |db: f64| (DB_MAX - db.clamp(DB_MIN, DB_MAX)) / (DB_MAX - DB_MIN) * plot_h as f64;
    let row_of = |db: f64| plot.y + (y_of(db).floor() as u16).min(plot_h - 1);
    let ascii = theme.glyphs == Glyphs::Ascii;

    // The 0 dB reference.
    let zero = row_of(0.0);
    for x in plot.x..plot.x + plot.width {
        buf[(x, zero)]
            .set_symbol(if ascii { "-" } else { "┈" })
            .set_style(Style::default().fg(theme.chrome_faint));
    }

    let all_off = levels.iter().all(|l| *l <= SUBHARM_LEVEL_MIN as f64);
    let ceiling_on = !all_off && ceiling < SUBHARM_CEILING_MAX as f64;
    if all_off {
        let text = "All bands off";
        let x = area.x + area.width.saturating_sub(text.len() as u16) / 2;
        buf.set_string(x, plot.y + plot_h / 2, text, theme.label());
    } else {
        let blocks = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
        let style = Style::default().fg(theme.accent);
        let bottom = plot.y + plot_h - 1;
        for col in 0..plot.width {
            let hz = hz_at((col as f64 + 0.5) / plot.width as f64);
            let top = (0..3)
                .filter_map(|b| subharm_sub_db(b, levels[b], boost, hz))
                .fold(f64::NEG_INFINITY, f64::max);
            if !top.is_finite() {
                continue;
            }
            let eighths = ((plot_h as f64 - y_of(top)) * 8.0).round() as u16;
            for r in 0..plot_h {
                let here = eighths.saturating_sub(r * 8).min(8);
                if here == 0 {
                    break;
                }
                let sym = if ascii {
                    '#'
                } else {
                    blocks[here as usize - 1]
                };
                buf[(plot.x + col, bottom - r)]
                    .set_char(sym)
                    .set_style(style);
            }
        }
        if ceiling_on {
            let y = row_of(ceiling);
            for x in plot.x..=x_of(80.0) {
                buf[(x, y)]
                    .set_symbol(if ascii { "-" } else { "╌" })
                    .set_style(Style::default().fg(theme.danger));
            }
        }
    }

    if boost > 0.0 {
        crate::graph::draw_curve(
            plot,
            buf,
            theme,
            &|frac| subharm_bell_db(boost, hz_at(frac)),
            &|db, rows| (DB_MAX - db) / (DB_MAX - DB_MIN) * (rows - 1.0),
            false,
            Style::default().fg(theme.ok),
        );
    }

    // The level marks, only where nothing has been drawn.
    for db in [12.0, 0.0, -12.0, -24.0, -36.0] {
        let text = if db > 0.0 {
            format!("+{db:.0}")
        } else {
            format!("{db:.0}")
        };
        let y = row_of(db);
        let clear = (0..text.len() as u16).all(|i| {
            let s = buf[(plot.x + i, y)].symbol();
            s == " " || s == "┈" || s == "-"
        });
        if clear {
            buf.set_string(plot.x, y, &text, theme.label());
        }
    }

    // Each band's range over it on the top row, as the Console labels its
    // blocks, so bands that meet can still be told apart.
    for (b, (lo, hi)) in [(24.0f64, 36.0f64), (36.0, 56.0), (56.0, 80.0)]
        .iter()
        .enumerate()
    {
        if levels[b] <= SUBHARM_LEVEL_MIN as f64 {
            continue;
        }
        let text = format!("{lo:.0}-{hi:.0}");
        let mid = x_of((lo * hi).sqrt());
        let x = mid.saturating_sub(text.len() as u16 / 2);
        if x >= plot.x && x + (text.len() as u16) < area.x + area.width {
            buf.set_string(x, plot.y, &text, theme.label());
        }
    }

    // What the two marks are, top right, where the Curves legend goes.
    let mut legend: Vec<(&str, Color)> = Vec::new();
    if boost > 0.0 {
        legend.push(("● LF boost", theme.ok));
    }
    if ceiling_on {
        legend.push((if ascii { "- ceiling" } else { "╌ ceiling" }, theme.danger));
    }
    for (i, (label, color)) in legend.iter().enumerate() {
        let w = label.chars().count() as u16;
        if i as u16 >= plot_h || w + 2 > area.width {
            break;
        }
        buf.set_string(
            area.x + area.width - w - 1,
            area.y + i as u16,
            label,
            Style::default().fg(*color),
        );
    }

    // The frequency axis, in the Console's four marks.
    let y = area.y + plot_h;
    for (hz, label) in [(20.0, "20"), (50.0, "50"), (100.0, "100"), (200.0, "200")] {
        let x = x_of(hz).saturating_sub(label.len() as u16 / 2);
        if x + label.len() as u16 <= area.x + area.width {
            buf.set_string(x, y, label, theme.label());
        }
    }
}

/// The output range a transfer graph draws: a little room above full scale,
/// because hot trim and asymmetry can pass it (`TubeTransferView.yMax`).
const TRANSFER_Y_MAX: f64 = 1.4;

/// The tube's transfer curve over the grey axes, with the full-scale rules
/// and the knees marked the way the band edges are on the bass spectrum.
///
/// One curve, as `DESIGN.md` 12 asks: the Console's dashed straight line is
/// drawn as a faint dotted guide, part of the grid rather than a second
/// series, and its orange clip shading becomes the two knee marks.
fn draw_transfer(
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    output: &[f64],
    knees: (Option<f64>, Option<f64>),
) {
    if area.height < 3 || area.width < 8 || output.len() < 2 {
        return;
    }
    let (w, h) = (area.width, area.height);
    let col = |x: f64| area.x + (((x + 1.0) / 2.0 * (w - 1) as f64).round() as u16).min(w - 1);
    let row = |y: f64| {
        let c = y.clamp(-TRANSFER_Y_MAX, TRANSFER_Y_MAX);
        area.y
            + (((TRANSFER_Y_MAX - c) / (2.0 * TRANSFER_Y_MAX) * (h - 1) as f64).round() as u16)
                .min(h - 1)
    };
    let faint = Style::default().fg(theme.chrome_faint);

    // The zero axes, then the full-scale rules over them.
    let (x0, y0) = (col(0.0), row(0.0));
    for x in area.x..area.x + w {
        buf[(x, y0)].set_symbol("─").set_style(faint);
    }
    for y in area.y..area.y + h {
        buf[(x0, y)]
            .set_symbol(if y == y0 { "┼" } else { "│" })
            .set_style(faint);
    }
    for fs in [-1.0, 1.0] {
        let y = row(fs);
        for x in area.x..area.x + w {
            if x != x0 {
                buf[(x, y)].set_symbol("┄").set_style(faint);
            }
        }
    }
    for x in [knees.0, knees.1].into_iter().flatten() {
        let x = col(x);
        for y in area.y..area.y + h {
            buf[(x, y)].set_symbol("┆").set_style(theme.chrome_style());
        }
    }

    let to_row = |v: f64, rows: f64| (TRANSFER_Y_MAX - v) / (2.0 * TRANSFER_Y_MAX) * (rows - 1.0);
    let clipped = |v: f64| {
        if v.abs() > TRANSFER_Y_MAX {
            f64::NAN
        } else {
            v
        }
    };
    let reference = |frac: f64| clipped(frac * 2.0 - 1.0);
    crate::graph::draw_curve(area, buf, theme, &reference, &to_row, true, faint);
    let n = output.len() - 1;
    let sample = |frac: f64| {
        let at = frac.clamp(0.0, 1.0) * n as f64;
        let i = (at.floor() as usize).min(n - 1);
        let t = at - i as f64;
        clipped(output[i] + (output[i + 1] - output[i]) * t)
    };
    crate::graph::draw_curve(
        area,
        buf,
        theme,
        &sample,
        &to_row,
        false,
        Style::default().fg(theme.accent),
    );

    // The Console's three labels: the axes and full scale out.
    let label = |buf: &mut Buffer, x: u16, y: u16, text: &str| {
        if x + text.len() as u16 <= area.x + w {
            buf.set_string(x, y, text, theme.label());
        }
    };
    label(buf, area.x + w - 2, y0.saturating_sub(1).max(area.y), "in");
    label(buf, x0 + 2, area.y, "out");
    label(
        buf,
        area.x,
        row(1.0).saturating_sub(1).max(area.y),
        "0 dBFS",
    );
}

/// The psychoacoustic-bass spectrum: two blocks over a log frequency axis with
/// the cutoff and its fourth harmonic marked, as `PsybassSpectrumView` draws
/// them.
fn draw_bars(area: Rect, buf: &mut Buffer, theme: &Theme, fc: f64, original: f64, harmonics: f64) {
    let (min_hz, max_hz) = (20.0f64, 20_000.0f64);
    let plot_h = area.height.saturating_sub(1);
    if plot_h == 0 || area.width < 8 {
        return;
    }
    let x_of = |hz: f64| -> u16 {
        let f =
            (hz.clamp(min_hz, max_hz).log10() - min_hz.log10()) / (max_hz.log10() - min_hz.log10());
        area.x + ((f * (area.width - 1) as f64).round() as u16).min(area.width - 1)
    };
    let fc_x = x_of(fc);
    let fc4_x = x_of(fc * 4.0);
    let base = area.y + plot_h - 1;

    let bar = |buf: &mut Buffer, x0: u16, x1: u16, frac: f64, color: Color| {
        let rows = (frac.clamp(0.0, 1.0) * plot_h as f64).round() as u16;
        for x in x0..x1.max(x0) {
            for r in 0..rows {
                buf[(x, base - r)]
                    .set_symbol("▓")
                    .set_style(Style::default().fg(color));
            }
        }
    };
    bar(buf, area.x, fc_x, original, theme.accent);
    bar(buf, fc_x, fc4_x, harmonics, theme.warning);

    for (x, label) in [(fc_x, "fc"), (fc4_x, "4fc")] {
        for y in area.y..=base {
            let cell = &mut buf[(x, y)];
            if cell.symbol() == " " {
                cell.set_symbol("┆").set_style(theme.chrome_style());
            }
        }
        if x + label.len() as u16 <= area.x + area.width {
            buf.set_string(x, area.y, label, theme.label());
        }
    }

    // The frequency axis, in the Console's three marks.
    let y = area.y + plot_h;
    for (hz, label) in [(100.0, "100"), (1000.0, "1k"), (10_000.0, "10k")] {
        let x = x_of(hz).saturating_sub(label.len() as u16 / 2);
        if x + label.len() as u16 <= area.x + area.width {
            buf.set_string(x, y, label, theme.label());
        }
    }
}

/// The Console's unsupported body: a warning banner with the version note and
/// the line telling the reader what to do about it.
pub fn draw_unsupported(area: Rect, buf: &mut Buffer, theme: &Theme, note: &str, action: &str) {
    if area.height == 0 {
        return;
    }
    Banner::warning(note, theme).body(action).render(area, buf);
}

/// The pill a panel shows in place of a toggle, for panels whose header
/// carries a state rather than a switch.
pub fn draw_pill(area: Rect, buf: &mut Buffer, theme: &Theme, text: &str, tone: StatusTone) {
    let pill = StatusPill::new(text, tone, theme);
    let w = pill.width();
    if w <= area.width {
        pill.render(Rect::new(area.x + area.width - w, area.y, w, 1), buf);
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use dspi_session::DeviceState;
    use dspi_session::probe::{Feature, SiggenCaps};

    /// The shell fixture with every tool panel's feature reported present, so
    /// the golden frames show the controls rather than the banner. A panel's
    /// banner is tested against the plain fixture, which has no features at
    /// all, exactly as an old firmware would answer.
    pub fn state() -> DeviceState {
        let mut s = crate::shell::fixture::state();
        for name in [
            "psychoacoustic_bass",
            "upmixer",
            "test_signals",
            "subharmonic_synth",
        ] {
            s.caps.features.push(Feature {
                name: name.into(),
                present: true,
                evidence: "mock".into(),
            });
        }
        s.caps.siggen = Some(SiggenCaps {
            version: 1,
            type_count: 15,
            output_channels: 9,
            multitone_max: 16,
            valid_channel_mask: 0x1FF,
            types: Vec::new(),
        });
        s
    }

    /// Draw one panel alone, as text.
    pub fn draw(
        screen: &mut dyn crate::shell::Screen,
        state: &DeviceState,
        w: u16,
        h: u16,
    ) -> String {
        let t = crate::theme::Theme::console(
            crate::theme::ColorDepth::TrueColor,
            crate::theme::Glyphs::Braille,
        );
        let mut term =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).expect("backend");
        term.draw(|f| screen.draw(f.area(), f.buffer_mut(), &t, state, true))
            .expect("draw");
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

    /// The foreground colour the row holding `needle` is drawn in, taken from
    /// the first cell of that word. A text frame cannot tell a dimmed row from
    /// a live one, and the dimming rules are half the parity work.
    pub fn fg_of(
        screen: &mut dyn crate::shell::Screen,
        state: &DeviceState,
        w: u16,
        h: u16,
        needle: &str,
    ) -> ratatui::style::Color {
        let t = crate::theme::Theme::console(
            crate::theme::ColorDepth::TrueColor,
            crate::theme::Glyphs::Braille,
        );
        let mut term =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).expect("backend");
        term.draw(|f| screen.draw(f.area(), f.buffer_mut(), &t, state, true))
            .expect("draw");
        let buf = term.backend().buffer();
        for y in 0..h {
            let line: String = (0..w).map(|x| buf[(x, y)].symbol()).collect();
            if let Some(at) = line.find(needle) {
                let x = line[..at].chars().count() as u16;
                return buf[(x, y)].fg;
            }
        }
        panic!("{needle:?} is not on the frame");
    }

    /// A tool panel inside the shell, so a golden frame is what a person
    /// actually sees at 80x24 and 120x40.
    pub fn frame(
        tool: crate::shell::Tool,
        screen: Box<dyn crate::shell::Screen>,
        state: &DeviceState,
        w: u16,
        h: u16,
    ) -> String {
        let theme = crate::theme::Theme::console(
            crate::theme::ColorDepth::TrueColor,
            crate::theme::Glyphs::Braille,
        );
        let model = crate::shell::fixture::rp2350(&theme);
        let mut shell = crate::shell::Shell::new(
            model,
            theme,
            Box::new(crate::shell::Placeholder::new("Overview", "")),
        );
        shell.open_tool(tool, screen);
        let mut term =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).expect("backend");
        term.draw(|f| shell.draw(f.area(), f.buffer_mut(), state))
            .expect("draw");
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ColorDepth, Glyphs};

    fn theme() -> Theme {
        Theme::console(ColorDepth::TrueColor, Glyphs::Braille)
    }

    #[test]
    fn the_window_scrolls_only_far_enough_to_show_the_focused_row() {
        let heights = [1u16, 3, 3, 3, 3, 3];
        // Everything above the fold already fits.
        assert_eq!(window(&heights, 10, 0, Some(2)), 0);
        // The fifth row needs two rows scrolled away.
        assert_eq!(window(&heights, 10, 0, Some(4)), 2);
        // Going back up scrolls back to the focused row itself.
        assert_eq!(window(&heights, 10, 2, Some(0)), 0);
        // No focus means no movement.
        assert_eq!(window(&heights, 10, 3, None), 3);
    }

    #[test]
    fn rows_report_the_height_they_want() {
        let t = theme();
        let _ = &t;
        let p = Row::Param(Param::new("Cutoff", 700.0, 500.0, 2000.0, "Hz").caption("A caption"));
        assert_eq!(p.height(60), 3);
        // At 80 columns the panel is narrow, so a parameter collapses to one
        // compact line.
        assert_eq!(p.height(36), 1);
        assert_eq!(Row::Blank.height(60), 1);
        assert_eq!(Row::Graph(PanelGraph::Disabled).height(60), GRAPH_ROWS);
        assert_eq!(
            Row::Tiles {
                labels: (0..15).map(|i| format!("t{i}")).collect(),
                selected: 0,
                cursor: 0,
                columns: 4,
            }
            .height(60),
            4
        );
    }

    #[test]
    fn only_the_control_rows_take_focus() {
        let rows = vec![
            Row::Section {
                title: "Parameters".into(),
                action: None,
            },
            Row::Param(Param::new("A", 1.0, 0.0, 2.0, "")),
            Row::Caption("note".into()),
            Row::Toggle {
                label: "B".into(),
                on: true,
                caption: None,
                enabled: true,
            },
        ];
        assert_eq!(focus_rows(&rows), vec![1, 3]);
    }

    #[test]
    fn a_chip_row_cycles_polarity_only_when_it_is_asked_to() {
        let t = theme();
        let chips = vec![ChipSpec {
            label: "1".into(),
            state: ChipState::On,
            color: t.outputs[0],
            enabled: true,
            dimmed: false,
        }];
        let plain = Row::Chips {
            chips: chips.clone(),
            cursor: 0,
            polarity: false,
        };
        let polar = Row::Chips {
            chips,
            cursor: 0,
            polarity: true,
        };
        let space = KeyEvent::new(KeyCode::Char(' '), crossterm::event::KeyModifiers::NONE);
        assert_eq!(
            plain.handle(space, &t),
            Some(Action::Chip(0, ChipState::Off))
        );
        assert_eq!(
            polar.handle(space, &t),
            Some(Action::Chip(0, ChipState::Inverted))
        );
    }

    #[test]
    fn a_tile_grid_walks_in_two_dimensions() {
        let t = theme();
        let tiles = |cursor: usize| Row::Tiles {
            labels: (0..15).map(|i| format!("t{i}")).collect(),
            selected: 0,
            cursor,
            columns: 4,
        };
        let key = |c| KeyEvent::new(c, crossterm::event::KeyModifiers::NONE);
        assert_eq!(
            tiles(0).handle(key(KeyCode::Down), &t),
            Some(Action::Selected(4))
        );
        assert_eq!(
            tiles(4).handle(key(KeyCode::Up), &t),
            Some(Action::Selected(0))
        );
        assert_eq!(
            tiles(0).handle(key(KeyCode::Left), &t),
            Some(Action::Selected(0)),
            "no wrap at the start"
        );
        assert_eq!(
            tiles(14).handle(key(KeyCode::Down), &t),
            Some(Action::Selected(14)),
            "no wrap at the end"
        );
        assert_eq!(
            tiles(3).handle(key(KeyCode::Enter), &t),
            Some(Action::Button(3))
        );
    }

    /// A panel without a header numbers its rows from 1 as well, so its
    /// last row is as reachable as a headed panel's.
    #[test]
    fn a_headless_body_reaches_its_last_row() {
        let rows = vec![
            Row::Param(Param::new("A", 1.0, 0.0, 2.0, "")),
            Row::Param(Param::new("B", 1.0, 0.0, 2.0, "")),
        ];
        let mut b = Body::headless();
        assert!(b.step(1, &rows));
        assert_eq!(b.focused_row(&rows), Some(1));
        assert!(!b.step(1, &rows), "and stops there");
        b.focus = 9;
        b.clamp(&rows);
        assert_eq!(b.focus, 2);
        let mut headed = Body::default();
        headed.step(1, &rows);
        headed.step(1, &rows);
        assert_eq!(headed.focused_row(&rows), Some(1));
        assert!(!Body::headless().step(1, &[]), "nothing to move to");
    }

    #[test]
    fn a_feature_the_probe_never_saw_is_absent() {
        let mut s = crate::shell::fixture::state();
        assert!(!has_feature(&s, "upmixer"));
        s.caps.features.push(dspi_session::probe::Feature {
            name: "upmixer".into(),
            present: true,
            evidence: String::new(),
        });
        assert!(has_feature(&s, "upmixer"));
        s.caps.features[0].present = false;
        assert!(!has_feature(&s, "upmixer"));
    }

    #[test]
    fn the_output_masks_follow_the_device() {
        let s = crate::shell::fixture::state();
        assert_eq!(all_outputs_mask(&s), 0x1FF);
        assert_eq!(sub_output(&s), 8);
    }
}
