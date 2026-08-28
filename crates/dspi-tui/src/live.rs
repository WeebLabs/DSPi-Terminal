//! The shell, wired to a device.
//!
//! `Live` owns the shell, the device state and the session, and runs the
//! event loop: keys go to the shell, the shell's events become writes, meter
//! polls and notifications update the state, and the state is projected back
//! into the shell's model every frame. The command line and the palette live
//! here too, since they need the device.
//!
//! Screens (the detail region, the tool panels, Settings) come from a
//! [`Screens`] factory so the phases that build them plug in without touching
//! the loop.

use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEvent, KeyModifiers};
use dspi_cmd::{Candidate, Context};
use dspi_proto::dsp;
use dspi_proto::value::Value;
use dspi_session::{Applied, DeviceState, Notifications, Outcome, Session, Source};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Widget};

use crate::app::Performance;
use crate::graph::GraphCurve;
use crate::screens::{
    self, CrossfeedPanel, InputPage, LevellerPanel, LoudnessPanel, MatrixPanel, OutputPage,
    Overview, PresetChoice, PresetMenu, PsybassPanel, Shared, SignalsPanel, UpmixerPanel,
    clipboard, panel, presets,
};
use crate::settings::{AppConfig, SettingsData, SettingsScreen};
use crate::shell::{
    ChannelItem, Placeholder, Screen, Selection, Shell, ShellEvent, ShellModel, Tool, VolumeMode,
};
use crate::theme::{ChannelRole, Glyphs, Theme};
use crate::widgets::text::truncate;
use crate::widgets::{Button, Dialog, DialogOutcome, PeakHold, PopupList};

/// Makes the screens the shell shows. The default makes placeholders; later
/// phases replace it.
pub trait Screens {
    fn detail(&self, state: &DeviceState, selection: Selection) -> Box<dyn Screen>;
    fn tool(&self, state: &DeviceState, tool: Tool) -> Box<dyn Screen>;
    fn settings(&self, state: &DeviceState) -> Box<dyn Screen>;

    /// The application-side state the screens share, when the factory keeps
    /// one. The runner needs it for the sidebar actions that are not a screen:
    /// the preset menu, the channel clipboard, and mirroring onto a linked
    /// input pair.
    fn shared(&self) -> Option<Shared> {
        None
    }

    /// The app-side settings file, which Settings > Graphing writes and the
    /// graph reads on start.
    fn config(&self) -> AppConfig {
        AppConfig::default()
    }

    /// Read what Settings needs that `DeviceState` does not carry. Called just
    /// before the page opens, so a change made elsewhere is on screen.
    fn refresh_settings(&self, _session: &mut Session) {}
}

pub struct PlaceholderScreens;

impl Screens for PlaceholderScreens {
    fn detail(&self, state: &DeviceState, selection: Selection) -> Box<dyn Screen> {
        let (title, body) = match selection {
            Selection::Overview => (
                "Overview".to_string(),
                "The dashboard cards arrive in Phase 4.".to_string(),
            ),
            Selection::Input(i) => (
                state.channel_name(i),
                "The input page arrives in Phase 4.".into(),
            ),
            Selection::Output(o) => (
                state.channel_name(state.caps.num_inputs as usize + o),
                "The output page arrives in Phase 4.".into(),
            ),
        };
        Box::new(Placeholder::new(title, body))
    }

    fn tool(&self, _state: &DeviceState, tool: Tool) -> Box<dyn Screen> {
        Box::new(Placeholder::new(
            tool.title(),
            "This panel arrives in a later phase.",
        ))
    }

    fn settings(&self, _state: &DeviceState) -> Box<dyn Screen> {
        Box::new(Placeholder::new("Settings", "Settings arrive in Phase 7."))
    }
}

/// The Console's screens: the dashboard, the input page and the output page.
///
/// The Matrix Mixer is the one tool panel that exists; the rest, and Settings,
/// are still placeholders that later phases replace. Every screen it makes
/// shares one [`Shared`] handle, which is where the linked pairs, the preset
/// names and the channel clipboard live.
pub struct ConsoleScreens {
    pub shared: Shared,
    /// What Settings reads that the bulk packet does not carry, refreshed the
    /// moment before the page opens.
    pub settings: std::rc::Rc<std::cell::RefCell<SettingsData>>,
    /// The app-side settings file, read once at start.
    pub config: AppConfig,
}

impl Default for ConsoleScreens {
    fn default() -> Self {
        Self::new()
    }
}

impl ConsoleScreens {
    pub fn new() -> Self {
        Self {
            shared: screens::shared(),
            settings: std::rc::Rc::new(std::cell::RefCell::new(SettingsData::default())),
            config: AppConfig::load(),
        }
    }
}

impl Screens for ConsoleScreens {
    fn detail(&self, state: &DeviceState, selection: Selection) -> Box<dyn Screen> {
        match selection {
            Selection::Overview => Box::new(Overview::new(self.shared.clone())),
            Selection::Input(i) => Box::new(InputPage::new(i, self.shared.clone(), state)),
            Selection::Output(o) => Box::new(OutputPage::new(o, self.shared.clone(), state)),
        }
    }

    fn tool(&self, _state: &DeviceState, tool: Tool) -> Box<dyn Screen> {
        match tool {
            Tool::Matrix => Box::new(MatrixPanel::new(self.shared.clone())),
            Tool::Crossfeed => Box::new(CrossfeedPanel::new()),
            Tool::Loudness => Box::new(LoudnessPanel::new()),
            Tool::Leveller => Box::new(LevellerPanel::new()),
            Tool::Psybass => Box::new(PsybassPanel::new()),
            Tool::Upmixer => Box::new(UpmixerPanel::new()),
            Tool::Signals => Box::new(SignalsPanel::new()),
            _ => Box::new(Placeholder::new(
                tool.title(),
                "This panel arrives in a later phase.",
            )),
        }
    }

    fn settings(&self, state: &DeviceState) -> Box<dyn Screen> {
        Box::new(SettingsScreen::new(
            state,
            self.settings.borrow().clone(),
            self.config.clone(),
        ))
    }

    fn shared(&self) -> Option<Shared> {
        Some(self.shared.clone())
    }

    fn config(&self) -> AppConfig {
        self.config.clone()
    }

    fn refresh_settings(&self, session: &mut Session) {
        *self.settings.borrow_mut() = SettingsData::read(session);
    }
}

/// The `:` line and the `Ctrl-P` palette.
#[derive(Debug, Clone, PartialEq)]
pub struct Prompt {
    pub palette: bool,
    pub input: String,
    pub candidates: Vec<Candidate>,
    pub index: usize,
}

impl Prompt {
    pub fn new(palette: bool, ctx: &Context) -> Self {
        let mut p = Self {
            palette,
            input: String::new(),
            candidates: Vec::new(),
            index: 0,
        };
        p.refresh(ctx);
        p
    }

