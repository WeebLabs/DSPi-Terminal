//! What a screen is, from the shell's point of view.
//!
//! The detail region, every tool panel and every Settings page implement
//! this. The shell owns focus, the frame, dialogs and popups; a screen owns
//! what is inside its rectangle and reports what the person asked for.

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::theme::Theme;
use crate::widgets::{Dialog, KeyHelp, PopupList};

/// What a screen wants after handling a key.
#[derive(Debug, Clone, PartialEq)]
pub enum ScreenEvent {
    /// The key was not for this screen; the shell may use it.
    Unhandled,
    /// Handled; nothing for the shell to do.
    Handled,
    /// Open a popup list; the screen gets `PopupList` results back through
    /// `Screen::popup_result`.
    Popup(PopupList),
    /// Open a dialog; results come back through `Screen::dialog_result`.
    Dialog(Dialog),
    /// A command in the shared grammar, to run through the session.
    Command(String),
    /// Something to say on the echo line.
    Status(String),
    /// The screen wants to close (a tool panel or Settings).
    Close,
}

pub trait Screen {
    /// The panel title, drawn by the shell when the screen fills the pane.
    fn title(&self) -> String;

    fn draw(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme, focused: bool);

    fn handle(&mut self, key: KeyEvent) -> ScreenEvent;

    /// Keys for the key line and the help overlay.
    fn keys(&self) -> &'static [KeyHelp];

    /// A popup this screen opened has closed with a choice (or none).
    fn popup_result(&mut self, _choice: Option<usize>) -> ScreenEvent {
        ScreenEvent::Handled
    }

    /// A dialog this screen opened has closed.
    fn dialog_result(&mut self, _outcome: crate::widgets::DialogOutcome) -> ScreenEvent {
        ScreenEvent::Handled
    }

    /// Called every tick with the elapsed time, for screens with motion.
    fn tick(&mut self, _dt_ms: u32) {}
}

/// A screen with nothing in it yet: names the selection and its keys.
pub struct Placeholder {
    pub title: String,
    pub body: String,
}

impl Placeholder {
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
        }
    }
}

impl Screen for Placeholder {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn draw(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme, _focused: bool) {
        if area.height == 0 {
            return;
        }
        buf.set_string(area.x + 1, area.y, &self.title, theme.title());
        for (i, line) in
            crate::widgets::text::wrap(&self.body, area.width.saturating_sub(2) as usize, 6)
                .iter()
                .enumerate()
        {
            if 1 + i as u16 >= area.height {
                break;
            }
            buf.set_string(area.x + 1, area.y + 1 + i as u16, line, theme.label());
        }
    }

    fn handle(&mut self, _key: KeyEvent) -> ScreenEvent {
        ScreenEvent::Unhandled
    }

    fn keys(&self) -> &'static [KeyHelp] {
        &[]
    }
}
