//! One command grammar, shared by the one-shot CLI and the TUI's `:` line.
//!
//! Both front-ends parse with the same code, so there is one syntax to learn and
//! one place it can drift. The grammar is derived from the parameter registry
//! rather than hand-written, which means a new firmware parameter becomes
//! typable, completable and documented with no work here.
//!
//! It also makes the TUI's echo line worth something: what it prints is exactly
//! what a shell would accept, so a user can copy a line out of the interface
//! into a script.

pub mod complete;
pub mod shell;

use dspi_proto::registry::{Kind, ParamDesc, Target, by_path};
use dspi_proto::value::Value;

pub use complete::{Candidate, complete};

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ParseError {
    #[error("no parameter or command called `{0}`. Try `dspi params`.")]
    Unknown(String),

    #[error("`{path}` needs {want} index(es) before the value; got {got}")]
    WrongArity {
        path: String,
        want: usize,
        got: usize,
    },

    #[error("`{0}` is not a channel on this device")]
    UnknownChannel(String),

    #[error("`{value}` is not valid for {path}; expected {expected}")]
    BadValue {
        path: String,
        value: String,
        expected: String,
    },

    #[error("`{0}` needs a value")]
    MissingValue(String),

    #[error("nothing to do")]
    Empty,
}

/// What the device looks like, for resolving names to indices.
///
/// Held separately rather than baked in, because channel naming is per-device: a
/// user who renamed a channel should be able to type their own name.
#[derive(Debug, Default, Clone)]
pub struct Context {
    /// Channel slugs, indexed by channel number.
    pub channel_slugs: Vec<String>,
    pub num_inputs: u8,
    pub num_outputs: u8,
    pub max_bands: u8,
}

impl Context {
    /// Resolve a channel argument: a device slug, a canonical form, or a number.
    pub fn channel(&self, token: &str) -> Option<u8> {
        let lower = token.to_ascii_lowercase();

        if let Some(i) = self.channel_slugs.iter().position(|s| *s == lower) {
            return Some(i as u8);
        }
        // `in.3` and `out.2` work even when the device uses its own names, so a
        // script is not tied to one machine's labelling.
        if let Some(n) = lower.strip_prefix("in.").and_then(|n| n.parse::<u8>().ok())
            && n >= 1
            && n <= self.num_inputs
        {
            return Some(n - 1);
        }
        if let Some(n) = lower
            .strip_prefix("out.")
            .and_then(|n| n.parse::<u8>().ok())
            && n >= 1
            && n <= self.num_outputs
        {
            return Some(self.num_inputs + n - 1);
        }
        token
            .parse::<u8>()
            .ok()
            .filter(|n| *n < self.num_inputs + self.num_outputs)
    }

    pub fn num_channels(&self) -> u8 {
        self.num_inputs + self.num_outputs
    }
}

/// A parsed command, ready to run or to render back as text.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Set {
        path: &'static str,
        indices: Vec<u8>,
        value: Value,
    },
    Get {
        path: &'static str,
        indices: Vec<u8>,
    },
    /// A whole EQ band in one go, which is also how the firmware stores it.
    SetBand {
        channel: u8,
        band: u8,
        filter_type: u8,
        freq: f32,
        q: f32,
        gain: f32,
    },
    /// A non-parameter verb: list, dump, doctor, and so on.
    Verb { name: String, args: Vec<String> },
}

/// Verbs that are not parameters.
pub const VERBS: &[(&str, &str)] = &[
    ("list", "List every connected DSPi"),
    ("dump", "Print the full device state"),
    ("params", "List every parameter"),
    ("get", "Read one parameter"),
    ("set", "Write one parameter"),
    ("eq", "Set a whole filter band at once"),
    ("doctor", "Diagnose connection problems"),
    ("completions", "Generate shell completions"),
];

