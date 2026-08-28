//! The AutoEQ browser: `AutoEQ/AutoEQBrowser.swift` as a tool panel.
//!
//! Search on top, results below with the Console's source capsule and its
//! heart, and a bottom bar carrying the selection summary and the two actions.
//! The favourites are the terminal's version of the Console's Favorite
//! Profiles submenu: with the search empty they are the list, under their own
//! header, so the profiles someone actually uses are one keypress away.
//!
//! Applying does not write anything here. It asks for the same `pre` and `eq`
//! commands a person could type, which is what puts an AutoEQ profile on the
//! undo stack and on the echo line like every other change.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use dspi_session::DeviceState;
use dspi_session::autoeq::{Database, Entry};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;

use super::{Shared, channel_token, number, type_token};
use crate::shell::{Screen, ScreenEvent};
use crate::theme::{ChannelRole, Glyphs, Theme};
use crate::widgets::text::{fit_left, truncate};
use crate::widgets::{KeyHelp, SectionHeader};

const KEYS: &[KeyHelp] = &[
    KeyHelp::new("/", "Search"),
    KeyHelp::new("↑ ↓", "Select"),
    KeyHelp::new("f", "Favourite"),
    KeyHelp::new("Enter", "Apply"),
];

/// The Console's placeholder and its three empty states.
pub const SEARCH_PLACEHOLDER: &str = "Search headphones...";
pub const LOADING: &str = "Loading headphone database...";
pub const NOTHING_SELECTED: &str = "Select a headphone to apply its EQ profile";

/// The Console's source capsule colours, mapped onto the palette: oratory1990
/// orange, crinacle purple (the violet the fifth input uses), rtings blue,
/// innerfidelity green, everything else grey.
pub fn source_color(source: &str, theme: &Theme) -> Color {
    match source {
        "oratory1990" => theme.warning,
        "crinacle" => theme.role_color(ChannelRole::Input(4)),
        "rtings" => theme.accent,
        "innerfidelity" => theme.ok,
        _ => theme.dim,
    }
}

pub struct AutoEqPanel {
    shared: Shared,
    query: String,
    /// Set while the search field is taking keys, so `f` is a favourite rather
    /// than a letter.
    typing: bool,
    cursor: usize,
    scroll: usize,
    /// Why the database would not load, which is the Console's error state.
    error: Option<String>,
    /// Whether a favourite change is written to the favourites file. Off in
    /// tests, which must not touch the person's own list.
    persist: bool,
}

impl AutoEqPanel {
    pub fn new(shared: Shared) -> Self {
        let mut error = None;
        {
            let need = shared.borrow().autoeq.is_none();
            if need {
                match dspi_session::autoeq::load() {
                    Ok(db) => {
                        let mut s = shared.borrow_mut();
                        s.autoeq = Some(std::rc::Rc::new(db));
                        s.favourites = dspi_session::autoeq::load_favourites();
                    }
                    Err(e) => error = Some(e.to_string()),
                }
            }
        }
        Self {
            shared,
            query: String::new(),
            typing: false,
            cursor: 0,
            scroll: 0,
            error,
            persist: true,
        }
    }

    /// The same panel with the favourites file left alone.
    #[cfg(test)]
    fn in_memory(shared: Shared) -> Self {
        Self {
            persist: false,
            ..Self::new(shared)
        }
    }

    fn database(&self) -> Option<std::rc::Rc<Database>> {
        self.shared.borrow().autoeq.clone()
    }

    /// What the list shows: the search hits, or the favourites when nothing
    /// has been typed.
    fn hits(&self, db: &Database) -> Vec<String> {
        if self.query.trim().is_empty() {
            let favourites = &self.shared.borrow().favourites;
            return favourites
                .iter()
                .filter(|id| db.get(id).is_some())
                .cloned()
                .collect();
        }
        db.search(&self.query)
            .into_iter()
            .take(500)
            .map(|e| e.id.clone())
            .collect()
    }

    fn selected_id(&self, db: &Database) -> Option<String> {
        self.hits(db).get(self.cursor).cloned()
    }

    fn is_favourite(&self, id: &str) -> bool {
        self.shared.borrow().favourites.iter().any(|f| f == id)
    }

