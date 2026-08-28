//! Settings: the Console's Settings window, full screen.
//!
//! A grouped sidebar on the left listing only the pages this device can offer,
//! a scrolling column of rows on the right, and one save bar along the bottom
//! that appears while anything is unsaved. `docs/plan/DESIGN.md` 7.13 is the
//! layout and `docs/plan/survey-console.md` 2.24 to 2.33 is the content, whose
//! strings are copied rather than paraphrased.
//!
//! Three things are worth knowing before reading a page:
//!
//! - **Rows are data.** A page builds a `Vec<(Item, Row)>` from the live state
//!   every frame and never keeps a copy of a device value. The shell draws the
//!   rows, owns the cursor and the scroll, and hands a widget's [`Action`] back
//!   to the page with the row's own `Item`, so a page only ever writes the
//!   `match` that turns a gesture into a command.
//! - **Global Parameters is staged, the hardware pages are live.** That is the
//!   Console's split: the flash directory is written once on Save, while pins,
//!   types and clocks apply to RAM as they are edited and are flashed by
//!   `dev.save.io` (survey 2.28 and 2.29).
//! - **The save bar has three dirty categories** (survey 2.29): the global
//!   draft, live output-config edits not yet flashed (only while the output
//!   config mode is independent), and the Control Surfaces live preview, which
//!   the device itself reports and Phase 7B fills in.

pub mod about;
pub mod config;
pub mod global;
pub mod overview;

use crossterm::event::{KeyCode, KeyEvent};
use dspi_proto::Platform;
use dspi_proto::packets::{
    CtrlIfaceStatus, I2cCtrlConfig, PresetDirectory, PresetStartup, SpdifInputConfig,
    UartCtrlConfig,
};
use dspi_session::{DeviceState, PinClaim, Session};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::widgets::{Block, BorderType, Borders, Widget};

use crate::shell::{Screen, ScreenEvent};
use crate::theme::{Glyphs, Theme};
use crate::widgets::text::{fit_left, truncate, wrap};
use crate::widgets::{
    Action, Banner, BannerKind, Dialog, DialogOutcome, KeyHelp, ParamRow, PickerRow, PinCell,
    PinGrid, PopupList, SaveBar, SectionHeader, StatusPill, StatusTone, ToggleRow,
};

pub use config::AppConfig;

// ---------------------------------------------------------------------------
// What the pages read that the bulk packet does not carry
// ---------------------------------------------------------------------------

/// The device facts Settings needs that are not in `DeviceState`.
///
/// The preset directory, the startup slot and the two control-interface
/// configurations have no bulk representation, so they cost a round trip each.
/// [`SettingsData::read`] takes them all at once when Settings opens; until it
/// has run, every field is `None` and the pages say so rather than inventing a
/// value.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SettingsData {
    pub directory: Option<PresetDirectory>,
    pub startup: Option<PresetStartup>,
    pub uart: Option<UartCtrlConfig>,
    pub i2c: Option<I2cCtrlConfig>,
    pub iface: Option<CtrlIfaceStatus>,
    pub spdif: Option<SpdifInputConfig>,
    /// Who owns each GPIO, from `PinMap::build`. Falls back to the claims the
    /// bulk packet alone can prove when it has not been read.
    pub claims: Option<Vec<PinClaim>>,
    /// Preset slot names, for the Default Preset picker.
    pub preset_names: Vec<String>,
    /// The device's own Control Surfaces unsaved flag (survey 3.5). Phase 7B
    /// sets it; until then Settings never shows the control-surface category.
    pub cs_dirty: bool,
}

impl SettingsData {
    /// Read everything Settings needs in one go.
    ///
    /// Every read is optional: a stall means this firmware does not have the
    /// feature, which the pages present as an absent section rather than an
    /// error.
    pub fn read(session: &mut Session) -> Self {
        use dspi_proto::generated::opcodes as op;
        let get = |session: &mut Session, opcode: u8, len: u16| -> Option<Vec<u8>> {
            session
                .with_transport(|t| t.control_in(opcode, 0, len))
                .ok()
        };
        let directory = get(
            session,
            op::REQ_PRESET_GET_DIR,
            PresetDirectory::SIZE as u16,
        )
        .and_then(|d| PresetDirectory::decode(&d).ok());
        let startup = get(
            session,
            op::REQ_PRESET_GET_STARTUP,
            PresetStartup::SIZE as u16,
        )
        .and_then(|d| PresetStartup::decode(&d).ok());
        let uart = get(
            session,
            op::REQ_GET_UART_CONFIG,
            UartCtrlConfig::SIZE as u16,
        )
        .and_then(|d| UartCtrlConfig::decode(&d).ok());
        let i2c = get(session, op::REQ_GET_I2C_CONFIG, I2cCtrlConfig::SIZE as u16)
            .and_then(|d| I2cCtrlConfig::decode(&d).ok());
        let iface = get(
            session,
            op::REQ_GET_CTRL_IFACE_STATUS,
            CtrlIfaceStatus::SIZE as u16,
        )
        .and_then(|d| CtrlIfaceStatus::decode(&d).ok());
        let spdif = get(
            session,
            op::REQ_GET_SPDIF_INPUT_CONFIG,
            SpdifInputConfig::SIZE as u16,
        )
        .and_then(|d| SpdifInputConfig::decode(&d).ok());
        let claims = dspi_session::PinMap::build(session)
            .ok()
            .map(|m| m.claims().to_vec());
        let preset_names = (0..10u8)
            .map(|slot| match session.read("preset.name", &[slot]) {
                Ok(dspi_proto::value::Value::Text(t)) => t.trim().to_string(),
                _ => String::new(),
            })
            .collect();
        Self {
            directory,
            startup,
            uart,
            i2c,
            iface,
            spdif,
            claims,
            preset_names,
            cs_dirty: false,
        }
    }

    /// 0 independent, 1 with preset. The Console's `presetOutputConfigMode`;
    /// 1 is the firmware's default, so an unread directory reads as 1.
    pub fn output_config_mode(&self) -> u8 {
        self.directory
            .as_ref()
            .map(|d| d.output_config_mode)
            .unwrap_or(1)
    }

    /// 0 independent, 1 with preset; 0 is the firmware's default.
    pub fn master_volume_mode(&self) -> u8 {
        self.directory
            .as_ref()
            .map(|d| d.master_volume_mode)
            .unwrap_or(0)
    }

    pub fn startup_mode(&self) -> u8 {
        self.startup
            .as_ref()
            .map(|s| s.mode)
            .or_else(|| self.directory.as_ref().map(|d| d.startup_mode))
            .unwrap_or(0)
    }

    pub fn default_slot(&self) -> u8 {
        self.startup
            .as_ref()
            .map(|s| s.default_slot)
            .or_else(|| self.directory.as_ref().map(|d| d.default_slot))
            .unwrap_or(0)
    }

    pub fn occupied(&self, slot: u8) -> bool {
        self.directory.as_ref().is_some_and(|d| d.is_occupied(slot))
    }

    /// Who owns each GPIO: the device's own answer when Settings has read it,
    /// and otherwise the claims the bulk snapshot alone can prove.
    pub fn pin_claims(&self, state: &DeviceState) -> Vec<PinClaim> {
        match &self.claims {
            Some(c) => c.clone(),
            None => overview::claims_from_state(state),
        }
    }

    /// The owner of `gpio`, ignoring one consumer's own claim, as the Console's
    /// `pinInUseBy(_:excluding:)` does.
    pub fn owner_of(&self, state: &DeviceState, gpio: u8, excluding: &str) -> Option<String> {
        self.pin_claims(state)
            .into_iter()
            .find(|c| c.gpio == gpio && c.owner != excluding)
            .map(|c| c.owner)
    }
}

/// Everything a page reads while it builds its rows.
pub(crate) struct Cx<'a> {
    pub state: &'a DeviceState,
    pub data: &'a SettingsData,
    pub config: &'a AppConfig,
    pub connected: bool,
    /// Whether the staged global draft differs from the device, which is what
    /// disables the DAC mute test.
    pub global_dirty: bool,
}

