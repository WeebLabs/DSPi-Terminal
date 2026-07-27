//! The application shell: state, layout, rendering and input.
//!
//! Three layers of interaction live here, and each is a complete way to work:
//! browse with the arrow keys, search with the palette, or type with the `:`
//! line. The echo line ties them together by showing the command equivalent of
//! whatever was just done, in the exact syntax a shell would accept.

use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use dspi_cmd::{Candidate, Context};
use dspi_proto::dsp;
use dspi_proto::registry::Group;
use dspi_session::Session;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Row, Table};

use crate::fields::{Field, FieldState, Targets, fields_for, nudge};
use crate::theme::{Glyphs, Theme};
use crate::widgets::{Bode, Curve, Meter, frequency_axis};

/// How hard the interface is allowed to work.
///
/// A Raspberry Pi reached over SSH pays for every repaint in bytes on the wire,
/// not in processor time, so the useful lever is how often the screen changes
/// rather than how fast it can be drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Performance {
    /// How often to poll the device's meters.
    pub meter_interval: Duration,
    /// How long to wait for a keypress before redrawing.
    pub event_timeout: Duration,
    pub animate: bool,
}

impl Default for Performance {
    fn default() -> Self {
        Self {
            meter_interval: Duration::from_millis(50),
            event_timeout: Duration::from_millis(30),
            animate: true,
        }
    }
}

impl Performance {
    /// The conservative profile: meters at 10 Hz and no animation.
    pub fn lite() -> Self {
        Self {
            meter_interval: Duration::from_millis(100),
            event_timeout: Duration::from_millis(100),
            animate: false,
        }
    }

    /// Pick a profile from the machine and the connection.
    ///
    /// A single-core part or an ARMv6 (a Pi Zero or an original Pi) gets the
    /// lite profile, as does any session reached over SSH, where the cost of a
    /// repaint is the link rather than the processor.
    pub fn detect() -> Self {
        if is_low_powered() || over_ssh() {
            Self::lite()
        } else {
            Self::default()
        }
    }
}

/// True for a single-core machine or an ARMv6 part.
fn is_low_powered() -> bool {
    if std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        <= 1
    {
        return true;
    }
    // Only Linux exposes this, and it is the only place a Pi Zero appears.
    std::fs::read_to_string("/proc/cpuinfo")
        .map(|s| s.contains("ARMv6"))
        .unwrap_or(false)
}

fn over_ssh() -> bool {
    std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some()
}

/// The panels, in signal-flow order. Crossover folds into Filters as a sub-tab
/// and Output folds into Matrix, keeping the bar on one line at 80 columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Dashboard,
    Input,
    Filters,
    Matrix,
    Dynamics,
    Spatial,
    Surfaces,
    Presets,
    System,
    Raw,
}

impl Panel {
    pub const ALL: [Panel; 10] = [
        Panel::Dashboard,
        Panel::Input,
        Panel::Filters,
        Panel::Matrix,
        Panel::Dynamics,
        Panel::Spatial,
        Panel::Surfaces,
        Panel::Presets,
        Panel::System,
        Panel::Raw,
    ];

    /// Which registry group supplies this panel's fields, when it is a plain
    /// field list rather than a bespoke layout.
    pub fn group(self) -> Option<Group> {
        Some(match self {
            Panel::Input => Group::Input,
            Panel::Dynamics => Group::Dynamics,
            Panel::Spatial => Group::Spatial,
            Panel::Presets => Group::Presets,
            Panel::System => Group::System,
            Panel::Surfaces => Group::Surfaces,
            _ => return None,
        })
    }

    pub fn title(self) -> &'static str {
        match self {
            Panel::Dashboard => "Dashboard",
            Panel::Input => "Input",
            Panel::Filters => "Filters",
            Panel::Matrix => "Matrix",
            Panel::Dynamics => "Dynamics",
            Panel::Spatial => "Spatial",
            Panel::Surfaces => "Surfaces",
            Panel::Presets => "Presets",
            Panel::System => "System",
            Panel::Raw => "Raw",
        }
    }
}

/// What the user is typing into, if anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Browse,
    Command,
    Palette,
}

/// How much of the interface is on show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Simple,
    Advanced,
    Expert,
}

impl Level {
    fn next(self) -> Self {
        match self {
            Level::Simple => Level::Advanced,
            Level::Advanced => Level::Expert,
            Level::Expert => Level::Simple,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Level::Simple => "Simple",
            Level::Advanced => "Advanced",
            Level::Expert => "Expert",
        }
    }
}

/// How the graph and the band table share the Filters panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Split {
    GraphOnly,
    Split,
    TableOnly,
}

impl Split {
    fn next(self) -> Self {
        match self {
            Split::GraphOnly => Split::Split,
            Split::Split => Split::TableOnly,
            Split::TableOnly => Split::GraphOnly,
        }
    }
}

/// One channel's live state, as the interface needs it.
#[derive(Debug, Clone, Default)]
pub struct ChannelView {
    pub name: String,
    pub slug: String,
    pub is_output: bool,
    pub bands: Vec<dsp::Band>,
    pub curve: Vec<f64>,
    pub peak: f32,
    pub clipped: bool,
}

pub struct App {
    pub theme: Theme,
    pub panel: Panel,
    pub mode: Mode,
    pub level: Level,
    pub split: Split,

    pub channels: Vec<ChannelView>,
    pub selected_channel: usize,
    pub selected_band: usize,

    pub device: String,
    pub platform: String,
    pub firmware: String,
    pub preset: String,
    pub dirty: bool,

    pub cpu: (u8, u8),
    /// How many inputs are actually carrying audio, per the device.
    pub active_inputs: u8,

    /// Vertical dB window, shared by every view so a comparison never silently
    /// changes scale underneath the reader.
    pub db_range: f64,
    /// Cursor position along the curve, as a point index; `None` hides it.
    pub cursor: Option<usize>,
    pub show_phase: bool,
    pub phase_unwrapped: bool,
    /// Grid mode draws one small plot per channel instead of an overlay.
    pub grid_mode: bool,
    /// Which channels the graph shows.
    pub visible: Vec<bool>,
    pub meters_expanded: bool,
    pub graph_expanded: bool,

    /// The `:` line or palette query.
    pub input: String,
    pub candidates: Vec<Candidate>,
    pub candidate_index: usize,

    /// The canonical command form of the last change, for the echo line.
    pub echo: String,
    pub status: Option<(String, Instant)>,

    /// The crosspoint grid, indexed `[input][output]`.
    pub matrix: Vec<Vec<dspi_session::Crosspoint>>,
    pub outputs: Vec<dspi_session::OutputStrip>,
    pub matrix_focus: (usize, usize),

    /// The current panel's rows, when it is a field list.
    pub fields: Vec<Field>,
    pub selected_field: usize,

    pub ctx: Context,
    pub perf: Performance,
    pub should_quit: bool,
}

impl App {
    pub fn new(theme: Theme) -> Self {
        Self {
            theme,
            panel: Panel::Dashboard,
            mode: Mode::Browse,
            level: Level::Advanced,
            split: Split::Split,
            channels: Vec::new(),
            selected_channel: 0,
            selected_band: 0,
            device: String::new(),
            platform: String::new(),
            firmware: String::new(),
            preset: String::new(),
            dirty: false,
            cpu: (0, 0),
            active_inputs: 0,
            db_range: 30.0,
            cursor: None,
            show_phase: false,
            phase_unwrapped: false,
            grid_mode: false,
            visible: Vec::new(),
            meters_expanded: false,
            graph_expanded: false,
            input: String::new(),
            candidates: Vec::new(),
            candidate_index: 0,
            echo: String::new(),
            status: None,
            matrix: Vec::new(),
            outputs: Vec::new(),
            matrix_focus: (0, 0),
            fields: Vec::new(),
            selected_field: 0,
            ctx: Context::default(),
            perf: Performance::detect(),
            should_quit: false,
        }
    }

    /// Seed the interface from a connected device.
    pub fn from_session(theme: Theme, session: &Session) -> Self {
        let mut app = Self::new(theme);
        let caps = session.capabilities();

        app.device = caps.serial.clone();
        app.platform = caps.platform.name();
        app.firmware = caps.firmware.clone();
        app.preset = caps
            .active_preset
            .map(|p| format!("Preset {p}"))
            .unwrap_or_default();

        app.channels = caps
            .channels
            .iter()
            .map(|c| ChannelView {
                name: c.name.clone(),
                slug: c.slug.clone(),
                is_output: c.is_output,
                bands: vec![dsp::Band::default(); caps.max_bands as usize],
                curve: vec![0.0; dsp::POINTS],
                peak: 0.0,
                clipped: false,
            })
            .collect();

        app.visible = vec![true; app.channels.len()];
        app.ctx = Context {
            channel_slugs: caps.channels.iter().map(|c| c.slug.clone()).collect(),
            num_inputs: caps.num_inputs,
            num_outputs: caps.num_outputs,
            max_bands: caps.max_bands,
        };
        app
    }

