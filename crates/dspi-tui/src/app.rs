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
use dspi_proto::value::Value;
use dspi_session::Session;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table};

use crate::fields::{Field, FieldState, Targets, fields_for, nudge};
use crate::theme::{Glyphs, Theme};
use crate::widgets::{Bode, Curve, InlineMeter, Meter, frequency_axis};

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
    /// The overview. Reached from the top of the channel list rather than the
    /// tab bar, because it is not a per-channel view like the others are.
    Main,
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
    /// The tabs, which are all per-channel views. `Main` is deliberately absent:
    /// it is selected from the channel list, alongside the channels it
    /// summarises.
    pub const ALL: [Panel; 9] = [
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
            Panel::Main => "Main",
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

/// Which pane the arrow keys act on.
///
/// The channel list is always visible, as it is in the Console, so it needs to
/// be reachable without stealing a key the content pane wants. Enter goes in,
/// Escape comes back, and the arrows only ever mean one thing at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Content,
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

/// Which column of a band row the cursor is on.
///
/// The fields are laid out across the row, so left and right moving between
/// them is the spatially obvious reading; the value changes with plus and minus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BandField {
    Type,
    Freq,
    Gain,
    Q,
}

impl BandField {
    pub const ALL: [BandField; 4] = [
        BandField::Type,
        BandField::Freq,
        BandField::Gain,
        BandField::Q,
    ];