impl Cx<'_> {
    /// Whether a probed feature is present.
    ///
    /// A device that was never probed (no rows at all) is assumed to have
    /// everything, which is what keeps the fixtures and the gallery showing
    /// every page; a probe that ran and said no hides the section, which is the
    /// Console's "absent features are removed, not disabled".
    pub fn feature(&self, name: &str) -> bool {
        let f = &self.state.caps.features;
        f.is_empty() || f.iter().any(|x| x.name == name && x.present)
    }

    pub fn platform(&self) -> Platform {
        self.state.caps.platform
    }

    /// GPIOs that nothing else holds, keeping `current` so a picker can always
    /// render its own value.
    pub fn free_pins(&self, excluding: &str, current: Option<u8>) -> Vec<u8> {
        dspi_session::pins::valid_pins(self.platform())
            .into_iter()
            .filter(|p| {
                Some(*p) == current || self.data.owner_of(self.state, *p, excluding).is_none()
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// The row kit
// ---------------------------------------------------------------------------

/// One line (or block) of a Settings page.
///
/// A page never draws; it describes. That keeps the eleven pages free of
/// layout arithmetic and means the scroll, the cursor and the focus ring are
/// written once.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Row {
    Section(String),
    Blank,
    /// A dim paragraph: a section footer or an explanatory note.
    Note(String),
    Banner(BannerKind, String, String),
    /// A read-only `label ....... value` line.
    Pair(String, String),
    /// An inline result row, the Console's `statusRow`.
    Status(String, bool),
    Toggle {
        label: String,
        on: bool,
        caption: Option<String>,
        enabled: bool,
    },
    Pick {
        label: String,
        choices: Vec<String>,
        selected: usize,
        caption: Option<String>,
        enabled: bool,
    },
    Number {
        label: String,
        value: f64,
        min: f64,
        max: f64,
        step: f64,
        unit: String,
        decimals: usize,
        caption: Option<String>,
        enabled: bool,
    },
    /// A row of one or more buttons, right aligned, with an optional label and
    /// caption on the left.
    Buttons {
        label: String,
        caption: Option<String>,
        buttons: Vec<String>,
        /// Which button the row's `Action::Selected` will name.
        cursor: usize,
        enabled: bool,
    },
    /// A pill on the right of a labelled line: ` Active ` and friends.
    Pill {
        label: String,
        text: String,
        tone: StatusTone,
        caption: Option<String>,
    },
    /// The GPIO map. `←`/`→` walk it and the echo line names the owner.
    Grid {
        cells: Vec<PinCell>,
        cursor: usize,
    },
}

impl Row {
    pub fn note(text: impl Into<String>) -> Self {
        Self::Note(text.into())
    }
    pub fn section(text: impl Into<String>) -> Self {
        Self::Section(text.into())
    }
    pub fn pair(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self::Pair(label.into(), value.into())
    }
    pub fn warning(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self::Banner(BannerKind::Warning, title.into(), body.into())
    }

    pub fn focusable(&self) -> bool {
        matches!(
            self,
            Row::Toggle { .. }
                | Row::Pick { .. }
                | Row::Number { .. }
                | Row::Buttons { .. }
                | Row::Grid { .. }
        )
    }

    fn caption_lines(caption: &Option<String>, width: u16) -> u16 {
        caption
            .as_ref()
            .map(|c| wrap(c, width.saturating_sub(2) as usize, 3).len() as u16)
            .unwrap_or(0)
    }

    pub fn height(&self, width: u16) -> u16 {
        match self {
            Row::Blank => 1,
            Row::Section(_) => 1,
            Row::Note(t) => wrap(t, width.saturating_sub(2) as usize, 4).len() as u16,
            Row::Banner(_, _, body) => {
                1 + wrap(body, width.saturating_sub(3) as usize, 3).len() as u16
            }
            Row::Pair(..) | Row::Status(..) => 1,
            Row::Toggle { caption, .. } | Row::Pick { caption, .. } => {
                1 + Self::caption_lines(caption, width)
            }
            Row::Pill { caption, .. } => 1 + Self::caption_lines(caption, width),
            Row::Buttons { caption, .. } => 1 + Self::caption_lines(caption, width),
            Row::Number { caption, .. } => 2 + Self::caption_lines(caption, width),
            Row::Grid { cells, .. } => (cells.len() as u16).div_ceil(9),
        }
    }

    fn draw(&self, area: Rect, buf: &mut Buffer, theme: &Theme, focused: bool) {
        match self {
            Row::Blank => {}
            Row::Section(t) => SectionHeader::new(t, theme).render(area, buf),
            Row::Note(t) => {
                for (i, line) in wrap(t, area.width.saturating_sub(2) as usize, 4)
                    .iter()
                    .enumerate()
                {
                    if i as u16 >= area.height {
                        break;
                    }
                    buf.set_string(area.x + 1, area.y + i as u16, line, theme.label());
                }
            }
            Row::Banner(kind, title, body) => {
                let b = match kind {
                    BannerKind::Warning => Banner::warning(title, theme),
                    BannerKind::Info => Banner::info(title, theme),
                    BannerKind::Error => Banner::error(title, theme),
                };
                b.body(body).render(area, buf);
            }
            Row::Pair(label, value) => {
                buf.set_string(
                    area.x + 1,
                    area.y,
                    truncate(label, area.width as usize),
                    theme.label(),
                );
                let vx = area
                    .x
                    .saturating_add(area.width)
                    .saturating_sub(value.chars().count() as u16);
                buf.set_string(vx, area.y, value, theme.value());
            }
            Row::Status(text, is_error) => {
                let glyph = match (is_error, theme.glyphs) {
                    (true, Glyphs::Ascii) => "!",
                    (false, Glyphs::Ascii) => "+",
                    (true, _) => "▲",
                    (false, _) => "✓",
                };
                let style = if *is_error {
                    theme.warning_style()
                } else {
                    ratatui::style::Style::default().fg(theme.ok)
                };
                buf.set_string(area.x + 1, area.y, glyph, style);
                buf.set_string(
                    area.x + 3,
                    area.y,
                    truncate(text, area.width.saturating_sub(4) as usize),
                    if *is_error {
                        theme.warning_style()
                    } else {
                        theme.label()
                    },
                );
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
                if let Some(c) = caption {
                    t = t.caption(c);
                }
                t.render(area, buf);
            }
            Row::Pick {
                label,
                choices,
                selected,
                caption,
                enabled,
            } => {
                let refs: Vec<&str> = choices.iter().map(String::as_str).collect();
                let mut p = PickerRow::new(label, &refs, *selected, theme)
                    .focused(focused)
                    .enabled(*enabled);
                if let Some(c) = caption {
                    p = p.caption(c);
                }
                p.render(area, buf);
            }
            Row::Number {
                label,
                value,
                min,
                max,
                step,
                unit,
                decimals,
                caption,
                enabled,
            } => {
                let mut r = ParamRow::new(label, *value, *min, *max, unit, theme)
                    .step(*step)
                    .decimals(*decimals)
                    .focused(focused)
                    .enabled(*enabled);
                if let Some(c) = caption {
                    r = r.caption(c);
                }
                r.render(area, buf);
            }
            Row::Buttons {
                label,
                caption,
                buttons,
                cursor,
                enabled,
            } => {
                buf.set_string(
                    area.x,
                    area.y,
                    if focused { "▸" } else { " " },
                    theme.focused(),
                );
                let width: usize = buttons.iter().map(|b| b.chars().count() + 3).sum();
                buf.set_string(
                    area.x + 1,
                    area.y,
                    truncate(label, (area.width as usize).saturating_sub(width + 3)),
                    if *enabled {
                        theme.value()
                    } else {
                        theme.label()
                    },
                );
                let mut x = area.x + area.width - width as u16;
                for (i, b) in buttons.iter().enumerate() {
                    let text = format!(" {b} ");
                    let style = if !*enabled {
                        theme.label()
                    } else if focused && i == *cursor {
                        theme.pill(theme.accent).add_modifier(Modifier::BOLD)
                    } else {
                        theme.focused()
                    };
                    buf.set_string(x, area.y, &text, style);
                    x += text.chars().count() as u16 + 1;
                }
                if let Some(c) = caption
                    && area.height >= 2
                {
                    for (i, line) in wrap(c, area.width.saturating_sub(2) as usize, 3)
                        .iter()
                        .enumerate()
                    {
                        buf.set_string(area.x + 1, area.y + 1 + i as u16, line, theme.label());
                    }
                }
            }
            Row::Pill {
                label,
                text,
                tone,
                caption,
            } => {
                buf.set_string(
                    area.x + 1,
                    area.y,
                    truncate(label, area.width as usize),
                    theme.value(),
                );
                let pill = StatusPill::new(text, *tone, theme);
                let w = pill.width();
                if area.width > w + 2 {
                    pill.render(Rect::new(area.x + area.width - w, area.y, w, 1), buf);
                }
                if let Some(c) = caption
                    && area.height >= 2
                {
                    for (i, line) in wrap(c, area.width.saturating_sub(2) as usize, 3)
                        .iter()
                        .enumerate()
                    {
                        buf.set_string(area.x + 1, area.y + 1 + i as u16, line, theme.label());
                    }
                }
            }
            Row::Grid { cells, cursor } => {
                PinGrid::new(cells, theme)
                    .focused(focused, *cursor)
                    .render(area, buf);
            }
        }
    }

    /// The widget's own key handling, so a page never re-implements arrows.
    fn handle(&self, key: KeyEvent, theme: &Theme) -> Option<Action> {
        match self {
            Row::Toggle {
                label, on, enabled, ..
            } => ToggleRow::new(label, *on, theme)
                .enabled(*enabled)
                .handle(key),
            Row::Pick {
                label,
                choices,
                selected,
                enabled,
                ..
            } => {
                let refs: Vec<&str> = choices.iter().map(String::as_str).collect();
                PickerRow::new(label, &refs, *selected, theme)
                    .enabled(*enabled)
                    .handle(key)
            }
            Row::Number {
                label,
                value,
                min,
                max,
                step,
                unit,
                enabled,
                ..
            } => ParamRow::new(label, *value, *min, *max, unit, theme)
                .step(*step)
                .enabled(*enabled)
                .handle(key),
            Row::Buttons {
                buttons,
                cursor,
                enabled,
                ..
            } => {
                if !*enabled {
                    return None;
                }
                match key.code {
                    KeyCode::Left => Some(Action::Selected(cursor.saturating_sub(1))),
                    KeyCode::Right => Some(Action::Selected(
                        (cursor + 1).min(buttons.len().saturating_sub(1)),
                    )),
                    KeyCode::Enter | KeyCode::Char(' ') => Some(Action::Open),
                    _ => None,
                }
            }
            Row::Grid { cells, cursor } => match key.code {
                KeyCode::Left => Some(Action::Selected(cursor.saturating_sub(1))),
                KeyCode::Right => Some(Action::Selected(
                    (cursor + 1).min(cells.len().saturating_sub(1)),
                )),
                _ => None,
            },
            _ => None,
        }
    }
}

/// What a page wants after a key.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PageEvent {
    Handled,
    Unhandled,
    /// A device write in the shared grammar.
    Command(String),
    /// A device write that is an output-config edit, so it marks the second
    /// dirty category while the output config mode is independent.
    IoCommand(String),
    Status(String),
    Dialog(Dialog),
    Popup(PopupList),
    /// The app-side settings file changed and should be written out.
    Config(Box<AppConfig>),
}

/// A Settings page.
pub(crate) trait SettingsPage {
    /// The rows, rebuilt from the live state.
    fn rows(&self, cx: &Cx<'_>) -> Vec<Row>;
    /// A widget action on the focusable row at `index`.
    fn act(&mut self, index: usize, action: Action, cx: &Cx<'_>) -> PageEvent;
    fn cursor(&self) -> usize;
    fn set_cursor(&mut self, i: usize);
    fn keys(&self) -> &'static [KeyHelp] {
        PAGE_KEYS
    }
    fn dialog_result(&mut self, _outcome: DialogOutcome, _cx: &Cx<'_>) -> PageEvent {
        PageEvent::Handled
    }
    fn popup_result(&mut self, _choice: Option<usize>, _cx: &Cx<'_>) -> PageEvent {
        PageEvent::Handled
    }
    /// A key the row widgets did not want.
    fn key(&mut self, _key: KeyEvent, _cx: &Cx<'_>) -> PageEvent {
        PageEvent::Unhandled
    }
}

pub(crate) const PAGE_KEYS: &[KeyHelp] = &[
    KeyHelp::new("↑ ↓", "Move between controls"),
    KeyHelp::new("← →", "Change"),
    KeyHelp::new("Enter", "Open or activate"),
    KeyHelp::new("Space", "Toggle"),
    KeyHelp::new("Tab", "Sidebar, page, save bar"),
    KeyHelp::new("[ ]", "Back, forward"),
    KeyHelp::new("Esc", "Leave Settings"),
];

// ---------------------------------------------------------------------------
// Pages and the sidebar
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    About,
    Advanced,
    Graphing,
    Overview,
    Inputs,
    Outputs,
    I2s,
    Global,
    Surfaces,
    Interfaces,
    Groups,
    Macros,
}

impl Page {
    /// The Console's page title.
    pub fn title(self) -> &'static str {
        match self {
            Page::About => "About",
            Page::Advanced => "Advanced",
            Page::Graphing => "Graphing",
            Page::Overview => "Overview",
            Page::Inputs => "Inputs",
            Page::Outputs => "Outputs",
            Page::I2s => "I2S Configuration",
            Page::Global => "Global Parameters",
            Page::Surfaces => "Control Surfaces",
            Page::Interfaces => "Control Interfaces",
            Page::Groups => "Channel Groups",
            Page::Macros => "Macros",
        }
    }

    /// The sidebar's label, shortened where the column cannot hold the title.
    pub fn short(self) -> &'static str {
        match self {
            Page::I2s => "I2S Config.",
            Page::Global => "Global Params",
            Page::Surfaces => "Control Surf.",
            Page::Interfaces => "Control Interf.",
            other => other.title(),
        }
    }
}