    /// Toggle the selection's favourite flag, and write the list back.
    ///
    /// The list is saved on every change rather than at exit, because a
    /// terminal program is as likely to be closed with a signal as with `q`.
    fn toggle_favourite(&mut self, id: &str) -> ScreenEvent {
        let (added, ids) = {
            let mut s = self.shared.borrow_mut();
            match s.favourites.iter().position(|f| f == id) {
                Some(i) => {
                    s.favourites.remove(i);
                    (false, s.favourites.clone())
                }
                None => {
                    s.favourites.push(id.to_string());
                    (true, s.favourites.clone())
                }
            }
        };
        if self.persist
            && let Err(e) = dspi_session::autoeq::save_favourites(&ids)
        {
            return ScreenEvent::Status(e.to_string());
        }
        ScreenEvent::Status(if added {
            "Added to favourites".into()
        } else {
            "Removed from favourites".into()
        })
    }

    /// The commands that put a profile on every input channel.
    ///
    /// A headphone correction belongs on what is being listened to, so it goes
    /// on every input rather than one: applying it to one side only would be
    /// worse than not applying it at all.
    pub fn apply_commands(entry: &Entry, state: &DeviceState) -> (Vec<String>, String) {
        let plan = entry.plan(state.caps.max_bands);
        let mut out = Vec::new();
        for ch in 0..state.caps.num_inputs as usize {
            let token = channel_token(state, ch);
            out.push(format!("pre {token} {}", number(plan.preamp_db)));
            for b in &plan.bands {
                out.push(format!(
                    "eq {token} {} {} {} {} {}",
                    b.band + 1,
                    type_token(b.filter_type),
                    number(b.freq),
                    number(b.q),
                    number(b.gain_db),
                ));
            }
            for band in &plan.cleared {
                out.push(format!("eq {token} {} flat 1000 0.707 0", band + 1));
            }
        }

        let mut note = format!(
            "Applied {} to {} input channel(s): {} bands, preamp {:+.1} dB",
            entry.id,
            state.caps.num_inputs,
            plan.bands.len(),
            plan.preamp_db
        );
        if plan.dropped > 0 {
            note.push_str(&format!(
                ". The profile has {} bands, this device has {}; the rest were dropped",
                entry.filters.len(),
                state.caps.max_bands
            ));
        }
        if !plan.unsupported.is_empty() {
            note.push_str(&format!(
                ". Shapes this build does not know: {}",
                plan.unsupported.join(", ")
            ));
        }
        (out, note)
    }
}

impl Screen for AutoEqPanel {
    fn title(&self) -> String {
        "AutoEQ".into()
    }

