//! The output page: the Console's `ChannelSettingsView` over its filter list.
//!
//! The routing panel first, because what reaches this output is the first
//! thing anyone wants to know, then the output's own gain, delay and mute, and
//! then the PEQ or crossover bank behind the tab strip. Every routing edit is
//! one `mix` write, which is how the firmware stores a crosspoint.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use dspi_session::DeviceState;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

use super::filters::{FilterList, FilterMode};
use super::{Shared, channel_name, max_delay_ms, number, output_channel, supports_crossover};
use crate::shell::{Screen, ScreenEvent};
use crate::theme::{ChannelRole, Glyphs, Theme};
use crate::widgets::{DialogOutcome, KeyHelp, NumberEdit, ParamRow, Taper};

/// The USB stereo pair every device has, which is what the Console's routing
/// panel shows (`BASE_MATRIX_INPUTS`; config.h:702 on the two-channel model).
/// The rest of the matrix lives in the Matrix Mixer.
pub const BASE_INPUTS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    /// One input's crosspoint row: connect, gain, invert.
    Route(usize),
    Gain,
    Delay,
    Mute,
    Tabs,
}

pub struct OutputPage {
    pub output: usize,
    /// The channel's name at the time the page was made, for the title row.
    /// The shell asks for the title before it draws, so this is read once
    /// rather than every frame; a rename rebuilds the page.
    name: String,
    #[allow(dead_code)]
    shared: Shared,
    pub list: FilterList,
    /// `None` while the filter list has the keyboard.
    header: Option<usize>,
    /// Which column of a routing row the cursor is on.
    column: usize,
    edit: Option<NumberEdit>,
    /// The filter list raised the dialog on screen.
    list_dialog: bool,
}

const KEYS: &[KeyHelp] = &[
    KeyHelp::new("↑ ↓", "Row, or band"),
    KeyHelp::new("← →", "Field"),
    KeyHelp::new("Enter", "Edit"),
    KeyHelp::new("Space", "Connect, mute, bypass"),
    KeyHelp::new("i", "Invert"),
    KeyHelp::new("1-9,0", "Jump to a band"),
    KeyHelp::new("a", "Enable All"),
    KeyHelp::new("A", "Bypass All"),
    KeyHelp::new("D", "Clear All"),
    KeyHelp::new("x", "PEQ / XO"),
    KeyHelp::new("Backspace", "Reset to 0"),
];

impl OutputPage {
    pub fn new(output: usize, shared: Shared, state: &DeviceState) -> Self {
        let channel = output_channel(state, output);
        Self {
            output,
            name: channel_name(state, channel),
            shared,
            list: FilterList::new(channel)
                .linkwitz(true)
                .tabbed(supports_crossover(state)),
            header: Some(0),
            column: 0,
            edit: None,
            list_dialog: false,
        }
    }

    fn inputs(&self, state: &DeviceState) -> usize {
        (state.caps.num_inputs as usize).min(BASE_INPUTS)
    }

    fn items(&self, state: &DeviceState) -> Vec<Item> {
        let mut v: Vec<Item> = (0..self.inputs(state)).map(Item::Route).collect();
        v.push(Item::Gain);
        v.push(Item::Delay);
        v.push(Item::Mute);
        if supports_crossover(state) {
            v.push(Item::Tabs);
        }
        v
    }

    fn item(&self, state: &DeviceState) -> Option<Item> {
        let items = self.items(state);
        self.header
            .and_then(|i| items.get(i.min(items.len().saturating_sub(1))).copied())
    }

    /// One crosspoint write, with whatever the other two fields already are.
    fn mix_command(&self, input: usize, enabled: bool, gain: f32, invert: bool) -> String {
        let mut s = format!(
            "mix {input} {} {} {}",
            self.output,
            if enabled { "on" } else { "off" },
            number(gain)
        );
        if invert {
            s.push_str(" inv");
        }
        s
    }