/// The Console's four sidebar groups, in display order.
const GROUPS: [(&str, &[Page]); 4] = [
    ("Application", &[Page::About, Page::Advanced]),
    ("Display", &[Page::Graphing]),
    (
        "System",
        &[
            Page::Overview,
            Page::Inputs,
            Page::Outputs,
            Page::I2s,
            Page::Global,
        ],
    ),
    (
        "Control",
        &[Page::Surfaces, Page::Interfaces, Page::Groups, Page::Macros],
    ),
];

/// Whether this device can offer a page (survey section 1, the availability
/// column). A page it cannot is not drawn dim; it is not there.
pub fn available(page: Page, state: &DeviceState, connected: bool) -> bool {
    let feature = |name: &str| {
        let f = &state.caps.features;
        f.is_empty() || f.iter().any(|x| x.name == name && x.present)
    };
    match page {
        // The I2S clock pins are an RP mux concern; a platform we do not know
        // is assumed not to have them, which is the Console's STM32 rule.
        Page::I2s => matches!(state.caps.platform, Platform::Rp2040 | Platform::Rp2350),
        Page::Inputs => {
            feature("spdif_multi_input")
                || feature("i2s_input_channels")
                || feature("adat_input")
                || feature("lg_sound_sync")
        }
        Page::Interfaces => feature("uart_control") || feature("i2c_control"),
        // Shown while disconnected too: the page carries its own placeholder.
        Page::Surfaces => state.caps.cs.is_some() || !connected,
        Page::Groups => state.caps.cs.as_ref().is_some_and(|c| c.max_groups > 0),
        Page::Macros => state.caps.cs.as_ref().is_some_and(|c| c.max_macros > 0),
        _ => true,
    }
}

/// One sidebar line: a group header or a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Entry {
    Group(&'static str),
    Page(Page),
}

fn sidebar_entries(state: &DeviceState, connected: bool) -> Vec<Entry> {
    let mut out = Vec::new();
    for (title, pages) in GROUPS {
        let shown: Vec<Page> = pages
            .iter()
            .copied()
            .filter(|p| available(*p, state, connected))
            .collect();
        if shown.is_empty() {
            continue;
        }
        out.push(Entry::Group(title));
        out.extend(shown.into_iter().map(Entry::Page));
    }
    out
}

// ---------------------------------------------------------------------------
// The output-config baseline
// ---------------------------------------------------------------------------

/// The Console's `OutputConfigSnapshot`: the device-global wiring as it stood
/// before the first live edit, so Revert can put it back.
#[derive(Debug, Clone, PartialEq)]
pub struct IoSnapshot {
    output_pins: Vec<u8>,
    output_types: [u8; 4],
    bck_pin: u8,
    bck_pin_slave: u8,
    clock_pin_mode: u8,
    mck_enabled: bool,
    mck_pin: u8,
    mck_multiplier: u8,
    adat_enabled: bool,
    adat_pin: u8,
    spdif_pins: [u8; 4],
    spdif_enabled: [bool; 3],
    i2s_rx_pins: Vec<u8>,
    i2s_channels: u8,
    i2s_rate_hz: u32,
    i2s_clock_mode: u8,
    adat_in_enabled: bool,
    adat_in_pin: Option<u8>,
    adat_in_clock_mode: u8,
}

/// The rate codes the bulk packet's `i2s_input_rate` stands for
/// (bulk_params.h:207).
pub const INPUT_RATES_HZ: [u32; 3] = [44_100, 48_000, 96_000];

