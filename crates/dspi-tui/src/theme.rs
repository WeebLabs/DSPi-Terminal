//! Colour and glyph choices, and how they degrade.
//!
//! Two principles from the design: **data is bright, chrome is dim**, so values
//! and curves carry the colour while borders sit back; and **colour means
//! something**, so a channel's hue is the same in the graph, the legend and the
//! meters, and semantic colours are reserved for pending, clipping and hazard.

use ratatui::style::{Color, Modifier, Style};

/// How much colour the terminal can actually show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorDepth {
    TrueColor,
    Ansi256,
    Ansi16,
    Mono,
}

/// What the environment says about the terminal.
///
/// Gathered into a struct so the decision is a pure function and can be tested
/// for every platform, rather than only for whatever shell happens to be
/// running the test suite.
#[derive(Debug, Clone, Default)]
pub struct TermEnv {
    pub no_color: bool,
    pub colorterm: Option<String>,
    pub term: Option<String>,
    /// Windows Terminal sets this; conhost does not.
    pub wt_session: bool,
    /// ConEmu sets this to "ON" when it is handling ANSI itself.
    pub conemu_ansi: Option<String>,
    pub term_program: Option<String>,
    pub windows: bool,
    /// Set by the user to force the coarser renderer.
    pub force_no_unicode: bool,
}

impl TermEnv {
    pub fn detect() -> Self {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        Self {
            no_color: std::env::var_os("NO_COLOR").is_some(),
            colorterm: var("COLORTERM"),
            term: var("TERM"),
            wt_session: std::env::var_os("WT_SESSION").is_some(),
            conemu_ansi: var("ConEmuANSI"),
            term_program: var("TERM_PROGRAM"),
            windows: cfg!(target_os = "windows"),
            force_no_unicode: std::env::var_os("DSPI_NO_UNICODE").is_some(),
        }
    }
}

impl ColorDepth {
    pub fn detect() -> Self {
        Self::from_env(&TermEnv::detect())
    }

    /// Work out the colour depth without touching the environment.
    ///
    /// Windows needs its own path: it normally sets neither `TERM` nor
    /// `COLORTERM`, so the usual Unix sniffing would report 16 colours for
    /// Windows Terminal, which in fact does full truecolor.
    pub fn from_env(e: &TermEnv) -> Self {
        if e.no_color {
            return Self::Mono;
        }
        if e.term.as_deref() == Some("dumb") {
            return Self::Mono;
        }

        if matches!(e.colorterm.as_deref(), Some("truecolor" | "24bit")) {
            return Self::TrueColor;
        }

        if e.windows {
            // Windows Terminal and ConEmu both do truecolor; the classic
            // console does not.
            if e.wt_session || e.conemu_ansi.as_deref() == Some("ON") {
                return Self::TrueColor;
            }
            return Self::Ansi16;
        }

        // Some terminals advertise truecolor only through TERM_PROGRAM.
        if matches!(
            e.term_program.as_deref(),
            Some("iTerm.app" | "WezTerm" | "vscode" | "Apple_Terminal")
        ) && e.term_program.as_deref() != Some("Apple_Terminal")
        {
            return Self::TrueColor;
        }

        match e.term.as_deref() {
            Some(t) if t.contains("256") => Self::Ansi256,
            Some(_) => Self::Ansi16,
            None => Self::Ansi16,
        }
    }
}

/// Which glyphs are safe to draw with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyphs {
    /// 2x4 dots per cell, the smoothest curve a terminal can draw.
    Braille,
    /// Block elements: coarser vertically, but universally available.
    Blocks,
    /// Plain ASCII, for `TERM=dumb` and captured output.
    Ascii,
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub depth: ColorDepth,
    pub glyphs: Glyphs,

    pub bg: Color,
    /// Values, curves, meters: the things worth looking at.
    pub fg: Color,
    /// Borders and labels, deliberately quiet.
    pub chrome: Color,
    pub dim: Color,
    /// Focus, and nothing else.
    pub accent: Color,

    pub ok: Color,
    pub pending: Color,
    pub danger: Color,

    /// One hue per channel, reused everywhere that channel appears.
    pub channels: Vec<Color>,
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark(ColorDepth::detect(), Glyphs::Braille)
    }
}

