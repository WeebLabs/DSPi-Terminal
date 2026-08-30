//! The filter list: the Console's `FilterListView`, in both of its modes.
//!
//! One table over a channel's bands, with the Console's columns and its
//! hierarchical type picker. The PEQ mode edits the ten-band bank; the XO mode
//! edits the four crossover bands the firmware addresses as 20 to 23, choosing
//! a family, a direction and a slope rather than a raw type byte.
//!
//! Every edit is one whole-band write, because that is how the firmware stores
//! a band: the `eq` verb sends the 16-byte packet, so changing one field means
//! sending the other five back unchanged.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use dspi_proto::FilterType;
use dspi_proto::value::EqParamPacket;
use dspi_proto::xover::{self, Family};
use dspi_session::DeviceState;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

use super::linkwitz::LinkwitzPanel;
use super::{
    available_types, band_command, channel_token, cleared_band, display_band, number,
    supports_bypass, type_name,
};
use crate::shell::ScreenEvent;
use crate::theme::{Glyphs, Theme};
use crate::widgets::table::Cell;
use crate::widgets::text::q as q_text;
use crate::widgets::{
    Button, Column, Dialog, DialogOutcome, KeyHelp, NumberEdit, PopupList, Table,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    Peq,
    Xo,
}

/// One editable cell of a row. Which of these a row has depends on its type,
/// exactly as the Console hides the columns a type does not use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Type,
    Freq,
    Gain,
    Q,
    /// The Linkwitz row's settings button, which opens its panel.
    Config,
    Family,
    Direction,
    Slope,
}

impl Field {
    /// Which table column this field is drawn in.
    fn column(self) -> usize {
        match self {
            Field::Type | Field::Family => 1,
            Field::Freq | Field::Config | Field::Direction => 2,
            Field::Gain | Field::Slope => 3,
            Field::Q => 4,
        }
    }
}

/// What a popup or a dialog this list opened is waiting to hear back about.
#[derive(Debug, Clone, PartialEq)]
enum Pending {
    Type(Vec<Option<FilterType>>),
    Family(Vec<Option<Family>>),
    Direction,
    Slope(Vec<u8>),
    ClearAll,
    BypassAll,
}

const PEQ_COLUMNS: &[Column] = &[
    Column::right("#", 2),
    Column::left("Type", 21),
    Column::right("Freq", 9),
    Column::right("Gain", 9),
    Column::right("Width", 7),
];

const XO_COLUMNS: &[Column] = &[
    Column::right("#", 2),
    Column::left("Family", 14),
    Column::left("Type", 10),
    Column::right("Slope", 10),
    Column::right("Freq", 9),
];

pub const PEQ_KEYS: &[KeyHelp] = &[
    KeyHelp::new("↑ ↓", "Band"),
    KeyHelp::new("← →", "Field"),
    KeyHelp::new("Enter", "Edit"),
    KeyHelp::new("Space", "Bypass"),
    KeyHelp::new("1-9,0", "Jump to a band"),
    KeyHelp::new("a", "Enable All"),
    KeyHelp::new("A", "Bypass All"),
    KeyHelp::new("D", "Clear All"),
];

pub const TABBED_KEYS: &[KeyHelp] = &[
    KeyHelp::new("↑ ↓", "Band"),
    KeyHelp::new("← →", "Field"),
    KeyHelp::new("Enter", "Edit"),
    KeyHelp::new("Space", "Bypass"),
    KeyHelp::new("1-9,0", "Jump to a band"),
    KeyHelp::new("a", "Enable All"),
    KeyHelp::new("A", "Bypass All"),
    KeyHelp::new("D", "Clear All"),
    KeyHelp::new("x", "PEQ / XO"),
];

pub struct FilterList {
    /// The unified channel index whose bank this is.
    pub channel: usize,
    pub mode: FilterMode,
    pub band: usize,
    pub field: usize,
    pub edit: Option<NumberEdit>,
    pub scroll: usize,
    /// A second channel every write is mirrored onto: the other half of a
    /// linked input pair.
    pub mirror: Option<usize>,
    /// The Console hides the Linkwitz Transform on inputs.
    pub include_linkwitz: bool,
    /// Whether `x` switches between PEQ and XO.
    pub can_switch_mode: bool,
    pub panel: Option<LinkwitzPanel>,
    pending: Option<Pending>,
}

impl FilterList {
    pub fn new(channel: usize) -> Self {
        Self {
            channel,
            mode: FilterMode::Peq,
            band: 0,
            field: 0,
            edit: None,
            scroll: 0,
            mirror: None,
            include_linkwitz: false,
            can_switch_mode: false,
            panel: None,
            pending: None,
        }
    }

    pub fn linkwitz(mut self, on: bool) -> Self {
        self.include_linkwitz = on;
        self
    }

    pub fn tabbed(mut self, on: bool) -> Self {
        self.can_switch_mode = on;
        self
    }

