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
    pub palette: Palette,

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

/// Which palette to draw with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Palette {
    /// Amber phosphor, after the monitors this kind of instrument used to be
    /// driven from. The default.
    Amber,
    /// A conventional dark theme, for anyone who would rather have hues.
    Dark,
}

impl Palette {
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "amber" => Some(Self::Amber),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    pub const NAMES: &'static [&'static str] = &["amber", "dark"];
}

impl Default for Theme {
    fn default() -> Self {
        Self::new(Palette::Amber, ColorDepth::detect(), Glyphs::Braille)
    }
}

impl Theme {
    pub fn new(palette: Palette, depth: ColorDepth, glyphs: Glyphs) -> Self {
        match palette {
            Palette::Amber => Self::amber(depth, glyphs),
            Palette::Dark => Self::dark(depth, glyphs),
        }
    }

    /// Amber, in the orange-to-red half of it.
    ///
    /// Not a strict phosphor simulation. A true amber tube sits around yellow-
    /// gold, which reads green next to anything else on a modern display, so the
    /// palette is pulled into orange and red where "amber" actually lives in
    /// most people's heads. The channel ramp runs from light orange to deep
    /// red-amber: enough spread to tell seventeen traces apart, narrow enough
    /// that the screen still reads as one thing rather than a rainbow.
    ///
    /// Text is deliberately lighter than the ramp. A monitor would have drawn
    /// everything in the one phosphor; here the values want to be legible more
    /// than they want to be authentic.
    ///
    /// Emphasis is brightness and reverse video, as it was on the hardware,
    /// rather than a second colour.
    pub fn amber(depth: ColorDepth, glyphs: Glyphs) -> Self {
        let channels = match depth {
            ColorDepth::TrueColor => (0..17)
                .map(|i| {
                    // Light orange to deep red-amber. Green stays well under
                    // red throughout, which is what keeps it out of the yellow
                    // and gold that read as green on screen.
                    let t = i as f32 / 16.0;
                    let r = 255.0 - 41.0 * t;
                    let g = 176.0 - 118.0 * t;
                    let b = 96.0 - 64.0 * t;
                    Color::Rgb(r as u8, g as u8, b as u8)
                })
                .collect(),
            // The 256-colour cube's amber and orange run, light to dark.
            // The cube's orange and red run, light to dark, avoiding the
            // yellows entirely.
            ColorDepth::Ansi256 => (0..17)
                .map(|i| {
                    Color::Indexed(
                        // Walked down the cube from light orange to dark red,
                        // every entry checked to resolve with red leading and
                        // green well clear of it. Chosen by hand, one of these
                        // was magenta.
                        [
                            216, 215, 214, 209, 208, 203, 202, 179, 173, 172, 167, 166, 160, 131,
                            130, 124, 94,
                        ][i],
                    )
                })
                .collect(),
            ColorDepth::Ansi16 => (0..17)
                .map(|i| {
                    if i % 2 == 0 {
                        Color::Yellow
                    } else {
                        Color::LightYellow
                    }
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
                // Text is a light warm tint rather than the ramp's orange, so
                // values stay easy to read against the chrome.
                Color::Rgb(0xFF, 0xDC, 0xC4),
                // Chrome is a deep burnt orange, far enough below the text that
                // the eye lands on the data and not the borders.
                Color::Rgb(0x6E, 0x30, 0x18),
                Color::Rgb(0xD1, 0x7A, 0x4E),
                Color::Rgb(0xFF, 0xEC, 0xD6),
                Color::Rgb(0xFF, 0x9E, 0x5A),
                Color::Rgb(0xFF, 0xB0, 0x74),
                // The hottest point, blooming toward white without reaching it.
                Color::Rgb(0xFF, 0xE6, 0xD0),
            ),
            // The 256-colour cube can express this palette perfectly well, so it
            // gets real colours rather than the crude ANSI names. Apple Terminal
            // has no truecolor at all, which makes this the branch a good number
            // of people actually see.
            ColorDepth::Ansi256 => (
                Color::Indexed(223), // 255,215,175 warm cream text
                Color::Indexed(130), // 175,95,0 burnt orange chrome
                Color::Indexed(173), // 215,135,95 mid orange labels
                Color::Indexed(224), // 255,215,215 the hottest highlight
                Color::Indexed(208), // 255,135,0
                Color::Indexed(215), // 255,175,95
                Color::Indexed(224),
            ),
            // Sixteen colours has no orange at all; yellow is the closest warm
            // tone available, so the character survives even if the hue cannot.
            _ => (
                Color::Yellow,
                Color::DarkGray,
                Color::Yellow,
                Color::LightYellow,
                Color::Yellow,
                Color::LightYellow,
                Color::LightYellow,
            ),
        };

        Self {
            depth,
            glyphs,
            palette: Palette::Amber,
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
            palette: Palette::Dark,
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

    /// A field that is armed: the next keystroke changes the device.
    ///
    /// Reverse video rather than a colour, because "this one is live" has to
    /// read at a glance on a monochrome terminal too, and the surrounding cells
    /// already use colour to mean focus.
    pub fn editing(&self) -> Style {
        Style::default()
            .fg(self.accent)
            .add_modifier(Modifier::REVERSED | Modifier::BOLD)
    }

    /// In monochrome, channels are told apart by line pattern rather than hue,
    /// which also serves colour-blind users. Callers ask for the dash pattern
    /// instead of assuming colour will carry the distinction.
    pub fn needs_pattern_distinction(&self) -> bool {
        self.depth == ColorDepth::Mono
    }

    /// How to draw something that must not be missed, such as a clip.
    ///
    /// A single-phosphor screen had no second colour to reach for, so it used
    /// reverse video. That reads as deliberate rather than as a limitation, and
    /// it survives every colour depth including none.
    pub fn alarm(&self) -> Style {
        match self.palette {
            Palette::Amber => Style::default()
                .fg(self.danger)
                .add_modifier(Modifier::REVERSED),
            Palette::Dark => Style::default().fg(self.danger),
        }
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

#[cfg(test)]
mod amber_tests {
    use super::*;
    use ratatui::style::Color;

    fn rgb(c: Color) -> (u8, u8, u8) {
        match c {
            Color::Rgb(r, g, b) => (r, g, b),
            other => panic!("expected an RGB colour, got {other:?}"),
        }
    }

    fn amber() -> Theme {
        Theme::amber(ColorDepth::TrueColor, Glyphs::Braille)
    }

    /// Everything on screen stays warm, with red leading. A colour where green
    /// catches up reads as yellow or gold, which next to anything else looks
    /// green, and that is exactly what this palette is not.
    #[test]
    fn every_colour_is_warm_with_red_leading() {
        let t = amber();
        let mut all = vec![t.fg, t.chrome, t.dim, t.accent, t.ok, t.pending, t.danger];
        all.extend(t.channels.iter().copied());

        for c in all {
            let (r, g, b) = rgb(c);
            assert!(r > g, "{c:?}: red must lead green");
            assert!(g >= b, "{c:?}: green must lead blue");
            assert!(
                r as i32 - b as i32 >= 40,
                "{c:?} is too close to grey to read as amber"
            );
        }
    }

    /// The channel ramp is the part most likely to drift toward gold, so it
    /// carries a tighter rule than the chrome does.
    #[test]
    fn the_channel_ramp_is_orange_not_gold() {
        for c in amber().channels {
            let (r, g, _) = rgb(c);
            assert!(
                r as i32 - g as i32 >= 70,
                "{c:?} has too much green: that reads as yellow, not orange"
            );
        }
    }

    /// Data bright, chrome dim. If the borders are as loud as the values, the
    /// eye lands in the wrong place.
    #[test]
    fn chrome_sits_well_below_the_data() {
        let t = amber();
        let lum = |c: Color| {
            let (r, g, b) = rgb(c);
            0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32
        };
        assert!(lum(t.chrome) < lum(t.dim), "chrome should be below labels");
        assert!(lum(t.dim) < lum(t.fg), "labels should be below values");
        assert!(lum(t.fg) < lum(t.accent), "focus should be the brightest");
    }

    /// Text is lighter than the ramp on purpose: legibility beat authenticity
    /// here, and a real single-phosphor screen is not the goal.
    #[test]
    fn text_is_lighter_than_the_channel_colours() {
        let t = amber();
        let lum = |c: Color| {
            let (r, g, b) = rgb(c);
            0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32
        };
        for c in &t.channels {
            assert!(
                lum(t.fg) > lum(*c),
                "text should sit above the ramp, but {c:?} is brighter"
            );
        }
    }

    /// Seventeen traces cannot be told apart by brightness alone, so the ramp
    /// walks a narrow band inside the amber range.
    #[test]
    fn channels_are_distinguishable_without_leaving_the_range() {
        let t = amber();
        let mut seen = std::collections::HashSet::new();
        for c in &t.channels {
            assert!(seen.insert(rgb(*c)), "duplicate channel colour {c:?}");
        }
        assert_eq!(t.channels.len(), 17);

        // The ends of the ramp must actually differ, or the middle is wasted.
        let (_, g0, b0) = rgb(t.channels[0]);
        let (_, g16, b16) = rgb(t.channels[16]);
        assert!(g0 > g16 + 60, "the ramp barely moves: {g0} to {g16}");
        assert!(b0 > b16, "{b0} to {b16}");
    }

    /// A single-phosphor screen had no second colour for emphasis, so it used
    /// reverse video. That reads as deliberate rather than as a shortage.
    #[test]
    fn an_alarm_uses_reverse_video_on_amber() {
        assert!(amber().alarm().add_modifier.contains(Modifier::REVERSED));
        // The conventional theme has hues to spare and does not need it.
        assert!(
            !Theme::dark(ColorDepth::TrueColor, Glyphs::Braille)
                .alarm()
                .add_modifier
                .contains(Modifier::REVERSED)
        );
    }

    #[test]
    fn the_palette_survives_a_poorer_terminal() {
        for depth in [ColorDepth::Ansi256, ColorDepth::Ansi16, ColorDepth::Mono] {
            let t = Theme::amber(depth, Glyphs::Blocks);
            assert_eq!(t.channels.len(), 17, "{depth:?} lost channel colours");
            assert_eq!(t.palette, Palette::Amber);
        }
    }

    #[test]
    fn amber_is_what_you_get_by_default() {
        assert_eq!(Theme::default().palette, Palette::Amber);
    }

    #[test]
    fn themes_are_selectable_by_name() {
        assert_eq!(Palette::parse("amber"), Some(Palette::Amber));
        assert_eq!(Palette::parse("Dark"), Some(Palette::Dark));
        assert_eq!(Palette::parse("chartreuse"), None);
    }
}

#[cfg(test)]
mod indexed_palette_tests {
    use super::*;
    use ratatui::style::Color;

    /// Resolve an xterm-256 index to its RGB value.
    ///
    /// Indices 16-231 are a 6x6x6 cube; 232-255 are a grey ramp. Without this
    /// the indexed palette cannot be checked against the same rules as the
    /// truecolor one, which is how it drifted in the first place.
    fn resolve(c: Color) -> (u8, u8, u8) {
        const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
        match c {
            Color::Rgb(r, g, b) => (r, g, b),
            Color::Indexed(i) if (16..232).contains(&i) => {
                let n = i - 16;
                (
                    LEVELS[(n / 36) as usize],
                    LEVELS[((n % 36) / 6) as usize],
                    LEVELS[(n % 6) as usize],
                )
            }
            Color::Indexed(i) if i >= 232 => {
                let v = 8 + 10 * (i - 232);
                (v, v, v)
            }
            other => panic!("cannot resolve {other:?}"),
        }
    }

    fn indexed() -> Theme {
        Theme::amber(ColorDepth::Ansi256, Glyphs::Braille)
    }

    /// The failure this whole module exists to catch: the truecolor palette was
    /// tuned while the 256-colour one, which is what Apple Terminal and plenty
    /// of others actually get, was left on plain ANSI yellow and grey.
    #[test]
    fn the_indexed_palette_is_warm_like_the_truecolor_one() {
        let t = indexed();
        let mut all = vec![t.fg, t.chrome, t.dim, t.accent, t.ok, t.pending, t.danger];
        all.extend(t.channels.iter().copied());

        for c in all {
            let (r, g, b) = resolve(c);
            assert!(r > g, "{c:?} resolves to ({r},{g},{b}): red must lead");
            assert!(
                g >= b,
                "{c:?} resolves to ({r},{g},{b}): green must lead blue"
            );
        }
    }

    #[test]
    fn no_plain_ansi_names_survive_at_256_colours() {
        let t = indexed();
        for c in [t.fg, t.chrome, t.dim, t.accent, t.ok, t.pending, t.danger] {
            assert!(
                matches!(c, Color::Indexed(_)),
                "{c:?} is a crude ANSI name where the cube was available"
            );
        }
    }

    #[test]
    fn the_indexed_ramp_is_orange_not_gold() {
        for c in indexed().channels {
            let (r, g, _) = resolve(c);
            assert!(
                r as i32 - g as i32 >= 40,
                "{c:?} resolves to ({r},{g},..): too much green for orange"
            );
        }
    }

    #[test]
    fn indexed_text_still_sits_above_the_chrome() {
        let t = indexed();
        let lum = |c: Color| {
            let (r, g, b) = resolve(c);
            0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32
        };
        assert!(lum(t.chrome) < lum(t.dim), "chrome should be below labels");
        assert!(lum(t.dim) < lum(t.fg), "labels should be below text");
    }

    /// Sixteen colours has no orange, so yellow is the honest fallback; the rule
    /// is only that nothing goes cold.
    #[test]
    fn sixteen_colours_stays_warm_even_without_orange() {
        let t = Theme::amber(ColorDepth::Ansi16, Glyphs::Blocks);
        for c in [t.fg, t.dim, t.accent] {
            assert!(
                matches!(c, Color::Yellow | Color::LightYellow),
                "{c:?} is not a warm tone"
            );
        }
    }
}
