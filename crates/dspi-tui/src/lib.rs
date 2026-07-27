//! The DSPi terminal interface.

pub mod app;
pub mod theme;
pub mod widgets;

pub use app::{App, run};

/// The terminal's current size, or `None` when stdout is not a terminal.
///
/// Exposed here so callers do not need their own crossterm dependency, which
/// could drift to a different version than the one actually drawing.
pub fn terminal_size() -> Option<(u16, u16)> {
    crossterm::terminal::size().ok()
}

/// Render one frame to text, without a terminal.
///
/// Used by the golden-frame tests and by `dspi screenshot`, so what the tests
/// check is exactly what a user sees rather than an approximation of it.
pub fn render_to_string(app: &App, width: u16, height: u16) -> String {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut term = Terminal::new(TestBackend::new(width, height)).expect("test backend");
    term.draw(|f| app.draw(f)).expect("draw");
    let buf = term.backend().buffer();
    let area = *buf.area();
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}
pub use theme::Theme;