    pub fn keys(&self) -> &'static [KeyHelp] {
        if self.can_switch_mode {
            TABBED_KEYS
        } else {
            PEQ_KEYS
        }
    }

    /// The Console's footer strip for the echo line: `Enable All │ Bypass
    /// All   Clear All   PEQ │ XO`, each half greyed when it would be a
    /// no-op (`BypassAllControls`), the mode tabs showing the other mode as
    /// the live one.
    pub fn actions(&self, state: &DeviceState, xo_available: bool) -> Vec<(String, bool)> {
        let bands = self.bands(state);
        let live: Vec<&EqParamPacket> = bands
            .iter()
            .filter(|b| b.filter_type != FilterType::Flat)
            .collect();
        let any_bypassed = live.iter().any(|b| b.bypass);
        let any_armed = live.iter().any(|b| !b.bypass);
        let mut out = vec![
            ("Enable All".to_string(), any_bypassed),
            ("|".to_string(), true),
            ("Bypass All".to_string(), any_armed),
            ("Clear All".to_string(), !live.is_empty()),
        ];
        if xo_available {
            out.push(("PEQ".to_string(), self.mode == FilterMode::Xo));
            out.push(("|".to_string(), true));
            out.push(("XO".to_string(), self.mode == FilterMode::Peq));
        }
        out
    }

    pub fn bands(&self, state: &DeviceState) -> Vec<EqParamPacket> {
        match self.mode {
            FilterMode::Peq => state.bands(self.channel as u8),
            FilterMode::Xo => state.xover_bands(self.channel as u8),
        }
    }

    /// The band number the firmware uses: 0-based in the PEQ bank, 20 to 23 in
    /// the crossover bank.
    fn wire_band(&self, index: usize) -> u8 {
        match self.mode {
            FilterMode::Peq => index as u8,
            FilterMode::Xo => 20 + index as u8,
        }
    }

    /// The rows this list needs, header included.
    pub fn height(&self, state: &DeviceState) -> u16 {
        self.bands(state).len() as u16 + 1
    }

    /// The fields a row offers, in the order the arrows walk them.
    pub fn fields(&self, band: &EqParamPacket) -> Vec<Field> {
        match self.mode {
            FilterMode::Peq => {
                if band.filter_type == FilterType::Flat {
                    vec![Field::Type]
                } else if band.filter_type.is_linkwitz() {
                    vec![Field::Type, Field::Config]
                } else {
                    let mut f = vec![Field::Type, Field::Freq];
                    if band.filter_type.uses_gain() {
                        f.push(Field::Gain);
                    }
                    if band.filter_type.uses_q() {
                        f.push(Field::Q);
                    }
                    f
                }
            }
            FilterMode::Xo => {
                if band.filter_type.is_crossover() {
                    vec![Field::Family, Field::Direction, Field::Slope, Field::Freq]
                } else {
                    vec![Field::Family]
                }
            }
        }
    }

    fn current(&self, state: &DeviceState) -> Option<EqParamPacket> {
        self.bands(state).get(self.band).copied()
    }

    fn current_field(&self, state: &DeviceState) -> Option<Field> {
        let band = self.current(state)?;
        let fields = self.fields(&band);
        fields
            .get(self.field.min(fields.len().saturating_sub(1)))
            .copied()
    }

    /// The commands that write one band, mirrored onto a linked partner.
    fn write(&self, state: &DeviceState, index: usize, p: &EqParamPacket) -> String {
        let wire = self.wire_band(index);
        let mut lines = band_command(state, self.channel, wire, p);
        if let Some(m) = self.mirror {
            lines.extend(band_command(state, m, wire, p));
        }
        lines.join("\n")
    }

    fn bypass_command(&self, state: &DeviceState, index: usize, on: bool) -> String {
        let n = display_band(self.wire_band(index));
        let word = if on { "on" } else { "off" };
        let mut lines = vec![format!(
            "eq.bypass {} {n} {word}",
            channel_token(state, self.channel)
        )];
        if let Some(m) = self.mirror {
            lines.push(format!("eq.bypass {} {n} {word}", channel_token(state, m)));
        }
        lines.join("\n")
    }

    // -----------------------------------------------------------------
    // Drawing
    // -----------------------------------------------------------------

    pub fn draw(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        state: &DeviceState,
        focused: bool,
    ) {
        if area.height == 0 || area.width < 20 {
            return;
        }
        let bands = self.bands(state);
        if bands.is_empty() {
            return;
        }
        self.band = self.band.min(bands.len() - 1);
        let columns = match self.mode {
            FilterMode::Peq => PEQ_COLUMNS,
            FilterMode::Xo => XO_COLUMNS,
        };
        let rows: Vec<Vec<Cell>> = bands
            .iter()
            .enumerate()
            .map(|(i, b)| self.row(i, b))
            .collect();
        let body = area.height.saturating_sub(1) as usize;
        self.scroll = Table::scroll_to(self.band, self.scroll, body);

        let field = self.current_field(state);
        let cursor = (self.band, field.map(Field::column).unwrap_or(1));
        let armed = self.edit.as_ref().map(|_| cursor);
        Table::new(columns, theme)
            .rows(rows)
            .focused(focused, cursor)
            .armed(armed)
            .scroll(self.scroll)
            .render(area, buf);

        // `DESIGN.md` 7.6 gives the Linkwitz row all four of its values on one
        // line, `f0 40 Hz Q0 0.5 → fp 25 Hz Qp 0.71  +8.2 dB  ⚙`, which is
        // wider than any one column. It is painted over the FREQ / GAIN /
        // WIDTH span the row left empty.
        if self.mode == FilterMode::Peq {
            let mut fx = area.x + 2;
            for c in columns.iter().take(2) {
                fx += c.width + 1;
            }
            let room = (area.x + area.width).saturating_sub(fx) as usize;
            for (i, b) in bands.iter().enumerate().skip(self.scroll).take(body) {
                if !b.filter_type.is_linkwitz() {
                    continue;
                }
                let y = area.y + 1 + (i - self.scroll) as u16;
                let text = linkwitz_line(b, theme);
                let armed = focused && i == self.band && field == Some(Field::Config);
                let style = if armed {
                    theme.focused()
                } else {
                    theme.value()
                };
                buf.set_string(fx, y, crate::widgets::text::truncate(&text, room), style);
            }
        }

        // The bypass disc lives in the margin the table leaves free, and is
        // hidden entirely when the firmware has no per-band bypass.
        if supports_bypass(state) {
            for (i, b) in bands.iter().enumerate().skip(self.scroll).take(body) {
                let y = area.y + 1 + (i - self.scroll) as u16;
                let (glyph, style) = disc(b, theme);
                buf.set_string(area.x + 1, y, glyph, style);
            }
        }

        if let Some(panel) = &self.panel {
            panel.draw(area, buf, theme);
        }
    }

    fn row(&self, index: usize, b: &EqParamPacket) -> Vec<Cell> {
        let n = Cell::dim((index + 1).to_string());
        let active = b.filter_type != FilterType::Flat;
        match self.mode {
            FilterMode::Peq => {
                let mut cells = vec![n, Cell::from(type_name(b.filter_type))];
                if !active {
                    cells.push(Cell::default());
                    cells.push(Cell::default());
                    cells.push(Cell::default());
                    return cells;
                }
                if b.filter_type.is_linkwitz() {
                    // Its four values do not fit the FREQ / GAIN / WIDTH
                    // columns one apiece, so `DESIGN.md` 7.6 runs them across
                    // the three as one line, which `draw` paints over the span
                    // these three empty cells leave.
                    cells.push(Cell::default());
                    cells.push(Cell::default());
                    cells.push(Cell::default());
                    return cells;
                }
                cells.push(Cell::from(format!("{:.0} Hz", b.freq)));
                cells.push(if b.filter_type.uses_gain() {
                    Cell::from(format!("{:+.1} dB", b.gain_db))
                } else {
                    Cell::default()
                });
                cells.push(if b.filter_type.uses_q() {
                    Cell::from(q_text(b.q as f64))
                } else {
                    Cell::default()
                });
                cells
            }
            FilterMode::Xo => {
                let meta = xover::meta(b.filter_type.to_raw());
                let mut cells = vec![
                    n,
                    Cell::from(match meta {
                        Some(m) => m.family.label().to_string(),
                        None => "Off".to_string(),
                    }),
                ];
                match meta {
                    Some(m) => {
                        cells.push(Cell::from(if m.high_pass {
                            "High Pass"
                        } else {
                            "Low Pass"
                        }));
                        cells.push(Cell::from(format!("{} dB/oct", m.order as u16 * 6)));
                        cells.push(Cell::from(format!("{:.0} Hz", b.freq)));
                    }
                    None => {
                        cells.push(Cell::default());
                        cells.push(Cell::default());
                        cells.push(Cell::default());
                    }
                }
                cells
            }
        }
    }

    // -----------------------------------------------------------------
    // Keys
    // -----------------------------------------------------------------

    /// Handle a key. `Up` on the first band is left unhandled, which is how
    /// the page above knows to take the focus back into its header.
    pub fn handle(&mut self, key: KeyEvent, state: &DeviceState) -> ScreenEvent {
        if self.panel.is_some() {
            return self.handle_panel(key, state);
        }
        let bands = self.bands(state);
        if bands.is_empty() {
            return ScreenEvent::Unhandled;
        }
        self.band = self.band.min(bands.len() - 1);
        let band = bands[self.band];
        let fields = self.fields(&band);
        self.field = self.field.min(fields.len().saturating_sub(1));

        if self.edit.is_some() {
            return self.handle_edit(key, state, &band);
        }

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.band == 0 {
                    return ScreenEvent::Unhandled;
                }
                self.band -= 1;
                self.field = 0;
                ScreenEvent::Handled
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.band = (self.band + 1).min(bands.len() - 1);
                self.field = 0;
                ScreenEvent::Handled
            }
            KeyCode::Left => {
                if self.field == 0 {
                    return ScreenEvent::Unhandled;
                }
                self.field -= 1;
                ScreenEvent::Handled
            }
            KeyCode::Right => {
                self.field = (self.field + 1).min(fields.len().saturating_sub(1));
                ScreenEvent::Handled
            }
            KeyCode::Char(d @ '0'..='9') => {
                let want = if d == '0' {
                    9
                } else {
                    d as usize - '1' as usize
                };
                if want < bands.len() {
                    self.band = want;
                    self.field = 0;
                }
                ScreenEvent::Handled
            }
            KeyCode::Char(' ') => {
                if band.filter_type == FilterType::Flat || !supports_bypass(state) {
                    return ScreenEvent::Handled;
                }
                ScreenEvent::Command(self.bypass_command(state, self.band, !band.bypass))
            }
            KeyCode::Enter => self.open(state, &band),
            KeyCode::Char('a') => self.set_all_bypassed(state, false),
            KeyCode::Char('A') => {
                if self.mode == FilterMode::Xo {
                    self.pending = Some(Pending::BypassAll);
                    // Bypassing a crossover sends full-range audio to the
                    // driver, so the Console asks first, in these words.
                    ScreenEvent::Dialog(
                        Dialog::confirm(
                            "Bypass this output's crossovers?",
                            "This sends full-range audio to this output with no crossover \
                             protection, which can damage unprotected drivers such as \
                             tweeters. Continue only if you are sure.",
                            vec![Button::destructive("Bypass All"), Button::new("Cancel")],
                        )
                        .critical(),
                    )
                } else {
                    self.set_all_bypassed(state, true)
                }
            }
            KeyCode::Char('D') => {
                self.pending = Some(Pending::ClearAll);
                ScreenEvent::Dialog(Dialog::confirm(
                    "Clear All Bands?",
                    "Every band in this list will be reset to its default (flat) state. \
                     This cannot be undone.",
                    vec![Button::destructive("Clear All"), Button::new("Cancel")],
                ))
            }
            KeyCode::Char('x') if self.can_switch_mode => {
                self.mode = match self.mode {
                    FilterMode::Peq => FilterMode::Xo,
                    FilterMode::Xo => FilterMode::Peq,
                };
                self.band = 0;
                self.field = 0;
                self.scroll = 0;
                ScreenEvent::Handled
            }
            _ => ScreenEvent::Unhandled,
        }
    }

    /// Enter on a field: a picker opens, a number arms, the Linkwitz button
    /// opens its panel.
    fn open(&mut self, state: &DeviceState, band: &EqParamPacket) -> ScreenEvent {
        let Some(field) = self.current_field(state) else {
            return ScreenEvent::Handled;
        };
        match field {
            Field::Type => {
                let (popup, map) = type_menu(state, self.include_linkwitz, band.filter_type);
                self.pending = Some(Pending::Type(map));
                ScreenEvent::Popup(popup)
            }
            Field::Family => {
                let (popup, map) = family_menu(band.filter_type);
                self.pending = Some(Pending::Family(map));
                ScreenEvent::Popup(popup)
            }
            Field::Direction => {
                self.pending = Some(Pending::Direction);
                let selected = usize::from(
                    xover::meta(band.filter_type.to_raw()).is_some_and(|m| m.high_pass),
                );
                ScreenEvent::Popup(PopupList::new(
                    "Type",
                    vec!["Low Pass".into(), "High Pass".into()],
                    selected,
                ))
            }
            Field::Slope => {
                let Some(m) = xover::meta(band.filter_type.to_raw()) else {
                    return ScreenEvent::Handled;
                };
                let orders = available_orders(m.family);
                let selected = orders.iter().position(|o| *o == m.order).unwrap_or(0);
                let items = orders
                    .iter()
                    .map(|o| format!("{} dB/oct", *o as u16 * 6))
                    .collect();
                self.pending = Some(Pending::Slope(orders));
                ScreenEvent::Popup(PopupList::new("Slope", items, selected))
            }
            Field::Config => {
                self.panel = Some(LinkwitzPanel::new(*band));
                ScreenEvent::Handled
            }
            Field::Freq => {
                self.edit = Some(seed(format!("{:.0}", band.freq)));
                ScreenEvent::Handled
            }
            Field::Gain => {
                self.edit = Some(seed(gain_text(band.gain_db as f64)));
                ScreenEvent::Handled
            }
            Field::Q => {
                self.edit = Some(seed(q_text(band.q as f64)));
                ScreenEvent::Handled
            }
        }
    }

    /// Keys while a numeric field is armed: arrows nudge live, typing
    /// replaces, Enter commits and Escape reverts.
    fn handle_edit(
        &mut self,
        key: KeyEvent,
        state: &DeviceState,
        band: &EqParamPacket,
    ) -> ScreenEvent {
        let Some(field) = self.current_field(state) else {
            self.edit = None;
            return ScreenEvent::Handled;
        };
        let coarse = key.modifiers.contains(KeyModifiers::SHIFT);
        if matches!(key.code, KeyCode::Left | KeyCode::Right) {
            let dir = if key.code == KeyCode::Left { -1.0 } else { 1.0 };
            let current = self
                .edit
                .as_ref()
                .and_then(|e| e.value())
                .unwrap_or(field_value(band, field));
            let next = nudge(field, current, dir, coarse);
            self.edit = Some(seed(format_field(field, next)));
            let mut p = *band;
            set_field(&mut p, field, next);
            return ScreenEvent::Command(self.write(state, self.band, &p));
        }
        let Some(edit) = self.edit.as_mut() else {
            return ScreenEvent::Handled;
        };
        match edit.handle(key) {
            Some(crate::widgets::Action::Committed(v)) => {
                self.edit = None;
                let v = clamp(field, v);
                let mut p = *band;
                set_field(&mut p, field, v);
                ScreenEvent::Command(self.write(state, self.band, &p))
            }
            Some(crate::widgets::Action::Closed) => {
                self.edit = None;
                ScreenEvent::Handled
            }
            _ => ScreenEvent::Handled,
        }
    }

    fn handle_panel(&mut self, key: KeyEvent, state: &DeviceState) -> ScreenEvent {
        let Some(panel) = self.panel.as_mut() else {
            return ScreenEvent::Unhandled;
        };
        match panel.handle(key) {
            super::linkwitz::PanelEvent::Handled => ScreenEvent::Handled,
            super::linkwitz::PanelEvent::Close => {
                self.panel = None;
                ScreenEvent::Handled
            }
            super::linkwitz::PanelEvent::Apply(p) => {
                let index = self.band;
                self.panel = None;
                ScreenEvent::Command(self.write(state, index, &p))
            }
        }
    }

    /// `a` and `A`: every band that is not Off, as the Console's
    /// `BypassAllControls` does.
    fn set_all_bypassed(&self, state: &DeviceState, on: bool) -> ScreenEvent {
        if !supports_bypass(state) {
            return ScreenEvent::Handled;
        }
        let lines: Vec<String> = self
            .bands(state)
            .iter()
            .enumerate()
            .filter(|(_, b)| b.filter_type != FilterType::Flat && b.bypass != on)
            .map(|(i, _)| self.bypass_command(state, i, on))
            .collect();
        if lines.is_empty() {
            return ScreenEvent::Handled;
        }
        ScreenEvent::Command(lines.join("\n"))
    }

    fn clear_all(&self, state: &DeviceState) -> ScreenEvent {
        let lines: Vec<String> = self
            .bands(state)
            .iter()
            .enumerate()
            .map(|(i, _)| {
                let p = cleared_band(self.channel as u8, self.wire_band(i));
                self.write(state, i, &p)
            })
            .collect();
        ScreenEvent::Command(lines.join("\n"))
    }

    pub fn popup_result(&mut self, choice: Option<usize>, state: &DeviceState) -> ScreenEvent {
        let pending = self.pending.take();
        let (Some(i), Some(pending)) = (choice, pending) else {
            return ScreenEvent::Handled;
        };
        let Some(band) = self.current(state) else {
            return ScreenEvent::Handled;
        };
        let mut p = band;
        match pending {
            Pending::Type(map) => {
                let Some(Some(t)) = map.get(i).copied() else {
                    return ScreenEvent::Handled;
                };
                retype(&mut p, t);
            }
            Pending::Family(map) => {
                let Some(family) = map.get(i).copied() else {
                    return ScreenEvent::Handled;
                };
                apply_family(&mut p, family);
            }
            Pending::Direction => {
                let Some(m) = xover::meta(p.filter_type.to_raw()) else {
                    return ScreenEvent::Handled;
                };
                p.filter_type = FilterType::from_raw(
                    xover::Meta {
                        high_pass: i == 1,
                        ..m
                    }
                    .to_type(),
                );
            }
            Pending::Slope(orders) => {
                let (Some(m), Some(order)) =
                    (xover::meta(p.filter_type.to_raw()), orders.get(i).copied())
                else {
                    return ScreenEvent::Handled;
                };
                p.filter_type = FilterType::from_raw(xover::Meta { order, ..m }.to_type());
            }
            Pending::ClearAll | Pending::BypassAll => return ScreenEvent::Handled,
        }
        self.field = self.field.min(self.fields(&p).len().saturating_sub(1));
        ScreenEvent::Command(self.write(state, self.band, &p))
    }

    pub fn dialog_result(&mut self, outcome: DialogOutcome, state: &DeviceState) -> ScreenEvent {
        let pending = self.pending.take();
        if outcome != DialogOutcome::Button(0) {
            return ScreenEvent::Handled;
        }
        match pending {
            Some(Pending::ClearAll) => self.clear_all(state),
            Some(Pending::BypassAll) => self.set_all_bypassed(state, true),
            _ => ScreenEvent::Handled,
        }
    }
}

