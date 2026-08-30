//! The shell: the Console's window shape in a terminal.
//!
//! A channel sidebar with meters and pills on the left; on the right the
//! response graph with its legend and, beneath it, the detail region that
//! shows the overview, an input or an output. Tool panels replace the right
//! pane; Settings replaces the screen. The shell owns focus, the Escape stack,
//! popups, dialogs and the help overlay, and asks the application for
//! everything else through [`ShellEvent`]. See `docs/plan/DESIGN.md` 2 and 3.

pub mod fixture;
pub mod layout;
pub mod model;
pub mod screen;
pub mod sidebar;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use dspi_session::DeviceState;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, Widget};

pub use layout::{Density, Regions};
pub use model::{ChannelItem, GraphHeight, Selection, ShellModel, StripItem, VolumeMode};
pub use screen::{Placeholder, Screen, ScreenEvent, SessionReply, SessionRequest};
pub use sidebar::FooterRow;

use crate::graph::{Graph, legend};
use crate::theme::{Glyphs, Theme};
use crate::widgets::text::{fit_left, truncate};
use crate::widgets::{Action, Dialog, HelpOverlay, KeyHelp, LegendRow, PopupList};

/// The Console's tool windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Matrix,
    Loudness,
    Crossfeed,
    Psybass,
    Upmixer,
    Leveller,
    Signals,
    Stats,
    Monitor,
    AutoEq,
}

impl Tool {
    /// The Console's Shift-Cmd mnemonics, as uppercase letters.
    pub fn from_key(c: char) -> Option<Self> {
        Some(match c {
            'M' => Self::Matrix,
            'L' => Self::Loudness,
            'X' => Self::Crossfeed,
            'P' => Self::Psybass,
            'U' => Self::Upmixer,
            'V' => Self::Leveller,
            'G' => Self::Signals,
            'T' => Self::Stats,
            'I' => Self::Monitor,
            'B' => Self::AutoEq,
            _ => return None,
        })
    }

    pub fn key(self) -> char {
        match self {
            Self::Matrix => 'M',
            Self::Loudness => 'L',
            Self::Crossfeed => 'X',
            Self::Psybass => 'P',
            Self::Upmixer => 'U',
            Self::Leveller => 'V',
            Self::Signals => 'G',
            Self::Stats => 'T',
            Self::Monitor => 'I',
            Self::AutoEq => 'B',
        }
    }

    /// The Console's window title.
    pub fn title(self) -> &'static str {
        match self {
            Self::Matrix => "Matrix Mixer",
            Self::Loudness => "Loudness Compensation",
            Self::Crossfeed => "Crossfeed",
            Self::Psybass => "Psychoacoustic Bass",
            Self::Upmixer => "Stereo Upmixer",
            Self::Leveller => "Volume Leveller",
            Self::Signals => "Test Signals",
            Self::Stats => "System Statistics",
            Self::Monitor => "Interrupt Monitor",
            Self::AutoEq => "AutoEQ",
        }
    }
}

/// Which region has the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Footer(FooterRow),
    Legend,
    /// The detail region, a tool panel, or Settings: whichever screen is on
    /// top.
    Screen,
}

/// What the shell asks the application to do.
#[derive(Debug, Clone, PartialEq)]
pub enum ShellEvent {
    Select(Selection),
    /// Toggle a channel's curve, by sidebar row.
    ToggleVisible(usize),
    /// A quick-strip feature was toggled (Space), by strip index.
    StripToggle(usize),
    /// A quick-strip item was opened (Enter), by strip index.
    StripOpen(usize),
    OpenTool(Tool),
    CloseTool,
    OpenSettings,
    CloseSettings,
    /// The preset picker moved by one, or asked to open (`None`).
    Preset(Option<i32>),
    Source(Option<i32>),
    VolumeChanged(f64),
    VolumeReset,
    VolumeModeToggle,
    /// Space on the volume row: toggle the user mute.
    VolumeMute,
    /// A command in the shared grammar.
    Command(String),
    /// A screen's session request, to run and answer.
    Session(SessionRequest, ScreenOwner),
    Status(String),
    Palette,
    CommandLine,
    SavePreset,
    DevicePicker,
    Undo,
    Redo,
    Quit,
    ClearClips,
    BypassToggle,
    GraphCursor(Option<f64>),
    GraphZoom(f64),
    GraphPhase,
    GraphHeight,
    GraphPopout,
    Rename(usize),
    CopyParams(usize),
    PasteParams(usize),
    Identify(usize),
}

/// Which screen made a request, so its answer goes back there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenOwner {
    Detail,
    Tool,
    Settings,
}

type Owner = ScreenOwner;

pub struct Shell {
    pub model: ShellModel,
    pub theme: Theme,
    pub focus: Focus,
    pub sidebar_cursor: usize,
    pub sidebar_scroll: usize,
    pub strip_cursor: usize,
    pub legend_cursor: usize,
    pub detail: Box<dyn Screen>,
    pub tool: Option<(Tool, Box<dyn Screen>)>,
    pub settings: Option<Box<dyn Screen>>,
    pub dialog: Option<Dialog>,
    dialog_owner: Owner,
    pub popup: Option<PopupList>,
    popup_owner: Owner,
    pub help: bool,
    pub graph_popout: bool,
    /// The regions of the last frame, for hit testing and tests.
    pub regions: Option<Regions>,
}

const GLOBAL_KEYS: &[KeyHelp] = &[
    KeyHelp::new("Tab", "Next region"),
    KeyHelp::new("Ctrl-P", "Search everything"),
    KeyHelp::new(":", "Command line"),
    KeyHelp::new("M L X P U V G T I B", "Open a tool"),
    KeyHelp::new(",", "Settings"),
    KeyHelp::new("Ctrl-S", "Commit parameters to the preset"),
    KeyHelp::new("Ctrl-D", "Device picker"),
    KeyHelp::new("Ctrl-Z Ctrl-Y", "Undo, redo"),
    KeyHelp::new("= g p + -", "Graph height, pop-out, phase, zoom"),
    KeyHelp::new("b c", "Bypass master EQ, clear clips"),
    KeyHelp::new("q", "Quit"),
];

const SIDEBAR_KEYS: &[KeyHelp] = &[
    KeyHelp::new("↑ ↓", "Select a channel"),
    KeyHelp::new("Enter", "Back to the overview"),
    KeyHelp::new("Space", "Show or hide its curve"),
    KeyHelp::new("r", "Rename"),
    KeyHelp::new("y Y", "Copy, paste parameters"),
    KeyHelp::new("i", "Identify (outputs)"),
];