impl IoSnapshot {
    pub fn capture(state: &DeviceState) -> Self {
        let i2s = state.i2s();
        let (adat_enabled, adat_pin) = state.adat_output();
        let input = state.input_config();
        let mut spdif_pins = [0u8; 4];
        let mut spdif_enabled = [false; 3];
        let mut i2s_rx_pins = Vec::new();
        let mut i2s_channels = 2;
        let mut i2s_rate_hz = 48_000;
        let mut i2s_clock_mode = 0;
        let mut adat_in_enabled = false;
        let mut adat_in_pin = None;
        let mut adat_in_clock_mode = 0;
        if let Some(c) = &input {
            spdif_pins[0] = c.spdif_rx_pin;
            for (i, p) in c.spdif_rx_pin_ext.iter().enumerate() {
                spdif_pins[i + 1] = p.unwrap_or(0);
            }
            let mask = c.spdif_rx_enabled_ext.unwrap_or(0);
            for (i, e) in spdif_enabled.iter_mut().enumerate() {
                *e = mask & (1u8 << i) != 0;
            }
            i2s_rx_pins.push(c.i2s_rx_pin);
            i2s_rx_pins.extend(c.i2s_rx_pin_ext.iter().map(|p| p.unwrap_or(0)));
            i2s_channels = c.i2s_input_channels.unwrap_or(2);
            i2s_rate_hz = INPUT_RATES_HZ
                .get(c.i2s_input_rate as usize)
                .copied()
                .unwrap_or(48_000);
            i2s_clock_mode = c.i2s_clock_mode;
            adat_in_enabled = c.adat_input_enabled.unwrap_or(false);
            adat_in_pin = c.adat_input_pin;
            adat_in_clock_mode = c.adat_clock_mode.unwrap_or(0);
        }
        Self {
            output_pins: state.output_pins(),
            output_types: i2s.output_types,
            bck_pin: i2s.bck_pin,
            bck_pin_slave: i2s.bck_pin_slave,
            clock_pin_mode: i2s.clock_pin_mode.unwrap_or(0),
            mck_enabled: i2s.mck_enabled,
            mck_pin: i2s.mck_pin,
            mck_multiplier: i2s.mck_multiplier,
            adat_enabled,
            adat_pin,
            spdif_pins,
            spdif_enabled,
            i2s_rx_pins,
            i2s_channels,
            i2s_rate_hz,
            i2s_clock_mode,
            adat_in_enabled,
            adat_in_pin,
            adat_in_clock_mode,
        }
    }

    /// The writes that put the baseline back, in the Console's order: types
    /// then pins, clocks, S/PDIF (disable before repinning, enable after), the
    /// I2S input, then ADAT.
    ///
    /// Indices are the wire's own, zero based, which is what the command
    /// grammar takes.
    pub fn restore_commands(&self, now: &Self) -> Vec<String> {
        let mut out = Vec::new();
        for (slot, t) in self.output_types.iter().enumerate() {
            if now.output_types.get(slot) != Some(t) {
                out.push(format!(
                    "out.type {slot} {}",
                    if *t == 1 { "i2s" } else { "spdif" }
                ));
            }
        }
        for (i, p) in self.output_pins.iter().enumerate() {
            if now.output_pins.get(i) != Some(p) {
                out.push(format!("out.pin {i} {p}"));
            }
        }
        if self.bck_pin != now.bck_pin {
            out.push(format!("i2s.bck {}", self.bck_pin));
        }
        if self.mck_enabled != now.mck_enabled {
            out.push(format!(
                "i2s.mck {}",
                if self.mck_enabled { "on" } else { "off" }
            ));
        }
        if self.mck_pin != now.mck_pin {
            out.push(format!("i2s.mck.pin {}", self.mck_pin));
        }
        if self.mck_multiplier != now.mck_multiplier {
            out.push(format!(
                "i2s.mck.mult {}",
                if self.mck_multiplier == 1 {
                    "256x"
                } else {
                    "128x"
                }
            ));
        }
        if self.spdif_pins[0] != now.spdif_pins[0] {
            out.push(format!("in.spdif.pin 0 {}", self.spdif_pins[0]));
        }
        for i in 0..3 {
            // Index 1 is S/PDIF 2: the enable opcode only takes the optional
            // inputs (config.h:454).
            let idx = i + 1;
            if now.spdif_enabled[i] && !self.spdif_enabled[i] {
                out.push(format!("in.spdif.enable {idx} off"));
            }
            if self.spdif_pins[i + 1] != now.spdif_pins[i + 1] {
                out.push(format!("in.spdif.pin {idx} {}", self.spdif_pins[i + 1]));
            }
            if !now.spdif_enabled[i] && self.spdif_enabled[i] {
                out.push(format!("in.spdif.enable {idx} on"));
            }
        }
        if self.i2s_channels != now.i2s_channels {
            out.push(format!("in.i2s.channels {}", self.i2s_channels));
        }
        for (pair, p) in self.i2s_rx_pins.iter().enumerate() {
            if now.i2s_rx_pins.get(pair) != Some(p) {
                out.push(format!("in.i2s.pin {pair} {p}"));
            }
        }
        if self.i2s_rate_hz != now.i2s_rate_hz {
            out.push(format!("in.rate {}", self.i2s_rate_hz));
        }
        if self.i2s_clock_mode != now.i2s_clock_mode {
            out.push(format!(
                "in.i2s.clock {}",
                if self.i2s_clock_mode == 1 {
                    "slave"
                } else {
                    "master"
                }
            ));
        }
        // The Console restores the slave BCK pair before the clock-pin mode, so
        // re-entering split finds a valid pair. There is no registry path for
        // `REQ_SET_I2S_BCK_PIN` with role 1 (`i2s.bck` is role 0 only), so the
        // slave pair cannot be restored from here; see the phase report.
        if self.clock_pin_mode != now.clock_pin_mode {
            out.push(format!(
                "i2s.clockpins {}",
                if self.clock_pin_mode == 1 {
                    "split"
                } else {
                    "unified"
                }
            ));
        }
        if self.adat_pin != now.adat_pin {
            out.push(format!("adat.pin {}", self.adat_pin));
        }
        if self.adat_enabled != now.adat_enabled {
            out.push(format!(
                "adat.enable {}",
                if self.adat_enabled { "on" } else { "off" }
            ));
        }
        if self.adat_in_pin != now.adat_in_pin
            && let Some(p) = self.adat_in_pin
        {
            out.push(format!("in.adat.pin {p}"));
        }
        if self.adat_in_enabled != now.adat_in_enabled {
            out.push(format!(
                "in.adat.enable {}",
                if self.adat_in_enabled { "on" } else { "off" }
            ));
        }
        if self.adat_in_clock_mode != now.adat_in_clock_mode {
            out.push(format!(
                "in.adat.clock {}",
                if self.adat_in_clock_mode == 1 {
                    "slave"
                } else {
                    "master"
                }
            ));
        }
        out
    }
}

// ---------------------------------------------------------------------------
// The screen
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Sidebar,
    Page,
    SaveBar,
}

/// Which overlay this screen is waiting on, so its result reaches the right
/// place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    Page,
    Revert,
}

struct Pages {
    about: about::AboutPage,
    overview: overview::OverviewPage,
    global: global::GlobalPage,
    /// The three Phase 7B pages share one placeholder.
    later: LaterPage,
}

/// A page Phase 7B fills in.
pub(crate) struct LaterPage {
    title: &'static str,
}

impl SettingsPage for LaterPage {
    fn rows(&self, _cx: &Cx<'_>) -> Vec<Row> {
        vec![
            Row::section(self.title),
            Row::Blank,
            Row::note("Control surfaces arrive with Phase 7B."),
        ]
    }
    fn act(&mut self, _index: usize, _action: Action, _cx: &Cx<'_>) -> PageEvent {
        PageEvent::Handled
    }
    fn cursor(&self) -> usize {
        0
    }
    fn set_cursor(&mut self, _i: usize) {}
    fn keys(&self) -> &'static [KeyHelp] {
        PAGE_KEYS
    }
}

pub struct SettingsScreen {
    data: SettingsData,
    config: AppConfig,
    page: Page,
    sidebar_cursor: usize,
    sidebar_scroll: usize,
    page_scroll: u16,
    history: Vec<Page>,
    hpos: usize,
    focus: Focus,
    save_cursor: usize,
    pages: Pages,
    io_baseline: Option<IoSnapshot>,
    io_dirty: bool,
    pending: Option<Pending>,
    /// True while the device is there. Settings is the one screen that has to
    /// work without one, since About and Graphing need nothing.
    connected: bool,
    /// Where the app-side settings go. `None` is the platform's own location;
    /// tests point it somewhere disposable so they never touch a real one.
    config_path: Option<std::path::PathBuf>,
}

impl SettingsScreen {
    pub fn new(state: &DeviceState, data: SettingsData, config: AppConfig) -> Self {
        let mut s = Self {
            pages: Pages {
                about: about::AboutPage,
                overview: overview::OverviewPage::default(),
                global: global::GlobalPage::new(state, &data),
                later: LaterPage { title: "" },
            },
            data,
            config,
            page: Page::About,
            sidebar_cursor: 0,
            sidebar_scroll: 0,
            page_scroll: 0,
            history: vec![Page::About],
            hpos: 0,
            focus: Focus::Sidebar,
            save_cursor: 1,
            io_baseline: None,
            io_dirty: false,
            pending: None,
            connected: true,
            config_path: None,
        };
        s.select_row_for(state);
        s
    }

    pub fn connected(mut self, c: bool) -> Self {
        self.connected = c;
        self
    }

    /// Write the app-side settings here rather than to the platform's own
    /// location.
    pub fn config_path(mut self, path: std::path::PathBuf) -> Self {
        self.config_path = Some(path);
        self
    }

