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

use crate::actions;
use crate::graph::GraphCurve;
use crate::perf::Performance;
use crate::screens::{
    self, AutoEqPanel, CrossfeedPanel, InputPage, LevellerPanel, LoudnessPanel, MatrixPanel,
    MonitorPanel, OutputPage, Overview, PresetChoice, PresetMenu, PsybassPanel, Shared,
    SignalsPanel, SpectrumPanel, StatsPanel, UpmixerPanel, clipboard, panel, presets,
};
use crate::settings::{AppConfig, SettingsData, SettingsScreen};
use crate::shell::{
    ChannelItem, Placeholder, Screen, Selection, Shell, ShellEvent, ShellModel, Tool, VolumeMode,
};
use crate::theme::{ChannelRole, Glyphs, Theme};
use crate::widgets::text::truncate;
use crate::widgets::{Button, Dialog, DialogOutcome, PeakHold, PopupList};

/// The connect animation's length.
const REVEAL: Duration = Duration::from_millis(400);
/// How long a remotely changed value takes to reach its new position.
const EASE: Duration = Duration::from_millis(120);
/// The Console clears a clip latch this long after it lit.
const CLIP_HOLD: Duration = Duration::from_secs(3);

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
        let config = AppConfig::load();
        let shared = screens::shared();
        // The analyser's settings live with the engine, so a change on the
        // Settings page reaches it without a restart.
        shared
            .borrow_mut()
            .spectrum
            .adopt_settings(&config.spectrum);
        Self {
            shared,
            settings: std::rc::Rc::new(std::cell::RefCell::new(SettingsData::default())),
            config,
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

    fn tool(&self, state: &DeviceState, tool: Tool) -> Box<dyn Screen> {
        match tool {
            Tool::Matrix => Box::new(MatrixPanel::new(self.shared.clone())),
            Tool::Crossfeed => Box::new(CrossfeedPanel::new()),
            Tool::Loudness => Box::new(LoudnessPanel::new()),
            Tool::Leveller => Box::new(LevellerPanel::new()),
            Tool::Psybass => Box::new(PsybassPanel::new()),
            Tool::Upmixer => Box::new(UpmixerPanel::new()),
            Tool::Signals => Box::new(SignalsPanel::new()),
            Tool::Stats => Box::new(StatsPanel::new(self.shared.clone())),
            Tool::Monitor => Box::new(MonitorPanel::new(self.shared.clone())),
            Tool::AutoEq => Box::new(AutoEqPanel::new(self.shared.clone())),
            Tool::Spectrum => Box::new(SpectrumPanel::open(self.shared.clone(), state)),
        }
    }

    fn settings(&self, state: &DeviceState) -> Box<dyn Screen> {
        let mut config = self.config.clone();
        config.spectrum = self.shared.borrow().spectrum.settings.clone();
        let shared = self.shared.clone();
        Box::new(
            SettingsScreen::new(state, self.settings.borrow().clone(), config).on_config(
                move |c: &AppConfig| shared.borrow_mut().spectrum.adopt_settings(&c.spectrum),
            ),
        )
    }

    fn shared(&self) -> Option<Shared> {
        Some(self.shared.clone())
    }

    fn config(&self) -> AppConfig {
        self.config.clone()
    }

    fn refresh_settings(&self, session: &mut Session) {
        let mut data = SettingsData::read(session);
        data.rta = self.shared.borrow().spectrum.engine.caps().copied();
        *self.settings.borrow_mut() = data;
    }
}

/// The verbs the interface answers itself: the Console's File, Tools and
/// AutoEQ menus.
///
/// They are not parameters, so they are not in the shared grammar and a shell
/// one-shot cannot run them; they need an interface with dialogs in it. They
/// are still typed and completed like everything else, because a person who
/// has learned `:` should not have to learn a second way to reach half of what
/// the program does.
pub const APP_VERBS: &[(&str, &str)] = &[
    ("import", "Import Filters..."),
    ("export", "Export Filters..."),
    ("import-config", "Import Device Configuration..."),
    ("export-config", "Export Device Configuration..."),
    ("autoeq", "AutoEQ: browse profiles, or `autoeq update`"),
    ("save-master", "Save Master Volume"),
    ("save-output-config", "Save Output Configuration"),
    ("commit", "Commit Parameters..."),
    ("revert", "Revert to Saved..."),
    ("factory-reset", "Factory Reset..."),
    ("bootloader", "Firmware Update..."),
    ("device", "Device picker"),
    ("reconnect", "Reconnect to the device"),
    ("clear-favourites", "AutoEQ: clear favourites"),
];

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
        // The interface's own verbs are offered alongside the grammar's, so
        // the palette is the whole program rather than the half of it that is
        // parameters.
        if tokens.is_empty() {
            let mine: Vec<Candidate> = APP_VERBS
                .iter()
                .filter(|(name, _)| name.starts_with(partial))
                .map(|(name, detail)| Candidate {
                    value: (*name).to_string(),
                    detail: (*detail).to_string(),
                    kind: dspi_cmd::complete::CandidateKind::Verb,
                })
                .collect();
            self.candidates.splice(0..0, mine);
        }
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
    Rename {
        channel: usize,
    },
    PresetList(Vec<Option<PresetChoice>>),
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
    /// A command that cannot be undone, typed on the `:` line.
    Hazard {
        path: String,
        indices: Vec<u8>,
        value: Value,
    },
    /// The Source row's list.
    SourceList,
    /// A path to read or write, by what it is for.
    Path(FileAction),
    /// Which channels an import lands on, and what each row stands for.
    ImportChannels {
        file: Box<dspi_session::filterfile::FilterFile>,
        targets: Vec<actions::ImportTarget>,
    },
    /// The `.dspipreset` options checklist, holding the document it describes.
    ConfigOptions(Box<dspi_session::preset_file::PresetDocument>),
    /// A report with nothing left to decide.
    Report,
    Commit,
    Revert,
    FactoryReset,
    Bootloader,
    /// The wait after the reboot; it closes itself.
    BootWait,
    /// Which device to talk to, with the serial each row stands for.
    DevicePicker(Vec<String>),
    /// The AutoEQ Update Database menu, and the confirm in front of the
    /// rebuild.
    AutoEqUpdate,
    AutoEqRebuild,
    AutoEqRebuildProgress,
}

/// Which file action a path dialog is collecting a path for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileAction {
    ImportFilters,
    ExportFilters,
    ImportConfig,
    ExportConfig,
    ImportAutoEqDatabase,
}

#[derive(Debug, Clone, PartialEq)]
enum PendingAction {
    Quit,
    LoadPreset(u8),
    /// Copy the active preset to another slot once the live state is clean.
    CopyTo(u8),
    /// Open another device, by serial.
    SwitchDevice(String),
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
    /// Whether a linked partner's curve is drawn under the selected one;
    /// `.` toggles it (DESIGN 12.2).
    partner_shown: bool,
    screens: Box<dyn Screens>,
    /// The state the screens share, when the factory keeps one.
    shared: Shared,
    pub should_quit: bool,
    last_tick: Instant,
    /// The volume the slider is drawn at, easing toward the device's value
    /// when someone else moved it.
    shown_volume: Option<f64>,
    ease_from: Option<(Instant, f64)>,
    /// When the clip latch was first set, for the Console's auto-clear.
    clip_since: Option<Instant>,
    /// The notification reader, re-armed on every device switch.
    notes: Option<Notifications>,
    last_screen_poll: Instant,
    /// A hazardous command has been confirmed and may go through once.
    hazard_confirmed: bool,
    /// The channel the pop-out graph is pinned to when Graphing says it does
    /// not follow the selection.
    popout_pinned: Option<usize>,
    devices_checked: Option<Instant>,
    /// When the upmixer's telemetry was last read. It is not a notification,
    /// so the only way to move the gauges is to ask, and once a second is
    /// enough for a meter a person is watching.
    last_upmix_poll: Instant,
    /// When the Stats panel's diagnostics were last read, on the Console's own
    /// two-second cadence.
    last_stats_poll: Instant,
    /// When the notification log started, which is what its time column counts
    /// from.
    started: Instant,
    /// The bootloader handoff, while it is running.
    boot: Option<actions::BootloaderWatch>,
    /// An AutoEQ rebuild, while it is running.
    rebuild: Option<dspi_session::autoeq::RebuildHandle>,
    /// The serial of a device to open in place of this one. The event loop
    /// owns the transport, so the picker asks and the loop does it.
    pub switch_to: Option<String>,
    /// A key is waiting: the loop sets this before a tick so the analyser
    /// sits that tick out and the key's write goes first.
    pub input_pending: bool,
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
            partner_shown: true,
            screens,
            shared,
            should_quit: false,
            last_tick: Instant::now(),
            shown_volume: None,
            ease_from: None,
            clip_since: None,
            notes: None,
            last_screen_poll: Instant::now(),
            hazard_confirmed: false,
            popout_pinned: None,
            devices_checked: None,
            last_upmix_poll: Instant::now() - Duration::from_secs(2),
            last_stats_poll: Instant::now() - Duration::from_secs(3),
            started: Instant::now(),
            boot: None,
            rebuild: None,
            switch_to: None,
            input_pending: false,
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
        m.connected = s.connected;
        m.preset_label = match caps.active_preset {
            Some(p) => PresetMenu::slot_label(&self.shared.borrow(), p),
            None => "Empty".into(),
        };
        m.preset_dirty = s.has_unsaved_changes();