    /// The frequency the cursor sits on.
    pub fn cursor_hz(&self) -> Option<f64> {
        self.cursor
            .map(|i| dsp::frequencies()[i.min(dsp::POINTS - 1)])
    }

    /// What every visible channel reads at the cursor.
    ///
    /// A numeric readout is something a terminal does better than a graphical
    /// interface, because the numbers are already text.
    pub fn cursor_readout(&self) -> Vec<(String, f64)> {
        let Some(i) = self.cursor else {
            return Vec::new();
        };
        self.channels
            .iter()
            .enumerate()
            .filter(|(n, _)| self.visible.get(*n).copied().unwrap_or(true))
            .filter_map(|(_, c)| c.curve.get(i).map(|db| (c.slug.clone(), *db)))
            .collect()
    }

    fn move_cursor(&mut self, delta: isize) {
        let next = match self.cursor {
            None => dsp::POINTS as isize / 2,
            Some(i) => i as isize + delta,
        };
        self.cursor = Some(next.clamp(0, dsp::POINTS as isize - 1) as usize);
    }

    /// Rebuild the current panel's field list.
    ///
    /// Called on panel and level changes. The list comes from the registry, so
    /// a firmware parameter added tomorrow appears here with no work.
    pub fn rebuild_fields(&mut self) {
        self.fields = match self.panel.group() {
            Some(g) => fields_for(
                g,
                self.level,
                &Targets {
                    inputs: self.ctx.num_inputs,
                    outputs: self.ctx.num_outputs,
                    channels: self.ctx.num_channels(),
                },
            ),
            None => Vec::new(),
        };
        self.selected_field = 0;
    }

    /// Read the routing grid.
    pub fn load_matrix(&mut self, session: &mut Session) {
        if let Ok((grid, strips)) = session.read_matrix() {
            self.matrix = grid;
            self.outputs = strips;
        }
    }

    /// Read every row of the current panel from the device.
    ///
    /// A parameter the device does not have is marked unavailable with the
    /// reason, rather than left blank or silently dropped: "this firmware has no
    /// upmixer" is useful, an empty row is not.
    pub fn load_fields(&mut self, session: &mut Session) {
        for i in 0..self.fields.len() {
            let (path, indices) = (self.fields[i].path, self.fields[i].indices.clone());
            match session.read(path, &indices) {
                Ok(v) => {
                    self.fields[i].value = Some(v);
                    self.fields[i].state = FieldState::Settled;
                    self.fields[i].unavailable = None;
                }
                Err(dspi_session::WriteError::Unavailable { why, .. }) => {
                    self.fields[i].unavailable = Some(why);
                    self.fields[i].state = FieldState::Unknown;
                }
                Err(_) => {
                    // A read that fails for another reason leaves the row blank
                    // rather than claiming a value we do not have.
                    self.fields[i].state = FieldState::Unknown;
                }
            }
        }
    }

    /// Adjust the focused field and write it.
    ///
    /// Everything goes through the session's single write path, so a field edit
    /// gets the same validation, capability gating and readback verification as
    /// a typed command. The outcome is recorded on the row, which is how a
    /// silent rejection becomes visible.
    pub fn edit_focused(&mut self, session: &mut Session, up: bool, coarse: bool) {
        let Some(field) = self.fields.get(self.selected_field) else {
            return;
        };
        if !field.is_editable() {
            return;
        }
        let (Some(d), Some(current)) = (field.desc(), field.value.clone()) else {
            return;
        };
        let Some(next) = nudge(d, &current, up, coarse) else {
            return;
        };

        let (path, indices) = (field.path, field.indices.clone());
        let i = self.selected_field;
        self.fields[i].state = FieldState::Pending;

        match session.write(path, &indices, next.clone()) {
            Ok(dspi_session::Outcome::Rejected { actual, .. }) => {
                self.fields[i].value = Some(actual.clone());
                self.fields[i].state = FieldState::Rejected(actual);
            }
            Ok(_) => {
                self.fields[i].value = Some(next.clone());
                self.fields[i].state = FieldState::Settled;
                self.dirty = true;
                // Echo the change as a command, so the interface teaches the
                // syntax simply by being used.
                self.echo = dspi_cmd::format(
                    &dspi_cmd::Command::Set {
                        path,
                        indices,
                        value: next,
                    },
                    &self.ctx,
                );
            }
            Err(e) => {
                self.fields[i].state = FieldState::Unknown;
                self.note(e.to_string());
            }
        }
    }

    pub fn focused_field(&self) -> Option<&Field> {
        self.fields.get(self.selected_field)
    }

    /// Take a meter poll.
    pub fn apply_meters(&mut self, m: &dspi_session::Meters) {
        for (i, c) in self.channels.iter_mut().enumerate() {
            c.peak = m.peaks.get(i).copied().unwrap_or(0.0);
            c.clipped = m.clipped.get(i).copied().unwrap_or(false);
        }
        self.cpu = (m.cpu0, m.cpu1);
        self.active_inputs = m.active_inputs;
    }

    /// Recompute one channel's curve after its bands change.
    pub fn recompute(&mut self, channel: usize) {
        if let Some(c) = self.channels.get_mut(channel) {
            c.curve = dsp::curve(&c.bands, 0.0);
        }
    }

    pub fn selected(&self) -> Option<&ChannelView> {
        self.channels.get(self.selected_channel)
    }