    fn draw(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        _state: &DeviceState,
        focused: bool,
    ) {
        if area.height < 4 {
            return;
        }
        let db = self.database();
        let count = db.as_ref().map(|d| d.entries.len()).unwrap_or(0);
        let origin = match db.as_ref().map(|d| d.origin) {
            Some(dspi_session::autoeq::Origin::User) => "your copy",
            Some(dspi_session::autoeq::Origin::Bundled) => "built-in",
            None => "unavailable",
        };
        buf.set_string(
            area.x + 1,
            area.y,
            truncate(
                &format!("AutoEQ · Browse Profiles · {count} profiles, {origin}"),
                area.width.saturating_sub(2) as usize,
            ),
            theme.title(),
        );

        // The search field.
        let y = area.y + 1;
        let shown = if self.query.is_empty() && !self.typing {
            (SEARCH_PLACEHOLDER.to_string(), theme.label())
        } else if self.typing {
            (format!("{}▏", self.query), theme.editing())
        } else {
            (self.query.clone(), theme.value())
        };
        buf.set_string(area.x + 1, y, "Search", theme.label());
        buf.set_string(
            area.x + 9,
            y,
            fit_left(&shown.0, area.width.saturating_sub(10) as usize),
            shown.1.add_modifier(Modifier::UNDERLINED),
        );

        let list = Rect::new(area.x, area.y + 3, area.width, area.height - 4);
        let bottom = area.y + area.height - 1;

        let Some(db) = db else {
            buf.set_string(
                area.x + 1,
                list.y,
                self.error.as_deref().unwrap_or(LOADING),
                theme.label(),
            );
            return;
        };
        let hits = self.hits(&db);
        if self.query.trim().is_empty() {
            SectionHeader::new("Favourites", theme)
                .render(Rect::new(area.x, area.y + 2, area.width, 1), buf);
        }
        if hits.is_empty() {
            let empty = if self.query.trim().is_empty() {
                if count == 0 {
                    LOADING.to_string()
                } else {
                    "No favourites yet. Search for a headphone, then press f.".to_string()
                }
            } else {
                format!("No headphones found matching \"{}\"", self.query)
            };
            buf.set_string(
                area.x + 1,
                list.y,
                truncate(&empty, area.width as usize - 2),
                theme.label(),
            );
        } else {
            self.cursor = self.cursor.min(hits.len() - 1);
            let visible = list.height as usize;
            self.scroll = crate::widgets::Table::scroll_to(self.cursor, self.scroll, visible);
            for (i, id) in hits.iter().enumerate().skip(self.scroll).take(visible) {
                let Some(e) = db.get(id) else { continue };
                let ly = list.y + (i - self.scroll) as u16;
                let here = focused && i == self.cursor;
                buf.set_string(list.x, ly, if here { "▸" } else { " " }, theme.focused());
                let heart = match (self.is_favourite(id), theme.glyphs) {
                    (true, Glyphs::Ascii) => "*",
                    (false, _) => " ",
                    (true, _) => "♥",
                };
                buf.set_string(list.x + 1, ly, heart, Style::default().fg(theme.danger));
                let name_w = (list.width as usize).saturating_sub(32);
                buf.set_string(
                    list.x + 3,
                    ly,
                    fit_left(&e.display_name(), name_w),
                    if here { theme.focused() } else { theme.value() },
                );
                let sx = list.x + 3 + name_w as u16;
                let capsule = format!(" {} ", e.source_label());
                buf.set_string(
                    sx,
                    ly,
                    fit_left(&capsule, 16),
                    theme.pill(source_color(&e.source, theme)),
                );
                buf.set_string(sx + 17, ly, fit_left(&e.form_factor, 12), theme.label());
            }
        }

        // The bottom bar: the Console's selection summary and its two actions.
        let actions = "Esc Cancel   Enter Apply";
        let summary = match self.selected_id(&db).and_then(|id| {
            db.get(&id).map(|e| {
                format!(
                    "{}  {}  {}",
                    e.display_name(),
                    e.source_label(),
                    e.form_factor
                )
            })
        }) {
            Some(s) => s,
            None => NOTHING_SELECTED.into(),
        };
        buf.set_string(
            area.x + 1,
            bottom,
            truncate(
                &summary,
                (area.width as usize).saturating_sub(actions.len() + 3),
            ),
            theme.value(),
        );
        buf.set_string(
            area.x + area.width - actions.len() as u16 - 1,
            bottom,
            actions,
            theme.label(),
        );
    }

    fn handle(&mut self, key: KeyEvent, state: &DeviceState) -> ScreenEvent {
        if self.typing {
            match key.code {
                KeyCode::Esc | KeyCode::Enter => {
                    self.typing = false;
                    return ScreenEvent::Handled;
                }
                KeyCode::Backspace => {
                    self.query.pop();
                    self.cursor = 0;
                    self.scroll = 0;
                    return ScreenEvent::Handled;
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.query.push(c);
                    self.cursor = 0;
                    self.scroll = 0;
                    return ScreenEvent::Handled;
                }
                _ => {}
            }
        }
        let Some(db) = self.database() else {
            return ScreenEvent::Unhandled;
        };
        let hits = self.hits(&db);
        match key.code {
            KeyCode::Char('/') => {
                self.typing = true;
                ScreenEvent::Handled
            }
            KeyCode::Up => {
                self.cursor = self.cursor.saturating_sub(1);
                ScreenEvent::Handled
            }
            KeyCode::Down => {
                self.cursor = (self.cursor + 1).min(hits.len().saturating_sub(1));
                ScreenEvent::Handled
            }
            KeyCode::PageUp => {
                self.cursor = self.cursor.saturating_sub(10);
                ScreenEvent::Handled
            }
            KeyCode::PageDown => {
                self.cursor = (self.cursor + 10).min(hits.len().saturating_sub(1));
                ScreenEvent::Handled
            }
            KeyCode::Char('f') => match hits.get(self.cursor) {
                Some(id) => {
                    let id = id.clone();
                    self.toggle_favourite(&id)
                }
                None => ScreenEvent::Status(NOTHING_SELECTED.into()),
            },
            KeyCode::Enter => match hits.get(self.cursor).and_then(|id| db.get(id)) {
                Some(entry) => {
                    let (commands, note) = Self::apply_commands(entry, state);
                    ScreenEvent::Command(format!("{}\n# {note}", commands.join("\n")))
                }
                None => ScreenEvent::Status(NOTHING_SELECTED.into()),
            },
            _ => ScreenEvent::Unhandled,
        }
    }

