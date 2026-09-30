//! The app-side settings file.
//!
//! Everything the Console keeps in `AppSettings` (its `@AppStorage` defaults)
//! rather than on the device: the Graphing page and the volume-mode choice.
//! The device holds none of this, so it lives in a file of our own at
//! `~/.config/dspi/config.toml` (or the platform equivalent `dirs` reports).
//!
//! Reading is total: a missing file, an unreadable one or a malformed one all
//! give the defaults, because a settings file is never worth refusing to start
//! over. Unknown keys are ignored and known keys are optional, so a file
//! written by a newer build still loads here.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::graph::{GraphSettings, GridStrength};

/// Settings > Graphing, with the Console's defaults.
///
/// `line_glow` and `animation_speed` have no terminal meaning today (there is
/// no glow to draw and the curve draw-in is fixed), but they are carried so
/// the file round-trips and a later phase can honour them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Graphing {
    pub line_glow: bool,
    pub show_phase: bool,
    pub unwrap_phase: bool,
    pub line_width: f64,
    pub animation_speed: f64,
    pub freq_grid: bool,
    pub freq_labels: bool,
    pub db_grid: bool,
    pub db_labels: bool,
    /// Show Frequency Readout and Show Gain Readout
    /// (`DSPi_ConsoleApp.swift:54-55`, both on by default).
    pub freq_readout: bool,
    pub gain_readout: bool,
    /// Grid Opacity, in the three steps [`GridStrength`] describes.
    pub grid: GridStrength,
    /// The dashboard's cards per row: 0 is Auto, else 1 to 3
    /// (`DashboardView.swift:507-560`).
    pub dashboard_cards: u8,
    pub db_range: f64,
    pub db_center: f64,
    pub min_hz: f64,
    pub max_hz: f64,
    pub popout_follows_selection: bool,
}

impl Default for Graphing {
    fn default() -> Self {
        Self {
            line_glow: false,
            show_phase: false,
            unwrap_phase: false,
            line_width: 2.0,
            animation_speed: 0.2,
            freq_grid: true,
            freq_labels: true,
            db_grid: true,
            db_labels: true,
            freq_readout: true,
            gain_readout: true,
            grid: GridStrength::Dim,
            dashboard_cards: 0,
            db_range: 50.0,
            db_center: 0.0,
            min_hz: 15.0,
            max_hz: 20_000.0,
            popout_follows_selection: true,
        }
    }
}

/// Settings > Display > Spectrum Analyser, with the Console's defaults
/// (`DSPi_ConsoleApp.swift:107-121`).
///
/// The last three belong to the device, but the analyser forgets them at every
/// power cycle, so this file is the only place they can live; the runner
/// pushes them on connect. The first five are how the Terminal draws.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Spectrum {
    /// 30..100 %, in steps of 5. A terminal cell has no opacity, so below
    /// [`Spectrum::DIM_BELOW`] the curves are drawn dim and otherwise at full
    /// strength.
    pub strength_pct: u8,
    pub peak_hold: bool,
    pub smoothing: bool,
    /// The dB axis: -60, -90 or -120 dBFS at the bottom...
    pub floor_db: i32,
    /// ...and 0, +6 or +12 dBFS at the top.
    pub ceiling_db: i32,
    /// The transform size as an FFT order; `None` is the device's default.
    pub transform_order: Option<u8>,
    pub averaging_ms: u16,
    pub peak_decay_db_s: u8,
}

impl Default for Spectrum {
    fn default() -> Self {
        Self {
            strength_pct: 100,
            peak_hold: true,
            smoothing: true,
            floor_db: -90,
            ceiling_db: 6,
            transform_order: None,
            averaging_ms: 300,
            peak_decay_db_s: 12,
        }
    }
}

impl Spectrum {
    pub const FLOORS: [i32; 3] = [-60, -90, -120];
    pub const CEILINGS: [i32; 3] = [0, 6, 12];
    pub const AVERAGING_MS: [u16; 6] = [0, 50, 125, 300, 1000, 3000];
    pub const PEAK_DECAYS: [u8; 4] = [0, 4, 12, 30];
    /// The strength below which curves are drawn dim: the middle of the
    /// Console's 30..100 % slider.
    pub const DIM_BELOW: u8 = 65;

    /// The device-side half, in the shape the engine pushes.
    pub fn engine_options(&self) -> dspi_session::rta::Options {
        dspi_session::rta::Options {
            fft_order: self.transform_order,
            avg_ms: self.averaging_ms,
            peak_decay_db_s: self.peak_decay_db_s,
        }
    }
}

/// Which volume the sidebar's slider drives when the app starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum VolumeChoice {
    #[default]
    User,
    Master,
}