    fn note(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now()));
    }

    // ---------------------------------------------------------------- input

    pub fn on_key(&mut self, key: KeyEvent) {
        match self.mode {
            Mode::Browse => self.on_browse_key(key),
            Mode::Command | Mode::Palette => self.on_input_key(key),
        }
    }

    fn on_browse_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('c') if ctrl => self.should_quit = true,

            KeyCode::Char('p') if ctrl => {
                self.mode = Mode::Palette;
                self.input.clear();
                self.refresh_candidates();
            }
            KeyCode::Char(':') => {
                self.mode = Mode::Command;
                self.input.clear();
                self.refresh_candidates();
            }

            KeyCode::Tab => self.cycle_panel(1),
            KeyCode::BackTab => self.cycle_panel(-1),
            KeyCode::Char(c @ '1'..='9') => {
                let i = c as usize - '1' as usize;
                if let Some(p) = Panel::ALL.get(i) {
                    self.panel = *p;
                    self.rebuild_fields();
                }
            }
            // The guarded arm must precede the plain one, or it never matches.
            KeyCode::Char('0') if self.panel == Panel::Filters => self.db_range = 30.0,
            KeyCode::Char('0') => self.panel = Panel::Raw,

            KeyCode::Char('M') => self.meters_expanded = !self.meters_expanded,
            KeyCode::Char('G') => self.graph_expanded = !self.graph_expanded,
            KeyCode::Char('=') => {
                self.split = self.split.next();
                let s = format!("{:?}", self.split);
                self.note(format!("layout: {s}"));
            }
            KeyCode::F(2) => {
                self.level = self.level.next();
                let l = self.level.name();
                self.note(format!("showing {l} controls"));
                self.rebuild_fields();
            }

            KeyCode::Up if self.panel == Panel::Matrix => self.move_matrix(-1, 0),
            KeyCode::Down if self.panel == Panel::Matrix => self.move_matrix(1, 0),
            KeyCode::Left if self.panel == Panel::Matrix => self.move_matrix(0, -1),
            KeyCode::Right if self.panel == Panel::Matrix => self.move_matrix(0, 1),
            KeyCode::Up if !self.fields.is_empty() => self.move_field(-1),
            KeyCode::Down if !self.fields.is_empty() => self.move_field(1),
            KeyCode::Up => self.move_band(-1),
            KeyCode::Down => self.move_band(1),
            KeyCode::Left => self.move_channel(-1),
            KeyCode::Right => self.move_channel(1),

            // Graph controls. `h`/`l` move the cursor rather than the selection,
            // so reading a curve never disturbs what is being edited.
            KeyCode::Char('h') => self.move_cursor(-2),
            KeyCode::Char('l') => self.move_cursor(2),
            KeyCode::Char('H') => self.move_cursor(-20),
            KeyCode::Char('L') => self.move_cursor(20),
            KeyCode::Char('x') => self.cursor = None,
            KeyCode::Char('+') => self.db_range = (self.db_range - 5.0).max(10.0),
            KeyCode::Char('-') => self.db_range = (self.db_range + 5.0).min(100.0),
            KeyCode::Char('m') => self.grid_mode = !self.grid_mode,
            KeyCode::Char('P') => self.show_phase = !self.show_phase,
            KeyCode::Char('u') => self.phase_unwrapped = !self.phase_unwrapped,
            KeyCode::Char(' ') => {
                // Toggle the selected channel's visibility, the keyboard
                // equivalent of clicking its legend pill.
                if let Some(v) = self.visible.get_mut(self.selected_channel) {
                    *v = !*v;
                }
            }

            _ => {}
        }
    }

    fn on_input_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Browse;
                self.input.clear();
                self.candidates.clear();
            }
            KeyCode::Enter => {
                // Executing against a device belongs to the caller; the shell
                // records what was asked for so the echo line stays honest even
                // when there is nothing connected.
                if !self.input.is_empty() {
                    self.echo = self.input.clone();
                }
                self.mode = Mode::Browse;
                self.input.clear();
                self.candidates.clear();
            }
            KeyCode::Tab => {
                if let Some(c) = self.candidates.get(self.candidate_index)
                    && !c.value.is_empty()
                {
                    let mut tokens: Vec<&str> = self.input.split_whitespace().collect();
                    if !self.input.ends_with(' ') {
                        tokens.pop();
                    }
                    let mut next = tokens.join(" ");
                    if !next.is_empty() {
                        next.push(' ');
                    }
                    next.push_str(&c.value);
                    next.push(' ');
                    self.input = next;
                    self.refresh_candidates();
                }
            }
            KeyCode::Down => {
                if !self.candidates.is_empty() {
                    self.candidate_index = (self.candidate_index + 1) % self.candidates.len();
                }
            }
            KeyCode::Up => {
                if !self.candidates.is_empty() {
                    self.candidate_index = self
                        .candidate_index
                        .checked_sub(1)
                        .unwrap_or(self.candidates.len() - 1);
                }
            }
            KeyCode::Backspace => {
                self.input.pop();
                self.refresh_candidates();
            }
            KeyCode::Char(c) => {
                self.input.push(c);
                self.refresh_candidates();
            }
            _ => {}
        }
    }

    /// Ask the shared grammar what could come next.
    pub fn refresh_candidates(&mut self) {
        let ends_with_space = self.input.ends_with(' ');
        let mut tokens: Vec<&str> = self.input.split_whitespace().collect();
        let partial = if ends_with_space {
            ""
        } else {
            tokens.pop().unwrap_or("")
        };

        self.candidates = dspi_cmd::complete(&tokens, partial, &self.ctx);
        self.candidate_index = 0;
    }

    fn move_matrix(&mut self, di: isize, dobj: isize) {
        if self.matrix.is_empty() {
            return;
        }
        let rows = self.matrix.len() as isize;
        let cols = self.matrix[0].len() as isize;
        self.matrix_focus.0 = ((self.matrix_focus.0 as isize + di).rem_euclid(rows)) as usize;
        self.matrix_focus.1 = ((self.matrix_focus.1 as isize + dobj).rem_euclid(cols)) as usize;
    }

    fn move_field(&mut self, delta: isize) {
        if self.fields.is_empty() {
            return;
        }
        let n = self.fields.len() as isize;
        self.selected_field = ((self.selected_field as isize + delta).rem_euclid(n)) as usize;
    }

    fn cycle_panel(&mut self, delta: isize) {
        let i = Panel::ALL
            .iter()
            .position(|p| *p == self.panel)
            .unwrap_or(0) as isize;
        let n = Panel::ALL.len() as isize;
        self.panel = Panel::ALL[((i + delta).rem_euclid(n)) as usize];
        self.rebuild_fields();
    }

    fn move_channel(&mut self, delta: isize) {
        if self.channels.is_empty() {
            return;
        }
        let n = self.channels.len() as isize;
        self.selected_channel = ((self.selected_channel as isize + delta).rem_euclid(n)) as usize;
        self.selected_band = 0;
    }

    fn move_band(&mut self, delta: isize) {
        let Some(c) = self.channels.get(self.selected_channel) else {
            return;
        };
        if c.bands.is_empty() {
            return;
        }
        let n = c.bands.len() as isize;
        self.selected_band = ((self.selected_band as isize + delta).rem_euclid(n)) as usize;
    }

    // -------------------------------------------------------------- drawing

    pub fn draw(&self, f: &mut Frame) {
        let area = f.area();

        // Below this there is not enough room to be useful, and pretending
        // otherwise produces an unreadable mess.
        if area.width < 60 || area.height < 12 {
            f.render_widget(
                Paragraph::new("Terminal too small.\nDSPi needs at least 60x12.")
                    .style(Style::default().fg(self.theme.danger)),
                area,
            );
            return;
        }

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // title
                Constraint::Length(1), // tabs
                Constraint::Min(3),    // body
                Constraint::Length(1), // echo
                Constraint::Length(1), // keys
            ])
            .split(area);

        self.draw_title(f, rows[0]);
        self.draw_tabs(f, rows[1]);

        if self.meters_expanded {
            self.draw_meter_bridge(f, rows[2]);
        } else {
            let body = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Min(30), Constraint::Length(26)])
                .split(rows[2]);
            self.draw_panel(f, body[0]);
            self.draw_meter_rail(f, body[1]);
        }

        self.draw_echo(f, rows[3]);
        self.draw_keys(f, rows[4]);

        if self.mode != Mode::Browse {
            self.draw_input_overlay(f, area);
        }
    }

    fn draw_title(&self, f: &mut Frame, area: Rect) {
        let mut spans = vec![
            Span::styled("DSPi", self.theme.focused()),
            Span::styled("  ", self.theme.label()),
        ];
        if !self.platform.is_empty() {
            spans.push(Span::styled(
                format!("{} · fw {}  ", self.platform, self.firmware),
                self.theme.label(),
            ));
        }
        if !self.preset.is_empty() {
            spans.push(Span::styled(self.preset.clone(), self.theme.value()));
        }
        if self.dirty {
            // Unsaved live state is the protocol's sharpest edge, so it is
            // always visible rather than discoverable.
            spans.push(Span::styled(" ●", Style::default().fg(self.theme.pending)));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn draw_tabs(&self, f: &mut Frame, area: Rect) {
        let mut spans = Vec::new();
        for p in Panel::ALL {
            if self.level < Level::Expert && p == Panel::Raw {
                continue;
            }
            let style = if p == self.panel {
                self.theme.focused()
            } else {
                self.theme.label()
            };
            spans.push(Span::styled(format!(" {} ", p.title()), style));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn draw_panel(&self, f: &mut Frame, area: Rect) {
        match self.panel {
            Panel::Dashboard => self.draw_dashboard(f, area),
            Panel::Filters => self.draw_filters(f, area),
            Panel::Matrix => self.draw_matrix(f, area),
            other => match other.group() {
                Some(_) => self.draw_fields(f, area, other.title()),
                None => {
                    let block = Block::default()
                        .borders(Borders::ALL)
                        .border_style(self.theme.chrome_style())
                        .title(other.title());
                    f.render_widget(
                        Paragraph::new("Not built yet.")
                            .style(self.theme.label())
                            .block(block),
                        area,
                    );
                }
            },
        }
    }

    fn draw_dashboard(&self, f: &mut Frame, area: Rect) {
        if self.grid_mode {
            self.draw_grid(f, area);
            return;
        }
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(6), Constraint::Length(5)])
            .split(area);

        self.draw_graph(f, rows[0], "Response");

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.chrome_style())
            .title(Span::styled("Now", self.theme.label()));
        let inner = block.inner(rows[1]);
        f.render_widget(block, rows[1]);

        let lines = vec![
            Line::from(vec![
                Span::styled("Device   ", self.theme.label()),
                Span::styled(self.device.clone(), self.theme.value()),
            ]),
            Line::from(vec![
                Span::styled("Channels ", self.theme.label()),
                Span::styled(
                    format!(
                        "{} ({} in, {} out)",
                        self.channels.len(),
                        self.ctx.num_inputs,
                        self.ctx.num_outputs
                    ),
                    self.theme.value(),
                ),
                Span::styled(
                    if self.active_inputs > 0 {
                        format!("  ·  {} active", self.active_inputs)
                    } else {
                        String::new()
                    },
                    self.theme.label(),
                ),
            ]),
            Line::from(vec![
                Span::styled("CPU      ", self.theme.label()),
                Span::styled(
                    format!("{}% / {}%", self.cpu.0, self.cpu.1),
                    self.theme.value(),
                ),
            ]),
        ];
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_filters(&self, f: &mut Frame, area: Rect) {
        if self.grid_mode {
            self.draw_grid(f, area);
            return;
        }
        let (graph_h, table_h) = match self.split {
            Split::GraphOnly => (area.height, 0),
            Split::TableOnly => (0, area.height),
            // The graph wants width far more than height: a Bode plot spans
            // three decades, so it gets the full width and a modest slice of the
            // rows, with the band table below.
            Split::Split => {
                let g = (area.height / 2).clamp(6, 14);
                (g, area.height.saturating_sub(g))
            }
        };

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(graph_h), Constraint::Min(0)])
            .split(area);

        if graph_h > 0 {
            let title = self
                .selected()
                .map(|c| format!("Response · {}", c.name))
                .unwrap_or_else(|| "Response".into());
            self.draw_graph(f, rows[0], &title);
        }
        if table_h > 0 {
            self.draw_bands(f, rows[1]);
        }
    }

    fn draw_graph(&self, f: &mut Frame, area: Rect, title: &str) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.chrome_style())
            .title(Span::styled(title.to_string(), self.theme.label()));
        let inner = block.inner(area);
        f.render_widget(block, area);

        if inner.height < 3 {
            return;
        }

        // The bottom row carries the frequency labels.
        let plot = Rect::new(inner.x, inner.y, inner.width, inner.height - 1);
        let axis = Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1);

        let mut bode = Bode::new(&self.theme);
        bode.db_top = self.db_range / 2.0;
        bode.db_bottom = -self.db_range / 2.0;
        bode.cursor = self.cursor;
        if self.panel == Panel::Filters
            && let Some(c) = self.selected()
            && let Some(b) = c.bands.get(self.selected_band)
            && !matches!(b.filter_type, dspi_proto::FilterType::Flat)
        {
            bode.marker_hz = Some(b.freq as f64);
        }

        for (i, c) in self.channels.iter().enumerate() {
            // On the dashboard, showing seventeen curves at once is noise; the
            // selected channel plus its neighbour is what a user is comparing.
            if !self.visible.get(i).copied().unwrap_or(true) {
                continue;
            }
            let show = self.panel == Panel::Filters && i == self.selected_channel
                || self.panel == Panel::Dashboard && i < 2;
            if !show {
                continue;
            }
            bode = bode.curve(Curve {
                label: &c.name,
                color: self.theme.channel(i as u8),
                points: &c.curve,
                focused: i == self.selected_channel,
            });
        }

        // Phase shares the plot but not the axis: it is mapped onto the same
        // vertical space with +/-180 degrees spanning the full dB window, which
        // is how the Console scales it. Drawn dim and last so it reads as an
        // overlay rather than as another response.
        let phase_points;
        if self.show_phase
            && let Some(c) = self.selected()
        {
            let wrapped = dsp::phase_curve(&c.bands);
            let degrees = if self.phase_unwrapped {
                dsp::unwrap_phase(&wrapped)
            } else {
                wrapped
            };
            let scale = (self.db_range / 2.0) / 180.0;
            phase_points = degrees.iter().map(|d| d * scale).collect::<Vec<_>>();
            bode = bode.curve(Curve {
                label: "phase",
                color: self.theme.dim,
                points: &phase_points,
                focused: false,
            });
        }

        f.render_widget(bode, plot);

        // The cursor readout replaces the frequency labels while it is up: the
        // numbers say more than the axis does, and a terminal renders them
        // better than a graphical plot can.
        if let Some(hz) = self.cursor_hz() {
            let mut spans = vec![Span::styled(
                if hz >= 1000.0 {
                    format!("{:.2} kHz", hz / 1000.0)
                } else {
                    format!("{hz:.0} Hz")
                },
                Style::default().fg(self.theme.pending),
            )];
            for (slug, db) in self.cursor_readout().into_iter().take(4) {
                spans.push(Span::styled(format!("  {slug} "), self.theme.label()));
                spans.push(Span::styled(format!("{db:+.1}"), self.theme.value()));
            }
            f.render_widget(Paragraph::new(Line::from(spans)), axis);
        } else {
            f.render_widget(
                Paragraph::new(frequency_axis(axis.width, &self.theme)),
                axis,
            );
        }
    }

    /// Small multiples: one mini plot per channel.
    ///
    /// This answers "which channel looks wrong?" across seventeen channels,
    /// which an overlay of seventeen curves cannot. Every cell shares the main
    /// graph's dB window rather than autoscaling, because autoscaled small
    /// multiples lie about relative magnitude.
    fn draw_grid(&self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.chrome_style())
            .title(Span::styled(
                format!("Responses · all {} channels", self.channels.len()),
                self.theme.label(),
            ));
        let inner = block.inner(area);
        f.render_widget(block, area);

        // Cells need room for a name plus a readable curve; below that the
        // curve degrades to a sparkline rather than becoming a smudge.
        const CELL_W: u16 = 22;
        const CELL_H: u16 = 4;
        let cols = (inner.width / CELL_W).max(1);
        let rows = (inner.height / CELL_H).max(1);
        let capacity = (cols * rows) as usize;

        for (i, c) in self.channels.iter().enumerate().take(capacity) {
            let cx = inner.x + (i as u16 % cols) * CELL_W;
            let cy = inner.y + (i as u16 / cols) * CELL_H;

            let selected = i == self.selected_channel;
            let hidden = !self.visible.get(i).copied().unwrap_or(true);

            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    c.slug.clone(),
                    if selected {
                        self.theme.focused()
                    } else if hidden {
                        Style::default().fg(self.theme.chrome)
                    } else {
                        self.theme.label()
                    },
                ))),
                Rect::new(cx, cy, CELL_W.min(inner.width), 1),
            );

            if hidden {
                continue;
            }

            let plot = Rect::new(
                cx,
                cy + 1,
                CELL_W.saturating_sub(1).min(inner.width),
                (CELL_H - 1).min(inner.height.saturating_sub(cy - inner.y + 1)),
            );
            if plot.height == 0 || plot.width < 4 {
                continue;
            }

            let mut bode = Bode::new(&self.theme);
            bode.db_top = self.db_range / 2.0;
            bode.db_bottom = -self.db_range / 2.0;
            // A flat channel is drawn flat and dim, never omitted: "no EQ" must
            // look different from "missing".
            f.render_widget(
                bode.curve(Curve {
                    label: &c.name,
                    color: self.theme.channel(i as u8),
                    points: &c.curve,
                    focused: selected,
                }),
                plot,
            );
        }

        if self.channels.len() > capacity {
            // Never let a truncated view read as a complete one.
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!(
                        "{} more channels; widen the terminal",
                        self.channels.len() - capacity
                    ),
                    Style::default().fg(self.theme.pending),
                ))),
                Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
            );
        }
    }

    /// The crosspoint grid, sized from the device.
    ///
    /// On an RP2350 this is 8 inputs by 9 outputs, which does not fit a fixed
    /// layout, so the grid scrolls around a focus reticle rather than assuming
    /// it can show everything.
    fn draw_matrix(&self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.chrome_style())
            .title(Span::styled(
                format!(
                    "Matrix · {} in x {} out",
                    self.ctx.num_inputs, self.ctx.num_outputs
                ),
                self.theme.label(),
            ));
        let inner = block.inner(area);
        f.render_widget(block, area);

        if self.matrix.is_empty() || inner.height < 4 {
            f.render_widget(
                Paragraph::new("No routing read yet.").style(self.theme.label()),
                inner,
            );
            return;
        }

        const LABEL_W: u16 = 8;
        const CELL_W: u16 = 7;
        let fit = ((inner.width.saturating_sub(LABEL_W)) / CELL_W).max(1) as usize;
        let n_out = self.matrix[0].len();
        // Scroll so the focused output stays on screen.
        let first = self
            .matrix_focus
            .1
            .saturating_sub(fit.saturating_sub(1))
            .min(n_out.saturating_sub(1));
        let shown = (first..n_out).take(fit);

        let mut header = vec![Span::styled(
            " ".repeat(LABEL_W as usize),
            self.theme.label(),
        )];
        for o in shown.clone() {
            let name = self
                .channels
                .get(self.ctx.num_inputs as usize + o)
                .map(|c| c.slug.clone())
                .unwrap_or_else(|| format!("out{o}"));
            // Keep the tail segments rather than the last six characters:
            // "spdif.1.l" truncated blindly becomes "if.1.l", which reads as a
            // different name entirely. The row labels carry the full name.
            header.push(Span::styled(
                format!("{:>6} ", abbreviate(&name, 6)),
                self.theme.label(),
            ));
        }
        f.render_widget(
            Paragraph::new(Line::from(header)),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );

        let rows = (inner.height.saturating_sub(2)) as usize;
        for (r, (i, row)) in self.matrix.iter().enumerate().take(rows).enumerate() {
            let label = self
                .channels
                .get(i)
                .map(|c| c.slug.clone())
                .unwrap_or_else(|| format!("in{i}"));
            let mut spans = vec![Span::styled(
                format!("{label:<8}"),
                if i == self.matrix_focus.0 {
                    self.theme.focused()
                } else {
                    self.theme.label()
                },
            )];

            for o in shown.clone() {
                let cell = row.get(o).copied().unwrap_or_default();
                let focused = (i, o) == self.matrix_focus;
                // Off is a dot rather than a zero, so an active route stands out
                // at a glance across a grid this size.
                let text = if !cell.enabled {
                    "    ·  ".to_string()
                } else if cell.phase_invert {
                    format!("{:>5.1}ø ", cell.gain_db)
                } else {
                    format!("{:>5.1}  ", cell.gain_db)
                };
                spans.push(Span::styled(
                    text,
                    if focused {
                        self.theme.focused()
                    } else if cell.enabled {
                        Style::default().fg(self.theme.channel(i as u8))
                    } else {
                        Style::default().fg(self.theme.chrome)
                    },
                ));
            }
            f.render_widget(
                Paragraph::new(Line::from(spans)),
                Rect::new(inner.x, inner.y + 1 + r as u16, inner.width, 1),
            );
        }

        // The focused crosspoint, spelled out, plus its output's strip.
        let (i, o) = self.matrix_focus;
        let cell = self
            .matrix
            .get(i)
            .and_then(|r| r.get(o))
            .copied()
            .unwrap_or_default();
        let strip = self.outputs.get(o).copied().unwrap_or_default();
        // Kept short enough to survive the meter rail taking a quarter of the
        // width; a detail line that truncates is worse than a terser one.
        let detail = format!(
            "{}→{} {} {:+.1}dB{} · out {} {:+.1}dB {:.1}ms",
            self.channels.get(i).map(|c| c.slug.as_str()).unwrap_or("?"),
            self.channels
                .get(self.ctx.num_inputs as usize + o)
                .map(|c| c.slug.as_str())
                .unwrap_or("?"),
            if cell.enabled { "on" } else { "off" },
            cell.gain_db,
            if cell.phase_invert { " inv" } else { "" },
            if strip.enabled { "on" } else { "off" },
            strip.gain_db,
            strip.delay_ms,
        );
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(detail, self.theme.label()))),
            Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
        );
    }

    /// Draw a field list: one row per registry parameter.
    fn draw_fields(&self, f: &mut Frame, area: Rect, title: &str) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.chrome_style())
            .title(Span::styled(title.to_string(), self.theme.label()));
        let inner = block.inner(area);
        f.render_widget(block, area);

        if self.fields.is_empty() {
            f.render_widget(
                Paragraph::new(
                    "Nothing adjustable here yet.\n\n\
                     Structured views for this panel are still to come; the \
                     parameters are reachable now through the command line \
                     and the palette.",
                )
                .style(self.theme.label()),
                inner,
            );
            return;
        }

        // The last row explains whatever is focused, so the meaning of a control
        // is never something the user has to already know.
        let list_h = inner.height.saturating_sub(2);

        for (i, field) in self.fields.iter().take(list_h as usize).enumerate() {
            let Some(d) = field.desc() else { continue };
            let focused = i == self.selected_field;

            let name_style = if focused {
                self.theme.focused()
            } else if field.unavailable.is_some() {
                Style::default().fg(self.theme.chrome)
            } else {
                self.theme.label()
            };

            let (value_text, value_style) = match (&field.unavailable, &field.state) {
                (Some(_), _) => (
                    "unavailable".to_string(),
                    Style::default().fg(self.theme.chrome),
                ),
                (None, FieldState::Pending) => (
                    format!("{} ·", field.display()),
                    Style::default().fg(self.theme.pending),
                ),
                // A rejection has to be impossible to miss: the device took the
                // write and kept something else.
                (None, FieldState::Rejected(actual)) => (
                    format!(
                        "{}  ⚠ {}",
                        field.display(),
                        crate::fields::display_value(d, actual)
                    ),
                    Style::default().fg(self.theme.danger),
                ),
                _ => (field.display(), self.theme.value()),
            };

            let row = Line::from(vec![
                Span::styled(
                    format!("{}{:<22}", if focused { "▸" } else { " " }, d.label),
                    name_style,
                ),
                Span::styled(value_text, value_style),
            ]);
            f.render_widget(
                Paragraph::new(row),
                Rect::new(inner.x, inner.y + i as u16, inner.width, 1),
            );
        }

        if let Some(field) = self.focused_field()
            && let Some(d) = field.desc()
            && inner.height >= 2
        {
            let help = match &field.unavailable {
                Some(why) => format!("{} — {why}", d.plain),
                None => match d.kind {
                    dspi_proto::registry::Kind::Float { unit, min, max } => {
                        format!("{}  ({min} to {max}{})", d.plain, unit.suffix())
                    }
                    _ => d.plain.to_string(),
                },
            };
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(help, self.theme.label()))),
                Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
            );
        }
    }

    fn draw_bands(&self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.chrome_style())
            .title(Span::styled("Bands", self.theme.label()));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let Some(ch) = self.selected() else {
            return;
        };

        let rows: Vec<Row> = ch
            .bands
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let focused = i == self.selected_band;
                let style = if focused {
                    self.theme.focused()
                } else {
                    self.theme.value()
                };
                Row::new(vec![
                    format!("{}{}", if focused { "▸" } else { " " }, i + 1),
                    if b.bypass {
                        "○".into()
                    } else {
                        "●".to_string()
                    },
                    b.filter_type.label(),
                    format!("{:.0} Hz", b.freq),
                    format!("{:+.1} dB", b.gain_db),
                    format!("{:.2}", b.q),
                ])
                .style(style)
            })
            .collect();

        let table = Table::new(
            rows,
            [
                Constraint::Length(4),
                Constraint::Length(3),
                Constraint::Length(18),
                Constraint::Length(10),
                Constraint::Length(10),
                Constraint::Length(6),
            ],
        )
        .header(Row::new(vec!["#", "on", "type", "freq", "gain", "Q"]).style(self.theme.label()));

        f.render_widget(table, inner);
    }

    fn draw_meter_rail(&self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.chrome_style())
            .title(Span::styled("Levels", self.theme.label()));
        let inner = block.inner(area);
        f.render_widget(block, area);

        // The rail follows context: it shows what the current panel is about
        // rather than every channel, which would not fit and would not help.
        let shown: Vec<usize> = match self.panel {
            Panel::Filters => {
                let s = self.selected_channel;
                let mate = if s.is_multiple_of(2) {
                    s + 1
                } else {
                    s.saturating_sub(1)
                };
                vec![s, mate]
                    .into_iter()
                    .filter(|i| *i < self.channels.len())
                    .collect()
            }
            _ => (0..self.channels.len())
                .take(inner.height as usize - 1)
                .collect(),
        };

        // Size the label column to the device's own names. A fixed width
        // truncates "spdif.2.l" to "spdif.2.", which is indistinguishable from
        // its pair; the names come from the device, so the width must too.
        let label_width = shown
            .iter()
            .map(|i| self.channels[*i].slug.chars().count())
            .max()
            .unwrap_or(6)
            .clamp(6, 12) as u16;

        for (row, i) in shown.iter().enumerate() {
            if row as u16 >= inner.height.saturating_sub(1) {
                break;
            }
            let c = &self.channels[*i];
            f.render_widget(
                Meter {
                    label: &c.slug,
                    level: c.peak,
                    clipped: c.clipped,
                    color: self.theme.channel(*i as u8),
                    theme: &self.theme,
                    label_width,
                },
                Rect::new(inner.x, inner.y + row as u16, inner.width, 1),
            );
        }

        if inner.height > 1 {
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!("CPU {}/{}%", self.cpu.0, self.cpu.1),
                    self.theme.label(),
                ))),
                Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
            );
        }
    }

    fn draw_meter_bridge(&self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.chrome_style())
            .title(Span::styled(
                format!("Meters · {} channels", self.channels.len()),
                self.theme.label(),
            ));
        let inner = block.inner(area);
        f.render_widget(block, area);

        for (i, c) in self.channels.iter().enumerate() {
            if i as u16 >= inner.height {
                break;
            }
            f.render_widget(
                Meter {
                    label: &c.name,
                    level: c.peak,
                    clipped: c.clipped,
                    color: self.theme.channel(i as u8),
                    theme: &self.theme,
                    label_width: 12,
                },
                Rect::new(inner.x, inner.y + i as u16, inner.width, 1),
            );
        }
    }

    fn draw_echo(&self, f: &mut Frame, area: Rect) {
        // Dimmed, and in the exact syntax a shell accepts, so the interface
        // teaches the command line simply by being used.
        let text = if let Some((msg, at)) = &self.status
            && at.elapsed() < Duration::from_secs(3)
        {
            Span::styled(msg.clone(), Style::default().fg(self.theme.pending))
        } else if self.echo.is_empty() {
            Span::raw("")
        } else {
            Span::styled(format!(":{}", self.echo), self.theme.label())
        };
        f.render_widget(Paragraph::new(Line::from(text)), area);
    }

    fn draw_keys(&self, f: &mut Frame, area: Rect) {
        // Show the keys that matter here, rather than one list that is mostly
        // irrelevant wherever you happen to be.
        let keys = match self.panel {
            Panel::Filters => {
                "^P palette · : cmd · h/l cursor · +/- zoom · space hide · = split · q quit"
            }
            _ => "^P palette · : cmd · Tab panel · G graph · M meters · F2 level · q quit",
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(keys, self.theme.label()))),
            area,
        );
    }

    fn draw_input_overlay(&self, f: &mut Frame, area: Rect) {
        let height = (self.candidates.len().min(8) as u16 + 3).min(area.height);
        let rect = Rect::new(
            area.x + 2,
            area.y + area.height.saturating_sub(height + 2),
            area.width.saturating_sub(4),
            height,
        );

        f.render_widget(Clear, rect);

        let title = if self.mode == Mode::Palette {
            "Search"
        } else {
            "Command"
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(self.theme.accent))
            .title(Span::styled(title, self.theme.focused()));
        let inner = block.inner(rect);
        f.render_widget(block, rect);

        let prompt = if self.mode == Mode::Palette {
            "› "
        } else {
            ":"
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(prompt, Style::default().fg(self.theme.accent)),
                Span::styled(
                    self.input.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled("▏", Style::default().fg(self.theme.accent)),
            ])),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );

        for (i, c) in self
            .candidates
            .iter()
            .take(inner.height as usize - 1)
            .enumerate()
        {
            let y = inner.y + 1 + i as u16;
            let selected = i == self.candidate_index;
            let style = if selected {
                self.theme.focused()
            } else {
                self.theme.value()
            };
            let shown = if c.value.is_empty() {
                // A hint has nothing to insert; show only what it explains.
                Line::from(Span::styled(format!("  {}", c.detail), self.theme.label()))
            } else {
                Line::from(vec![
                    Span::styled(format!("  {:<24}", c.value), style),
                    Span::styled(c.detail.clone(), self.theme.label()),
                ])
            };
            f.render_widget(Paragraph::new(shown), Rect::new(inner.x, y, inner.width, 1));
        }
    }
}