fn seed(text: String) -> NumberEdit {
    NumberEdit { text, dirty: false }
}

/// The Linkwitz row's inline reading: driver, target, the implied DC boost,
/// and the button that opens the panel (`DESIGN.md` 7.6).
fn linkwitz_line(b: &EqParamPacket, theme: &Theme) -> String {
    let ascii = theme.glyphs == Glyphs::Ascii;
    format!(
        "f0 {} Hz Q0 {} {} fp {} Hz Qp {}  {:+.1} dB  {}",
        number(b.freq),
        q_text(b.q as f64),
        if ascii { "->" } else { "→" },
        number(b.gain_db),
        q_text(b.qp.unwrap_or(super::linkwitz::DEFAULT_QP) as f64),
        super::linkwitz::boost_of(b),
        if ascii { "cfg" } else { "⚙" },
    )
}

fn disc(b: &EqParamPacket, theme: &Theme) -> (&'static str, Style) {
    let ascii = theme.glyphs == Glyphs::Ascii;
    if b.filter_type == FilterType::Flat {
        return (" ", theme.label());
    }
    if b.bypass {
        (if ascii { "o" } else { "○" }, theme.label())
    } else {
        (
            if ascii { "*" } else { "●" },
            Style::default().fg(theme.accent),
        )
    }
}

