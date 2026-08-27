//! The widget kit: every visual element the screens are built from.
//!
//! Each widget is a function of its data, the theme and whether it has focus,
//! and each one that takes input owns its key handling through `handle()` so
//! screens do not re-implement arrows and Enter differently. The design these
//! widgets follow is `docs/plan/DESIGN.md` section 6.
//!
//! `legacy` holds the widgets the previous interface drew with; it goes when
//! the screens that use it are rewritten.

pub mod card;
pub mod chips;
pub mod dialog;
pub mod help;
pub mod legacy;
pub mod legend;
pub mod meter;
pub mod param;
pub mod picker;
pub mod pingrid;
pub mod savebar;
pub mod section;
pub mod slider;
pub mod status;
pub mod table;
pub mod text;
pub mod toggle;

pub use legacy::*;

pub use card::Card;
pub use chips::{ChipRow, ChipState};
pub use dialog::{Button, Dialog, DialogKind, DialogOutcome};
pub use help::{HelpOverlay, KeyHelp};
pub use legend::{LegendPill, LegendRow};
pub use meter::{CpuMeter, LevelMeter, PeakHold};
pub use param::{NumberEdit, ParamRow, Taper};
pub use picker::{PickerRow, PopupList, Segmented};
pub use pingrid::{PinCell, PinGrid, PinRole};
pub use savebar::SaveBar;
pub use section::SectionHeader;
pub use status::{Banner, BannerKind, StatusPill, StatusTone};
pub use table::{Column, Table};
pub use toggle::ToggleRow;

/// What a widget wants the screen to do after a key.
///
/// Widgets never touch the device; they report what the person asked for and
/// the screen turns that into a write.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// A numeric value moved (live, while nudging or dragging).
    Changed(f64),
    /// A numeric value was committed (Enter after typing, or a nudge that
    /// should be confirmed).
    Committed(f64),
    /// A boolean flipped.
    Toggled(bool),
    /// A choice was picked, by index.
    Selected(usize),
    /// The Backspace reset, to the widget's default.
    Reset,
    /// Enter on something that opens (a popup, a dialog, a panel).
    Open,
    /// Escape closed a popup or reverted an edit.
    Closed,
    /// A button in a dialog, by index.
    Button(usize),
    /// A chip changed state, by index.
    Chip(usize, ChipState),
    /// Focus should move to the next or previous element.
    Next,
    Prev,
}

#[cfg(test)]
pub(crate) mod testing {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::widgets::Widget;

    /// Render a widget alone into a buffer of the given size, as text.
    pub fn render<W: Widget>(w: W, width: u16, height: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(width, height)).expect("backend");
        term.draw(|f| {
            let area = Rect::new(0, 0, width, height);
            w.render(area, f.buffer_mut());
        })
        .expect("draw");
        let buf = term.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The same, but returning the raw buffer so a test can look at styles.
    pub fn render_buf<W: Widget>(w: W, width: u16, height: u16) -> ratatui::buffer::Buffer {
        let mut term = Terminal::new(TestBackend::new(width, height)).expect("backend");
        term.draw(|f| {
            let area = Rect::new(0, 0, width, height);
            w.render(area, f.buffer_mut());
        })
        .expect("draw");
        term.backend().buffer().clone()
    }

    pub fn key(code: crossterm::event::KeyCode) -> crossterm::event::KeyEvent {
        crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
    }

    pub fn shift(code: crossterm::event::KeyCode) -> crossterm::event::KeyEvent {
        crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::SHIFT)
    }

    pub fn every_theme() -> Vec<crate::theme::Theme> {
        use crate::theme::{ColorDepth, Glyphs, Theme};
        vec![
            Theme::console(ColorDepth::TrueColor, Glyphs::Braille),
            Theme::console(ColorDepth::Ansi256, Glyphs::Braille),
            Theme::console(ColorDepth::Ansi16, Glyphs::Blocks),
            Theme::mono(Glyphs::Ascii),
        ]
    }
}