/// Parse a token list into a command.
pub fn parse(tokens: &[&str], ctx: &Context) -> Result<Command, ParseError> {
    let Some(&first) = tokens.first() else {
        return Err(ParseError::Empty);
    };

    match first {
        "get" => parse_get(&tokens[1..], ctx),
        "set" => parse_set(&tokens[1..], ctx),
        "eq" => parse_eq(&tokens[1..], ctx),
        v if VERBS.iter().any(|(n, _)| *n == v) => Ok(Command::Verb {
            name: v.to_string(),
            args: tokens[1..].iter().map(|s| s.to_string()).collect(),
        }),
        // A bare path is a set, which is what makes `dspi vol.user -18` work.
        _ => parse_set(tokens, ctx),
    }
}

fn parse_get(tokens: &[&str], ctx: &Context) -> Result<Command, ParseError> {
    let Some(&path) = tokens.first() else {
        return Err(ParseError::MissingValue("get".into()));
    };
    let d = lookup(path)?;
    let indices = parse_indices(d, &tokens[1..], ctx)?;
    Ok(Command::Get {
        path: d.path,
        indices,
    })
}

fn parse_set(tokens: &[&str], ctx: &Context) -> Result<Command, ParseError> {
    let Some(&path) = tokens.first() else {
        return Err(ParseError::Empty);
    };
    let d = lookup(path)?;
    let rest = &tokens[1..];
    let want = d.target.arity();

    // A trigger takes indices but no value, so it is the one shape where the
    // last token is still an index.
    if matches!(d.kind, Kind::Trigger) {
        let indices = parse_indices(d, rest, ctx)?;
        return Ok(Command::Set {
            path: d.path,
            indices,
            value: Value::Trigger,
        });
    }

    if rest.len() < want + 1 {
        return Err(ParseError::WrongArity {
            path: d.path.into(),
            want,
            got: rest.len().saturating_sub(want.min(rest.len())),
        });
    }

    let indices = parse_indices(d, &rest[..want], ctx)?;

    // A crosspoint is a packet whose first two bytes are the indices, so it is
    // assembled here where they are known rather than in `parse_value`.
    if d.path == "mix" {
        return Ok(Command::Set {
            path: d.path,
            value: parse_crosspoint(&indices, &rest[want..])?,
            indices,
        });
    }

    let raw = rest[want..].join(" ");
    let value = parse_value(d, &raw)?;

    Ok(Command::Set {
        path: d.path,
        indices,
        value,
    })
}

/// A crossover type by its file code: `lr4lp`, `bw2hp`, `bes4lp`. The codes
/// are regenerated from `dspi_proto::xover` so this table cannot drift from
/// the one the filter files use.
fn crossover_from_token(token: &str) -> Option<u8> {
    let want = token.to_ascii_lowercase();
    (dspi_proto::xover::XOVER_FIRST..=dspi_proto::xover::XOVER_LAST)
        .find(|t| crossover_token(*t).as_deref() == Some(want.as_str()))
}

fn crossover_token(raw: u8) -> Option<String> {
    dspi_proto::xover::meta(raw).map(|m| {
        format!(
            "{}{}{}",
            m.family.short(),
            m.order,
            if m.high_pass { "hp" } else { "lp" }
        )
    })
}

/// `mix <input> <output> on|off [gain dB] [inv]`
///
/// The 8-byte `MatrixRoutePacket` (config.h:845-851): input, output, enabled,
/// phase_invert, then the gain as a little-endian float.
fn parse_crosspoint(indices: &[u8], tokens: &[&str]) -> Result<Value, ParseError> {
    let bad = |value: &str, expected: &str| ParseError::BadValue {
        path: "mix".into(),
        value: value.into(),
        expected: expected.into(),
    };
    let Some(&on) = tokens.first() else {
        return Err(ParseError::MissingValue("mix".into()));
    };
    let enabled = match on.to_ascii_lowercase().as_str() {
        "on" | "true" | "yes" | "1" => true,
        "off" | "false" | "no" | "0" => false,
        other => return Err(bad(other, "on or off")),
    };
    let gain: f32 = match tokens.get(1) {
        None => 0.0,
        Some(t) => t.parse().map_err(|_| bad(t, "a gain in dB"))?,
    };
    let invert = match tokens.get(2) {
        None => false,
        Some(t) => match t.to_ascii_lowercase().as_str() {
            "inv" | "invert" | "inverted" => true,
            "norm" | "normal" => false,
            other => return Err(bad(other, "inv or norm")),
        },
    };
    let mut bytes = vec![indices[0], indices[1], enabled as u8, invert as u8];
    bytes.extend_from_slice(&gain.to_le_bytes());
    Ok(Value::Bytes(bytes))
}