const FOOTER_KEYS: &[KeyHelp] = &[
    KeyHelp::new("↑ ↓", "Move between rows"),
    KeyHelp::new("← →", "Change"),
    KeyHelp::new("Enter", "Open"),
    KeyHelp::new("Space", "Toggle a feature, or mute"),
    KeyHelp::new("Backspace", "Reset the volume"),
];

const LEGEND_KEYS: &[KeyHelp] = &[
    KeyHelp::new("← →", "Move between channels"),
    KeyHelp::new("Space", "Show or hide"),
    KeyHelp::new("h l", "Move the cursor"),
];

impl Shell {
    pub fn new(model: ShellModel, theme: Theme, detail: Box<dyn Screen>) -> Self {
        let cursor = model.row_of_selection().unwrap_or(0);
        Self {
            model,
            theme,
            focus: Focus::Sidebar,
            sidebar_cursor: cursor,
            sidebar_scroll: 0,
            strip_cursor: 0,
            legend_cursor: 0,
            detail,
            tool: None,
            settings: None,
            dialog: None,
            dialog_owner: Owner::Detail,
            popup: None,
            popup_owner: Owner::Detail,
            help: false,
            graph_popout: false,
            regions: None,
        }
    }

    pub fn open_tool(&mut self, tool: Tool, screen: Box<dyn Screen>) {
        self.tool = Some((tool, screen));
        self.focus = Focus::Screen;
    }

    pub fn close_tool(&mut self) {
        self.tool = None;
        if self.focus == Focus::Screen {
            self.focus = Focus::Sidebar;
        }
    }

    pub fn open_settings(&mut self, screen: Box<dyn Screen>) {
        self.settings = Some(screen);
        self.focus = Focus::Screen;
    }

    pub fn close_settings(&mut self) {
        self.settings = None;
        self.focus = Focus::Sidebar;
    }

