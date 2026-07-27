//! AutoEQ headphone correction profiles.
//!
//! The database is bundled so the feature works offline and on a machine that
//! has never seen the Console, following the same resolution order the Console
//! uses: a user-supplied copy wins, otherwise the built-in one. That ordering is
//! what lets someone drop in a newer export without waiting for an app release.
//!
//! Bundling costs about 5 MB in the binary, which is most of its size. The
//! `bundled-autoeq` feature is on by default and can be turned off for a build
//! where that matters, such as an armv7 Raspberry Pi; the user-supplied path
//! still works without it.

use serde::Deserialize;

/// Where a user-supplied database is looked for, under the platform's config
/// directory.
pub const USER_DB_NAME: &str = "autoeq_database.json";

#[cfg(feature = "bundled-autoeq")]
const BUNDLED: &str = include_str!("../autoeq/autoeq_database.json");

#[derive(Debug, Clone, Deserialize)]
pub struct Database {
    #[serde(default)]
    pub version: i32,
    #[serde(default)]
    pub generated_at: String,
    #[serde(default)]
    pub entries: Vec<Entry>,
    /// Which copy this came from, so a stale bundle is visible rather than
    /// silently assumed current.
    #[serde(skip)]
    pub origin: Origin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Origin {
    #[default]
    Bundled,
    User,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    #[serde(default)]
    pub manufacturer: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub form_factor: String,
    /// The trim the profile expects, which is almost always negative: the
    /// corrections boost, and without it the result clips.
    #[serde(default)]
    pub preamp: f32,
    #[serde(default)]
    pub filters: Vec<Filter>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Filter {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub freq: f32,
    #[serde(default)]
    pub q: f32,
    #[serde(default)]
    pub gain: f32,
}

#[derive(Debug, thiserror::Error)]
pub enum AutoEqError {
    #[error("no AutoEQ database available; supply one at {0}")]
    Missing(String),

    #[error("could not read the AutoEQ database: {0}")]
    Unreadable(String),
}

/// Where a user-supplied database would live.
pub fn user_db_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| {
                let p = std::path::PathBuf::from(h);
                if cfg!(target_os = "macos") {
                    p.join("Library/Application Support")
                } else {
                    p.join(".config")
                }
            })
        })
        .or_else(|| std::env::var_os("APPDATA").map(std::path::PathBuf::from))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    base.join("dspi-term").join(USER_DB_NAME)
}

/// Load the database, preferring a user-supplied copy.
pub fn load() -> Result<Database, AutoEqError> {
    let path = user_db_path();
    if path.exists() {
        let text =
            std::fs::read_to_string(&path).map_err(|e| AutoEqError::Unreadable(e.to_string()))?;
        let mut db: Database =
            serde_json::from_str(&text).map_err(|e| AutoEqError::Unreadable(e.to_string()))?;
        db.origin = Origin::User;
        return Ok(db);
    }

    #[cfg(feature = "bundled-autoeq")]
    {
        let mut db: Database =
            serde_json::from_str(BUNDLED).map_err(|e| AutoEqError::Unreadable(e.to_string()))?;
        db.origin = Origin::Bundled;
        Ok(db)
    }
    #[cfg(not(feature = "bundled-autoeq"))]
    {
        Err(AutoEqError::Missing(path.display().to_string()))
    }
}

impl Database {
    /// Find profiles matching a query.
    ///
    /// Matching is on manufacturer, model and id together, so "sennheiser hd600"
    /// works as well as "hd600", and the terms may appear in any order.
    pub fn search(&self, query: &str) -> Vec<&Entry> {
        let terms: Vec<String> = query
            .split_whitespace()
            .map(|t| t.to_ascii_lowercase())
            .collect();
        if terms.is_empty() {
            return Vec::new();
        }

        let mut hits: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|e| {
                let hay = format!("{} {} {}", e.manufacturer, e.model, e.id).to_ascii_lowercase();
                terms.iter().all(|t| hay.contains(t))
            })
            .collect();

        // An exact model match should not be buried under longer names that
        // merely contain it.
        hits.sort_by_key(|e| (e.model.len(), e.id.clone()));
        hits
    }

    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }
}

/// Map an AutoEQ filter shape onto a firmware filter type.
///
/// An unrecognised shape returns `None` rather than being forced into a peaking
/// filter, because silently changing a correction's shape produces a curve that
/// is wrong in a way nobody can see.
pub fn filter_type(kind: &str) -> Option<dspi_proto::FilterType> {
    use dspi_proto::FilterType as F;
    Some(match kind.to_ascii_uppercase().as_str() {
        "PK" | "PEAKING" => F::Peaking,
        "LS" | "LOWSHELF" | "LSC" => F::LowShelf,
        "HS" | "HIGHSHELF" | "HSC" => F::HighShelf,
        "LP" | "LOWPASS" | "LPQ" => F::LowPass,
        "HP" | "HIGHPASS" | "HPQ" => F::HighPass,
        "NO" | "NOTCH" => F::Notch,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "bundled-autoeq")]
    #[test]
    fn the_bundled_database_loads_and_is_populated() {
        let db = load().expect("the bundled database should always load");
        assert!(
            db.entries.len() > 1000,
            "only {} entries; the bundle looks truncated",
            db.entries.len()
        );
        assert_eq!(db.origin, Origin::Bundled);
    }

    #[cfg(feature = "bundled-autoeq")]
    #[test]
    fn a_well_known_headphone_is_findable() {
        let db = load().unwrap();
        let hits = db.search("hd600");
        assert!(
            !hits.is_empty(),
            "the HD 600 should be in any AutoEQ export"
        );

        let e = hits[0];
        assert!(
            !e.filters.is_empty(),
            "a profile with no filters is useless"
        );
        // Corrections boost, so the trim is essentially always negative.
        assert!(e.preamp <= 0.0, "preamp should be a cut, got {}", e.preamp);
    }

    #[cfg(feature = "bundled-autoeq")]
    #[test]
    fn every_filter_shape_in_the_database_is_understood() {
        let db = load().unwrap();
        let mut unknown = std::collections::BTreeSet::new();
        for e in &db.entries {
            for f in &e.filters {
                if filter_type(&f.kind).is_none() {
                    unknown.insert(f.kind.clone());
                }
            }
        }
        assert!(
            unknown.is_empty(),
            "shapes the mapper does not know: {unknown:?}"
        );
    }

    #[cfg(feature = "bundled-autoeq")]
    #[test]
    fn search_terms_may_arrive_in_any_order() {
        let db = load().unwrap();
        let a = db.search("sennheiser hd600").len();
        let b = db.search("hd600 sennheiser").len();
        assert_eq!(a, b);
    }

    #[test]
    fn filter_shapes_map_to_firmware_types() {
        use dspi_proto::FilterType as F;
        assert_eq!(filter_type("PK"), Some(F::Peaking));
        assert_eq!(filter_type("lowshelf"), Some(F::LowShelf));
        assert_eq!(filter_type("HSC"), Some(F::HighShelf));
    }

    /// Forcing an unknown shape into a peaking filter would produce a curve
    /// that is wrong in a way nobody can see.
    #[test]
    fn an_unknown_shape_is_refused_rather_than_guessed() {
        assert_eq!(filter_type("WOBBLE"), None);
        assert_eq!(filter_type(""), None);
    }

    #[test]
    fn the_user_path_is_somewhere_sensible() {
        let p = user_db_path();
        assert!(p.ends_with(USER_DB_NAME));
        assert!(p.to_string_lossy().contains("dspi-term"));
    }
}
