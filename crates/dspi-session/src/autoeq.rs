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

use serde::{Deserialize, Serialize};

/// Where a user-supplied database is looked for, under the platform's config
/// directory.
pub const USER_DB_NAME: &str = "autoeq_database.json";

/// Where the favourite profiles are kept, beside the database.
///
/// The Console keeps its favourites in `UserDefaults` under
/// `AutoEQ.FavoriteProfiles`, which has no cross-platform equivalent; a small
/// JSON file of ids in the same directory as the database is the terminal's
/// version of the same list.
pub const FAVOURITES_NAME: &str = "autoeq_favourites.json";

#[cfg(feature = "bundled-autoeq")]
const BUNDLED: &str = include_str!("../autoeq/autoeq_database.json");

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Database {
    #[serde(default)]
    pub version: i32,
    #[serde(default)]
    pub generated_at: String,
    /// The count the file states, which is not necessarily the number of
    /// entries it carries; the Console writes it and reads neither.
    #[serde(default)]
    pub entry_count: i32,
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

#[derive(Debug, Clone, Deserialize, Serialize)]
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

#[derive(Debug, Clone, Deserialize, Serialize)]
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

    #[error("could not write {path}: {reason}")]
    Unwritable { path: String, reason: String },

    #[error("{0}")]
    Download(String),
}

/// The directory the user's database and favourites live in.
pub fn config_dir() -> std::path::PathBuf {
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
    base.join("dspi-term")
}

/// Where a user-supplied database would live.
pub fn user_db_path() -> std::path::PathBuf {
    config_dir().join(USER_DB_NAME)
}

/// Whether a user-supplied database is in place, which is what decides whether
/// "Reset to Built-in" is offered.
pub fn has_user_database() -> bool {
    user_db_path().exists()
}

pub fn favourites_path() -> std::path::PathBuf {
    config_dir().join(FAVOURITES_NAME)
}

/// The favourite profile ids, oldest first. A missing or unreadable file is an
/// empty list: a favourites file is a convenience, never a reason to refuse to
/// open the browser.
pub fn load_favourites() -> Vec<String> {
    std::fs::read_to_string(favourites_path())
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok())
        .unwrap_or_default()
}

pub fn save_favourites(ids: &[String]) -> Result<(), AutoEqError> {
    let path = favourites_path();
    write_config(
        &path,
        &serde_json::to_string_pretty(ids).unwrap_or_default(),
    )
}

fn write_config(path: &std::path::Path, text: &str) -> Result<(), AutoEqError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| AutoEqError::Unwritable {
            path: dir.display().to_string(),
            reason: e.to_string(),
        })?;
    }
    std::fs::write(path, text).map_err(|e| AutoEqError::Unwritable {
        path: path.display().to_string(),
        reason: e.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Applying a profile
// ---------------------------------------------------------------------------

/// One band a profile wants written, already mapped onto a firmware type.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedBand {
    /// 0-based index into the channel's PEQ bank.
    pub band: u8,
    pub filter_type: dspi_proto::FilterType,
    pub freq: f32,
    pub q: f32,
    pub gain_db: f32,
}

/// What applying a profile to one channel comes to, worked out before anything
/// is written so the caller can say what will not survive.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ApplyPlan {
    pub preamp_db: f32,
    pub bands: Vec<PlannedBand>,
    /// Bands to clear: everything the profile does not use. Without this,
    /// whatever was there before survives underneath the correction.
    pub cleared: Vec<u8>,
    /// Shapes this build does not know, named once each.
    pub unsupported: Vec<String>,
    /// Bands past the end of the device's bank.
    pub dropped: usize,
}

impl Entry {
    /// The Console's `displayName`.
    pub fn display_name(&self) -> String {
        if self.model.is_empty() {
            self.manufacturer.clone()
        } else {
            format!("{} {}", self.manufacturer, self.model)
        }
    }

    /// The Console's `sourceDisplayName`: the capsule's text.
    pub fn source_label(&self) -> &str {
        match self.source.as_str() {
            "oratory1990" => "oratory1990",
            "crinacle" => "Crinacle",
            "rtings" => "Rtings",
            "innerfidelity" => "InnerFidelity",
            "headphone.com" => "Headphone.com",
            other => other,
        }
    }