/// Shorten a channel name for a narrow column, keeping the parts that identify
/// it. Dropping characters off the front turns "spdif.1.l" into "if.1.l", which
/// looks like a name of its own.
pub fn abbreviate(name: &str, width: usize) -> String {
    if name.chars().count() <= width {
        return name.to_string();
    }
    let parts: Vec<&str> = name.split('.').collect();
    // Prefer the trailing segments, which are the index and side.
    for take in (1..parts.len()).rev() {
        let candidate = parts[parts.len() - take..].join(".");
        if candidate.chars().count() <= width {
            return candidate;
        }
    }
    parts.last().map(|s| s.to_string()).unwrap_or_default()
}

/// Run the interface until the user quits.
///
/// Meters are polled rather than pushed, because the combined status read is one
/// transfer for the whole device. Everything else waits on the notification
/// endpoint, so the poll rate only has to keep the meters looking alive.
pub fn run(mut app: App, session: &mut Session) -> io::Result<()> {
    let perf = app.perf;
    let mut terminal = ratatui::init();
    let mut last_poll = Instant::now() - perf.meter_interval;

    let result = (|| -> io::Result<()> {
        loop {
            if last_poll.elapsed() >= perf.meter_interval {
                if let Ok(m) = session.meters() {
                    app.apply_meters(&m);
                }
                last_poll = Instant::now();
            }

            terminal.draw(|f| app.draw(f))?;

            if event::poll(perf.event_timeout)?
                && let Event::Key(key) = event::read()?
                && key.kind == event::KeyEventKind::Press
            {
                let before = app.panel;
                let level_before = app.level;

                // Editing needs the device, so it is routed here rather than
                // buried in the key handler, which stays free of I/O.
                match key.code {
                    KeyCode::Left | KeyCode::Right if !app.fields.is_empty() => {
                        app.edit_focused(
                            session,
                            key.code == KeyCode::Right,
                            key.modifiers.contains(KeyModifiers::SHIFT),
                        );
                    }
                    _ => app.on_key(key),
                }

                if app.panel != before || app.level != level_before {
                    app.load_fields(session);
                }
            }
            if app.should_quit {
                return Ok(());
            }
        }
    })();
    ratatui::restore();
    result
}