/// A crosspoint packet back as the line that would produce it.
fn format_crosspoint(bytes: &[u8]) -> String {
    if bytes.len() < 8 {
        return String::new();
    }
    let gain = f32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let mut s = format!(
        "{} {}",
        if bytes[2] != 0 { "on" } else { "off" },
        format_value("out.gain", &Value::Float(gain))
    );
    if bytes[3] != 0 {
        s.push_str(" inv");
    }
    s
}

/// `eq <channel> <band> <type> [freq] [q] [gain]`
///
/// Writing a band in one command matches how the firmware stores it, so it is
/// both fewer transfers and impossible to leave half-applied.
fn parse_eq(tokens: &[&str], ctx: &Context) -> Result<Command, ParseError> {
    if tokens.len() < 3 {
        return Err(ParseError::WrongArity {
            path: "eq".into(),
            want: 3,
            got: tokens.len(),
        });
    }

    let channel = ctx
        .channel(tokens[0])
        .ok_or_else(|| ParseError::UnknownChannel(tokens[0].into()))?;
    let band = parse_band(tokens[1], ctx)?;

    let type_desc = by_path("eq.type").expect("eq.type is in the registry");
    // The registry's choice list covers the PEQ types; the crossover types
    // occupy 32 to 63 and are named by family, order and direction instead.
    let filter_type = match crossover_from_token(tokens[2]) {
        Some(raw) => raw,
        None => parse_value(type_desc, tokens[2])?
            .as_u8()
            .ok_or_else(|| ParseError::BadValue {
                path: "eq.type".into(),
                value: tokens[2].into(),
                expected: "a filter type".into(),
            })?,
    };

    let num = |i: usize, default: f32| -> Result<f32, ParseError> {
        match tokens.get(i) {
            None => Ok(default),
            Some(t) => t.parse::<f32>().map_err(|_| ParseError::BadValue {
                path: "eq".into(),
                value: (*t).into(),
                expected: "a number".into(),
            }),
        }
    };

    Ok(Command::SetBand {
        channel,
        band,
        filter_type,
        freq: num(3, 1000.0)?,
        q: num(4, 0.707)?,
        gain: num(5, 0.0)?,
    })
}

fn lookup(path: &str) -> Result<&'static ParamDesc, ParseError> {
    by_path(path).ok_or_else(|| ParseError::Unknown(path.into()))
}

fn parse_indices(d: &ParamDesc, tokens: &[&str], ctx: &Context) -> Result<Vec<u8>, ParseError> {
    let want = d.target.arity();
    if tokens.len() < want {
        return Err(ParseError::WrongArity {
            path: d.path.into(),
            want,
            got: tokens.len(),
        });
    }

    let mut out = Vec::with_capacity(want);
    for (i, tok) in tokens.iter().take(want).enumerate() {
        let v = match (d.target, i) {
            // Channel-shaped positions accept device names.
            (Target::Channel, 0) | (Target::ChannelBand, 0) => ctx
                .channel(tok)
                .ok_or_else(|| ParseError::UnknownChannel((*tok).into()))?,
            (Target::ChannelBand, 1) => parse_band(tok, ctx)?,
            _ => tok.parse::<u8>().map_err(|_| ParseError::BadValue {
                path: d.path.into(),
                value: (*tok).into(),
                expected: "an index".into(),
            })?,
        };
        out.push(v);
    }
    Ok(out)
}