    fn gain_row<'a>(&self, state: &DeviceState, theme: &'a Theme) -> ParamRow<'a> {
        ParamRow::new(
            "GAIN",
            state.output(self.output).gain_db as f64,
            -60.0,
            10.0,
            "dB",
            theme,
        )
        .step(0.5)
        .decimals(1)
        .compact(true)
    }

    fn delay_row<'a>(&self, state: &DeviceState, theme: &'a Theme) -> ParamRow<'a> {
        ParamRow::new(
            "DELAY",
            state.output(self.output).delay_ms as f64,
            0.0,
            max_delay_ms(state),
            "ms",
            theme,
        )
        .step(0.1)
        .decimals(1)
        .taper(Taper::Linear)
        .compact(true)
    }

    fn nudged(&self, state: &DeviceState, item: Item, dir: f64, coarse: bool) -> f64 {
        let out = state.output(self.output);
        let mult = if coarse { 10.0 } else { 1.0 };
        match item {
            Item::Gain => (out.gain_db as f64 + dir * 0.5 * mult).clamp(-60.0, 10.0),
            Item::Delay => (out.delay_ms as f64 + dir * 0.1 * mult).clamp(0.0, max_delay_ms(state)),
            _ => 0.0,
        }
    }

    fn write(&self, item: Item, v: f64) -> ScreenEvent {
        let o = self.output;
        ScreenEvent::Command(match item {
            Item::Gain => format!("out.gain {o} {}", number(v as f32)),
            Item::Delay => format!("out.delay {o} {}", number(v as f32)),
            _ => return ScreenEvent::Handled,
        })
    }

    fn draw_header(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        state: &DeviceState,
        focused: bool,
    ) -> u16 {
        let items = self.items(state);
        let here = |i: Item| focused && self.item(state) == Some(i);
        let ascii = theme.glyphs == Glyphs::Ascii;
        let mut y = area.y;
        let bottom = area.y + area.height;

        for input in 0..self.inputs(state) {
            if y >= bottom {
                return y;
            }
            let c = state.crosspoint(input, self.output);
            let selected = here(Item::Route(input));
            let color = theme.hue_for(ChannelRole::Input(input as u8), selected);
            buf.set_string(area.x, y, if selected { "▸" } else { " " }, theme.focused());
            let dot = match (c.enabled, ascii) {
                (true, true) => "*",
                (false, true) => "o",
                (true, false) => "●",
                (false, false) => "○",
            };
            buf.set_string(
                area.x + 1,
                y,
                dot,
                if c.enabled {
                    Style::default().fg(color)
                } else {
                    theme.label()
                },
            );
            let name = channel_name(state, input);
            buf.set_string(
                area.x + 3,
                y,
                crate::widgets::text::fit_left(&name, 10),
                if c.enabled {
                    Style::default().fg(color)
                } else {
                    theme.label()
                },
            );
            let gain = match (&self.edit, selected && self.column == 1) {
                (Some(e), true) => format!("[{}]", e.text),
                _ => format!("{:+.1} dB", c.gain_db),
            };
            buf.set_string(
                area.x + 14,
                y,
                crate::widgets::text::fit_right(&gain, 9),
                cell_style(theme, selected && self.column == 1, self.edit.is_some()),
            );
            buf.set_string(
                area.x + 25,
                y,
                "INV",
                if c.phase_invert {
                    theme.warning_style()
                } else if selected && self.column == 2 {
                    theme.focused()
                } else {
                    theme.label()
                },
            );
            y += 1;
        }

        let w = area.width.min(48);
        if y < bottom {
            self.gain_row(state, theme)
                .focused(here(Item::Gain))
                .edit(self.edit.as_ref().filter(|_| here(Item::Gain)))
                .render(Rect::new(area.x, y, w, 1), buf);
            y += 1;
        }
        if y < bottom {
            self.delay_row(state, theme)
                .focused(here(Item::Delay))
                .edit(self.edit.as_ref().filter(|_| here(Item::Delay)))
                .render(Rect::new(area.x, y, w, 1), buf);
            y += 1;
        }
        if y < bottom {
            let muted = state.output(self.output).mute;
            buf.set_string(
                area.x,
                y,
                if here(Item::Mute) { "▸" } else { " " },
                theme.focused(),
            );
            buf.set_string(area.x + 1, y, "MUTE", theme.section());
            let text = match (muted, ascii) {
                (true, true) => " MUTED ",
                (false, true) => " o ",
                (true, false) => " MUTED ",
                (false, false) => " ○ ",
            };
            buf.set_string(
                area.x + 9,
                y,
                text,
                if muted {
                    theme.pill(theme.danger)
                } else {
                    theme.label()
                },
            );
            y += 1;
        }
        if items.contains(&Item::Tabs) && y < bottom {
            let peq = self.list.mode == FilterMode::Peq;
            let sel = |on: bool| {
                if !on {
                    theme.label()
                } else if here(Item::Tabs) {
                    theme.pill(theme.accent)
                } else {
                    theme.pill(theme.fg)
                }
            };
            buf.set_string(
                area.x,
                y,
                if here(Item::Tabs) { "▸" } else { " " },
                theme.focused(),
            );
            buf.set_string(area.x + 1, y, " PEQ ", sel(peq));
            buf.set_string(area.x + 6, y, "│", theme.chrome_style());
            buf.set_string(area.x + 7, y, " XO ", sel(!peq));
            y += 1;
        }
        y
    }
}