    pub fn refresh(&mut self, ctx: &Context) {
        let ends_with_space = self.input.ends_with(' ');
        let mut tokens: Vec<&str> = self.input.split_whitespace().collect();
        let partial = if ends_with_space {
            ""
        } else {
            tokens.pop().unwrap_or("")
        };
        self.candidates = dspi_cmd::complete(&tokens, partial, ctx);
        self.index = 0;
    }

    /// Returns `Some(line)` on Enter, `Some("")` on Escape.
    pub fn handle(&mut self, key: KeyEvent, ctx: &Context) -> Option<String> {
        match key.code {
            KeyCode::Esc => Some(String::new()),
            KeyCode::Enter => Some(std::mem::take(&mut self.input)),
            KeyCode::Tab => {
                if let Some(c) = self.candidates.get(self.index)
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
                    self.refresh(ctx);
                }
                None
            }
            KeyCode::Down => {
                if !self.candidates.is_empty() {
                    self.index = (self.index + 1) % self.candidates.len();
                }
                None
            }
            KeyCode::Up => {
                if !self.candidates.is_empty() {
                    self.index = self
                        .index
                        .checked_sub(1)
                        .unwrap_or(self.candidates.len() - 1);
                }
                None
            }
            KeyCode::Backspace => {
                self.input.pop();
                self.refresh(ctx);
                None
            }
            KeyCode::Char(c) => {
                self.input.push(c);
                self.refresh(ctx);
                None
            }
            _ => None,
        }
    }

    pub fn draw(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let height = (self.candidates.len().min(8) as u16 + 3).min(area.height);
        let rect = Rect::new(
            area.x + 2,
            area.y + area.height.saturating_sub(height + 2),
            area.width.saturating_sub(4),
            height,
        );
        Clear.render(rect, buf);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(if theme.glyphs == Glyphs::Ascii {
                BorderType::Plain
            } else {
                BorderType::Rounded
            })
            .border_style(Style::default().fg(theme.accent))
            .title(if self.palette {
                " Search "
            } else {
                " Command "
            })
            .title_style(theme.title());
        let inner = block.inner(rect);
        block.render(rect, buf);
        let prompt = if self.palette { "› " } else { ":" };
        buf.set_string(inner.x, inner.y, prompt, Style::default().fg(theme.accent));
        let px = inner.x + prompt.chars().count() as u16;
        buf.set_string(
            px,
            inner.y,
            &self.input,
            Style::default().add_modifier(Modifier::BOLD),
        );
        buf.set_string(
            px + self.input.chars().count() as u16,
            inner.y,
            "▏",
            Style::default().fg(theme.accent),
        );
        for (i, c) in self
            .candidates
            .iter()
            .take(inner.height as usize - 1)
            .enumerate()
        {
            let y = inner.y + 1 + i as u16;
            let style = if i == self.index {
                theme.focused()
            } else {
                theme.value()
            };
            if c.value.is_empty() {
                buf.set_string(
                    inner.x,
                    y,
                    truncate(&format!("  {}", c.detail), inner.width as usize),
                    theme.label(),
                );
            } else {
                buf.set_string(
                    inner.x,
                    y,
                    format!("  {:<24}", truncate(&c.value, 24)),
                    style,
                );
                buf.set_string(
                    inner.x + 26,
                    y,
                    truncate(&c.detail, (inner.width as usize).saturating_sub(26)),
                    theme.label(),
                );
            }
        }
    }
}