/// Bands are 1-based for humans and 0-based on the wire, except the crossover
/// bands, which the firmware addresses as 20-23 and which we accept verbatim.
fn parse_band(token: &str, ctx: &Context) -> Result<u8, ParseError> {
    let n: u16 = token.parse().map_err(|_| ParseError::BadValue {
        path: "band".into(),
        value: token.into(),
        expected: "a band number".into(),
    })?;

    if (20..24).contains(&n) {
        return Ok(n as u8);
    }
    // The live count, not a floor of 10: on a device with fewer bands, `.max(10)`
    // would accept numbers the firmware then stalls on.
    let top = ctx.max_bands as u16;
    if n >= 1 && n <= top {
        return Ok((n - 1) as u8);
    }
    Err(ParseError::BadValue {
        path: "band".into(),
        value: token.into(),
        expected: format!("1 to {top}, or 20 to 23 for crossover"),
    })
}

pub fn parse_value(d: &ParamDesc, raw: &str) -> Result<Value, ParseError> {
    let bad = |expected: &str| ParseError::BadValue {
        path: d.path.into(),
        value: raw.into(),
        expected: expected.into(),
    };

    Ok(match d.kind {
        Kind::Bool => match raw.to_ascii_lowercase().as_str() {
            "on" | "true" | "yes" | "1" | "enabled" => Value::Bool(true),
            "off" | "false" | "no" | "0" | "disabled" => Value::Bool(false),
            _ => return Err(bad("on or off")),
        },
        Kind::Trigger => Value::Trigger,
        Kind::Choice(variants) => {
            let lower = raw.to_ascii_lowercase();
            match variants.iter().find(|(_, n)| *n == lower) {
                Some((v, _)) => Value::Choice(*v),
                None => match raw.parse::<u8>() {
                    Ok(n) if variants.iter().any(|(v, _)| *v == n) => Value::Choice(n),
                    _ => {
                        let names: Vec<&str> = variants.iter().map(|(_, n)| *n).collect();
                        return Err(bad(&names.join(", ")));
                    }
                },
            }
        }
        Kind::Text { .. } => Value::Text(raw.trim_matches('"').to_string()),
        Kind::Int { .. } => Value::Int(raw.parse().map_err(|_| bad("a whole number"))?),
        Kind::Mask => Value::Mask(match raw.strip_prefix("0x") {
            Some(hex) => u32::from_str_radix(hex, 16).map_err(|_| bad("a hex mask"))?,
            None => raw.parse().map_err(|_| bad("a mask"))?,
        }),
        _ => Value::Float(raw.parse().map_err(|_| bad("a number"))?),
    })
}

/// Render a command back as canonical text.
///
/// This is what the TUI echoes after a UI action, and it must be re-parseable:
/// the round-trip is what lets a user copy a line out of the interface and paste
/// it into a shell script.
pub fn format(cmd: &Command, ctx: &Context) -> String {
    match cmd {
        Command::Get { path, indices } => {
            let mut s = format!("get {path}");
            for (i, idx) in indices.iter().enumerate() {
                s.push(' ');
                s.push_str(&format_index(path, i, *idx, ctx));
            }
            s
        }
        Command::Set {
            path,
            indices,
            value: Value::Bytes(bytes),
        } if *path == "mix" => {
            format!(
                "mix {} {} {}",
                indices.first().copied().unwrap_or(0),
                indices.get(1).copied().unwrap_or(0),
                format_crosspoint(bytes)
            )
        }
        Command::Set {
            path,
            indices,
            value,
        } => {
            let mut s = (*path).to_string();
            for (i, idx) in indices.iter().enumerate() {
                s.push(' ');
                s.push_str(&format_index(path, i, *idx, ctx));
            }
            if !matches!(value, Value::Trigger) {
                s.push(' ');
                s.push_str(&format_value(path, value));
            }
            s
        }
        Command::SetBand {
            channel,
            band,
            filter_type,
            freq,
            q,
            gain,
        } => {
            let ty = crossover_token(*filter_type).unwrap_or_else(|| {
                by_path("eq.type")
                    .and_then(|d| match d.kind {
                        Kind::Choice(v) => v
                            .iter()
                            .find(|(raw, _)| raw == filter_type)
                            .map(|(_, n)| *n),
                        _ => None,
                    })
                    .unwrap_or("?")
                    .to_string()
            });
            format!(
                "eq {} {} {ty} {freq} {q} {gain}",
                ctx.channel_slugs
                    .get(*channel as usize)
                    .cloned()
                    .unwrap_or_else(|| channel.to_string()),
                display_band(*band),
            )
        }
        Command::Verb { name, args } => {
            if args.is_empty() {
                name.clone()
            } else {
                format!("{name} {}", args.join(" "))
            }
        }
    }
}