    /// Work out what applying this profile to a channel with `max_bands` bands
    /// would do.
    pub fn plan(&self, max_bands: u8) -> ApplyPlan {
        let mut plan = ApplyPlan {
            preamp_db: self.preamp,
            dropped: self.filters.len().saturating_sub(max_bands as usize),
            ..Default::default()
        };
        for (i, f) in self.filters.iter().enumerate() {
            if i as u8 >= max_bands {
                break;
            }
            match filter_type(&f.kind) {
                Some(kind) => plan.bands.push(PlannedBand {
                    band: i as u8,
                    filter_type: kind,
                    freq: f.freq,
                    q: f.q,
                    gain_db: f.gain,
                }),
                None => {
                    if !plan.unsupported.contains(&f.kind) {
                        plan.unsupported.push(f.kind.clone());
                    }
                }
            }
        }
        plan.cleared = (self.filters.len().min(max_bands as usize) as u8..max_bands).collect();
        plan
    }
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

/// The name the database file gives a firmware filter type, which is the
/// Console's `filterTypes` table read the other way round.
fn kind_name(t: dspi_proto::FilterType) -> Option<&'static str> {
    use dspi_proto::FilterType as F;
    Some(match t {
        F::Peaking => "peaking",
        F::LowShelf => "lowShelf",
        F::HighShelf => "highShelf",
        F::LowPass => "lowPass",
        F::HighPass => "highPass",
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Update Database
// ---------------------------------------------------------------------------

/// Fetching a URL, so the rebuild can be driven by a test without a network.
///
/// The default implementation shells out to `curl`, which every platform this
/// runs on ships: an HTTP client crate would pull a TLS stack and a hundred
/// transitive dependencies into a program whose whole point is to talk to a USB
/// device. Nothing here runs unless the person asks for it.
pub trait Fetch: Send + Sync {
    fn get(&self, url: &str) -> Result<String, String>;
}

pub struct CurlFetch;

impl Fetch for CurlFetch {
    fn get(&self, url: &str) -> Result<String, String> {
        let out = std::process::Command::new("curl")
            .args([
                "-fsSL",
                "-H",
                "Accept: application/vnd.github.v3+json",
                "--max-time",
                "30",
                url,
            ])
            .output()
            .map_err(|e| format!("could not run curl: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "{} answered nothing usable",
                url.split('/').next_back().unwrap_or(url)
            ));
        }
        String::from_utf8(out.stdout).map_err(|_| "the download was not text".to_string())
    }
}

/// The AutoEQ sources, in the Console's priority order: when two sources
/// measure the same headphone the lower number wins.
const SOURCES: [(&str, &str, u8); 5] = [
    ("oratory1990", "oratory1990", 1),
    ("crinacle", "crinacle", 2),
    ("Rtings", "rtings", 3),
    ("Innerfidelity", "innerfidelity", 3),
    ("Headphone.com Legacy", "headphone.com", 4),
];

const REPO: &str = "jaakkopasanen/AutoEq";

/// Where the rebuild has got to, for the progress dialog.
#[derive(Debug, Clone, PartialEq)]
pub struct RebuildProgress {
    pub fraction: f32,
    pub status: String,
    /// Set once, at the end: the entry count, or why it stopped.
    pub done: Option<Result<usize, String>>,
}

/// A rebuild running on its own thread.
///
/// The interface polls [`RebuildHandle::poll`] each tick rather than blocking:
/// a full rebuild is thousands of requests and takes minutes, and an interface
/// that stops answering for minutes is broken however good the result is.
pub struct RebuildHandle {
    rx: std::sync::mpsc::Receiver<RebuildProgress>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    latest: RebuildProgress,
}

impl RebuildHandle {
    /// The newest progress, or the last one seen if nothing new arrived.
    pub fn poll(&mut self) -> &RebuildProgress {
        while let Ok(p) = self.rx.try_recv() {
            self.latest = p;
        }
        &self.latest
    }

    /// Stop at the next profile boundary.
    pub fn cancel(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Drop for RebuildHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// What the rebuild will fetch, said in full before it is started.
///
/// Nothing in this program touches the network unless someone reads this and
/// says yes.
pub fn rebuild_warning() -> String {
    format!(
        "You are about to rebuild the AutoEQ database by downloading all profiles \
         from GitHub.\n\nThis requires an internet connection and may take several \
         minutes. It reads https://api.github.com/repos/{REPO}/contents to list the \
         profiles and https://raw.githubusercontent.com/{REPO} to download each one, \
         sends nothing about you or your device, and writes the result to {}.\n\nDo \
         you wish to proceed?",
        user_db_path().display()
    )
}

/// Start a rebuild on a background thread.
pub fn rebuild_from_github(fetch: Box<dyn Fetch>) -> RebuildHandle {
    let (tx, rx) = std::sync::mpsc::channel();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = std::sync::Arc::clone(&stop);
    std::thread::Builder::new()
        .name("dspi-autoeq-rebuild".into())
        .spawn(move || {
            let send = |fraction: f32, status: String, done: Option<Result<usize, String>>| {
                let _ = tx.send(RebuildProgress {
                    fraction,
                    status,
                    done,
                });
            };
            match run_rebuild(fetch.as_ref(), &flag, &user_db_path(), &send) {
                Ok(n) => send(1.0, format!("Complete! {n} profiles."), Some(Ok(n))),
                Err(e) => send(0.0, e.clone(), Some(Err(e))),
            }
        })
        .expect("spawn the AutoEQ rebuild");
    RebuildHandle {
        rx,
        stop,
        latest: RebuildProgress {
            fraction: 0.0,
            status: "Connecting to GitHub...".into(),
            done: None,
        },
    }
}

fn run_rebuild(
    fetch: &dyn Fetch,
    stop: &std::sync::atomic::AtomicBool,
    out: &std::path::Path,
    send: &dyn Fn(f32, String, Option<Result<usize, String>>),
) -> Result<usize, String> {
    let stopped = || stop.load(std::sync::atomic::Ordering::Relaxed);
    send(0.0, "Connecting to GitHub...".into(), None);
    send(0.0, "Discovering profiles...".into(), None);

    // (source folder, source name, priority, target, headphone)
    let mut wanted: Vec<(String, String, u8, String, String)> = Vec::new();
    for (folder, name, priority) in SOURCES {
        if stopped() {
            return Err("Cancelled.".into());
        }
        let Ok(targets) = list_dir(fetch, &format!("results/{folder}")) else {
            continue;
        };
        for target in targets {
            let Ok(headphones) = list_dir(fetch, &format!("results/{folder}/{target}")) else {
                continue;
            };
            for headphone in headphones {
                wanted.push((
                    folder.to_string(),
                    name.to_string(),
                    priority,
                    target.clone(),
                    headphone,
                ));
            }
        }
        send(0.0, format!("Found {} profiles...", wanted.len()), None);
    }
    if wanted.is_empty() {
        return Err("Rebuild failed: GitHub listed no profiles.".into());
    }

    let total = wanted.len();
    send(0.0, format!("Downloading 0 / {total} profiles..."), None);

    // Keyed by manufacturer and model so two sources measuring the same
    // headphone collapse to the better-regarded one.
    let mut best: std::collections::BTreeMap<String, (Entry, u8)> =
        std::collections::BTreeMap::new();
    for (done, (folder, name, priority, target, headphone)) in wanted.into_iter().enumerate() {
        if stopped() {
            return Err("Cancelled.".into());
        }
        let url = format!(
            "https://raw.githubusercontent.com/{REPO}/master/results/{}/{}/{}/{} ParametricEQ.txt",
            encode(&folder),
            encode(&target),
            encode(&headphone),
            encode(&headphone)
        );
        if let Ok(text) = fetch.get(&url)
            && let Some(entry) = profile_entry(&text, &name, &target, &headphone)
        {
            let key = format!(
                "{}/{}",
                entry.manufacturer.to_ascii_lowercase(),
                entry.model.to_ascii_lowercase()
            );
            match best.get(&key) {
                Some((_, p)) if *p <= priority => {}
                _ => {
                    best.insert(key, (entry, priority));
                }
            }
        }
        if done % 20 == 0 || done + 1 == total {
            send(
                (done + 1) as f32 / total as f32,
                format!("Downloading {} / {total} profiles...", done + 1),
                None,
            );
        }
    }

    send(1.0, "Building database...".into(), None);
    let mut entries: Vec<Entry> = best.into_values().map(|(e, _)| e).collect();
    entries.sort_by_key(|e| {
        (
            e.manufacturer.to_ascii_lowercase(),
            e.model.to_ascii_lowercase(),
        )
    });
    let count = entries.len();
    let db = Database {
        version: 1,
        generated_at: String::new(),
        entry_count: count as i32,
        entries,
        origin: Origin::User,
    };
    let text = serde_json::to_string_pretty(&db).map_err(|e| e.to_string())?;
    write_config(out, &text).map_err(|e| e.to_string())?;
    Ok(count)
}

/// The directory listing from the GitHub contents API, folder names only.
fn list_dir(fetch: &dyn Fetch, path: &str) -> Result<Vec<String>, String> {
    let text = fetch.get(&format!(
        "https://api.github.com/repos/{REPO}/contents/{}",
        encode(path)
    ))?;
    let items: Vec<serde_json::Value> =
        serde_json::from_str(&text).map_err(|e| format!("GitHub answered something else: {e}"))?;
    Ok(items
        .into_iter()
        .filter(|i| i.get("type").and_then(|t| t.as_str()) == Some("dir"))
        .filter_map(|i| {
            i.get("name")
                .and_then(|n| n.as_str())
                .map(|s| s.to_string())
        })
        .collect())
}

/// Percent-encode the characters a headphone name actually contains; the path
/// separators stay as they are, which is what `urlPathAllowed` does.
fn encode(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        match c {
            ' ' => out.push_str("%20"),
            '#' => out.push_str("%23"),
            '?' => out.push_str("%3F"),
            '+' => out.push_str("%2B"),
            '&' => out.push_str("%26"),
            other => out.push(other),
        }
    }
    out
}

/// Turn one downloaded `ParametricEQ.txt` into a database entry.
///
/// The file is REW's format, which the filter-file reader already speaks, so
/// there is one parser for both rather than two that can disagree.
pub fn profile_entry(text: &str, source: &str, target: &str, headphone: &str) -> Option<Entry> {
    let file = crate::filterfile::parse(text).ok()?;
    let bank = file.channels.first()?;
    let filters: Vec<Filter> = bank
        .peq
        .iter()
        .filter_map(|b| {
            Some(Filter {
                kind: kind_name(b.filter_type)?.into(),
                freq: b.freq,
                q: b.q,
                gain: b.gain_db,
            })
        })
        .collect();
    if filters.is_empty() {
        return None;
    }
    let (manufacturer, model) = split_name(headphone);
    Some(Entry {
        id: format!("{source}/{headphone}"),
        manufacturer,
        model,
        source: source.to_string(),
        form_factor: form_factor(target),
        preamp: bank.preamp_db.unwrap_or(0.0),
        filters,
    })
}

/// The Console's `detectFormFactor`, which reads it off the target folder.
fn form_factor(target: &str) -> String {
    let lower = target.to_ascii_lowercase();
    if lower.contains("in-ear") || lower.contains("in_ear") || lower.contains("iem") {
        "in-ear".into()
    } else if lower.contains("earbud") {
        "earbud".into()
    } else {
        "over-ear".into()
    }
}

/// The Console's `parseHeadphoneName`: a known manufacturer wins over the
/// first-word split, so "Dan Clark Audio Aeon" does not become manufacturer
/// "Dan".
fn split_name(folder: &str) -> (String, String) {
    const KNOWN: [&str; 62] = [
        "AKG",
        "Audio-Technica",
        "Audeze",
        "Bang & Olufsen",
        "Beats",
        "Beyerdynamic",
        "Bose",
        "Campfire Audio",
        "Dan Clark Audio",
        "Denon",
        "FiiO",
        "Final",
        "Focal",
        "Grado",
        "HarmonicDyne",
        "HIFIMAN",
        "JBL",
        "Koss",
        "Massdrop",
        "Meze",
        "Moondrop",
        "Philips",
        "Pioneer",
        "Sennheiser",
        "Shure",
        "Sony",
        "SteelSeries",
        "STAX",
        "Tin HiFi",
        "V-MODA",
        "ZMF",
        "64 Audio",
        "7Hz",
        "Anker",
        "Apple",
        "AFUL",
        "BLON",
        "CCA",
        "Dunu",
        "Empire Ears",
        "Etymotic",
        "FatFreq",
        "Hidizs",
        "HiBy",
        "iBasso",
        "JVC",
        "KZ",
        "Letshuoer",
        "Linsoul",
        "Noble Audio",
        "QKZ",
        "Samsung",
        "See Audio",
        "Simgot",
        "SoftEars",
        "Tangzu",
        "Thieaudio",
        "Tinhifi",
        "Tripowin",
        "TRN",
        "Truthear",
        "Unique Melody",
    ];
    let lower = folder.to_ascii_lowercase();
    for mfr in KNOWN {
        let m = mfr.to_ascii_lowercase();
        if lower == m {
            return (mfr.into(), String::new());
        }
        if let Some(rest) = lower.strip_prefix(&format!("{m} ")) {
            let _ = rest;
            let model = folder[mfr.len()..].trim().to_string();
            return (mfr.into(), model);
        }
    }
    match folder.split_once(' ') {
        Some((a, b)) => (a.into(), b.into()),
        None => (folder.into(), String::new()),
    }
}

/// Replace the user database with a file the person chose.
pub fn import_database(path: &std::path::Path) -> Result<usize, AutoEqError> {
    let text = std::fs::read_to_string(path).map_err(|e| AutoEqError::Unreadable(e.to_string()))?;
    let db: Database =
        serde_json::from_str(&text).map_err(|e| AutoEqError::Unreadable(e.to_string()))?;
    write_config(&user_db_path(), &text)?;
    Ok(db.entries.len())
}

/// Throw the user database away and go back to the bundled one.
pub fn reset_to_builtin() -> Result<usize, AutoEqError> {
    let path = user_db_path();
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| AutoEqError::Unwritable {
            path: path.display().to_string(),
            reason: e.to_string(),
        })?;
    }
    Ok(load()?.entries.len())
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
        assert!(favourites_path().ends_with(FAVOURITES_NAME));
    }

