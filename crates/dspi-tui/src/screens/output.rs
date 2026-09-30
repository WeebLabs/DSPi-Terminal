//! The output page: the Console's `ChannelSettingsView` over its filter list.
//!
//! The routing panel first, because what reaches this output is the first
//! thing anyone wants to know, then the output's own gain, delay and mute, and
//! then the PEQ or crossover bank behind the tab strip. Every routing edit is
//! one `mix` write, which is how the firmware stores a crosspoint.
//!
//! On firmware with the output limiter, its cell sits beside MUTE, where the
//! Console puts its icon, and its settings open in place of the filter list
//! (see [`super::limiter`]).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use dspi_session::DeviceState;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

use super::filters::{FilterList, FilterMode};
use super::limiter::{self, LimiterSettings};
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
    /// The limiter's settings, open in place of the filter list.
    limiter: Option<LimiterSettings>,
}

const KEYS: &[KeyHelp] = &[
    KeyHelp::new("↑ ↓", "Row, or band"),
    KeyHelp::new("← →", "Field"),
    KeyHelp::new("Enter", "Edit"),
    KeyHelp::new("Space", "Connect, mute, limiter, bypass"),
    KeyHelp::new("i", "Invert"),
    KeyHelp::new("1-9,0", "Jump to a band"),
    KeyHelp::new("a", "Enable All"),
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
            limiter: None,
        }
    }

    /// Is the cursor on the limiter cell of the MUTE row?
    fn on_limiter(&self, state: &DeviceState, item: Item) -> bool {
        item == Item::Mute && self.column == 1 && limiter::available(state, self.output)
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
            let on_limiter = here(Item::Mute) && self.on_limiter(state, Item::Mute);
            buf.set_string(
                area.x,
                y,
                if here(Item::Mute) && !on_limiter {
                    "▸"
                } else {
                    " "
                },
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
            if limiter::available(state, self.output) {
                limiter::draw_indicator(
                    area.x + 17,
                    y,
                    area.x + area.width,
                    buf,
                    theme,
                    state,
                    self.output,
                    on_limiter,
                );
            }
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

impl OutputPage {
    /// The page grammar behind `;` (DESIGN 13): the output's own gain,
    /// delay, mute and enable, a PEQ band in one line, the crossover (`xo hp
    /// 80 lr4`), and Bypass All and Clear All over the bank showing.
    fn quick_reply(&self, line: &str, state: &DeviceState) -> crate::shell::Quick {
        use super::quick::{ghost, number as num, verb};
        use crate::shell::Quick;
        const VERBS: &[&str] = &[
            "gain", "delay", "mute", "unmute", "on", "off", "xo", "bypass", "clear", "name",
            "limit", "release", "link",
        ];
        const SUMMARY: &str = "gain -3 · delay 2.5 · mute · unmute · on · off · 3 peak 1k -2 · xo hp 80 lr4 · bypass · clear · name Sub";
        const LIMITER: &str = " · limit on · limit -1 · release 100 · link 1";
        let o = self.output;
        let channel = output_channel(state, o);
        let lower = line.to_ascii_lowercase();
        let tokens: Vec<&str> = lower.split_whitespace().collect();
        let hint = |h: &str| Quick {
            fallthrough: false,
            hint: h.to_string(),
            ghost: ghost(&lower, VERBS),
            commands: Vec::new(),
        };
        // The limiter's words only where there is a limiter.
        let summary = if limiter::available(state, o) {
            format!("{SUMMARY}{LIMITER}")
        } else {
            SUMMARY.to_string()
        };
        let Some(&first) = tokens.first() else {
            return hint(&summary);
        };
        if let Ok(band) = first.parse::<usize>() {
            let max = state.caps.max_bands as usize;
            if band == 0 || band > max {
                return hint(&format!("bands are 1 to {max}"));
            }
            return self.quick_band(state, channel, band as u8 - 1, &tokens[1..]);
        }
        let one = |hint: String, command: String| Quick {
            fallthrough: false,
            hint,
            ghost: None,
            commands: vec![command],
        };
        match verb(first, VERBS) {
            Some("gain") => match tokens.get(1).and_then(|t| num(t)) {
                Some(db) => {
                    let db = db.clamp(-60.0, 10.0);
                    one(
                        format!("gain {db:+.1} dB"),
                        format!("out.gain {o} {}", number(db as f32)),
                    )
                }
                None => hint("gain <dB>"),
            },
            Some("delay") => match tokens.get(1).and_then(|t| num(t)) {
                Some(ms) => {
                    let ms = ms.clamp(0.0, max_delay_ms(state));
                    one(
                        format!("delay {ms:.1} ms"),
                        format!("out.delay {o} {}", number(ms as f32)),
                    )
                }
                None => hint("delay <ms>"),
            },
            Some("mute") => one("muted".into(), format!("out.mute {o} on")),
            Some("unmute") => one("unmuted".into(), format!("out.mute {o} off")),
            Some("on") => one("enabled".into(), format!("out.enable {o} on")),
            Some("off") => one("disabled".into(), format!("out.enable {o} off")),
            Some("xo") => self.quick_xover(state, channel, &tokens[1..]),
            Some(v @ ("limit" | "release" | "link")) => Quick {
                ghost: ghost(&lower, VERBS),
                ..limiter::quick(state, o, v, &tokens[1..])
            },
            // Bypass All and Clear All over the bank the tabs show; they
            // have no keys since `A` and `D` open tools. Bypassing the
            // crossovers opens the Console's critical dialog on Enter
            // (`quick_run`, `Components.swift:1619-1628`), and the hint
            // carries its warning before then.
            Some("bypass") => match self.list.bypass_all_command(state, true) {
                Some(_) if self.list.mode == FilterMode::Xo => hint(
                    "Bypass this output's crossovers? This sends full-range audio to this \
                     output with no crossover protection, which can damage unprotected drivers \
                     such as tweeters.",
                ),
                Some(c) => one("Bypass All".into(), c),
                None => hint("nothing to bypass"),
            },
            // Enter asks the Console's "Clear All Bands?" first.
            Some("clear") => match self.list.clear_all_command(state) {
                Some(_) => hint(match self.list.mode {
                    FilterMode::Xo => "Clear All Bands? · every crossover band off",
                    FilterMode::Peq => "Clear All Bands? · every band off",
                }),
                None => hint("nothing to clear"),
            },
            Some("name") => {
                let name = line
                    .trim_start()
                    .split_once(char::is_whitespace)
                    .map(|(_, rest)| rest.trim())
                    .unwrap_or("");
                if name.is_empty() {
                    hint("name <text> · renames this channel")
                } else {
                    one(
                        format!("rename to {name}"),
                        format!("ch.name {channel} {name}"),
                    )
                }
            }
            _ => Quick {
                fallthrough: true,
                hint: summary,
                ghost: ghost(&lower, VERBS),
                commands: Vec::new(),
            },
        }
    }

    /// A PEQ band edit, same shape as the input page's.
    fn quick_band(
        &self,
        state: &DeviceState,
        channel: usize,
        band: u8,
        args: &[&str],
    ) -> crate::shell::Quick {
        use super::quick::{ghost, number as num, verb};
        use crate::shell::Quick;
        use dspi_proto::FilterType;
        const TYPES: &[&str] = &[
            "peak",
            "lowshelf",
            "highshelf",
            "lowpass",
            "highpass",
            "notch",
            "ls",
            "hs",
            "lp",
            "hp",
            "off",
        ];
        let hint = |h: &str| Quick {
            fallthrough: false,
            hint: h.to_string(),
            ghost: args.first().and_then(|t| ghost(t, TYPES)),
            commands: Vec::new(),
        };
        let Some(ty) = args.first().and_then(|t| verb(t, TYPES)) else {
            return hint("peak · ls · hs · lp · hp · notch · off, then <freq> <gain> [q]");
        };
        let n = super::display_band(band);
        let write = |p: &dspi_proto::value::EqParamPacket, what: String| Quick {
            fallthrough: false,
            hint: format!("band {n}: {what}"),
            ghost: None,
            commands: super::band_command(state, channel, band, p),
        };
        if ty == "off" {
            return write(&super::cleared_band(channel as u8, band), "off".into());
        }
        let (filter_type, needs_gain, default_q) = match ty {
            "peak" => (FilterType::Peaking, true, 1.0),
            "lowshelf" | "ls" => (FilterType::LowShelf, true, 0.707),
            "highshelf" | "hs" => (FilterType::HighShelf, true, 0.707),
            "lowpass" | "lp" => (FilterType::LowPass, false, 0.707),
            "highpass" | "hp" => (FilterType::HighPass, false, 0.707),
            _ => (FilterType::Notch, false, 1.0),
        };
        let Some(freq) = args.get(1).and_then(|t| num(t)) else {
            return hint("frequency next: 1k · 80 · 2.5k");
        };
        let (gain, q) = if needs_gain {
            let Some(g) = args.get(2).and_then(|t| num(t)) else {
                return hint("gain next: -2 · +4.5");
            };
            (g, args.get(3).and_then(|t| num(t)).unwrap_or(default_q))
        } else {
            (0.0, args.get(2).and_then(|t| num(t)).unwrap_or(default_q))
        };
        let p = dspi_proto::value::EqParamPacket {
            filter_type,
            freq: freq.clamp(10.0, 20_000.0) as f32,
            q: q.clamp(0.1, 30.0) as f32,
            gain_db: gain.clamp(-30.0, 30.0) as f32,
            ..super::cleared_band(channel as u8, band)
        };
        let what = if needs_gain {
            format!("{ty} {freq:.0} Hz {gain:+.1} dB q {q}")
        } else {
            format!("{ty} {freq:.0} Hz q {q}")
        };
        write(&p, what)
    }

    /// `xo hp 80 [lr4]` writes the first crossover slot; `xo off` clears
    /// all four. Families: lr2 lr4 lr8 · bw1..bw8 · bes2..bes8.
    fn quick_xover(
        &self,
        state: &DeviceState,
        channel: usize,
        args: &[&str],
    ) -> crate::shell::Quick {
        use super::quick::number as num;
        use crate::shell::Quick;
        if !supports_crossover(state) {
            return Quick {
                fallthrough: false,
                hint: "this firmware has no crossover bank".into(),
                ghost: None,
                commands: Vec::new(),
            };
        }
        let hint = |h: &str| Quick {
            fallthrough: false,
            hint: h.to_string(),
            ghost: None,
            commands: Vec::new(),
        };
        match args.first().copied() {
            Some("off") => {
                let mut commands = Vec::new();
                for band in 20..24u8 {
                    commands.extend(super::band_command(
                        state,
                        channel,
                        band,
                        &super::cleared_band(channel as u8, band),
                    ));
                }
                Quick {
                    fallthrough: false,
                    hint: "crossover off".into(),
                    ghost: None,
                    commands,
                }
            }
            Some(side @ ("hp" | "lp")) => {
                let Some(freq) = args.get(1).and_then(|t| num(t)) else {
                    return hint("corner next: xo hp 80 · xo lp 80 lr4");
                };
                let family = args.get(2).copied().unwrap_or("lr4");
                let token = format!("{family}{side}");
                Quick {
                    fallthrough: false,
                    hint: format!(
                        "{} {freq:.0} Hz {}",
                        side.to_uppercase(),
                        family.to_uppercase()
                    ),
                    ghost: None,
                    commands: vec![format!(
                        "eq {} 20 {token} {} 0.707 0",
                        super::channel_token(state, channel),
                        number(freq.clamp(10.0, 20_000.0) as f32)
                    )],
                }
            }
            _ => hint("xo hp <freq> [lr2|lr4|lr8|bw2|bes4…] · xo lp <freq> · xo off"),
        }
    }
}

impl Screen for OutputPage {
    fn quick(&self, line: &str, state: &DeviceState) -> Option<crate::shell::Quick> {
        Some(self.quick_reply(line, state))
    }

    /// `bypass` on the XO tab and `clear` on either ask the Console's
    /// questions before they write, as its footer buttons do.
    fn quick_run(&mut self, line: &str, state: &DeviceState) -> Option<ScreenEvent> {
        let lower = line.trim().to_ascii_lowercase();
        let word = super::quick::verb(&lower, &["bypass", "clear"]).filter(|_| {
            // Only the bare word: anything longer is another grammar's.
            !lower.contains(char::is_whitespace)
        })?;
        let ev = match word {
            "bypass" if self.list.mode == FilterMode::Xo => self.list.bypass_all(state),
            "clear" => self.list.clear_all_confirm(state),
            _ => return None,
        };
        if matches!(ev, ScreenEvent::Dialog(_)) {
            self.list_dialog = true;
        }
        Some(ev)
    }

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
        let open = self.limiter.is_some();
        let y = self.draw_header(area, buf, theme, state, focused && !open);
        let used = y - area.y;
        if area.height > used {
            let rest = Rect::new(area.x, y, area.width, area.height - used);
            match self.limiter.as_mut() {
                Some(l) => l.draw(rest, buf, theme, state, focused),
                None => self
                    .list
                    .draw(rest, buf, theme, state, focused && self.header.is_none()),
            }
        }
    }

    fn handle(&mut self, key: KeyEvent, state: &DeviceState) -> ScreenEvent {
        if let Some(l) = self.limiter.as_mut() {
            return match l.handle(key, state) {
                limiter::Outcome::Event(ev) => ev,
                limiter::Outcome::Close => {
                    self.limiter = None;
                    ScreenEvent::Handled
                }
            };
        }
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
                    // Between MUTE and the limiter cell beside it.
                    Item::Mute => {
                        if limiter::available(state, self.output) {
                            self.column = usize::from(dir > 0.0);
                        }
                        ScreenEvent::Handled
                    }
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
                // Space switches the limiter and Enter opens its settings, as
                // the Console's click and right-click do.
                Item::Mute if self.on_limiter(state, item) => {
                    if key.code == KeyCode::Enter {
                        self.limiter = Some(LimiterSettings::new(self.output));
                        ScreenEvent::Handled
                    } else {
                        ScreenEvent::Command(limiter::toggle_command(state, self.output))
                    }
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
        if self.limiter.is_some() {
            return Vec::new();
        }
        self.list.actions(state, true)
    }

    fn keys(&self) -> &'static [KeyHelp] {
        if self.limiter.is_some() {
            limiter::KEYS
        } else {
            KEYS
        }
    }

    fn popup_result(&mut self, choice: Option<usize>, state: &DeviceState) -> ScreenEvent {
        if let Some(l) = self.limiter.as_mut().filter(|l| l.awaiting_menu()) {
            return l.popup_result(choice, state);
        }
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

    /// Bypass All and Clear All moved from `A` and `D` to the command bar,
    /// over whichever bank the tabs show. Bypassing crossovers keeps the
    /// Console's critical confirmation (audit D20), and Clear All its
    /// question, before anything is written.
    #[test]
    fn the_command_bar_bypasses_and_clears_the_bank_showing() {
        let (mut p, state) = page(0);
        // The fixture's OUT L has a crossover and no PEQ bands.
        assert!(p.quick_reply("bypass", &state).commands.is_empty());
        assert!(p.quick_reply("clear", &state).commands.is_empty());
        assert_eq!(
            p.quick_run("clear", &state),
            Some(ScreenEvent::Status("nothing to clear".into()))
        );
        p.list.mode = FilterMode::Xo;
        let q = p.quick_reply("bypass", &state);
        assert!(q.commands.is_empty(), "Enter asks first: {q:?}");
        assert!(
            q.hint.starts_with("Bypass this output's crossovers?"),
            "{q:?}"
        );
        assert!(q.hint.contains("can damage unprotected drivers"), "{q:?}");
        let Some(ScreenEvent::Dialog(d)) = p.quick_run("bypass", &state) else {
            panic!("the Console's dialog");
        };
        assert_eq!(d.title, "Bypass this output's crossovers?");
        assert!(d.body.contains("Continue only if you are sure."));
        assert!(d.critical && d.buttons[0].destructive);
        assert_eq!(
            p.dialog_result(DialogOutcome::Cancelled, &state),
            ScreenEvent::Handled,
            "Cancel writes nothing"
        );
        p.quick_run("bypass", &state);
        match p.dialog_result(DialogOutcome::Button(0), &state) {
            ScreenEvent::Command(c) => assert!(c.starts_with("eq.bypass out.1 "), "{c}"),
            other => panic!("{other:?}"),
        }
        let Some(ScreenEvent::Dialog(d)) = p.quick_run("clear", &state) else {
            panic!("Clear All asks");
        };
        assert_eq!(d.title, "Clear All Bands?");
        match p.dialog_result(DialogOutcome::Button(0), &state) {
            ScreenEvent::Command(c) => assert!(c.lines().all(|l| l.contains(" flat ")), "{c}"),
            other => panic!("{other:?}"),
        }
        // A longer line is another grammar's, and runs as commands.
        assert_eq!(p.quick_run("clear 3", &state), None);
    }

    /// Through the shell: `;bypass` and Enter on the XO tab put the critical
    /// dialog on screen and send nothing.
    #[test]
    fn semicolon_bypass_on_the_xo_tab_raises_the_dialog() {
        let (mut p, state) = page(0);
        p.list.mode = FilterMode::Xo;
        let theme = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let mut s =
            crate::shell::Shell::new(crate::shell::fixture::rp2350(&theme), theme, Box::new(p));
        let mut out = s.handle(key(KeyCode::Char(';')), &state);
        for c in "bypass".chars() {
            out.extend(s.handle(key(KeyCode::Char(c)), &state));
        }
        out.extend(s.handle(key(KeyCode::Enter), &state));
        assert!(out.is_empty(), "nothing sent: {out:?}");
        let d = s.dialog.as_ref().expect("the dialog is up");
        assert_eq!(d.title, "Bypass this output's crossovers?");
    }

    /// Routing names are the sidebar's channel names, not 7.1 labels
    /// (`Components.swift:950`, `inputChannelName`), cut to fit.
    #[test]
    fn routing_rows_carry_the_sidebar_names() {
        let (mut p, state) = page(0);
        let mut b = state.bulk.as_bytes().to_vec();
        let names = dspi_proto::generated::SECTIONS
            .iter()
            .find(|(n, _, _)| *n == "channel_names")
            .map(|(_, o, _)| *o)
            .expect("section");
        b[names..names + 64].fill(0);
        b[names..names + 3].copy_from_slice(b"Mac");
        b[names + 32..names + 32 + 16].copy_from_slice(b"Turntable Righty");
        let mut state = state;
        state.replace_bulk(dspi_proto::wire::BulkPacket::decode(b).expect("packet"));
        let f = draw(&mut p, &state, 94, 21);
        let lines: Vec<&str> = f.lines().collect();
        assert!(lines[0].contains("Mac") && !lines[0].contains("FL"), "{f}");
        assert!(lines[1].contains("Turntable…"), "{f}");
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

    // ------------------------------------------------------- the limiter

    fn limited() -> DeviceState {
        limiter::demo_state(fixture::state())
    }

    /// The whole shell with this page in it, so a golden frame is what a
    /// person sees.
    fn shell_frame(p: OutputPage, state: &DeviceState, w: u16, h: u16) -> String {
        let theme = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let mut model = fixture::rp2350(&theme);
        fixture::select(
            &mut model,
            state,
            &theme,
            crate::shell::Selection::Output(0),
        );
        let mut shell = crate::shell::Shell::new(model, theme, Box::new(p));
        shell.focus = crate::shell::Focus::Screen;
        crate::render_frame(w, h, |area, buf| shell.draw(area, buf, state))
    }

    fn mute_line(f: &str) -> String {
        f.lines()
            .find(|l| l.contains("MUTE"))
            .unwrap_or_else(|| panic!("no MUTE row:\n{f}"))
            .to_string()
    }

    #[test]
    fn the_limiter_cell_needs_the_feature() {
        let (mut p, state) = page(0);
        let f = draw(&mut p, &state, 94, 21);
        assert!(!f.contains("LIMITER"), "{f}");
        // Right on MUTE has nowhere to go without it.
        p.header = Some(4);
        p.handle(key(KeyCode::Right), &state);
        assert_eq!(p.column, 0);
    }

    #[test]
    fn golden_frames_with_the_limiter_off_on_and_reducing() {
        let mut state = limited();
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            // On, at the default ceiling, nothing to take off.
            let f = shell_frame(OutputPage::new(0, shared(), &state), &state, w, h);
            assert_eq!(f.lines().count(), h as usize);
            let m = mute_line(&f);
            assert!(
                m.contains("LIMITER ● -1.0 dBFS") && m.contains("GR 0.0 dB"),
                "{w}x{h}:\n{f}"
            );
            assert!(f.contains("TYPE"), "the filter list is still there:\n{f}");
            // Off: grey, and says so.
            let off = shell_frame(OutputPage::new(2, shared(), &state), &state, w, h);
            assert!(mute_line(&off).contains("LIMITER ○ Off"), "{w}x{h}:\n{off}");
        }
        // Reducing: the reading is next to the indicator, in the warning
        // colour, as the Console's icon turns orange.
        state.limiter_meter = Some(dspi_proto::packets::LimiterMeter {
            centi_db: vec![320, 0, 0, 0, 0, 0, 0, 0, 0],
        });
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            let f = shell_frame(OutputPage::new(0, shared(), &state), &state, w, h);
            assert!(mute_line(&f).contains("GR 3.2 dB"), "{w}x{h}:\n{f}");
        }
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let mut p = OutputPage::new(0, shared(), &state);
        let mut buf = Buffer::empty(Rect::new(0, 0, 94, 21));
        p.draw(Rect::new(0, 0, 94, 21), &mut buf, &t, &state, true);
        let y = 4; // the MUTE row, under two routes, gain and delay
        let dot = (0..94u16)
            .find(|x| buf[(*x, y)].symbol() == "●" && *x > 17)
            .expect("the indicator");
        assert_eq!(buf[(dot, y)].fg, t.warning);
        state.limiter_meter = None;
        let mut buf = Buffer::empty(Rect::new(0, 0, 94, 21));
        p.draw(Rect::new(0, 0, 94, 21), &mut buf, &t, &state, true);
        assert_eq!(buf[(dot, y)].fg, t.accent, "on and idle is the accent");
        let mut off = OutputPage::new(2, shared(), &state);
        let mut buf = Buffer::empty(Rect::new(0, 0, 94, 21));
        off.draw(Rect::new(0, 0, 94, 21), &mut buf, &t, &state, true);
        assert_eq!(buf[(dot, y)].fg, t.dim, "off is grey");
    }

    #[test]
    fn golden_frames_of_the_settings() {
        let state = limited();
        let name = |o| channel_name(&state, output_channel(&state, o));
        let title = format!("Output Limiter · {}", name(0));
        let linked = format!("Linked with {}.", name(1));
        // `downs` walks the cursor down the settings first, which is how the
        // lower rows come into view on a short terminal.
        let open = |downs: usize, w: u16, h: u16| {
            let mut p = OutputPage::new(0, shared(), &state);
            p.header = Some(4);
            p.column = 1;
            p.handle(key(KeyCode::Enter), &state);
            assert!(p.limiter.is_some());
            for _ in 0..downs {
                p.handle(key(KeyCode::Down), &state);
            }
            let f = shell_frame(p, &state, w, h);
            assert_eq!(f.lines().count(), h as usize);
            assert!(!f.contains("TYPE"), "the list gives way:\n{f}");
            assert!(f.contains("MUTE"), "the page header stays:\n{f}");
            f
        };
        let everything = [
            title.as_str(),
            "● On",
            "Threshold",
            "-30 dBFS",
            "Release",
            "10 ms",
            "Link group",
            linked.as_str(),
            "Copy to all outputs",
            "All outputs ▾",
            "32 samples of latency",
            "Esc Close",
        ];
        let f = open(0, 120, 40);
        for want in everything {
            assert!(f.contains(want), "{want:?} at 120x40:\n{f}");
        }
        // At 80x24 the column scrolls under the page header.
        let top = open(0, 80, 24);
        for want in [title.as_str(), "● On", "Threshold", "-30 dBFS"] {
            assert!(top.contains(want), "{want:?} at 80x24:\n{top}");
        }
        let bottom = open(4, 80, 24);
        for want in [
            "Link group",
            linked.as_str(),
            "Copy to all outputs",
            "All outputs ▾",
        ] {
            assert!(bottom.contains(want), "{want:?} at 80x24:\n{bottom}");
        }
    }

    #[test]
    fn the_limiter_cell_toggles_on_space_and_opens_on_enter() {
        let state = limited();
        let mut p = OutputPage::new(2, shared(), &state);
        p.header = Some(4);
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &state),
            ScreenEvent::Command("out.mute 2 on".into()),
            "MUTE first"
        );
        p.handle(key(KeyCode::Right), &state);
        assert_eq!(p.column, 1);
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &state),
            ScreenEvent::Command("limit.on 2 on".into())
        );
        assert_eq!(p.keys(), KEYS);
        p.handle(key(KeyCode::Enter), &state);
        assert!(p.limiter.is_some());
        assert_eq!(p.keys(), limiter::KEYS);
        assert!(p.actions(&state).is_empty(), "no filter-list footer");
        // The menu's answer goes to the settings, not the filter list.
        for _ in 0..4 {
            p.handle(key(KeyCode::Down), &state);
        }
        p.handle(key(KeyCode::Right), &state);
        // Off: the menu stays shut.
        assert_eq!(p.handle(key(KeyCode::Enter), &state), ScreenEvent::Handled);
        p.handle(key(KeyCode::Esc), &state);
        assert!(p.limiter.is_none());
        assert_eq!(p.keys(), KEYS);
        let mut on = OutputPage::new(0, shared(), &state);
        on.limiter = Some(LimiterSettings::new(0));
        for _ in 0..4 {
            on.handle(key(KeyCode::Down), &state);
        }
        on.handle(key(KeyCode::Right), &state);
        assert!(matches!(
            on.handle(key(KeyCode::Enter), &state),
            ScreenEvent::Popup(_)
        ));
        assert_eq!(
            on.popup_result(Some(0), &state),
            ScreenEvent::Command(limiter::link_all_pairs(&state))
        );
    }

    #[test]
    fn the_command_bar_has_the_limiters_words() {
        let state = limited();
        let p = OutputPage::new(3, shared(), &state);
        let q = |line: &str| p.quick(line, &state).unwrap();
        assert_eq!(q("limit on").commands, vec!["limit.on 3 on"]);
        assert_eq!(q("limit off").commands, vec!["limit.on 3 off"]);
        assert_eq!(q("limit -3").commands, vec!["limit.threshold 3 -3"]);
        assert_eq!(q("release 200").commands, vec!["limit.release 3 200"]);
        assert_eq!(q("link 2").commands, vec!["limit.link 3 2"]);
        assert_eq!(q("link off").commands, vec!["limit.link 3 0"]);
        assert_eq!(q("rel").ghost.as_deref(), Some("ease"));
        assert_eq!(q("lim").ghost.as_deref(), Some("it"));
        assert!(q("").hint.contains("limit on"), "{}", q("").hint);
        for partial in ["limit", "release", "link", "link 9"] {
            let r = q(partial);
            assert!(r.commands.is_empty(), "{partial:?} ran {:?}", r.commands);
            assert!(!r.hint.is_empty());
        }
        // Without the limiter the summary does not offer it.
        let (plain, st) = page(0);
        assert!(!plain.quick("", &st).unwrap().hint.contains("limit"));
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
