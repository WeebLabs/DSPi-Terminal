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
    /// Unsaved, phase invert, conflict, and every banner. `pending` is the
    /// same colour under its older name; both are kept so a screen can say
    /// which meaning it intends.
    pub pending: Color,
    pub warning: Color,
    pub danger: Color,
    /// Minor grid lines and the unfilled part of a meter: below chrome.
    pub chrome_faint: Color,

    /// One hue per channel in the RP2350 order (eight inputs, eight outputs,
    /// the sub). Use [`Theme::role_color`] when the topology is known.
    pub channels: Vec<Color>,
    /// The Console's input, output and sub colours by role, so an RP2040's
    /// first output gets the first output colour rather than the third
    /// input's.
    pub inputs: Vec<Color>,
    pub outputs: Vec<Color>,
    pub sub: Color,
}

/// Which channel a colour is for, in the Console's terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelRole {
    Input(u8),
    Output(u8),
    Sub,
}

impl ChannelRole {
    /// The role of a unified-space channel index, given the discovered
    /// topology. The sub is always the last channel.
    pub fn of(index: u8, num_inputs: u8, num_outputs: u8) -> Self {
        let total = num_inputs + num_outputs;
        if index < num_inputs {
            Self::Input(index)
        } else if num_outputs > 0 && index == total - 1 {
            Self::Sub
        } else {
            Self::Output(index - num_inputs)
        }
    }

    /// The Console's descriptor: `IN1`..`IN8`, `OUT1`..`OUT9`.
    pub fn descriptor(self, num_outputs: u8) -> String {
        match self {
            Self::Input(i) => format!("IN{}", i + 1),
            Self::Output(o) => format!("OUT{}", o + 1),
            Self::Sub => format!("OUT{num_outputs}"),
        }
    }
}

/// Which palette to draw with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Palette {
    /// The DSPi Console's own colours, so a channel looks the same here as it
    /// does there. The default.
    Console,
    /// Amber phosphor, after the monitors this kind of instrument used to be
    /// driven from.
    Amber,
    /// A conventional dark theme, for anyone who would rather have hues.
    Dark,
    /// No colour at all, for a monochrome terminal or a captured log.
    Mono,
}

impl Palette {
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "console" => Some(Self::Console),
            "amber" => Some(Self::Amber),
            "dark" => Some(Self::Dark),
            "mono" => Some(Self::Mono),
            _ => None,
        }
    }

    pub const NAMES: &'static [&'static str] = &["console", "amber", "dark", "mono"];
}

impl Default for Theme {
    fn default() -> Self {
        Self::new(Palette::Console, ColorDepth::detect(), Glyphs::Braille)
    }
}

/// A truecolor value with its hand-chosen 256-colour and 16-colour stand-ins.
///
/// The 256 index is the nearest cube entry unless that would make two
/// channels that can share a graph indistinguishable, in which case the
/// design document records the step taken.
#[derive(Debug, Clone, Copy)]
struct Swatch {
    rgb: (u8, u8, u8),
    indexed: u8,
    ansi: Color,
}

impl Swatch {
    const fn new(rgb: (u8, u8, u8), indexed: u8, ansi: Color) -> Self {
        Self { rgb, indexed, ansi }
    }

    fn at(self, depth: ColorDepth) -> Color {
        match depth {
            ColorDepth::TrueColor => Color::Rgb(self.rgb.0, self.rgb.1, self.rgb.2),
            ColorDepth::Ansi256 => Color::Indexed(self.indexed),
            ColorDepth::Ansi16 => self.ansi,
            ColorDepth::Mono => Color::Reset,
        }
    }
}

/// `ChannelPalette.inputs` in the Console, FL FR FC LFE BL BR SL SR.
const CONSOLE_INPUTS: [Swatch; 8] = [
    Swatch::new((0x4A, 0x8F, 0xE3), 68, Color::Blue),
    Swatch::new((0xF5, 0x73, 0x73), 203, Color::Red),
    Swatch::new((0x73, 0xC7, 0x8C), 78, Color::Green),
    Swatch::new((0xED, 0xB3, 0x4D), 215, Color::Yellow),
    Swatch::new((0x99, 0x8C, 0xEB), 104, Color::Magenta),
    Swatch::new((0xE6, 0x8C, 0xC7), 176, Color::LightMagenta),
    Swatch::new((0x66, 0xC7, 0xD1), 80, Color::Cyan),
    Swatch::new((0xCC, 0xB8, 0x6B), 179, Color::LightYellow),
];

/// `ChannelPalette.outputs` in the Console: the four stereo pairs.
const CONSOLE_OUTPUTS: [Swatch; 8] = [
    Swatch::new((0x45, 0xC2, 0xA3), 79, Color::Cyan),
    Swatch::new((0x59, 0xD1, 0x80), 84, Color::LightGreen),
    Swatch::new((0xF0, 0xC4, 0x59), 221, Color::LightYellow),
    Swatch::new((0xF2, 0xA6, 0x4D), 209, Color::LightRed),
    Swatch::new((0x59, 0x8C, 0xF2), 33, Color::LightBlue),
    Swatch::new((0x8C, 0xB3, 0xF2), 111, Color::LightCyan),
    Swatch::new((0xD9, 0x73, 0x8C), 168, Color::Magenta),
    Swatch::new((0xF2, 0x99, 0xA6), 217, Color::LightMagenta),
];