/// Which app-level dialog is up, so its outcome is routed here rather than
/// to a screen.
#[derive(Debug, Clone, PartialEq)]
enum AppDialog {
    /// Quit, or switch to `Some(slot)`, after saving or discarding.
    Unsaved {
        then: PendingAction,
    },
    SavePreset,
    Rename {
        channel: usize,
    },
    PresetList,
    /// The `Copy to...` submenu, with the slot each row stands for.
    PresetCopyTo(Vec<u8>),
    PresetRename(u8),
    PresetClear(u8),
    PresetClearAll,
    /// The Console's Core 1 collision prompt; `index` is the output to enable
    /// once the other side has been let go.
    Core1 {
        index: u8,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PendingAction {
    Quit,
    LoadPreset(u8),
}

pub struct Live {
    pub shell: Shell,
    pub state: DeviceState,
    pub perf: Performance,
    pub ctx: Context,
    pub prompt: Option<Prompt>,
    dialog: Option<(AppDialog, Dialog)>,
    popup: Option<(AppDialog, PopupList)>,
    status_until: Option<Instant>,
    peaks: Vec<PeakHold>,
    visible: Vec<bool>,
    screens: Box<dyn Screens>,
    /// The state the screens share, when the factory keeps one.
    shared: Shared,
    pub should_quit: bool,
    last_tick: Instant,
    /// When the upmixer's telemetry was last read. It is not a notification,
    /// so the only way to move the gauges is to ask, and once a second is
    /// enough for a meter a person is watching.
    last_upmix_poll: Instant,
}

impl Live {
    pub fn new(
        state: DeviceState,
        theme: Theme,
        perf: Performance,
        screens: Box<dyn Screens>,
    ) -> Self {
        let caps = &state.caps;
        let ctx = Context {
            channel_slugs: caps.channels.iter().map(|c| c.slug.clone()).collect(),
            num_inputs: caps.num_inputs,
            num_outputs: caps.num_outputs,
            max_bands: caps.max_bands,
        };
        let n = caps.num_channels as usize;
        let config = screens.config();
        let mut model = ShellModel::empty();
        model.selection = Selection::Overview;
        model.graph = crate::graph::GraphSettings::from_config(&config.graphing);
        model.volume_mode = match config.volume.mode {
            crate::settings::config::VolumeChoice::Master => VolumeMode::Master,
            crate::settings::config::VolumeChoice::User => VolumeMode::User,
        };
        let detail = screens.detail(&state, Selection::Overview);
        let shared = screens.shared().unwrap_or_else(screens::shared);
        let shell = Shell::new(model, theme, detail);
        let mut live = Self {
            shell,
            state,
            perf,
            ctx,
            prompt: None,
            dialog: None,
            popup: None,
            status_until: None,
            peaks: vec![PeakHold::default(); n],
            visible: vec![true; n],
            screens,
            shared,
            should_quit: false,
            last_tick: Instant::now(),
            last_upmix_poll: Instant::now() - Duration::from_secs(2),
        };
        live.sync_model();
        live
    }

    /// Project the device state into the shell's model.
    pub fn sync_model(&mut self) {
        let s = &self.state;
        let caps = &s.caps;
        let t = &self.shell.theme;
        let m = &mut self.shell.model;
        let (ni, no) = (caps.num_inputs as usize, caps.num_outputs as usize);
        m.platform = format!("{:?}", caps.platform).to_uppercase();
        m.firmware = caps.firmware.clone();
        m.serial_short = if caps.serial.len() > 8 {
            caps.serial[caps.serial.len() - 8..].to_string()
        } else {
            caps.serial.clone()
        };
        m.connected = true;
        m.preset_label = match caps.active_preset {
            Some(p) => PresetMenu::slot_label(&self.shared.borrow(), p),
            None => "Empty".into(),
        };
        m.preset_dirty = s.has_unsaved_changes();

        let item =
            |ch: usize, role: ChannelRole, inactive: bool, peaks: &[PeakHold], visible: &[bool]| {
                ChannelItem {
                    name: {
                        let n = s.channel_name(ch);
                        if n.is_empty() {
                            role.descriptor(no as u8)
                        } else {
                            n
                        }
                    },
                    descriptor: role.descriptor(no as u8),
                    role,
                    color: t.role_color(role),
                    level: s.meters.peaks.get(ch).copied().unwrap_or(0.0),
                    peak: peaks.get(ch).map(|p| p.peak).unwrap_or(0.0),
                    clipped: s.is_clipped(ch),
                    visible: visible.get(ch).copied().unwrap_or(true),
                    inactive,
                    index: ch as u8,
                }
            };
        m.inputs = (0..ni)
            .map(|i| {
                item(
                    i,
                    ChannelRole::Input(i as u8),
                    false,
                    &self.peaks,
                    &self.visible,
                )
            })
            .collect();
        m.outputs = (0..no)
            .map(|o| {
                let out = s.output(o);
                let role = ChannelRole::of((ni + o) as u8, ni as u8, no as u8);
                item(
                    ni + o,
                    role,
                    !out.enabled || out.mute,
                    &self.peaks,
                    &self.visible,
                )
            })
            .collect();

        let g = s.global();
        m.strip[1].state = Some(s.crossfeed().enabled);
        m.strip[2].state = Some(g.loudness_enabled);
        m.strip[3].state = Some(s.leveller().enabled);
        m.strip[4].state = Some(s.psybass().enabled);
        m.strip[7].state = Some(g.bypass);

        let source_supported = caps
            .features
            .iter()
            .any(|f| f.name == "spdif_multi_input" && f.present)
            || caps
                .features
                .iter()
                .any(|f| f.name == "i2s_input_channels" && f.present);
        m.source = if source_supported {
            let choices: Vec<String> = vec![
                "USB".into(),
                "S/PDIF".into(),
                "I2S".into(),
                "ADAT".into(),
                "S/PDIF 2".into(),
                "S/PDIF 3".into(),
                "S/PDIF 4".into(),
            ];
            let idx = s
                .input_config()
                .map(|c| c.input_source as usize)
                .unwrap_or(0)
                .min(choices.len() - 1);
            Some((choices, idx))
        } else {
            None
        };
        m.volume_db = match m.volume_mode {
            VolumeMode::User => s.user_volume().0 as f64,
            VolumeMode::Master => s.master_volume_db() as f64,
        };
        m.cpu = (s.meters.cpu0, s.meters.cpu1);

        // Curves: PEQ plus crossover bands, with output gain folded in.
        let selected_index: Option<usize> = match m.selection {
            Selection::Overview => None,
            Selection::Input(i) => Some(i),
            Selection::Output(o) => Some(ni + o),
        };
        let mut curves = Vec::with_capacity(ni + no);
        for ch in 0..ni + no {
            let mut bands: Vec<dsp::Band> = s
                .bands(ch as u8)
                .iter()
                .map(|p| dsp::Band {
                    filter_type: p.filter_type,
                    freq: p.freq,
                    q: p.q,
                    gain_db: p.gain_db,
                    bypass: p.bypass,
                })
                .collect();
            let gain = if ch >= ni {
                bands.extend(s.xover_bands(ch as u8).iter().map(|p| dsp::Band {
                    filter_type: p.filter_type,
                    freq: p.freq,
                    q: p.q,
                    gain_db: p.gain_db,
                    bypass: p.bypass,
                }));
                s.output(ch - ni).gain_db as f64
            } else {
                0.0
            };
            let role = ChannelRole::of(ch as u8, ni as u8, no as u8);
            curves.push(GraphCurve {
                descriptor: role.descriptor(no as u8),
                color: t.role_color(role),
                magnitude: dsp::curve(&bands, gain),
                phase: if selected_index == Some(ch) || m.graph.show_phase {
                    Some(dsp::phase_curve(&bands))
                } else {
                    None
                },
                selected: selected_index == Some(ch),
                visible: self.visible.get(ch).copied().unwrap_or(true),
            });
        }
        m.curves = curves;
    }

    fn note(&mut self, text: impl Into<String>) {
        self.shell.model.status = Some(text.into());
        self.status_until = Some(Instant::now() + Duration::from_secs(3));
    }

    fn echo(&mut self, text: impl Into<String>) {
        self.shell.model.echo = text.into();
    }

    /// Re-read the bulk packet and refresh the model.
    pub fn refresh(&mut self, session: &mut Session) {
        match session.snapshot() {
            Ok(b) => self.state.replace_bulk(b),
            Err(e) => self.note(e.to_string()),
        }
        self.sync_model();
    }

    /// Write one registry parameter, echoing the canonical command.
    pub fn set(&mut self, session: &mut Session, path: &str, indices: &[u8], value: Value) {
        let Some(desc) = dspi_proto::registry::by_path(path) else {
            self.note(format!("unknown parameter {path}"));
            return;
        };
        // Enabling an output goes through the Core 1 interlock, which may
        // need the Console's confirmation before the other side is freed.
        if path == "out.enable"
            && let (Some(&index), Some(enable)) = (indices.first(), value.as_bool())
        {
            self.enable_output(session, index, enable);
            return;
        }
        let cmd = dspi_cmd::Command::Set {
            path: desc.path,
            indices: indices.to_vec(),
            value: value.clone(),
        };
        match session.write(path, indices, value) {
            // A packet parameter has no scalar readback: the confirming read
            // asks for one byte of a structure, so a byte-for-byte comparison
            // is not evidence of anything. Take the device's word for it.
            Ok(Outcome::Rejected { .. })
                if matches!(desc.kind, dspi_proto::registry::Kind::Packet) =>
            {
                self.echo(dspi_cmd::format(&cmd, &self.ctx));
                self.refresh(session);
            }
            Ok(Outcome::Rejected { actual, .. }) => {
                let shown = dspi_proto::registry::by_path(path)
                    .map(|d| crate::fields::display_value(d, &actual))
                    .unwrap_or_default();
                self.note(format!("{path} was not applied; device kept {shown}"));
            }
            Ok(_) => {
                self.echo(dspi_cmd::format(&cmd, &self.ctx));
                self.refresh(session);
            }
            Err(e) => self.note(e.to_string()),
        }
    }

    /// Run one or more commands, one per line.
    ///
    /// A screen asks for several at once when one gesture is several writes:
    /// an edit mirrored onto a linked input pair, Clear All over a whole bank,
    /// a channel paste. The echo line ends up showing the last of them, which
    /// is the one the person's finger was on.
    pub fn run_commands(&mut self, session: &mut Session, lines: &str) {
        for line in lines.lines() {
            if !line.trim().is_empty() {
                self.run_command(session, line);
            }
        }
    }

    fn enable_output(&mut self, session: &mut Session, index: u8, enable: bool) {
        match session.enable_output(index, enable) {
            Ok(dspi_session::EnableOutcome::Done) => {
                self.echo(format!(
                    ":out.enable {} {}",
                    index + 1,
                    if enable { "on" } else { "off" }
                ));
                self.refresh(session);
            }
            Ok(dspi_session::EnableOutcome::NeedsConfirm(c)) => {
                self.dialog = Some((
                    AppDialog::Core1 { index },
                    Dialog::confirm(
                        c.title,
                        c.body,
                        vec![Button::destructive(c.confirm), Button::new("Cancel")],
                    ),
                ));
            }
            Ok(dspi_session::EnableOutcome::Rejected) => {
                self.note("The device kept the output as it was")
            }
            Err(e) => self.note(e.to_string()),
        }
    }

    fn undo(&mut self, session: &mut Session, redo: bool) {
        let result = if redo { session.redo() } else { session.undo() };
        match result {
            Ok(Some(u)) => {
                match u.command {
                    Some(c) => self.echo(format!("{} {c}", if redo { "redo:" } else { "undo:" })),
                    None => self.note("Nothing reversible to undo"),
                }
                if !u.skipped.is_empty() {
                    self.note(format!("Skipped {}", u.skipped.join("; ")));
                }
                self.refresh(session);
            }
            Ok(None) => self.note(if redo {
                "Nothing to redo"
            } else {
                "Nothing to undo"
            }),
            Err(e) => self.note(e.to_string()),
        }
    }

    /// Run a typed command against the device.
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
            } => {
                self.set(session, path, indices, value.clone());
            }
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
                qp,
            } => {
                let packet = dspi_proto::value::EqParamPacket {
                    channel,
                    band,
                    filter_type: dspi_proto::FilterType::from_raw(filter_type),
                    bypass: false,
                    freq,
                    q,
                    gain_db: gain,
                    qp,
                };
                match session.write_band(&packet) {
                    Ok(Outcome::Rejected { .. }) => self.note("the band was not applied as sent"),
                    Ok(_) => {
                        self.echo(dspi_cmd::format(&cmd, &self.ctx));
                        self.refresh(session);
                    }
                    Err(e) => self.note(e.to_string()),
                }
            }
            dspi_cmd::Command::Verb { name, .. } => match name.as_str() {
                "undo" => self.undo(session, false),
                "redo" => self.undo(session, true),
                other => self.note(format!("`{other}` only works from the shell")),
            },
        }
    }

    fn select(&mut self, sel: Selection) {
        self.shell.model.selection = sel;
        self.shell.detail = self.screens.detail(&self.state, sel);
        if let Some(row) = self.shell.model.row_of_selection() {
            self.shell.sidebar_cursor = row;
        }
        self.sync_model();
    }

    /// Ask before losing unsaved changes, in the Console's words.
    fn unsaved_dialog(&self) -> Dialog {
        let diff = self.state.unsaved_diff();
        let summary = dspi_session::PresetSnapshot::summary(&diff, 6);
        Dialog::confirm(
            "Unsaved Changes",
            format!(
                "The current preset has unsaved changes:\n\n{summary}\n\nSave before continuing?"
            ),
            vec![
                Button::new("Save"),
                Button::destructive("Discard"),
                Button::new("Cancel"),
            ],
        )
    }

    fn save_active_preset(&mut self, session: &mut Session) -> bool {
        let Some(slot) = self.state.caps.active_preset else {
            self.note("No active preset slot");
            return false;
        };
        match session.write("preset.save", &[slot], Value::Trigger) {
            Ok(Outcome::Rejected { .. }) | Err(_) => {
                self.note("Save Failed");
                false
            }
            Ok(_) => {
                self.state.mark_saved();
                self.echo(format!(":preset.save {}", slot + 1));
                self.sync_model();
                true
            }
        }
    }

    fn load_preset(&mut self, session: &mut Session, slot: u8) {
        match session.write("preset.load", &[slot], Value::Trigger) {
            Ok(Outcome::Rejected { .. }) | Err(_) => self.note("Load Failed"),
            Ok(_) => {
                self.state.caps.active_preset = Some(slot);
                self.refresh(session);
                self.state.mark_saved();
                self.echo(format!(":preset.load {}", slot + 1));
                self.sync_model();
            }
        }
    }

    fn strip_path(&self, i: usize) -> Option<&'static str> {
        match i {
            1 => Some("cf.on"),
            2 => Some("loud.on"),
            3 => Some("lev.on"),
            4 => Some("bass.on"),
            7 => Some("bypass"),
            _ => None,
        }
    }

    fn strip_tool(&self, i: usize) -> Option<Tool> {
        match i {
            0 => Some(Tool::Matrix),
            1 => Some(Tool::Crossfeed),
            2 => Some(Tool::Loudness),
            3 => Some(Tool::Leveller),
            4 => Some(Tool::Psybass),
            5 => Some(Tool::Stats),
            _ => None,
        }
    }

    /// Turn a shell event into device traffic or a state change.
    pub fn handle_event(&mut self, session: &mut Session, ev: ShellEvent) {
        match ev {
            ShellEvent::Select(sel) => self.select(sel),
            ShellEvent::ToggleVisible(row) => {
                if let Some(v) = self.visible.get_mut(row) {
                    *v = !*v;
                }
                self.sync_model();
            }
            ShellEvent::StripToggle(i) => {
                if let Some(path) = self.strip_path(i) {
                    let on = self.shell.model.strip[i].state.unwrap_or(false);
                    self.set(session, path, &[], Value::Bool(!on));
                }
            }
            ShellEvent::StripOpen(i) => {
                if i == 6 {
                    self.open_settings(session);
                } else if let Some(tool) = self.strip_tool(i) {
                    self.open_tool(tool);
                }
            }
            ShellEvent::OpenTool(tool) => self.open_tool(tool),
            ShellEvent::CloseTool => self.shell.close_tool(),
            ShellEvent::OpenSettings => self.open_settings(session),
            ShellEvent::CloseSettings => self.shell.close_settings(),
            ShellEvent::Preset(Some(delta)) => {
                let cur = self.state.caps.active_preset.unwrap_or(0) as i32;
                let next = (cur + delta).clamp(0, 9) as u8;
                if next as i32 != cur {
                    self.request_preset(session, next);
                }
            }
            ShellEvent::Preset(None) => {
                let active = self.state.caps.active_preset.unwrap_or(0);
                let dirty = self.state.has_unsaved_changes();
                let popup = PresetMenu::popup(&self.shared.borrow(), active, dirty);
                self.popup = Some((AppDialog::PresetList, popup));
            }
            ShellEvent::Source(Some(delta)) => {
                if let Some((choices, idx)) = &self.shell.model.source {
                    let next = (*idx as i32 + delta).clamp(0, choices.len() as i32 - 1) as u8;
                    if next as usize != *idx {
                        self.set(session, "in.source", &[], Value::Choice(next));
                    }
                }
            }
            ShellEvent::Source(None) => {}
            ShellEvent::VolumeChanged(db) => {
                let path = match self.shell.model.volume_mode {
                    VolumeMode::User => "vol.user",
                    VolumeMode::Master => "vol.master",
                };
                self.set(session, path, &[], Value::Float(db as f32));
            }
            ShellEvent::VolumeReset => {
                let path = match self.shell.model.volume_mode {
                    VolumeMode::User => "vol.user",
                    VolumeMode::Master => "vol.master",
                };
                self.set(session, path, &[], Value::Float(0.0));
            }
            ShellEvent::VolumeMute => {
                let muted = self.state.user_volume().1;
                self.set(session, "vol.mute", &[], Value::Bool(!muted));
            }
            ShellEvent::VolumeModeToggle => {
                self.shell.model.volume_mode = match self.shell.model.volume_mode {
                    VolumeMode::User => VolumeMode::Master,
                    VolumeMode::Master => VolumeMode::User,
                };
                self.sync_model();
            }
            ShellEvent::Command(c) => self.run_commands(session, &c),
            ShellEvent::Status(s) => self.note(s),
            ShellEvent::Palette => self.prompt = Some(Prompt::new(true, &self.ctx)),
            ShellEvent::CommandLine => self.prompt = Some(Prompt::new(false, &self.ctx)),
            ShellEvent::SavePreset => {
                let slot = self.state.caps.active_preset.map(|p| p + 1).unwrap_or(1);
                self.dialog = Some((
                    AppDialog::SavePreset,
                    Dialog::confirm(
                        "Save Preset",
                        format!("Save current parameters to preset slot {slot}?"),
                        vec![Button::new("Save"), Button::new("Cancel")],
                    ),
                ));
            }
            ShellEvent::DevicePicker => self.note("One device connected"),
            ShellEvent::Undo => self.undo(session, false),
            ShellEvent::Redo => self.undo(session, true),
            ShellEvent::Quit => {
                if self.state.has_unsaved_changes() {
                    self.dialog = Some((
                        AppDialog::Unsaved {
                            then: PendingAction::Quit,
                        },
                        self.unsaved_dialog(),
                    ));
                } else {
                    self.should_quit = true;
                }
            }
            ShellEvent::ClearClips => {
                self.state.clear_clip_latch();
                let _ = session.write("meters.clear", &[], Value::Trigger);
                self.sync_model();
            }
            ShellEvent::BypassToggle => {
                let on = self.state.global().bypass;
                self.set(session, "bypass", &[], Value::Bool(!on));
            }
            ShellEvent::GraphCursor(hz) => {
                self.shell.model.cursor_hz = hz;
                if let Some(hz) = hz {
                    let m = &self.shell.model;
                    let g = crate::graph::Graph::new(&m.curves, &m.graph, &self.shell.theme);
                    let parts: Vec<String> = g
                        .readout(hz)
                        .iter()
                        .map(|(d, db)| format!("{d} {db:+.1}"))
                        .collect();
                    let text = format!("{:.0} Hz  {}", hz, parts.join("  "));
                    self.shell.model.status = Some(text);
                    self.status_until = Some(Instant::now() + Duration::from_secs(5));
                }
            }
            ShellEvent::GraphZoom(delta) => self.shell.model.graph.zoom(delta),
            ShellEvent::GraphPhase => {
                self.shell.model.graph.show_phase = !self.shell.model.graph.show_phase;
                self.sync_model();
            }
            ShellEvent::GraphHeight => {
                self.shell.model.graph_height = self.shell.model.graph_height.next();
            }
            ShellEvent::GraphPopout => self.shell.graph_popout = !self.shell.graph_popout,
            ShellEvent::Rename(row) => {
                let name = self.state.channel_name(row);
                self.dialog = Some((
                    AppDialog::Rename { channel: row },
                    Dialog::text("Rename", "", name, "Name"),
                ));
            }
            ShellEvent::CopyParams(row) => {
                let clip = clipboard::copy(&self.state, row);
                self.note(clipboard::copied_message(&clip));
                self.shared.borrow_mut().clipboard = Some(clip);
            }
            ShellEvent::PasteParams(row) => {
                let clip = self.shared.borrow().clipboard.clone();
                let Some(clip) = clip else {
                    self.note("Nothing to paste");
                    return;
                };
                // A linked pair mirrors every other edit, so a paste has to
                // mirror too or the two halves drift apart.
                let mirror = self
                    .shared
                    .borrow()
                    .linked_partner(row, self.state.caps.num_inputs as usize);
                let cmds = clipboard::paste_commands(&clip, &self.state, row, mirror);
                self.run_commands(session, &cmds.join("\n"));
                let name = screens::channel_name(&self.state, row);
                self.note(format!("Pasted {} onto {name}", clip.source));
            }
            ShellEvent::Identify(row) => self.identify(session, row),
        }
    }

    /// Play the channel-ID tone on one output, which is the Console's
    /// Identify: a counted blip melody so the listener can tell which speaker
    /// is which. It is only offered while the generator exists.
    fn identify(&mut self, session: &mut Session, row: usize) {
        if !panel::has_feature(&self.state, "test_signals") {
            self.note("Firmware has no signal generator");
            return;
        }
        let ni = self.state.caps.num_inputs as usize;
        let output = row.saturating_sub(ni);
        let name = screens::channel_name(&self.state, row);
        self.run_commands(
            session,
            &format!(
                "sig.config type=channel-id channels=0x{:X} invert=0x0 level=-20 duration=0 \
                 repeat=1 gap=0 flags=walk p1=120\nsig.control start",
                1u16 << output
            ),
        );
        self.note(format!("Identifying {name}"));
    }

    fn request_preset(&mut self, session: &mut Session, slot: u8) {
        if self.state.has_unsaved_changes() {
            self.dialog = Some((
                AppDialog::Unsaved {
                    then: PendingAction::LoadPreset(slot),
                },
                self.unsaved_dialog(),
            ));
        } else {
            self.load_preset(session, slot);
        }
    }

    fn open_tool(&mut self, tool: Tool) {
        let screen = self.screens.tool(&self.state, tool);
        self.shell.open_tool(tool, screen);
    }

    fn open_settings(&mut self, session: &mut Session) {
        self.screens.refresh_settings(session);
        let screen = self.screens.settings(&self.state);
        self.shell.open_settings(screen);
    }

    fn finish_dialog(&mut self, session: &mut Session, kind: AppDialog, outcome: DialogOutcome) {
        match (kind, outcome) {
            (AppDialog::Unsaved { then }, DialogOutcome::Button(0)) => {
                if self.save_active_preset(session) {
                    self.run_pending(session, then);
                }
            }
            (AppDialog::Unsaved { then }, DialogOutcome::Button(1)) => {
                self.run_pending(session, then)
            }
            (AppDialog::SavePreset, DialogOutcome::Button(0)) => {
                self.save_active_preset(session);
            }
            (AppDialog::Rename { channel }, DialogOutcome::Text(name)) => {
                self.set(session, "ch.name", &[channel as u8], Value::Text(name));
                // The detail region names the channel in its title, so it has
                // to be rebuilt for the new name to show.
                let sel = self.shell.model.selection;
                self.shell.detail = self.screens.detail(&self.state, sel);
            }
            (AppDialog::PresetList, DialogOutcome::Picked(i)) => {
                self.preset_action(session, PresetMenu::choice(i))
            }
            (AppDialog::PresetCopyTo(slots), DialogOutcome::Picked(i)) => {
                if let Some(dest) = slots.get(i).copied() {
                    self.copy_preset_to(session, dest);
                }
            }
            (AppDialog::PresetRename(slot), DialogOutcome::Text(name)) => {
                self.set(session, "preset.name", &[slot], Value::Text(name));
                self.refresh_presets(session);
            }
            (AppDialog::PresetClear(slot), DialogOutcome::Button(0)) => {
                self.clear_preset(session, slot);
                self.refresh_presets(session);
            }
            (AppDialog::Core1 { index }, DialogOutcome::Button(0)) => {
                match session.enable_output_confirmed(index) {
                    Ok(dspi_session::EnableOutcome::Done) => {
                        self.echo(format!(":out.enable {} on", index + 1));
                        self.refresh(session);
                    }
                    Ok(_) => self.note("The device kept the output as it was"),
                    Err(e) => self.note(e.to_string()),
                }
            }
            (AppDialog::PresetClearAll, DialogOutcome::Button(0)) => {
                for slot in 0..presets::SLOTS as u8 {
                    self.clear_preset(session, slot);
                }
                self.refresh_presets(session);
            }
            _ => {}
        }
    }

    /// One item of the Preset row's menu.
    fn preset_action(&mut self, session: &mut Session, choice: Option<PresetChoice>) {
        let active = self.state.caps.active_preset.unwrap_or(0);
        match choice {
            Some(PresetChoice::Slot(slot)) => self.request_preset(session, slot),
            Some(PresetChoice::Save) => {
                // The Console names an unnamed slot before saving it, so a
                // saved preset never shows as Empty.
                if PresetMenu::dropdown_label(&self.shared.borrow(), active) == "Empty" {
                    self.set(
                        session,
                        "preset.name",
                        &[active],
                        Value::Text(format!("Preset {}", active + 1)),
                    );
                }
                if self.save_active_preset(session) {
                    self.refresh_presets(session);
                }
            }
            Some(PresetChoice::Rename) => {
                let d = PresetMenu::rename_dialog(&self.shared.borrow(), active);
                self.dialog = Some((AppDialog::PresetRename(active), d));
            }
            Some(PresetChoice::SetDefault) => {
                // `WirePresetStartup`: mode 0 is "a specified slot".
                self.set(
                    session,
                    "preset.startup",
                    &[],
                    Value::Bytes(vec![0, active]),
                );
                self.shared.borrow_mut().default_slot = Some(active);
            }
            Some(PresetChoice::CopyTo) => {
                let (popup, slots) = PresetMenu::copy_to_popup(&self.shared.borrow(), active);
                self.popup = Some((AppDialog::PresetCopyTo(slots), popup));
            }
            Some(PresetChoice::Clear) => {
                let label = PresetMenu::slot_label(&self.shared.borrow(), active);
                self.dialog = Some((
                    AppDialog::PresetClear(active),
                    PresetMenu::clear_dialog(&label),
                ));
            }
            Some(PresetChoice::ClearAll) => {
                self.dialog = Some((AppDialog::PresetClearAll, PresetMenu::clear_all_dialog()));
            }
            None => {}
        }
    }

    /// Copy the live parameters into another slot.
    ///
    /// Saving to the destination makes the firmware treat it as the last
    /// active slot, so the source is re-saved afterwards to put that back.
    /// Data-wise the second write is a no-op.
    fn copy_preset_to(&mut self, session: &mut Session, dest: u8) {
        let source = self.state.caps.active_preset.unwrap_or(0);
        if dest == source {
            return;
        }
        match session.write("preset.save", &[dest], Value::Trigger) {
            Ok(Outcome::Rejected { .. }) | Err(_) => {
                self.note("Save Failed");
                return;
            }
            Ok(_) => {}
        }
        let _ = session.write("preset.save", &[source], Value::Trigger);
        self.refresh_presets(session);
        self.echo(format!(":preset.save {}", dest + 1));
    }

    fn clear_preset(&mut self, session: &mut Session, slot: u8) {
        if session
            .write("preset.delete", &[slot], Value::Trigger)
            .is_err()
        {
            self.note("Clear Failed");
        }
    }

    /// Read the slot names back, which is where the Preset menu's labels come
    /// from. Called on connect and after anything that changes them.
    pub fn refresh_presets(&mut self, session: &mut Session) {
        let names: Vec<String> = (0..presets::SLOTS as u8)
            .map(|slot| match session.read("preset.name", &[slot]) {
                Ok(Value::Text(t)) => t.trim().to_string(),
                _ => String::new(),
            })
            .collect();
        // The directory says which slots hold anything and which one the
        // device loads at power on (config.h REQ_PRESET_GET_DIR, 7 bytes).
        let dir = session
            .with_transport(|t| {
                t.control_in(
                    dspi_proto::generated::opcodes::REQ_PRESET_GET_DIR,
                    0,
                    dspi_proto::packets::PresetDirectory::SIZE as u16,
                )
            })
            .ok()
            .and_then(|d| dspi_proto::packets::PresetDirectory::decode(&d).ok());
        {
            let mut shared = self.shared.borrow_mut();
            shared.preset_names = names;
            if let Some(dir) = dir {
                shared.occupied = dir.occupied;
                shared.default_slot = (dir.startup_mode == 0).then_some(dir.default_slot);
            }
        }
        self.sync_model();
    }

    fn run_pending(&mut self, session: &mut Session, then: PendingAction) {
        match then {
            PendingAction::Quit => self.should_quit = true,
            PendingAction::LoadPreset(slot) => self.load_preset(session, slot),
        }
    }

    /// One key, from the top of the overlay stack down.
    pub fn handle_key(&mut self, session: &mut Session, key: KeyEvent) {
        if let Some(p) = &mut self.prompt {
            if let Some(line) = p.handle(key, &self.ctx) {
                self.prompt = None;
                if !line.trim().is_empty() {
                    self.run_command(session, &line);
                }
            }
            return;
        }
        if let Some((kind, d)) = &mut self.dialog {
            if let Some(outcome) = d.handle(key) {
                let kind = kind.clone();
                self.dialog = None;
                self.finish_dialog(session, kind, outcome);
            }
            return;
        }
        if let Some((kind, p)) = &mut self.popup {
            match p.handle(key) {
                Some(crate::widgets::Action::Selected(i)) => {
                    let kind = kind.clone();
                    self.popup = None;
                    self.finish_dialog(session, kind, DialogOutcome::Picked(i));
                }
                Some(crate::widgets::Action::Closed) => self.popup = None,
                _ => {}
            }
            return;
        }
        let events = self.shell.handle(key, &self.state);
        for ev in events {
            self.handle_event(session, ev);
        }
    }

    /// Refresh the upmixer's telemetry, at most once a second and only while
    /// its panel is the one on screen.
    ///
    /// `REQ_UPMIX_GET_STATUS` is a 16-byte structure with no scalar readback,
    /// so it goes through the transport rather than the registry, the way the
    /// preset directory does.
    fn poll_upmix_status(&mut self, session: &mut Session, now: Instant) {
        let showing = matches!(self.shell.tool, Some((Tool::Upmixer, _)));
        if !showing || !panel::has_feature(&self.state, "upmixer") {
            return;
        }
        if now.duration_since(self.last_upmix_poll) < Duration::from_secs(1) {
            return;
        }
        self.last_upmix_poll = now;
        let read = session.with_transport(|t| {
            t.control_in(
                dspi_proto::generated::opcodes::REQ_UPMIX_GET_STATUS,
                0,
                dspi_proto::packets::UpmixStatus::SIZE as u16,
            )
        });
        if let Ok(bytes) = read
            && let Ok(status) = dspi_proto::packets::UpmixStatus::decode(&bytes)
        {
            self.state.upmix_status = Some(status);
        }
    }

    /// Meters, notifications, status expiry, peak ballistics.
    pub fn tick(&mut self, session: &mut Session, notifications: Option<&Notifications>) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f32();
        self.last_tick = now;

        if let Ok(m) = session.meters() {
            for (i, p) in self.peaks.iter_mut().enumerate() {
                p.update(m.peaks.get(i).copied().unwrap_or(0.0), dt);
            }
            self.state.update_meters(m);
        }
        self.poll_upmix_status(session, now);

        let mut reread = false;
        let mut changed_elsewhere: Option<(&'static str, Source)> = None;
        if let Some(n) = notifications {
            for note in n.drain() {
                match self.state.apply(&note) {
                    Applied::Section { name, source } => {
                        if !source.is_ours() {
                            changed_elsewhere = Some((name, source));
                        }
                    }
                    Applied::NeedsReread { .. } => reread = true,
                    Applied::PresetLoaded { slot } => {
                        self.state.caps.active_preset = Some(slot);
                        reread = true;
                        self.note(format!("Preset {} loaded", slot + 1));
                    }
                    Applied::InputFormat { channels } => {
                        self.note(format!("{channels} input channels active"));
                    }
                    Applied::Status(_) | Applied::Nothing => {}
                }
            }
            if n.is_disconnected() {
                self.shell.model.connected = false;
            }
        }
        if reread {
            self.refresh(session);
        }
        if let Some((name, source)) = changed_elsewhere {
            self.note(format!("{} {}", name.replace('_', " "), source.describe()));
        }
        if let Some(until) = self.status_until
            && now >= until
        {
            self.shell.model.status = None;
            self.status_until = None;
        }
        self.sync_model();
    }

    pub fn draw(&mut self, area: Rect, buf: &mut Buffer) {
        self.shell.draw(area, buf, &self.state);
        let theme = self.shell.theme.clone();
        if let Some((_, p)) = &self.popup {
            let (w, h) = p.size(area.width.saturating_sub(4), area.height.saturating_sub(4));
            let r = Rect::new(
                area.x + (area.width - w) / 2,
                area.y + (area.height - h) / 2,
                w,
                h,
            );
            p.draw(r, buf, &theme);
        }
        if let Some((_, d)) = &self.dialog {
            let r = d.size(area);
            d.draw(r, buf, &theme);
        }
        if let Some(p) = &self.prompt {
            p.draw(area, buf, &theme);
        }
    }
}

