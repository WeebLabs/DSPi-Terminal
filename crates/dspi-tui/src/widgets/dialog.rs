//! Modal dialogs: confirm, text entry, list, checklist, progress and report.
//!
//! One type, several kinds, so every dialog gets the same frame, the same
//! button row and the same keys: `Tab` and arrows between buttons, `Enter`
//! for the default, `Esc` for cancel, a button's first letter as its
//! accelerator.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Widget};

use super::text::{fit_left, truncate, wrap};
use crate::theme::{Glyphs, Theme};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    pub label: String,
    /// Drawn in `danger`: Discard, Clear, Factory Reset.
    pub destructive: bool,
}

impl Button {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            destructive: false,
        }
    }
    pub fn destructive(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            destructive: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DialogKind {
    /// Body text and buttons.
    Confirm,
    /// A single text field.
    Text { value: String, placeholder: String },
    /// A list to pick one item from; `cursor` is the highlighted row.
    List {
        items: Vec<String>,
        cursor: usize,
        filter: String,
    },
    /// Check any number of items.
    Checklist {
        items: Vec<(String, bool)>,
        cursor: usize,
    },
    /// A progress bar 0..1 with a status line; no buttons until done.
    Progress { fraction: f32, status: String },
    /// Scrolling text, usually a result report.
    Report { lines: Vec<String>, scroll: usize },
}

/// What a dialog key produced.
#[derive(Debug, Clone, PartialEq)]
pub enum DialogOutcome {
    /// A button by index (the default is 0 unless `default` says otherwise).
    Button(usize),
    Cancelled,
    /// Text kind: the committed text.
    Text(String),
    /// List kind: the chosen item.
    Picked(usize),
    /// Checklist kind: which items are checked, with the button pressed.
    Checked(Vec<bool>, usize),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Dialog {
    pub title: String,
    pub body: String,
    pub kind: DialogKind,
    pub buttons: Vec<Button>,
    pub focus: usize,
    pub default: usize,
    /// A critical dialog draws its border in `danger`.
    pub critical: bool,
}

impl Dialog {
    pub fn confirm(
        title: impl Into<String>,
        body: impl Into<String>,
        buttons: Vec<Button>,
    ) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            kind: DialogKind::Confirm,
            buttons,
            focus: 0,
            default: 0,
            critical: false,
        }
    }

    pub fn text(
        title: impl Into<String>,
        body: impl Into<String>,
        value: impl Into<String>,
        placeholder: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            kind: DialogKind::Text {
                value: value.into(),
                placeholder: placeholder.into(),
            },
            buttons: vec![Button::new("OK"), Button::new("Cancel")],
            focus: 0,
            default: 0,
            critical: false,
        }
    }

    pub fn list(
        title: impl Into<String>,
        body: impl Into<String>,
        items: Vec<String>,
        cursor: usize,
    ) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            kind: DialogKind::List {
                items,
                cursor,
                filter: String::new(),
            },
            buttons: vec![Button::new("Select"), Button::new("Cancel")],
            focus: 0,
            default: 0,
            critical: false,
        }
    }

    pub fn checklist(
        title: impl Into<String>,
        body: impl Into<String>,
        items: Vec<(String, bool)>,
        buttons: Vec<Button>,
    ) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            kind: DialogKind::Checklist { items, cursor: 0 },
            buttons,
            focus: 0,
            default: 0,
            critical: false,
        }
    }

    pub fn progress(title: impl Into<String>, status: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: String::new(),
            kind: DialogKind::Progress {
                fraction: 0.0,
                status: status.into(),
            },
            buttons: Vec::new(),
            focus: 0,
            default: 0,
            critical: false,
        }
    }

    pub fn report(title: impl Into<String>, lines: Vec<String>) -> Self {
        Self {
            title: title.into(),
            body: String::new(),
            kind: DialogKind::Report { lines, scroll: 0 },
            buttons: vec![Button::new("OK")],
            focus: 0,
            default: 0,
            critical: false,
        }
    }

    pub fn critical(mut self) -> Self {
        self.critical = true;
        self
    }

    pub fn default_button(mut self, i: usize) -> Self {
        self.default = i;
        self.focus = i;
        self
    }

    /// Advance a progress dialog.
    pub fn set_progress(&mut self, fraction: f32, status: impl Into<String>) {
        if let DialogKind::Progress {
            fraction: f,
            status: s,
        } = &mut self.kind
        {
            *f = fraction.clamp(0.0, 1.0);
            *s = status.into();
        }
    }

    fn cancel_index(&self) -> Option<usize> {
        self.buttons.iter().position(|b| b.label == "Cancel")
    }

    fn finish(&self, button: usize) -> DialogOutcome {
        if Some(button) == self.cancel_index() {
            return DialogOutcome::Cancelled;
        }
        match &self.kind {
            DialogKind::Text { value, .. } => DialogOutcome::Text(value.clone()),
            DialogKind::List { cursor, .. } => DialogOutcome::Picked(*cursor),
            DialogKind::Checklist { items, .. } => {
                DialogOutcome::Checked(items.iter().map(|(_, c)| *c).collect(), button)
            }
            _ => DialogOutcome::Button(button),
        }
    }

    pub fn handle(&mut self, key: KeyEvent) -> Option<DialogOutcome> {
        let n = self.buttons.len();
        // Kind-specific keys first.
        match &mut self.kind {
            DialogKind::Text { value, .. } => match key.code {
                KeyCode::Char(c) => {
                    value.push(c);
                    return None;
                }
                KeyCode::Backspace => {
                    value.pop();
                    return None;
                }
                _ => {}
            },
            DialogKind::List {
                items,
                cursor,
                filter,
            } => match key.code {
                KeyCode::Up => {
                    *cursor = cursor.saturating_sub(1);
                    return None;
                }
                KeyCode::Down => {
                    *cursor = (*cursor + 1).min(items.len().saturating_sub(1));
                    return None;
                }
                KeyCode::Char(c) if c != ' ' => {
                    filter.push(c);
                    let f = filter.to_lowercase();
                    if let Some(i) = items.iter().position(|it| it.to_lowercase().contains(&f)) {
                        *cursor = i;
                    }
                    return None;
                }
                KeyCode::Backspace => {
                    filter.pop();
                    return None;
                }
                _ => {}
            },
            DialogKind::Checklist { items, cursor } => match key.code {
                KeyCode::Up => {
                    *cursor = cursor.saturating_sub(1);
                    return None;
                }
                KeyCode::Down => {
                    *cursor = (*cursor + 1).min(items.len().saturating_sub(1));
                    return None;
                }
                KeyCode::Char(' ') => {
                    if let Some(it) = items.get_mut(*cursor) {
                        it.1 = !it.1;
                    }
                    return None;
                }
                _ => {}
            },
            DialogKind::Report { lines, scroll } => match key.code {
                KeyCode::Up => {
                    *scroll = scroll.saturating_sub(1);
                    return None;
                }
                KeyCode::Down => {
                    *scroll = (*scroll + 1).min(lines.len().saturating_sub(1));
                    return None;
                }
                _ => {}
            },
            DialogKind::Progress { .. } => {
                return if key.code == KeyCode::Esc {
                    Some(DialogOutcome::Cancelled)
                } else {
                    None
                };
            }
            DialogKind::Confirm => {}
        }

        match key.code {
            KeyCode::Esc => Some(DialogOutcome::Cancelled),
            KeyCode::Enter => Some(self.finish(self.focus)),
            KeyCode::Tab | KeyCode::Right if n > 0 => {
                self.focus = (self.focus + 1) % n;
                None
            }
            KeyCode::BackTab | KeyCode::Left if n > 0 => {
                self.focus = (self.focus + n - 1) % n;
                None
            }
            KeyCode::Char(c) => {
                // Accelerators: the first letter of a button, when the kind
                // does not consume letters.
                // A letter two buttons share is no accelerator at all: `c`
                // must never pick `Clear All` over `Cancel`.
                let lc = c.to_ascii_lowercase();
                let matches: Vec<usize> = self
                    .buttons
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| {
                        b.label
                            .chars()
                            .next()
                            .is_some_and(|f| f.to_ascii_lowercase() == lc)
                    })
                    .map(|(i, _)| i)
                    .collect();
                match matches.as_slice() {
                    [i] => Some(self.finish(*i)),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// The size the dialog wants inside `max`.
    pub fn size(&self, max: Rect) -> Rect {
        let w = match &self.kind {
            DialogKind::Report { lines, .. } => {
                lines.iter().map(|l| l.len()).max().unwrap_or(30).max(30) as u16 + 4
            }
            DialogKind::List { items, .. } => {
                items.iter().map(|l| l.len()).max().unwrap_or(30).max(30) as u16 + 6
            }
            _ => 56,
        }
        .min(max.width.saturating_sub(4))
        .max(20);
        let body_lines = wrap(&self.body, w as usize - 4, 12).len() as u16;
        let kind_rows = match &self.kind {
            DialogKind::Confirm => 0,
            DialogKind::Text { .. } => 2,
            DialogKind::List { items, .. } => (items.len() as u16).min(10) + 1,
            DialogKind::Checklist { items, .. } => items.len() as u16 + 1,
            DialogKind::Progress { .. } => 3,
            DialogKind::Report { lines, .. } => (lines.len() as u16).min(14) + 1,
        };
        let buttons = u16::from(!self.buttons.is_empty()) * 2;
        let h = (2 + body_lines + u16::from(body_lines > 0) + kind_rows + buttons)
            .min(max.height.saturating_sub(2))
            .max(5);
        let x = max.x + (max.width.saturating_sub(w)) / 2;
        let y = max.y + (max.height.saturating_sub(h)) / 2;
        Rect::new(x, y, w, h)
    }

    pub fn draw(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let t = theme;
        Clear.render(area, buf);
        let border = if self.critical { t.danger } else { t.accent };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(if t.glyphs == Glyphs::Ascii {
                BorderType::Plain
            } else {
                BorderType::Rounded
            })
            .border_style(Style::default().fg(border))
            .title(format!(" {} ", self.title))
            .title_style(t.title());
        let inner = block.inner(area);
        block.render(area, buf);
        if inner.height == 0 {
            return;
        }
        let w = inner.width as usize - 2;
        let mut y = inner.y;
        let bottom = inner.y + inner.height;
        let button_rows = u16::from(!self.buttons.is_empty()) * 2;

        for line in wrap(&self.body, w, 12) {
            if y + button_rows >= bottom {
                break;
            }
            buf.set_string(inner.x + 1, y, &line, t.value());
            y += 1;
        }
        if !self.body.is_empty() {
            y += 1;
        }

        match &self.kind {
            DialogKind::Confirm => {}
            DialogKind::Text { value, placeholder } => {
                if y < bottom {
                    let shown = if value.is_empty() {
                        (placeholder.clone(), t.label())
                    } else {
                        (value.clone(), t.value())
                    };
                    let field = fit_left(&format!("{}▏", shown.0), w);
                    buf.set_string(
                        inner.x + 1,
                        y,
                        &field,
                        shown.1.add_modifier(Modifier::UNDERLINED),
                    );
                }
            }
            DialogKind::List {
                items,
                cursor,
                filter,
            } => {
                let rows = (bottom.saturating_sub(y + button_rows)) as usize;
                let start = cursor.saturating_sub(rows.saturating_sub(1));
                for (i, item) in items.iter().enumerate().skip(start).take(rows) {
                    let (text, style) = if i == *cursor {
                        (format!("▸ {item}"), t.pill(t.accent))
                    } else {
                        (format!("  {item}"), t.value())
                    };
                    buf.set_string(inner.x + 1, y, fit_left(&text, w), style);
                    y += 1;
                }
                if !filter.is_empty() && y < bottom {
                    buf.set_string(
                        inner.x + 1,
                        y,
                        truncate(&format!("/{filter}"), w),
                        t.label(),
                    );
                }
            }
            DialogKind::Checklist { items, cursor } => {
                for (i, (item, checked)) in items.iter().enumerate() {
                    if y + button_rows >= bottom {
                        break;
                    }
                    let box_ = match (checked, t.glyphs) {
                        (true, Glyphs::Ascii) => "[x]",
                        (false, Glyphs::Ascii) => "[ ]",
                        (true, _) => "☑",
                        (false, _) => "☐",
                    };
                    let text = format!("{box_} {item}");
                    let style = if i == *cursor { t.focused() } else { t.value() };
                    buf.set_string(inner.x + 1, y, fit_left(&text, w), style);
                    y += 1;
                }
            }
            DialogKind::Progress { fraction, status } => {
                if y + 1 < bottom {
                    let bar_w = w;
                    let filled = (fraction * bar_w as f32).round() as usize;
                    let (f, e) = if t.glyphs == Glyphs::Ascii {
                        ("#", ".")
                    } else {
                        ("█", "░")
                    };
                    let bar = format!("{}{}", f.repeat(filled), e.repeat(bar_w - filled));
                    buf.set_string(inner.x + 1, y, &bar, Style::default().fg(t.accent));
                    buf.set_string(inner.x + 1, y + 1, truncate(status, w), t.label());
                }
            }
            DialogKind::Report { lines, scroll } => {
                let rows = (bottom.saturating_sub(y + button_rows)) as usize;
                for line in lines.iter().skip(*scroll).take(rows) {
                    buf.set_string(inner.x + 1, y, fit_left(line, w), t.value());
                    y += 1;
                }
            }
        }

        // Buttons, right-aligned on the last row.
        if !self.buttons.is_empty() {
            let by = bottom - 1;
            let total: usize = self.buttons.iter().map(|b| b.label.len() + 4).sum();
            let mut x = inner.x + inner.width - total.min(inner.width as usize) as u16;
            for (i, b) in self.buttons.iter().enumerate() {
                let text = format!(" {} ", b.label);
                let color = if b.destructive {
                    t.danger
                } else if i == self.default {
                    t.accent
                } else {
                    t.fg
                };
                let style = if i == self.focus {
                    t.pill(color).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(color)
                };
                buf.set_string(x, by, &text, style);
                x += text.len() as u16 + 2;
            }
        }
    }
}

impl Dialog {
    /// Convenience: is a key one this dialog would swallow? Everything, while
    /// it is open; the screen behind it gets nothing.
    pub fn is_modal(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::key;
    use super::*;
    use crate::theme::ColorDepth;

    fn draw(d: &Dialog, w: u16, h: u16) -> String {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        term.draw(|f| {
            let area = d.size(Rect::new(0, 0, w, h));
            d.draw(area, f.buffer_mut(), &t)
        })
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
    fn a_confirm_has_its_buttons_on_the_last_row_with_accelerators() {
        let mut d = Dialog::confirm(
            "Unsaved Changes",
            "The current preset has unsaved changes. Save before continuing?",
            vec![
                Button::new("Save"),
                Button::destructive("Discard"),
                Button::new("Cancel"),
            ],
        );
        let s = draw(&d, 70, 12);
        assert!(s.contains(" Unsaved Changes "), "{s}");
        assert!(s.contains("Save    Discard    Cancel"), "{s}");
        assert_eq!(
            d.handle(key(KeyCode::Char('d'))),
            Some(DialogOutcome::Button(1))
        );
        assert_eq!(
            d.handle(key(KeyCode::Char('c'))),
            Some(DialogOutcome::Cancelled)
        );
        assert_eq!(d.handle(key(KeyCode::Esc)), Some(DialogOutcome::Cancelled));
        d.handle(key(KeyCode::Tab));
        assert_eq!(d.focus, 1);
        assert_eq!(
            d.handle(key(KeyCode::Enter)),
            Some(DialogOutcome::Button(1))
        );
    }

    #[test]
    fn a_shared_first_letter_is_not_an_accelerator() {
        let mut d = Dialog::confirm(
            "Clear All Bands?",
            "",
            vec![Button::destructive("Clear All"), Button::new("Cancel")],
        );
        assert_eq!(d.handle(key(KeyCode::Char('c'))), None, "ambiguous");
        assert_eq!(d.handle(key(KeyCode::Esc)), Some(DialogOutcome::Cancelled));
    }

    #[test]
    fn a_text_dialog_types_and_commits() {
        let mut d = Dialog::text("Rename Preset", "", "Living Room", "Name");
        d.handle(key(KeyCode::Backspace));
        d.handle(key(KeyCode::Char('s')));
        assert_eq!(
            d.handle(key(KeyCode::Enter)),
            Some(DialogOutcome::Text("Living Roos".into()))
        );
        let s = draw(&d, 70, 12);
        assert!(s.contains("Living Roos▏"), "{s}");
    }

    #[test]
    fn a_list_picks_by_cursor_and_jumps_on_typing() {
        let mut d = Dialog::list(
            "Device",
            "",
            vec!["A1B2".into(), "C3D4".into(), "E5F6".into()],
            0,
        );
        d.handle(key(KeyCode::Down));
        assert_eq!(
            d.handle(key(KeyCode::Enter)),
            Some(DialogOutcome::Picked(1))
        );
        d.handle(key(KeyCode::Char('e')));
        assert_eq!(
            d.handle(key(KeyCode::Enter)),
            Some(DialogOutcome::Picked(2))
        );
    }

    #[test]
    fn a_checklist_reports_what_is_checked() {
        let mut d = Dialog::checklist(
            "Import Device Configuration",
            "",
            vec![
                ("Volume levels".into(), false),
                ("Hardware I/O".into(), false),
            ],
            vec![Button::new("Import"), Button::new("Cancel")],
        );
        d.handle(key(KeyCode::Down));
        d.handle(key(KeyCode::Char(' ')));
        assert_eq!(
            d.handle(key(KeyCode::Enter)),
            Some(DialogOutcome::Checked(vec![false, true], 0))
        );
        let s = draw(&d, 70, 12);
        assert!(
            s.contains("☐ Volume levels") && s.contains("☑ Hardware I/O"),
            "{s}"
        );
    }

    #[test]
    fn progress_draws_a_bar_and_takes_no_buttons() {
        let mut d = Dialog::progress("Import", "Writing settings to the device...");
        d.set_progress(0.5, "Writing settings to the device...");
        let s = draw(&d, 70, 10);
        assert!(s.contains("█████"), "{s}");
        assert!(s.contains("Writing settings"), "{s}");
        assert_eq!(d.handle(key(KeyCode::Enter)), None);
    }
}