    fn keys(&self) -> &'static [KeyHelp] {
        KEYS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::panel::testing;
    use crate::screens::shared;
    use crate::shell::Tool;
    use crate::widgets::testing::key;
    use dspi_session::autoeq::{Entry, Filter};

    fn entry(id: &str, mfr: &str, model: &str, source: &str) -> Entry {
        Entry {
            id: id.into(),
            manufacturer: mfr.into(),
            model: model.into(),
            source: source.into(),
            form_factor: "over-ear".into(),
            preamp: -6.5,
            filters: vec![
                Filter {
                    kind: "peaking".into(),
                    freq: 105.0,
                    q: 0.7,
                    gain: 3.0,
                },
                Filter {
                    kind: "lowShelf".into(),
                    freq: 60.0,
                    q: 0.71,
                    gain: 5.5,
                },
            ],
        }
    }

    fn db() -> Database {
        Database {
            version: 1,
            generated_at: "2026-01-18T05:41:39Z".into(),
            entry_count: 2,
            entries: vec![
                entry(
                    "oratory1990/Sennheiser HD 600",
                    "Sennheiser",
                    "HD 600",
                    "oratory1990",
                ),
                entry(
                    "crinacle/Moondrop Blessing 3",
                    "Moondrop",
                    "Blessing 3",
                    "crinacle",
                ),
            ],
            origin: dspi_session::autoeq::Origin::Bundled,
        }
    }

    fn panel() -> (AutoEqPanel, DeviceState, crate::screens::Shared) {
        let shared = shared();
        shared.borrow_mut().autoeq = Some(std::rc::Rc::new(db()));
        let p = AutoEqPanel::in_memory(shared.clone());
        (p, testing::state(), shared)
    }

    fn typed(p: &mut AutoEqPanel, state: &DeviceState, text: &str) {
        p.handle(key(KeyCode::Char('/')), state);
        for c in text.chars() {
            p.handle(key(KeyCode::Char(c)), state);
        }
        p.handle(key(KeyCode::Enter), state);
    }

    #[test]
    fn searching_lists_the_matches_with_their_source_capsule() {
        let (mut p, state, _) = panel();
        typed(&mut p, &state, "hd 600");
        let f = testing::draw(&mut p, &state, 100, 20);
        assert!(f.contains("Sennheiser HD 600"), "{f}");
        assert!(f.contains("oratory1990"), "the source capsule: {f}");
        assert!(f.contains("over-ear"), "{f}");
        assert!(
            !f.contains("Moondrop"),
            "the other entry does not match: {f}"
        );
        assert!(f.contains("Enter Apply") && f.contains("Esc Cancel"), "{f}");
    }

    #[test]
    fn nothing_matching_says_so_in_the_consoles_words() {
        let (mut p, state, _) = panel();
        typed(&mut p, &state, "wobbletron");
        let f = testing::draw(&mut p, &state, 100, 20);
        assert!(
            f.contains("No headphones found matching \"wobbletron\""),
            "{f}"
        );
        assert!(f.contains(NOTHING_SELECTED), "{f}");
    }

    /// With the search empty the list is the favourites, which is the
    /// terminal's version of the Favorite Profiles submenu.
    #[test]
    fn the_favourites_are_the_list_when_nothing_is_typed() {
        let (mut p, state, shared) = panel();
        let f = testing::draw(&mut p, &state, 100, 20);
        assert!(f.contains("FAVOURITES"), "{f}");
        assert!(f.contains("No favourites yet"), "{f}");

        shared
            .borrow_mut()
            .favourites
            .push("crinacle/Moondrop Blessing 3".into());
        let f = testing::draw(&mut p, &state, 100, 20);
        assert!(f.contains("Moondrop Blessing 3"), "{f}");
        assert!(f.contains("♥"), "the heart: {f}");
        assert!(!f.contains("Sennheiser"), "only favourites: {f}");
    }

