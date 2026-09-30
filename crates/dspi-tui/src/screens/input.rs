//! The input page: the Console's `InputChannelHeader` over its filter list.
//!
//! Three controls on one row: the link pill for this input's adjacent pair,
//! the preamp, and Clear PEQ. Linking is application state, as it is in the
//! Console: the device knows nothing about it, and all it does is mirror
//! future edits onto the partner. That is why linking a pair that already
//! differs has to ask which side to keep, since mirroring cannot merge.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use dspi_proto::value::EqParamPacket;
use dspi_session::DeviceState;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

use super::filters::FilterList;
use super::{Shared, cleared_band, number};
use crate::shell::{Screen, ScreenEvent};
use crate::theme::{Glyphs, Theme};
use crate::widgets::{
    Button, Dialog, DialogOutcome, KeyHelp, NumberEdit, ParamRow, StatusPill, StatusTone,
};

/// The three header controls, in the Console's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Link,
    Preamp,
    Clear,
}

/// Which dialog or popup this page is waiting on.
#[derive(Debug, Clone, PartialEq)]
enum Pending {
    /// The link mismatch prompt: keep this input, or keep its partner.
    Mismatch {
        partner: usize,
    },
    ClearPeq,
    /// The filter list raised it; hand the outcome straight back.
    List,
}

pub struct InputPage {
    pub input: usize,
    /// The channel's name at the time the page was made, for the title row.
    /// The shell asks for the title before it draws, so this is read once
    /// rather than every frame; a rename rebuilds the page.
    name: String,
    shared: Shared,
    pub list: FilterList,
    /// `None` while the filter list has the keyboard.
    header: Option<usize>,
    preamp_edit: Option<NumberEdit>,
    pending: Option<Pending>,
}

const KEYS: &[KeyHelp] = &[
    KeyHelp::new("↑ ↓", "Band, or the header"),
    KeyHelp::new("← →", "Field"),
    KeyHelp::new("Enter", "Edit"),
    KeyHelp::new("Space", "Bypass"),
    KeyHelp::new("1-9,0", "Jump to a band"),
    KeyHelp::new("a", "Enable All"),
    KeyHelp::new("Backspace", "Reset the preamp"),
];

impl InputPage {
    pub fn new(input: usize, shared: Shared, state: &DeviceState) -> Self {
        let mut list = FilterList::new(input).linkwitz(false);
        list.mirror = shared
            .borrow()
            .linked_partner(input, state.caps.num_inputs as usize);
        Self {
            input,
            name: super::channel_name(state, input),
            shared,
            list,
            header: Some(1),
            preamp_edit: None,
            pending: None,
        }
    }

    fn partner(&self) -> usize {
        self.input ^ 1
    }

    /// A pair is only offered while both of its inputs are live.
    fn pair_available(&self, state: &DeviceState) -> bool {
        self.input.max(self.partner()) < state.caps.num_inputs as usize
    }

    fn linked(&self, state: &DeviceState) -> bool {
        self.shared
            .borrow()
            .linked_partner(self.input, state.caps.num_inputs as usize)
            .is_some()
    }

    /// `1/2`, `3/4`, keyed to the sidebar's IN1..IN8 numbering.
    fn pair_label(&self) -> String {
        format!(
            "{}/{}",
            self.input.min(self.partner()) + 1,
            self.input.max(self.partner()) + 1
        )
    }

    fn items(&self, state: &DeviceState) -> Vec<Item> {
        let mut v = Vec::new();
        if self.pair_available(state) {
            v.push(Item::Link);
        }
        v.push(Item::Preamp);
        v.push(Item::Clear);
        v
    }

    fn item(&self, state: &DeviceState) -> Option<Item> {
        let items = self.items(state);
        self.header
            .and_then(|i| items.get(i.min(items.len().saturating_sub(1))).copied())
    }