/// Run the live interface until the person quits.
pub fn run(mut live: Live, session: &mut Session) -> io::Result<()> {
    let notifications = session
        .with_transport(|t| Ok(t.notifications()))
        .ok()
        .flatten()
        .map(Notifications::start);
    let perf = live.perf;
    live.refresh_presets(session);
    let mut terminal = ratatui::init();
    let mut last_poll = Instant::now() - perf.meter_interval;

    let result = (|| -> io::Result<()> {
        loop {
            if last_poll.elapsed() >= perf.meter_interval {
                live.tick(session, notifications.as_ref());
                last_poll = Instant::now();
            }
            terminal.draw(|f| {
                let area = f.area();
                live.draw(area, f.buffer_mut());
            })?;
            if event::poll(perf.event_timeout)?
                && let TermEvent::Key(key) = event::read()?
                && key.kind == event::KeyEventKind::Press
            {
                // Ctrl-C always leaves, even with a dialog up.
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    live.should_quit = true;
                }
                live.handle_key(session, key);
            }
            if live.should_quit {
                return Ok(());
            }
        }
    })();
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ColorDepth, Glyphs};
    use crate::widgets::testing::key;
    use dspi_proto::generated::opcodes as op;
    use dspi_session::Capabilities;
    use dspi_transport::mock::LogHandle;
    use dspi_transport::{MockTransport, Transport};

    fn packet() -> Vec<u8> {
        crate::shell::fixture::packet()
    }

    fn caps() -> Capabilities {
        crate::shell::fixture::caps()
    }

    fn live() -> (Live, Session, LogHandle) {
        let mock = MockTransport::new()
            .window(op::REQ_GET_ALL_PARAMS_CHUNK, packet())
            // The readback after a nudge from -12.0 dB must agree with the write.
            .data(op::REQ_GET_USER_VOLUME, (-12.5f32).to_le_bytes().to_vec())
            .data(op::REQ_GET_STATUS, vec![0; 41]);
        let log = mock.log_handle();
        let session = Session::new(Box::new(mock), caps()).unwrap();
        let state = DeviceState::new(
            caps(),
            dspi_proto::wire::BulkPacket::decode(packet()).unwrap(),
        );
        let theme = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let live = Live::new(
            state,
            theme,
            Performance::default(),
            Box::new(PlaceholderScreens),
        );
        (live, session, log)
    }

    #[test]
    fn the_model_is_projected_from_the_device_state() {
        let (l, _, _) = live();
        let m = &l.shell.model;
        assert_eq!(m.inputs.len(), 8);
        assert_eq!(m.outputs.len(), 9);
        assert_eq!(m.inputs[0].name, "FL");
        assert_eq!(m.inputs[1].name, "FR");
        assert_eq!(m.outputs[8].descriptor, "OUT9");
        // The names have not been read back yet, so the slot reads as Empty.
        assert_eq!(m.preset_label, "3: Empty");
        assert!(!m.preset_dirty);
        assert_eq!(m.serial_short, "1B8B4E3A");
        assert_eq!(m.curves.len(), 17);
        // An unnamed channel shows its descriptor instead of a blank.
        let (mut l, _, _) = live();
        let (_, n, _) = dspi_proto::generated::SECTIONS[9];
        l.state.bulk.patch(n + 32, &[0; 32]);
        l.sync_model();
        assert_eq!(l.shell.model.inputs[1].name, "IN2");
    }

    /// The same runner, wired to the Console's screens rather than the
    /// placeholders.
    fn console() -> (Live, Session, LogHandle) {
        let mock = MockTransport::new()
            .window(op::REQ_GET_ALL_PARAMS_CHUNK, packet())
            .data(op::REQ_GET_USER_VOLUME, (-12.5f32).to_le_bytes().to_vec())
            .data(op::REQ_GET_STATUS, vec![0; 41]);
        let log = mock.log_handle();
        let session = Session::new(Box::new(mock), caps()).unwrap();
        let state = DeviceState::new(
            caps(),
            dspi_proto::wire::BulkPacket::decode(packet()).unwrap(),
        );
        let theme = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let live = Live::new(
            state,
            theme,
            Performance::default(),
            Box::new(ConsoleScreens::new()),
        );
        (live, session, log)
    }

    #[test]
    fn the_console_screens_are_the_default_detail_region() {
        let (mut l, mut s, _) = console();
        // The overview is what the shell opens on.
        assert_eq!(l.shell.detail.title(), "Overview");
        l.handle_event(&mut s, ShellEvent::Select(Selection::Input(0)));
        assert!(
            l.shell.detail.keys().iter().any(|k| k.does == "Enable All"),
            "the input page carries the filter list's actions"
        );
        l.handle_event(&mut s, ShellEvent::Select(Selection::Output(0)));
        assert!(l.shell.detail.keys().iter().any(|k| k.key == "x"));
    }

    #[test]
    fn the_preset_row_opens_the_consoles_whole_menu() {
        let (mut l, mut s, _) = console();
        l.shell.focus = crate::shell::Focus::Footer(crate::shell::FooterRow::Preset);
        l.handle_key(&mut s, key(KeyCode::Enter));
        let (_, popup) = l.popup.as_ref().expect("the preset popup");
        assert_eq!(popup.items[0], "1: Empty");
        assert_eq!(popup.items[11], "Save");
        assert_eq!(popup.items[16], "Clear All Slots...");
        // Picking an action rather than a slot opens its own step.
        l.handle_key(&mut s, key(KeyCode::End));
        l.handle_key(&mut s, key(KeyCode::Enter));
        assert!(matches!(l.dialog, Some((AppDialog::PresetClearAll, _))));
    }

    #[test]
    fn copy_and_paste_carry_a_channels_parameters() {
        let (mut l, mut s, log) = console();
        l.handle_event(&mut s, ShellEvent::CopyParams(0));
        assert!(l.shared.borrow().clipboard.is_some());
        assert_eq!(
            l.shell.model.status.as_deref(),
            Some("Copied FL parameters")
        );
        l.handle_event(&mut s, ShellEvent::PasteParams(1));
        let writes = log
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.opcode == op::REQ_SET_EQ_PARAM)
            .count();
        assert_eq!(writes, 10, "every band travels");
        assert!(
            l.shell
                .model
                .status
                .as_deref()
                .is_some_and(|s| s.starts_with("Pasted FL onto")),
            "{:?}",
            l.shell.model.status
        );
    }

    #[test]
    fn nothing_to_paste_says_so() {
        let (mut l, mut s, _) = console();
        l.handle_event(&mut s, ShellEvent::PasteParams(1));
        assert_eq!(l.shell.model.status.as_deref(), Some("Nothing to paste"));
    }

    /// A screen asks for several writes at once when one gesture is several
    /// writes: a mirrored edit, a Clear All, a paste.
    #[test]
    fn a_multi_line_command_runs_every_line() {
        let (mut l, mut s, log) = console();
        l.handle_event(
            &mut s,
            ShellEvent::Command("eq.bypass in.1 2 on\neq.bypass in.2 2 on".into()),
        );
        let writes = log
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.opcode == op::REQ_SET_BAND_BYPASS)
            .count();
        assert_eq!(writes, 2);
        assert_eq!(l.shell.model.echo, "eq.bypass in.2 2 on");
    }

    #[test]
    fn a_volume_nudge_writes_the_user_volume() {
        let (mut l, mut s, log) = live();
        l.shell.focus = crate::shell::Focus::Footer(crate::shell::FooterRow::Volume);
        l.handle_key(&mut s, key(KeyCode::Left));
        let wrote = log
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.opcode == op::REQ_SET_USER_VOLUME);
        assert!(wrote, "expected a SET_USER_VOLUME on the wire");
        assert!(
            l.shell.model.echo.contains("vol.user"),
            "{}",
            l.shell.model.echo
        );
    }

    #[test]
    fn the_command_line_runs_the_shared_grammar() {
        let (mut l, mut s, log) = live();
        l.handle_key(&mut s, key(KeyCode::Char(':')));
        assert!(l.prompt.is_some());
        for c in "bypass on".chars() {
            l.handle_key(&mut s, key(KeyCode::Char(c)));
        }
        l.handle_key(&mut s, key(KeyCode::Enter));
        assert!(l.prompt.is_none());
        assert!(
            log.lock()
                .unwrap()
                .iter()
                .any(|e| e.opcode == op::REQ_SET_BYPASS)
        );
    }

    #[test]
    fn quitting_with_unsaved_changes_asks_first() {
        let (mut l, mut s, _) = live();
        let (_, g, _) = dspi_proto::generated::SECTIONS[1];
        l.state.bulk.patch(g, &(-6.0f32).to_le_bytes());
        l.sync_model();
        assert!(l.shell.model.preset_dirty);
        l.handle_key(&mut s, key(KeyCode::Char('q')));
        assert!(!l.should_quit);
        assert!(matches!(l.dialog, Some((AppDialog::Unsaved { .. }, _))));
        l.handle_key(&mut s, key(KeyCode::Char('d')));
        assert!(l.should_quit, "Discard quits");
    }

    #[test]
    fn a_notification_from_elsewhere_lands_in_the_model_and_the_echo_line() {
        let (mut l, mut s, _) = live();
        let mock = MockTransport::new();
        let (_, u, _) = dspi_proto::generated::SECTIONS[16];
        let mut p = vec![2, 2, 0, 1];
        p.extend((u as u16).to_le_bytes());
        p.extend(4u16.to_le_bytes());
        p.extend([7, 0, 0, 0]);
        p.extend((-30.0f32).to_le_bytes());
        mock.push_notification(p);
        let n = Notifications::start(mock.notifications().unwrap());
        std::thread::sleep(Duration::from_millis(50));
        l.tick(&mut s, Some(&n));
        assert_eq!(l.state.user_volume().0, -30.0);
        assert_eq!(l.shell.model.volume_db, -30.0);
        assert_eq!(
            l.shell.model.status.as_deref(),
            Some("user volume changed by the system volume")
        );
    }
}