fn format_index(path: &str, position: usize, index: u8, ctx: &Context) -> String {
    let Some(d) = by_path(path) else {
        return index.to_string();
    };
    match (d.target, position) {
        (Target::Channel, 0) | (Target::ChannelBand, 0) => ctx
            .channel_slugs
            .get(index as usize)
            .cloned()
            .unwrap_or_else(|| index.to_string()),
        (Target::ChannelBand, 1) => display_band(index),
        _ => index.to_string(),
    }
}

fn display_band(band: u8) -> String {
    if (20..24).contains(&band) {
        band.to_string()
    } else {
        (band + 1).to_string()
    }
}

fn format_value(path: &str, v: &Value) -> String {
    let Some(d) = by_path(path) else {
        return v.display(dspi_proto::value::Unit::None);
    };
    match (d.kind, v) {
        (Kind::Choice(variants), _) => match v.as_u8().and_then(|n| {
            variants
                .iter()
                .find(|(raw, _)| *raw == n)
                .map(|(_, name)| *name)
        }) {
            Some(name) => name.to_string(),
            None => v.as_u8().unwrap_or(0).to_string(),
        },
        (Kind::Bool, Value::Bool(b)) => (if *b { "on" } else { "off" }).into(),
        (Kind::Text { .. }, Value::Text(t)) => {
            if t.contains(' ') {
                format!("\"{t}\"")
            } else {
                t.clone()
            }
        }
        // The echo is meant to be pasted into a shell, so it needs to read as
        // a value someone would type. Full float precision is technically
        // faithful and practically noise.
        (_, Value::Float(f)) => {
            let rounded = (f * 100.0).round() / 100.0;
            if rounded.fract() == 0.0 {
                format!("{rounded:.0}")
            } else {
                format!("{rounded}")
            }
        }
        (_, Value::Int(i)) => format!("{i}"),
        (_, Value::Mask(m)) => format!("0x{m:X}"),
        _ => v.display(d.kind.unit()),
    }
}