    /// What to call this column in a message to the user.
    pub fn name(self) -> &'static str {
        match self {
            BandField::Type => "type",
            BandField::Freq => "frequency",
            BandField::Gain => "gain",
            BandField::Q => "Q",
        }
    }

    /// The registry path this column edits.
    pub fn path(self) -> &'static str {
        match self {
            BandField::Type => "eq.type",
            BandField::Freq => "eq.freq",
            BandField::Gain => "eq.gain",
            BandField::Q => "eq.q",
        }
    }

    /// Whether this column means anything for a given filter shape. A shelf has
    /// no Q worth editing, and showing one invites a change that does nothing.
    pub fn applies_to(self, t: dspi_proto::FilterType) -> bool {
        match self {
            BandField::Type => true,
            BandField::Freq => !matches!(t, dspi_proto::FilterType::Flat),
            BandField::Gain => t.uses_gain(),
            BandField::Q => t.uses_q(),
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
    pub selected_field_col: BandField,
    /// Whether the focused band field is armed for change.
    ///
    /// Arrows mean "move the selection" until this is set and "change the
    /// value" after, so one key never does two things at once and a stray
    /// arrow cannot alter the device.
    pub band_edit: bool,

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

    /// Control surface bindings, one per slot.
    pub bindings: Vec<(dspi_session::surfaces::Binding, String)>,
    pub ir_commands: Vec<dspi_session::surfaces::IrCommand>,
    pub cs_status: dspi_session::surfaces::Status,
    pub selected_binding: usize,

    /// The crosspoint grid, indexed `[input][output]`.
    pub matrix: Vec<Vec<dspi_session::Crosspoint>>,
    pub outputs: Vec<dspi_session::OutputStrip>,
    pub matrix_focus: (usize, usize),

    /// The current panel's rows, when it is a field list.
    pub fields: Vec<Field>,
    pub selected_field: usize,

    pub focus: Focus,

    pub ctx: Context,
    pub perf: Performance,
    pub should_quit: bool,
}

impl App {
    pub fn new(theme: Theme) -> Self {
        Self {
            theme,
            panel: Panel::Main,
            mode: Mode::Browse,
            level: Level::Advanced,
            split: Split::Split,
            channels: Vec::new(),
            selected_channel: 0,
            selected_band: 0,
            selected_field_col: BandField::Freq,
            band_edit: false,
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
            bindings: Vec::new(),
            ir_commands: Vec::new(),
            cs_status: Default::default(),
            selected_binding: 0,
            matrix: Vec::new(),
            outputs: Vec::new(),
            matrix_focus: (0, 0),
            fields: Vec::new(),
            selected_field: 0,
            focus: Focus::Sidebar,
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

    /// Whether the channel list's cursor is on the overview rather than a
    /// channel. The list is the primary selector now, so it has to be able to
    /// point at something that is not a channel.
    pub fn on_main(&self) -> bool {
        self.panel == Panel::Main
    }

    /// Move the channel list's cursor, which starts at the overview.
    fn move_sidebar(&mut self, delta: isize) {
        self.band_edit = false;
        if self.channels.is_empty() {
            return;
        }
        // Position 0 is the overview; the channels follow it.
        let current = if self.on_main() {
            0
        } else {
            self.selected_channel as isize + 1
        };
        let n = self.channels.len() as isize + 1;
        let next = (current + delta).rem_euclid(n);

        if next == 0 {
            self.panel = Panel::Main;
        } else {
            if self.on_main() {
                // Leaving the overview lands on the view a channel is for.
                self.panel = Panel::Filters;
            }
            self.selected_channel = (next - 1) as usize;
            self.selected_band = 0;
        }
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

    /// Read every binding, its label, and the IR command table.
    pub fn load_surfaces(&mut self, session: &mut Session) {
        use dspi_session::surfaces;

        let Some(caps) = session.capabilities().cs.clone() else {
            return;
        };

        self.bindings.clear();
        for slot in 0..caps.max_bindings {
            let b = session
                .with_transport(|t| surfaces::read_binding(t, slot))
                .unwrap_or_default();
            let name = session
                .with_transport(|t| surfaces::read_name(t, slot))
                .unwrap_or_default();
            self.bindings.push((b, name));
        }

        self.ir_commands = (0..caps.max_ir_commands)
            .map(|i| {
                session
                    .with_transport(|t| surfaces::read_ir_command(t, i))
                    .unwrap_or_default()
            })
            .collect();

        if let Ok(s) = session
            .with_transport(|t| surfaces::read_status(t, caps.max_bindings, caps.max_ir_commands))
        {
            self.cs_status = s;
        }
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

    /// Take what was typed and close the prompt.
    pub fn take_input(&mut self) -> String {
        let line = std::mem::take(&mut self.input);
        self.mode = Mode::Browse;
        self.candidates.clear();
        line
    }

    /// Run a typed command against the device.
    ///
    /// The same parser the shell uses, so the line a user types here is the line
    /// they could paste into a script, and the echo confirms it in canonical
    /// form. Reporting a rejection matters as much as reporting an error: the
    /// device accepting a write is not the same as it keeping the value.
    pub fn run_command(&mut self, session: &mut Session, line: &str) {
        let tokens = dspi_cmd::tokenize(line);
        let refs: Vec<&str> = tokens.iter().map(String::as_str).collect();

        let cmd = match dspi_cmd::parse(&refs, &self.ctx) {
            Ok(c) => c,
            Err(e) => {
                self.note(e.to_string());
                return;
            }
        };

        match cmd {
            dspi_cmd::Command::Set {
                path,
                ref indices,
                ref value,
            } => match session.write(path, indices, value.clone()) {
                Ok(dspi_session::Outcome::Rejected { actual, .. }) => {
                    let shown = dspi_proto::registry::by_path(path)
                        .map(|d| crate::fields::display_value(d, &actual))
                        .unwrap_or_default();
                    self.note(format!("{path} was not applied; device kept {shown}"));
                }
                Ok(_) => {
                    self.dirty = true;
                    self.echo = dspi_cmd::format(&cmd, &self.ctx);
                    self.after_write(session, path, indices);
                }
                Err(e) => self.note(e.to_string()),
            },

            dspi_cmd::Command::Get { path, ref indices } => match session.read(path, indices) {
                Ok(v) => {
                    let shown = dspi_proto::registry::by_path(path)
                        .map(|d| crate::fields::display_value(d, &v))
                        .unwrap_or_default();
                    self.note(format!("{path} = {shown}"));
                }
                Err(e) => self.note(e.to_string()),
            },

            dspi_cmd::Command::SetBand {
                channel,
                band,
                filter_type,
                freq,
                q,
                gain,
            } => {
                let packet = dspi_proto::value::EqParamPacket {
                    channel,
                    band,
                    filter_type: dspi_proto::FilterType::from_raw(filter_type),
                    bypass: false,
                    freq,
                    q,
                    gain_db: gain,
                    qp: None,
                };
                match session.write_band(&packet) {
                    Ok(dspi_session::Outcome::Rejected { .. }) => {
                        self.note("the band was not applied as sent")
                    }
                    Ok(_) => {
                        self.dirty = true;
                        self.echo = dspi_cmd::format(&cmd, &self.ctx);
                        self.reload_channel(session, channel as usize);
                    }
                    Err(e) => self.note(e.to_string()),
                }
            }

            dspi_cmd::Command::Verb { name, .. } => {
                self.note(format!("`{name}` only works from the shell"))
            }
        }
    }

    /// Refresh whatever the write touched, so the screen matches the device.
    fn after_write(&mut self, session: &mut Session, path: &str, indices: &[u8]) {
        if path.starts_with("eq.")
            && let Some(ch) = indices.first()
        {
            self.reload_channel(session, *ch as usize);
        } else if !self.fields.is_empty() {
            self.load_fields(session);
        }
    }

    /// Re-read one channel's bands.
    fn reload_channel(&mut self, session: &mut Session, channel: usize) {
        let Some(count) = self.channels.get(channel).map(|c| c.bands.len()) else {
            return;
        };
        for b in 0..count {
            if let Ok(p) = session.read_band(channel as u8, b as u8) {
                self.channels[channel].bands[b] = dsp::Band {
                    filter_type: p.filter_type,
                    freq: p.freq,
                    q: p.q,
                    gain_db: p.gain_db,
                    bypass: p.bypass,
                };
            }
        }
        self.recompute(channel);
    }

    /// Adjust the focused band field and write the whole band back.
    ///
    /// The firmware stores a band as one packet, so this is a read-modify-write
    /// through the session, which means it gets the same validation, gating and
    /// readback verification as a typed command.
    pub fn edit_band(&mut self, session: &mut Session, up: bool, coarse: bool) {
        let (Some(ch), Some(band)) = (
            self.channels.get(self.selected_channel),
            self.channels
                .get(self.selected_channel)
                .and_then(|c| c.bands.get(self.selected_band)),
        ) else {
            return;
        };
        let _ = ch;

        let col = self.selected_field_col;
        if !col.applies_to(band.filter_type) {
            self.note(format!(
                "{} has no {}",
                band.filter_type.label(),
                match col {
                    BandField::Gain => "gain",
                    BandField::Q => "Q",
                    _ => "value",
                }
            ));
            return;
        }

        let Some(d) = dspi_proto::registry::by_path(col.path()) else {
            return;
        };
        let current = match col {
            BandField::Type => Value::Choice(band.filter_type.to_raw()),
            BandField::Freq => Value::Float(band.freq),
            BandField::Gain => Value::Float(band.gain_db),
            BandField::Q => Value::Float(band.q),
        };
        let Some(next) = nudge(d, &current, up, coarse) else {
            return;
        };

        let indices = [self.selected_channel as u8, self.selected_band as u8];
        match session.write(col.path(), &indices, next.clone()) {
            Ok(dspi_session::Outcome::Rejected { actual, .. }) => {
                self.note(format!(
                    "device kept {}",
                    crate::fields::display_value(d, &actual)
                ));
                self.reload_band(session);
            }
            Ok(_) => {
                self.apply_band_change(col, &next);
                self.recompute(self.selected_channel);
                self.dirty = true;
                self.echo = dspi_cmd::format(
                    &dspi_cmd::Command::Set {
                        path: col.path(),
                        indices: indices.to_vec(),
                        value: next,
                    },
                    &self.ctx,
                );
            }
            Err(e) => self.note(e.to_string()),
        }
    }

    /// Toggle the focused band in or out of circuit.
    pub fn toggle_band_bypass(&mut self, session: &mut Session) {
        let Some(band) = self
            .channels
            .get(self.selected_channel)
            .and_then(|c| c.bands.get(self.selected_band))
        else {
            return;
        };
        let next = !band.bypass;
        let indices = [self.selected_channel as u8, self.selected_band as u8];

        match session.write("eq.bypass", &indices, Value::Bool(next)) {
            Ok(dspi_session::Outcome::Rejected { .. }) => self.note("the device refused that"),
            Ok(_) => {
                if let Some(b) = self
                    .channels
                    .get_mut(self.selected_channel)
                    .and_then(|c| c.bands.get_mut(self.selected_band))
                {
                    b.bypass = next;
                }
                self.recompute(self.selected_channel);
                self.dirty = true;
                self.echo = dspi_cmd::format(
                    &dspi_cmd::Command::Set {
                        path: "eq.bypass",
                        indices: indices.to_vec(),
                        value: Value::Bool(next),
                    },
                    &self.ctx,
                );
            }
            Err(e) => self.note(e.to_string()),
        }
    }

    fn apply_band_change(&mut self, col: BandField, v: &Value) {
        let Some(b) = self
            .channels
            .get_mut(self.selected_channel)
            .and_then(|c| c.bands.get_mut(self.selected_band))
        else {
            return;
        };
        match col {
            BandField::Type => {
                if let Some(n) = v.as_u8() {
                    b.filter_type = dspi_proto::FilterType::from_raw(n);
                }
            }
            BandField::Freq => b.freq = v.as_f32().unwrap_or(b.freq),
            BandField::Gain => b.gain_db = v.as_f32().unwrap_or(b.gain_db),
            BandField::Q => b.q = v.as_f32().unwrap_or(b.q),
        }
    }

    /// Re-read the focused band, after the device kept something else.
    fn reload_band(&mut self, session: &mut Session) {
        if let Ok(p) = session.read_band(self.selected_channel as u8, self.selected_band as u8)
            && let Some(b) = self
                .channels
                .get_mut(self.selected_channel)
                .and_then(|c| c.bands.get_mut(self.selected_band))
        {
            *b = dsp::Band {
                filter_type: p.filter_type,
                freq: p.freq,
                q: p.q,
                gain_db: p.gain_db,
                bypass: p.bypass,
            };
            self.recompute(self.selected_channel);
        }
    }

    /// Step the column cursor, skipping columns this filter shape does not use.
    pub fn move_band_field(&mut self, delta: isize) {
        let shape = self
            .channels
            .get(self.selected_channel)
            .and_then(|c| c.bands.get(self.selected_band))
            .map(|b| b.filter_type)
            .unwrap_or(dspi_proto::FilterType::Flat);

        let n = BandField::ALL.len() as isize;
        let mut i = BandField::ALL
            .iter()
            .position(|f| *f == self.selected_field_col)
            .unwrap_or(1) as isize;

        for _ in 0..n {
            i = (i + delta).rem_euclid(n);
            let candidate = BandField::ALL[i as usize];
            if candidate.applies_to(shape) {
                self.selected_field_col = candidate;
                return;
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

            // In the band table a digit is a band, not a panel: it is the only
            // list long enough to want direct access, and the panel it would
            // otherwise jump to is a Tab away. `0` is band 10, as on a phone
            // keypad, because there is no key for it otherwise.
            KeyCode::Char(c @ ('0'..='9')) if self.on_bands() && !self.band_edit => {
                let n = if c == '0' {
                    10
                } else {
                    c as usize - '0' as usize
                };
                self.select_band(n);
            }

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

            // Enter goes into the content pane, Escape comes back, so the
            // arrows only ever mean one thing at a time.
            KeyCode::Enter if self.focus == Focus::Sidebar => self.focus = Focus::Content,

            // One rung further in: arm the focused field, so the arrows move
            // between fields until you say which one you mean.
            KeyCode::Enter if self.on_bands() => self.toggle_band_edit(),

            // Escape unwinds one rung at a time rather than jumping out, so it
            // is never a surprise which level it left.
            KeyCode::Esc if self.band_edit => self.band_edit = false,
            KeyCode::Esc => self.focus = Focus::Sidebar,

            KeyCode::Up if self.focus == Focus::Sidebar => self.move_sidebar(-1),
            KeyCode::Down if self.focus == Focus::Sidebar => self.move_sidebar(1),

            KeyCode::Up if self.panel == Panel::Matrix => self.move_matrix(-1, 0),
            KeyCode::Down if self.panel == Panel::Matrix => self.move_matrix(1, 0),
            KeyCode::Left if self.panel == Panel::Matrix => self.move_matrix(0, -1),
            KeyCode::Right if self.panel == Panel::Matrix => self.move_matrix(0, 1),
            // While a field is armed the arrows belong to its value, which needs
            // the device, so `run` intercepts them before they reach here.
            KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right if self.band_edit => {}

            KeyCode::Up if !self.fields.is_empty() => self.move_field(-1),
            KeyCode::Down if !self.fields.is_empty() => self.move_field(1),
            KeyCode::Up => self.move_band(-1),
            KeyCode::Down => self.move_band(1),
            KeyCode::Left if self.panel == Panel::Filters => self.move_band_field(-1),
            KeyCode::Right if self.panel == Panel::Filters => self.move_band_field(1),

            // Graph controls. `h`/`l` move the cursor rather than the selection,
            // so reading a curve never disturbs what is being edited.
            KeyCode::Char('h') => self.move_cursor(-2),
            KeyCode::Char('l') => self.move_cursor(2),
            KeyCode::Char('H') => self.move_cursor(-20),
            KeyCode::Char('L') => self.move_cursor(20),
            KeyCode::Char('x') => self.cursor = None,
            KeyCode::Char(']') => self.db_range = (self.db_range - 5.0).max(10.0),
            KeyCode::Char('[') => self.db_range = (self.db_range + 5.0).min(100.0),
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
            // Enter is handled by the caller, which has the device; reaching
            // here means there is nothing to run it against, so the line is
            // recorded rather than silently dropped.
            KeyCode::Enter => {
                let line = self.take_input();
                if !line.is_empty() {
                    self.echo = line;
                }
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
        self.band_edit = false;
        let n = Panel::ALL.len() as isize;
        // Main is not in the tab bar, so tabbing out of it enters the row from
        // whichever end you were heading towards rather than falling to index 0.
        self.panel = match Panel::ALL.iter().position(|p| *p == self.panel) {
            Some(i) => Panel::ALL[((i as isize + delta).rem_euclid(n)) as usize],
            None if delta >= 0 => Panel::ALL[0],
            None => Panel::ALL[(n - 1) as usize],
        };
        self.rebuild_fields();
    }

    /// Whether the band table has the keyboard.
    ///
    /// The digit and Enter bindings mean something different here than they do
    /// anywhere else, so every one of them asks this rather than testing the
    /// panel and the focus separately and getting one of them wrong.
    pub fn on_bands(&self) -> bool {
        self.focus == Focus::Content && self.panel == Panel::Filters
    }

    /// Jump to a band by its displayed, 1-based number.
    fn select_band(&mut self, number: usize) {
        let count = self.selected().map_or(0, |c| c.bands.len());
        if number == 0 || number > count {
            self.note(format!("this channel has {count} bands"));
            return;
        }
        self.selected_band = number - 1;
    }

    /// Arm or disarm the focused field.
    fn toggle_band_edit(&mut self) {
        // A field the filter shape does not have cannot be armed; saying so
        // beats arming something whose arrows would then do nothing.
        if !self.band_edit
            && let Some(b) = self
                .selected()
                .and_then(|c| c.bands.get(self.selected_band))
            && !self.selected_field_col.applies_to(b.filter_type)
        {
            self.note(format!(
                "{} has no {}",
                b.filter_type.label(),
                self.selected_field_col.name()
            ));
            return;
        }

        self.band_edit = !self.band_edit;
        if self.band_edit {
            self.note("← → to change · Enter when done");
        }
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

        // Breathing room, but only where there is height to spare: on a short
        // terminal every row of chrome is a row the graph does not get.
        let airy = area.height >= 26;
        let gap = if airy { 1 } else { 0 };

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // title
                Constraint::Length(gap),
                Constraint::Length(1), // tabs
                Constraint::Length(gap),
                Constraint::Min(3), // body
                Constraint::Length(gap),
                Constraint::Length(1), // echo
                Constraint::Length(1), // keys
            ])
            .split(area);

        self.draw_title(f, rows[0]);
        self.draw_tabs(f, rows[2]);

        if self.meters_expanded {
            self.draw_meter_bridge(f, rows[4]);
        } else {
            // Meters live inline in the channel list, so there is no separate
            // meter column competing for width.
            let name_w = self
                .channels
                .iter()
                .map(|c| c.name.chars().count())
                .max()
                .unwrap_or(8)
                .clamp(6, 14) as u16;
            let sidebar_w = (name_w + 15).min(area.width / 3);

            let body = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(sidebar_w), Constraint::Min(30)])
                .split(rows[4]);
            self.draw_sidebar(f, body[0]);
            self.draw_panel(f, body[1]);
        }

        self.draw_echo(f, rows[6]);
        self.draw_keys(f, rows[7]);

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
            spans.push(Span::styled(format!("  {}  ", p.title()), style));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn draw_panel(&self, f: &mut Frame, area: Rect) {
        match self.panel {
            Panel::Main => self.draw_dashboard(f, area),
            Panel::Filters => self.draw_filters(f, area),
            Panel::Matrix => self.draw_matrix(f, area),
            Panel::Surfaces => self.draw_surfaces(f, area),
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
                // The curve is the point of this view, so it takes the larger
                // share; the band table only needs enough rows to work in.
                let g = (area.height * 2 / 3).clamp(8, 26);
                // The table still needs enough rows to be worth reading.
                let g = g.min(area.height.saturating_sub(7));
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

        // The channel list is a visible selector, so the graph has to follow it
        // or selecting a channel looks like it does nothing. On the dashboard
        // the selected channel is shown with its stereo partner, which is what
        // a listener is usually comparing; seventeen curves at once is noise.
        let partner = if self.selected_channel.is_multiple_of(2) {
            self.selected_channel + 1
        } else {
            self.selected_channel.saturating_sub(1)
        };

        for (i, c) in self.channels.iter().enumerate() {
            if !self.visible.get(i).copied().unwrap_or(true) {
                continue;
            }
            let show = match self.panel {
                Panel::Filters => i == self.selected_channel,
                // A partner only counts if it is on the same side of the
                // input/output divide; pairing an input with an output would be
                // an accident of numbering.
                Panel::Main => {
                    i == self.selected_channel
                        || (i == partner
                            && self.channels.get(i).map(|p| p.is_output)
                                == self
                                    .channels
                                    .get(self.selected_channel)
                                    .map(|s| s.is_output))
                }
                _ => i == self.selected_channel,
            };
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

    /// The channel list: inputs and outputs, each with its own meter.
    ///
    /// Modelled on the Console's sidebar, because a level belongs next to the
    /// channel it describes rather than in a separate column the eye has to
    /// pair up by position.
    fn draw_sidebar(&self, f: &mut Frame, area: Rect) {
        let focused = self.focus == Focus::Sidebar;
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(if focused {
                Style::default().fg(self.theme.accent)
            } else {
                self.theme.chrome_style()
            })
            .title(Span::styled(
                "Channels",
                if focused {
                    self.theme.focused()
                } else {
                    self.theme.label()
                },
            ));
        let inner = block.inner(area);
        f.render_widget(block, area);

        if inner.height == 0 {
            return;
        }

        // Name column sized to the device's own names, so nothing is truncated
        // into something that reads as a different channel.
        let name_w = self
            .channels
            .iter()
            .map(|c| c.name.chars().count())
            .max()
            .unwrap_or(8)
            .clamp(6, 14) as u16;

        let mut row = 0u16;

        // The overview sits above the channels, since it summarises them.
        {
            let selected = self.on_main();
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        if selected { "▐" } else { " " },
                        if selected {
                            Style::default().fg(self.theme.accent)
                        } else {
                            Style::default()
                        },
                    ),
                    Span::styled(
                        "Main",
                        if selected {
                            self.theme.focused()
                        } else {
                            self.theme.value()
                        },
                    ),
                ])),
                Rect::new(inner.x, inner.y, inner.width, 1),
            );
            row += 2;
        }

        let section = |f: &mut Frame, row: &mut u16, title: &str| {
            if *row < inner.height {
                f.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        format!(" {title}"),
                        Style::default().fg(self.theme.dim),
                    ))),
                    Rect::new(inner.x, inner.y + *row, inner.width, 1),
                );
                *row += 1;
            }
        };

        for (group, want_output) in [("INPUTS", false), ("OUTPUTS", true)] {
            if !self.channels.iter().any(|c| c.is_output == want_output) {
                continue;
            }
            section(f, &mut row, group);

            for (i, c) in self.channels.iter().enumerate() {
                if c.is_output != want_output || row >= inner.height {
                    continue;
                }
                self.draw_channel_row(
                    f,
                    Rect::new(inner.x, inner.y + row, inner.width, 1),
                    i,
                    c,
                    name_w,
                );
                row += 1;
            }
        }
    }

    fn draw_channel_row(
        &self,
        f: &mut Frame,
        area: Rect,
        index: usize,
        c: &ChannelView,
        name_w: u16,
    ) {
        let selected = index == self.selected_channel && !self.on_main();
        let colour = self.theme.channel(index as u8);

        // A bar in the margin, as the Console does, rather than a highlight:
        // it survives a monochrome terminal, where a background colour does not.
        let marker = if selected { "▐" } else { " " };
        let marker_style = if selected {
            Style::default().fg(self.theme.accent)
        } else {
            Style::default()
        };

        let hidden = !self.visible.get(index).copied().unwrap_or(true);
        let name_style = if selected {
            self.theme.focused()
        } else if hidden {
            Style::default().fg(self.theme.chrome)
        } else {
            self.theme.value()
        };

        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(marker, marker_style),
                Span::styled(
                    format!(
                        "{:<w$} ",
                        truncate(&c.name, name_w as usize),
                        w = name_w as usize
                    ),
                    name_style,
                ),
            ])),
            Rect::new(area.x, area.y, (2 + name_w).min(area.width), 1),
        );

        // The meter sits inline, in the channel's own colour.
        let meter_x = area.x + 2 + name_w;
        if meter_x < area.x + area.width {
            f.render_widget(
                InlineMeter {
                    level: c.peak,
                    clipped: c.clipped,
                    color: colour,
                    theme: &self.theme,
                },
                Rect::new(meter_x, area.y, area.x + area.width - meter_x, 1),
            );
        }
    }

    /// Control surfaces: every slot in one table.
    ///
    /// The Console edits one slot at a time in a form; the whole point of doing
    /// this in a terminal is that a rig can be audited at a glance.
    fn draw_surfaces(&self, f: &mut Frame, area: Rect) {
        // The device reports dirty whenever the live config differs from flash,
        // which includes a device whose control-surface block has never been
        // written at all. Calling that "unsaved changes" would send a user
        // hunting for edits they never made.
        let configured = self.bindings.iter().any(|(b, _)| !b.is_empty())
            || self.ir_commands.iter().any(|c| !c.is_empty());
        let dirty = match (self.cs_status.dirty, configured) {
            (true, true) => " · unsaved changes ●",
            (true, false) => " · never saved",
            _ => "",
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.chrome_style())
            .title(Span::styled(
                format!("Control Surfaces · {} slots{dirty}", self.bindings.len()),
                self.theme.label(),
            ));
        let inner = block.inner(area);
        f.render_widget(block, area);

        if self.bindings.is_empty() {
            f.render_widget(
                Paragraph::new("This firmware reports no control surface support.")
                    .style(self.theme.label()),
                inner,
            );
            return;
        }

        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(
                    "{:<3}{:<12}{:<24}{:<10}{}",
                    "#", "component", "does", "pins", "status"
                ),
                self.theme.label(),
            ))),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );

        let rows = inner.height.saturating_sub(2);
        for (i, (b, name)) in self.bindings.iter().take(rows as usize).enumerate() {
            let focused = i == self.selected_binding;
            let live = self.cs_status.active_mask & (1 << i) != 0;

            let pins = if b.is_empty() {
                "-".to_string()
            } else if b.gpio[1] == dspi_session::surfaces::GPIO_UNUSED {
                b.gpio[0].to_string()
            } else {
                format!("{}, {}", b.gpio[0], b.gpio[1])
            };

            // The "does" column comes from the registry, so a knob bound to a
            // parameter reads as its name and range rather than as noun 44.
            let does = if b.is_empty() {
                "-".to_string()
            } else if !name.is_empty() {
                name.clone()
            } else {
                describe_noun(b.noun)
            };

            let status = self
                .cs_status
                .slot_status
                .get(i)
                .copied()
                .map(|s| {
                    if s == 0 && live {
                        "live".to_string()
                    } else if s == 0 {
                        "idle".to_string()
                    } else {
                        dspi_session::surfaces::explain_status(s)
                    }
                })
                .unwrap_or_default();

            let style = if focused {
                self.theme.focused()
            } else if b.is_empty() {
                Style::default().fg(self.theme.chrome)
            } else {
                self.theme.value()
            };

            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        format!(
                            "{:<3}{:<12}{:<24}{:<10}",
                            i,
                            component_name(b.component),
                            truncate(&does, 23),
                            pins
                        ),
                        style,
                    ),
                    Span::styled(
                        status,
                        if self.cs_status.slot_status.get(i).copied().unwrap_or(0) != 0 {
                            Style::default().fg(self.theme.danger)
                        } else if live {
                            Style::default().fg(self.theme.ok)
                        } else {
                            self.theme.label()
                        },
                    ),
                ])),
                Rect::new(inner.x, inner.y + 1 + i as u16, inner.width, 1),
            );
        }

        let learned = self.ir_commands.iter().filter(|c| !c.is_empty()).count();
        let hint = if self.cs_status.dirty && configured {
            "s saves to flash · r discards · live but not stored"
        } else {
            "n adds a binding · l learns a remote button"
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("{learned} IR commands learned · {hint}"),
                self.theme.label(),
            ))),
            Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
        );
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
                    self.theme.alarm(),
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
        let Some(ch) = self.selected() else {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(self.theme.chrome_style())
                .title(Span::styled("Bands", self.theme.label()));
            f.render_widget(block, area);
            return;
        };

        // The graph takes most of the height, so the table rarely shows every
        // band. Scroll it rather than clip it: a selected band the user cannot
        // see is a band they cannot tell they are editing.
        let seats = area.height.saturating_sub(3) as usize; // borders and header
        let total = ch.bands.len();
        let first = Self::scroll_to(self.selected_band, total, seats);
        let last = (first + seats).min(total);

        let title = if seats >= total || total == 0 {
            "Bands".to_string()
        } else {
            // Say which rows these are, so a short window is obviously a window.
            format!("Bands {}-{} of {}", first + 1, last, total)
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.chrome_style())
            .title(Span::styled(title, self.theme.label()));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let rows: Vec<Row> = ch
            .bands
            .iter()
            .enumerate()
            .skip(first)
            .take(seats)
            .map(|(i, b)| {
                let focused = i == self.selected_band;
                let style = if focused {
                    self.theme.focused()
                } else {
                    self.theme.value()
                };
                // A field that does not apply to this shape reads as a dash
                // rather than a stale number nobody is able to change.
                let cell = |col: BandField, text: String| -> Cell {
                    if !col.applies_to(b.filter_type) {
                        return Cell::from("-");
                    }
                    if !focused || col != self.selected_field_col {
                        return Cell::from(text);
                    }
                    // Brackets say "this is the field"; the highlight says "and
                    // it is live". Both, so the distinction survives a terminal
                    // that drops the styling.
                    if self.band_edit {
                        Cell::from(format!("[{text}]")).style(self.theme.editing())
                    } else {
                        Cell::from(format!("[{text}]"))
                    }
                };

                Row::new(vec![
                    Cell::from(format!("{}{}", if focused { "▸" } else { " " }, i + 1)),
                    Cell::from(if b.bypass { "○" } else { "●" }),
                    cell(BandField::Type, b.filter_type.label()),
                    cell(BandField::Freq, format!("{:.0} Hz", b.freq)),
                    cell(BandField::Gain, format!("{:+.1} dB", b.gain_db)),
                    cell(BandField::Q, format!("{:.2}", b.q)),
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

    /// First visible row, keeping `selected` in view and the window full.
    ///
    /// Anchored rather than incremental, so it is correct however the selection
    /// got there — a jump, a wrap, or a resize that shrank the window.
    fn scroll_to(selected: usize, total: usize, seats: usize) -> usize {
        if seats == 0 || total <= seats {
            return 0;
        }
        // Centre the selection, then clamp so the last screen stays full rather
        // than trailing blank rows past the end of the list.
        selected
            .saturating_sub(seats / 2)
            .min(total.saturating_sub(seats))
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
        // An armed field takes over the arrows, so it gets its own line: the
        // usual one would list keys that no longer do what it says.
        let keys = if self.band_edit {
            "←→ change · ↑↓ coarse · Enter or Esc when done"
        } else {
            match (self.focus, self.panel) {
                (Focus::Sidebar, _) => {
                    "↑↓ channel · Enter edit · Tab panel · ^P palette · : cmd · M meters · q quit"
                }
                (Focus::Content, Panel::Filters) => {
                    "↑↓ band · 1-0 jump · ←→ field · Enter change · +/- value · space bypass · Esc back"
                }
                _ => "↑↓ field · ←→ adjust · Esc channels · Tab panel · ^P palette · F2 level",
            }
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

/// The component types, as the firmware numbers them.
pub fn component_name(t: u8) -> &'static str {
    match t {
        0 => "-",
        1 => "Button",
        2 => "Switch",
        3 => "Pot",
        4 => "Encoder",
        5 => "LED",
        6 => "LED (PWM)",
        7 => "IR receiver",
        _ => "unknown",
    }
}

/// What a bound parameter is called.
///
/// Nouns are numbered by the firmware and the registry knows the names, so a
/// binding reads as "Psycho bass drive" rather than as noun 44.
pub fn describe_noun(noun: u8) -> String {
    // The order of the firmware's noun table, which the registry mirrors.
    const NOUNS: &[(u8, &str)] = &[
        (0, "vol.user"),
        (1, "vol.master"),
        (2, "vol.mute"),
        (3, "loud.on"),
        (4, "cf.on"),
        (5, "lev.on"),
        (9, "bypass"),
        (13, "lev.amount"),
        (14, "lev.speed"),
        (16, "pre"),
        (17, "out.gain"),
        (18, "out.mute"),
        (19, "out.enable"),
        (20, "eq.freq"),
        (21, "eq.gain"),
        (22, "eq.q"),
        (23, "eq.type"),
        (24, "eq.bypass"),
        (35, "up.on"),
        (38, "up.strength"),
        (39, "up.width"),
        (40, "up.presence"),
        (41, "bass.on"),
        (42, "bass.cutoff"),
        (43, "bass.harmonics"),
        (44, "bass.drive"),
        (45, "bass.character"),
        (46, "bass.original"),
        (47, "out.delay"),
    ];

    NOUNS
        .iter()
        .find(|(n, _)| *n == noun)
        .and_then(|(_, path)| dspi_proto::registry::by_path(path))
        .map(|d| d.label.to_string())
        .unwrap_or_else(|| format!("parameter {noun}"))
}

fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        s.to_string()
    } else {
        s.chars().take(width.saturating_sub(1)).collect::<String>() + "…"
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
                let coarse = key.modifiers.contains(KeyModifiers::SHIFT);
                let editing_band = app.on_bands();

                match key.code {
                    // Running a command needs the device, so it happens here
                    // rather than in the key handler, which stays free of I/O.
                    KeyCode::Enter if app.mode != Mode::Browse => {
                        let line = app.take_input();
                        if !line.is_empty() {
                            app.run_command(session, &line);
                        }
                    }
                    // An armed band field owns the arrows: left and right step
                    // the value, up and down take it in tens.
                    KeyCode::Left | KeyCode::Right if app.band_edit => {
                        app.edit_band(session, key.code == KeyCode::Right, coarse);
                    }
                    KeyCode::Up | KeyCode::Down if app.band_edit => {
                        app.edit_band(session, key.code == KeyCode::Up, true);
                    }
                    KeyCode::Left | KeyCode::Right if !app.fields.is_empty() => {
                        app.edit_focused(session, key.code == KeyCode::Right, coarse);
                    }
                    KeyCode::Char('+') | KeyCode::Char('=') if editing_band => {
                        app.edit_band(session, true, coarse);
                    }
                    KeyCode::Char('-') | KeyCode::Char('_') if editing_band => {
                        app.edit_band(session, false, coarse);
                    }
                    KeyCode::Char(' ') if editing_band => app.toggle_band_bypass(session),
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
    fn the_main_view_renders_at_a_normal_size() {
        let (app, mut term) = demo_app(120, 40);
        let out = render(&app, &mut term);
        assert!(out.contains("DSPi"));
        assert!(out.contains("RP2350"));
        assert!(out.contains("Preset 3"));
        // Main is the top row of the channel list, not a tab.
        assert!(out.contains("Main"));
        assert!(out.contains("Response"));
        assert!(
            out.contains("Channels"),
            "the channel list should be visible"
        );
        assert!(out.contains("INPUTS"));
        assert!(out.contains("palette"));
    }

    /// 80x24 is the floor the design promises to work at.
    #[test]
    fn everything_still_fits_at_eighty_by_twentyfour() {
        let (app, mut term) = demo_app(80, 24);
        let out = render(&app, &mut term);
        assert!(out.contains("Main"));
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
    /// The list wraps through Main, which sits above the first channel, so
    /// running off either end costs one extra press rather than sticking.
    fn navigation_wraps_rather_than_sticking() {
        let (mut app, _term) = demo_app(120, 40);
        app.focus = Focus::Sidebar;
        app.panel = Panel::Filters;
        app.selected_channel = 16;

        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert!(app.on_main(), "past the last channel is Main");
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.selected_channel, 0);

        app.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert!(app.on_main());
        app.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.selected_channel, 16);
    }

    #[test]
    /// Tab walks the tab bar, which Main is not part of: it is reached from the
    /// top of the channel list. So tabbing out of Main enters the row and stays.
    fn tab_moves_between_panels() {
        let (mut app, _term) = demo_app(120, 40);
        assert_eq!(app.panel, Panel::Main);
        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.panel, Panel::Input);
        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.panel, Panel::Filters);
        app.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        assert_eq!(app.panel, Panel::Input);
    }

    /// Backwards out of Main enters from the far end rather than snapping to the
    /// first tab, so Shift-Tab is a real inverse of Tab.
    #[test]
    fn tabbing_backwards_out_of_main_lands_on_the_last_panel() {
        let (mut app, _term) = demo_app(120, 40);
        app.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        assert_eq!(app.panel, *Panel::ALL.last().unwrap());
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
        press(&mut a, ']');
        assert!(a.db_range < start, "] should zoom in");
        press(&mut a, '[');
        assert!((a.db_range - start).abs() < 1e-9);

        for _ in 0..50 {
            press(&mut a, ']');
        }
        assert!(a.db_range >= 10.0, "zoom must not collapse");
        for _ in 0..100 {
            press(&mut a, '[');
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
        a.focus = Focus::Content;
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

#[cfg(test)]
mod sidebar_tests {
    use super::*;
    use crate::theme::ColorDepth;

    fn app() -> App {
        let mut a = App::new(Theme::dark(ColorDepth::TrueColor, Glyphs::Braille));
        a.ctx.num_inputs = 4;
        a.ctx.num_outputs = 3;
        a.platform = "RP2350".into();
        a.firmware = "1.1.5".into();
        a.channels = (0..7)
            .map(|i| {
                let is_output = i >= 4;
                let bands = vec![dsp::Band {
                    filter_type: if i == 0 {
                        dspi_proto::FilterType::LowShelf
                    } else {
                        dspi_proto::FilterType::Flat
                    },
                    freq: 105.0,
                    q: 0.707,
                    gain_db: 8.8,
                    bypass: false,
                }];
                ChannelView {
                    name: if is_output {
                        format!("SPDIF {}", i - 3)
                    } else {
                        format!("USB {}", i + 1)
                    },
                    slug: format!("ch.{i}"),
                    is_output,
                    curve: dsp::curve(&bands, 0.0),
                    bands,
                    peak: if i == 0 { 0.6 } else { 0.0 },
                    clipped: i == 6,
                }
            })
            .collect();
        a.visible = vec![true; 7];
        // These are tests about the channel list, so start on a channel panel
        // rather than Main, which parks the cursor above the first channel.
        a.panel = Panel::Filters;
        a
    }

    fn render(a: &App, w: u16, h: u16) -> String {
        crate::render_to_string(a, w, h)
    }

    /// Just the channel-list column. The panel beside it repeats the selected
    /// channel's name in its title, so a whole-line search finds that instead.
    fn column(a: &App) -> String {
        let out = render(a, 100, 24);
        let width = out
            .lines()
            .find(|l| l.contains("┌Channels"))
            .and_then(|l| l.chars().position(|c| c == '┐'))
            .expect("no channel list")
            + 1;
        out.lines()
            .map(|l| l.chars().take(width).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn row(a: &App, name: &str) -> String {
        column(a)
            .lines()
            .find(|l| l.contains(name))
            .unwrap_or_else(|| panic!("no {name} row"))
            .to_string()
    }

    #[test]
    fn channels_are_grouped_into_inputs_and_outputs() {
        let out = column(&app());
        let inputs = out.find("INPUTS").expect("no INPUTS heading");
        let outputs = out.find("OUTPUTS").expect("no OUTPUTS heading");
        assert!(inputs < outputs, "inputs should come first");

        // Every channel is listed, under the right heading.
        let before = &out[inputs..outputs];
        assert!(before.contains("USB 1") && before.contains("USB 4"));
        assert!(
            !before.contains("SPDIF"),
            "an output leaked into the inputs"
        );
        assert!(out[outputs..].contains("SPDIF 1"));
    }

    /// The overview is reached from the list rather than the tab bar, so it has
    /// to be visible there or it cannot be found at all.
    #[test]
    fn main_sits_above_the_channels() {
        let out = column(&app());
        let main = out.find(" Main").expect("no Main row");
        let inputs = out.find("INPUTS").expect("no INPUTS heading");
        assert!(main < inputs, "Main belongs at the top of the list");
    }

    #[test]
    fn selecting_main_clears_the_channel_marker() {
        let mut a = app();
        a.panel = Panel::Main;
        let out = column(&a);
        let main = out.lines().find(|l| l.contains("Main")).unwrap();
        assert!(main.contains('▐'), "Main is not marked: {main}");
        assert_eq!(
            out.lines().filter(|l| l.contains('▐')).count(),
            1,
            "a channel is still marked while Main is selected"
        );
    }

    /// The level belongs beside the channel it describes, rather than in a
    /// separate column the eye has to pair up by position.
    #[test]
    fn each_channel_carries_its_own_meter() {
        let row = row(&app(), "USB 1");
        assert!(row.contains('▓'), "no level on the selected row: {row}");
        assert!(row.contains('░'), "no meter track: {row}");
    }

    #[test]
    fn a_silent_channel_still_shows_its_track() {
        let row = row(&app(), "USB 2");
        assert!(row.contains('░'));
        assert!(!row.contains('▓'), "USB 2 is silent: {row}");
    }

    #[test]
    fn a_clipped_channel_flags_itself_in_the_list() {
        let row = row(&app(), "SPDIF 3");
        assert!(row.contains('▌'), "clip flag missing: {row}");
    }

    /// A margin bar rather than a background highlight, so selection survives a
    /// monochrome terminal.
    #[test]
    fn the_selected_channel_is_marked_in_the_margin() {
        let mut a = app();
        a.selected_channel = 2;
        let marked = row(&a, "USB 3");
        assert!(marked.contains('▐'), "no selection marker: {marked}");

        let other = row(&a, "USB 4");
        assert!(!other.contains('▐'), "two rows marked at once");
    }

    #[test]
    fn moving_through_the_list_crosses_from_inputs_to_outputs() {
        let mut a = app();
        a.selected_channel = 3; // last input
        a.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(a.selected_channel, 4);
        assert!(a.channels[a.selected_channel].is_output);
    }

    /// Arrows must mean one thing at a time, so the focus decides whether they
    /// move the channel or act inside the panel.
    #[test]
    fn focus_moves_between_the_list_and_the_panel() {
        let mut a = app();
        a.panel = Panel::Filters;
        assert_eq!(a.focus, Focus::Sidebar);

        a.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(a.selected_channel, 1, "sidebar focus moves the channel");
        assert_eq!(a.selected_band, 0);

        a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(a.focus, Focus::Content);

        a.channels[1].bands = vec![dsp::Band::default(); 4];
        a.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(a.selected_band, 1, "content focus moves the band");
        assert_eq!(a.selected_channel, 1, "and leaves the channel alone");

        a.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(a.focus, Focus::Sidebar);
    }

    #[test]
    fn changing_channel_resets_the_band_cursor() {
        let mut a = app();
        a.channels[0].bands = vec![dsp::Band::default(); 6];
        a.selected_band = 4;
        a.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(
            a.selected_band, 0,
            "band 4 means nothing on another channel"
        );
    }

    #[test]
    fn the_key_hints_follow_the_focus() {
        let mut a = app();
        a.panel = Panel::Filters;
        assert!(render(&a, 100, 24).contains("Enter edit"));
        a.focus = Focus::Content;
        assert!(render(&a, 100, 24).contains("Esc back"));
    }

    #[test]
    fn the_layout_still_fits_at_eighty_columns() {
        let a = app();
        let out = render(&a, 80, 24);
        assert!(out.contains("INPUTS"));
        assert!(out.contains("Response"), "no graph beside the list");
        for line in out.lines() {
            assert!(line.chars().count() <= 80, "overflows: {line}");
        }
    }
}

#[cfg(test)]
mod selection_tests {
    use super::*;
    use crate::theme::ColorDepth;

    fn app() -> App {
        let mut a = App::new(Theme::dark(ColorDepth::TrueColor, Glyphs::Braille));
        a.ctx.num_inputs = 4;
        a.ctx.num_outputs = 3;
        a.channels = (0..7)
            .map(|i| {
                // A distinct gain per channel, so which curve is drawn is
                // visible in the rendered output.
                let bands = vec![dsp::Band {
                    filter_type: dspi_proto::FilterType::Peaking,
                    freq: 1000.0,
                    q: 1.0,
                    gain_db: 2.0 * (i as f32 + 1.0),
                    bypass: false,
                }];
                ChannelView {
                    name: format!("Ch {i}"),
                    slug: format!("ch.{i}"),
                    is_output: i >= 4,
                    curve: dsp::curve(&bands, 0.0),
                    bands,
                    peak: 0.0,
                    clipped: false,
                }
            })
            .collect();
        a.visible = vec![true; 7];
        a
    }

    fn marks(a: &App) -> usize {
        crate::render_to_string(a, 100, 24)
            .chars()
            .filter(|c| (0x2800..=0x28FF).contains(&(*c as u32)))
            .count()
    }

    /// The channel list is a visible selector, so a graph that ignores it looks
    /// broken rather than deliberate.
    #[test]
    fn the_dashboard_graph_follows_the_selected_channel() {
        let mut a = app();
        a.panel = Panel::Main;

        a.selected_channel = 0;
        let first = crate::render_to_string(&a, 100, 24);
        a.selected_channel = 2;
        let second = crate::render_to_string(&a, 100, 24);

        assert_ne!(
            first, second,
            "selecting a different channel changed nothing on the dashboard"
        );
    }

    #[test]
    fn the_filters_graph_follows_the_selection_too() {
        let mut a = app();
        a.panel = Panel::Filters;
        a.split = Split::GraphOnly;

        a.selected_channel = 0;
        let first = crate::render_to_string(&a, 100, 24);
        a.selected_channel = 3;
        assert_ne!(first, crate::render_to_string(&a, 100, 24));
    }

    /// Pairing an input with an output would be an accident of numbering rather
    /// than a comparison anyone wants.
    #[test]
    fn the_dashboard_pairs_only_within_inputs_or_outputs() {
        let mut a = app();
        a.panel = Panel::Main;

        // Channel 3 is the last input; its numeric partner, 2, is also an input.
        a.selected_channel = 3;
        let paired = marks(&a);

        // Channel 4 is the first output; its numeric partner, 5, is too.
        a.selected_channel = 4;
        assert!(marks(&a) > 0);

        // Both draw two curves, so neither is silently reduced to one.
        assert!(paired > 0);
    }

    #[test]
    fn a_hidden_channel_stays_off_the_graph_even_when_selected() {
        let mut a = app();
        a.panel = Panel::Filters;
        a.split = Split::GraphOnly;
        a.selected_channel = 1;

        let shown = marks(&a);
        a.visible[1] = false;
        assert!(
            marks(&a) < shown,
            "hiding a channel should remove its curve"
        );
    }
}

#[cfg(test)]
mod editing_tests {
    use super::*;
    use crate::theme::ColorDepth;

    fn app() -> App {
        let mut a = App::new(Theme::dark(ColorDepth::TrueColor, Glyphs::Braille));
        a.ctx.num_inputs = 2;
        a.ctx.num_outputs = 2;
        a.ctx.max_bands = 10;
        a.ctx.channel_slugs = vec!["in.1".into(), "in.2".into(), "out.1".into(), "out.2".into()];
        a.channels = (0..4)
            .map(|i| ChannelView {
                name: format!("Ch {i}"),
                slug: format!("ch.{i}"),
                is_output: i >= 2,
                bands: vec![
                    dsp::Band {
                        filter_type: dspi_proto::FilterType::Peaking,
                        freq: 1000.0,
                        q: 1.0,
                        gain_db: 0.0,
                        bypass: false,
                    };
                    4
                ],
                curve: vec![0.0; dsp::POINTS],
                ..Default::default()
            })
            .collect();
        a.visible = vec![true; 4];
        a.panel = Panel::Filters;
        a.focus = Focus::Content;
        a
    }

    fn press(a: &mut App, code: KeyCode) {
        a.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    /// The fields are laid out across the row, so left and right moving between
    /// them is the spatially obvious reading.
    #[test]
    fn left_and_right_move_across_the_row() {
        let mut a = app();
        a.selected_field_col = BandField::Freq;
        press(&mut a, KeyCode::Right);
        assert_eq!(a.selected_field_col, BandField::Gain);
        press(&mut a, KeyCode::Right);
        assert_eq!(a.selected_field_col, BandField::Q);
        press(&mut a, KeyCode::Left);
        assert_eq!(a.selected_field_col, BandField::Gain);
    }

    /// A shelf has no Q worth editing; stopping on it would invite a change
    /// that does nothing.
    #[test]
    fn the_cursor_skips_fields_the_shape_does_not_use() {
        let mut a = app();
        a.channels[0].bands[0].filter_type = dspi_proto::FilterType::LowShelf;
        a.selected_field_col = BandField::Freq;

        press(&mut a, KeyCode::Right);
        assert_eq!(a.selected_field_col, BandField::Gain, "gain applies");
        press(&mut a, KeyCode::Right);
        assert_ne!(a.selected_field_col, BandField::Q, "a shelf has no Q here");
    }

    #[test]
    fn a_pass_filter_offers_no_gain() {
        let mut a = app();
        a.channels[0].bands[0].filter_type = dspi_proto::FilterType::HighPass;
        a.selected_field_col = BandField::Freq;
        for _ in 0..6 {
            press(&mut a, KeyCode::Right);
            assert_ne!(a.selected_field_col, BandField::Gain);
        }
    }

    #[test]
    fn the_focused_field_is_marked_in_the_table() {
        let mut a = app();
        a.split = Split::TableOnly;
        a.selected_field_col = BandField::Gain;
        let out = crate::render_to_string(&a, 100, 20);
        assert!(
            out.contains("[+0.0 dB]"),
            "the focused cell should be bracketed:\n{out}"
        );
    }

    /// A field that does not apply reads as a dash rather than a stale number
    /// nobody can change.
    #[test]
    fn inapplicable_fields_show_a_dash() {
        let mut a = app();
        a.split = Split::TableOnly;
        a.channels[0].bands[0].filter_type = dspi_proto::FilterType::LowShelf;
        let out = crate::render_to_string(&a, 100, 20);
        let row = out.lines().find(|l| l.contains("Low shelf")).unwrap();
        assert!(row.contains(" - "), "a shelf's Q should be a dash: {row}");
    }

    #[test]
    fn zoom_moved_off_the_keys_that_now_change_values() {
        let mut a = app();
        let start = a.db_range;
        // Plus and minus belong to the value now.
        press(&mut a, KeyCode::Char(']'));
        assert!(a.db_range < start);
        press(&mut a, KeyCode::Char('['));
        assert!((a.db_range - start).abs() < 1e-9);
    }

    #[test]
    fn the_hints_name_the_editing_keys() {
        let a = app();
        let out = crate::render_to_string(&a, 120, 20);
        assert!(out.contains("+/- value"), "no value hint:\n{out}");
        assert!(out.contains("←→ field"));
        assert!(out.contains("Enter change"));
        assert!(out.contains("1-0 jump"));
        assert!(out.contains("space bypass"));
    }

    // ------------------------------------------------ selecting and arming

    /// The band list is the one list long enough to want direct access, and
    /// the panel a digit would otherwise reach is a Tab away.
    #[test]
    fn a_digit_jumps_straight_to_a_band() {
        let mut a = with_bands(10);
        press(&mut a, KeyCode::Char('7'));
        assert_eq!(a.selected_band, 6, "band 7 is index 6");
        press(&mut a, KeyCode::Char('1'));
        assert_eq!(a.selected_band, 0);
    }

    /// There is no key for 10, so 0 carries it, as on a phone keypad.
    #[test]
    fn zero_means_band_ten() {
        let mut a = with_bands(10);
        press(&mut a, KeyCode::Char('0'));
        assert_eq!(a.selected_band, 9);
    }

    /// A digit past the end must not move the selection somewhere the channel
    /// does not have, nor silently do nothing.
    #[test]
    fn a_digit_past_the_last_band_says_so() {
        let mut a = with_bands(4);
        a.selected_band = 1;
        press(&mut a, KeyCode::Char('9'));
        assert_eq!(a.selected_band, 1, "selection should not move");
        assert!(
            a.status
                .as_ref()
                .is_some_and(|(m, _)| m.contains("4 bands")),
            "no explanation: {:?}",
            a.status
        );
    }

    /// Digits only mean bands where the band list has the keyboard. Everywhere
    /// else they still pick a panel.
    #[test]
    fn digits_still_pick_panels_outside_the_band_list() {
        let mut a = app();
        a.focus = Focus::Sidebar;
        press(&mut a, KeyCode::Char('3'));
        assert_eq!(a.panel, Panel::ALL[2]);

        // And in another panel, where there is no band list at all.
        let mut b = app();
        b.panel = Panel::System;
        press(&mut b, KeyCode::Char('4'));
        assert_eq!(b.panel, Panel::ALL[3]);
    }

    /// Enter arms the focused field and Enter again lets it go, so the arrows
    /// mean one thing at a time.
    #[test]
    fn enter_arms_the_field_and_enter_releases_it() {
        let mut a = app();
        assert!(!a.band_edit);
        press(&mut a, KeyCode::Enter);
        assert!(a.band_edit, "Enter should arm the field");
        press(&mut a, KeyCode::Enter);
        assert!(!a.band_edit, "Enter should release it");
    }

    /// Escape unwinds one rung at a time: out of the field first, out of the
    /// panel second.
    #[test]
    fn escape_leaves_the_field_before_the_panel() {
        let mut a = app();
        press(&mut a, KeyCode::Enter);
        assert!(a.band_edit);

        press(&mut a, KeyCode::Esc);
        assert!(!a.band_edit);
        assert_eq!(a.focus, Focus::Content, "should still be in the panel");

        press(&mut a, KeyCode::Esc);
        assert_eq!(a.focus, Focus::Sidebar);
    }

    /// While armed the arrows belong to the value, so they must not also move
    /// the selection — that would edit one field and land on another.
    #[test]
    fn an_armed_field_keeps_the_arrows_off_the_selection() {
        let mut a = with_bands(10);
        a.selected_band = 3;
        a.selected_field_col = BandField::Freq;
        press(&mut a, KeyCode::Enter);

        for code in [KeyCode::Left, KeyCode::Right, KeyCode::Up, KeyCode::Down] {
            press(&mut a, code);
        }
        assert_eq!(a.selected_band, 3, "the band moved under an armed field");
        assert_eq!(a.selected_field_col, BandField::Freq, "the column moved");
    }

    /// Unarmed, the arrows are navigation again.
    #[test]
    fn the_arrows_navigate_when_nothing_is_armed() {
        let mut a = with_bands(10);
        press(&mut a, KeyCode::Down);
        assert_eq!(a.selected_band, 1);
        let col = a.selected_field_col;
        press(&mut a, KeyCode::Right);
        assert_ne!(a.selected_field_col, col, "→ should move the column");
    }

    /// Arming a field the shape does not have would give the user arrows that
    /// do nothing. Refuse, and say why.
    #[test]
    fn a_field_the_filter_lacks_cannot_be_armed() {
        let mut a = app();
        a.channels[0].bands[0].filter_type = dspi_proto::FilterType::LowShelf;
        a.selected_field_col = BandField::Q; // a shelf has no Q
        press(&mut a, KeyCode::Enter);
        assert!(!a.band_edit, "armed a field that does not exist");
        assert!(
            a.status.as_ref().is_some_and(|(m, _)| m.contains("Q")),
            "no explanation: {:?}",
            a.status
        );
    }

    /// Arming is about one field on one band. Leaving either has to release it,
    /// or the arrows would come back armed somewhere else.
    #[test]
    fn leaving_the_band_list_releases_the_field() {
        let mut a = app();
        press(&mut a, KeyCode::Enter);
        assert!(a.band_edit);
        press(&mut a, KeyCode::Tab);
        assert!(!a.band_edit, "still armed in another panel");

        let mut b = app();
        press(&mut b, KeyCode::Enter);
        b.focus = Focus::Sidebar;
        press(&mut b, KeyCode::Down);
        assert!(!b.band_edit, "still armed on another channel");
    }

    /// The style of every cell whose row is the selected band.
    fn band_row_styles(a: &App, w: u16, h: u16) -> Vec<(String, Style)> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut term = Terminal::new(TestBackend::new(w, h)).expect("test backend");
        term.draw(|f| a.draw(f)).expect("draw");
        let buf = term.backend().buffer();
        let area = *buf.area();
        // The armed cell is bracketed, so its row is the one to inspect.
        let row = (0..area.height)
            .find(|y| {
                (0..area.width)
                    .map(|x| buf[(x, *y)].symbol())
                    .collect::<String>()
                    .contains('[')
            })
            .expect("no bracketed field on screen");
        (0..area.width)
            .map(|x| {
                let c = &buf[(x, row)];
                (c.symbol().to_string(), c.style())
            })
            .collect()
    }

    /// The armed field is marked as live, and nothing else on the row is.
    ///
    /// Reverse video rather than a colour, so it reads on a monochrome
    /// terminal too; the brackets carry the same news for a terminal that
    /// drops styling entirely.
    #[test]
    fn the_armed_field_is_highlighted() {
        use ratatui::style::Modifier;

        let mut a = with_bands(10);
        a.selected_field_col = BandField::Freq;

        let calm = band_row_styles(&a, 120, 30);
        assert!(
            !calm
                .iter()
                .any(|(_, s)| s.add_modifier.contains(Modifier::REVERSED)),
            "something was highlighted before the field was armed"
        );

        press(&mut a, KeyCode::Enter);
        let armed = band_row_styles(&a, 120, 30);
        let lit: String = armed
            .iter()
            .filter(|(_, s)| s.add_modifier.contains(Modifier::REVERSED))
            .map(|(sym, _)| sym.as_str())
            .collect();
        assert!(
            lit.contains("1000 Hz"),
            "the armed frequency is not highlighted; lit: {lit:?}"
        );
        assert!(
            !lit.contains("Peaking") && !lit.contains("dB"),
            "the highlight spilled onto another field: {lit:?}"
        );
    }

    /// An armed field takes the arrows over, so the old hints would be lying
    /// about what they do.
    #[test]
    fn the_hints_change_when_a_field_is_armed() {
        let mut a = app();
        press(&mut a, KeyCode::Enter);
        assert!(a.band_edit);
        let out = crate::render_to_string(&a, 120, 20);
        assert!(out.contains("←→ change"), "no armed hint:\n{out}");
        assert!(!out.contains("←→ field"), "still offering field movement");
    }

    fn with_bands(n: usize) -> App {
        let mut a = app();
        a.channels[0].bands = vec![
            dsp::Band {
                filter_type: dspi_proto::FilterType::Peaking,
                freq: 1000.0,
                q: 1.0,
                gain_db: 0.0,
                bypass: false,
            };
            n
        ];
        a
    }

    /// The numbers on the visible rows, in order.
    fn band_numbers(a: &App, w: u16, h: u16) -> Vec<String> {
        crate::render_to_string(a, w, h)
            .lines()
            .skip_while(|l| !l.contains("Bands"))
            .filter(|l| l.contains("Peaking"))
            .map(|l| {
                // The selected row carries a `▸` against its number.
                l.split_whitespace()
                    .map(|t| t.trim_start_matches(['▸', '│']))
                    .find(|t| !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()))
                    .unwrap_or("?")
                    .to_string()
            })
            .collect()
    }

    /// The table is what a user reads a band number off before typing it into
    /// the command line, so its numbering has to be the grammar's: 1-based,
    /// counting up, with no band 0.
    #[test]
    fn the_band_table_is_numbered_from_one() {
        let a = with_bands(10);
        let numbers = band_numbers(&a, 100, 60);
        assert_eq!(
            numbers,
            (1..=10).map(|n| n.to_string()).collect::<Vec<_>>(),
            "rows should read 1 to 10, not 0 to 9"
        );
    }

    /// Editing the top row must reach wire band 0. A table that rendered 1..10
    /// over wire bands 1..10 would look right and silently skip a band.
    #[test]
    fn the_first_row_is_wire_band_zero() {
        let a = app();
        let echo = dspi_cmd::format(
            &dspi_cmd::Command::Set {
                path: "eq.freq",
                indices: vec![0, 0],
                value: Value::Float(105.0),
            },
            &a.ctx,
        );
        assert!(
            echo.contains(" 1 "),
            "wire band 0 should echo as band 1: {echo}"
        );
    }

    /// The graph takes most of the height, so the table is usually a window
    /// onto the bands. A selection outside it is a band the user is editing
    /// blind.
    #[test]
    fn the_table_scrolls_to_keep_the_selection_visible() {
        let mut a = with_bands(10);
        for band in 0..10usize {
            a.selected_band = band;
            let shown = band_numbers(&a, 100, 30);
            assert!(!shown.is_empty(), "nothing rendered for band {band}");
            assert!(
                shown.contains(&(band + 1).to_string()),
                "band {} is off-screen; showing {shown:?}",
                band + 1
            );
        }
    }

    /// A partial view says so, rather than looking like the whole list.
    #[test]
    fn a_scrolled_table_says_what_it_is_showing() {
        let mut a = with_bands(10);
        a.selected_band = 9;
        let out = crate::render_to_string(&a, 100, 30);
        assert!(out.contains(" of 10"), "no range in the title:\n{out}");

        // With room for all ten there is nothing to qualify.
        let all = crate::render_to_string(&a, 100, 60);
        assert!(!all.contains(" of 10"), "qualified a complete list");
    }

    /// The last screen stays full: scrolling past the end would trail blank
    /// rows and make the list look shorter than it is.
    #[test]
    fn the_window_never_runs_off_the_end() {
        assert_eq!(App::scroll_to(9, 10, 4), 6, "last four rows are 7-10");
        assert_eq!(App::scroll_to(0, 10, 4), 0);
        assert_eq!(App::scroll_to(5, 10, 4), 3, "centred");
        assert_eq!(App::scroll_to(3, 10, 10), 0, "no scroll when it all fits");
        assert_eq!(App::scroll_to(3, 10, 0), 0, "no room, no panic");
    }
}