impl VolumeChoice {
    pub const CHOICES: [&'static str; 2] = ["User", "Master"];

    pub fn index(self) -> usize {
        match self {
            Self::User => 0,
            Self::Master => 1,
        }
    }

    pub fn from_index(i: usize) -> Self {
        if i == 1 { Self::Master } else { Self::User }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Volume {
    pub mode: VolumeChoice,
}

/// Settings > Advanced, the app-side half.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Advanced {
    pub show_debug_info: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AppConfig {
    pub graphing: Graphing,
    pub volume: Volume,
    pub advanced: Advanced,
    pub spectrum: Spectrum,
    /// `console`, `amber`, `dark` or `mono`; `--theme` overrides it.
    #[serde(default)]
    pub theme: Option<String>,
}

impl AppConfig {
    /// `~/.config/dspi/config.toml`, or the platform's equivalent.
    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("dspi").join("config.toml"))
    }

    /// The stored settings, or the defaults when there is nothing readable.
    pub fn load() -> Self {
        match Self::path() {
            Some(p) => Self::load_from(&p),
            None => Self::default(),
        }
    }

    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .map(|s| Self::from_toml(&s))
            .unwrap_or_default()
    }

    /// Parse, falling back to the defaults on anything unparseable.
    pub fn from_toml(text: &str) -> Self {
        toml::from_str(text).unwrap_or_default()
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<PathBuf> {
        let Some(path) = Self::path() else {
            return Err(std::io::Error::other("no configuration directory"));
        };
        self.save_to(&path)?;
        Ok(path)
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.to_toml())
    }
}

impl GraphSettings {
    /// The graph settings the Graphing page holds, as the graph wants them.
    ///
    /// The live runner calls this on start so a change made in Settings is
    /// there the next time the app opens.
    pub fn from_config(g: &Graphing) -> Self {
        Self {
            min_hz: g.min_hz,
            max_hz: g.max_hz,
            db_center: g.db_center,
            db_range: g.db_range,
            show_phase: g.show_phase,
            unwrap_phase: g.unwrap_phase,
            freq_grid: g.freq_grid,
            freq_labels: g.freq_labels,
            db_grid: g.db_grid,
            db_labels: g.db_labels,
            freq_readout: g.freq_readout,
            gain_readout: g.gain_readout,
            grid: g.grid,
            dashboard_cards: g.dashboard_cards.min(3),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_consoles() {
        let c = AppConfig::default();
        assert_eq!(c.graphing.line_width, 2.0);
        assert_eq!(c.graphing.animation_speed, 0.2);
        assert!(!c.graphing.show_phase);
        assert!(!c.graphing.unwrap_phase);
        assert_eq!(c.graphing.db_range, 50.0);
        assert_eq!(c.graphing.db_center, 0.0);
        assert_eq!(c.graphing.min_hz, 15.0);
        assert_eq!(c.graphing.max_hz, 20_000.0);
        assert_eq!(c.volume.mode, VolumeChoice::User);
        let s = &c.spectrum;
        assert_eq!(s.strength_pct, 100);
        assert!(s.peak_hold && s.smoothing);
        assert_eq!((s.floor_db, s.ceiling_db), (-90, 6));
        assert_eq!(s.transform_order, None, "the device's own default");
        assert_eq!((s.averaging_ms, s.peak_decay_db_s), (300, 12));
    }

    #[test]
    fn a_config_round_trips_through_the_file() {
        let dir = std::env::temp_dir().join(format!("dspi-cfg-{}", std::process::id()));
        let path = dir.join("config.toml");
        let mut c = AppConfig::default();
        c.graphing.show_phase = true;
        c.graphing.min_hz = 100.0;
        c.graphing.max_hz = 10_000.0;
        c.graphing.db_range = 30.0;
        c.volume.mode = VolumeChoice::Master;
        c.advanced.show_debug_info = true;
        c.spectrum.transform_order = Some(9);
        c.spectrum.floor_db = -120;
        c.save_to(&path).expect("write");
        let back = AppConfig::load_from(&path);
        assert_eq!(back, c);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_or_broken_file_gives_the_defaults() {
        let path = std::env::temp_dir().join("dspi-config-that-does-not-exist.toml");
        assert_eq!(AppConfig::load_from(&path), AppConfig::default());
        assert_eq!(
            AppConfig::from_toml("this is not toml ]["),
            AppConfig::default()
        );
        // A partial file keeps the defaults for everything it does not name.
        let c = AppConfig::from_toml("[graphing]\nmin_hz = 20.0\n");
        assert_eq!(c.graphing.min_hz, 20.0);
        assert_eq!(c.graphing.max_hz, 20_000.0);
    }

    #[test]
    fn the_graph_reads_its_settings_from_the_config() {
        let mut c = AppConfig::default();
        c.graphing.show_phase = true;
        c.graphing.db_center = -10.0;
        c.graphing.freq_labels = false;
        let g = GraphSettings::from_config(&c.graphing);
        assert!(g.show_phase);
        assert_eq!(g.db_center, -10.0);
        assert!(!g.freq_labels);
        assert_eq!(g.max_hz, 20_000.0);
    }
}