impl Theme {
    pub fn dark(depth: ColorDepth, glyphs: Glyphs) -> Self {
        // Seventeen distinguishable hues is a real constraint at 256 colours and
        // with common colour-vision deficiencies, so the ramp deliberately walks
        // around the wheel rather than through it, and never relies on a
        // red/green distinction alone.
        let channels = match depth {
            ColorDepth::TrueColor => vec![
                Color::Rgb(0x4E, 0xC9, 0xF0), // inputs: cool
                Color::Rgb(0x6E, 0xB4, 0xF7),
                Color::Rgb(0x8E, 0xA6, 0xF5),
                Color::Rgb(0xAD, 0x9B, 0xEE),
                Color::Rgb(0xC7, 0x92, 0xE0),
                Color::Rgb(0xDD, 0x8C, 0xC8),
                Color::Rgb(0xEE, 0x8A, 0xAB),
                Color::Rgb(0xF7, 0x90, 0x8C),
                Color::Rgb(0xF5, 0xA6, 0x62), // outputs: warm
                Color::Rgb(0xE8, 0xBE, 0x4C),
                Color::Rgb(0xD2, 0xD4, 0x4A),
                Color::Rgb(0xB2, 0xE1, 0x5C),
                Color::Rgb(0x8B, 0xE4, 0x7C),
                Color::Rgb(0x63, 0xDF, 0x9F),
                Color::Rgb(0x46, 0xD5, 0xBF),
                Color::Rgb(0x3D, 0xC6, 0xD8),
                Color::Rgb(0x9A, 0xA8, 0xB8), // sub: neutral
            ],
            ColorDepth::Ansi256 => (0..17)
                .map(|i| {
                    Color::Indexed(
                        [
                            39, 74, 105, 141, 176, 175, 210, 209, 215, 221, 185, 149, 114, 79, 44,
                            45, 145,
                        ][i],
                    )
                })
                .collect(),
            ColorDepth::Ansi16 => (0..17)
                .map(|i| {
                    [
                        Color::Cyan,
                        Color::LightCyan,
                        Color::Blue,
                        Color::LightBlue,
                        Color::Magenta,
                        Color::LightMagenta,
                        Color::Red,
                        Color::LightRed,
                        Color::Yellow,
                        Color::LightYellow,
                        Color::Green,
                        Color::LightGreen,
                        Color::Cyan,
                        Color::LightCyan,
                        Color::Blue,
                        Color::LightBlue,
                        Color::White,
                    ][i]
                })
                .collect(),
            ColorDepth::Mono => vec![Color::Reset; 17],
        };

        let (fg, chrome, dim, accent, ok, pending, danger) = match depth {
            ColorDepth::Mono => (
                Color::Reset,
                Color::Reset,
                Color::Reset,
                Color::Reset,
                Color::Reset,
                Color::Reset,
                Color::Reset,
            ),
            ColorDepth::TrueColor => (
                Color::Rgb(0xE6, 0xEA, 0xF0),
                Color::Rgb(0x50, 0x59, 0x66),
                Color::Rgb(0x7A, 0x84, 0x92),
                Color::Rgb(0x5A, 0xC8, 0xFA),
                Color::Rgb(0x5E, 0xD1, 0x8E),
                Color::Rgb(0xE8, 0xA5, 0x3C),
                Color::Rgb(0xF2, 0x5D, 0x5D),
            ),
            _ => (
                Color::White,
                Color::DarkGray,
                Color::Gray,
                Color::LightCyan,
                Color::Green,
                Color::Yellow,
                Color::Red,
            ),
        };

        Self {
            depth,
            glyphs,
            bg: Color::Reset,
            fg,
            chrome,
            dim,
            accent,
            ok,
            pending,
            danger,
            channels,
        }
    }

    pub fn channel(&self, index: u8) -> Color {
        *self.channels.get(index as usize).unwrap_or(&self.fg)
    }

    pub fn chrome_style(&self) -> Style {
        Style::default().fg(self.chrome)
    }

    pub fn label(&self) -> Style {
        Style::default().fg(self.dim)
    }

    pub fn value(&self) -> Style {
        Style::default().fg(self.fg)
    }

    pub fn focused(&self) -> Style {
        Style::default()
            .fg(self.accent)
            .add_modifier(Modifier::BOLD)
    }