    /// The screen on top: Settings, then a tool, then the detail.
    fn top(&mut self) -> (&mut (dyn Screen + 'static), Owner) {
        if let Some(s) = self.settings.as_deref_mut() {
            return (s, Owner::Settings);
        }
        if let Some((_, s)) = self.tool.as_mut() {
            return (s.as_mut(), Owner::Tool);
        }
        (self.detail.as_mut(), Owner::Detail)
    }

    fn screen_by(&mut self, owner: Owner) -> Option<&mut (dyn Screen + 'static)> {
        match owner {
            Owner::Settings => self.settings.as_deref_mut(),
            Owner::Tool => self.tool.as_mut().map(|(_, s)| s.as_mut()),
            Owner::Detail => Some(self.detail.as_mut()),
        }
    }

    /// Handle one key. Returns what the application should do.
    pub fn handle(&mut self, key: KeyEvent, state: &DeviceState) -> Vec<ShellEvent> {
        let mut out = Vec::new();

        if self.help {
            self.help = false;
            return out;
        }

        if let Some(d) = &mut self.dialog {
            if let Some(outcome) = d.handle(key) {
                self.dialog = None;
                let owner = self.dialog_owner;
                if let Some(s) = self.screen_by(owner) {
                    let ev = s.dialog_result(outcome, state);
                    self.absorb(ev, owner, &mut out);
                }
            }
            return out;
        }

        if let Some(p) = &mut self.popup {
            match p.handle(key) {
                Some(Action::Selected(i)) => {
                    self.popup = None;
                    let owner = self.popup_owner;
                    if let Some(s) = self.screen_by(owner) {
                        let ev = s.popup_result(Some(i), state);
                        self.absorb(ev, owner, &mut out);
                    }
                }
                Some(Action::Closed) => {
                    self.popup = None;
                    let owner = self.popup_owner;
                    if let Some(s) = self.screen_by(owner) {
                        let ev = s.popup_result(None, state);
                        self.absorb(ev, owner, &mut out);
                    }
                }
                _ => {}
            }
            return out;
        }

        // Keys that work everywhere, before any screen.
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match (key.code, ctrl) {
            (KeyCode::Char('c'), true) => {
                out.push(ShellEvent::Quit);
                return out;
            }
            (KeyCode::Char('p'), true) => {
                out.push(ShellEvent::Palette);
                return out;
            }
            (KeyCode::Char('s'), true) => {
                // Inside Settings, Ctrl-S is the save bar's Save; the screen
                // gets first refusal and the preset save is the fallback.
                if self.settings.is_some() {
                    let (s, owner) = self.top();
                    let ev = s.handle(key, state);
                    if ev != ScreenEvent::Unhandled {
                        self.absorb(ev, owner, &mut out);
                        return out;
                    }
                }
                out.push(ShellEvent::SavePreset);
                return out;
            }
            (KeyCode::Char('d'), true) => {
                out.push(ShellEvent::DevicePicker);
                return out;
            }
            (KeyCode::Char('z'), true) => {
                out.push(ShellEvent::Undo);
                return out;
            }
            (KeyCode::Char('y'), true) => {
                out.push(ShellEvent::Redo);
                return out;
            }
            (KeyCode::Char('?'), _) => {
                self.help = true;
                return out;
            }
            _ => {}
        }

        // Settings is modal over the main window.
        if self.settings.is_some() {
            if matches!(key.code, KeyCode::Char(',')) {
                out.push(ShellEvent::CloseSettings);
                return out;
            }
            let (s, owner) = self.top();
            let ev = s.handle(key, state);
            if ev == ScreenEvent::Unhandled && key.code == KeyCode::Esc {
                out.push(ShellEvent::CloseSettings);
                return out;
            }
            self.absorb(ev, owner, &mut out);
            return out;
        }

        // The focused screen gets the key first when it has focus.
        if self.focus == Focus::Screen {
            let (s, owner) = self.top();
            let ev = s.handle(key, state);
            if ev != ScreenEvent::Unhandled {
                self.absorb(ev, owner, &mut out);
                return out;
            }
        }

        match key.code {
            KeyCode::Tab => {
                self.focus = self.next_focus();
                return out;
            }
            KeyCode::BackTab => {
                self.focus = self.prev_focus();
                return out;
            }
            KeyCode::Esc => {
                if self.tool.is_some() {
                    out.push(ShellEvent::CloseTool);
                } else if self.focus != Focus::Sidebar {
                    self.focus = Focus::Sidebar;
                } else if self.model.selection != Selection::Overview {
                    out.push(ShellEvent::Select(Selection::Overview));
                }
                return out;
            }
            KeyCode::Char(':') => {
                out.push(ShellEvent::CommandLine);
                return out;
            }
            KeyCode::Char(',') => {
                out.push(ShellEvent::OpenSettings);
                return out;
            }
            KeyCode::Char('q') => {
                out.push(ShellEvent::Quit);
                return out;
            }
            KeyCode::Char(c) if c.is_ascii_uppercase() => {
                if let Some(tool) = Tool::from_key(c) {
                    if self.tool.as_ref().is_some_and(|(t, _)| *t == tool) {
                        out.push(ShellEvent::CloseTool);
                    } else {
                        out.push(ShellEvent::OpenTool(tool));
                    }
                    return out;
                }
            }
            _ => {}
        }

        // Region keys.
        match self.focus {
            Focus::Sidebar => self.handle_sidebar(key, &mut out),
            Focus::Footer(row) => self.handle_footer(row, key, &mut out),
            Focus::Legend => self.handle_legend(key, &mut out),
            Focus::Screen => {}
        }
        if !out.is_empty() {
            return out;
        }

        // Graph keys, when nothing above wanted the key.
        match key.code {
            KeyCode::Char('=') => out.push(ShellEvent::GraphHeight),
            KeyCode::Char('g') => out.push(ShellEvent::GraphPopout),
            KeyCode::Char('p') => out.push(ShellEvent::GraphPhase),
            KeyCode::Char('+') => out.push(ShellEvent::GraphZoom(-10.0)),
            KeyCode::Char('-') => out.push(ShellEvent::GraphZoom(10.0)),
            KeyCode::Char('h') => out.push(self.cursor_step(-1)),
            KeyCode::Char('l') => out.push(self.cursor_step(1)),
            KeyCode::Char('b') => out.push(ShellEvent::BypassToggle),
            KeyCode::Char('c') => out.push(ShellEvent::ClearClips),
            _ => {}
        }
        out
    }

    fn cursor_step(&self, dir: i32) -> ShellEvent {
        let hz = self.model.cursor_hz.unwrap_or(1000.0);
        ShellEvent::GraphCursor(Some(self.model.graph.step_cursor(hz, dir)))
    }

    fn absorb(&mut self, ev: ScreenEvent, owner: Owner, out: &mut Vec<ShellEvent>) {
        match ev {
            ScreenEvent::Unhandled | ScreenEvent::Handled => {}
            ScreenEvent::Popup(p) => {
                self.popup = Some(p);
                self.popup_owner = owner;
            }
            ScreenEvent::Dialog(d) => {
                self.dialog = Some(d);
                self.dialog_owner = owner;
            }
            ScreenEvent::Command(c) => out.push(ShellEvent::Command(c)),
            ScreenEvent::Select(sel) => out.push(ShellEvent::Select(sel)),
            ScreenEvent::Session(req) => out.push(ShellEvent::Session(req, owner)),
            ScreenEvent::Status(s) => out.push(ShellEvent::Status(s)),
            ScreenEvent::Close => match owner {
                Owner::Settings => out.push(ShellEvent::CloseSettings),
                Owner::Tool => out.push(ShellEvent::CloseTool),
                Owner::Detail => {}
            },
        }
    }

    /// Deliver a session reply to the screen that asked, and absorb whatever
    /// it wants next.
    pub fn deliver(
        &mut self,
        owner: ScreenOwner,
        tag: u32,
        reply: SessionReply,
        state: &DeviceState,
    ) -> Vec<ShellEvent> {
        let mut out = Vec::new();
        if let Some(s) = self.screen_by(owner) {
            let ev = s.session_result(tag, reply, state);
            self.absorb(ev, owner, &mut out);
        }
        out
    }

    /// Open a dialog from the application (for example the unsaved-changes
    /// prompt); its outcome goes to the detail screen unless the app reads
    /// it back itself via `take_dialog_outcome`.
    pub fn show_dialog(&mut self, d: Dialog) {
        self.dialog = Some(d);
        self.dialog_owner = Owner::Detail;
    }

    pub fn show_popup(&mut self, p: PopupList) {
        self.popup = Some(p);
        self.popup_owner = Owner::Detail;
    }

    fn next_focus(&self) -> Focus {
        let has_source = self.model.source.is_some();
        match self.focus {
            Focus::Sidebar => Focus::Footer(FooterRow::Strip),
            Focus::Footer(r) => {
                let n = r.next(has_source);
                if n == r {
                    if self.graph_visible() {
                        Focus::Legend
                    } else {
                        Focus::Screen
                    }
                } else {
                    Focus::Footer(n)
                }
            }
            Focus::Legend => Focus::Screen,
            Focus::Screen => Focus::Sidebar,
        }
    }

    fn prev_focus(&self) -> Focus {
        let has_source = self.model.source.is_some();
        match self.focus {
            Focus::Sidebar => Focus::Screen,
            Focus::Footer(r) => {
                let p = r.prev(has_source);
                if p == r {
                    Focus::Sidebar
                } else {
                    Focus::Footer(p)
                }
            }
            Focus::Legend => Focus::Footer(FooterRow::Volume),
            Focus::Screen => {
                if self.graph_visible() {
                    Focus::Legend
                } else {
                    Focus::Footer(FooterRow::Volume)
                }
            }
        }
    }

    fn graph_visible(&self) -> bool {
        self.tool.is_none() && self.model.graph_height != GraphHeight::Hidden
    }

    fn handle_sidebar(&mut self, key: KeyEvent, out: &mut Vec<ShellEvent>) {
        let n = self.model.channel_count();
        if n == 0 {
            return;
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.sidebar_cursor = self.sidebar_cursor.saturating_sub(1);
                out.push(ShellEvent::Select(
                    self.model.selection_of_row(self.sidebar_cursor),
                ));
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.sidebar_cursor = (self.sidebar_cursor + 1).min(n - 1);
                out.push(ShellEvent::Select(
                    self.model.selection_of_row(self.sidebar_cursor),
                ));
            }
            KeyCode::Char(d @ '1'..='8') => {
                let i = d as usize - '1' as usize;
                if i < self.model.inputs.len() {
                    self.sidebar_cursor = i;
                    out.push(ShellEvent::Select(Selection::Input(i)));
                }
            }
            KeyCode::Enter => {
                let here = self.model.selection_of_row(self.sidebar_cursor);
                if self.model.selection == here {
                    out.push(ShellEvent::Select(Selection::Overview));
                } else {
                    out.push(ShellEvent::Select(here));
                }
            }
            KeyCode::Char(' ') => out.push(ShellEvent::ToggleVisible(self.sidebar_cursor)),
            KeyCode::Char('r') => out.push(ShellEvent::Rename(self.sidebar_cursor)),
            KeyCode::Char('y') => out.push(ShellEvent::CopyParams(self.sidebar_cursor)),
            KeyCode::Char('Y') => out.push(ShellEvent::PasteParams(self.sidebar_cursor)),
            KeyCode::Char('i') => {
                if self.sidebar_cursor >= self.model.inputs.len() {
                    out.push(ShellEvent::Identify(self.sidebar_cursor));
                }
            }
            KeyCode::Right => self.focus = Focus::Screen,
            _ => {}
        }
    }