fn field_value(b: &EqParamPacket, field: Field) -> f64 {
    match field {
        Field::Freq => b.freq as f64,
        Field::Gain => b.gain_db as f64,
        Field::Q => b.q as f64,
        _ => 0.0,
    }
}

fn set_field(b: &mut EqParamPacket, field: Field, v: f64) {
    match field {
        Field::Freq => b.freq = v as f32,
        Field::Gain => b.gain_db = v as f32,
        Field::Q => b.q = v as f32,
        _ => {}
    }
}

fn format_field(field: Field, v: f64) -> String {
    match field {
        Field::Freq => format!("{v:.0}"),
        Field::Gain => gain_text(v),
        _ => q_text(v),
    }
}

/// A band gain as the armed cell carries it.
///
/// The WIDTH and GAIN columns show one decimal, but the Console's GAIN
/// `ValueField` is 3 dp and `DESIGN.md` 5 says "gain 1 dp displayed, 3 dp
/// editable". Seeding the edit at one decimal quantised the stored value:
/// arming a band whose gain is 8.875 and pressing anything wrote 8.9 back.
fn gain_text(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" || s == "-0" {
        "0".into()
    } else {
        s.to_string()
    }
}

/// The limits the Console's `ValueField` enforces, and the registry agrees
/// with: 10 Hz, and a Q of at least 0.1.
fn clamp(field: Field, v: f64) -> f64 {
    match field {
        Field::Freq => v.clamp(10.0, 24000.0),
        Field::Gain => v.clamp(-24.0, 24.0),
        Field::Q => v.clamp(0.1, 20.0),
        _ => v,
    }
}

