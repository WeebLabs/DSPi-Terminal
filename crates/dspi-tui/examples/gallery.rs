//! Render the shell with fixture data, for design review without a device.
//!
//!   gallery [width] [height] [console|amber|dark|mono] [rp2350|rp2040] [--ansi]
//!
//! Prints the frame as text, or as ANSI escapes with `--ansi` so the colours
//! can be looked at by piping to a terminal.

use dspi_tui::shell::{Placeholder, Shell, fixture};
use dspi_tui::theme::{ColorDepth, Glyphs, Palette, Theme};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::{Color, Modifier};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ansi = args.iter().any(|a| a == "--ansi");
    let args: Vec<&String> = args.iter().filter(|a| *a != "--ansi").collect();
    let num = |i: usize, d: u16| args.get(i).and_then(|a| a.parse().ok()).unwrap_or(d);
    let (w, h) = (num(0, 120), num(1, 40));
    let palette = args
        .get(2)
        .and_then(|a| Palette::parse(a))
        .unwrap_or(Palette::Console);
    let depth = if ansi {
        ColorDepth::detect()
    } else {
        ColorDepth::TrueColor
    };
    let theme = Theme::new(palette, depth, Glyphs::Braille);
    let model = match args.get(3).map(|s| s.as_str()) {
        Some("rp2040") => fixture::rp2040(&theme),
        _ => fixture::rp2350(&theme),
    };
    let mut shell = Shell::new(
        model,
        theme,
        Box::new(Placeholder::new("FL", "Input page (Phase 4)")),
    );

    let mut term = Terminal::new(TestBackend::new(w, h)).expect("backend");
    term.draw(|f| shell.draw(f.area(), f.buffer_mut()))
        .expect("draw");
    let buf = term.backend().buffer();

    fn sgr(c: Color, fg: bool) -> String {
        let base = if fg { 38 } else { 48 };
        match c {
            Color::Rgb(r, g, b) => format!("\x1b[{base};2;{r};{g};{b}m"),
            Color::Indexed(i) => format!("\x1b[{base};5;{i}m"),
            Color::Reset => format!("\x1b[{}m", if fg { 39 } else { 49 }),
            _ => String::new(),
        }
    }

    for y in 0..h {
        let mut line = String::new();
        for x in 0..w {
            let cell = &buf[(x, y)];
            if ansi {
                line.push_str("\x1b[0m");
                line.push_str(&sgr(cell.fg, true));
                line.push_str(&sgr(cell.bg, false));
                if cell.modifier.contains(Modifier::REVERSED) {
                    line.push_str("\x1b[7m");
                }
                if cell.modifier.contains(Modifier::BOLD) {
                    line.push_str("\x1b[1m");
                }
            }
            line.push_str(cell.symbol());
        }
        if ansi {
            line.push_str("\x1b[0m");
        }
        println!("{}", line.trim_end());
    }
}