/// Split a line into tokens, honouring double quotes so names with spaces work.
pub fn tokenize(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;

    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn ctx() -> Context {
        Context {
            channel_slugs: (0..17)
                .map(|i| {
                    if i < 8 {
                        format!("usb.{}", i + 1)
                    } else if i == 16 {
                        "pdm".to_string()
                    } else {
                        format!(
                            "i2s.{}.{}",
                            (i - 8) / 2 + 1,
                            if i % 2 == 0 { "l" } else { "r" }
                        )
                    }
                })
                .collect(),
            num_inputs: 8,
            num_outputs: 9,
            // The live count, as a real device reports it.
            max_bands: 10,
        }
    }

    fn p(line: &str) -> Result<Command, ParseError> {
        let toks = tokenize(line);
        let refs: Vec<&str> = toks.iter().map(String::as_str).collect();
        parse(&refs, &ctx())
    }

    #[test]
    fn a_bare_path_is_a_set() {
        assert_eq!(
            p("vol.user -18").unwrap(),
            Command::Set {
                path: "vol.user",
                indices: vec![],
                value: Value::Float(-18.0)
            }
        );
    }

    /// A crosspoint is the one packet the interface writes constantly, so it
    /// has to be typable and it has to round-trip through the echo line.
    #[test]
    fn a_crosspoint_takes_its_state_gain_and_polarity() {
        let cmd = p("mix 0 8 on -6 inv").unwrap();
        match &cmd {
            Command::Set {
                path: "mix",
                indices,
                value: Value::Bytes(b),
            } => {
                assert_eq!(indices, &vec![0, 8]);
                assert_eq!(b[..4], [0, 8, 1, 1]);
                assert_eq!(f32::from_le_bytes([b[4], b[5], b[6], b[7]]), -6.0);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(format(&cmd, &ctx()), "mix 0 8 on -6 inv");
        // The gain and the polarity are optional.
        match p("mix 1 2 off").unwrap() {
            Command::Set {
                value: Value::Bytes(b),
                ..
            } => assert_eq!(b, vec![1, 2, 0, 0, 0, 0, 0, 0]),
            other => panic!("{other:?}"),
        }
        assert!(p("mix 0 1 maybe").is_err());
        assert!(p("mix 0 1 on 0 sideways").is_err());
    }

    /// The crossover types live outside the registry's choice list, since the
    /// UI picks them by family, order and direction rather than by number.
    #[test]
    fn a_crossover_band_is_named_by_its_family_and_order() {
        let cmd = p("eq pdm 20 lr4hp 80").unwrap();
        match &cmd {
            Command::SetBand {
                channel,
                band,
                filter_type,
                freq,
                ..
            } => {
                assert_eq!((*channel, *band, *filter_type, *freq), (16, 20, 35, 80.0));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(format(&cmd, &ctx()), "eq pdm 20 lr4hp 80 0.707 0");
        assert_eq!(
            p("eq pdm 21 BES8LP 120").unwrap(),
            p("eq pdm 21 bes8lp 120").unwrap(),
            "the codes are case-insensitive"
        );
        assert!(p("eq pdm 20 lr3lp 80").is_err(), "LR has no third order");
    }

    #[test]
    fn explicit_set_and_get_both_work() {
        assert!(matches!(p("set vol.user -18"), Ok(Command::Set { .. })));
        assert!(matches!(p("get vol.user"), Ok(Command::Get { .. })));
    }

    #[test]
    fn booleans_accept_the_words_people_type() {
        for word in ["on", "true", "yes", "1", "enabled"] {
            match p(&format!("bass.on {word}")).unwrap() {
                Command::Set { value, .. } => assert_eq!(value, Value::Bool(true)),
                other => panic!("{other:?}"),
            }
        }
        for word in ["off", "false", "no", "0", "disabled"] {
            match p(&format!("bass.on {word}")).unwrap() {
                Command::Set { value, .. } => assert_eq!(value, Value::Bool(false)),
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn choices_accept_names_and_numbers() {
        match p("in.source spdif").unwrap() {
            Command::Set { value, .. } => assert_eq!(value, Value::Choice(1)),
            other => panic!("{other:?}"),
        }
        match p("in.source 1").unwrap() {
            Command::Set { value, .. } => assert_eq!(value, Value::Choice(1)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_bad_choice_lists_the_alternatives() {
        let e = p("in.source banana").unwrap_err();
        let msg = e.to_string();
        assert!(msg.contains("usb"), "should list the options: {msg}");
        assert!(msg.contains("spdif"), "{msg}");
    }

    /// Device names are what the user sees, so they must be what the user types.
    #[test]
    fn channels_resolve_by_device_name() {
        match p("ch.delay i2s.1.l 5").unwrap() {
            Command::Set { indices, .. } => assert_eq!(indices, vec![8]),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn channels_also_resolve_by_canonical_form_and_number() {
        // out.1 is the first output, whatever the device calls it.
        match p("ch.delay out.1 5").unwrap() {
            Command::Set { indices, .. } => assert_eq!(indices, vec![8]),
            other => panic!("{other:?}"),
        }
        match p("ch.delay in.3 5").unwrap() {
            Command::Set { indices, .. } => assert_eq!(indices, vec![2]),
            other => panic!("{other:?}"),
        }
        match p("ch.delay 8 5").unwrap() {
            Command::Set { indices, .. } => assert_eq!(indices, vec![8]),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_unknown_channel_is_named_in_the_error() {
        let e = p("ch.delay nonsense 5").unwrap_err();
        assert!(e.to_string().contains("nonsense"));
    }

    /// Humans count bands from one; the wire counts from zero. Crossover bands
    /// keep their wire numbers because that is how they are documented.
    #[test]
    fn bands_are_one_based_except_crossovers() {
        match p("eq.freq i2s.1.l 1 2856").unwrap() {
            Command::Set { indices, .. } => assert_eq!(indices, vec![8, 0]),
            other => panic!("{other:?}"),
        }
        match p("eq.freq i2s.1.l 20 80").unwrap() {
            Command::Set { indices, .. } => assert_eq!(indices, vec![8, 20]),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn band_zero_is_rejected_rather_than_wrapping() {
        assert!(p("eq.freq i2s.1.l 0 2856").is_err());
    }

    /// The top of the range is the other end of the same off-by-one: the last
    /// band a user can type must reach the last band the firmware has.
    #[test]
    fn the_last_band_is_reachable_and_the_next_one_is_not() {
        let top = ctx().max_bands; // 10
        match p(&format!("eq.freq usb.1 {top} 2856")).unwrap() {
            Command::Set { indices, .. } => assert_eq!(indices, vec![0, top - 1]),
            other => panic!("{other:?}"),
        }
        assert!(
            p(&format!("eq.freq usb.1 {} 2856", top + 1)).is_err(),
            "band {} is past the live count",
            top + 1
        );
    }

    /// The range follows the device rather than a constant, so a part with
    /// fewer bands does not accept numbers the firmware then stalls on.
    #[test]
    fn the_band_range_tracks_the_device() {
        let small = Context {
            max_bands: 4,
            ..ctx()
        };
        let parse = |line: &str| {
            let toks = tokenize(line);
            let refs: Vec<&str> = toks.iter().map(String::as_str).collect();
            super::parse(&refs, &small)
        };
        assert!(parse("eq.freq usb.1 4 2856").is_ok());
        assert!(parse("eq.freq usb.1 5 2856").is_err());
    }

    /// What the echo line prints has to be what the parser accepts, or copying a
    /// line out of the interface into a shell silently edits a different band.
    #[test]
    fn a_formatted_band_parses_back_to_the_same_band() {
        for typed in [1u8, 5, 10, 20, 23] {
            let cmd = p(&format!("eq.freq usb.1 {typed} 2856")).unwrap();
            let text = format(&cmd, &ctx());
            assert!(
                text.contains(&format!(" {typed} ")),
                "band {typed} came back as `{text}`"
            );
            assert_eq!(p(&text).unwrap(), cmd, "`{text}` did not round-trip");
        }
    }

    #[test]
    fn the_compound_eq_form_sets_a_whole_band() {
        assert_eq!(
            p("eq usb.1 3 peak 2856 3.58 -8.6").unwrap(),
            Command::SetBand {
                channel: 0,
                band: 2,
                filter_type: 1,
                freq: 2856.0,
                q: 3.58,
                gain: -8.6,
            }
        );
    }

    #[test]
    fn the_compound_form_defaults_the_optional_fields() {
        match p("eq usb.1 1 highpass 80").unwrap() {
            Command::SetBand { freq, q, gain, .. } => {
                assert_eq!(freq, 80.0);
                assert_eq!(q, 0.707);
                assert_eq!(gain, 0.0);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn missing_values_are_reported_not_guessed() {
        let e = p("vol.user").unwrap_err();
        assert!(matches!(e, ParseError::WrongArity { .. }), "{e}");
    }

    #[test]
    fn triggers_take_indices_but_no_value() {
        assert_eq!(
            p("preset.save 3").unwrap(),
            Command::Set {
                path: "preset.save",
                indices: vec![3],
                value: Value::Trigger
            }
        );
    }

    #[test]
    fn quoted_text_survives_tokenizing() {
        assert_eq!(
            tokenize(r#"preset.name 3 "Living Room""#),
            vec!["preset.name", "3", "Living Room"]
        );
        match p(r#"preset.name 3 "Living Room""#).unwrap() {
            Command::Set { value, .. } => assert_eq!(value, Value::Text("Living Room".into())),
            other => panic!("{other:?}"),
        }
    }

    /// The echo line has to produce something a shell will accept, or it teaches
    /// a syntax that does not work.
    #[test]
    fn formatting_round_trips_through_the_parser() {
        let ctx = ctx();
        for line in [
            "vol.user -18",
            "bass.on on",
            "in.source spdif",
            "ch.delay i2s.1.l 5",
            "eq.freq i2s.1.l 3 2856",
            "eq.freq i2s.1.l 20 80",
            "preset.save 3",
            "eq usb.1 3 peak 2856 3.58 -8.6",
        ] {
            let cmd = p(line).unwrap();
            let rendered = format(&cmd, &ctx);
            let toks = tokenize(&rendered);
            let refs: Vec<&str> = toks.iter().map(String::as_str).collect();
            let reparsed = parse(&refs, &ctx).unwrap_or_else(|e| {
                panic!("`{line}` rendered as `{rendered}`, which does not parse: {e}")
            });
            assert_eq!(cmd, reparsed, "`{line}` did not survive the round trip");
        }
    }

    #[test]
    fn formatting_quotes_names_with_spaces() {
        let ctx = ctx();
        let cmd = p(r#"preset.name 3 "Living Room""#).unwrap();
        assert_eq!(format(&cmd, &ctx), r#"preset.name 3 "Living Room""#);
    }

    #[test]
    fn verbs_pass_through() {
        assert_eq!(
            p("list").unwrap(),
            Command::Verb {
                name: "list".into(),
                args: vec![]
            }
        );
    }

    #[test]
    fn an_unknown_word_suggests_where_to_look() {
        let e = p("wobble 3").unwrap_err();
        assert!(e.to_string().contains("dspi params"), "{e}");
    }
}

#[cfg(test)]
mod echo_tests {
    use super::*;
    use crate::tests::ctx;

    fn rendered(path: &'static str, indices: Vec<u8>, value: Value) -> String {
        format(
            &Command::Set {
                path,
                indices,
                value,
            },
            &ctx(),
        )
    }

    /// The echo exists to be pasted into a shell, so it has to read as a value
    /// someone would type.
    #[test]
    fn floats_are_rounded_to_something_typeable() {
        assert_eq!(
            rendered("eq.freq", vec![0, 0], Value::Float(111.243_63)),
            "eq.freq usb.1 1 111.24"
        );
        assert_eq!(
            rendered("vol.user", vec![], Value::Float(-18.0)),
            "vol.user -18"
        );
        assert_eq!(
            rendered("eq.q", vec![0, 0], Value::Float(3.58)),
            "eq.q usb.1 1 3.58"
        );
    }

    /// Rounding must not break the round trip, or the echo teaches a line that
    /// produces a different result than the one it describes.
    #[test]
    fn a_rounded_echo_still_parses_back() {
        let line = rendered("eq.freq", vec![0, 0], Value::Float(111.243_63));
        let toks = tokenize(&line);
        let refs: Vec<&str> = toks.iter().map(String::as_str).collect();
        match parse(&refs, &ctx()).unwrap() {
            Command::Set { value, indices, .. } => {
                assert_eq!(indices, vec![0, 0]);
                assert_eq!(value, Value::Float(111.24));
            }
            other => panic!("{other:?}"),
        }
    }
}