/// `ChannelPalette.pdm`: the mono subwoofer.
const CONSOLE_SUB: Swatch = Swatch::new((0xBA, 0x87, 0xF2), 141, Color::Magenta);

const CONSOLE_ACCENT: Swatch = Swatch::new((0x3A, 0x8D, 0xFF), 75, Color::LightBlue);
const CONSOLE_DANGER: Swatch = Swatch::new((0xF5, 0x45, 0x3C), 203, Color::LightRed);
const CONSOLE_WARNING: Swatch = Swatch::new((0xF0, 0xA0, 0x30), 214, Color::Yellow);
const CONSOLE_OK: Swatch = Swatch::new((0x5A, 0xC2, 0x6B), 78, Color::Green);
const CONSOLE_FG: Swatch = Swatch::new((0xE6, 0xE6, 0xE6), 252, Color::White);
const CONSOLE_DIM: Swatch = Swatch::new((0x8C, 0x8C, 0x8C), 245, Color::DarkGray);
const CONSOLE_CHROME: Swatch = Swatch::new((0x4A, 0x4A, 0x4A), 238, Color::DarkGray);
const CONSOLE_CHROME_FAINT: Swatch = Swatch::new((0x33, 0x33, 0x33), 236, Color::Black);

impl Theme {
    pub fn new(palette: Palette, depth: ColorDepth, glyphs: Glyphs) -> Self {
        match palette {
            Palette::Console => Self::console(depth, glyphs),
            Palette::Amber => Self::amber(depth, glyphs),
            Palette::Dark => Self::dark(depth, glyphs),
            Palette::Mono => Self::mono(glyphs),
        }
    }

    /// The Console's palette, quantised for poorer terminals.
    ///
    /// The channel colours are `ChannelPalette.swift` verbatim at truecolor.
    /// The 256-colour indices are the nearest cube entries except where two
    /// channels that can share a graph landed on one index (FC with the second
    /// output, LFE with the fourth), which were moved a step. At sixteen
    /// colours hues repeat and the graph labels its curves instead.
    pub fn console(depth: ColorDepth, glyphs: Glyphs) -> Self {
        let inputs: Vec<Color> = CONSOLE_INPUTS.iter().map(|s| s.at(depth)).collect();
        let outputs: Vec<Color> = CONSOLE_OUTPUTS.iter().map(|s| s.at(depth)).collect();
        let sub = CONSOLE_SUB.at(depth);
        let channels = inputs
            .iter()
            .chain(outputs.iter())
            .copied()
            .chain(std::iter::once(sub))
            .collect();
        Self {
            depth,
            glyphs,
            palette: Palette::Console,
            bg: Color::Reset,
            fg: CONSOLE_FG.at(depth),
            chrome: CONSOLE_CHROME.at(depth),
            chrome_faint: CONSOLE_CHROME_FAINT.at(depth),
            dim: CONSOLE_DIM.at(depth),
            accent: CONSOLE_ACCENT.at(depth),
            ok: CONSOLE_OK.at(depth),
            pending: CONSOLE_WARNING.at(depth),
            warning: CONSOLE_WARNING.at(depth),
            danger: CONSOLE_DANGER.at(depth),
            channels,
            inputs,
            outputs,
            sub,
        }
    }

    /// No colour, whatever the terminal could do. Everything that colour
    /// would have said is said with reverse video, bold, and patterns.
    pub fn mono(glyphs: Glyphs) -> Self {
        let mut t = Self::console(ColorDepth::Mono, glyphs);
        t.palette = Palette::Mono;
        t
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

        Self::from_ramp(
            Palette::Amber,
            depth,
            glyphs,
            [fg, chrome, dim, accent, ok, pending, danger],
            channels,
        )
    }