        let item = |ch: usize, role: ChannelRole, inactive: bool, peaks: &[PeakHold]| ChannelItem {
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
            inactive,
            index: ch as u8,
        };
        let active_inputs = s.meters.active_inputs as usize;
        m.inputs = (0..ni)
            .map(|i| {
                item(
                    i,
                    ChannelRole::Input(i as u8),
                    active_inputs > 0 && i >= active_inputs,
                    &self.peaks,
                )
            })
            .collect();
        m.outputs = (0..no)
            .map(|o| {
                let out = s.output(o);
                let role = ChannelRole::of((ni + o) as u8, ni as u8, no as u8);
                item(ni + o, role, !out.enabled || out.mute, &self.peaks)
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
        let target = match m.volume_mode {
            VolumeMode::User => s.user_volume().0 as f64,
            VolumeMode::Master => s.master_volume_db() as f64,
        };
        // A value the device changed eases to its new place, so a knob turn
        // reads as motion rather than a jump. Our own writes snap.
        m.volume_db = match (self.ease_from, self.perf.animate) {
            (Some((since, from)), true) => {
                let t = (since.elapsed().as_secs_f64() / EASE.as_secs_f64()).min(1.0);
                if t >= 1.0 {
                    self.ease_from = None;
                }
                from + (target - from) * t
            }
            _ => target,
        };
        self.shown_volume = Some(target);
        m.cpu = (s.meters.cpu0, s.meters.cpu1);

        // Curves: the graphed channel's PEQ plus crossover bands, with output
        // gain folded in, and its linked partner's underneath in grey.
        let selected_index: Option<usize> = match m.selection {
            Selection::Overview => None,
            Selection::Input(i) => Some(i),
            Selection::Output(o) => Some(ni + o),
        };
        let graphed = self.popout_pinned.or(selected_index);
        let partner = graphed
            .filter(|_| self.partner_shown)
            .and_then(|ch| self.shared.borrow().linked_partner(ch, ni));
        let mut curves = Vec::with_capacity(2);
        for ch in partner.into_iter().chain(graphed) {
            let principal = Some(ch) == graphed;
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
                color: if principal { t.role_color(role) } else { t.dim },
                magnitude: reveal(dsp::curve(&bands, gain), self.started, self.perf.animate),
                phase: if principal && m.graph.show_phase {
                    Some(dsp::phase_curve(&bands))
                } else {
                    None
                },
                selected: principal,
            });
        }
        m.curves = curves;
        m.graph_channel = graphed.map(|ch| screens::channel_name(s, ch));
        self.shared.borrow_mut().graph = m.graph.clone();
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
        // A command that cannot be undone is confirmed first, the way every
        // destructive button in the Console is.
        if matches!(
            desc.hazard,
            dspi_proto::registry::Hazard::Irreversible | dspi_proto::registry::Hazard::Flash
        ) && !self.hazard_confirmed
        {
            self.dialog = Some((
                AppDialog::Hazard {
                    path: path.to_string(),
                    indices: indices.to_vec(),
                    value: value.clone(),
                },
                Dialog::confirm(
                    "Are you sure?",
                    format!("`{path}`: {}. This cannot be undone.", desc.plain),
                    vec![Button::destructive("Continue"), Button::new("Cancel")],
                ),
            ));
            return;
        }
        self.hazard_confirmed = false;
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
                    .map(|d| display_value(d, &actual))
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
    /// A `#` line is not a command but the note to leave on the echo line once
    /// the block has run, which is how a screen that issues a hundred writes
    /// for one gesture says what it just did.
    pub fn run_commands(&mut self, session: &mut Session, lines: &str) {
        let mut note = None;
        for line in lines.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match line.strip_prefix('#') {
                Some(text) => note = Some(text.trim().to_string()),
                None => self.run_command(session, line),
            }
        }
        if let Some(note) = note {
            self.note(note);
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
        if let Some(first) = tokens.first()
            && APP_VERBS.iter().any(|(n, _)| n == first)
        {
            let args: Vec<&str> = tokens[1..].iter().map(String::as_str).collect();
            self.app_verb(session, first, &args);
            return;
        }
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
                        .map(|d| display_value(d, &v))
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
                "reconnect" => {
                    // The Console's right-click on the device name.
                    self.switch_to = Some(self.state.caps.serial.clone());
                }
                "clear-favourites" => match dspi_session::autoeq::save_favourites(&[]) {
                    Ok(()) => self.note("Favourites cleared"),
                    Err(e) => self.note(e.to_string()),
                },
                other => self.note(format!("`{other}` only works from the shell")),
            },
        }
    }

    /// One of the interface's own verbs: the Console's File, Tools and AutoEQ
    /// menus.
    ///
    /// Each one either acts at once or raises the first dialog of its flow;
    /// the rest of each flow is in `finish_dialog`, so a whole flow reads in
    /// one place rather than being spread through the key handler.
    fn app_verb(&mut self, session: &mut Session, verb: &str, args: &[&str]) {
        match verb {
            "import" => self.ask_path(session, FileAction::ImportFilters, args),
            "export" => self.ask_path(session, FileAction::ExportFilters, args),
            "import-config" => self.ask_path(session, FileAction::ImportConfig, args),
            "export-config" => self.ask_path(session, FileAction::ExportConfig, args),
            "commit" => {
                let slot = self.state.caps.active_preset.unwrap_or(0);
                self.dialog = Some((AppDialog::Commit, actions::commit_dialog(slot)));
            }
            "revert" => self.dialog = Some((AppDialog::Revert, actions::revert_dialog())),
            "factory-reset" => {
                self.dialog = Some((AppDialog::FactoryReset, actions::factory_reset_dialog()))
            }
            "bootloader" => self.dialog = Some((AppDialog::Bootloader, actions::firmware_dialog())),
            "save-master" => match actions::save_master_volume(session, &self.state) {
                Ok(m) | Err(m) => self.note(m),
            },
            "save-output-config" => match actions::save_output_config(session) {
                Ok(m) | Err(m) => self.note(m),
            },
            "device" => self.open_device_picker(),
            "autoeq" => match args.first().copied() {
                Some("update") => self.autoeq_update(),
                _ => self.open_tool(Tool::AutoEq),
            },
            other => self.note(format!("`{other}` is not one of this interface's verbs")),
        }
    }

    /// Raise a path dialog, or act at once when the path came with the verb:
    /// `:export tuning.txt` should not stop to ask.
    fn ask_path(&mut self, session: &mut Session, action: FileAction, args: &[&str]) {
        if let Some(path) = args.first().filter(|p| !p.is_empty()) {
            self.run_file_action(session, action, path);
            return;
        }
        let (title, body, default, button) = match action {
            FileAction::ImportFilters => (
                "Import Filters",
                "A REW, DSPi or DSPi for Windows filter file.",
                String::new(),
                "Import",
            ),
            FileAction::ExportFilters => (
                "Export Filters",
                "Where to write this device's filters.",
                actions::DEFAULT_FILTER_NAME.to_string(),
                "Export",
            ),
            FileAction::ImportConfig => (
                "Import Device Configuration",
                "A .dspipreset document.",
                String::new(),
                "Import",
            ),
            FileAction::ExportConfig => (
                "Export Device Configuration",
                "Where to write this device's whole configuration.",
                actions::DEFAULT_CONFIG_NAME.to_string(),
                "Export",
            ),
            FileAction::ImportAutoEqDatabase => (
                "Import AutoEQ Database",
                "Select an autoeq_database.json file.",
                String::new(),
                "Import",
            ),
        };
        self.dialog = Some((
            AppDialog::Path(action),
            actions::path_dialog(title, body, &default, button),
        ));
    }

    /// A path has been settled on: read it, write it, or move to the step that
    /// needs to ask something else.
    fn run_file_action(&mut self, session: &mut Session, action: FileAction, path: &str) {
        match action {
            FileAction::ExportFilters => match actions::export_filters(&self.state, path) {
                Ok(m) => self.note(m),
                Err(e) => self.note(e),
            },
            FileAction::ExportConfig => match actions::export_config(session, path) {
                Ok(m) => self.note(m),
                Err(e) => self.note(e),
            },
            FileAction::ImportFilters => {
                let text = match std::fs::read_to_string(actions::expand(path)) {
                    Ok(t) => t,
                    Err(e) => return self.note(format!("Failed to read file: {e}")),
                };
                match dspi_session::filterfile::parse(&text) {
                    Ok(file) => {
                        let (dialog, targets) = actions::channel_picker(&file, &self.state);
                        if targets.is_empty() {
                            return self.note("No valid filters found in file");
                        }
                        self.dialog = Some((
                            AppDialog::ImportChannels {
                                file: Box::new(file),
                                targets,
                            },
                            dialog,
                        ));
                    }
                    Err(_) => self.note("Failed to parse DSPi Console filter file"),
                }
            }
            FileAction::ImportConfig => match actions::read_config(path) {
                Ok(doc) => {
                    let dialog = actions::import_options_dialog(&doc, &self.state);
                    self.dialog = Some((AppDialog::ConfigOptions(Box::new(doc)), dialog));
                }
                Err(e) => self.note(e),
            },
            FileAction::ImportAutoEqDatabase => {
                match dspi_session::autoeq::import_database(&actions::expand(path)) {
                    Ok(n) => {
                        self.reload_autoeq();
                        self.note(format!("Database updated successfully.  Entries: {n}"));
                    }
                    Err(e) => self.note(format!("Failed to import database: {e}")),
                }
            }
        }
    }

    /// Throw away the cached database so the next look at it re-reads.
    fn reload_autoeq(&mut self) {
        let mut shared = self.shared.borrow_mut();
        shared.autoeq = None;
        shared.favourites = dspi_session::autoeq::load_favourites();
    }

    fn open_device_picker(&mut self) {
        let devices = actions::device_list();
        if devices.len() < 2 {
            self.note(if devices.is_empty() {
                "No devices found"
            } else {
                "One device connected"
            });
            return;
        }
        let serials: Vec<String> = devices.iter().map(|d| d.serial.clone()).collect();
        let items: Vec<String> = devices
            .iter()
            .map(|d| format!("{}  {}", d.short_name(), d.bus_id))
            .collect();
        let here = self.state.caps.serial.clone();
        let cursor = serials.iter().position(|s| *s == here).unwrap_or(0);
        self.dialog = Some((
            AppDialog::DevicePicker(serials),
            Dialog::list("Device", "", items, cursor),
        ));
    }

    /// The Console's Update Database menu, with its three methods.
    fn autoeq_update(&mut self) {
        let db = self.shared.borrow().autoeq.clone();
        let (count, date) = match &db {
            Some(d) => (
                d.entries.len(),
                if d.generated_at.is_empty() {
                    "Unknown".to_string()
                } else {
                    d.generated_at.clone()
                },
            ),
            None => (0, "Unknown".to_string()),
        };
        let mut buttons = vec![
            Button::new("Rebuild from GitHub"),
            Button::new("Import File..."),
        ];
        // Only offered once there is a user copy to throw away, as the
        // Console's menu does.
        if dspi_session::autoeq::has_user_database() {
            buttons.push(Button::new("Reset to Built-in"));
        }
        buttons.push(Button::new("Cancel"));
        self.dialog = Some((
            AppDialog::AutoEqUpdate,
            Dialog::confirm(
                "Update AutoEQ Database",
                format!("Current database: {date}\nEntries: {count}\n\nChoose an update method:"),
                buttons,
            ),
        ));
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
        // The status byte tells a corrupt slot from a refused one; the
        // registry's trigger path keeps only success.
        let status = session.with_transport(|t| {
            t.control_in(
                dspi_proto::generated::opcodes::REQ_PRESET_LOAD,
                slot as u16,
                1,
            )
        });
        match status.map(|b| b.first().copied().unwrap_or(0)) {
            // PRESET_ERR_CRC (config.h): the Console's own words.
            Ok(3) => self.note("Load Failed: Preset data is corrupted."),
            Ok(0) => {
                self.state.caps.active_preset = Some(slot);
                self.refresh(session);
                self.state.mark_saved();
                self.echo(format!(":preset.load {}", slot + 1));
                self.sync_model();
            }
            Ok(_) | Err(_) => self.note("Load Failed"),
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
            ShellEvent::GraphPartner => {
                self.partner_shown = !self.partner_shown;
                self.sync_model();
            }
            ShellEvent::StripToggle(i) => {
                if let Some(path) = self.strip_path(i) {
                    let on = self.shell.model.strip[i].state.unwrap_or(false);
                    self.set(session, path, &[], Value::Bool(!on));
                } else {
                    // The Console's click on a plain tool button opens it.
                    self.handle_event(session, ShellEvent::StripOpen(i));
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
                let (popup, choices) = PresetMenu::popup(&self.shared.borrow(), active, dirty);
                self.popup = Some((AppDialog::PresetList(choices), popup));
            }
            ShellEvent::Source(Some(delta)) => {
                if let Some((choices, idx)) = &self.shell.model.source {
                    let next = (*idx as i32 + delta).clamp(0, choices.len() as i32 - 1) as u8;
                    if next as usize != *idx {
                        self.set(session, "in.source", &[], Value::Choice(next));
                    }
                }
            }
            ShellEvent::Source(None) => {
                if let Some((choices, idx)) = &self.shell.model.source {
                    let popup = PopupList::new("Source", choices.clone(), *idx);
                    self.popup = Some((AppDialog::SourceList, popup));
                }
            }
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
            ShellEvent::Session(req, owner) => {
                let reply = (req.run)(session);
                self.refresh(session);
                let follow = self.shell.deliver(owner, req.tag, reply, &self.state);
                for ev in follow {
                    self.handle_event(session, ev);
                }
            }
            ShellEvent::Status(s) => self.note(s),
            ShellEvent::Palette => self.prompt = Some(Prompt::new(true, &self.ctx)),
            ShellEvent::CommandLine => self.prompt = Some(Prompt::new(false, &self.ctx)),
            ShellEvent::SavePreset => {
                // Ctrl-S and `:commit` are the Console's one Commit Parameters
                // action, so they raise one dialog rather than two copies of
                // the same wording that could drift apart.
                let slot = self.state.caps.active_preset.unwrap_or(0);
                self.dialog = Some((AppDialog::Commit, actions::commit_dialog(slot)));
            }
            ShellEvent::DevicePicker => self.open_device_picker(),
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
            ShellEvent::GraphPopout => {
                self.shell.graph_popout = !self.shell.graph_popout;
                // The pop-out stays on the channel it opened on unless
                // Graphing says it follows the selection (the Console's
                // setting).
                self.popout_pinned = if self.shell.graph_popout
                    && !self.screens.config().graphing.popout_follows_selection
                {
                    self.shell.model.row_of_selection()
                } else {
                    None
                };
                self.sync_model();
            }
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
        let ni = self.state.caps.num_inputs as usize;
        let output = row.saturating_sub(ni);
        let name = screens::channel_name(&self.state, row);
        match screens::identify_command(&self.state, output) {
            Some(cmd) => {
                self.run_commands(session, &cmd);
                self.note(format!("Identifying {name}"));
            }
            None => self.note("Firmware has no signal generator"),
        }
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

    /// Show a screen by name, as `dspi screenshot` and the gallery ask:
    /// `overview`, `input` (the first input), `output` (the first output),
    /// a tool's lowercase title word (`matrix`, `crossfeed`, `loudness`,
    /// `leveller`, `psybass`, `upmixer`, `signals`, `stats`, `monitor`,
    /// `autoeq`, `spectrum`), or `settings`. Returns false for a name it does not know.
    pub fn show(&mut self, session: &mut Session, name: &str) -> bool {
        match name.to_ascii_lowercase().as_str() {
            "overview" => self.select(Selection::Overview),
            "input" => self.select(Selection::Input(0)),
            "output" => self.select(Selection::Output(0)),
            "matrix" => self.open_tool(Tool::Matrix),
            "crossfeed" => self.open_tool(Tool::Crossfeed),
            "loudness" => self.open_tool(Tool::Loudness),
            "leveller" => self.open_tool(Tool::Leveller),
            "psybass" | "bass" => self.open_tool(Tool::Psybass),
            "upmixer" => self.open_tool(Tool::Upmixer),
            "signals" => self.open_tool(Tool::Signals),
            "stats" => self.open_tool(Tool::Stats),
            "monitor" => self.open_tool(Tool::Monitor),
            "autoeq" => self.open_tool(Tool::AutoEq),
            "spectrum" => {
                if !self.shared.borrow().spectrum.engine.supported() {
                    self.connect_analyser(session);
                }
                self.open_tool(Tool::Spectrum)
            }
            "settings" => self.open_settings(session),
            _ => return false,
        }
        true
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
            (
                AppDialog::Unsaved {
                    then: PendingAction::CopyTo(dest),
                },
                DialogOutcome::Button(1),
            ) => {
                let source = self.state.caps.active_preset.unwrap_or(0);
                self.load_preset(session, source);
                self.copy_preset_to(session, dest);
            }
            (AppDialog::Unsaved { then }, DialogOutcome::Button(1)) => {
                self.run_pending(session, then)
            }
            (AppDialog::Rename { channel }, DialogOutcome::Text(name)) => {
                self.set(session, "ch.name", &[channel as u8], Value::Text(name));
                // The detail region names the channel in its title, so it has
                // to be rebuilt for the new name to show.
                let sel = self.shell.model.selection;
                self.shell.detail = self.screens.detail(&self.state, sel);
            }
            (AppDialog::PresetList(choices), DialogOutcome::Picked(i)) => {
                self.preset_action(session, choices.get(i).copied().flatten())
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
            (
                AppDialog::Hazard {
                    path,
                    indices,
                    value,
                },
                DialogOutcome::Button(0),
            ) => {
                self.hazard_confirmed = true;
                self.set(session, &path, &indices, value);
            }
            (AppDialog::SourceList, DialogOutcome::Picked(i)) => {
                self.set(session, "in.source", &[], Value::Choice(i as u8));
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

            // ---------------------------------------------------- file flows
            (AppDialog::Path(action), DialogOutcome::Text(path)) => {
                if !path.trim().is_empty() {
                    self.run_file_action(session, action, path.trim());
                }
            }
            (AppDialog::ImportChannels { file, targets }, DialogOutcome::Checked(checked, 0)) => {
                let targets: Vec<actions::ImportTarget> = targets
                    .into_iter()
                    .zip(checked)
                    .map(|(mut t, on)| {
                        t.checked = on;
                        t
                    })
                    .collect();
                let count = targets.iter().filter(|t| t.checked).count();
                let (commands, notes) = actions::import_commands(&file, &self.state, &targets);
                self.run_commands(session, &commands.join("\n"));
                let report = actions::import_report(&file, &self.state, count, &notes);
                self.note(report);
            }
            (AppDialog::ConfigOptions(doc), DialogOutcome::Checked(checked, 0)) => {
                let options = dspi_session::preset_file::ApplyOptions {
                    audio_processing: true,
                    volume_levels: checked.first().copied().unwrap_or(false),
                    hardware_io: checked.get(1).copied().unwrap_or(false),
                };
                // The Console shows a progress sheet here. Applying is
                // synchronous over the same control endpoint everything else
                // uses, so what a bar would animate is a few hundred
                // milliseconds of transfers; the report is the part that
                // matters and it comes up when they are done.
                let report = dspi_session::preset_file::apply(session, &doc, options);
                self.refresh(session);
                self.dialog = Some((
                    AppDialog::Report,
                    Dialog::report("Import Device Configuration", report.lines(false)),
                ));
            }

            // --------------------------------------------------------- tools
            (AppDialog::Commit, DialogOutcome::Button(0)) => {
                if self.save_active_preset(session) {
                    self.note("Preset saved successfully");
                }
            }
            (AppDialog::Revert, DialogOutcome::Button(0)) => {
                // The old synchronous REQ_LOAD_PARAMS crashed the device on
                // S/PDIF input and was reassigned; a host reverts by re-loading
                // the active slot, which is deferred and stream-safe
                // (config.h:201-204).
                match self.state.caps.active_preset {
                    Some(slot) => {
                        self.load_preset(session, slot);
                        self.note("Parameters reverted successfully");
                    }
                    None => self
                        .note("No saved parameters found.  The device is using factory defaults."),
                }
            }
            (AppDialog::FactoryReset, DialogOutcome::Button(0)) => {
                match session.write("dev.reset", &[], Value::Trigger) {
                    Ok(_) => {
                        self.refresh(session);
                        self.state.mark_saved();
                        self.note("Factory reset complete");
                    }
                    Err(_) => self.note("Failed to reset parameters"),
                }
            }
            (AppDialog::Bootloader, DialogOutcome::Button(0)) => {
                // The device answers, waits 100 ms and resets; there is nothing
                // to acknowledge and every later transfer will fail, which is
                // expected rather than an error (survey 6.2).
                let _ = session.write("dev.bootloader", &[], Value::Trigger);
                let watch = actions::BootloaderWatch::new();
                self.dialog = Some((
                    AppDialog::BootWait,
                    Dialog::progress("Firmware Update", watch.status()),
                ));
                self.boot = Some(watch);
            }
            (AppDialog::BootWait, _) => self.boot = None,
            (AppDialog::DevicePicker(serials), DialogOutcome::Picked(i)) => {
                if let Some(serial) = serials.get(i).cloned() {
                    self.switch_device(serial);
                }
            }

            // -------------------------------------------------------- autoeq
            (AppDialog::AutoEqUpdate, DialogOutcome::Button(0)) => {
                self.dialog = Some((
                    AppDialog::AutoEqRebuild,
                    Dialog::confirm(
                        "Rebuild AutoEQ Database",
                        dspi_session::autoeq::rebuild_warning(),
                        vec![Button::new("Rebuild"), Button::new("Cancel")],
                    )
                    .default_button(1),
                ));
            }
            (AppDialog::AutoEqUpdate, DialogOutcome::Button(1)) => {
                self.ask_path(session, FileAction::ImportAutoEqDatabase, &[])
            }
            (AppDialog::AutoEqUpdate, DialogOutcome::Button(2)) => {
                match dspi_session::autoeq::reset_to_builtin() {
                    Ok(n) => {
                        self.reload_autoeq();
                        self.note(format!("Reset to built-in database.  Entries: {n}"));
                    }
                    Err(e) => self.note(format!("Failed to reset: {e}")),
                }
            }
            (AppDialog::AutoEqRebuild, DialogOutcome::Button(0)) => {
                let handle = dspi_session::autoeq::rebuild_from_github(Box::new(
                    dspi_session::autoeq::CurlFetch,
                ));
                self.dialog = Some((
                    AppDialog::AutoEqRebuildProgress,
                    Dialog::progress("Updating Database", "Connecting to GitHub..."),
                ));
                self.rebuild = Some(handle);
            }
            (AppDialog::AutoEqRebuildProgress, _) => {
                // Escape on the progress dialog stops the download rather than
                // leaving a thread hammering GitHub with nobody watching.
                self.rebuild = None;
                self.note("Rebuild cancelled");
            }
            _ => {}
        }
    }

    /// Switch to another device, asking first if this one has unsaved changes.
    fn switch_device(&mut self, serial: String) {
        if serial == self.state.caps.serial {
            return;
        }
        if self.state.has_unsaved_changes() {
            self.dialog = Some((
                AppDialog::Unsaved {
                    then: PendingAction::SwitchDevice(serial),
                },
                self.unsaved_dialog(),
            ));
        } else {
            self.switch_to = Some(serial);
        }
    }

    /// Open another device in place of this one.
    ///
    /// Everything that describes the old device goes: the capabilities, the
    /// bulk shadow, the meters, the preset names and the diagnostics. Keeping
    /// any of it would show one device's numbers under another one's name.
    pub fn adopt(&mut self, session: &mut Session, state: DeviceState) {
        let caps = &state.caps;
        self.ctx = Context {
            channel_slugs: caps.channels.iter().map(|c| c.slug.clone()).collect(),
            num_inputs: caps.num_inputs,
            num_outputs: caps.num_outputs,
            max_bands: caps.max_bands,
        };
        let n = caps.num_channels as usize;
        self.peaks = vec![PeakHold::default(); n];
        self.popout_pinned = None;
        self.state = state;
        // The Console discards Settings drafts and device facts on a different
        // serial; the pages re-read on their next open.
        self.screens.refresh_settings(session);
        {
            let mut shared = self.shared.borrow_mut();
            shared.stats = actions::Stats::default();
            shared.preset_names.clear();
            shared.occupied = 0;
            shared.default_slot = None;
        }
        self.shell.model.connected = true;
        self.select(Selection::Overview);
        self.refresh_presets(session);
        self.note(format!("Switched to {}", self.state.caps.serial));
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
        // The second save below writes the live state over the source slot,
        // so the live state must be the source's saved state first: ask, as
        // the Console does, and on Discard reload the source before copying.
        if self.state.has_unsaved_changes() {
            self.dialog = Some((
                AppDialog::Unsaved {
                    then: PendingAction::CopyTo(dest),
                },
                self.unsaved_dialog(),
            ));
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
            PendingAction::SwitchDevice(serial) => self.switch_to = Some(serial),
            PendingAction::CopyTo(dest) => self.copy_preset_to(session, dest),
        }
    }

    /// The notification reader for the current session, started or
    /// restarted; the old one is dropped first so its interface is released.
    pub fn arm_notifications(&mut self, session: &mut Session) {
        self.notes = None;
        self.notes = session
            .with_transport(|t| Ok(t.notifications()))
            .ok()
            .flatten()
            .map(Notifications::start);
    }

    /// Every attached device, for the picker and the title bar.
    pub fn refresh_devices(&mut self) {
        self.shell.model.devices = dspi_transport::list_devices()
            .map(|ds| {
                ds.iter()
                    .map(|d| format!("DSPi {}", d.short_name()))
                    .collect()
            })
            .unwrap_or_default();
        self.devices_checked = Some(Instant::now());
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
            // A path field completes on Tab the way a shell does. The dialog
            // widget cannot do this itself: only the flow knows the field is a
            // path rather than a name.
            if key.code == KeyCode::Tab
                && matches!(kind, AppDialog::Path(_))
                && let crate::widgets::DialogKind::Text { value, .. } = &mut d.kind
                && let Some(completed) = actions::complete_path(value)
            {
                *value = completed;
                return;
            }
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

    /// Refresh the Stats panel's diagnostics, every two seconds and only while
    /// that panel is on screen.
    ///
    /// None of it is in the bulk packet and none of it is notified, so it can
    /// only be asked for; two dozen control transfers every two seconds is
    /// worth it for a panel someone is reading and worth nothing otherwise.
    /// The screen on top gets its once-a-second poll.
    fn poll_screen(&mut self, session: &mut Session, now: Instant) {
        if now.duration_since(self.last_screen_poll) < Duration::from_secs(1) {
            return;
        }
        self.last_screen_poll = now;
        if let Some(s) = self.shell.settings.as_deref_mut() {
            s.poll(session, &self.state);
        } else if let Some((_, s)) = self.shell.tool.as_mut() {
            s.poll(session, &self.state);
        }
    }

    fn poll_stats(&mut self, session: &mut Session, now: Instant) {
        if !matches!(self.shell.tool, Some((Tool::Stats, _)))
            || now.duration_since(self.last_stats_poll) < Duration::from_secs(2)
        {
            return;
        }
        self.last_stats_poll = now;
        let previous = self.shared.borrow().stats.clone();
        let stats = actions::read_stats(session, &self.state, &previous);
        self.shared.borrow_mut().stats = stats;
    }

    fn on_meters(&mut self, m: dspi_session::Meters, dt: f32, session: &mut Session) {
        let now = Instant::now();
        for (i, p) in self.peaks.iter_mut().enumerate() {
            p.update(m.peaks.get(i).copied().unwrap_or(0.0), dt);
        }
        self.state.update_meters(m);
        match (self.state.clip_latched != 0, self.clip_since) {
            (true, None) => self.clip_since = Some(now),
            (true, Some(since)) if now.duration_since(since) >= CLIP_HOLD => {
                // The Console clears the latch itself after three
                // seconds and asks the device to forget too.
                self.state.clear_clip_latch();
                let _ = session.write("meters.clear", &[], Value::Trigger);
                self.clip_since = None;
            }
            (false, _) => self.clip_since = None,
            _ => {}
        }
    }

    /// Meters, notifications, status expiry, peak ballistics.
    pub fn tick(&mut self, session: &mut Session, notifications: Option<&Notifications>) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f32();
        self.last_tick = now;

        match session.meters() {
            Ok(m) => self.on_meters(m, dt, session),
            // The control path is the other witness to a disconnect.
            Err(dspi_session::WriteError::Transport(
                dspi_transport::TransportError::Disconnected,
            )) => {
                self.state.connected = false;
            }
            Err(_) => {}
        }
        self.poll_upmix_status(session, now);
        self.poll_stats(session, now);
        self.poll_screen(session, now);
        self.tick_analyser(session, now);

        let mut reread = false;
        let mut loaded_elsewhere = false;
        let mut changed_elsewhere: Option<(&'static str, Source)> = None;
        if let Some(n) = notifications {
            self.shared.borrow_mut().log.active = true;
            let at = now.duration_since(self.started).as_secs_f64();
            for note in n.drain() {
                // The monitor is fed from this drain rather than from a reader
                // of its own: two readers on one endpoint would each see half
                // the events, and the log has to keep running while the panel
                // is closed so opening it shows what just happened.
                self.shared.borrow_mut().log.push(at, &note);
                match self.state.apply(&note) {
                    Applied::Section { name, source } => {
                        if !source.is_ours() {
                            changed_elsewhere = Some((name, source));
                            if matches!(name, "user_volume" | "master_volume")
                                && let Some(from) = self.shown_volume
                            {
                                self.ease_from = Some((now, from));
                            }
                        }
                    }
                    Applied::NeedsReread { source } => {
                        reread = true;
                        if source == Source::Preset {
                            loaded_elsewhere = true;
                        }
                    }
                    Applied::PresetLoaded { slot } => {
                        self.state.caps.active_preset = Some(slot);
                        reread = true;
                        loaded_elsewhere = true;
                        self.note(format!("Preset {} loaded", slot + 1));
                    }
                    Applied::InputFormat { channels } => {
                        self.note(format!("{channels} input channels active"));
                    }
                    Applied::Status(_) | Applied::Nothing => {}
                }
            }
            if n.is_disconnected() {
                self.state.connected = false;
            }
        }
        if reread || self.state.stale {
            self.refresh(session);
            self.state.stale = false;
        }
        // A preset the device loaded is the new baseline, as a preset this
        // host loaded is; otherwise the marker compares against the old one.
        if loaded_elsewhere {
            self.state.mark_saved();
        }
        if self
            .devices_checked
            .is_none_or(|t| now.duration_since(t) >= Duration::from_secs(5))
        {
            self.refresh_devices();
        }
        if let Some((name, source)) = changed_elsewhere {
            self.echo(format!("{} {}", name.replace('_', " "), source.describe()));
        }
        if let Some(until) = self.status_until
            && now >= until
        {
            self.shell.model.status = None;
            self.status_until = None;
        }
        self.advance_boot();
        self.advance_rebuild();
        self.sync_model();
    }

    /// Read the spectrum analyser's caps, and push the Settings page's
    /// values on the next tick: the device forgets them at every power cycle.
    /// Called on connect and after a device switch.
    pub fn connect_analyser(&mut self, session: &mut Session) {
        let absent = self
            .state
            .caps
            .features
            .iter()
            .any(|f| f.name == "spectrum_analyser" && !f.present);
        let mut app = self.shared.borrow_mut();
        let engine = &mut app.spectrum.engine;
        if absent {
            engine.disconnect();
        } else {
            let _ = session.with_transport(|t| Ok(engine.connect(t)));
        }
    }

    /// One analyser poll, inside its budget, after the meters so they are
    /// never late for it. Skipped outright when a key is waiting.
    fn tick_analyser(&mut self, session: &mut Session, now: Instant) {
        if self.input_pending {
            return;
        }
        let report = {
            let mut app = self.shared.borrow_mut();
            let engine = &mut app.spectrum.engine;
            session
                .with_transport(|t| Ok(engine.tick(t, now, dspi_session::rta::Budget::default())))
                .unwrap_or_default()
        };
        if report.disconnected {
            self.state.connected = false;
        }
    }

    /// Move the bootloader handoff along, and close it out when it lands.
    fn advance_boot(&mut self) {
        let Some(watch) = self.boot.as_mut() else {
            return;
        };
        watch.poll(&actions::RealBootProbe);
        let (status, fraction, finished) = (watch.status(), watch.fraction(), watch.finished());
        if let Some((AppDialog::BootWait, dialog)) = self.dialog.as_mut() {
            dialog.set_progress(fraction, status.clone());
            if finished {
                // A progress dialog has no buttons, so the wait becomes a
                // report the person can dismiss once it has an answer.
                *dialog = Dialog::report("Firmware Update", vec![status]);
            }
        }
        if finished {
            self.dialog = self.dialog.take().map(|(_, d)| (AppDialog::Report, d));
            self.boot = None;
        }
    }

    /// Move the AutoEQ rebuild's progress dialog along.
    fn advance_rebuild(&mut self) {
        let Some(handle) = self.rebuild.as_mut() else {
            return;
        };
        let progress = handle.poll().clone();
        if let Some((AppDialog::AutoEqRebuildProgress, dialog)) = self.dialog.as_mut() {
            dialog.set_progress(progress.fraction, progress.status.clone());
        }
        let Some(done) = progress.done else { return };
        self.rebuild = None;
        self.dialog = None;
        match done {
            Ok(n) => {
                self.reload_autoeq();
                self.note(format!("Database rebuilt successfully! Entries: {n}"));
            }
            Err(e) => self.note(format!("Rebuild failed: {e}")),
        }
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

/// Open another DSPi by serial, probing it from scratch.
///
/// Nothing about the old device is carried over: a different unit can have a
/// different channel count, a different firmware and different features, and
/// reusing any of that would show one device's shape under another's name.
fn open_device(serial: &str) -> Result<(Session, DeviceState), String> {
    let mut transport =
        dspi_transport::UsbTransport::open_serial(serial).map_err(|e| e.to_string())?;
    let caps = dspi_session::probe(&mut transport).map_err(|e| e.to_string())?;
    let mut session =
        Session::new(Box::new(transport), caps.clone()).ok_or("unusable channel map")?;
    let bulk = session.snapshot().map_err(|e| e.to_string())?;
    Ok((session, DeviceState::new(caps, bulk)))
}

/// Run the live interface until the person quits.
pub fn run(mut live: Live, session: &mut Session) -> io::Result<()> {
    live.arm_notifications(session);
    live.connect_analyser(session);
    live.refresh_devices();
    let perf = live.perf;
    live.refresh_presets(session);
    let mut terminal = ratatui::init();
    let mut last_poll = Instant::now() - perf.meter_interval;

    let result = (|| -> io::Result<()> {
        loop {
            if last_poll.elapsed() >= perf.meter_interval {
                live.input_pending = event::poll(Duration::ZERO)?;
                let notes = live.notes.take();
                live.tick(session, notes.as_ref());
                live.notes = notes;
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
            if let Some(serial) = live.switch_to.take() {
                match open_device(&serial) {
                    Ok((next, state)) => {
                        *session = next;
                        live.adopt(session, state);
                        live.arm_notifications(session);
                        live.connect_analyser(session);
                    }
                    Err(e) => live.note(format!("Could not open {serial}: {e}")),
                }
            }
            if live.should_quit {
                return Ok(());
            }
        }
    })();
    ratatui::restore();
    result
}

/// The connect animation: the curve draws left to right over `REVEAL`, the
/// rest of it hidden as non-finite points the graph skips.
fn reveal(mut points: Vec<f64>, started: Instant, animate: bool) -> Vec<f64> {
    if !animate {
        return points;
    }
    let t = started.elapsed().as_secs_f64() / REVEAL.as_secs_f64();
    if t >= 1.0 {
        return points;
    }
    let shown = (t * points.len() as f64) as usize;
    for p in points.iter_mut().skip(shown) {
        *p = f64::NAN;
    }
    points
}

/// A value in the words the registry uses for it: a choice by name, a number
/// with its unit.
fn display_value(d: &dspi_proto::registry::ParamDesc, v: &Value) -> String {
    if let (dspi_proto::registry::Kind::Choice(variants), Some(n)) = (d.kind, v.as_u8())
        && let Some((_, name)) = variants.iter().find(|(raw, _)| *raw == n)
    {
        return (*name).to_string();
    }
    v.display(d.kind.unit())
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
        // Tests run without the reveal and the easing, as --lite does, so a
        // value asserted right after a change is the value itself.
        let live = Live::new(
            state,
            theme,
            Performance::lite(),
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
        assert!(m.curves.is_empty(), "the overview graphs nothing");
        assert_eq!(m.graph_channel, None);
        // An unnamed channel shows its descriptor instead of a blank.
        let (mut l, _, _) = live();
        let (_, n, _) = dspi_proto::generated::SECTIONS[9];
        l.state.bulk.patch(n + 32, &[0; 32]);
        l.sync_model();
        assert_eq!(l.shell.model.inputs[1].name, "IN2");
    }

    #[test]
    fn the_graph_carries_the_selected_channel_and_its_linked_partner() {
        let (mut l, mut session, _) = live();
        l.select(Selection::Input(0));
        let m = &l.shell.model;
        assert_eq!(m.curves.len(), 1, "unlinked: the channel alone");
        assert_eq!(m.graph_channel.as_deref(), Some("FL"));
        assert!(m.curves[0].selected);
        assert_eq!(m.curves[0].color, l.shell.theme.inputs[0]);

        l.shared.borrow_mut().set_linked(0, true);
        l.sync_model();
        let m = &l.shell.model;
        assert_eq!(m.curves.len(), 2, "linked: the partner underneath");
        assert_eq!(m.curves[0].descriptor, "IN2");
        assert_eq!(m.curves[0].color, l.shell.theme.dim);
        assert!(!m.curves[0].selected);
        assert_eq!(m.curves[1].descriptor, "IN1");

        l.handle_event(&mut session, ShellEvent::GraphPartner);
        assert_eq!(l.shell.model.curves.len(), 1, "`.` hides the partner");
        l.handle_event(&mut session, ShellEvent::GraphPartner);
        assert_eq!(l.shell.model.curves.len(), 2);

        // An output has no partner, whatever the link says.
        l.select(Selection::Output(0));
        assert_eq!(l.shell.model.curves.len(), 1);
        assert_eq!(l.shell.model.graph_channel.as_deref(), Some("OUT L"));
    }

    /// The same runner, wired to the Console's screens rather than the
    /// placeholders.
    fn console() -> (Live, Session, LogHandle) {
        console_with(false)
    }

    /// The same runner against a device that answers every opcode rather than
    /// stalling the ones the fixture does not script.
    ///
    /// The File and Tools verbs touch dozens of opcodes each, and an
    /// unscripted one costs the transport's whole busy-retry backoff; a test
    /// of what reaches the wire should not spend minutes waiting for a mock to
    /// decide it is not going to answer.
    fn answering() -> (Live, Session, LogHandle) {
        console_with(true)
    }

    fn console_with(answer_everything: bool) -> (Live, Session, LogHandle) {
        let mut mock = MockTransport::new()
            .window(op::REQ_GET_ALL_PARAMS_CHUNK, packet())
            .data(op::REQ_GET_USER_VOLUME, (-12.5f32).to_le_bytes().to_vec())
            .data(op::REQ_GET_STATUS, vec![0; 41]);
        if answer_everything {
            mock = mock.answering_everything(vec![0; 64]);
        }
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
        // Nothing is stored on the fixture device, so the Clear rows are
        // absent, as the Console hides them; Copy to... is the last action.
        assert_eq!(popup.items.last().map(String::as_str), Some("Copy to..."));
        assert!(!popup.items.iter().any(|i| i.starts_with("Clear")));
        // With a stored slot the menu ends in Clear All Slots..., and picking
        // it opens its own step.
        l.popup = None;
        l.shared.borrow_mut().occupied = 0b101;
        l.handle_key(&mut s, key(KeyCode::Enter));
        let (_, popup) = l.popup.as_ref().expect("the preset popup");
        assert_eq!(
            popup.items.last().map(String::as_str),
            Some("Clear All Slots...")
        );
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

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dspi-live-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        dir.join(name)
    }

    /// Every File and Tools verb is offered by the palette, or half the
    /// program is unreachable without reading the source.
    #[test]
    fn the_palette_offers_the_interfaces_own_verbs() {
        let (l, _, _) = console();
        let p = Prompt::new(true, &l.ctx);
        for (verb, _) in APP_VERBS {
            assert!(
                p.candidates.iter().any(|c| c.value == *verb),
                "{verb} is not in the palette"
            );
        }
        // And they complete like anything else.
        let mut p = Prompt::new(true, &l.ctx);
        p.input = "import".into();
        p.refresh(&l.ctx);
        let offered: Vec<&str> = p.candidates.iter().map(|c| c.value.as_str()).collect();
        assert!(offered.contains(&"import") && offered.contains(&"import-config"));
    }

    #[test]
    fn export_writes_a_filter_file_without_asking_when_the_path_came_with_the_verb() {
        let (mut l, mut s, _) = console();
        let path = scratch("tuning.txt");
        l.run_command(&mut s, &format!("export {}", path.display()));
        assert!(l.dialog.is_none(), "no dialog when the path was given");
        assert_eq!(
            l.shell.model.status.as_deref(),
            Some("Filters exported successfully")
        );
        let text = std::fs::read_to_string(&path).expect("the file");
        assert!(text.contains("# DSPi Console Filter Settings"), "{text}");
        assert!(text.contains("[Input 0: FL]"), "{text}");
        let _ = std::fs::remove_file(&path);
    }

    /// The whole import flow: a path, then the channel checklist, then the
    /// bands on the wire and the Console's report.
    #[test]
    fn import_asks_which_channels_then_writes_the_bands() {
        let (mut l, mut s, log) = answering();
        let path = scratch("rew.txt");
        std::fs::write(
            &path,
            "Preamp: -6.5 dB\nFilter 1: ON PK Fc 100 Hz Gain 3.0 dB Q 1.00\n",
        )
        .unwrap();

        l.run_command(&mut s, &format!("import {}", path.display()));
        let (kind, dialog) = l.dialog.as_ref().expect("the channel picker");
        assert!(matches!(kind, AppDialog::ImportChannels { .. }));
        assert_eq!(dialog.title, "Import Filters");
        assert!(
            dialog
                .body
                .contains("Found 1 filter(s) and a -6.5 dB preamp")
        );

        // Take everything but the first channel off, then import.
        l.handle_key(&mut s, key(KeyCode::Down));
        for _ in 0..7 {
            l.handle_key(&mut s, key(KeyCode::Char(' ')));
            l.handle_key(&mut s, key(KeyCode::Down));
        }
        l.handle_key(&mut s, key(KeyCode::Enter));

        let bands = log
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.opcode == op::REQ_SET_EQ_PARAM)
            .count();
        assert_eq!(bands, 10, "one channel's whole bank, filled and cleared");
        assert_eq!(
            l.shell.model.status.as_deref(),
            Some("Filters imported to 1 channel(s)")
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_config_export_and_import_round_trip_through_the_options_checklist() {
        let (mut l, mut s, _) = answering();
        let path = scratch("config.dspipreset");
        l.run_command(&mut s, &format!("export-config {}", path.display()));
        assert!(
            l.shell
                .model
                .status
                .as_deref()
                .is_some_and(|m| m.starts_with("Configuration exported.")),
            "{:?}",
            l.shell.model.status
        );

        l.run_command(&mut s, &format!("import-config {}", path.display()));
        let (kind, dialog) = l.dialog.as_ref().expect("the options checklist");
        assert!(matches!(kind, AppDialog::ConfigOptions(_)));
        assert!(
            dialog.body.contains(
                "EQ, crossover, delays, gains, routing and the DSP features are always applied."
            ),
            "{}",
            dialog.body
        );
        // Import with both options left off, which is the Console's default.
        l.handle_key(&mut s, key(KeyCode::Enter));
        let (kind, dialog) = l.dialog.as_ref().expect("the report");
        assert!(matches!(kind, AppDialog::Report));
        match &dialog.kind {
            crate::widgets::DialogKind::Report { lines, .. } => {
                assert!(lines[0].starts_with("Applied "), "{lines:?}");
                assert!(
                    lines
                        .last()
                        .unwrap()
                        .contains("These changes are live but not yet stored on the device."),
                    "{lines:?}"
                );
            }
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_path_dialog_completes_on_tab() {
        let (mut l, mut s, _) = console();
        let path = scratch("completable.txt");
        std::fs::write(&path, "x").unwrap();
        l.run_command(&mut s, "import");
        let stem = path
            .to_string_lossy()
            .replace("completable.txt", "completab");
        if let Some((_, d)) = l.dialog.as_mut()
            && let crate::widgets::DialogKind::Text { value, .. } = &mut d.kind
        {
            *value = stem;
        }
        l.handle_key(&mut s, key(KeyCode::Tab));
        let Some((_, d)) = l.dialog.as_ref() else {
            panic!("the dialog closed")
        };
        match &d.kind {
            crate::widgets::DialogKind::Text { value, .. } => {
                assert_eq!(value, &path.to_string_lossy().to_string())
            }
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_tools_verbs_confirm_before_they_act() {
        let (mut l, mut s, log) = answering();

        l.run_command(&mut s, "commit");
        assert_eq!(
            l.dialog.as_ref().unwrap().1.body,
            "Save current parameters to preset slot 3?"
        );
        l.handle_key(&mut s, key(KeyCode::Enter));
        assert!(
            log.lock()
                .unwrap()
                .iter()
                .any(|e| e.opcode == op::REQ_PRESET_SAVE),
            "Save writes the preset"
        );

        l.run_command(&mut s, "factory-reset");
        let (_, d) = l.dialog.as_ref().unwrap();
        assert!(d.critical, "a factory reset is a critical confirm");
        // Escape leaves the device alone.
        l.handle_key(&mut s, key(KeyCode::Esc));
        assert!(
            !log.lock()
                .unwrap()
                .iter()
                .any(|e| e.opcode == op::REQ_FACTORY_RESET),
            "cancelling wrote nothing"
        );
        l.run_command(&mut s, "factory-reset");
        l.handle_key(&mut s, key(KeyCode::Char('r')));
        assert!(
            log.lock()
                .unwrap()
                .iter()
                .any(|e| e.opcode == op::REQ_FACTORY_RESET)
        );
        assert_eq!(
            l.shell.model.status.as_deref(),
            Some("Factory reset complete")
        );
    }

    /// Revert re-loads the active slot: the old synchronous load opcode was
    /// reassigned because it crashed the device on S/PDIF input.
    #[test]
    fn revert_reloads_the_active_preset_rather_than_the_reassigned_opcode() {
        let (mut l, mut s, log) = console();
        l.run_command(&mut s, "revert");
        assert_eq!(l.dialog.as_ref().unwrap().1.title, "Revert to Saved");
        l.handle_key(&mut s, key(KeyCode::Char('r')));
        let sent = log.lock().unwrap();
        assert!(sent.iter().any(|e| e.opcode == op::REQ_PRESET_LOAD));
        assert!(
            !sent.iter().any(|e| e.opcode == op::REQ_SAVE_OUTPUT_CONFIG),
            "0x52 is Save Output Configuration now, not Load Params"
        );
    }

    #[test]
    fn save_master_and_save_output_config_report_what_they_did() {
        let (mut l, mut s, log) = answering();
        l.run_command(&mut s, "save-master");
        assert!(
            l.shell
                .model
                .status
                .as_deref()
                .is_some_and(|m| m.starts_with("Master volume saved (")),
            "{:?}",
            l.shell.model.status
        );
        assert!(
            l.shell
                .model
                .status
                .as_deref()
                .is_some_and(|m| m.ends_with("It will be applied on next boot.")),
            "{:?}",
            l.shell.model.status
        );
        l.run_command(&mut s, "save-output-config");
        assert_eq!(
            l.shell.model.status.as_deref(),
            Some("Output configuration saved. It will be applied on next boot.")
        );
        let sent = log.lock().unwrap();
        assert!(sent.iter().any(|e| e.opcode == op::REQ_SAVE_MASTER_VOLUME));
        assert!(sent.iter().any(|e| e.opcode == op::REQ_SAVE_OUTPUT_CONFIG));
    }

    /// The bootloader jump has no acknowledgement and every later transfer
    /// fails, so what the person sees afterwards is the whole of the feature.
    #[test]
    fn the_firmware_update_confirms_then_waits_for_the_drive() {
        let (mut l, mut s, log) = console();
        l.run_command(&mut s, "bootloader");
        let (_, d) = l.dialog.as_ref().expect("the confirm");
        assert!(d.critical);
        assert_eq!(d.buttons[0].label, "Reboot into Bootloader");
        l.handle_key(&mut s, key(KeyCode::Char('r')));

        assert!(
            log.lock()
                .unwrap()
                .iter()
                .any(|e| e.opcode == op::REQ_ENTER_BOOTLOADER)
        );
        assert!(l.boot.is_some(), "the wait started");
        let (kind, d) = l.dialog.as_ref().expect("the progress dialog");
        assert!(matches!(kind, AppDialog::BootWait));
        assert_eq!(d.title, "Firmware Update");
        match &d.kind {
            crate::widgets::DialogKind::Progress { status, .. } => {
                assert_eq!(status, "Waiting for the device to disconnect...")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_autoeq_verb_opens_the_browser_and_its_update_menu() {
        let (mut l, mut s, _) = console();
        l.run_command(&mut s, "autoeq");
        assert!(matches!(l.shell.tool, Some((Tool::AutoEq, _))));

        l.run_command(&mut s, "autoeq update");
        let (kind, d) = l.dialog.as_ref().expect("the update menu");
        assert!(matches!(kind, AppDialog::AutoEqUpdate));
        assert_eq!(d.title, "Update AutoEQ Database");
        assert!(d.body.contains("Choose an update method:"), "{}", d.body);
        assert_eq!(d.buttons[0].label, "Rebuild from GitHub");
        assert_eq!(d.buttons[1].label, "Import File...");

        // The rebuild is opt-in behind a warning that names what it fetches.
        l.handle_key(&mut s, key(KeyCode::Enter));
        let (kind, d) = l.dialog.as_ref().expect("the rebuild confirm");
        assert!(matches!(kind, AppDialog::AutoEqRebuild));
        assert!(d.body.contains("api.github.com"), "{}", d.body);
        assert_eq!(d.default, 1, "Cancel is the default");
    }

    /// One device is not a picker; the Console only offers one with more than
    /// one attached.
    #[test]
    fn the_device_picker_says_so_when_there_is_nothing_to_pick() {
        let (mut l, mut s, _) = console();
        l.handle_event(&mut s, ShellEvent::DevicePicker);
        assert!(l.dialog.is_none());
        assert!(
            l.shell
                .model
                .status
                .as_deref()
                .is_some_and(|m| m.contains("device")),
            "{:?}",
            l.shell.model.status
        );
    }

    /// A switch with unsaved changes asks first, and only goes through once
    /// the answer is in.
    #[test]
    fn switching_device_with_unsaved_changes_asks_first() {
        let (mut l, mut s, _) = console();
        let (_, g, _) = dspi_proto::generated::SECTIONS[1];
        l.state.bulk.patch(g, &(-6.0f32).to_le_bytes());
        l.sync_model();
        l.switch_device("OTHER0000000001".into());
        assert!(l.switch_to.is_none(), "not yet");
        assert!(matches!(l.dialog, Some((AppDialog::Unsaved { .. }, _))));
        l.handle_key(&mut s, key(KeyCode::Char('d')));
        assert_eq!(l.switch_to.as_deref(), Some("OTHER0000000001"));
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
            l.shell.model.echo,
            "user volume changed by the system volume"
        );
    }

    /// The analyser is read inside the loop's own tick, after the meters,
    /// only while its panel watches; closing the panel stops the device, and
    /// a waiting key makes it sit a tick out.
    #[test]
    fn the_analyser_is_polled_while_its_panel_is_open_and_stopped_after() {
        use crate::screens::spectrum::demo;
        use dspi_transport::mock::{Direction, Reply};
        let centres = |r: std::ops::Range<usize>| -> Vec<u8> {
            demo::CENTRES[r]
                .iter()
                .flat_map(|c| c.to_le_bytes())
                .collect()
        };
        let mock = MockTransport::new()
            .window(op::REQ_GET_ALL_PARAMS_CHUNK, packet())
            .data(op::REQ_GET_STATUS, vec![0; 41])
            .reply(
                op::REQ_RTA_GET_CAPS,
                Reply::Sequence(vec![
                    Reply::Data(demo::caps().encode().to_vec()),
                    Reply::Data(centres(0..32)),
                    Reply::Data(centres(32..37)),
                ]),
            )
            .data(op::REQ_RTA_GET_BANDS, demo::frame(0).encode().to_vec())
            .data(op::REQ_RTA_CONTROL, vec![1]);
        let log = mock.log_handle();
        let mut s = Session::new(Box::new(mock), caps()).unwrap();
        let state = DeviceState::new(
            caps(),
            dspi_proto::wire::BulkPacket::decode(packet()).unwrap(),
        );
        let theme = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let mut l = Live::new(
            state,
            theme,
            Performance::lite(),
            Box::new(ConsoleScreens::new()),
        );
        l.connect_analyser(&mut s);
        assert!(l.shared.borrow().spectrum.engine.supported());
        let rta = |o: u8| (op::REQ_RTA_SET_CONFIG..=op::REQ_RTA_GET_BANDS_ALL).contains(&o);

        // Nothing watches yet, so nothing is read.
        log.lock().unwrap().clear();
        l.tick(&mut s, None);
        assert!(!log.lock().unwrap().iter().any(|x| rta(x.opcode)));

        l.handle_event(&mut s, ShellEvent::OpenTool(Tool::Spectrum));
        log.lock().unwrap().clear();
        l.tick(&mut s, None);
        let ops: Vec<u8> = log.lock().unwrap().iter().map(|x| x.opcode).collect();
        let first_rta = ops.iter().position(|o| rta(*o)).expect("the analyser ran");
        assert!(
            first_rta > 0 && !rta(ops[0]),
            "the meters go first: {ops:02X?}"
        );
        assert!(ops.contains(&op::REQ_RTA_GET_BANDS), "{ops:02X?}");

        // A key waiting: the analyser sits this tick out.
        l.input_pending = true;
        log.lock().unwrap().clear();
        l.tick(&mut s, None);
        assert!(!log.lock().unwrap().iter().any(|x| rta(x.opcode)));
        l.input_pending = false;

        l.handle_event(&mut s, ShellEvent::CloseTool);
        log.lock().unwrap().clear();
        l.tick(&mut s, None);
        let stops: Vec<(Direction, u16)> = log
            .lock()
            .unwrap()
            .iter()
            .filter(|x| x.opcode == op::REQ_RTA_CONTROL)
            .map(|x| (x.direction, x.value))
            .collect();
        assert_eq!(stops, vec![(Direction::In, 0)], "STOP, wValue 0");
    }
}