    /// In monochrome, channels are told apart by line pattern rather than hue,
    /// which also serves colour-blind users. Callers ask for the dash pattern
    /// instead of assuming colour will carry the distinction.
    pub fn needs_pattern_distinction(&self) -> bool {
        self.depth == ColorDepth::Mono
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_is_a_hue_for_every_channel_the_hardware_can_have() {
        for depth in [
            ColorDepth::TrueColor,
            ColorDepth::Ansi256,
            ColorDepth::Ansi16,
            ColorDepth::Mono,
        ] {
            let t = Theme::dark(depth, Glyphs::Braille);
            assert_eq!(
                t.channels.len(),
                17,
                "{depth:?} is short of channel colours"
            );
        }
    }

    #[test]
    fn truecolor_channel_hues_are_all_distinct() {
        let t = Theme::dark(ColorDepth::TrueColor, Glyphs::Braille);
        let mut seen = std::collections::HashSet::new();
        for c in &t.channels {
            assert!(
                seen.insert(format!("{c:?}")),
                "duplicate channel colour {c:?}"
            );
        }
    }

    #[test]
    fn an_out_of_range_channel_falls_back_rather_than_panicking() {
        let t = Theme::dark(ColorDepth::TrueColor, Glyphs::Braille);
        assert_eq!(t.channel(200), t.fg);
    }

    #[test]
    fn monochrome_asks_for_pattern_distinction_instead_of_colour() {
        assert!(Theme::dark(ColorDepth::Mono, Glyphs::Ascii).needs_pattern_distinction());
        assert!(!Theme::dark(ColorDepth::TrueColor, Glyphs::Braille).needs_pattern_distinction());
    }
}

#[cfg(test)]
mod detection_tests {
    use super::*;
    use crate::app::glyphs_for;

    fn env() -> TermEnv {
        TermEnv::default()
    }

    /// Windows sets neither TERM nor COLORTERM, so the usual Unix sniffing would
    /// report 16 colours for Windows Terminal, which does full truecolor.
    #[test]
    fn windows_terminal_gets_truecolor_and_braille() {
        let e = TermEnv {
            windows: true,
            wt_session: true,
            ..env()
        };
        assert_eq!(ColorDepth::from_env(&e), ColorDepth::TrueColor);
        assert_eq!(glyphs_for(&e), Glyphs::Braille);
    }

    /// The classic console renders braille as boxes in most fonts, so it gets
    /// blocks even though it can handle Unicode.
    #[test]
    fn the_windows_classic_console_gets_blocks() {
        let e = TermEnv {
            windows: true,
            ..env()
        };
        assert_eq!(ColorDepth::from_env(&e), ColorDepth::Ansi16);
        assert_eq!(glyphs_for(&e), Glyphs::Blocks);
    }

    #[test]
    fn conemu_is_treated_like_windows_terminal() {
        let e = TermEnv {
            windows: true,
            conemu_ansi: Some("ON".into()),
            ..env()
        };
        assert_eq!(ColorDepth::from_env(&e), ColorDepth::TrueColor);
        assert_eq!(glyphs_for(&e), Glyphs::Braille);
    }

    #[test]
    fn colorterm_wins_everywhere() {
        for windows in [true, false] {
            let e = TermEnv {
                windows,
                colorterm: Some("truecolor".into()),
                ..env()
            };
            assert_eq!(ColorDepth::from_env(&e), ColorDepth::TrueColor);
        }
    }

    #[test]
    fn unix_terminals_are_read_from_term() {
        let e = TermEnv {
            term: Some("xterm-256color".into()),
            ..env()
        };
        assert_eq!(ColorDepth::from_env(&e), ColorDepth::Ansi256);

        let e = TermEnv {
            term: Some("xterm".into()),
            ..env()
        };
        assert_eq!(ColorDepth::from_env(&e), ColorDepth::Ansi16);
    }

    /// NO_COLOR is a cross-ecosystem convention and overrides everything.
    #[test]
    fn no_color_beats_every_other_signal() {
        let e = TermEnv {
            no_color: true,
            colorterm: Some("truecolor".into()),
            wt_session: true,
            windows: true,
            ..env()
        };
        assert_eq!(ColorDepth::from_env(&e), ColorDepth::Mono);
    }

    #[test]
    fn a_dumb_terminal_gets_no_colour_and_plain_ascii() {
        let e = TermEnv {
            term: Some("dumb".into()),
            colorterm: Some("truecolor".into()),
            ..env()
        };
        assert_eq!(ColorDepth::from_env(&e), ColorDepth::Mono);
        assert_eq!(glyphs_for(&e), Glyphs::Ascii);
    }

    #[test]
    fn the_user_can_force_the_coarser_renderer() {
        let e = TermEnv {
            force_no_unicode: true,
            colorterm: Some("truecolor".into()),
            ..env()
        };
        assert_eq!(glyphs_for(&e), Glyphs::Blocks);
    }

    /// A bare Linux console or a Raspberry Pi over SSH with no TERM should still
    /// render, just conservatively.
    #[test]
    fn an_unknown_environment_still_works() {
        let e = env();
        assert_eq!(ColorDepth::from_env(&e), ColorDepth::Ansi16);
        assert_eq!(glyphs_for(&e), Glyphs::Braille);
    }
}