    fn entry(filters: Vec<(&str, f32)>) -> Entry {
        Entry {
            id: "oratory1990/Sennheiser HD 600".into(),
            manufacturer: "Sennheiser".into(),
            model: "HD 600".into(),
            source: "oratory1990".into(),
            form_factor: "over-ear".into(),
            preamp: -6.5,
            filters: filters
                .into_iter()
                .map(|(kind, freq)| Filter {
                    kind: kind.into(),
                    freq,
                    q: 0.7,
                    gain: 3.0,
                })
                .collect(),
        }
    }

    /// Bands the profile does not use have to be cleared, or whatever was on
    /// the channel before survives underneath the correction.
    #[test]
    fn a_plan_clears_the_bands_the_profile_does_not_use() {
        let plan = entry(vec![("peaking", 100.0), ("lowShelf", 105.0)]).plan(10);
        assert_eq!(plan.preamp_db, -6.5);
        assert_eq!(plan.bands.len(), 2);
        assert_eq!(plan.bands[1].filter_type, dspi_proto::FilterType::LowShelf);
        assert_eq!(plan.cleared, (2u8..10).collect::<Vec<_>>());
        assert_eq!(plan.dropped, 0);
        assert!(plan.unsupported.is_empty());
    }

    #[test]
    fn a_plan_reports_what_will_not_fit_or_is_not_understood() {
        let mut e = entry(vec![("peaking", 100.0); 12]);
        e.filters[1].kind = "wobble".into();
        let plan = e.plan(10);
        assert_eq!(plan.dropped, 2, "12 bands into a bank of 10");
        assert_eq!(plan.bands.len(), 9, "the unknown shape is not invented");
        assert_eq!(plan.unsupported, vec!["wobble".to_string()]);
    }