/// One arrow press. Frequency and Q are heard as ratios, so they step by one;
/// a semitone is the ratio the ear reads as a step (DESIGN 6.2). The Console's
/// scroll wheel moves frequency by 10 Hz, which a keyboard cannot match
/// usefully across three decades.
fn nudge(field: Field, value: f64, dir: f64, coarse: bool) -> f64 {
    const SEMITONE: f64 = 1.059_463_094_359_295_3; // 2^(1/12)
    let steps = if coarse { 10.0 } else { 1.0 };
    let v = match field {
        // A frequency is a whole number of hertz everywhere it is shown, so a
        // nudge lands on one rather than three decimals of ratio.
        Field::Freq => (value * SEMITONE.powf(dir * steps)).round(),
        Field::Q => value * SEMITONE.powf(dir * steps),
        Field::Gain => value + dir * 0.1 * steps,
        _ => value,
    };
    clamp(field, v)
}

/// Change a band's type the way the Console's `retyped(to:)` does: the
/// Linkwitz Transform repurposes the gain field for `fp` in Hz, so a leftover
/// dB gain must not survive the switch.
fn retype(p: &mut EqParamPacket, t: FilterType) {
    let was_linkwitz = p.filter_type.is_linkwitz();
    p.filter_type = t;
    if t.is_linkwitz() && !was_linkwitz {
        // Seed a target an octave below the driver, which is what the panel
        // opens on, rather than reading a dB value as a frequency.
        p.gain_db = (p.freq / 2.0).max(10.0);
        p.q = p.q.clamp(0.1, 20.0);
        p.qp = Some(p.qp.unwrap_or(0.707));
    } else if was_linkwitz && !t.is_linkwitz() {
        p.gain_db = 0.0;
        p.qp = None;
    }
}

fn apply_family(p: &mut EqParamPacket, family: Option<Family>) {
    let Some(family) = family else {
        p.filter_type = FilterType::Flat;
        return;
    };
    let current = xover::meta(p.filter_type.to_raw());
    let orders = available_orders(family);
    // A band that is not a crossover yet has order zero, and the Console snaps
    // to the closest order the family offers, which is its gentlest.
    let want = current.map(|m| m.order).unwrap_or(0);
    let order = if orders.contains(&want) {
        want
    } else {
        orders
            .iter()
            .copied()
            .min_by_key(|o| o.abs_diff(want))
            .unwrap_or(2)
    };
    let high_pass = current.map(|m| m.high_pass).unwrap_or(false);
    p.filter_type = FilterType::from_raw(
        xover::Meta {
            family,
            order,
            high_pass,
        }
        .to_type(),
    );
    if p.freq <= 0.0 {
        p.freq = 1000.0;
    }
}

/// The orders each family offers, `CrossoverFamily.availableOrders`.
pub fn available_orders(family: Family) -> Vec<u8> {
    match family {
        Family::LinkwitzRiley | Family::Bessel => vec![2, 4, 6, 8],
        Family::Butterworth => (1..=8).collect(),
    }
}

/// The Console's hierarchical type menu, flattened with group headers.
pub fn type_menu(
    state: &DeviceState,
    include_linkwitz: bool,
    current: FilterType,
) -> (PopupList, Vec<Option<FilterType>>) {
    let groups: &[(&str, &[FilterType])] = &[
        ("Off", &[FilterType::Flat]),
        ("Peaking", &[FilterType::Peaking]),
        ("Low Shelf", &[FilterType::LowShelf1, FilterType::LowShelf]),
        (
            "High Shelf",
            &[FilterType::HighShelf1, FilterType::HighShelf],
        ),
        ("High Cut", &[FilterType::LowPass1, FilterType::LowPass]),
        ("Low Cut", &[FilterType::HighPass1, FilterType::HighPass]),
        ("Notch", &[FilterType::Notch]),
        ("All Pass", &[FilterType::AllPass1, FilterType::AllPass]),
        ("Linkwitz Transform", &[FilterType::LinkwitzTransform]),
    ];
    let available = available_types(state, include_linkwitz);
    let mut items = Vec::new();
    let mut map: Vec<Option<FilterType>> = Vec::new();
    let mut selected = 0;
    for (name, variants) in groups {
        let present: Vec<FilterType> = variants
            .iter()
            .copied()
            .filter(|t| available.contains(t))
            .collect();
        match present.len() {
            0 => {}
            1 => {
                if present[0] == current {
                    selected = items.len();
                }
                items.push((*name).to_string());
                map.push(Some(present[0]));
            }
            _ => {
                items.push(format!("#{name}"));
                map.push(None);
                for t in present {
                    if t == current {
                        selected = items.len();
                    }
                    items.push(order_label(t).to_string());
                    map.push(Some(t));
                }
            }
        }
    }
    (PopupList::new("Type", items, selected), map)
}