/// Choose glyphs the terminal can actually render.
pub fn detect_glyphs() -> Glyphs {
    glyphs_for(&crate::theme::TermEnv::detect())
}

/// Pick a glyph set without touching the environment.
///
/// The Windows classic console renders braille as boxes in most fonts, so it
/// gets blocks even though it is perfectly capable of Unicode. Windows Terminal
/// handles braille correctly and is detected separately.
pub fn glyphs_for(e: &crate::theme::TermEnv) -> Glyphs {
    if e.term.as_deref() == Some("dumb") {
        return Glyphs::Ascii;
    }
    if e.force_no_unicode {
        return Glyphs::Blocks;
    }
    if e.windows && !e.wt_session && e.conemu_ansi.as_deref() != Some("ON") {
        return Glyphs::Blocks;
    }
    Glyphs::Braille
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ColorDepth;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn demo_app(w: u16, h: u16) -> (App, Terminal<TestBackend>) {
        let mut app = App::new(Theme::dark(ColorDepth::TrueColor, Glyphs::Braille));
        app.platform = "RP2350".into();
        app.firmware = "1.1.5".into();
        app.preset = "Preset 3".into();
        app.device = "4BA1DDB9D1443D6A".into();
        app.cpu = (34, 8);

        app.channels = (0..17)
            .map(|i| {
                let mut bands = vec![dsp::Band::default(); 12];
                if i == 0 {
                    bands[0] = dsp::Band {
                        filter_type: dspi_proto::FilterType::LowShelf,
                        freq: 105.0,
                        q: 0.707,
                        gain_db: 8.8,
                        bypass: false,
                    };
                    bands[1] = dsp::Band {
                        filter_type: dspi_proto::FilterType::Peaking,
                        freq: 2856.0,
                        q: 3.58,
                        gain_db: -8.6,
                        bypass: false,
                    };
                }
                ChannelView {
                    name: format!("Ch {}", i + 1),
                    slug: format!("ch.{}", i + 1),
                    is_output: i >= 8,
                    curve: dsp::curve(&bands, 0.0),
                    bands,
                    peak: 0.5,
                    clipped: i == 16,
                }
            })
            .collect();

        app.ctx = Context {
            channel_slugs: app.channels.iter().map(|c| c.slug.clone()).collect(),
            num_inputs: 8,
            num_outputs: 9,
            max_bands: 12,
        };

        (app, Terminal::new(TestBackend::new(w, h)).unwrap())
    }

    fn render(app: &App, term: &mut Terminal<TestBackend>) -> String {
        term.draw(|f| app.draw(f)).unwrap();
        let buf = term.backend().buffer();
        let area = *buf.area();
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_dashboard_renders_at_a_normal_size() {
        let (app, mut term) = demo_app(120, 40);
        let out = render(&app, &mut term);
        assert!(out.contains("DSPi"));
        assert!(out.contains("RP2350"));
        assert!(out.contains("Preset 3"));
        assert!(out.contains("Dashboard"));
        assert!(out.contains("Response"));
        assert!(out.contains("Levels"));
        assert!(out.contains("palette"));
    }

    /// 80x24 is the floor the design promises to work at.
    #[test]
    fn everything_still_fits_at_eighty_by_twentyfour() {
        let (app, mut term) = demo_app(80, 24);
        let out = render(&app, &mut term);
        assert!(out.contains("Dashboard"));
        assert!(out.contains("Response"));
        for line in out.lines() {
            assert!(
                line.chars().count() <= 80,
                "line overflows 80 columns: {line}"
            );
        }
    }

    #[test]
    fn a_hopeless_terminal_says_so_rather_than_rendering_rubbish() {
        let (app, mut term) = demo_app(40, 8);
        let out = render(&app, &mut term);
        assert!(out.contains("too small"), "{out}");
    }

    #[test]
    fn the_filters_panel_shows_the_band_table_and_marks_the_focus() {
        let (mut app, mut term) = demo_app(120, 40);
        app.panel = Panel::Filters;
        app.selected_channel = 0;
        app.selected_band = 1;
        let out = render(&app, &mut term);
        assert!(out.contains("Bands"));
        assert!(
            out.contains("Peaking"),
            "band types should be named:\n{out}"
        );
        assert!(out.contains("2856 Hz"));
        assert!(out.contains('▸'), "the focused band should be marked");
    }

    #[test]
    fn the_curve_is_actually_drawn() {
        let (mut app, mut term) = demo_app(120, 40);
        app.panel = Panel::Filters;
        let out = render(&app, &mut term);
        assert!(
            out.chars().any(|c| (0x2800..=0x28FF).contains(&(c as u32))),
            "no braille curve in the graph:\n{out}"
        );
    }

    #[test]
    fn the_split_key_cycles_the_layout() {
        let (mut app, mut term) = demo_app(120, 40);
        app.panel = Panel::Filters;

        app.split = Split::TableOnly;
        let table_only = render(&app, &mut term);
        assert!(table_only.contains("Bands"));
        assert!(!table_only.contains("Response"));

        app.split = Split::GraphOnly;
        let graph_only = render(&app, &mut term);
        assert!(graph_only.contains("Response"));
        assert!(!graph_only.contains("Bands"));
    }

    #[test]
    fn the_meter_bridge_shows_every_channel_with_its_clip_flag() {
        let (mut app, mut term) = demo_app(120, 40);
        app.meters_expanded = true;
        let out = render(&app, &mut term);
        assert!(out.contains("17 channels"));
        assert!(out.contains("Ch 17"), "the last channel should be listed");
        assert!(
            out.contains('▌'),
            "the clipped channel should show its flag"
        );
    }

    #[test]
    fn the_command_line_completes_from_the_registry() {
        let (mut app, mut term) = demo_app(120, 40);
        app.mode = Mode::Command;
        app.input = "bass.".into();
        app.refresh_candidates();
        let out = render(&app, &mut term);
        assert!(out.contains("Command"));
        assert!(out.contains("bass.drive"), "completion missing:\n{out}");
    }

    #[test]
    fn the_palette_searches_the_same_table() {
        let (mut app, mut term) = demo_app(120, 40);
        app.mode = Mode::Palette;
        app.input = "vol".into();
        app.refresh_candidates();
        let out = render(&app, &mut term);
        assert!(out.contains("Search"));
        assert!(out.contains("vol.user"));
    }

    #[test]
    fn tab_accepts_the_highlighted_completion() {
        let (mut app, _term) = demo_app(120, 40);
        app.mode = Mode::Command;
        app.input = "bass.dr".into();
        app.refresh_candidates();
        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.input, "bass.drive ");
    }

    #[test]
    fn escape_abandons_the_line_without_echoing_it() {
        let (mut app, _term) = demo_app(120, 40);
        app.mode = Mode::Command;
        app.input = "vol.user -18".into();
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.mode, Mode::Browse);
        assert!(app.echo.is_empty());
    }

    #[test]
    fn enter_records_the_command_for_the_echo_line() {
        let (mut app, mut term) = demo_app(120, 40);
        app.mode = Mode::Command;
        app.input = "vol.user -18".into();
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.echo, "vol.user -18");
        let out = render(&app, &mut term);
        assert!(out.contains(":vol.user -18"), "echo line missing:\n{out}");
    }

    #[test]
    fn navigation_wraps_rather_than_sticking() {
        let (mut app, _term) = demo_app(120, 40);
        app.selected_channel = 16;
        app.on_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.selected_channel, 0);
        app.on_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.selected_channel, 16);
    }

    #[test]
    fn tab_moves_between_panels() {
        let (mut app, _term) = demo_app(120, 40);
        assert_eq!(app.panel, Panel::Dashboard);
        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.panel, Panel::Input);
        app.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        assert_eq!(app.panel, Panel::Dashboard);
    }

    /// The Raw panel can issue any opcode, so it stays out of sight until the
    /// user has explicitly asked for expert controls.
    #[test]
    fn the_raw_panel_is_hidden_below_expert_level() {
        let (mut app, mut term) = demo_app(120, 40);
        app.level = Level::Advanced;
        assert!(
            !render(&app, &mut term).contains("Raw"),
            "Raw must not be reachable below Expert"
        );
        app.level = Level::Expert;
        assert!(render(&app, &mut term).contains("Raw"));
    }

    #[test]
    fn q_and_ctrl_c_both_quit() {
        for key in [
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        ] {
            let (mut app, _term) = demo_app(120, 40);
            app.on_key(key);
            assert!(app.should_quit);
        }
    }

    #[test]
    fn the_unsaved_marker_appears_only_when_there_is_something_to_lose() {
        let (mut app, mut term) = demo_app(120, 40);
        assert!(!render(&app, &mut term).contains('●'));
        app.dirty = true;
        assert!(render(&app, &mut term).contains('●'));
    }
}

