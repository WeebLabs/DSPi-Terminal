//! The Console's human-readable filter file.
//!
//! Format and grammar follow `FilterFile.swift`, because these files are the
//! way a tuning moves between the two applications and a near-miss is worse
//! than no support at all.
//!
//! The grammar is a superset of REW's, so REW and AutoEQ exports parse
//! unchanged. Everything after the type token is read as label/value pairs,
//! which means units and any extra trailing text another tool emits are ignored
//! rather than misread.

use dspi_proto::FilterType;
use dspi_proto::dsp::Band;
use dspi_proto::xover;

/// The version this writer emits. A file with no stamp is version 1: inputs
/// only, PEQ bands only, no preamp.
pub const FORMAT_VERSION: u32 = 2;

/// Which bank a band line belongs to. Crossover bands live in their own
/// per-output bank at wire indices 20-23, so they need a separate keyword
/// rather than sharing the PEQ numbering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bank {
    Peq,
    Crossover,
}

impl Bank {
    fn keyword(self) -> &'static str {
        match self {
            Bank::Peq => "Filter",
            Bank::Crossover => "Xover",
        }
    }
}

/// One channel's worth of a filter file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChannelBank {
    /// The header as written, for reporting what could not be placed.
    pub header: String,
    /// Index from an `[Input 3: ...]`-style header, when there is one.
    pub index: Option<u8>,
    pub is_output: bool,
    pub preamp_db: Option<f32>,
    pub peq: Vec<Band>,
    pub crossover: Vec<Band>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FilterFile {
    pub format_version: u32,
    pub channels: Vec<ChannelBank>,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum FilterFileError {
    #[error("this does not look like a filter file: no channel sections found")]
    NoChannels,
}

/// The two-letter codes the Console writes, and reads back.
fn type_code(t: FilterType) -> Option<&'static str> {
    Some(match t {
        FilterType::Flat => return None,
        FilterType::Peaking => "PK",
        FilterType::LowShelf => "LS",
        FilterType::HighShelf => "HS",
        FilterType::LowPass => "LP",
        FilterType::HighPass => "HP",
        FilterType::Notch => "NT",
        FilterType::AllPass => "AP",
        FilterType::AllPass1 => "AP1",
        FilterType::LowShelf1 => "LS1",
        FilterType::HighShelf1 => "HS1",
        // The Console writes these as its `shortLabel`, DSPMath.swift:178-179.
        FilterType::LowPass1 => "LP1",
        FilterType::HighPass1 => "HP1",
        FilterType::LinkwitzTransform => "LT",
        // Crossovers spell out their family and order with the space removed.
        FilterType::Unknown(raw) => {
            return xover::meta(raw).map(|_| "XO").filter(|_| false);
        }
    })
}

/// A crossover type's code, e.g. `LR4LP`.
fn xover_code(raw: u8) -> Option<String> {
    xover::meta(raw).map(|m| {
        format!(
            "{}{}{}",
            m.family.short().to_uppercase(),
            m.order,
            if m.high_pass { "HP" } else { "LP" }
        )
    })
}

fn code_for(t: FilterType) -> Option<String> {
    if t.is_crossover() {
        xover_code(t.to_raw())
    } else {
        type_code(t).map(|s| s.to_string())
    }
}