    #[test]
    fn a_profile_applies_as_the_commands_a_person_could_type() {
        let (mut p, state, _) = panel();
        typed(&mut p, &state, "hd 600");
        let ev = p.handle(key(KeyCode::Enter), &state);
        let ScreenEvent::Command(cmds) = ev else {
            panic!("{ev:?}");
        };
        let lines: Vec<&str> = cmds.lines().collect();
        assert_eq!(lines[0], "pre in.1 -6.5");
        assert_eq!(lines[1], "eq in.1 1 peak 105 0.7 3");
        assert_eq!(lines[2], "eq in.1 2 lowshelf 60 0.71 5.5");
        // Everything the profile does not use is cleared, or the old tuning
        // survives underneath the correction.
        assert_eq!(lines[3], "eq in.1 3 flat 1000 0.707 0");
        assert!(
            lines.iter().any(|l| l.starts_with("pre in.8 ")),
            "every input, not just the first: {cmds}"
        );
        assert!(
            cmds.contains("# Applied oratory1990/Sennheiser HD 600 to 8 input channel(s)"),
            "{cmds}"
        );
    }

    #[test]
    fn f_toggles_the_favourite_on_the_selection() {
        let (mut p, state, shared) = panel();
        typed(&mut p, &state, "hd 600");
        let _ = p.handle(key(KeyCode::Char('f')), &state);
        assert_eq!(
            shared.borrow().favourites,
            vec!["oratory1990/Sennheiser HD 600".to_string()]
        );
        let _ = p.handle(key(KeyCode::Char('f')), &state);
        assert!(shared.borrow().favourites.is_empty());
    }

    /// While the search field is taking keys, `f` is a letter; the rest of the
    /// time it is the favourite toggle.
    #[test]
    fn the_search_field_is_armed_before_it_takes_letters() {
        let (mut p, state, shared) = panel();
        typed(&mut p, &state, "focal");
        assert_eq!(p.query, "focal");
        assert!(
            shared.borrow().favourites.is_empty(),
            "no favourite was set"
        );
        p.handle(key(KeyCode::Char('f')), &state);
        assert_eq!(p.query, "focal", "unarmed, f is not a letter");
    }

    #[test]
    fn the_source_capsules_are_the_consoles_five() {
        let t = crate::theme::Theme::console(
            crate::theme::ColorDepth::TrueColor,
            crate::theme::Glyphs::Braille,
        );
        assert_eq!(source_color("oratory1990", &t), t.warning);
        assert_eq!(source_color("rtings", &t), t.accent);
        assert_eq!(source_color("innerfidelity", &t), t.ok);
        assert_eq!(source_color("headphone.com", &t), t.dim);
        assert_ne!(source_color("crinacle", &t), t.dim);
    }

    #[test]
    fn golden_frames_at_both_sizes() {
        let shared = shared();
        shared.borrow_mut().autoeq = Some(std::rc::Rc::new(db()));
        shared
            .borrow_mut()
            .favourites
            .push("oratory1990/Sennheiser HD 600".into());
        let state = testing::state();
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            let f = testing::frame(
                Tool::AutoEq,
                Box::new(AutoEqPanel::in_memory(shared.clone())),
                &state,
                w,
                h,
            );
            assert_eq!(f.lines().count(), h as usize, "{w}x{h}");
            assert!(f.contains("AutoEQ"), "{w}x{h}:\n{f}");
            assert!(f.contains("B closes"), "{w}x{h}:\n{f}");
            assert!(f.contains("Favourite"), "the key line: {w}x{h}\n{f}");
        }
    }

    #[test]
    fn every_advertised_key_is_handled() {
        let (_, state, shared) = panel();
        shared
            .borrow_mut()
            .favourites
            .push("crinacle/Moondrop Blessing 3".into());
        for help in KEYS {
            for k in crate::screens::tests::keys_for(help.key) {
                let mut p = AutoEqPanel::in_memory(shared.clone());
                let before = (p.cursor, p.typing);
                let ev = p.handle(k, &state);
                assert!(
                    ev != ScreenEvent::Unhandled || (p.cursor, p.typing) != before,
                    "{:?} is not bound",
                    k.code
                );
            }
        }
    }
}