#[cfg(test)]
mod graph_tests {
    use super::*;
    use crate::theme::ColorDepth;

    fn app() -> App {
        let mut a = App::new(Theme::dark(ColorDepth::TrueColor, Glyphs::Braille));
        a.channels = (0..3)
            .map(|i| {
                let bands = vec![dsp::Band {
                    filter_type: dspi_proto::FilterType::Peaking,
                    freq: 1000.0,
                    q: 2.0,
                    gain_db: 6.0 * (i as f32 + 1.0),
                    bypass: false,
                }];
                ChannelView {
                    name: format!("Ch {i}"),
                    slug: format!("ch.{i}"),
                    is_output: false,
                    curve: dsp::curve(&bands, 0.0),
                    bands,
                    peak: 0.0,
                    clipped: false,
                }
            })
            .collect();
        a.visible = vec![true; 3];
        a
    }

    fn press(a: &mut App, c: char) {
        a.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }

    #[test]
    fn the_cursor_starts_mid_band_and_moves_both_ways() {
        let mut a = app();
        assert!(a.cursor.is_none());
        press(&mut a, 'l');
        let first = a.cursor.unwrap();
        press(&mut a, 'l');
        assert!(a.cursor.unwrap() > first);
        press(&mut a, 'h');
        assert_eq!(a.cursor.unwrap(), first);
    }