    /// Open on a named page, which is what `--settings <page>` and a later
    /// deep link need.
    pub fn open(mut self, page: Page, state: &DeviceState) -> Self {
        if available(page, state, self.connected) {
            self.page = page;
            self.history = vec![page];
            self.hpos = 0;
            self.select_row_for(state);
        }
        self
    }

    pub fn page(&self) -> Page {
        self.page
    }

    /// The page name a `--settings` argument stands for.
    pub fn page_from_name(name: &str) -> Option<Page> {
        Some(
            match name.to_ascii_lowercase().replace([' ', '-', '_'], "") {
                n if n == "about" => Page::About,
                n if n == "advanced" => Page::Advanced,
                n if n == "graphing" => Page::Graphing,
                n if n == "overview" || n == "pins" => Page::Overview,
                n if n == "inputs" => Page::Inputs,
                n if n == "outputs" => Page::Outputs,
                n if n == "i2s" || n == "i2sconfiguration" => Page::I2s,
                n if n == "global" || n == "globalparameters" => Page::Global,
                n if n == "surfaces" || n == "controlsurfaces" => Page::Surfaces,
                n if n == "interfaces" || n == "controlinterfaces" => Page::Interfaces,
                n if n == "groups" || n == "channelgroups" => Page::Groups,
                n if n == "macros" => Page::Macros,
                _ => return None,
            },
        )
    }

    fn entries(&self, state: &DeviceState) -> Vec<Entry> {
        sidebar_entries(state, self.connected)
    }

    fn select_row_for(&mut self, state: &DeviceState) {
        if let Some(i) = self
            .entries(state)
            .iter()
            .position(|e| *e == Entry::Page(self.page))
        {
            self.sidebar_cursor = i;
        }
    }

    fn cx<'a>(&'a self, state: &'a DeviceState) -> Cx<'a> {
        Cx {
            state,
            data: &self.data,
            config: &self.config,
            connected: self.connected,
            global_dirty: self.pages.global.dirty(state, &self.data),
        }
    }

    fn current(&mut self) -> &mut dyn SettingsPage {
        match self.page {
            Page::About => &mut self.pages.about,
            Page::Overview => &mut self.pages.overview,
            Page::Global => &mut self.pages.global,
            Page::Surfaces | Page::Groups | Page::Macros | Page::Advanced | Page::Graphing | Page::Inputs | Page::Outputs | Page::I2s | Page::Interfaces => {
                self.pages.later.title = self.page.title();
                &mut self.pages.later
            }
        }
    }

    fn current_ref(&self) -> &dyn SettingsPage {
        match self.page {
            Page::About => &self.pages.about,
            Page::Overview => &self.pages.overview,
            Page::Global => &self.pages.global,
            Page::Surfaces | Page::Groups | Page::Macros | Page::Advanced | Page::Graphing | Page::Inputs | Page::Outputs | Page::I2s | Page::Interfaces => &self.pages.later,
        }
    }

    // -- dirty state, the Console's three categories ------------------------

    fn global_dirty(&self, state: &DeviceState) -> bool {
        self.pages.global.dirty(state, &self.data)
    }

    /// Live output-config edits count only while the wiring is device-global;
    /// with-preset mode saves them with the preset instead (survey 2.29).
    fn output_dirty(&self) -> bool {
        self.io_dirty && self.data.output_config_mode() == 0
    }

    fn cs_dirty(&self) -> bool {
        self.data.cs_dirty
    }

    pub fn dirty(&self, state: &DeviceState) -> bool {
        self.global_dirty(state) || self.output_dirty() || self.cs_dirty()
    }

    /// The bar's subtitle: the Console says so when the only pending change is
    /// already live on the device.
    fn save_subtitle(&self, state: &DeviceState) -> &'static str {
        if self.cs_dirty() && !self.global_dirty(state) && !self.output_dirty() {
            SaveBar::CS_ONLY
        } else {
            SaveBar::FLASH
        }
    }

    /// Note that a page made a live output-config edit, capturing the baseline
    /// on the first one (the Console's `beginOutputEdit`).
    fn begin_output_edit(&mut self, state: &DeviceState) {
        if self.data.output_config_mode() != 0 {
            return;
        }
        if !self.io_dirty {
            self.io_baseline = Some(IoSnapshot::capture(state));
            self.io_dirty = true;
        }
    }

    /// Save: the global draft's four writes, then the output-config flash,
    /// then the control-surface flash (survey 2.29).
    fn save(&mut self, state: &DeviceState) -> ScreenEvent {
        let mut lines = Vec::new();
        if self.global_dirty(state) {
            lines.extend(self.pages.global.save_commands(state, &self.data));
        }
        if self.output_dirty() {
            lines.push("dev.save.io".to_string());
        }
        if self.cs_dirty() {
            lines.push("cs.save".to_string());
        }
        self.pages.global.mark_saved();
        self.io_dirty = false;
        self.io_baseline = None;
        if lines.is_empty() {
            return ScreenEvent::Handled;
        }
        ScreenEvent::Command(lines.join("\n"))
    }

    /// Revert: drop the global draft, put the captured wiring back, and tell
    /// the device to drop its control-surface preview.
    fn revert(&mut self, state: &DeviceState) -> ScreenEvent {
        let mut lines = Vec::new();
        self.pages.global.revert();
        if self.output_dirty()
            && let Some(base) = self.io_baseline.take()
        {
            lines.extend(base.restore_commands(&IoSnapshot::capture(state)));
        }
        if self.cs_dirty() {
            lines.push("cs.revert".to_string());
        }
        self.io_dirty = false;
        if lines.is_empty() {
            return ScreenEvent::Status("Reverted".into());
        }
        ScreenEvent::Command(lines.join("\n"))
    }

    // -- navigation ---------------------------------------------------------

    fn go(&mut self, page: Page, state: &DeviceState) {
        if page == self.page {
            return;
        }
        self.page = page;
        self.page_scroll = 0;
        self.history.truncate(self.hpos + 1);
        self.history.push(page);
        self.hpos = self.history.len() - 1;
        self.select_row_for(state);
    }

    fn back(&mut self, state: &DeviceState) -> bool {
        if self.hpos == 0 {
            return false;
        }
        self.hpos -= 1;
        self.page = self.history[self.hpos];
        self.page_scroll = 0;
        self.select_row_for(state);
        true
    }

    fn forward(&mut self, state: &DeviceState) -> bool {
        if self.hpos + 1 >= self.history.len() {
            return false;
        }
        self.hpos += 1;
        self.page = self.history[self.hpos];
        self.page_scroll = 0;
        self.select_row_for(state);
        true
    }

    fn next_focus(&mut self, state: &DeviceState) {
        self.focus = match self.focus {
            Focus::Sidebar => Focus::Page,
            Focus::Page if self.dirty(state) => Focus::SaveBar,
            Focus::Page => Focus::Sidebar,
            Focus::SaveBar => Focus::Sidebar,
        };
    }

    fn absorb(&mut self, ev: PageEvent, state: &DeviceState) -> ScreenEvent {
        match ev {
            PageEvent::Handled => ScreenEvent::Handled,
            PageEvent::Unhandled => ScreenEvent::Unhandled,
            PageEvent::Command(c) => ScreenEvent::Command(c),
            PageEvent::IoCommand(c) => {
                self.begin_output_edit(state);
                ScreenEvent::Command(c)
            }
            PageEvent::Status(s) => ScreenEvent::Status(s),
            PageEvent::Dialog(d) => {
                self.pending = Some(Pending::Page);
                ScreenEvent::Dialog(d)
            }
            PageEvent::Popup(p) => {
                self.pending = Some(Pending::Page);
                ScreenEvent::Popup(p)
            }
            PageEvent::Config(c) => {
                self.config = *c;
                let result = match &self.config_path {
                    Some(p) => self.config.save_to(p).map(|()| p.clone()),
                    None => self.config.save(),
                };
                match result {
                    Ok(p) => ScreenEvent::Status(format!("Saved {}", p.display())),
                    Err(e) => ScreenEvent::Status(format!("Could not save settings: {e}")),
                }
            }
        }
    }

    // -- drawing ------------------------------------------------------------

    fn draw_sidebar(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme, state: &DeviceState) {
        let entries = self.entries(state);
        let focused = self.focus == Focus::Sidebar;
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(if theme.glyphs == Glyphs::Ascii {
                BorderType::Plain
            } else {
                BorderType::Rounded
            })
            .border_style(if focused {
                ratatui::style::Style::default().fg(theme.accent)
            } else {
                theme.chrome_style()
            });
        let inner = block.inner(area);
        block.render(area, buf);
        if inner.height == 0 {
            return;
        }
        let rows = inner.height as usize;
        if self.sidebar_cursor < self.sidebar_scroll {
            self.sidebar_scroll = self.sidebar_cursor;
        } else if self.sidebar_cursor >= self.sidebar_scroll + rows {
            self.sidebar_scroll = self.sidebar_cursor + 1 - rows;
        }
        for (i, e) in entries
            .iter()
            .enumerate()
            .skip(self.sidebar_scroll)
            .take(rows)
        {
            let y = inner.y + (i - self.sidebar_scroll) as u16;
            match e {
                Entry::Group(t) => {
                    buf.set_string(
                        inner.x,
                        y,
                        truncate(&t.to_uppercase(), inner.width as usize),
                        theme.section(),
                    );
                }
                Entry::Page(p) => {
                    let here = *p == self.page;
                    let marker = if focused && i == self.sidebar_cursor {
                        "▸"
                    } else if here {
                        "▍"
                    } else {
                        " "
                    };
                    let style = if here { theme.focused() } else { theme.value() };
                    buf.set_string(
                        inner.x,
                        y,
                        marker,
                        ratatui::style::Style::default().fg(theme.accent),
                    );
                    buf.set_string(
                        inner.x + 2,
                        y,
                        fit_left(p.short(), inner.width.saturating_sub(2) as usize),
                        style,
                    );
                }
            }
        }
    }

    /// Draw the page's rows, scrolled so the cursor is visible.
    fn draw_page(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme, state: &DeviceState) {
        let cx = self.cx(state);
        let rows = self.current_ref().rows(&cx);
        let cursor = self.current_ref().cursor();
        let focused = self.focus == Focus::Page;
        let heights: Vec<u16> = rows.iter().map(|r| r.height(area.width)).collect();
        let focusable: Vec<usize> = (0..rows.len()).filter(|i| rows[*i].focusable()).collect();
        let cursor_row = focusable.get(cursor).copied();

        // Keep the focused row on screen.
        if let Some(cr) = cursor_row {
            let top: u16 = heights[..cr].iter().sum();
            let bottom = top + heights[cr];
            if top < self.page_scroll {
                self.page_scroll = top;
            } else if bottom > self.page_scroll + area.height {
                self.page_scroll = bottom.saturating_sub(area.height);
            }
        }
        let total: u16 = heights.iter().sum();
        self.page_scroll = self.page_scroll.min(total.saturating_sub(area.height));

        let mut y = 0u16;
        for (i, row) in rows.iter().enumerate() {
            let h = heights[i];
            if y + h > self.page_scroll && y < self.page_scroll + area.height {
                let top = y.saturating_sub(self.page_scroll);
                let visible = h.min(area.height.saturating_sub(top));
                if visible > 0 {
                    let r = Rect::new(area.x, area.y + top, area.width, visible);
                    row.draw(r, buf, theme, focused && cursor_row == Some(i));
                }
            }
            y += h;
        }
    }
}