    #[test]
    fn an_entry_names_itself_the_way_the_console_does() {
        let e = entry(vec![("peaking", 100.0)]);
        assert_eq!(e.display_name(), "Sennheiser HD 600");
        assert_eq!(e.source_label(), "oratory1990");
        let mut bare = e.clone();
        bare.model = String::new();
        assert_eq!(bare.display_name(), "Sennheiser");
        bare.source = "crinacle".into();
        assert_eq!(bare.source_label(), "Crinacle");
    }

    #[test]
    fn a_headphone_folder_splits_on_a_known_manufacturer_first() {
        assert_eq!(
            split_name("Dan Clark Audio Aeon 2"),
            ("Dan Clark Audio".into(), "Aeon 2".into())
        );
        assert_eq!(
            split_name("Sennheiser HD 600"),
            ("Sennheiser".into(), "HD 600".into())
        );
        assert_eq!(
            split_name("Wobbletron X1"),
            ("Wobbletron".into(), "X1".into())
        );
        assert_eq!(split_name("Nameless"), ("Nameless".into(), "".into()));
    }

    #[test]
    fn the_form_factor_comes_off_the_target_folder() {
        assert_eq!(form_factor("harman_in-ear_2019v2"), "in-ear");
        assert_eq!(form_factor("harman_earbud_2019"), "earbud");
        assert_eq!(form_factor("harman_over-ear_2018"), "over-ear");
    }

