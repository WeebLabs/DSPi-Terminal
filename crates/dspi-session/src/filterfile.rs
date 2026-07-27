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
        "LS" => Some(FilterType::LowShelf),
        "HS" => Some(FilterType::HighShelf),
        "LP" | "LPQ" => Some(FilterType::LowPass),
        "HP" | "HPQ" => Some(FilterType::HighPass),
        "NT" | "NO" | "NOTCH" => Some(FilterType::Notch),
        "AP" => Some(FilterType::AllPass),
        "AP1" => Some(FilterType::AllPass1),
        "LS1" | "LSC" => Some(FilterType::LowShelf1),
        "HS1" | "HSC" => Some(FilterType::HighShelf1),
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
fn parse_header(inner: &str) -> ChannelBank {
    let lower = inner.to_ascii_lowercase();
    let is_output = lower.starts_with("output");

    let index = inner
        .split(':')
        .next()
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
        .find_map(|t| t.parse::<f32>().ok())
}

fn parse_band(line: &str) -> Option<(Bank, Band)> {
    let mut words = line.split_whitespace();
    let keyword = words.next()?;
    let bank = match keyword.to_ascii_lowercase().as_str() {
        "filter" => Bank::Peq,
        "xover" => Bank::Crossover,
        _ => return None,
    };

    // The index token, which may carry the colon.
    let _index = words.next()?;

    let state = words.next()?;
    if state.eq_ignore_ascii_case("off") || state.eq_ignore_ascii_case("none") {
        return Some((bank, Band::default()));
    }
    if !state.eq_ignore_ascii_case("on") {
        return None;
    }

    let code = words.next()?;
    // A type this build does not know is left flat rather than guessed at, and
    // the caller can see it did not survive.
    let filter_type = type_from_code(code)?;

    let mut band = Band {
        filter_type,
        bypass: line.contains("[Bypassed]"),
        ..Default::default()
    };

    // Everything after the type is label/value pairs, so extra text from other
    // tools is ignored rather than misread.
    let rest: Vec<&str> = words.collect();
    let mut i = 0;
    while i < rest.len() {
        let label = rest[i].trim_end_matches(':').to_ascii_lowercase();
        let value = rest.get(i + 1).and_then(|v| v.parse::<f32>().ok());
        if let Some(v) = value {
            match label.as_str() {
                "fc" | "freq" | "frequency" => band.freq = v,
                "gain" => band.gain_db = v,
                "q" => band.q = v,
                // The Linkwitz Transform carries fp in the gain field on the
                // wire, so that is where it goes.
                "fp" => band.gain_db = v,
                "qp" => {}
                _ => {}
            }
            i += 2;
        } else {
            i += 1;
        }
    }

    Some((bank, band))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../../tests/fixtures/DSPiFilters.txt");

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
        for raw in 0u8..=11 {
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
}
