//! How hard to drive the terminal, and which glyphs it can draw.

use std::time::Duration;

use crate::theme::Glyphs;

/// How hard the interface is allowed to work.
///
/// A Raspberry Pi reached over SSH pays for every repaint in bytes on the wire,
/// not in processor time, so the useful lever is how often the screen changes
/// rather than how fast it can be drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Performance {
    /// How often to poll the device's meters.
    pub meter_interval: Duration,
    /// How long to wait for a keypress before redrawing.
    pub event_timeout: Duration,
    pub animate: bool,
}

impl Default for Performance {
    fn default() -> Self {
        Self {
            meter_interval: Duration::from_millis(50),
            event_timeout: Duration::from_millis(30),
            animate: true,
        }
    }
}

impl Performance {
    /// The conservative profile: meters at 10 Hz and no animation.
    pub fn lite() -> Self {
        Self {
            meter_interval: Duration::from_millis(100),
            event_timeout: Duration::from_millis(100),
            animate: false,
        }
    }

    /// Pick a profile from the machine and the connection.
    ///
    /// A single-core part or an ARMv6 (a Pi Zero or an original Pi) gets the
    /// lite profile, as does any session reached over SSH, where the cost of a
    /// repaint is the link rather than the processor.
    pub fn detect() -> Self {
        if is_low_powered() || over_ssh() {
            Self::lite()
        } else {
            Self::default()
        }
    }
}

/// Choose glyphs the terminal can actually render.
pub fn detect_glyphs() -> Glyphs {
    glyphs_for(&crate::theme::TermEnv::detect())
}

/// Pick a glyph set without touching the environment.
///
/// The Windows classic console renders braille as boxes in most fonts, so it
/// gets blocks even though it is perfectly capable of Unicode. Windows Terminal
/// handles braille correctly and is detected separately.
pub fn glyphs_for(e: &crate::theme::TermEnv) -> Glyphs {
    if e.term.as_deref() == Some("dumb") {
        return Glyphs::Ascii;
    }
    if e.force_no_unicode {
        return Glyphs::Blocks;
    }
    // The Linux virtual console's fonts carry box drawing and the shade
    // blocks but no braille, which would draw the curve as question marks.
    if e.term.as_deref() == Some("linux") {
        return Glyphs::Blocks;
    }
    if e.windows && !e.wt_session && e.conemu_ansi.as_deref() != Some("ON") {
        return Glyphs::Blocks;
    }
    Glyphs::Braille
}

/// True for a single-core machine or an ARMv6 part.
fn is_low_powered() -> bool {
    if std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        <= 1
    {
        return true;
    }
    // Only Linux exposes this, and it is the only place a Pi Zero appears.
    std::fs::read_to_string("/proc/cpuinfo")
        .map(|s| s.contains("ARMv6"))
        .unwrap_or(false)
}

fn over_ssh() -> bool {
    std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some()
}