    /// A downloaded profile is REW's format, so the filter-file reader is the
    /// one parser for both.
    #[test]
    fn a_downloaded_profile_becomes_an_entry() {
        let text = "\
Preamp: -6.5 dB
Filter 1: ON PK Fc 105 Hz Gain 3.0 dB Q 0.70
Filter 2: ON LSC Fc 105 Hz Gain 5.5 dB Q 0.70
Filter 3: ON None
";
        let e = profile_entry(
            text,
            "oratory1990",
            "harman_over-ear_2018",
            "Sennheiser HD 600",
        )
        .expect("a profile with filters");
        assert_eq!(e.id, "oratory1990/Sennheiser HD 600");
        assert_eq!(e.manufacturer, "Sennheiser");
        assert_eq!(e.form_factor, "over-ear");
        assert_eq!(e.preamp, -6.5);
        assert_eq!(e.filters.len(), 2);
        assert_eq!(e.filters[1].kind, "lowShelf");
        // And it reads back through the same mapper the browser applies with.
        assert_eq!(
            filter_type(&e.filters[1].kind),
            Some(dspi_proto::FilterType::LowShelf)
        );
        assert!(profile_entry("nothing here", "s", "t", "h").is_none());
    }

    /// A rebuild is the one thing in this program that touches the network, so
    /// the warning has to say exactly what it will reach for.
    #[test]
    fn the_rebuild_warning_names_what_it_fetches() {
        let w = rebuild_warning();
        assert!(w.contains("api.github.com"), "{w}");
        assert!(w.contains("raw.githubusercontent.com"), "{w}");
        assert!(w.contains("sends nothing about you or your device"), "{w}");
        assert!(w.contains("Do you wish to proceed?"), "{w}");
    }