    fn preamp_row<'a>(&self, state: &DeviceState, theme: &'a Theme) -> ParamRow<'a> {
        ParamRow::new(
            "Preamp",
            state.preamp_db(self.input) as f64,
            -60.0,
            10.0,
            "dB",
            theme,
        )
        .step(0.5)
        .decimals(1)
        .compact(true)
    }

    /// One arrow press on the preamp: the Console's slider runs -60 to +10 dB,
    /// and a half-decibel is the useful grain for a trim.
    fn nudged_preamp(&self, state: &DeviceState, dir: f64, coarse: bool) -> f64 {
        let step = if coarse { 5.0 } else { 0.5 };
        (state.preamp_db(self.input) as f64 + dir * step).clamp(-60.0, 10.0)
    }

    /// The page grammar behind `;` (DESIGN 13): the preamp, a band in one
    /// line (`3 peak 1k -2 1.4`), the delay, `bypass`, `clear` and `name`. Edits
    /// mirror onto a linked partner like every other edit on this page.
    fn quick_reply(&self, line: &str, state: &DeviceState) -> crate::shell::Quick {
        use super::quick::{ghost, number as num, verb};
        use crate::shell::Quick;
        const VERBS: &[&str] = &["pre", "delay", "bypass", "clear", "name"];
        const SUMMARY: &str =
            "pre -5.3 · 3 peak 1k -2 [q] · 3 off · delay 2.5 · bypass · clear · name Front L";
        let lower = line.to_ascii_lowercase();
        let tokens: Vec<&str> = lower.split_whitespace().collect();
        let hint = |h: &str| Quick {
            fallthrough: false,
            hint: h.to_string(),
            ghost: ghost(&lower, VERBS),
            commands: Vec::new(),
        };
        let Some(&first) = tokens.first() else {
            return hint(SUMMARY);
        };
        // A band number starts a band edit: `3 peak 1k -2 [1.4]`, `3 off`.
        if let Ok(band) = first.parse::<usize>() {
            let max = state.caps.max_bands as usize;
            if band == 0 || band > max {
                return hint(&format!("bands are 1 to {max}"));
            }
            return self.quick_band(state, band as u8 - 1, &tokens[1..]);
        }
        match verb(first, VERBS) {
            Some("pre") => match tokens.get(1).and_then(|t| num(t)) {
                Some(db) => {
                    let db = db.clamp(-60.0, 10.0);
                    Quick {
                        fallthrough: false,
                        hint: format!("preamp {db:+.1} dB{}", self.mirror_note()),
                        ghost: None,
                        commands: vec![self.preamp_command(db)],
                    }
                }
                None => hint("pre <dB> · the input preamp"),
            },
            Some("delay") => match tokens.get(1).and_then(|t| num(t)) {
                Some(ms) => {
                    let ms = ms.clamp(0.0, super::max_delay_ms(state));
                    let mut commands = vec![format!(
                        "ch.delay {} {}",
                        super::channel_token(state, self.input),
                        number(ms as f32)
                    )];
                    if let Some(m) = self.list.mirror {
                        commands.push(format!(
                            "ch.delay {} {}",
                            super::channel_token(state, m),
                            number(ms as f32)
                        ));
                    }
                    Quick {
                        fallthrough: false,
                        hint: format!("delay {ms:.1} ms{}", self.mirror_note()),
                        ghost: None,
                        commands,
                    }
                }
                None => hint("delay <ms>"),
            },
            // Bypass All, which has no key since `A` opens the Spectrum
            // Analyser; Enable All stays on `a`.
            Some("bypass") => match self.list.bypass_all_command(state, true) {
                Some(c) => Quick {
                    fallthrough: false,
                    hint: format!("Bypass All{}", self.mirror_note()),
                    ghost: None,
                    commands: vec![c],
                },
                None => hint("nothing to bypass"),
            },
            // Enter asks the Console's "Clear All Bands?" first (`quick_run`,
            // `Components.swift:1643-1651`), as the header's Clear does.
            Some("clear") => hint(&format!(
                "Clear All Bands? · every band off{}",
                self.mirror_note()
            )),
            Some("name") => {
                let name = line
                    .trim_start()
                    .split_once(char::is_whitespace)
                    .map(|(_, rest)| rest.trim())
                    .unwrap_or("");
                if name.is_empty() {
                    hint("name <text> · renames this channel")
                } else {
                    Quick {
                        fallthrough: false,
                        hint: format!("rename to {name}"),
                        ghost: None,
                        commands: vec![format!("ch.name {} {name}", self.input)],
                    }
                }
            }
            _ => Quick {
                fallthrough: true,
                hint: SUMMARY.to_string(),
                ghost: ghost(&lower, VERBS),
                commands: Vec::new(),
            },
        }
    }

    /// `peak 1k -2 [q]` or `off`, on band `band` (0-based), mirrored.
    fn quick_band(&self, state: &DeviceState, band: u8, args: &[&str]) -> crate::shell::Quick {
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
            "allpass",
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
            return hint(
                "peak · ls · hs · lp · hp · notch · allpass · off, then <freq> <gain> [q]",
            );
        };
        let n = super::display_band(band);
        let write = |p: EqParamPacket, what: String| {
            let mut commands = super::band_command(state, self.input, band, &p);
            if let Some(m) = self.list.mirror {
                commands.extend(super::band_command(state, m, band, &p));
            }
            Quick {
                fallthrough: false,
                hint: format!("band {n}: {what}{}", self.mirror_note()),
                ghost: None,
                commands,
            }
        };
        if ty == "off" {
            return write(cleared_band(self.input as u8, band), "off".into());
        }
        let (filter_type, needs_gain, default_q) = match ty {
            "peak" => (FilterType::Peaking, true, 1.0),
            "lowshelf" | "ls" => (FilterType::LowShelf, true, 0.707),
            "highshelf" | "hs" => (FilterType::HighShelf, true, 0.707),
            "lowpass" | "lp" => (FilterType::LowPass, false, 0.707),
            "highpass" | "hp" => (FilterType::HighPass, false, 0.707),
            "notch" => (FilterType::Notch, false, 1.0),
            _ => (FilterType::AllPass, false, 0.707),
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
        let p = EqParamPacket {
            filter_type,
            freq: freq.clamp(10.0, 20_000.0) as f32,
            q: q.clamp(0.1, 30.0) as f32,
            gain_db: gain.clamp(-30.0, 30.0) as f32,
            ..cleared_band(self.input as u8, band)
        };
        let what = if needs_gain {
            format!("{ty} {freq:.0} Hz {gain:+.1} dB q {q}")
        } else {
            format!("{ty} {freq:.0} Hz q {q}")
        };
        write(p, what)
    }

    /// ` · mirrors to INn` when the pair is linked.
    fn mirror_note(&self) -> String {
        match self.list.mirror {
            Some(m) => format!(" · mirrors to IN{}", m + 1),
            None => String::new(),
        }
    }

    fn preamp_command(&self, db: f64) -> String {
        let mut lines = vec![format!("pre {} {}", self.input, number(db as f32))];
        if let Some(m) = self.list.mirror {
            lines.push(format!("pre {m} {}", number(db as f32)));
        }
        lines.join("\n")
    }

    /// Every band on this input, and on its partner when linked, set to Off.
    /// The Console's Clear All question (`Components.swift:1643-1651`).
    fn ask_clear(&mut self) -> ScreenEvent {
        self.pending = Some(Pending::ClearPeq);
        ScreenEvent::Dialog(Dialog::confirm(
            "Clear All Bands?",
            "Every band in this list will be reset to its default (flat) state. \
             This cannot be undone.",
            vec![Button::destructive("Clear All"), Button::new("Cancel")],
        ))
    }

    fn clear_peq(&self, state: &DeviceState) -> ScreenEvent {
        let mut lines = Vec::new();
        for target in std::iter::once(self.input).chain(self.list.mirror) {
            for band in 0..state.caps.max_bands {
                let p = cleared_band(target as u8, band);
                lines.extend(super::band_command(state, target, band, &p));
            }
        }
        ScreenEvent::Command(lines.join("\n"))
    }

    /// What differs between the two halves of a pair: filters, input trims, or
    /// both. The Console's `inputPairMismatch`.
    fn mismatch(&self, state: &DeviceState) -> (bool, bool) {
        let same = |a: &EqParamPacket, b: &EqParamPacket| {
            a.filter_type == b.filter_type
                && a.freq == b.freq
                && a.q == b.q
                && a.gain_db == b.gain_db
                && a.bypass == b.bypass
        };
        let mine = state.bands(self.input as u8);
        let theirs = state.bands(self.partner() as u8);
        let bands =
            mine.len() != theirs.len() || mine.iter().zip(&theirs).any(|(a, b)| !same(a, b));
        let preamp = state.preamp_db(self.input) != state.preamp_db(self.partner());
        (bands, preamp)
    }

    /// Copy one half of a pair onto the other, then link them.
    fn sync_pair(&self, state: &DeviceState, keep: usize) -> ScreenEvent {
        let onto = if keep == self.input {
            self.partner()
        } else {
            self.input
        };
        let mut lines = Vec::new();
        for (band, p) in state.bands(keep as u8).iter().enumerate() {
            lines.extend(super::band_command(state, onto, band as u8, p));
        }
        lines.push(format!("pre {onto} {}", number(state.preamp_db(keep))));
        ScreenEvent::Command(lines.join("\n"))
    }

    fn toggle_link(&mut self, state: &DeviceState) -> ScreenEvent {
        let pair = self.input / 2;
        if self.linked(state) {
            self.shared.borrow_mut().set_linked(pair, false);
            self.list.mirror = None;
            return ScreenEvent::Status(format!("Link {} off", self.pair_label()));
        }
        let (bands, preamp) = self.mismatch(state);
        if bands || preamp {
            let differences = match (bands, preamp) {
                (true, true) => "different filters and input trims",
                (true, false) => "different filters",
                _ => "different input trims",
            };
            self.pending = Some(Pending::Mismatch {
                partner: self.partner(),
            });
            return ScreenEvent::Dialog(Dialog::confirm(
                format!(
                    "Inputs {} and {} don't match",
                    self.input + 1,
                    self.partner() + 1
                ),
                format!(
                    "These inputs have {differences}. Linking mirrors future edits across \
                     both channels, but it cannot merge settings that already differ.\n\n\
                     Choose which channel's filters and trim to copy onto the other. This \
                     overwrites the other channel and cannot be undone."
                ),
                vec![
                    Button::new(format!("Keep IN{}", self.input + 1)),
                    Button::new(format!("Keep IN{}", self.partner() + 1)),
                    Button::new("Cancel"),
                ],
            ));
        }
        self.link_now(state);
        ScreenEvent::Status(format!("Link {} on", self.pair_label()))
    }

    fn link_now(&mut self, state: &DeviceState) {
        self.shared.borrow_mut().set_linked(self.input / 2, true);
        self.list.mirror = self
            .shared
            .borrow()
            .linked_partner(self.input, state.caps.num_inputs as usize);
    }

    fn draw_header(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        state: &DeviceState,
        focused: bool,
    ) {
        let items = self.items(state);
        let here = |i: Item| focused && self.item(state) == Some(i);
        let mut x = area.x;

        if items.contains(&Item::Link) {
            let text = format!("Link {}", self.pair_label());
            let w = text.len() as u16 + 2;
            if self.linked(state) {
                StatusPill::new(&text, StatusTone::Ok, theme)
                    .render(Rect::new(x, area.y, w, 1), buf);
            } else {
                buf.set_string(x + 1, area.y, &text, theme.label());
            }
            if here(Item::Link) {
                buf.set_string(x, area.y, "▸", theme.focused());
            }
            x += w + 2;
        }

        let clear = if self.linked(state) {
            format!("Clear {} PEQ", self.pair_label())
        } else {
            "Clear PEQ".to_string()
        };
        let clear_w = clear.len() as u16 + 2;
        let preamp_w = (area.x + area.width)
            .saturating_sub(x + clear_w + 2)
            .min(area.width);
        if preamp_w >= 12 {
            self.preamp_row(state, theme)
                .focused(here(Item::Preamp))
                .edit(self.preamp_edit.as_ref())
                .render(Rect::new(x, area.y, preamp_w, 1), buf);
        }

        let cx = area.x + area.width - clear_w;
        buf.set_string(
            cx,
            area.y,
            format!(" {clear} "),
            if here(Item::Clear) {
                theme.pill(theme.accent)
            } else {
                theme.value()
            },
        );
    }
}

