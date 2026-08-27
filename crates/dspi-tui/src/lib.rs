//! The DSPi terminal interface.

pub mod app;
pub mod fields;
pub mod graph;
pub mod live;
pub mod screens;
pub mod shell;
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

/// Render one frame with its colours, as ANSI escapes.
///
/// The plain text renderer strips styling, which makes it useless for reviewing
/// a palette. This keeps the colours so a theme can be looked at by piping the
/// output to a terminal.
pub fn render_to_ansi(app: &App, width: u16, height: u16) -> String {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;

    fn sgr(c: Color, fg: bool) -> String {
        let base = if fg { 38 } else { 48 };
        match c {
            Color::Rgb(r, g, b) => format!("\x1b[{base};2;{r};{g};{b}m"),
            Color::Indexed(i) => format!("\x1b[{base};5;{i}m"),
            Color::Reset => format!("\x1b[{}m", if fg { 39 } else { 49 }),
            _ => String::new(),
        }
    }

    let mut term = Terminal::new(TestBackend::new(width, height)).expect("test backend");
    term.draw(|f| app.draw(f)).expect("draw");
    let buf = term.backend().buffer();
    let area = *buf.area();

    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            let cell = &buf[(x, y)];
            out.push_str("\x1b[0m");
            out.push_str(&sgr(cell.fg, true));
            out.push_str(&sgr(cell.bg, false));
            if cell.modifier.contains(ratatui::style::Modifier::REVERSED) {
                out.push_str("\x1b[7m");
            }
            if cell.modifier.contains(ratatui::style::Modifier::BOLD) {
                out.push_str("\x1b[1m");
            }
            out.push_str(cell.symbol());
        }
        out.push_str("\x1b[0m\n");
    }
    out
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