impl Screen for SettingsScreen {
    fn title(&self) -> String {
        self.page.title().to_string()
    }

    fn draw(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        state: &DeviceState,
        _focused: bool,
    ) {
        if area.height < 4 || area.width < 40 {
            return;
        }
        let dirty = self.dirty(state);
        let body_h = area.height.saturating_sub(u16::from(dirty));
        let side_w = if area.width >= 100 { 20 } else { 18 };
        let sidebar = Rect::new(area.x, area.y, side_w, body_h);
        self.draw_sidebar(sidebar, buf, theme, state);

        let pane = Rect::new(area.x + side_w, area.y, area.width - side_w, body_h);
        let focused = self.focus == Focus::Page;
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(if theme.glyphs == Glyphs::Ascii {
                BorderType::Plain
            } else {
                BorderType::Rounded
            })
            .border_style(if focused {
                ratatui::style::Style::default().fg(theme.accent)
            } else {
                theme.chrome_style()
            })
            .title(format!(" {} ", self.page.title()))
            .title_style(if focused {
                theme.focused()
            } else {
                theme.section()
            });
        let inner = block.inner(pane);
        block.render(pane, buf);
        if inner.height > 0 && inner.width > 4 {
            self.draw_page(inner, buf, theme, state);
        }

        if dirty {
            let bar = Rect::new(area.x, area.y + body_h, area.width, 1);
            SaveBar::new(self.save_subtitle(state), theme)
                .focus((self.focus == Focus::SaveBar).then_some(self.save_cursor))
                .enabled(true, self.connected)
                .render(bar, buf);
        }
    }

    fn handle(&mut self, key: KeyEvent, state: &DeviceState) -> ScreenEvent {
        // History, from anywhere on the page.
        match key.code {
            KeyCode::Char('[') => {
                return if self.back(state) {
                    ScreenEvent::Handled
                } else {
                    ScreenEvent::Status("No page to go back to".into())
                };
            }
            KeyCode::Char(']') => {
                return if self.forward(state) {
                    ScreenEvent::Handled
                } else {
                    ScreenEvent::Status("No page to go forward to".into())
                };
            }
            KeyCode::Tab => {
                self.next_focus(state);
                return ScreenEvent::Handled;
            }
            KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::Sidebar if self.dirty(state) => Focus::SaveBar,
                    Focus::Sidebar => Focus::Page,
                    Focus::Page => Focus::Sidebar,
                    Focus::SaveBar => Focus::Page,
                };
                return ScreenEvent::Handled;
            }
            _ => {}
        }

        match self.focus {
            Focus::Sidebar => self.handle_sidebar(key, state),
            Focus::SaveBar => self.handle_save_bar(key, state),
            Focus::Page => self.handle_page(key, state),
        }
    }

    fn keys(&self) -> &'static [KeyHelp] {
        self.current_ref().keys()
    }

    fn popup_result(&mut self, choice: Option<usize>, state: &DeviceState) -> ScreenEvent {
        if self.pending.take() != Some(Pending::Page) {
            return ScreenEvent::Handled;
        }
        let ev = {
            let cx = Cx {
                state,
                data: &self.data,
                config: &self.config,
                connected: self.connected,
                global_dirty: self.pages.global.dirty(state, &self.data),
            };
            // The page is borrowed mutably while `cx` borrows the rest, so the
            // dispatch is written out rather than going through `current`.
            match self.page {
                Page::About => self.pages.about.popup_result(choice, &cx),
                Page::Overview => self.pages.overview.popup_result(choice, &cx),
                Page::Global => self.pages.global.popup_result(choice, &cx),
                _ => PageEvent::Handled,
            }
        };
        self.absorb(ev, state)
    }

    fn dialog_result(&mut self, outcome: DialogOutcome, state: &DeviceState) -> ScreenEvent {
        match self.pending.take() {
            Some(Pending::Revert) => {
                if outcome == DialogOutcome::Button(0) {
                    self.revert(state)
                } else {
                    ScreenEvent::Handled
                }
            }
            Some(Pending::Page) => {
                let ev = {
                    let cx = Cx {
                        state,
                        data: &self.data,
                        config: &self.config,
                        connected: self.connected,
                        global_dirty: self.pages.global.dirty(state, &self.data),
                    };
                    match self.page {
                        Page::About => self.pages.about.dialog_result(outcome, &cx),
                        Page::Overview => self.pages.overview.dialog_result(outcome, &cx),
                        Page::Global => self.pages.global.dialog_result(outcome, &cx),
                        _ => PageEvent::Handled,
                    }
                };
                self.absorb(ev, state)
            }
            None => ScreenEvent::Handled,
        }
    }
}