fn cell_style(theme: &Theme, focused: bool, armed: bool) -> Style {
    if focused && armed {
        theme.editing()
    } else if focused {
        theme.focused()
    } else {
        theme.value()
    }
}

impl Screen for OutputPage {
    fn title(&self) -> String {
        self.name.clone()
    }

    fn draw(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        state: &DeviceState,
        focused: bool,
    ) {
        if area.height < 2 || area.width < 20 {
            return;
        }
        let y = self.draw_header(area, buf, theme, state, focused);
        let used = y - area.y;
        if area.height > used {
            let list = Rect::new(area.x, y, area.width, area.height - used);
            self.list
                .draw(list, buf, theme, state, focused && self.header.is_none());
        }
    }

    fn handle(&mut self, key: KeyEvent, state: &DeviceState) -> ScreenEvent {
        let Some(index) = self.header else {
            let ev = self.list.handle(key, state);
            if ev == ScreenEvent::Unhandled && key.code == KeyCode::Up {
                self.header = Some(self.items(state).len() - 1);
                return ScreenEvent::Handled;
            }
            if matches!(ev, ScreenEvent::Dialog(_) | ScreenEvent::Popup(_)) {
                self.list_dialog = true;
            }
            return ev;
        };

        let items = self.items(state);
        let item = items[index.min(items.len() - 1)];
        let coarse = key.modifiers.contains(KeyModifiers::SHIFT);

        if self.edit.is_some() {
            return self.handle_edit(key, state, item, coarse);
        }

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.header = Some(index.saturating_sub(1));
                self.column = 0;
                ScreenEvent::Handled
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if index + 1 < items.len() {
                    self.header = Some(index + 1);
                    self.column = 0;
                } else {
                    self.header = None;
                    self.list.band = 0;
                    self.list.field = 0;
                }
                ScreenEvent::Handled
            }
            KeyCode::Left | KeyCode::Right => {
                let dir = if key.code == KeyCode::Left { -1.0 } else { 1.0 };
                match item {
                    Item::Route(_) => {
                        self.column = if dir < 0.0 {
                            self.column.saturating_sub(1)
                        } else {
                            (self.column + 1).min(2)
                        };
                        ScreenEvent::Handled
                    }
                    Item::Gain | Item::Delay => {
                        let v = self.nudged(state, item, dir, coarse);
                        self.write(item, v)
                    }
                    Item::Tabs => {
                        self.list.mode = if dir < 0.0 {
                            FilterMode::Peq
                        } else {
                            FilterMode::Xo
                        };
                        self.list.band = 0;
                        self.list.field = 0;
                        ScreenEvent::Handled
                    }
                    Item::Mute => ScreenEvent::Handled,
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => match item {
                Item::Route(input) => {
                    let c = state.crosspoint(input, self.output);
                    match self.column {
                        1 => {
                            self.edit = Some(NumberEdit {
                                text: format!("{:.1}", c.gain_db),
                                dirty: false,
                            });
                            ScreenEvent::Handled
                        }
                        2 => ScreenEvent::Command(self.mix_command(
                            input,
                            c.enabled,
                            c.gain_db,
                            !c.phase_invert,
                        )),
                        _ => ScreenEvent::Command(self.mix_command(
                            input,
                            !c.enabled,
                            c.gain_db,
                            c.phase_invert,
                        )),
                    }
                }
                Item::Gain => {
                    self.edit = Some(NumberEdit {
                        text: format!("{:.1}", state.output(self.output).gain_db),
                        dirty: false,
                    });
                    ScreenEvent::Handled
                }
                Item::Delay => {
                    self.edit = Some(NumberEdit {
                        text: format!("{:.1}", state.output(self.output).delay_ms),
                        dirty: false,
                    });
                    ScreenEvent::Handled
                }
                Item::Mute => {
                    let muted = state.output(self.output).mute;
                    ScreenEvent::Command(format!(
                        "out.mute {} {}",
                        self.output,
                        if muted { "off" } else { "on" }
                    ))
                }
                Item::Tabs => {
                    self.list.mode = match self.list.mode {
                        FilterMode::Peq => FilterMode::Xo,
                        FilterMode::Xo => FilterMode::Peq,
                    };
                    self.list.band = 0;
                    self.list.field = 0;
                    ScreenEvent::Handled
                }
            },
            KeyCode::Char('i') => match item {
                Item::Route(input) => {
                    let c = state.crosspoint(input, self.output);
                    ScreenEvent::Command(self.mix_command(
                        input,
                        c.enabled,
                        c.gain_db,
                        !c.phase_invert,
                    ))
                }
                _ => ScreenEvent::Unhandled,
            },
            KeyCode::Backspace => match item {
                Item::Route(input) => {
                    let c = state.crosspoint(input, self.output);
                    ScreenEvent::Command(self.mix_command(input, c.enabled, 0.0, c.phase_invert))
                }
                Item::Gain | Item::Delay => self.write(item, 0.0),
                _ => ScreenEvent::Handled,
            },
            KeyCode::Char('0'..='9') => {
                self.header = None;
                self.list.handle(key, state)
            }
            _ => {
                let ev = self.list.handle(key, state);
                if matches!(ev, ScreenEvent::Dialog(_) | ScreenEvent::Popup(_)) {
                    self.list_dialog = true;
                }
                ev
            }
        }
    }