    /// A fake GitHub, so the whole rebuild is exercised without a network.
    struct FakeGithub;

    impl Fetch for FakeGithub {
        fn get(&self, url: &str) -> Result<String, String> {
            if url.contains("api.github.com") {
                if url.ends_with("results/oratory1990") {
                    return Ok(r#"[{"name":"harman_over-ear_2018","type":"dir"}]"#.into());
                }
                if url.ends_with("harman_over-ear_2018") {
                    return Ok(
                        r#"[{"name":"Sennheiser HD 600","type":"dir"},{"name":"README.md","type":"file"}]"#
                            .into(),
                    );
                }
                return Err("404".into());
            }
            assert!(
                url.contains("Sennheiser%20HD%20600"),
                "spaces must be encoded: {url}"
            );
            Ok("Preamp: -6.5 dB\nFilter 1: ON PK Fc 105 Hz Gain 3.0 dB Q 0.70\n".into())
        }
    }

    #[test]
    fn a_rebuild_walks_the_listing_and_reports_progress() {
        let stop = std::sync::atomic::AtomicBool::new(false);
        let seen = std::sync::Mutex::new(Vec::new());
        // The rebuild writes wherever it is told, so it is pointed at a scratch
        // file rather than the real user database.
        let dir = std::env::temp_dir().join(format!("dspi-autoeq-{}", std::process::id()));
        let out = dir.join(USER_DB_NAME);

        let result = run_rebuild(&FakeGithub, &stop, &out, &|f, s, _| {
            seen.lock().unwrap().push((f, s));
        });

        let written = std::fs::read_to_string(&out).ok();
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(result, Ok(1), "one headphone, from one source");
        let lines: Vec<String> = seen
            .lock()
            .unwrap()
            .iter()
            .map(|(_, s)| s.clone())
            .collect();
        assert_eq!(lines[0], "Connecting to GitHub...");
        assert!(
            lines.contains(&"Discovering profiles...".to_string()),
            "{lines:?}"
        );
        assert!(
            lines.contains(&"Found 1 profiles...".to_string()),
            "{lines:?}"
        );
        assert!(
            lines.contains(&"Downloading 1 / 1 profiles...".to_string()),
            "{lines:?}"
        );
        assert!(
            lines.contains(&"Building database...".to_string()),
            "{lines:?}"
        );

        let written = written.expect("the rebuild writes the user database");
        assert!(written.contains("\"entryCount\": 1"), "{written}");
        assert!(written.contains("Sennheiser"), "{written}");
        let back: Database = serde_json::from_str(&written).unwrap();
        assert_eq!(back.entries[0].filters[0].kind, "peaking");
    }

    #[test]
    fn a_cancelled_rebuild_says_so_rather_than_writing_half_a_database() {
        let stop = std::sync::atomic::AtomicBool::new(true);
        let out = std::env::temp_dir().join("dspi-autoeq-cancelled.json");
        let result = run_rebuild(&FakeGithub, &stop, &out, &|_, _, _| {});
        assert_eq!(result, Err("Cancelled.".into()));
    }
}