impl SettingsScreen {
    fn handle_sidebar(&mut self, key: KeyEvent, state: &DeviceState) -> ScreenEvent {
        let entries = self.entries(state);
        if entries.is_empty() {
            return ScreenEvent::Unhandled;
        }
        let step = |from: usize, dir: i32| -> usize {
            let mut i = from as i32;
            loop {
                let next = i + dir;
                if next < 0 || next as usize >= entries.len() {
                    return from;
                }
                i = next;
                if matches!(entries[i as usize], Entry::Page(_)) {
                    return i as usize;
                }
            }
        };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.sidebar_cursor = step(self.sidebar_cursor, -1);
                if let Entry::Page(p) = entries[self.sidebar_cursor] {
                    self.go(p, state);
                }
                ScreenEvent::Handled
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.sidebar_cursor = step(self.sidebar_cursor, 1);
                if let Entry::Page(p) = entries[self.sidebar_cursor] {
                    self.go(p, state);
                }
                ScreenEvent::Handled
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char(' ') => {
                self.focus = Focus::Page;
                ScreenEvent::Handled
            }
            _ => ScreenEvent::Unhandled,
        }
    }

    fn handle_save_bar(&mut self, key: KeyEvent, state: &DeviceState) -> ScreenEvent {
        match key.code {
            KeyCode::Left => {
                self.save_cursor = 0;
                ScreenEvent::Handled
            }
            KeyCode::Right => {
                self.save_cursor = 1;
                ScreenEvent::Handled
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                if self.save_cursor == 1 {
                    self.save(state)
                } else {
                    self.pending = Some(Pending::Revert);
                    ScreenEvent::Dialog(Dialog::confirm(
                        "Revert Changes?",
                        "Staged changes are dropped and the device's wiring goes back to what it \
                         was when you started editing. This cannot be undone.",
                        vec![
                            crate::widgets::Button::destructive("Revert"),
                            crate::widgets::Button::new("Cancel"),
                        ],
                    ))
                }
            }
            KeyCode::Up => {
                self.focus = Focus::Page;
                ScreenEvent::Handled
            }
            _ => ScreenEvent::Unhandled,
        }
    }

    fn handle_page(&mut self, key: KeyEvent, state: &DeviceState) -> ScreenEvent {
        let theme = Theme::console(crate::theme::ColorDepth::TrueColor, Glyphs::Braille);
        let (rows, cursor) = {
            let cx = self.cx(state);
            let page = self.current_ref();
            (page.rows(&cx), page.cursor())
        };
        let focusable: Vec<usize> = (0..rows.len()).filter(|i| rows[*i].focusable()).collect();
        if focusable.is_empty() {
            // A read-only page still scrolls.
            return match key.code {
                KeyCode::Down | KeyCode::Char('j') => {
                    self.page_scroll = self.page_scroll.saturating_add(1);
                    ScreenEvent::Handled
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.page_scroll = self.page_scroll.saturating_sub(1);
                    ScreenEvent::Handled
                }
                KeyCode::Left => {
                    self.focus = Focus::Sidebar;
                    ScreenEvent::Handled
                }
                _ => ScreenEvent::Unhandled,
            };
        }
        let cursor = cursor.min(focusable.len() - 1);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.current().set_cursor(cursor.saturating_sub(1));
                return ScreenEvent::Handled;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.current()
                    .set_cursor((cursor + 1).min(focusable.len() - 1));
                return ScreenEvent::Handled;
            }
            _ => {}
        }
        let row = &rows[focusable[cursor]];
        if let Some(action) = row.handle(key, &theme) {
            let ev = {
                let cx = Cx {
                    state,
                    data: &self.data,
                    config: &self.config,
                    connected: self.connected,
                    global_dirty: self.pages.global.dirty(state, &self.data),
                };
                match self.page {
                    Page::About => self.pages.about.act(cursor, action, &cx),
                    Page::Overview => self.pages.overview.act(cursor, action, &cx),
                    Page::Global => self.pages.global.act(cursor, action, &cx),
                    _ => PageEvent::Handled,
                }
            };
            return self.absorb(ev, state);
        }
        if key.code == KeyCode::Left {
            self.focus = Focus::Sidebar;
            return ScreenEvent::Handled;
        }
        let ev = {
            let cx = Cx {
                state,
                data: &self.data,
                config: &self.config,
                connected: self.connected,
                global_dirty: self.pages.global.dirty(state, &self.data),
            };
            match self.page {
                Page::Global => self.pages.global.key(key, &cx),
                _ => PageEvent::Unhandled,
            }
        };
        self.absorb(ev, state)
    }
}

/// Fixture data for the gallery and the tests, so a Settings page can be
/// looked at without a device.
pub mod demo {
    use super::*;
    use crate::shell::fixture;

    /// A device with the wiring the pages actually show: pins claimed, an
    /// output type per slot, S/PDIF and I2S inputs configured.
    pub fn state() -> DeviceState {
        let mut s = fixture::state();
        let sec = |name: &str| {
            dspi_proto::generated::SECTIONS
                .iter()
                .find(|(n, _, _)| *n == name)
                .map(|(_, o, _)| *o)
                .expect("section")
        };
        // WirePinConfig: count then the pins.
        s.bulk.patch(sec("pins"), &[5, 6, 7, 8, 9, 10]);
        // WireI2SConfig: types, bck, mck pin, mck on, multiplier, clock mode.
        s.bulk
            .patch(sec("i2s_config"), &[0, 1, 0, 0, 14, 13, 1, 0, 1, 20]);
        // WireInputConfig at V28.
        s.bulk.patch(
            sec("input_config"),
            &[0, 5, 4, 1, 4, 17, 18, 0, 21, 22, 0, 2, 0, 13, 2, 1],
        );
        // The ADAT output, enabled on GPIO 12.
        s.bulk.patch(sec("adat_config"), &[1, 12]);
        // The DAC mute, enabled on GPIO 11 with the firmware's defaults.
        s.bulk.patch(sec("dac_hw_mute"), &[1, 1, 11, 0, 5, 0, 0, 0]);
        s
    }