    /// Build a theme from a flat 17-entry channel ramp, deriving the by-role
    /// tables from the RP2350 order the ramp is written in.
    #[allow(clippy::too_many_arguments)]
    fn from_ramp(
        palette: Palette,
        depth: ColorDepth,
        glyphs: Glyphs,
        [fg, chrome, dim, accent, ok, pending, danger]: [Color; 7],
        channels: Vec<Color>,
    ) -> Self {
        let inputs = channels[..8].to_vec();
        let outputs = channels[8..16].to_vec();
        let sub = channels[16];
        Self {
            depth,
            glyphs,
            palette,
            bg: Color::Reset,
            fg,
            chrome,
            chrome_faint: chrome,
            dim,
            accent,
            ok,
            pending,
            warning: pending,
            danger,
            channels,
            inputs,
            outputs,
            sub,
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

        Self::from_ramp(
            Palette::Dark,
            depth,
            glyphs,
            [fg, chrome, dim, accent, ok, pending, danger],
            channels,
        )
    }

    /// The colour of a channel by its RP2350-order index. Prefer
    /// [`Theme::channel_of`] when the topology is known.
    pub fn channel(&self, index: u8) -> Color {
        *self.channels.get(index as usize).unwrap_or(&self.fg)
    }

    /// The colour for a channel role, the way the Console assigns it.
    pub fn role_color(&self, role: ChannelRole) -> Color {
        match role {
            ChannelRole::Input(i) => *self.inputs.get(i as usize).unwrap_or(&self.accent),
            ChannelRole::Output(o) => *self.outputs.get(o as usize).unwrap_or(&self.accent),
            ChannelRole::Sub => self.sub,
        }
    }

    /// The colour of a unified-space channel on a device with this topology.
    pub fn channel_of(&self, index: u8, num_inputs: u8, num_outputs: u8) -> Color {
        self.role_color(ChannelRole::of(index, num_inputs, num_outputs))
    }

    /// Warning text: unsaved, inverted, conflicting.
    pub fn warning_style(&self) -> Style {
        Style::default().fg(self.warning)
    }

    /// A status pill or a selected chip: reverse video in the given colour,
    /// which survives every depth including none.
    pub fn pill(&self, color: Color) -> Style {
        Style::default().fg(color).add_modifier(Modifier::REVERSED)
    }

    /// A section header: the Console's small uppercase label.
    pub fn section(&self) -> Style {
        Style::default().fg(self.dim)
    }

    /// A panel title.
    pub fn title(&self) -> Style {
        Style::default().fg(self.fg).add_modifier(Modifier::BOLD)
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
            Palette::Dark => Style::default().fg(self.danger),
            _ => Style::default()
                .fg(self.danger)
                .add_modifier(Modifier::REVERSED),
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
    fn the_console_palette_is_what_you_get_by_default() {
        assert_eq!(Theme::default().palette, Palette::Console);
    }

    #[test]
    fn themes_are_selectable_by_name() {
        assert_eq!(Palette::parse("amber"), Some(Palette::Amber));
        assert_eq!(Palette::parse("Dark"), Some(Palette::Dark));
        assert_eq!(Palette::parse("console"), Some(Palette::Console));
        assert_eq!(Palette::parse("mono"), Some(Palette::Mono));
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

#[cfg(test)]
mod console_tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn the_channel_colours_are_the_consoles_own() {
        // ChannelPalette.swift: FL is (0.29, 0.56, 0.89), the sub is
        // (0.73, 0.53, 0.95). Rounded to bytes.
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        assert_eq!(
            t.role_color(ChannelRole::Input(0)),
            Color::Rgb(0x4A, 0x8F, 0xE3)
        );
        assert_eq!(t.role_color(ChannelRole::Sub), Color::Rgb(0xBA, 0x87, 0xF2));
    }

    #[test]
    fn no_two_channels_share_a_256_colour_index() {
        let t = Theme::console(ColorDepth::Ansi256, Glyphs::Braille);
        let mut seen = HashSet::new();
        for c in &t.channels {
            assert!(seen.insert(*c), "duplicate indexed colour {c:?}");
        }
        assert_eq!(seen.len(), 17);
    }

    #[test]
    fn roles_follow_the_discovered_topology() {
        // An RP2040 has two inputs, four outputs and the sub: seven channels.
        assert_eq!(ChannelRole::of(0, 2, 5), ChannelRole::Input(0));
        assert_eq!(ChannelRole::of(2, 2, 5), ChannelRole::Output(0));
        assert_eq!(ChannelRole::of(6, 2, 5), ChannelRole::Sub);
        // An RP2350: eight and nine.
        assert_eq!(ChannelRole::of(8, 8, 9), ChannelRole::Output(0));
        assert_eq!(ChannelRole::of(16, 8, 9), ChannelRole::Sub);
        assert_eq!(ChannelRole::Sub.descriptor(5), "OUT5");
        assert_eq!(ChannelRole::Sub.descriptor(9), "OUT9");
        assert_eq!(ChannelRole::Input(7).descriptor(9), "IN8");
    }

    #[test]
    fn an_rp2040s_first_output_wears_the_first_output_colour() {
        let t = Theme::console(ColorDepth::TrueColor, Glyphs::Braille);
        assert_eq!(t.channel_of(2, 2, 5), t.outputs[0]);
        assert_ne!(t.channel_of(2, 2, 5), t.channel(2));
    }

    #[test]
    fn mono_has_no_colour_anywhere() {
        let t = Theme::mono(Glyphs::Ascii);
        assert!(t.channels.iter().all(|c| *c == Color::Reset));
        assert_eq!(t.accent, Color::Reset);
        assert!(t.needs_pattern_distinction());
        assert!(t.pill(t.accent).add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn the_semantic_colours_stay_apart_from_the_channels_at_256() {
        // Danger shares an index with FR (both the Console's red), which is
        // acceptable because a clip zone is never adjacent to a curve. The
        // accent must not collide with any channel, since it marks focus on
        // rows that also carry channel colour.
        let t = Theme::console(ColorDepth::Ansi256, Glyphs::Braille);
        assert!(!t.channels.contains(&t.accent));
        assert!(!t.channels.contains(&t.warning));
    }
}