/// The submenu title for a shape that comes in two orders,
/// `FilterType.orderLabel`.
fn order_label(t: FilterType) -> &'static str {
    match t {
        FilterType::LowShelf1
        | FilterType::HighShelf1
        | FilterType::LowPass1
        | FilterType::HighPass1 => "6 dB/oct",
        FilterType::LowShelf
        | FilterType::HighShelf
        | FilterType::LowPass
        | FilterType::HighPass => "12 dB/oct",
        FilterType::AllPass1 => "180°",
        FilterType::AllPass => "360°",
        _ => "",
    }
}

/// The XO tab's FAMILY picker: Off plus the three families.
pub fn family_menu(current: FilterType) -> (PopupList, Vec<Option<Family>>) {
    let map = vec![
        None,
        Some(Family::LinkwitzRiley),
        Some(Family::Butterworth),
        Some(Family::Bessel),
    ];
    let items: Vec<String> = map
        .iter()
        .map(|f| match f {
            None => "Off".to_string(),
            Some(f) => f.label().to_string(),
        })
        .collect();
    let now = xover::meta(current.to_raw()).map(|m| m.family);
    let selected = map.iter().position(|f| *f == now).unwrap_or(0);
    (PopupList::new("Family", items, selected), map)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_action_strip_greys_what_would_do_nothing() {
        let state = crate::shell::fixture::state();
        // FL has five live bands, none bypassed.
        let list = super::FilterList::new(0);
        let a = list.actions(&state, true);
        let get = |name: &str| a.iter().find(|(l, _)| l == name).map(|(_, e)| *e).unwrap();
        assert!(!get("Enable All"), "nothing is bypassed");
        assert!(get("Bypass All"));
        assert!(get("Clear All"));
        assert!(!get("PEQ") && get("XO"), "PEQ is the current mode");
        // An input has no crossover tabs.
        assert!(!list.actions(&state, false).iter().any(|(l, _)| l == "XO"));
        // A channel with no bands offers nothing.
        let empty = super::FilterList::new(2);
        let a = empty.actions(&state, false);
        assert!(a.iter().filter(|(l, _)| l != "|").all(|(_, e)| !e), "{a:?}");
    }

    use super::*;
    use crate::shell::fixture;
    use crate::theme::{ColorDepth, Glyphs};
    use crate::widgets::testing::key;

    fn list(channel: usize) -> FilterList {
        FilterList::new(channel)
    }

    fn draw(l: &mut FilterList, w: u16, h: u16) -> String {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let state = fixture::state();
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        term.draw(|f| l.draw(f.area(), f.buffer_mut(), &t, &state, true))
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

    #[test]
    fn the_peq_table_has_the_consoles_columns_and_hides_what_a_type_ignores() {
        let mut l = list(0);
        let f = draw(&mut l, 56, 11);
        let lines: Vec<&str> = f.lines().collect();
        assert!(
            lines[0].contains("# TYPE") && lines[0].contains("FREQ"),
            "{f}"
        );
        assert!(
            lines[0].contains("GAIN") && lines[0].contains("WIDTH"),
            "{f}"
        );
        // A 12 dB/oct shelf has a gain and a Q, exactly as DESIGN 2.1's
        // reference row draws it.
        assert!(lines[1].contains("Low Shelf 12 dB/oct"), "{f}");
        assert!(
            lines[1].contains("105 Hz") && lines[1].contains("+8.8 dB"),
            "{f}"
        );
        assert!(
            lines[1].trim_end().ends_with("0.707"),
            "a 12 dB/oct shelf carries its Q in WIDTH: {f}"
        );
        // A peaking band has both.
        assert!(
            lines[2].contains("64 Hz") && lines[2].contains("0.3"),
            "{f}"
        );
        // Unset bands say Off and show nothing else.
        assert!(lines[6].contains("Off"), "{f}");
        // The armed disc is in the margin.
        assert!(lines[1].contains('●'), "{f}");
    }

    /// D54: `DESIGN.md` 7.6 draws the Linkwitz row's four values inline,
    /// `f0 40 Hz Q0 0.5 → fp 25 Hz Qp 0.71  +8.2 dB  ⚙`. The row used to show
    /// f0, fp and the gear alone, so half its parameters were invisible.
    #[test]
    fn the_linkwitz_row_shows_all_four_values_and_the_boost() {
        let mut state = fixture::state();
        let (_, eq, _) = dspi_proto::generated::SECTIONS
            .iter()
            .find(|(n, _, _)| *n == "eq")
            .copied()
            .unwrap();
        // Channel 16 band 0: a Linkwitz Transform, f0 40, Q0 0.5, fp 25.
        let at = eq + 16 * 12 * 16;
        state.bulk.patch(at, &[11, 0]);
        state
            .bulk
            .patch(at + 2, &((0.71 * 512.0) as u16).to_le_bytes());
        state.bulk.patch(at + 4, &40.0f32.to_le_bytes());
        state.bulk.patch(at + 8, &0.5f32.to_le_bytes());
        state.bulk.patch(at + 12, &25.0f32.to_le_bytes());

        let mut l = list(16);
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 11)).unwrap();
        term.draw(|f| l.draw(f.area(), f.buffer_mut(), &t, &state, true))
            .unwrap();
        let buf = term.backend().buffer();
        let line: String = (0..90).map(|x| buf[(x, 1)].symbol()).collect();
        assert!(line.contains("Linkwitz Transform"), "{line}");
        assert!(
            line.contains("f0 40 Hz Q0 0.5 → fp 25 Hz Qp 0.709"),
            "{line}"
        );
        assert!(line.contains("+8.2 dB"), "the implied DC boost: {line}");
        assert!(line.contains('⚙'), "{line}");

        // ASCII glyphs get an ASCII arrow and button.
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Ascii);
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 11)).unwrap();
        term.draw(|f| l.draw(f.area(), f.buffer_mut(), &t, &state, true))
            .unwrap();
        let buf = term.backend().buffer();
        let line: String = (0..90).map(|x| buf[(x, 1)].symbol()).collect();
        assert!(
            line.contains("-> fp 25 Hz") && line.contains("cfg"),
            "{line}"
        );
    }

    #[test]
    fn the_xo_table_names_the_family_direction_and_slope() {
        let mut l = list(16);
        l.mode = FilterMode::Xo;
        let f = draw(&mut l, 56, 6);
        assert!(f.contains("FAMILY"), "{f}");
        assert!(f.contains("SLOPE"), "{f}");
        // The fixture puts a plain 12 dB/oct low pass on the sub.
        assert!(f.contains("Butterworth") || f.contains("Off"), "{f}");
    }

    #[test]
    fn arrows_walk_the_bands_and_the_fields_a_type_offers() {
        let state = fixture::state();
        let mut l = list(0);
        assert_eq!(l.handle(key(KeyCode::Up), &state), ScreenEvent::Unhandled);
        assert_eq!(l.handle(key(KeyCode::Down), &state), ScreenEvent::Handled);
        assert_eq!(l.band, 1);
        // Band 2 is peaking: type, freq, gain, Q.
        l.handle(key(KeyCode::Right), &state);
        l.handle(key(KeyCode::Right), &state);
        l.handle(key(KeyCode::Right), &state);
        assert_eq!(l.field, 3);
        l.handle(key(KeyCode::Right), &state);
        assert_eq!(l.field, 3, "clamped to the fields this type has");
        // Band 1 is a 12 dB/oct low shelf, which has all four.
        l.handle(key(KeyCode::Up), &state);
        assert_eq!(l.band, 0);
        assert_eq!(l.field, 0);
        for _ in 0..4 {
            l.handle(key(KeyCode::Right), &state);
        }
        assert_eq!(l.field, 3, "type, freq, gain, Q");
    }

    /// D3: the second-order shelves take a Q, and it has to be reachable from
    /// the list, not only visible in it.
    #[test]
    fn a_twelve_db_per_octave_shelf_offers_an_editable_q() {
        let state = fixture::state();
        let mut l = list(0);
        let shelf = l.bands(&state)[0];
        assert_eq!(shelf.filter_type, FilterType::LowShelf);
        assert_eq!(
            l.fields(&shelf),
            vec![Field::Type, Field::Freq, Field::Gain, Field::Q]
        );
        // The first-order shelf beside it in the menu still has none.
        let mut first_order = shelf;
        first_order.filter_type = FilterType::LowShelf1;
        assert_eq!(
            l.fields(&first_order),
            vec![Field::Type, Field::Freq, Field::Gain]
        );

        l.field = 3;
        assert_eq!(l.handle(key(KeyCode::Enter), &state), ScreenEvent::Handled);
        assert_eq!(l.edit.as_ref().unwrap().text, "0.707");
        for c in "1.2".chars() {
            l.handle(key(KeyCode::Char(c)), &state);
        }
        assert_eq!(
            l.handle(key(KeyCode::Enter), &state),
            ScreenEvent::Command("eq in.1 1 lowshelf 105 1.2 8.8".into())
        );
    }

    /// D21: the Console's GAIN field is 3 dp. Arming at one decimal threw away
    /// the other two, and the next nudge wrote the rounded value back.
    #[test]
    fn arming_a_gain_keeps_all_three_decimals() {
        let mut state = fixture::state();
        let (_, eq, _) = dspi_proto::generated::SECTIONS
            .iter()
            .find(|(n, _, _)| *n == "eq")
            .copied()
            .unwrap();
        // Band 1 of channel 0 is the low shelf; give it a gain with three.
        state.bulk.patch(eq + 12, &8.875f32.to_le_bytes());

        let mut l = list(0);
        l.field = 2; // GAIN
        l.handle(key(KeyCode::Enter), &state);
        assert_eq!(l.edit.as_ref().unwrap().text, "8.875");
        // The table itself still shows one decimal.
        let cells = l.row(0, &l.bands(&state)[0]);
        assert_eq!(cells[3].text, "+8.9 dB");

        // And a nudge moves the stored value, not the rounded one.
        match l.handle(key(KeyCode::Right), &state) {
            ScreenEvent::Command(c) => {
                assert!(c.ends_with(" 8.975"), "{c}");
                assert_eq!(l.edit.as_ref().unwrap().text, "8.975");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_digit_jumps_to_a_band_and_zero_means_ten() {
        let state = fixture::state();
        let mut l = list(0);
        l.handle(key(KeyCode::Char('4')), &state);
        assert_eq!(l.band, 3);
        l.handle(key(KeyCode::Char('0')), &state);
        assert_eq!(l.band, 9);
    }

    #[test]
    fn editing_a_field_writes_the_whole_band() {
        let state = fixture::state();
        let mut l = list(0);
        l.band = 2;
        l.field = 1; // FREQ
        assert_eq!(l.handle(key(KeyCode::Enter), &state), ScreenEvent::Handled);
        assert_eq!(l.edit.as_ref().unwrap().text, "2856");
        for c in "3000".chars() {
            l.handle(key(KeyCode::Char(c)), &state);
        }
        assert_eq!(
            l.handle(key(KeyCode::Enter), &state),
            ScreenEvent::Command("eq in.1 3 peak 3000 3.58 -8.6".into())
        );
        assert!(l.edit.is_none());
    }

    #[test]
    fn arrows_nudge_an_armed_field_live_and_escape_reverts() {
        let state = fixture::state();
        let mut l = list(0);
        l.band = 2;
        l.field = 1;
        l.handle(key(KeyCode::Enter), &state);
        match l.handle(key(KeyCode::Right), &state) {
            ScreenEvent::Command(c) => {
                assert!(c.starts_with("eq in.1 3 peak 3026"), "{c}");
            }
            other => panic!("{other:?}"),
        }
        l.handle(key(KeyCode::Esc), &state);
        assert!(l.edit.is_none());
    }

    #[test]
    fn space_toggles_the_bypass_of_one_band() {
        let state = fixture::state();
        let mut l = list(0);
        l.band = 1;
        assert_eq!(
            l.handle(key(KeyCode::Char(' ')), &state),
            ScreenEvent::Command("eq.bypass in.1 2 on".into())
        );
        // An Off band has no audio contribution, so its flag does nothing.
        l.band = 8;
        assert_eq!(
            l.handle(key(KeyCode::Char(' ')), &state),
            ScreenEvent::Handled
        );
    }

    #[test]
    fn a_linked_pair_mirrors_every_write() {
        let state = fixture::state();
        let mut l = list(0);
        l.mirror = Some(1);
        l.band = 1;
        assert_eq!(
            l.handle(key(KeyCode::Char(' ')), &state),
            ScreenEvent::Command("eq.bypass in.1 2 on\neq.bypass in.2 2 on".into())
        );
    }

    #[test]
    fn enable_all_and_bypass_all_touch_every_band_that_is_not_off() {
        let state = fixture::state();
        let mut l = list(0);
        match l.handle(key(KeyCode::Char('A')), &state) {
            ScreenEvent::Command(c) => {
                let lines: Vec<&str> = c.lines().collect();
                assert_eq!(lines.len(), 5, "the fixture sets five bands: {c}");
                assert_eq!(lines[0], "eq.bypass in.1 1 on");
            }
            other => panic!("{other:?}"),
        }
        // Nothing is bypassed yet, so Enable All is a no-op.
        assert_eq!(
            l.handle(key(KeyCode::Char('a')), &state),
            ScreenEvent::Handled
        );
    }

    #[test]
    fn bypassing_all_crossovers_asks_the_consoles_question_first() {
        let state = fixture::state();
        let mut l = list(16);
        l.mode = FilterMode::Xo;
        match l.handle(key(KeyCode::Char('A')), &state) {
            ScreenEvent::Dialog(d) => {
                assert_eq!(d.title, "Bypass this output's crossovers?");
                assert!(
                    d.body
                        .contains("can damage unprotected drivers such as tweeters")
                );
                assert!(d.critical);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            l.dialog_result(DialogOutcome::Cancelled, &state),
            ScreenEvent::Handled
        );
    }

    #[test]
    fn clear_all_confirms_and_then_resets_every_band() {
        let state = fixture::state();
        let mut l = list(0);
        match l.handle(key(KeyCode::Char('D')), &state) {
            ScreenEvent::Dialog(d) => assert_eq!(d.title, "Clear All Bands?"),
            other => panic!("{other:?}"),
        }
        match l.dialog_result(DialogOutcome::Button(0), &state) {
            ScreenEvent::Command(c) => {
                let lines: Vec<&str> = c.lines().collect();
                assert_eq!(lines.len(), 10);
                assert_eq!(lines[0], "eq in.1 1 flat 1000 0.707 0");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_type_menu_is_the_consoles_groups_gated_by_the_firmware() {
        let state = fixture::state();
        let (popup, map) = type_menu(&state, true, FilterType::Peaking);
        assert_eq!(popup.items[0], "Off");
        assert_eq!(popup.items[1], "Peaking");
        assert_eq!(popup.items[2], "#Low Shelf");
        assert_eq!(popup.items[3], "6 dB/oct");
        assert_eq!(popup.items[4], "12 dB/oct");
        assert_eq!(popup.cursor, 1, "opens on the current type");
        assert!(popup.items.contains(&"Linkwitz Transform".to_string()));
        assert_eq!(map[2], None, "a group header picks nothing");
        assert_eq!(map[3], Some(FilterType::LowShelf1));

        let (popup, _) = type_menu(&state, false, FilterType::Peaking);
        assert!(!popup.items.contains(&"Linkwitz Transform".to_string()));
    }

    #[test]
    fn picking_a_type_writes_the_band_with_the_new_shape() {
        let state = fixture::state();
        let mut l = list(0);
        l.band = 2;
        let ScreenEvent::Popup(_) = l.handle(key(KeyCode::Enter), &state) else {
            panic!("the type field should open a picker");
        };
        // Index 4 is the 12 dB/oct low shelf.
        assert_eq!(
            l.popup_result(Some(4), &state),
            ScreenEvent::Command("eq in.1 3 lowshelf 2856 3.58 -8.6".into())
        );
    }

    #[test]
    fn the_xo_pickers_choose_a_family_a_direction_and_a_slope() {
        let state = fixture::state();
        let mut l = list(16);
        l.mode = FilterMode::Xo;
        // FAMILY: Linkwitz-Riley, which keeps the current 12 dB/oct order.
        let ScreenEvent::Popup(p) = l.handle(key(KeyCode::Enter), &state) else {
            panic!("family picker")
        };
        assert_eq!(
            p.items,
            vec!["Off", "Linkwitz-Riley", "Butterworth", "Bessel"]
        );
        assert_eq!(
            l.popup_result(Some(1), &state),
            ScreenEvent::Command("eq out.9 20 lr2lp 80 0.707 0".into())
        );
        // With a crossover in place the direction and slope pickers open on it.
        let mut state = state;
        let (_, xo, _) = dspi_proto::generated::SECTIONS
            .iter()
            .find(|(n, _, _)| *n == "crossovers")
            .copied()
            .unwrap();
        state.bulk.patch(xo + 16 * 4 * 16, &[32]);
        l.field = 1;
        let ScreenEvent::Popup(p) = l.handle(key(KeyCode::Enter), &state) else {
            panic!("direction picker")
        };
        assert_eq!(p.items, vec!["Low Pass", "High Pass"]);
        assert_eq!(
            l.popup_result(Some(1), &state),
            ScreenEvent::Command("eq out.9 20 lr2hp 80 0.707 0".into())
        );
        l.field = 2;
        let ScreenEvent::Popup(p) = l.handle(key(KeyCode::Enter), &state) else {
            panic!("slope picker")
        };
        assert_eq!(
            p.items,
            vec!["12 dB/oct", "24 dB/oct", "36 dB/oct", "48 dB/oct"]
        );
        assert_eq!(
            l.popup_result(Some(1), &state),
            ScreenEvent::Command("eq out.9 20 lr4lp 80 0.707 0".into())
        );
    }

    #[test]
    fn x_switches_between_peq_and_xo_only_where_the_tabs_are() {
        let state = fixture::state();
        let mut l = list(16).tabbed(true);
        assert_eq!(
            l.handle(key(KeyCode::Char('x')), &state),
            ScreenEvent::Handled
        );
        assert_eq!(l.mode, FilterMode::Xo);
        let mut l = list(0);
        assert_eq!(
            l.handle(key(KeyCode::Char('x')), &state),
            ScreenEvent::Unhandled
        );
    }

    #[test]
    fn the_bypass_disc_disappears_when_the_firmware_has_none() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let mut state = fixture::state();
        state.caps.firmware = "1.1.3".into();
        let mut l = list(0);
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(56, 11)).unwrap();
        term.draw(|f| l.draw(f.area(), f.buffer_mut(), &t, &state, true))
            .unwrap();
        let buf = term.backend().buffer();
        let f: String = (0..11)
            .map(|y| (0..56).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect();
        assert!(!f.contains('●'), "{f}");
    }
}