    fn handle_footer(&mut self, row: FooterRow, key: KeyEvent, out: &mut Vec<ShellEvent>) {
        let has_source = self.model.source.is_some();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                let p = row.prev(has_source);
                self.focus = if p == row {
                    Focus::Sidebar
                } else {
                    Focus::Footer(p)
                };
                return;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let n = row.next(has_source);
                self.focus = if n != row {
                    Focus::Footer(n)
                } else {
                    self.next_focus()
                };
                return;
            }
            _ => {}
        }
        match row {
            FooterRow::Strip => match key.code {
                KeyCode::Left => self.strip_cursor = self.strip_cursor.saturating_sub(1),
                KeyCode::Right => {
                    self.strip_cursor =
                        (self.strip_cursor + 1).min(self.model.strip.len().saturating_sub(1))
                }
                KeyCode::Char(' ') => out.push(ShellEvent::StripToggle(self.strip_cursor)),
                KeyCode::Enter => out.push(ShellEvent::StripOpen(self.strip_cursor)),
                _ => {}
            },
            FooterRow::Preset => match key.code {
                KeyCode::Left => out.push(ShellEvent::Preset(Some(-1))),
                KeyCode::Right => out.push(ShellEvent::Preset(Some(1))),
                KeyCode::Enter | KeyCode::Char(' ') => out.push(ShellEvent::Preset(None)),
                _ => {}
            },
            FooterRow::Source => match key.code {
                KeyCode::Left => out.push(ShellEvent::Source(Some(-1))),
                KeyCode::Right => out.push(ShellEvent::Source(Some(1))),
                KeyCode::Enter | KeyCode::Char(' ') => out.push(ShellEvent::Source(None)),
                _ => {}
            },
            FooterRow::Volume => {
                let coarse = key.modifiers.contains(KeyModifiers::SHIFT);
                let step = if coarse { 5.0 } else { 0.5 };
                let floor = if self.model.volume_mode == VolumeMode::Master {
                    -128.0
                } else {
                    -60.0
                };
                match key.code {
                    KeyCode::Left => out.push(ShellEvent::VolumeChanged(
                        (self.model.volume_db - step).max(floor),
                    )),
                    KeyCode::Right => out.push(ShellEvent::VolumeChanged(
                        (self.model.volume_db + step).min(0.0),
                    )),
                    KeyCode::Backspace => out.push(ShellEvent::VolumeReset),
                    KeyCode::Enter => out.push(ShellEvent::VolumeModeToggle),
                    KeyCode::Char(' ') => out.push(ShellEvent::VolumeMute),
                    _ => {}
                }
            }
        }
    }

    fn handle_legend(&mut self, key: KeyEvent, out: &mut Vec<ShellEvent>) {
        let pills = legend(&self.model.curves);
        let row = LegendRow::new(&pills, &self.theme).focused(true, self.legend_cursor);
        match row.handle(key) {
            Some(Action::Selected(i)) => self.legend_cursor = i,
            Some(Action::Chip(i, _)) => out.push(ShellEvent::ToggleVisible(i)),
            _ => {}
        }
    }

    /// Keys for the key line: the focused region's, then the globals that
    /// fit.
    fn key_line(&mut self) -> Vec<KeyHelp> {
        let mut keys: Vec<KeyHelp> = match self.focus {
            Focus::Sidebar => SIDEBAR_KEYS.to_vec(),
            Focus::Footer(_) => FOOTER_KEYS.to_vec(),
            Focus::Legend => LEGEND_KEYS.to_vec(),
            Focus::Screen => {
                let (s, _) = self.top();
                s.keys().to_vec()
            }
        };
        keys.push(KeyHelp::new("Tab", "region"));
        keys.push(KeyHelp::new("?", "help"));
        keys.push(KeyHelp::new(":", "command"));
        keys.push(KeyHelp::new("^P", "search"));
        keys
    }

    pub fn draw(&mut self, area: Rect, buf: &mut Buffer, state: &DeviceState) {
        let t = self.theme.clone();
        let pills = legend(&self.model.curves);
        let pane_w = area
            .width
            .saturating_sub(Density::of(area.width, area.height).sidebar_width() + 2);
        let legend_rows = LegendRow::rows_needed(&pills, pane_w, t.glyphs);
        let Some(regions) = layout::compute(area, self.model.graph_height, legend_rows) else {
            let msg = "Terminal too small: needs 80x24";
            let x = area.x + area.width.saturating_sub(msg.len() as u16) / 2;
            buf.set_string(x, area.y + area.height / 2, msg, t.value());
            self.regions = None;
            return;
        };

        if self.settings.is_some() {
            self.draw_settings(&regions, buf, &t, state);
        } else {
            self.draw_title(regions.title, buf, &t);
            self.draw_sidebar(&regions, buf, &t);
            self.draw_pane(&regions, buf, &t, state);
            self.draw_echo(regions.echo, buf, &t, state);
        }
        self.draw_keys(regions.keys, buf, &t);

        if let Some(p) = &self.popup {
            let (w, h) = p.size(area.width.saturating_sub(4), area.height.saturating_sub(4));
            let r = Rect::new(
                area.x + (area.width - w) / 2,
                area.y + (area.height - h) / 2,
                w,
                h,
            );
            p.draw(r, buf, &t);
        }
        if let Some(d) = &self.dialog {
            let r = d.size(area);
            d.draw(r, buf, &t);
        }
        if self.help {
            let title = match self.focus {
                Focus::Sidebar => "Channels".to_string(),
                Focus::Footer(_) => "Preset, source and volume".to_string(),
                Focus::Legend => "Graph".to_string(),
                Focus::Screen => {
                    let (s, _) = self.top();
                    s.title()
                }
            };
            let region_keys: Vec<KeyHelp> = match self.focus {
                Focus::Sidebar => SIDEBAR_KEYS.to_vec(),
                Focus::Footer(_) => FOOTER_KEYS.to_vec(),
                Focus::Legend => LEGEND_KEYS.to_vec(),
                Focus::Screen => {
                    let (s, _) = self.top();
                    s.keys().to_vec()
                }
            };
            let overlay = HelpOverlay::new(&title, &t)
                .section("Here", &region_keys)
                .section("Everywhere", GLOBAL_KEYS);
            let r = overlay.size(area);
            overlay.render(r, buf);
        }
        self.regions = Some(regions);
    }

    fn draw_title(&self, area: Rect, buf: &mut Buffer, t: &Theme) {
        let m = &self.model;
        let left = format!(
            " DSPi  {} · fw {} · {}",
            m.platform, m.firmware, m.serial_short
        );
        buf.set_string(
            area.x,
            area.y,
            truncate(&left, area.width as usize),
            t.title(),
        );
        let dot = match (m.connected, t.glyphs) {
            (true, Glyphs::Ascii) => "*",
            (false, Glyphs::Ascii) => "x",
            _ => "●",
        };
        let conn = if !m.connected && m.devices.is_empty() {
            "No Devices".to_string()
        } else if m.connected {
            "Connected".to_string()
        } else {
            "Not connected".to_string()
        };
        let preset = format!(
            "Preset {}{}",
            m.preset_label,
            if m.preset_dirty { " *" } else { "" }
        );
        let right = format!("{dot} {conn}   {preset} ");
        let rw = right.chars().count() as u16;
        if area.width > rw + left.chars().count() as u16 + 2 {
            let x = area.x + area.width - rw;
            buf.set_string(
                x,
                area.y,
                dot,
                Style::default().fg(if m.connected { t.ok } else { t.danger }),
            );
            buf.set_string(x + 2, area.y, &conn, t.value());
            buf.set_string(
                x + 2 + conn.len() as u16 + 3,
                area.y,
                &preset,
                if m.preset_dirty {
                    t.warning_style()
                } else {
                    t.value()
                },
            );
        }
    }

    fn block(&self, title: &str, focused: bool, t: &Theme) -> Block<'static> {
        let style = if focused {
            Style::default().fg(t.accent)
        } else {
            t.chrome_style()
        };
        let mut b = Block::default()
            .borders(Borders::ALL)
            .border_type(if t.glyphs == Glyphs::Ascii {
                BorderType::Plain
            } else {
                BorderType::Rounded
            })
            .border_style(style);
        if !title.is_empty() {
            b = b.title(format!(" {title} ")).title_style(if focused {
                t.focused()
            } else {
                t.section()
            });
        }
        b
    }

    fn draw_sidebar(&mut self, r: &Regions, buf: &mut Buffer, t: &Theme) {
        let focused = matches!(self.focus, Focus::Sidebar | Focus::Footer(_));
        self.block("", focused, t).render(r.sidebar, buf);
        self.sidebar_scroll = sidebar::scroll_for(
            &self.model,
            self.sidebar_cursor,
            r.sidebar_list.height,
            self.sidebar_scroll,
        );
        sidebar::draw_list(
            r.sidebar_list,
            buf,
            &self.model,
            t,
            self.focus == Focus::Sidebar,
            self.sidebar_cursor,
            self.sidebar_scroll,
        );
        let footer_focus = match self.focus {
            Focus::Footer(row) => Some(row),
            _ => None,
        };
        sidebar::draw_footer(
            r.sidebar_footer,
            buf,
            &self.model,
            t,
            footer_focus,
            self.strip_cursor,
        );
    }

    fn draw_pane(&mut self, r: &Regions, buf: &mut Buffer, t: &Theme, state: &DeviceState) {
        let screen_focused = self.focus == Focus::Screen;
        if let Some((tool, screen)) = self.tool.as_mut() {
            let title = format!("{}   {} closes", tool.title(), tool.key());
            let block = Self::block_static(&title, screen_focused, t);
            let inner = block.inner(r.pane);
            block.render(r.pane, buf);
            screen.draw(inner, buf, t, state, screen_focused);
            return;
        }

        let title = if self.graph_popout || r.graph.height > 0 {
            "Filter Response"
        } else {
            ""
        };
        let focused = screen_focused || self.focus == Focus::Legend;
        let mut block = self.block(title, focused, t);
        if !title.is_empty() {
            // The Console's pop-out button; `g` here.
            block = block.title_top(
                ratatui::text::Line::from(if t.glyphs == Glyphs::Ascii {
                    " g "
                } else {
                    " ⤢ g "
                })
                .right_aligned()
                .style(t.label()),
            );
        }
        block.render(r.pane, buf);

        let inner = layout::inset(r.pane);
        if self.graph_popout {
            // The pop-out: the graph fills the pane with its legend beneath.
            let graph_area = Rect::new(
                inner.x,
                inner.y,
                inner.width,
                inner.height.saturating_sub(1),
            );
            let legend_area = Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1);
            self.draw_graph(graph_area, legend_area, buf, t);
            return;
        }

        if r.graph.height > 0 {
            self.draw_graph(r.graph, r.legend, buf, t);
            // A rule between the graph and the detail.
            let y = r.detail.y;
            if y > r.legend.y && y < r.pane.y + r.pane.height - 1 {
                // The legend row doubles as the divider region; nothing to draw.
            }
        }
        let detail_focused = screen_focused;
        let title = self.detail.title();
        let mut detail = r.detail;
        if r.graph.height > 0 && detail.height > 1 {
            // Title row for the detail region, in the selected channel's
            // colour when a channel is selected.
            let color = self.model.selected_item().map(|c| c.color).unwrap_or(t.fg);
            let dot = if t.glyphs == Glyphs::Ascii {
                "*"
            } else {
                "●"
            };
            let divider = if t.glyphs == Glyphs::Ascii {
                "-"
            } else {
                "─"
            };
            buf.set_string(
                detail.x,
                detail.y,
                divider.repeat(detail.width as usize),
                t.chrome_style(),
            );
            let text = format!(" {dot} {title} ");
            buf.set_string(
                detail.x + 1,
                detail.y,
                &text,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            );
            detail = Rect::new(detail.x, detail.y + 1, detail.width, detail.height - 1);
        }
        self.detail.draw(detail, buf, t, state, detail_focused);
    }

    fn block_static(title: &str, focused: bool, t: &Theme) -> Block<'static> {
        let style = if focused {
            Style::default().fg(t.accent)
        } else {
            t.chrome_style()
        };
        Block::default()
            .borders(Borders::ALL)
            .border_type(if t.glyphs == Glyphs::Ascii {
                BorderType::Plain
            } else {
                BorderType::Rounded
            })
            .border_style(style)
            .title(format!(" {title} "))
            .title_style(if focused { t.focused() } else { t.section() })
    }

    fn draw_graph(&self, graph_area: Rect, legend_area: Rect, buf: &mut Buffer, t: &Theme) {
        let m = &self.model;
        Graph::new(&m.curves, &m.graph, t)
            .cursor(m.cursor_hz)
            .marker(m.marker_hz)
            .render(graph_area, buf);
        let pills = legend(&m.curves);
        let mut row =
            LegendRow::new(&pills, t).focused(self.focus == Focus::Legend, self.legend_cursor);
        if m.graph.show_phase {
            row = row.trailing("φ phase");
        }
        row.render(legend_area, buf);
    }

    fn draw_settings(&mut self, r: &Regions, buf: &mut Buffer, t: &Theme, state: &DeviceState) {
        let title = " Settings";
        buf.set_string(r.title.x, r.title.y, title, t.title());
        let back = "Esc back ";
        buf.set_string(
            r.title.x + r.title.width - back.len() as u16,
            r.title.y,
            back,
            t.label(),
        );
        let body = Rect::new(
            r.title.x,
            r.title.y + 1,
            r.title.width,
            r.echo.y - r.title.y,
        );
        if let Some(s) = self.settings.as_deref_mut() {
            s.draw(body, buf, t, state, true);
        }
    }

    fn draw_echo(&mut self, area: Rect, buf: &mut Buffer, t: &Theme, state: &DeviceState) {
        // The focused screen's actions sit on the right, the Console's
        // footer strip; the echo text keeps whatever is left.
        let actions = if self.focus == Focus::Screen {
            let (s, _) = self.top();
            s.actions(state)
        } else {
            Vec::new()
        };
        let strip_w: u16 = actions
            .iter()
            .map(|(l, _)| {
                if l == "|" {
                    1
                } else {
                    l.chars().count() as u16 + 2
                }
            })
            .sum();
        if strip_w > 0 && area.width > strip_w + 12 {
            let mut x = area.x + area.width - strip_w - 1;
            for (label, enabled) in &actions {
                if label == "|" {
                    buf.set_string(x, area.y, "│", t.chrome_style());
                    x += 1;
                } else {
                    let text = format!(" {label} ");
                    buf.set_string(
                        x,
                        area.y,
                        &text,
                        if *enabled { t.value() } else { t.label() },
                    );
                    x += text.chars().count() as u16;
                }
            }
        }
        let text = match &self.model.status {
            Some(s) => s.clone(),
            None => self.model.echo.clone(),
        };
        let style = if self.model.status.is_some() {
            t.value()
        } else {
            t.label()
        };
        let room = (area.width as usize)
            .saturating_sub(2 + if strip_w > 0 { strip_w as usize + 2 } else { 0 });
        buf.set_string(area.x + 1, area.y, truncate(&text, room), style);
    }

    fn draw_keys(&mut self, area: Rect, buf: &mut Buffer, t: &Theme) {
        let keys = self.key_line();
        let mut x = area.x + 1;
        for k in keys {
            let text = format!("{} {}", k.key, k.does);
            let w = text.chars().count() as u16 + 3;
            if x + w > area.x + area.width {
                break;
            }
            buf.set_string(x, area.y, k.key, t.focused());
            buf.set_string(
                x + k.key.chars().count() as u16 + 1,
                area.y,
                k.does,
                t.label(),
            );
            x += w;
            if x < area.x + area.width {
                buf.set_string(x - 2, area.y, "·", t.chrome_style());
            }
        }
        let _ = fit_left;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ColorDepth, Glyphs};
    use crate::widgets::testing::key;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn shell(w: u16, h: u16) -> (Shell, Terminal<TestBackend>) {
        let theme = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let model = fixture::rp2350(&theme);
        let shell = Shell::new(model, theme, Box::new(Placeholder::new("FL", "Input page")));
        let term = Terminal::new(TestBackend::new(w, h)).unwrap();
        (shell, term)
    }

    fn frame(shell: &mut Shell, term: &mut Terminal<TestBackend>) -> String {
        let state = fixture::state();
        term.draw(|f| shell.draw(f.area(), f.buffer_mut(), &state))
            .unwrap();
        let buf = term.backend().buffer();
        let a = *buf.area();
        (0..a.height)
            .map(|y| {
                (0..a.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_reference_frame_has_every_region() {
        let (mut s, mut term) = shell(120, 40);
        let f = frame(&mut s, &mut term);
        let lines: Vec<&str> = f.lines().collect();
        assert!(
            lines[0].starts_with(" DSPi  RP2350 · fw 1.1.6 · A1B2C3D4"),
            "{:?}",
            lines[0]
        );
        assert!(
            lines[0].contains("● Connected   Preset 3: Living Room *"),
            "{:?}",
            lines[0]
        );
        assert!(lines[2].contains("INPUTS"), "{:?}", lines[2]);
        assert!(
            lines[3].contains("▍▪FL") && lines[3].contains("IN1"),
            "{:?}",
            lines[3]
        );
        assert!(f.contains("OUTPUTS"));
        assert!(f.contains("Sub"));
        assert!(f.contains("● IN1"), "legend pills");
        assert!(f.contains("● FL"), "detail title in the selected channel");
        assert!(f.contains("Preset ‹3: Living"), "{f}");
        assert!(f.contains("Volume User"), "{f}");
        assert!(f.contains("C0"), "{f}");
        assert!(
            lines[38].contains(":eq in.1 3 freq 2856"),
            "echo: {:?}",
            lines[38]
        );
        assert!(
            lines[39].contains("Select a channel"),
            "keys: {:?}",
            lines[39]
        );
        assert_eq!(lines.len(), 40);
    }

    #[test]
    fn the_minimum_frame_still_fits_and_smaller_says_so() {
        let (mut s, mut term) = shell(80, 24);
        let f = frame(&mut s, &mut term);
        assert_eq!(f.lines().count(), 24);
        assert!(f.contains("INPUTS") && f.contains("OUTPUTS"));
        let (mut s, mut term) = shell(70, 20);
        let f = frame(&mut s, &mut term);
        assert!(f.contains("Terminal too small"));
    }

    #[test]
    fn the_wide_frame_uses_the_wide_density() {
        let (mut s, mut term) = shell(200, 60);
        let _ = frame(&mut s, &mut term);
        assert_eq!(s.regions.as_ref().unwrap().density, Density::Wide);
    }

    #[test]
    fn arrows_in_the_sidebar_select_and_enter_returns_to_the_overview() {
        let (mut s, _) = shell(120, 40);
        assert_eq!(
            s.handle(key(KeyCode::Down), &fixture::state()),
            vec![ShellEvent::Select(Selection::Input(1))]
        );
        s.model.selection = Selection::Input(1);
        assert_eq!(
            s.handle(key(KeyCode::Enter), &fixture::state()),
            vec![ShellEvent::Select(Selection::Overview)]
        );
        assert_eq!(
            s.handle(key(KeyCode::Char(' ')), &fixture::state()),
            vec![ShellEvent::ToggleVisible(1)]
        );
        for _ in 0..20 {
            s.handle(key(KeyCode::Down), &fixture::state());
        }
        assert_eq!(s.sidebar_cursor, 16, "clamped to the last output");
        assert_eq!(
            s.handle(key(KeyCode::Char('i')), &fixture::state()),
            vec![ShellEvent::Identify(16)]
        );
    }

    #[test]
    fn tab_walks_the_regions_and_escape_walks_back() {
        let (mut s, _) = shell(120, 40);
        s.handle(key(KeyCode::Tab), &fixture::state());
        assert_eq!(s.focus, Focus::Footer(FooterRow::Strip));
        s.handle(key(KeyCode::Tab), &fixture::state());
        assert_eq!(s.focus, Focus::Footer(FooterRow::Preset));
        s.handle(key(KeyCode::Tab), &fixture::state());
        assert_eq!(s.focus, Focus::Footer(FooterRow::Source));
        s.handle(key(KeyCode::Tab), &fixture::state());
        assert_eq!(s.focus, Focus::Footer(FooterRow::Volume));
        s.handle(key(KeyCode::Tab), &fixture::state());
        assert_eq!(s.focus, Focus::Legend);
        s.handle(key(KeyCode::Tab), &fixture::state());
        assert_eq!(s.focus, Focus::Screen);
        s.handle(key(KeyCode::Tab), &fixture::state());
        assert_eq!(s.focus, Focus::Sidebar);
        s.focus = Focus::Screen;
        assert_eq!(s.handle(key(KeyCode::Esc), &fixture::state()), vec![]);
        assert_eq!(s.focus, Focus::Sidebar);
        assert_eq!(
            s.handle(key(KeyCode::Esc), &fixture::state()),
            vec![ShellEvent::Select(Selection::Overview)]
        );
    }

    #[test]
    fn the_footer_rows_report_their_changes() {
        let (mut s, _) = shell(120, 40);
        s.focus = Focus::Footer(FooterRow::Strip);
        s.strip_cursor = 1;
        assert_eq!(
            s.handle(key(KeyCode::Char(' ')), &fixture::state()),
            vec![ShellEvent::StripToggle(1)]
        );
        assert_eq!(
            s.handle(key(KeyCode::Enter), &fixture::state()),
            vec![ShellEvent::StripOpen(1)]
        );
        s.focus = Focus::Footer(FooterRow::Preset);
        assert_eq!(
            s.handle(key(KeyCode::Right), &fixture::state()),
            vec![ShellEvent::Preset(Some(1))]
        );
        assert_eq!(
            s.handle(key(KeyCode::Enter), &fixture::state()),
            vec![ShellEvent::Preset(None)]
        );
        s.focus = Focus::Footer(FooterRow::Volume);
        assert_eq!(
            s.handle(key(KeyCode::Left), &fixture::state()),
            vec![ShellEvent::VolumeChanged(-12.5)]
        );
        assert_eq!(
            s.handle(key(KeyCode::Backspace), &fixture::state()),
            vec![ShellEvent::VolumeReset]
        );
        assert_eq!(
            s.handle(key(KeyCode::Enter), &fixture::state()),
            vec![ShellEvent::VolumeModeToggle]
        );
    }

    #[test]
    fn uppercase_letters_open_the_consoles_tools_and_again_closes_them() {
        let (mut s, mut term) = shell(120, 40);
        assert_eq!(
            s.handle(key(KeyCode::Char('M')), &fixture::state()),
            vec![ShellEvent::OpenTool(Tool::Matrix)]
        );
        s.open_tool(Tool::Matrix, Box::new(Placeholder::new("Matrix Mixer", "")));
        assert_eq!(s.focus, Focus::Screen);
        let f = frame(&mut s, &mut term);
        assert!(f.contains(" Matrix Mixer   M closes "), "{f}");
        assert!(
            !f.contains("● IN1"),
            "the graph is gone while a tool is open"
        );
        assert_eq!(
            s.handle(key(KeyCode::Char('M')), &fixture::state()),
            vec![ShellEvent::CloseTool]
        );
        assert_eq!(
            s.handle(key(KeyCode::Esc), &fixture::state()),
            vec![ShellEvent::CloseTool]
        );
        assert_eq!(
            s.handle(key(KeyCode::Char('L')), &fixture::state()),
            vec![ShellEvent::OpenTool(Tool::Loudness)]
        );
        assert_eq!(
            s.handle(key(KeyCode::Char('Q')), &fixture::state()),
            vec![],
            "not a tool"
        );
    }

    #[test]
    fn settings_takes_the_whole_screen_and_escape_leaves_it() {
        let (mut s, mut term) = shell(120, 40);
        assert_eq!(
            s.handle(key(KeyCode::Char(',')), &fixture::state()),
            vec![ShellEvent::OpenSettings]
        );
        s.open_settings(Box::new(Placeholder::new("Global Parameters", "")));
        let f = frame(&mut s, &mut term);
        assert!(f.lines().next().unwrap().starts_with(" Settings"), "{f}");
        assert!(!f.contains("INPUTS"), "{f}");
        assert_eq!(
            s.handle(key(KeyCode::Esc), &fixture::state()),
            vec![ShellEvent::CloseSettings]
        );
        assert_eq!(
            s.handle(key(KeyCode::Char(',')), &fixture::state()),
            vec![ShellEvent::CloseSettings]
        );
    }

    #[test]
    fn global_keys_work_from_anywhere() {
        let (mut s, _) = shell(120, 40);
        let ctrl = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        assert_eq!(
            s.handle(ctrl('p'), &fixture::state()),
            vec![ShellEvent::Palette]
        );
        assert_eq!(
            s.handle(key(KeyCode::Char(':')), &fixture::state()),
            vec![ShellEvent::CommandLine]
        );
        assert_eq!(
            s.handle(ctrl('s'), &fixture::state()),
            vec![ShellEvent::SavePreset]
        );
        assert_eq!(
            s.handle(ctrl('z'), &fixture::state()),
            vec![ShellEvent::Undo]
        );
        assert_eq!(
            s.handle(key(KeyCode::Char('q')), &fixture::state()),
            vec![ShellEvent::Quit]
        );
        assert_eq!(
            s.handle(key(KeyCode::Char('=')), &fixture::state()),
            vec![ShellEvent::GraphHeight]
        );
        assert_eq!(
            s.handle(key(KeyCode::Char('p')), &fixture::state()),
            vec![ShellEvent::GraphPhase]
        );
        assert_eq!(
            s.handle(key(KeyCode::Char('b')), &fixture::state()),
            vec![ShellEvent::BypassToggle]
        );
        assert_eq!(
            s.handle(key(KeyCode::Char('c')), &fixture::state()),
            vec![ShellEvent::ClearClips]
        );
        assert_eq!(
            s.handle(key(KeyCode::Char('+')), &fixture::state()),
            vec![ShellEvent::GraphZoom(-10.0)]
        );
        match s
            .handle(key(KeyCode::Char('l')), &fixture::state())
            .as_slice()
        {
            [ShellEvent::GraphCursor(Some(hz))] => assert!(*hz > 1000.0),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn help_opens_and_any_key_closes_it() {
        let (mut s, mut term) = shell(120, 40);
        s.handle(key(KeyCode::Char('?')), &fixture::state());
        assert!(s.help);
        let f = frame(&mut s, &mut term);
        assert!(
            f.contains("EVERYWHERE") && f.contains("Search everything"),
            "{f}"
        );
        s.handle(key(KeyCode::Char('x')), &fixture::state());
        assert!(!s.help);
    }

    #[test]
    fn a_dialog_swallows_keys_and_returns_its_outcome_to_its_owner() {
        let (mut s, mut term) = shell(120, 40);
        s.show_dialog(Dialog::confirm(
            "Unsaved Changes",
            "Save before continuing?",
            vec![
                crate::widgets::Button::new("Save"),
                crate::widgets::Button::new("Cancel"),
            ],
        ));
        let f = frame(&mut s, &mut term);
        assert!(f.contains(" Unsaved Changes "), "{f}");
        assert_eq!(
            s.handle(key(KeyCode::Char('q')), &fixture::state()),
            vec![],
            "q goes to the dialog, not quit"
        );
        s.handle(key(KeyCode::Esc), &fixture::state());
        assert!(s.dialog.is_none());
    }

    /// Turn a key-line token into the key events it stands for.
    fn keys_for(token: &str) -> Vec<KeyEvent> {
        token
            .split_whitespace()
            .map(|k| match k {
                "↑" => key(KeyCode::Up),
                "↓" => key(KeyCode::Down),
                "←" => key(KeyCode::Left),
                "→" => key(KeyCode::Right),
                "Enter" => key(KeyCode::Enter),
                "Space" => key(KeyCode::Char(' ')),
                "Tab" => key(KeyCode::Tab),
                "Backspace" => key(KeyCode::Backspace),
                "Esc" => key(KeyCode::Esc),
                "Ctrl-P" => KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
                "Ctrl-S" => KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
                "Ctrl-D" => KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
                "Ctrl-Z" => KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL),
                "Ctrl-Y" => KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL),
                other => {
                    let c = other.chars().next().unwrap();
                    assert_eq!(other.chars().count(), 1, "unknown key token {other}");
                    key(KeyCode::Char(c))
                }
            })
            .collect()
    }

    /// A key that is advertised must do something: produce an event, move
    /// focus, or open an overlay. This is the test the old interface lacked,
    /// where the Surfaces panel advertised four keys it did not bind.
    #[test]
    fn every_advertised_key_is_handled() {
        let regions: Vec<(Focus, &[KeyHelp])> = vec![
            (Focus::Sidebar, SIDEBAR_KEYS),
            (Focus::Footer(FooterRow::Volume), FOOTER_KEYS),
            (Focus::Legend, LEGEND_KEYS),
            (Focus::Sidebar, GLOBAL_KEYS),
        ];
        for (focus, keys) in regions {
            for help in keys {
                for k in keys_for(help.key) {
                    let (mut s, _) = shell(120, 40);
                    s.focus = focus;
                    // Start every cursor mid-range so a boundary no-op is not
                    // mistaken for a missing binding.
                    s.sidebar_cursor = 10;
                    s.model.selection = Selection::Output(2);
                    s.strip_cursor = 1;
                    s.legend_cursor = 3;
                    let before = (s.focus, s.help, s.legend_cursor, s.strip_cursor);
                    let events = s.handle(k, &fixture::state());
                    let after = (s.focus, s.help, s.legend_cursor, s.strip_cursor);
                    assert!(
                        !events.is_empty() || before != after,
                        "{:?} does nothing in {:?} (from {:?})",
                        k.code,
                        focus,
                        help.key
                    );
                }
            }
        }
    }

    #[test]
    fn the_rp2040_fixture_draws_its_five_outputs() {
        let theme = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let model = fixture::rp2040(&theme);
        let mut s = Shell::new(model, theme, Box::new(Placeholder::new("FL", "")));
        let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let f = frame(&mut s, &mut term);
        assert!(f.contains("OUT5") && !f.contains("OUT9"), "{f}");
        assert!(f.contains("RP2040"));
    }
}