    #[test]
    fn the_cursor_stops_at_the_ends_rather_than_wrapping() {
        let mut a = app();
        for _ in 0..300 {
            press(&mut a, 'L');
        }
        assert_eq!(a.cursor, Some(dsp::POINTS - 1));
        for _ in 0..300 {
            press(&mut a, 'H');
        }
        assert_eq!(a.cursor, Some(0));
    }

    /// The readout is the thing a terminal does better than a graphical plot,
    /// so it has to be right: one entry per visible channel, at the cursor.
    #[test]
    fn the_readout_reports_every_visible_channel() {
        let mut a = app();
        press(&mut a, 'l');
        let readout = a.cursor_readout();
        assert_eq!(readout.len(), 3);

        a.visible[1] = false;
        assert_eq!(a.cursor_readout().len(), 2);
    }

    #[test]
    fn the_readout_matches_the_curve_at_the_cursor_frequency() {
        let mut a = app();
        // Park the cursor on the filter's centre frequency.
        let freqs = dsp::frequencies();
        let idx = freqs
            .iter()
            .enumerate()
            .min_by(|x, y| {
                (x.1 - 1000.0)
                    .abs()
                    .partial_cmp(&(y.1 - 1000.0).abs())
                    .unwrap()
            })
            .unwrap()
            .0;
        a.cursor = Some(idx);

        let readout = a.cursor_readout();
        // Channel 0 has a +6 dB peak at 1 kHz.
        assert!(
            (readout[0].1 - 6.0).abs() < 0.3,
            "expected about +6 dB, got {}",
            readout[0].1
        );
        assert!((a.cursor_hz().unwrap() - 1000.0).abs() < 60.0);
    }

    #[test]
    fn x_dismisses_the_cursor() {
        let mut a = app();
        press(&mut a, 'l');
        assert!(a.cursor.is_some());
        press(&mut a, 'x');
        assert!(a.cursor.is_none());
        assert!(a.cursor_readout().is_empty());
    }

    #[test]
    fn zoom_narrows_and_widens_within_sane_limits() {
        let mut a = app();
        let start = a.db_range;
        press(&mut a, '+');
        assert!(a.db_range < start, "plus should zoom in");
        press(&mut a, '-');
        assert!((a.db_range - start).abs() < 1e-9);

        for _ in 0..50 {
            press(&mut a, '+');
        }
        assert!(a.db_range >= 10.0, "zoom must not collapse");
        for _ in 0..100 {
            press(&mut a, '-');
        }
        assert!(a.db_range <= 100.0, "zoom must not run away");
    }

    #[test]
    fn space_toggles_the_selected_channel_off_the_graph() {
        let mut a = app();
        a.selected_channel = 1;
        press(&mut a, ' ');
        assert!(!a.visible[1]);
        press(&mut a, ' ');
        assert!(a.visible[1]);
    }

    #[test]
    fn the_graph_modes_toggle() {
        let mut a = app();
        assert!(!a.grid_mode);
        press(&mut a, 'm');
        assert!(a.grid_mode);

        assert!(!a.show_phase);
        a.on_key(KeyEvent::new(KeyCode::Char('P'), KeyModifiers::NONE));
        assert!(a.show_phase);
        press(&mut a, 'u');
        assert!(a.phase_unwrapped);
    }

    /// Zoom is shared, so a channel compared against another is never read on a
    /// different scale without the reader noticing.
    #[test]
    fn zoom_is_shared_across_channels() {
        let mut a = app();
        press(&mut a, '+');
        let zoomed = a.db_range;
        a.selected_channel = 2;
        assert_eq!(a.db_range, zoomed);
    }
}

#[cfg(test)]
mod grid_and_phase_tests {
    use super::*;
    use crate::theme::ColorDepth;

    fn app_with(n: usize) -> App {
        let mut a = App::new(Theme::dark(ColorDepth::TrueColor, Glyphs::Braille));
        a.channels = (0..n)
            .map(|i| {
                let bands = vec![dsp::Band {
                    filter_type: if i == 0 {
                        dspi_proto::FilterType::Peaking
                    } else {
                        dspi_proto::FilterType::Flat
                    },
                    freq: 1000.0,
                    q: 2.0,
                    gain_db: 9.0,
                    bypass: false,
                }];
                ChannelView {
                    name: format!("Ch {i}"),
                    slug: format!("ch.{i}"),
                    is_output: false,
                    curve: dsp::curve(&bands, 0.0),
                    bands,
                    peak: 0.0,
                    clipped: false,
                }
            })
            .collect();
        a.visible = vec![true; n];
        a
    }