    pub fn data() -> SettingsData {
        SettingsData {
            directory: Some(PresetDirectory {
                occupied: 0b0000_0111,
                startup_mode: 0,
                default_slot: 2,
                last_active: 2,
                output_config_mode: 0,
                master_volume_mode: 0,
            }),
            startup: Some(PresetStartup {
                mode: 0,
                default_slot: 2,
                last_active: 2,
            }),
            uart: Some(UartCtrlConfig::default()),
            i2c: Some(I2cCtrlConfig::default()),
            iface: Some(CtrlIfaceStatus {
                uart_last_status: 0,
                uart_live: false,
                i2c_last_status: 0,
                i2c_live: false,
                proto_version: 2,
            }),
            spdif: Some(SpdifInputConfig {
                count: 4,
                enable_mask: 0b0000_0111,
                gpio: [5, 21, 22, 20],
            }),
            claims: None,
            preset_names: vec![
                "Movies".into(),
                "Music".into(),
                "Living Room".into(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            ],
            cs_dirty: false,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A scratch path for a test that saves the app-side settings, unique per
    /// test so two running at once cannot collide.
    pub(crate) fn scratch(name: &str) -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!("dspi-settings-{}-{name}", std::process::id()))
            .join("config.toml")
    }
    use crate::theme::{ColorDepth, Glyphs};
    use crossterm::event::KeyModifiers;

    pub(crate) fn theme() -> Theme {
        Theme::console(ColorDepth::TrueColor, Glyphs::Braille)
    }

    pub(crate) fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    pub(crate) use super::demo::{data, state};

    pub(crate) fn screen(page: Page) -> (SettingsScreen, DeviceState) {
        let st = state();
        let s = SettingsScreen::new(&st, data(), AppConfig::default())
            .config_path(scratch("default"))
            .open(page, &st);
        (s, st)
    }

    pub(crate) fn frame(s: &mut SettingsScreen, st: &DeviceState, w: u16, h: u16) -> String {
        let t = theme();
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        term.draw(|f| s.draw(f.area(), f.buffer_mut(), &t, st, true))
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

    const EVERY_PAGE: [Page; 12] = [
        Page::About,
        Page::Advanced,
        Page::Graphing,
        Page::Overview,
        Page::Inputs,
        Page::Outputs,
        Page::I2s,
        Page::Global,
        Page::Surfaces,
        Page::Interfaces,
        Page::Groups,
        Page::Macros,
    ];

    #[test]
    fn every_page_fills_the_screen_at_both_sizes() {
        for page in EVERY_PAGE {
            let (mut s, st) = screen(page);
            for (w, h) in [(120u16, 40u16), (80, 24)] {
                let f = frame(&mut s, &st, w, h);
                assert_eq!(f.lines().count(), h as usize, "{page:?} at {w}x{h}");
                assert!(f.contains("APPLICATION"), "the sidebar: {page:?}\n{f}");
                if available(page, &st, true) {
                    assert!(
                        f.contains(page.title()) || f.contains(page.short()),
                        "{page:?} at {w}x{h} names itself:\n{f}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_sidebar_lists_the_consoles_groups_in_order() {
        let (mut s, st) = screen(Page::Global);
        let f = frame(&mut s, &st, 120, 40);
        let at = |t: &str| f.find(t).unwrap_or_else(|| panic!("missing {t}\n{f}"));
        assert!(at("APPLICATION") < at("DISPLAY"));
        assert!(at("DISPLAY") < at("SYSTEM"));
        assert!(at("SYSTEM") < at("CONTROL"));
        assert!(f.contains("Global Params"));
        assert!(f.contains("I2S Config."));
    }

    #[test]
    fn an_unsupported_page_is_absent_rather_than_disabled() {
        let mut st = state();
        st.caps.features = vec![
            dspi_session::probe::Feature {
                name: "uart_control".into(),
                present: false,
                evidence: "0xF6 stalled".into(),
            },
            dspi_session::probe::Feature {
                name: "i2c_control".into(),
                present: false,
                evidence: "0xF8 stalled".into(),
            },
            dspi_session::probe::Feature {
                name: "spdif_multi_input".into(),
                present: false,
                evidence: "0xEF stalled".into(),
            },
            dspi_session::probe::Feature {
                name: "i2s_input_channels".into(),
                present: false,
                evidence: "0xF4 stalled".into(),
            },
            dspi_session::probe::Feature {
                name: "adat_input".into(),
                present: false,
                evidence: "0x69 stalled".into(),
            },
            dspi_session::probe::Feature {
                name: "lg_sound_sync".into(),
                present: false,
                evidence: "0xE7 stalled".into(),
            },
        ];
        assert!(!available(Page::Interfaces, &st, true));
        assert!(!available(Page::Inputs, &st, true));
        assert!(!available(Page::Groups, &st, true));
        assert!(available(Page::About, &st, true));
        let mut s = SettingsScreen::new(&st, data(), AppConfig::default());
        let f = frame(&mut s, &st, 120, 40);
        assert!(!f.contains("Control Interf."), "{f}");
        assert!(!f.contains("Channel Groups"), "{f}");
    }

    #[test]
    fn the_sidebar_walks_pages_and_skips_the_group_headers() {
        let (mut s, st) = screen(Page::About);
        assert_eq!(s.page(), Page::About);
        s.handle(key(KeyCode::Down), &st);
        assert_eq!(s.page(), Page::Advanced);
        s.handle(key(KeyCode::Down), &st);
        assert_eq!(s.page(), Page::Graphing, "stepped over DISPLAY");
        s.handle(key(KeyCode::Down), &st);
        assert_eq!(s.page(), Page::Overview, "stepped over SYSTEM");
        s.handle(key(KeyCode::Up), &st);
        assert_eq!(s.page(), Page::Graphing);
    }

    #[test]
    fn brackets_walk_the_visited_pages() {
        let (mut s, st) = screen(Page::About);
        s.handle(key(KeyCode::Down), &st);
        s.handle(key(KeyCode::Down), &st);
        assert_eq!(s.page(), Page::Graphing);
        s.handle(key(KeyCode::Char('[')), &st);
        assert_eq!(s.page(), Page::Advanced);
        s.handle(key(KeyCode::Char('[')), &st);
        assert_eq!(s.page(), Page::About);
        assert_eq!(
            s.handle(key(KeyCode::Char('[')), &st),
            ScreenEvent::Status("No page to go back to".into())
        );
        s.handle(key(KeyCode::Char(']')), &st);
        assert_eq!(s.page(), Page::Advanced);
        s.handle(key(KeyCode::Char(']')), &st);
        assert_eq!(s.page(), Page::Graphing);
        assert_eq!(
            s.handle(key(KeyCode::Char(']')), &st),
            ScreenEvent::Status("No page to go forward to".into())
        );
    }

    #[test]
    fn the_save_bar_appears_only_while_something_is_unsaved() {
        let (mut s, st) = screen(Page::Global);
        assert!(!s.dirty(&st));
        let f = frame(&mut s, &st, 120, 40);
        assert!(!f.contains("Unsaved changes"), "{f}");

        // Stage a change on the Global page: Tab into it, then flip the mode.
        s.handle(key(KeyCode::Tab), &st);
        s.handle(key(KeyCode::Right), &st);
        assert!(s.dirty(&st), "the staged draft is a dirty category");
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("● Unsaved changes"), "{f}");
        assert!(
            f.contains("Saving writes these settings to the device's flash."),
            "{f}"
        );
        assert!(f.contains("Revert") && f.contains("[Save]"), "{f}");
    }

    #[test]
    fn revert_confirms_and_then_restores_the_draft() {
        let (mut s, st) = screen(Page::Global);
        s.handle(key(KeyCode::Tab), &st);
        s.handle(key(KeyCode::Right), &st);
        assert!(s.dirty(&st));
        // Tab reaches the save bar only while it is there.
        s.handle(key(KeyCode::Tab), &st);
        assert_eq!(s.focus, Focus::SaveBar);
        s.handle(key(KeyCode::Left), &st);
        match s.handle(key(KeyCode::Enter), &st) {
            ScreenEvent::Dialog(d) => assert_eq!(d.title, "Revert Changes?"),
            other => panic!("{other:?}"),
        }
        s.dialog_result(DialogOutcome::Button(0), &st);
        assert!(!s.dirty(&st), "the draft went back to the device's values");
    }

    #[test]
    fn saving_issues_the_consoles_sequence() {
        let (mut s, st) = screen(Page::Global);
        s.handle(key(KeyCode::Tab), &st);
        s.handle(key(KeyCode::Right), &st);
        s.io_dirty = true;
        s.data.cs_dirty = true;
        s.handle(key(KeyCode::Tab), &st);
        s.save_cursor = 1;
        match s.handle(key(KeyCode::Enter), &st) {
            ScreenEvent::Command(c) => {
                let lines: Vec<&str> = c.lines().collect();
                assert!(lines[0].starts_with("preset.startup"), "{c}");
                assert_eq!(lines[lines.len() - 2], "dev.save.io", "{c}");
                assert_eq!(lines[lines.len() - 1], "cs.save", "{c}");
            }
            other => panic!("{other:?}"),
        }
        assert!(!s.global_dirty(&st) && !s.output_dirty());
        assert!(
            s.dirty(&st),
            "the control-surface flag stays until the device clears it"
        );
    }

    #[test]
    fn output_config_edits_only_count_while_the_wiring_is_device_global() {
        let (mut s, st) = screen(Page::Outputs);
        s.begin_output_edit(&st);
        assert!(s.output_dirty(), "independent mode flashes separately");
        let mut s2 = SettingsScreen::new(
            &st,
            {
                let mut d = data();
                if let Some(dir) = d.directory.as_mut() {
                    dir.output_config_mode = 1;
                }
                d
            },
            AppConfig::default(),
        );
        s2.begin_output_edit(&st);
        assert!(!s2.output_dirty(), "with-preset mode saves with the preset");
    }

    #[test]
    fn the_control_surface_category_says_the_change_is_already_live() {
        let (mut s, st) = screen(Page::About);
        s.data.cs_dirty = true;
        assert!(s.dirty(&st));
        assert_eq!(s.save_subtitle(&st), SaveBar::CS_ONLY);
        let f = frame(&mut s, &st, 120, 40);
        assert!(
            f.contains("Your controls are live now; saving keeps them across a reboot."),
            "{f}"
        );
    }

    #[test]
    fn a_page_name_maps_to_its_page() {
        assert_eq!(SettingsScreen::page_from_name("about"), Some(Page::About));
        assert_eq!(SettingsScreen::page_from_name("I2S"), Some(Page::I2s));
        assert_eq!(
            SettingsScreen::page_from_name("global-parameters"),
            Some(Page::Global)
        );
        assert_eq!(SettingsScreen::page_from_name("nope"), None);
    }

    #[test]
    fn the_io_baseline_names_the_writes_that_put_the_wiring_back() {
        let st = state();
        let base = IoSnapshot::capture(&st);
        let mut moved = base.clone();
        moved.output_pins[0] = 20;
        moved.output_types[1] = 0;
        moved.bck_pin = 2;
        moved.adat_enabled = false;
        let cmds = base.restore_commands(&moved);
        assert!(cmds.contains(&"out.pin 0 6".to_string()), "{cmds:?}");
        assert!(cmds.contains(&"out.type 1 i2s".to_string()), "{cmds:?}");
        assert!(cmds.contains(&"i2s.bck 14".to_string()), "{cmds:?}");
        assert!(cmds.contains(&"adat.enable on".to_string()), "{cmds:?}");
        // The type change leads, as the Console's restore does.
        assert!(cmds[0].starts_with("out.type"), "{cmds:?}");
        assert!(base.restore_commands(&base).is_empty());
    }

    /// The key events a key-line token stands for. Settings advertises `Tab`
    /// and the two history brackets, which the shared helper does not know.
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
                "Backspace" => key(KeyCode::Backspace),
                "Tab" => key(KeyCode::Tab),
                "Esc" => key(KeyCode::Esc),
                other => {
                    let c = other.chars().next().expect("a key token");
                    assert_eq!(other.chars().count(), 1, "unknown key token {other}");
                    key(KeyCode::Char(c))
                }
            })
            .collect()
    }

    /// An advertised key has to do something. A page may disable a particular
    /// control (an I2S BCK pin cannot move while an I2S output is running), so
    /// the key is offered at every focusable row and only has to land on one.
    #[test]
    fn every_advertised_key_is_handled_on_every_page() {
        for page in EVERY_PAGE {
            let advertised = screen(page).0.keys();
            let controls = {
                let (s, st) = screen(page);
                let cx = s.cx(&st);
                s.current_ref()
                    .rows(&cx)
                    .iter()
                    .filter(|r| r.focusable())
                    .count()
                    .max(1)
            };
            for help in advertised {
                for k in keys_for(help.key) {
                    // Esc is the shell's; Tab is the focus test's.
                    if matches!(k.code, KeyCode::Esc) {
                        continue;
                    }
                    let landed = (0..controls).any(|row| {
                        let (mut s, st) = screen(page);
                        s.focus = Focus::Page;
                        s.current().set_cursor(row);
                        let before = (s.page, s.focus, s.current_ref().cursor(), s.page_scroll);
                        let ev = s.handle(k, &st);
                        let after = (s.page, s.focus, s.current_ref().cursor(), s.page_scroll);
                        ev != ScreenEvent::Unhandled || before != after
                    });
                    assert!(
                        landed,
                        "{:?} does nothing anywhere on {page:?} (from {})",
                        k.code, help.key
                    );
                }
            }
        }
    }
}