fn type_from_code(code: &str) -> Option<FilterType> {
    let upper = code.to_ascii_uppercase();
    let plain = match upper.as_str() {
        "PK" | "PEQ" => Some(FilterType::Peaking),
        // LSC / HSC are REW's spelling of the second-order shelves, which is
        // how the Console reads them (DSPMath.swift:297-298) and how
        // `autoeq.rs` already read them. They are not the first-order pair.
        "LS" | "LSC" => Some(FilterType::LowShelf),
        "HS" | "HSC" => Some(FilterType::HighShelf),
        "LP" | "LPQ" => Some(FilterType::LowPass),
        "HP" | "HPQ" => Some(FilterType::HighPass),
        "NT" | "NO" | "NOTCH" => Some(FilterType::Notch),
        "AP" => Some(FilterType::AllPass),
        "AP1" => Some(FilterType::AllPass1),
        "LS1" => Some(FilterType::LowShelf1),
        "HS1" => Some(FilterType::HighShelf1),
        "LP1" => Some(FilterType::LowPass1),
        "HP1" => Some(FilterType::HighPass1),
        "LT" => Some(FilterType::LinkwitzTransform),
        _ => None,
    };
    if plain.is_some() {
        return plain;
    }
    // Crossover codes are matched by regenerating each candidate, so the table
    // cannot drift from the one used to write them.
    (xover::XOVER_FIRST..=xover::XOVER_LAST)
        .find(|t| xover_code(*t).as_deref() == Some(upper.as_str()))
        .map(FilterType::from_raw)
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

pub fn format_preamp(db: f32) -> String {
    format!("Preamp {db:+.1} dB\n")
}

/// One band line.
pub fn format_band(bank: Bank, index: usize, b: &Band, qp: Option<f32>) -> String {
    let keyword = format!("{:<6}", bank.keyword());

    let Some(code) = code_for(b.filter_type) else {
        return format!("{keyword} {index:2}: OFF\n");
    };

    let mut line = format!("{keyword} {index:2}: ON  {:<8}Fc {:7.1} Hz", code, b.freq);

    if b.filter_type.is_linkwitz() {
        // The Linkwitz Transform repurposes the wire fields, so its parameters
        // are spelled out with distinct labels; sharing the Gain/Q labels would
        // not survive a round trip through another tool.
        line.push_str(&format!(
            "  Q {:5.2}  Fp {:7.1} Hz  Qp {:5.2}",
            b.q,
            b.gain_db,
            qp.unwrap_or(0.707)
        ));
    } else {
        if b.filter_type.uses_gain() {
            line.push_str(&format!("  Gain {:+5.1} dB", b.gain_db));
        }
        if b.filter_type.uses_q() {
            line.push_str(&format!("  Q {:5.2}", b.q));
        }
    }

    if b.bypass {
        line.push_str("  [Bypassed]");
    }
    line.push('\n');
    line
}

/// Render a whole file.
pub fn write(file: &FilterFile, exported: &str) -> String {
    let mut out = String::from("# DSPi Console Filter Settings\n");
    out.push_str(&format!("# Exported: {exported}\n"));
    out.push_str(&format!("# Format: {FORMAT_VERSION}\n\n"));

    for ch in &file.channels {
        out.push_str(&format!("[{}]\n", ch.header));
        if let Some(p) = ch.preamp_db {
            out.push_str(&format_preamp(p));
        }
        for (i, b) in ch.peq.iter().enumerate() {
            out.push_str(&format_band(Bank::Peq, i + 1, b, None));
        }
        for (i, b) in ch.crossover.iter().enumerate() {
            out.push_str(&format_band(Bank::Crossover, i + 1, b, None));
        }
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// Parse a filter file, tolerating both the current format and the older
/// name-keyed one, and any extra text other tools emit.
pub fn parse(text: &str) -> Result<FilterFile, FilterFileError> {
    let mut file = FilterFile {
        format_version: 1,
        channels: Vec::new(),
    };
    let mut current: Option<ChannelBank> = None;

    for raw in text.lines() {
        let line = raw.trim();

        if let Some(rest) = line.strip_prefix('#') {
            if let Some(v) = rest.trim().strip_prefix("Format:")
                && let Ok(n) = v.trim().parse::<u32>()
            {
                file.format_version = n;
            }
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            if let Some(c) = current.take() {
                file.channels.push(c);
            }
            current = Some(parse_header(&line[1..line.len() - 1]));
            continue;
        }

        // A REW export has no section header at all: the bands start straight
        // away. They are one unnamed channel, which is what the Console's
        // single-channel picker then asks the person to place.
        if current.is_none() && (parse_preamp(line).is_some() || parse_band(line).is_some()) {
            current = Some(ChannelBank::default());
        }
        let Some(ch) = current.as_mut() else { continue };

        if let Some(p) = parse_preamp(line) {
            ch.preamp_db = Some(p);
            continue;
        }
        if let Some((bank, band)) = parse_band(line) {
            match bank {
                Bank::Peq => ch.peq.push(band),
                Bank::Crossover => ch.crossover.push(band),
            }
        }
    }

    if let Some(c) = current.take() {
        file.channels.push(c);
    }
    if file.channels.is_empty() {
        return Err(FilterFileError::NoChannels);
    }
    Ok(file)
}

/// `Input 3: USB 4` or `Output 0: SPDIF 1 L (Enabled)` or a bare legacy name.
///
/// The index is only read from this app's own shape: `Input`/`Output`, a
/// number, then a colon. Without the colon the header is a name, and reading a
/// number out of one would place `[SPDIF 1 L]` on input 1 and `[Input 5]` (the
/// Windows spelling, which counts from one) a channel off.
fn parse_header(inner: &str) -> ChannelBank {
    let lower = inner.to_ascii_lowercase();
    let is_output = lower.starts_with("output");

    let index = inner
        .split_once(':')
        .map(|(head, _)| head)
        .filter(|_| is_output || lower.starts_with("input"))
        .and_then(|head| head.split_whitespace().nth(1))
        .and_then(|n| n.parse::<u8>().ok());

    ChannelBank {
        header: inner.to_string(),
        index,
        is_output,
        ..Default::default()
    }
}

fn parse_preamp(line: &str) -> Option<f32> {
    let lower = line.to_ascii_lowercase();
    if !lower.starts_with("preamp") {
        return None;
    }
    // Tolerates "Preamp -6.5 dB" and REW's "Preamp: -6.5 dB".
    line[6..]
        .split(|c: char| c.is_whitespace() || c == ':')
        .filter(|s| !s.is_empty())
        .find_map(number)
}

/// Read a numeric token, tolerating a glued unit (`100Hz`) and either decimal
/// separator.
///
/// The separator rule is the Windows app's, so both ends agree on the same
/// file (`state_export_plan.md`, Phase 5): a token carrying both `.` and `,`
/// treats the **last** one as the decimal point and the other as thousands
/// grouping; a lone `,` is a decimal point. That reads `1.234,5` and `0,707`
/// and gives up only on a grouped integer like `1,000`, which neither app
/// writes.
fn number(token: &str) -> Option<f32> {
    let mut text = token.to_string();
    while let Some(last) = text.chars().last() {
        if last.is_ascii_digit() || last == '.' || last == ',' {
            break;
        }
        text.pop();
    }
    let dot = text.rfind('.');
    let comma = text.rfind(',');
    let text = match (dot, comma) {
        (Some(d), Some(c)) if c > d => text.replace('.', "").replace(',', "."),
        (Some(_), Some(_)) => text.replace(',', ""),
        (None, Some(_)) => text.replace(',', "."),
        _ => text,
    };
    text.parse::<f32>().ok()
}

/// Pair each non-numeric token with the number that follows it.
///
/// Units and any other trailing prose another tool emits are skipped rather
/// than misread, which is what lets one parser read REW, this app and the
/// Windows app.
fn labelled_values(tokens: &[&str]) -> Vec<(String, f32)> {
    let mut out = Vec::new();
    for pair in tokens.windows(2) {
        if number(pair[0]).is_some() {
            continue;
        }
        if let Some(v) = number(pair[1]) {
            out.push((pair[0].trim_end_matches(':').to_ascii_lowercase(), v));
        }
    }
    out
}

/// Compose a crossover type from the Windows spelling: a family token, an
/// `LP`/`HP` shape and a `Slope NN dB/oct`.
///
/// Regenerating each candidate from the shared table rather than transcribing
/// one means an order the firmware does not have (an odd-order Linkwitz-Riley)
/// simply fails to match instead of naming a type that is not there.
fn crossover_from_slope(
    family: &str,
    shape: Option<&str>,
    slope: Option<f32>,
) -> Option<FilterType> {
    let family = match family.to_ascii_uppercase().as_str() {
        "LR" => xover::Family::LinkwitzRiley,
        "BW" => xover::Family::Butterworth,
        // "BES" is this app's short name, "Bessel" the Windows one.
        "BES" | "BESSEL" => xover::Family::Bessel,
        _ => return None,
    };
    let high_pass = match shape?.to_ascii_uppercase().as_str() {
        "LP" => false,
        "HP" => true,
        _ => return None,
    };
    let slope = slope?;
    if slope < 6.0 {
        return None;
    }
    let order = (slope as u32 / 6) as u8;
    (xover::XOVER_FIRST..=xover::XOVER_LAST)
        .find(|raw| {
            xover::meta(*raw)
                .is_some_and(|m| m.family == family && m.order == order && m.high_pass == high_pass)
        })
        .map(FilterType::from_raw)
}

fn parse_band(line: &str) -> Option<(Bank, Band)> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let keyword = words.first()?;
    let bank = match keyword.to_ascii_lowercase().as_str() {
        "filter" => Bank::Peq,
        // `Xover` is this app's keyword, `Crossover` the Windows app's; a file
        // that lost its crossovers silently on the way across is worse than a
        // slightly wider parser.
        "xover" | "crossover" => Bank::Crossover,
        _ => return None,
    };

    // The index token, which may carry the colon.
    let _index = words.get(1)?;

    let state = words.get(2)?;
    if state.eq_ignore_ascii_case("off") || state.eq_ignore_ascii_case("none") {
        return Some((bank, Band::default()));
    }
    if !state.eq_ignore_ascii_case("on") {
        return None;
    }

    let code = words.get(3)?;
    let rest = &words[4..];
    let values = labelled_values(rest);
    let value_of = |name: &str| {
        values
            .iter()
            .rev()
            .find(|(l, _)| l == name)
            .map(|(_, v)| *v)
    };

    // A type this build does not know is left flat rather than guessed at, and
    // the caller can see it did not survive.
    let filter_type = match type_from_code(code) {
        Some(t) => t,
        None => crossover_from_slope(code, rest.first().copied(), value_of("slope"))?,
    };

    let mut band = Band {
        filter_type,
        // `[Bypassed]` is this app's marker, a bare `BYP` token the Windows
        // app's. Matched whole so a channel name cannot trip it.
        bypass: line.contains("[Bypassed]") || rest.iter().any(|t| t.eq_ignore_ascii_case("BYP")),
        ..Default::default()
    };

    for (label, v) in &values {
        match label.as_str() {
            "fc" | "freq" | "frequency" => band.freq = *v,
            "gain" => band.gain_db = *v,
            "q" => band.q = *v,
            // The Linkwitz Transform carries fp in the gain field on the wire,
            // so that is where it goes.
            "fp" => band.gain_db = *v,
            _ => {}
        }
    }

    Some((bank, band))
}

/// Which channel a file's section belongs to, before it is placed on a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelRef {
    Input(u8),
    Output(u8),
}

/// Resolve a DSPi Console for Windows channel header to a channel here.
///
/// The Windows export keys its sections by name rather than by index and always
/// writes the built-in default names (a user rename never reaches the file), so
/// this small table is the whole mapping (`state_export_plan.md`, Phase 5).
/// `None` for any header that is not one of them, including this app's own
/// index-keyed headers, which the caller has already tried.
pub fn windows_channel(header: &str, num_inputs: u8, pdm_output: u8) -> Option<ChannelRef> {
    let name = header.trim().to_ascii_uppercase();
    match name.as_str() {
        "MASTER L" => return Some(ChannelRef::Input(0)),
        "MASTER R" => return Some(ChannelRef::Input(1)),
        "PDM" => return Some(ChannelRef::Output(pdm_output)),
        _ => {}
    }

    // "Input N" numbers from 1, so input 3 is wire input 2. Inputs 1 and 2 are
    // spelled "Master L" and "Master R", never "Input 1".
    if let Some(n) = name.strip_prefix("INPUT ")
        && let Ok(index) = n.trim().parse::<u8>()
        && index > 2
        && index <= num_inputs
    {
        return Some(ChannelRef::Input(index - 1));
    }

    // "SPDIF k L" / "SPDIF k R": output pair k, left then right.
    if let Some(rest) = name.strip_prefix("SPDIF ") {
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if let [pair, side] = parts.as_slice()
            && let Ok(pair) = pair.parse::<u8>()
            && pair >= 1
        {
            return match *side {
                "L" => Some(ChannelRef::Output((pair - 1) * 2)),
                "R" => Some(ChannelRef::Output((pair - 1) * 2 + 1)),
                _ => None,
            };
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../../tests/fixtures/DSPiFilters.txt");
    const MACOS: &str = include_str!("../../../tests/fixtures/MacOSFilters.txt");
    const WINDOWS: &str = include_str!("../../../tests/fixtures/WindowsFilters.txt");
    const REW: &str = include_str!("../../../tests/fixtures/REWFilters.txt");

    fn band(t: FilterType, freq: f32, gain: f32, q: f32) -> Band {
        Band {
            filter_type: t,
            freq,
            q,
            gain_db: gain,
            bypass: false,
        }
    }

    /// The real export sitting in this repository, which is the only proof that
    /// the parser handles what the Console actually writes.
    #[test]
    fn the_real_fixture_parses() {
        let f = parse(FIXTURE).unwrap();
        assert_eq!(f.format_version, 1, "the fixture predates the format stamp");
        assert_eq!(f.channels.len(), 5, "Master L/R, Out L/R, Sub");

        let master_l = &f.channels[0];
        assert_eq!(master_l.header, "Master L");
        assert_eq!(master_l.peq.len(), 10);

        assert_eq!(master_l.peq[0].filter_type, FilterType::LowShelf);
        assert!((master_l.peq[0].freq - 105.0).abs() < 1e-3);
        assert!((master_l.peq[0].gain_db - 8.8).abs() < 1e-3);

        assert_eq!(master_l.peq[2].filter_type, FilterType::Peaking);
        assert!((master_l.peq[2].freq - 2856.0).abs() < 1e-3);
        assert!((master_l.peq[2].gain_db + 8.6).abs() < 1e-3);
        assert!((master_l.peq[2].q - 3.58).abs() < 1e-3);
    }

    #[test]
    fn the_fixtures_pass_filters_are_read() {
        let f = parse(FIXTURE).unwrap();
        let out_l = f.channels.iter().find(|c| c.header == "Out L").unwrap();
        assert_eq!(out_l.peq[0].filter_type, FilterType::HighPass);
        assert!((out_l.peq[0].freq - 80.0).abs() < 1e-3);

        let sub = f.channels.iter().find(|c| c.header == "Sub").unwrap();
        assert_eq!(sub.peq[0].filter_type, FilterType::LowPass);
        assert!((sub.peq[0].freq - 80.0).abs() < 1e-3);
    }

    /// Every PEQ type must survive a write and read, or an export quietly loses
    /// part of a tuning.
    #[test]
    fn every_peq_type_round_trips() {
        // 0..=13 at v1.1.6: config.h:884-891 lists FILTER_FLAT through
        // FILTER_HIGHPASS1, with the crossovers starting at 32.
        for raw in 0u8..=13 {
            let t = FilterType::from_raw(raw);
            if t == FilterType::Flat {
                continue;
            }
            let original = band(t, 2856.0, -8.6, 3.58);
            let line = format_band(Bank::Peq, 1, &original, Some(0.71));
            let (bank, parsed) = parse_band(line.trim())
                .unwrap_or_else(|| panic!("{t:?} produced a line that will not parse: {line}"));

            assert_eq!(bank, Bank::Peq);
            assert_eq!(parsed.filter_type, t, "type changed for {t:?}");
            assert!((parsed.freq - 2856.0).abs() < 0.1, "freq lost for {t:?}");
            if t.uses_gain() && !t.is_linkwitz() {
                assert!((parsed.gain_db + 8.6).abs() < 0.1, "gain lost for {t:?}");
            }
            if t.uses_q() {
                assert!((parsed.q - 3.58).abs() < 0.05, "Q lost for {t:?}");
            }
        }
    }

    /// The codes must be the Console's, or a file written here reads back on
    /// macOS as an unrecognised type and the band is silently dropped. Taken
    /// from `DSPi Console/DSPMath.swift:165-180` and its `init?(fileCode:)`.
    #[test]
    fn the_type_codes_are_the_ones_the_console_writes() {
        for (t, code) in [
            (FilterType::LowPass1, "LP1"),
            (FilterType::HighPass1, "HP1"),
            (FilterType::LowShelf1, "LS1"),
            (FilterType::HighShelf1, "HS1"),
            (FilterType::AllPass1, "AP1"),
        ] {
            assert_eq!(code_for(t).as_deref(), Some(code));
            assert_eq!(type_from_code(code), Some(t));
        }

        // REW's aliases for the second-order shelves. The Console reads them as
        // LS / HS, and so does `autoeq.rs`; reading them as the first-order
        // pair would halve the slope of every imported REW shelf.
        assert_eq!(type_from_code("LSC"), Some(FilterType::LowShelf));
        assert_eq!(type_from_code("HSC"), Some(FilterType::HighShelf));
    }

    #[test]
    fn every_crossover_type_round_trips() {
        for raw in xover::XOVER_FIRST..=xover::XOVER_LAST {
            let t = FilterType::from_raw(raw);
            let original = band(t, 2000.0, 0.0, 0.707);
            let line = format_band(Bank::Crossover, 1, &original, None);
            let (bank, parsed) = parse_band(line.trim())
                .unwrap_or_else(|| panic!("crossover {raw} will not parse: {line}"));

            assert_eq!(bank, Bank::Crossover);
            assert_eq!(parsed.filter_type.to_raw(), raw, "crossover {raw} changed");
            assert!((parsed.freq - 2000.0).abs() < 0.1);
        }
    }

    /// The Linkwitz Transform repurposes the wire fields, so it needs its own
    /// labels; sharing Gain and Q would not survive another tool reading it.
    #[test]
    fn the_linkwitz_transform_keeps_its_own_labels() {
        let b = band(FilterType::LinkwitzTransform, 40.0, 28.0, 0.7);
        let line = format_band(Bank::Peq, 4, &b, Some(0.71));
        assert!(line.contains("Fp"), "fp label missing: {line}");
        assert!(line.contains("Qp"), "qp label missing: {line}");
        assert!(!line.contains("Gain"), "fp must not be written as gain");

        let (_, parsed) = parse_band(line.trim()).unwrap();
        assert!((parsed.freq - 40.0).abs() < 0.1, "f0 lost");
        assert!((parsed.gain_db - 28.0).abs() < 0.1, "fp lost");
    }

    #[test]
    fn bypass_survives_a_round_trip() {
        let mut b = band(FilterType::Peaking, 1000.0, 3.0, 1.0);
        b.bypass = true;
        let line = format_band(Bank::Peq, 1, &b, None);
        assert!(line.contains("[Bypassed]"));
        assert!(parse_band(line.trim()).unwrap().1.bypass);
    }

    #[test]
    fn a_flat_band_writes_as_off_and_reads_back_flat() {
        let line = format_band(Bank::Peq, 5, &Band::default(), None);
        assert!(line.contains("OFF"), "{line}");
        let (_, parsed) = parse_band(line.trim()).unwrap();
        assert_eq!(parsed.filter_type, FilterType::Flat);
    }

    /// The grammar is a superset of REW's, so REW and AutoEQ exports must parse.
    #[test]
    fn rew_style_lines_are_accepted() {
        let rew = "\
Preamp: -6.5 dB
Filter 1: ON PK Fc 100 Hz Gain 3.0 dB Q 1.00
Filter 2: ON LS Fc 105 Hz Gain 8.8 dB
Filter 3: ON None
";
        let f = parse(&format!("[Input 0: In]\n{rew}")).unwrap();
        let ch = &f.channels[0];
        assert_eq!(ch.preamp_db, Some(-6.5));
        assert_eq!(ch.peq.len(), 2, "an unknown shape should not be invented");
        assert!((ch.peq[0].freq - 100.0).abs() < 1e-3);
    }

    #[test]
    fn extra_trailing_text_is_ignored_not_misread() {
        let line = "Filter  1: ON  PK      Fc   100.0 Hz  Gain  +3.0 dB  Q  1.00  (from REW v5)";
        let (_, b) = parse_band(line).unwrap();
        assert!((b.freq - 100.0).abs() < 1e-3);
        assert!((b.gain_db - 3.0).abs() < 1e-3);
        assert!((b.q - 1.0).abs() < 1e-3);
    }

    #[test]
    fn indexed_headers_are_understood() {
        let f = parse(
            "[Input 3: Turntable]\nFilter 1: OFF\n\n[Output 2: Sub (Enabled)]\nFilter 1: OFF\n",
        )
        .unwrap();
        assert_eq!(f.channels[0].index, Some(3));
        assert!(!f.channels[0].is_output);
        assert_eq!(f.channels[1].index, Some(2));
        assert!(f.channels[1].is_output);
    }

    #[test]
    fn a_whole_file_round_trips() {
        let original = FilterFile {
            format_version: FORMAT_VERSION,
            channels: vec![ChannelBank {
                header: "Input 0: USB 1".into(),
                index: Some(0),
                is_output: false,
                preamp_db: Some(-3.5),
                peq: vec![
                    band(FilterType::LowShelf, 105.0, 8.8, 0.707),
                    band(FilterType::Peaking, 2856.0, -8.6, 3.58),
                ],
                crossover: vec![band(FilterType::from_raw(34), 2000.0, 0.0, 0.707)],
            }],
        };

        let text = write(&original, "2026-07-27 12:00:00");
        let parsed = parse(&text).unwrap();

        assert_eq!(parsed.format_version, FORMAT_VERSION);
        assert_eq!(parsed.channels[0].preamp_db, Some(-3.5));
        assert_eq!(parsed.channels[0].peq.len(), 2);
        assert_eq!(parsed.channels[0].crossover.len(), 1);
        assert_eq!(
            parsed.channels[0].crossover[0].filter_type.to_raw(),
            34,
            "the crossover bank must stay separate from the PEQ bank"
        );
    }

    #[test]
    fn something_that_is_not_a_filter_file_is_refused() {
        assert_eq!(parse("hello\nworld\n"), Err(FilterFileError::NoChannels));
    }

    // -----------------------------------------------------------------------
    // The three dialects, against checked-in fixtures
    // -----------------------------------------------------------------------

    /// This app's own export: index-keyed headers, its own `Xover` keyword,
    /// `[Bypassed]`, and the Linkwitz Transform's separate labels.
    #[test]
    fn the_macos_fixture_round_trips() {
        let f = parse(MACOS).unwrap();
        assert_eq!(f.format_version, 2);
        assert_eq!(f.channels.len(), 4);

        let fl = &f.channels[0];
        assert_eq!(fl.index, Some(0));
        assert!(!fl.is_output);
        assert_eq!(fl.preamp_db, Some(-5.3));
        assert_eq!(fl.peq[2].filter_type, FilterType::Peaking);
        assert!(fl.peq[2].bypass, "[Bypassed] survives");
        assert_eq!(fl.peq[3].filter_type, FilterType::Flat, "OFF stays flat");

        let out = &f.channels[2];
        assert!(out.is_output);
        assert_eq!(out.index, Some(0));
        assert_eq!(out.peq[0].filter_type, FilterType::HighPass1);
        assert_eq!(out.peq[1].filter_type, FilterType::LinkwitzTransform);
        assert!((out.peq[1].freq - 40.0).abs() < 1e-3, "f0");
        assert!(
            (out.peq[1].gain_db - 28.0).abs() < 1e-3,
            "fp in the gain field"
        );
        assert_eq!(out.crossover.len(), 2);
        assert_eq!(out.crossover[0].filter_type.to_raw(), 35, "LR4HP");

        // Writing it back and reading it again keeps every band.
        let again = parse(&write(&f, "2026-03-14 09:21:04")).unwrap();
        assert_eq!(again.channels.len(), f.channels.len());
        assert_eq!(again.channels[2].crossover[0], out.crossover[0]);
        assert!(again.channels[0].peq[2].bypass);
    }

    /// The Windows dialect: `Crossover` with a family, a shape and a slope, a
    /// bare `BYP`, comma decimals with thousands grouping, and name-keyed
    /// headers. A file that lost its crossovers on the way across would be
    /// worse than refusing it outright.
    #[test]
    fn the_windows_fixture_reads_with_its_crossovers_and_bypass_intact() {
        let f = parse(WINDOWS).unwrap();
        assert_eq!(f.channels.len(), 5);

        let master_l = &f.channels[0];
        assert_eq!(master_l.header, "Master L");
        assert_eq!(master_l.preamp_db, Some(-6.5), "a comma decimal");
        assert!((master_l.peq[0].q - 0.7).abs() < 1e-3);
        assert!(
            (master_l.peq[1].freq - 1234.5).abs() < 1e-3,
            "1.234,5 is grouped, not 1.234"
        );
        assert_eq!(master_l.peq[2].filter_type, FilterType::Notch, "NO");
        assert!(master_l.peq[2].bypass, "a bare BYP token");
        assert_eq!(master_l.peq[3].filter_type, FilterType::Flat);

        let spdif = f.channels.iter().find(|c| c.header == "SPDIF 1 L").unwrap();
        assert_eq!(spdif.crossover.len(), 2, "Crossover is the same bank");
        assert_eq!(
            spdif.crossover[0].filter_type.to_raw(),
            35,
            "LR 24 dB/oct HP"
        );
        assert!((spdif.crossover[0].freq - 2000.0).abs() < 1e-3);
        assert_eq!(
            spdif.crossover[1].filter_type.to_raw(),
            42,
            "BW 12 dB/oct LP"
        );

        let pdm = f.channels.iter().find(|c| c.header == "PDM").unwrap();
        assert_eq!(pdm.crossover[0].filter_type.to_raw(), 58, "Bessel 4 LP");
    }

    /// Windows keys its sections by the firmware's default names, so the header
    /// itself is the channel map.
    #[test]
    fn windows_headers_name_the_channel() {
        assert_eq!(
            windows_channel("Master L", 8, 8),
            Some(ChannelRef::Input(0))
        );
        assert_eq!(
            windows_channel("master r", 8, 8),
            Some(ChannelRef::Input(1))
        );
        assert_eq!(windows_channel("Input 5", 8, 8), Some(ChannelRef::Input(4)));
        assert_eq!(
            windows_channel("Input 5", 2, 8),
            None,
            "a channel this device does not have"
        );
        assert_eq!(
            windows_channel("Input 1", 8, 8),
            None,
            "inputs 1 and 2 are spelled Master L and Master R"
        );
        assert_eq!(
            windows_channel("SPDIF 1 L", 8, 8),
            Some(ChannelRef::Output(0))
        );
        assert_eq!(
            windows_channel("SPDIF 2 R", 8, 8),
            Some(ChannelRef::Output(3))
        );
        assert_eq!(windows_channel("PDM", 8, 8), Some(ChannelRef::Output(8)));
        assert_eq!(windows_channel("Input 0: FL", 8, 8), None, "our own header");
    }

    /// An odd-order Linkwitz-Riley is not a filter the firmware has, so the
    /// composition must fail rather than name a type that is not there.
    #[test]
    fn an_impossible_crossover_slope_is_refused() {
        assert_eq!(crossover_from_slope("LR", Some("LP"), Some(18.0)), None);
        assert_eq!(
            crossover_from_slope("LR", Some("LP"), Some(24.0)).map(|t| t.to_raw()),
            Some(34)
        );
        assert_eq!(crossover_from_slope("WOBBLE", Some("LP"), Some(24.0)), None);
        assert_eq!(crossover_from_slope("BW", None, Some(12.0)), None);
        assert_eq!(crossover_from_slope("BW", Some("LP"), None), None);
    }

    /// A REW export has no section header at all; its bands are one unnamed
    /// channel for the person to place.
    #[test]
    fn a_headerless_rew_export_reads_as_one_channel() {
        let f = parse(REW).unwrap();
        assert_eq!(f.channels.len(), 1);
        let ch = &f.channels[0];
        assert!(ch.header.is_empty());
        assert_eq!(ch.index, None);
        assert_eq!(ch.preamp_db, Some(-6.5));
        assert_eq!(ch.peq.len(), 4, "the two `None` shapes are not invented");
        assert_eq!(ch.peq[2].filter_type, FilterType::LowShelf, "LSC");
        assert_eq!(ch.peq[3].filter_type, FilterType::HighShelf, "HSC");
        assert!((ch.peq[0].q - 3.5).abs() < 1e-3);
    }

    #[test]
    fn numbers_are_read_in_either_locale() {
        assert_eq!(number("100Hz"), Some(100.0));
        assert_eq!(number("0,707"), Some(0.707));
        assert_eq!(number("1.234,5"), Some(1234.5));
        assert_eq!(number("1,234.5"), Some(1234.5));
        assert_eq!(number("+8.8"), Some(8.8));
        assert_eq!(number("dB/oct"), None);
        assert_eq!(number("Fc"), None);
    }
}