    fn render(a: &App, w: u16, h: u16) -> String {
        crate::render_to_string(a, w, h)
    }

    #[test]
    fn grid_mode_names_every_channel_it_shows() {
        let mut a = app_with(6);
        a.grid_mode = true;
        let out = render(&a, 100, 24);
        assert!(out.contains("all 6 channels"));
        for i in 0..6 {
            assert!(out.contains(&format!("ch.{i}")), "ch.{i} missing:\n{out}");
        }
    }

    /// A truncated view must never read as a complete one.
    #[test]
    fn grid_mode_says_when_it_cannot_show_everything() {
        let mut a = app_with(17);
        a.grid_mode = true;
        let out = render(&a, 60, 14);
        assert!(
            out.contains("more channels"),
            "truncation went unreported:\n{out}"
        );
    }

    #[test]
    fn grid_mode_shows_everything_when_there_is_room() {
        let mut a = app_with(4);
        a.grid_mode = true;
        let out = render(&a, 120, 30);
        assert!(!out.contains("more channels"), "false truncation notice");
    }

    /// A channel with no EQ is drawn flat and dim, not omitted: "no filters"
    /// must look different from "not there".
    #[test]
    fn a_flat_channel_still_gets_a_cell() {
        let mut a = app_with(3);
        a.grid_mode = true;
        let out = render(&a, 100, 24);
        assert!(out.contains("ch.1") && out.contains("ch.2"));
        assert!(
            out.chars().any(|c| (0x2800..=0x28FF).contains(&(c as u32))),
            "flat channels should still draw a line"
        );
    }

    #[test]
    fn a_hidden_channel_keeps_its_label_but_loses_its_curve() {
        let mut a = app_with(2);
        a.grid_mode = true;
        a.visible[0] = false;
        let out = render(&a, 100, 24);
        assert!(out.contains("ch.0"), "a hidden channel is still listed");
    }

    #[test]
    fn the_phase_overlay_adds_a_second_trace() {
        let mut a = app_with(1);
        a.panel = Panel::Filters;
        a.split = Split::GraphOnly;

        let without = render(&a, 100, 20);
        a.show_phase = true;
        let with = render(&a, 100, 20);
        assert_ne!(without, with, "the phase overlay changed nothing");

        let count = |s: &str| {
            s.chars()
                .filter(|c| (0x2800..=0x28FF).contains(&(*c as u32)))
                .count()
        };
        assert!(
            count(&with) > count(&without),
            "phase should add marks, not replace them"
        );
    }

    #[test]
    fn unwrapping_phase_changes_what_is_drawn() {
        let mut a = app_with(1);
        a.panel = Panel::Filters;
        a.split = Split::GraphOnly;
        a.show_phase = true;
        // One filter does not accumulate enough phase to wrap inside the
        // plotted band, so a cascade is needed for the two views to differ at
        // all. `unwrap_phase` leaving a smooth curve untouched is correct, and
        // is covered directly by the maths tests.
        a.channels[0].bands = (0..4)
            .map(|_| dsp::Band {
                filter_type: dspi_proto::FilterType::HighPass,
                freq: 500.0,
                q: 4.0,
                gain_db: 0.0,
                bypass: false,
            })
            .collect();

        let wrapped = dsp::phase_curve(&a.channels[0].bands);
        assert!(
            wrapped.windows(2).any(|w| (w[1] - w[0]).abs() > 180.0),
            "the fixture must actually wrap, or this test proves nothing"
        );

        let before = render(&a, 100, 20);
        a.phase_unwrapped = true;
        assert_ne!(before, render(&a, 100, 20));
    }
}

#[cfg(test)]
mod performance_tests {
    use super::*;

    #[test]
    fn the_lite_profile_redraws_less_and_stops_animating() {
        let full = Performance::default();
        let lite = Performance::lite();
        assert!(lite.meter_interval > full.meter_interval);
        assert!(lite.event_timeout > full.event_timeout);
        assert!(full.animate && !lite.animate);
    }

    /// Both profiles must stay responsive enough to feel alive: a meter that
    /// updates once a second reads as broken rather than as economical.
    #[test]
    fn both_profiles_stay_within_useful_bounds() {
        for p in [Performance::default(), Performance::lite()] {
            assert!(p.meter_interval <= Duration::from_millis(200));
            assert!(p.event_timeout <= Duration::from_millis(200));
            assert!(p.meter_interval >= Duration::from_millis(20));
        }
    }

    #[test]
    fn detection_returns_one_of_the_two_profiles() {
        let p = Performance::detect();
        assert!(p == Performance::default() || p == Performance::lite());
    }
}

#[cfg(test)]
mod matrix_panel_tests {
    use super::*;
    use crate::theme::ColorDepth;
    use dspi_session::{Crosspoint, OutputStrip};

    fn app() -> App {
        let mut a = App::new(Theme::dark(ColorDepth::TrueColor, Glyphs::Braille));
        a.ctx.num_inputs = 8;
        a.ctx.num_outputs = 9;
        a.channels = (0..17)
            .map(|i| ChannelView {
                name: format!("Ch {i}"),
                slug: if i < 8 {
                    format!("usb.{}", i + 1)
                } else {
                    format!(
                        "spdif.{}.{}",
                        (i - 8) / 2 + 1,
                        if i % 2 == 0 { "l" } else { "r" }
                    )
                },
                is_output: i >= 8,
                ..Default::default()
            })
            .collect();
        a.matrix = (0..8)
            .map(|i| {
                (0..9)
                    .map(|o| Crosspoint {
                        enabled: i == o,
                        phase_invert: i == 1 && o == 1,
                        gain_db: if i == o { -3.0 } else { 0.0 },
                    })
                    .collect()
            })
            .collect();
        a.outputs = vec![
            OutputStrip {
                enabled: true,
                mute: false,
                gain_db: -6.0,
                delay_ms: 4.2,
            };
            9
        ];
        a
    }

    /// Blind truncation turns "spdif.1.l" into "if.1.l", which reads as a name
    /// of its own rather than an abbreviation.
    #[test]
    fn abbreviation_keeps_the_identifying_parts() {
        assert_eq!(abbreviate("spdif.1.l", 6), "1.l");
        assert_eq!(abbreviate("usb.1", 6), "usb.1");
        assert_eq!(abbreviate("pdm", 6), "pdm");
        assert!(!abbreviate("spdif.1.l", 6).starts_with("if"));
    }

    #[test]
    fn the_grid_is_sized_and_labelled_from_the_device() {
        let mut a = app();
        a.panel = Panel::Matrix;
        let out = crate::render_to_string(&a, 90, 18);
        assert!(out.contains("8 in x 9 out"));
        assert!(out.contains("usb.1"));
        assert!(out.contains("usb.8"));
    }

    /// An inactive crosspoint is a dot, so a live route stands out across a grid
    /// this size.
    #[test]
    fn routes_are_distinguishable_from_silence() {
        let mut a = app();
        a.panel = Panel::Matrix;
        let out = crate::render_to_string(&a, 90, 18);
        assert!(out.contains('·'), "inactive crosspoints should be dots");
        assert!(out.contains("-3.0"), "active crosspoints should show gain");
    }

    #[test]
    fn the_focused_crosspoint_is_spelled_out_in_full() {
        let mut a = app();
        a.panel = Panel::Matrix;
        a.matrix_focus = (1, 1);
        let out = crate::render_to_string(&a, 90, 18);
        assert!(
            out.contains("usb.2→spdif.1.r"),
            "detail line missing:\n{out}"
        );
        assert!(out.contains("inv"), "polarity should be stated");
        assert!(out.contains("4.2ms"), "the output strip should be shown");
    }

    #[test]
    fn navigation_moves_around_the_grid_and_wraps() {
        let mut a = app();
        a.panel = Panel::Matrix;
        a.on_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(a.matrix_focus, (0, 1));
        a.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(a.matrix_focus, (1, 1));
        a.on_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(a.matrix_focus, (1, 0));
        // Wrapping rather than sticking, matching the rest of the interface.
        a.on_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(a.matrix_focus, (1, 8));
    }

    /// Nine outputs do not fit a narrow terminal, so the grid scrolls to keep
    /// the focus visible rather than silently clipping it.
    #[test]
    fn the_grid_scrolls_to_follow_the_focus() {
        let mut a = app();
        a.panel = Panel::Matrix;
        a.matrix_focus = (0, 8);
        let out = crate::render_to_string(&a, 70, 18);
        // The last output's short name must be on screen.
        assert!(
            out.contains("5.l"),
            "focused column scrolled out of view:\n{out}"
        );
    }

    #[test]
    fn an_unread_matrix_says_so_rather_than_showing_an_empty_grid() {
        let mut a = app();
        a.panel = Panel::Matrix;
        a.matrix.clear();
        let out = crate::render_to_string(&a, 90, 18);
        assert!(out.contains("No routing read yet"));
    }
}