impl Screen for InputPage {
    fn quick(&self, line: &str, state: &DeviceState) -> Option<crate::shell::Quick> {
        Some(self.quick_reply(line, state))
    }

    /// `clear` asks the Console's question before it writes, as the header's
    /// Clear does.
    fn quick_run(&mut self, line: &str, _state: &DeviceState) -> Option<ScreenEvent> {
        let lower = line.trim().to_ascii_lowercase();
        (!lower.contains(char::is_whitespace)
            && super::quick::verb(&lower, &["pre", "delay", "bypass", "clear", "name"])
                == Some("clear"))
        .then(|| self.ask_clear())
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
        self.draw_header(area, buf, theme, state, focused);
        let rule = if theme.glyphs == Glyphs::Ascii {
            "-"
        } else {
            "─"
        };
        buf.set_string(
            area.x,
            area.y + 1,
            rule.repeat(area.width as usize),
            theme.chrome_style(),
        );
        if area.height > 2 {
            let list = Rect::new(area.x, area.y + 2, area.width, area.height - 2);
            self.list
                .draw(list, buf, theme, state, focused && self.header.is_none());
        }
    }

    fn handle(&mut self, key: KeyEvent, state: &DeviceState) -> ScreenEvent {
        // Keep the mirror in step with the shared link state, which the
        // sidebar's paste and other pages can change.
        self.list.mirror = self
            .shared
            .borrow()
            .linked_partner(self.input, state.caps.num_inputs as usize);

        let Some(index) = self.header else {
            // The list has the keyboard; Up off its first band comes back here.
            let ev = self.list.handle(key, state);
            if ev == ScreenEvent::Unhandled && key.code == KeyCode::Up {
                self.header = Some(self.items(state).len() - 1);
                return ScreenEvent::Handled;
            }
            if matches!(ev, ScreenEvent::Dialog(_) | ScreenEvent::Popup(_)) {
                self.pending = Some(Pending::List);
            }
            return ev;
        };

        let items = self.items(state);
        let item = items[index.min(items.len() - 1)];

        if let Some(edit) = self.preamp_edit.as_mut() {
            let coarse = key.modifiers.contains(KeyModifiers::SHIFT);
            if matches!(key.code, KeyCode::Left | KeyCode::Right) {
                let dir = if key.code == KeyCode::Left { -1.0 } else { 1.0 };
                let v = self.nudged_preamp(state, dir, coarse);
                self.preamp_edit = Some(NumberEdit {
                    text: format!("{v:.1}"),
                    dirty: false,
                });
                return ScreenEvent::Command(self.preamp_command(v));
            }
            return match edit.handle(key) {
                Some(crate::widgets::Action::Committed(v)) => {
                    self.preamp_edit = None;
                    ScreenEvent::Command(self.preamp_command(v.clamp(-60.0, 10.0)))
                }
                Some(crate::widgets::Action::Closed) => {
                    self.preamp_edit = None;
                    ScreenEvent::Handled
                }
                _ => ScreenEvent::Handled,
            };
        }

        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                self.header = None;
                self.list.band = 0;
                self.list.field = 0;
                ScreenEvent::Handled
            }
            KeyCode::Up | KeyCode::Char('k') => ScreenEvent::Handled,
            KeyCode::Left => {
                if item == Item::Preamp {
                    let coarse = key.modifiers.contains(KeyModifiers::SHIFT);
                    let v = self.nudged_preamp(state, -1.0, coarse);
                    return ScreenEvent::Command(self.preamp_command(v));
                }
                self.header = Some(index.saturating_sub(1));
                ScreenEvent::Handled
            }
            KeyCode::Right => {
                if item == Item::Preamp {
                    let coarse = key.modifiers.contains(KeyModifiers::SHIFT);
                    let v = self.nudged_preamp(state, 1.0, coarse);
                    return ScreenEvent::Command(self.preamp_command(v));
                }
                self.header = Some((index + 1).min(items.len() - 1));
                ScreenEvent::Handled
            }
            KeyCode::Enter | KeyCode::Char(' ') => match item {
                Item::Link => self.toggle_link(state),
                Item::Preamp => {
                    self.preamp_edit = Some(NumberEdit {
                        text: format!("{:.1}", state.preamp_db(self.input)),
                        dirty: false,
                    });
                    ScreenEvent::Handled
                }
                Item::Clear => self.ask_clear(),
            },
            KeyCode::Backspace if item == Item::Preamp => {
                ScreenEvent::Command(self.preamp_command(0.0))
            }
            // A digit jumps to a band from anywhere on the page, which is what
            // makes it worth having.
            KeyCode::Char('0'..='9') => {
                self.header = None;
                self.list.handle(key, state)
            }
            // The list's own actions work from the header too, since they act
            // on the whole bank rather than on the cursor.
            _ => {
                let ev = self.list.handle(key, state);
                if matches!(ev, ScreenEvent::Dialog(_) | ScreenEvent::Popup(_)) {
                    self.pending = Some(Pending::List);
                }
                ev
            }
        }
    }

    fn actions(&self, state: &DeviceState) -> Vec<(String, bool)> {
        self.list.actions(state, false)
    }

    fn keys(&self) -> &'static [KeyHelp] {
        KEYS
    }

    fn popup_result(&mut self, choice: Option<usize>, state: &DeviceState) -> ScreenEvent {
        self.pending = None;
        self.list.popup_result(choice, state)
    }

    fn dialog_result(&mut self, outcome: DialogOutcome, state: &DeviceState) -> ScreenEvent {
        match self.pending.take() {
            Some(Pending::List) => self.list.dialog_result(outcome, state),
            Some(Pending::ClearPeq) => {
                if outcome == DialogOutcome::Button(0) {
                    self.clear_peq(state)
                } else {
                    ScreenEvent::Handled
                }
            }
            Some(Pending::Mismatch { partner }) => match outcome {
                DialogOutcome::Button(0) => {
                    let ev = self.sync_pair(state, self.input);
                    self.link_now(state);
                    ev
                }
                DialogOutcome::Button(1) => {
                    let ev = self.sync_pair(state, partner);
                    self.link_now(state);
                    ev
                }
                _ => ScreenEvent::Handled,
            },
            None => ScreenEvent::Handled,
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

    fn page() -> (InputPage, DeviceState) {
        let state = fixture::state();
        let page = InputPage::new(0, shared(), &state);
        (page, state)
    }

    fn draw(p: &mut InputPage, state: &DeviceState, w: u16, h: u16) -> String {
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
    fn the_header_carries_the_link_pill_the_preamp_and_clear_peq() {
        let (mut p, state) = page();
        let f = draw(&mut p, &state, 94, 21);
        let head = f.lines().next().unwrap();
        assert!(head.contains("Link 1/2"), "{head}");
        assert!(head.contains("Preamp") && head.contains("0.0 dB"), "{head}");
        assert!(head.contains("Clear PEQ"), "{head}");
        // And the filter list is beneath it.
        assert!(
            f.contains("TYPE") && f.contains("Low Shelf 12 dB/oct"),
            "{f}"
        );
        // The Linkwitz Transform is hidden on inputs.
        assert!(!p.list.include_linkwitz);
    }

    #[test]
    fn the_minimum_frame_fits() {
        let (mut p, state) = page();
        let f = draw(&mut p, &state, 56, 10);
        assert!(f.contains("Link 1/2"), "{f}");
        assert!(f.contains("Clear PEQ"), "{f}");
        assert_eq!(f.lines().count(), 10);
    }

    #[test]
    fn the_link_pill_and_clear_label_change_when_the_pair_is_linked() {
        let (mut p, state) = page();
        p.link_now(&state);
        let f = draw(&mut p, &state, 94, 21);
        assert!(f.contains("Clear 1/2 PEQ"), "{f}");
        assert_eq!(p.list.mirror, Some(1));
    }

    #[test]
    fn the_link_pill_is_hidden_when_the_partner_is_not_live() {
        let mut state = fixture::state();
        state.caps.num_inputs = 1;
        let mut p = InputPage::new(0, shared(), &state);
        let f = draw(&mut p, &state, 94, 21);
        assert!(!f.contains("Link"), "{f}");
    }

    #[test]
    fn the_preamp_nudges_by_half_a_decibel_and_backspace_resets_it() {
        let (mut p, state) = page();
        p.header = Some(1);
        assert_eq!(
            p.handle(key(KeyCode::Right), &state),
            ScreenEvent::Command("pre 0 0.5".into())
        );
        assert_eq!(
            p.handle(key(KeyCode::Left), &state),
            ScreenEvent::Command("pre 0 -0.5".into())
        );
        assert_eq!(
            p.handle(key(KeyCode::Backspace), &state),
            ScreenEvent::Command("pre 0 0".into())
        );
    }

    #[test]
    fn a_linked_preamp_mirrors_onto_the_partner() {
        let (mut p, state) = page();
        p.link_now(&state);
        p.header = Some(1);
        assert_eq!(
            p.handle(key(KeyCode::Right), &state),
            ScreenEvent::Command("pre 0 0.5\npre 1 0.5".into())
        );
    }

    /// The fixture tunes IN1 and IN2 identically; break one band so the pair
    /// no longer matches.
    fn mismatched() -> DeviceState {
        let mut state = fixture::state();
        let (_, eq, _) = dspi_proto::generated::SECTIONS
            .iter()
            .find(|(n, _, _)| *n == "eq")
            .copied()
            .unwrap();
        state.bulk.patch(eq + 12 * 16, &[0]);
        state
    }

    #[test]
    fn linking_a_mismatched_pair_asks_which_side_to_keep() {
        let state = mismatched();
        let mut p = InputPage::new(0, shared(), &state);
        p.header = Some(0);
        match p.handle(key(KeyCode::Enter), &state) {
            ScreenEvent::Dialog(d) => {
                assert_eq!(d.title, "Inputs 1 and 2 don't match");
                assert!(d.body.contains("different filters"), "{}", d.body);
                assert_eq!(d.buttons[0].label, "Keep IN1");
                assert_eq!(d.buttons[1].label, "Keep IN2");
                assert_eq!(d.buttons[2].label, "Cancel");
            }
            other => panic!("{other:?}"),
        }
        // Keeping IN1 copies its bank onto IN2 and links the pair.
        match p.dialog_result(DialogOutcome::Button(0), &state) {
            ScreenEvent::Command(c) => {
                assert!(c.starts_with("eq in.2 1 lowshelf 105 0.707 8.8"), "{c}");
                assert!(c.ends_with("pre 1 0"), "{c}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(p.list.mirror, Some(1));
    }

    /// D81 asked whether the alert should be ordered by the pair rather than
    /// by the page. It should not: `Components.swift:790-817` names
    /// `pageChannel` first in the title and offers it as the first button,
    /// "so the default keeps the tuning the user is actually looking at". From
    /// IN2 the Console reads "Inputs 2 and 1 don't match" too.
    #[test]
    fn the_mismatch_alert_is_ordered_by_the_page_it_was_raised_from() {
        let state = mismatched();
        let mut p = InputPage::new(1, shared(), &state);
        p.header = Some(0);
        match p.handle(key(KeyCode::Enter), &state) {
            ScreenEvent::Dialog(d) => {
                assert_eq!(d.title, "Inputs 2 and 1 don't match");
                assert_eq!(d.buttons[0].label, "Keep IN2");
                assert_eq!(d.buttons[1].label, "Keep IN1");
            }
            other => panic!("{other:?}"),
        }
        // And the first button keeps the page you are on.
        match p.dialog_result(DialogOutcome::Button(0), &state) {
            ScreenEvent::Command(c) => assert!(c.starts_with("eq in.1 "), "{c}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_matching_pair_links_with_no_prompt() {
        // The fixture tunes IN1 and IN2 the same, which is the case that must
        // link with no prompt at all.
        let state = fixture::state();
        let mut p = InputPage::new(0, shared(), &state);
        p.header = Some(0);
        assert_eq!(
            p.handle(key(KeyCode::Enter), &state),
            ScreenEvent::Status("Link 1/2 on".into())
        );
        assert_eq!(p.list.mirror, Some(1));
        // And unlinking says so.
        assert_eq!(
            p.handle(key(KeyCode::Enter), &state),
            ScreenEvent::Status("Link 1/2 off".into())
        );
    }

    #[test]
    fn clear_peq_confirms_and_then_writes_every_band_off() {
        let (mut p, state) = page();
        p.header = Some(2);
        match p.handle(key(KeyCode::Enter), &state) {
            ScreenEvent::Dialog(d) => assert_eq!(d.title, "Clear All Bands?"),
            other => panic!("{other:?}"),
        }
        match p.dialog_result(DialogOutcome::Button(0), &state) {
            ScreenEvent::Command(c) => {
                assert_eq!(c.lines().count(), 10);
                assert_eq!(c.lines().next().unwrap(), "eq in.1 1 flat 1000 0.707 0");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn down_moves_into_the_list_and_up_comes_back() {
        let (mut p, state) = page();
        assert_eq!(p.handle(key(KeyCode::Down), &state), ScreenEvent::Handled);
        assert!(p.header.is_none());
        assert_eq!(p.handle(key(KeyCode::Down), &state), ScreenEvent::Handled);
        assert_eq!(p.list.band, 1);
        p.handle(key(KeyCode::Up), &state);
        assert_eq!(p.list.band, 0);
        p.handle(key(KeyCode::Up), &state);
        assert!(
            p.header.is_some(),
            "up off the first band returns to the header"
        );
    }

    #[test]
    fn a_band_edit_mirrors_while_linked() {
        let (mut p, state) = page();
        p.link_now(&state);
        p.header = None;
        p.list.band = 1;
        assert_eq!(
            p.handle(key(KeyCode::Char(' ')), &state),
            ScreenEvent::Command("eq.bypass in.1 2 on\neq.bypass in.2 2 on".into())
        );
    }

    #[test]
    fn every_advertised_key_is_handled() {
        let state = fixture::state();
        for help in KEYS {
            for k in crate::screens::tests::keys_for(help.key) {
                for header in [Some(1usize), None] {
                    let mut p = InputPage::new(0, shared(), &state);
                    p.header = header;
                    p.list.band = 2;
                    p.list.field = 1;
                    let before = (p.header, p.list.band, p.list.field);
                    let ev = p.handle(k, &state);
                    let after = (p.header, p.list.band, p.list.field);
                    if ev != ScreenEvent::Unhandled || before != after {
                        continue;
                    }
                    // Backspace is the preamp's reset, so it does nothing in
                    // the list; every other key must do something somewhere.
                    assert!(
                        k.code == KeyCode::Backspace,
                        "{:?} does nothing with header {:?} (from {:?})",
                        k.code,
                        header,
                        help.key
                    );
                }
            }
        }
    }
}
