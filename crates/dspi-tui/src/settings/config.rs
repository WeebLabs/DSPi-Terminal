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

use crate::graph::GraphSettings;

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
            db_range: 50.0,
            db_center: 0.0,
            min_hz: 15.0,
            max_hz: 20_000.0,
            popout_follows_selection: true,
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