    fn actions(&self, state: &DeviceState) -> Vec<(String, bool)> {
        self.list.actions(state, true)
    }

    fn keys(&self) -> &'static [KeyHelp] {
        KEYS
    }

    fn popup_result(&mut self, choice: Option<usize>, state: &DeviceState) -> ScreenEvent {
        self.list_dialog = false;
        self.list.popup_result(choice, state)
    }

    fn dialog_result(&mut self, outcome: DialogOutcome, state: &DeviceState) -> ScreenEvent {
        self.list_dialog = false;
        self.list.dialog_result(outcome, state)
    }
}

impl OutputPage {
    fn handle_edit(
        &mut self,
        key: KeyEvent,
        state: &DeviceState,
        item: Item,
        coarse: bool,
    ) -> ScreenEvent {
        if matches!(key.code, KeyCode::Left | KeyCode::Right) {
            let dir = if key.code == KeyCode::Left { -1.0 } else { 1.0 };
            let step = if coarse { 10.0 } else { 1.0 };
            let (v, ev) = match item {
                Item::Route(input) => {
                    let c = state.crosspoint(input, self.output);
                    // The Console's crosspoint field scrolls in half-decibel
                    // steps.
                    let v = (c.gain_db as f64 + dir * 0.5 * step).clamp(-60.0, 12.0);
                    (
                        v,
                        ScreenEvent::Command(self.mix_command(
                            input,
                            c.enabled,
                            v as f32,
                            c.phase_invert,
                        )),
                    )
                }
                other => {
                    let v = self.nudged(state, other, dir, coarse);
                    (v, self.write(other, v))
                }
            };
            self.edit = Some(NumberEdit {
                text: format!("{v:.1}"),
                dirty: false,
            });
            return ev;
        }
        let Some(edit) = self.edit.as_mut() else {
            return ScreenEvent::Handled;
        };
        match edit.handle(key) {
            Some(crate::widgets::Action::Committed(v)) => {
                self.edit = None;
                match item {
                    Item::Route(input) => {
                        let c = state.crosspoint(input, self.output);
                        ScreenEvent::Command(self.mix_command(
                            input,
                            c.enabled,
                            v.clamp(-60.0, 12.0) as f32,
                            c.phase_invert,
                        ))
                    }
                    Item::Gain => self.write(item, v.clamp(-60.0, 10.0)),
                    Item::Delay => self.write(item, v.clamp(0.0, max_delay_ms(state))),
                    _ => ScreenEvent::Handled,
                }
            }
            Some(crate::widgets::Action::Closed) => {
                self.edit = None;
                ScreenEvent::Handled
            }
            _ => ScreenEvent::Handled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::shared;
    use crate::shell::fixture;
    use crate::theme::{ColorDepth, Glyphs};
    use crate::widgets::testing::key;

    fn page(output: usize) -> (OutputPage, DeviceState) {
        let state = fixture::state();
        let page = OutputPage::new(output, shared(), &state);
        (page, state)
    }

    fn draw(p: &mut OutputPage, state: &DeviceState, w: u16, h: u16) -> String {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        term.draw(|f| p.draw(f.area(), f.buffer_mut(), &t, state, true))
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
    fn the_header_is_routing_then_gain_delay_mute_then_the_tabs() {
        let (mut p, state) = page(0);
        let f = draw(&mut p, &state, 94, 21);
        let lines: Vec<&str> = f.lines().collect();
        assert!(lines[0].contains("FL") && lines[0].contains("INV"), "{f}");
        assert!(lines[1].contains("FR"), "{f}");
        assert!(
            lines[2].contains("GAIN") && lines[2].contains("0.0 dB"),
            "{f}"
        );
        assert!(
            lines[3].contains("DELAY") && lines[3].contains("0.0 ms"),
            "{f}"
        );
        assert!(lines[4].contains("MUTE"), "{f}");
        assert!(lines[5].contains("PEQ") && lines[5].contains("XO"), "{f}");
        assert!(f.contains("TYPE"), "the filter list is beneath: {f}");
        // Only the base stereo pair; the rest is the Matrix Mixer's job.
        assert!(!f.contains("FC"), "{f}");
    }

    #[test]
    fn the_minimum_frame_fits() {
        let (mut p, state) = page(0);
        let f = draw(&mut p, &state, 56, 10);
        assert!(f.contains("GAIN") && f.contains("MUTE"), "{f}");
        assert_eq!(f.lines().count(), 10);
    }

    #[test]
    fn the_tab_strip_is_gone_on_firmware_without_crossovers() {
        let mut state = fixture::state();
        state.caps.wire_format = 10;
        let mut p = OutputPage::new(0, shared(), &state);
        let f = draw(&mut p, &state, 94, 21);
        assert!(!f.contains("XO"), "{f}");
        assert!(!p.list.can_switch_mode);
    }

    #[test]
    fn the_delay_range_follows_the_platform() {
        let state = fixture::state();
        assert_eq!(max_delay_ms(&state), 85.0);
        let mut rp2040 = fixture::state();
        rp2040.caps.platform = dspi_proto::Platform::Rp2040;
        assert_eq!(max_delay_ms(&rp2040), 42.0);
    }

    #[test]
    fn a_crosspoint_connects_inverts_and_takes_a_gain() {
        let (mut p, state) = page(0);
        // The fixture routes IN1 to OUT1 at 0 dB; Space disconnects it.
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &state),
            ScreenEvent::Command("mix 0 0 off 0".into())
        );
        assert_eq!(
            p.handle(key(KeyCode::Char('i')), &state),
            ScreenEvent::Command("mix 0 0 on 0 inv".into())
        );
        // The middle column is the crosspoint gain, in half-decibel nudges.
        p.handle(key(KeyCode::Right), &state);
        assert_eq!(p.column, 1);
        p.handle(key(KeyCode::Enter), &state);
        assert_eq!(
            p.handle(key(KeyCode::Right), &state),
            ScreenEvent::Command("mix 0 0 on 0.5".into())
        );
    }

    #[test]
    fn gain_delay_and_mute_write_the_outputs_own_settings() {
        let (mut p, state) = page(8);
        p.header = Some(2); // GAIN
        assert_eq!(
            p.handle(key(KeyCode::Right), &state),
            ScreenEvent::Command("out.gain 8 -2.5".into())
        );
        assert_eq!(
            p.handle(key(KeyCode::Backspace), &state),
            ScreenEvent::Command("out.gain 8 0".into())
        );
        p.header = Some(3); // DELAY
        assert_eq!(
            p.handle(key(KeyCode::Right), &state),
            ScreenEvent::Command("out.delay 8 0.1".into())
        );
        p.header = Some(4); // MUTE
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &state),
            ScreenEvent::Command("out.mute 8 on".into())
        );
    }

    #[test]
    fn a_typed_delay_is_clamped_to_the_platforms_maximum() {
        let (mut p, state) = page(0);
        p.header = Some(3);
        p.handle(key(KeyCode::Enter), &state);
        for c in "200".chars() {
            p.handle(key(KeyCode::Char(c)), &state);
        }
        assert_eq!(
            p.handle(key(KeyCode::Enter), &state),
            ScreenEvent::Command("out.delay 0 85".into())
        );
    }

    #[test]
    fn the_tabs_switch_the_list_and_x_does_too() {
        let (mut p, state) = page(0);
        p.header = Some(5);
        p.handle(key(KeyCode::Enter), &state);
        assert_eq!(p.list.mode, FilterMode::Xo);
        p.handle(key(KeyCode::Left), &state);
        assert_eq!(p.list.mode, FilterMode::Peq);
        p.header = None;
        p.handle(key(KeyCode::Char('x')), &state);
        assert_eq!(p.list.mode, FilterMode::Xo);
    }

    #[test]
    fn the_linkwitz_transform_is_offered_on_outputs() {
        let (p, _) = page(0);
        assert!(p.list.include_linkwitz);
    }

    #[test]
    fn down_walks_into_the_list_and_up_comes_back() {
        let (mut p, state) = page(0);
        for _ in 0..6 {
            p.handle(key(KeyCode::Down), &state);
        }
        assert!(p.header.is_none());
        p.handle(key(KeyCode::Up), &state);
        assert_eq!(p.header, Some(5));
    }

    #[test]
    fn every_advertised_key_is_handled() {
        let state = fixture::state();
        for help in KEYS {
            for k in crate::screens::tests::keys_for(help.key) {
                let mut done = false;
                for header in [Some(0usize), Some(2), Some(5), None] {
                    let mut p = OutputPage::new(0, shared(), &state);
                    p.header = header;
                    p.list.band = 2;
                    p.list.field = 1;
                    let before = (p.header, p.column, p.list.band, p.list.field, p.list.mode);
                    let ev = p.handle(k, &state);
                    let after = (p.header, p.column, p.list.band, p.list.field, p.list.mode);
                    if ev != ScreenEvent::Unhandled || before != after {
                        done = true;
                    }
                }
                assert!(
                    done,
                    "{:?} does nothing anywhere (from {:?})",
                    k.code, help.key
                );
            }
        }
    }
}
