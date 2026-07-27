//! Completion, at any position in a command.
//!
//! This is the feature that lets someone drive the device without knowing a
//! single command: at every point there is a list of legal next tokens with what
//! each one means. It is a walk of the parameter registry, not a word list, so
//! it cannot fall out of step with what the device actually accepts.
//!
//! The same function serves the TUI's `:` line and, indirectly, the generated
//! shell completions, so the two can never disagree.

use dspi_proto::registry::{Kind, REGISTRY, Target, by_path};

use crate::{Context, VERBS};

/// One suggestion, with enough context to render a useful menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The text to insert.
    pub value: String,
    /// A short description shown beside it.
    pub detail: String,
    /// What sort of thing this is, for grouping and icons.
    pub kind: CandidateKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateKind {
    Verb,
    Parameter,
    Channel,
    Band,
    Choice,
    /// A hint rather than something to insert, e.g. a numeric range.
    Hint,
}

impl Candidate {
    fn new(value: impl Into<String>, detail: impl Into<String>, kind: CandidateKind) -> Self {
        Self {
            value: value.into(),
            detail: detail.into(),
            kind,
        }
    }
}

/// Suggest what could come next.
///
/// `tokens` are the complete tokens already typed; `partial` is the fragment the
/// cursor is currently in (empty when the user has just typed a space).
pub fn complete(tokens: &[&str], partial: &str, ctx: &Context) -> Vec<Candidate> {
    let lower = partial.to_ascii_lowercase();
    let keep = |c: &Candidate| c.value.to_ascii_lowercase().starts_with(&lower);

    // Nothing typed yet: offer verbs and every parameter.
    if tokens.is_empty() {
        let mut out: Vec<Candidate> = VERBS
            .iter()
            .map(|(n, d)| Candidate::new(*n, *d, CandidateKind::Verb))
            .collect();
        out.extend(parameter_candidates());
        out.retain(keep);
        return out;
    }

    // `get`/`set` shift the frame by one; everything else is a bare path.
    let (head, rest) = match tokens[0] {
        "get" | "set" => {
            if tokens.len() == 1 {
                let mut out = parameter_candidates();
                out.retain(keep);
                return out;
            }
            (tokens[1], &tokens[2..])
        }
        "eq" => return complete_eq(&tokens[1..], partial, ctx),
        "completions" => {
            let mut out: Vec<Candidate> = ["bash", "zsh", "fish", "powershell"]
                .iter()
                .map(|s| Candidate::new(*s, "shell", CandidateKind::Choice))
                .collect();
            out.retain(keep);
            return out;
        }
        other => (other, &tokens[1..]),
    };

    let Some(d) = by_path(head) else {
        return Vec::new();
    };

    let arity = d.target.arity();
    let position = rest.len();

    // Still filling in indices.
    if position < arity {
        let mut out = match (d.target, position) {
            (Target::Channel, 0) | (Target::ChannelBand, 0) => channel_candidates(ctx),
            (Target::ChannelBand, 1) => band_candidates(ctx),
            (Target::Output, 0) => index_candidates(ctx.num_outputs, "output"),
            (Target::Input, 0) => index_candidates(ctx.num_inputs, "input"),
            (Target::Crosspoint, 0) => index_candidates(ctx.num_inputs, "input"),
            (Target::Crosspoint, 1) => index_candidates(ctx.num_outputs, "output"),
            (Target::PresetSlot, 0) => index_candidates(10, "preset slot"),
            (Target::LegacyChannel, 0) => index_candidates(3, "channel"),
            _ => vec![Candidate::new(
                "",
                format!("an index for {}", d.label),
                CandidateKind::Hint,
            )],
        };
        out.retain(|c| c.kind == CandidateKind::Hint || keep(c));
        return out;
    }

    // On to the value.
    if position == arity {
        let mut out = value_candidates(d);
        out.retain(|c| c.kind == CandidateKind::Hint || keep(c));
        return out;
    }

    Vec::new()
}

fn parameter_candidates() -> Vec<Candidate> {
    REGISTRY
        .iter()
        .filter(|d| d.set.is_some() || d.get.is_some())
        .map(|d| Candidate::new(d.path, d.plain, CandidateKind::Parameter))
        .collect()
}

fn channel_candidates(ctx: &Context) -> Vec<Candidate> {
    ctx.channel_slugs
        .iter()
        .enumerate()
        .map(|(i, slug)| {
            let what = if (i as u8) < ctx.num_inputs {
                "input"
            } else {
                "output"
            };
            Candidate::new(slug, what, CandidateKind::Channel)
        })
        .collect()
}

fn band_candidates(ctx: &Context) -> Vec<Candidate> {
    // The live count, not the wire array's depth: offering a reserved band
    // would complete to something the firmware rejects.
    let mut out: Vec<Candidate> = (1..=ctx.max_bands)
        .map(|b| Candidate::new(b.to_string(), "filter band", CandidateKind::Band))
        .collect();
    // Crossover bands keep their wire numbers, and are only valid on outputs.
    out.extend(
        (20..24).map(|b| Candidate::new(b.to_string(), "crossover band", CandidateKind::Band)),
    );
    out
}

fn index_candidates(count: u8, what: &str) -> Vec<Candidate> {
    (0..count)
        .map(|i| Candidate::new(i.to_string(), what, CandidateKind::Band))
        .collect()
}

/// What can legally follow, given the parameter's kind. Ranges are offered as a
/// hint rather than as insertable text, since there is nothing to complete.
fn value_candidates(d: &dspi_proto::registry::ParamDesc) -> Vec<Candidate> {
    match d.kind {
        Kind::Bool => vec![
            Candidate::new("on", d.plain, CandidateKind::Choice),
            Candidate::new("off", d.plain, CandidateKind::Choice),
        ],
        Kind::Choice(variants) => variants
            .iter()
            .map(|(_, name)| Candidate::new(*name, d.label, CandidateKind::Choice))
            .collect(),
        Kind::Float { unit, min, max } => vec![Candidate::new(
            "",
            format!("{min} to {max}{}", unit.suffix()),
            CandidateKind::Hint,
        )],
        Kind::Int { unit, min, max } => vec![Candidate::new(
            "",
            format!("{min} to {max}{}", unit.suffix()),
            CandidateKind::Hint,
        )],
        Kind::Text { max } => vec![Candidate::new(
            "",
            format!("text, up to {max} characters"),
            CandidateKind::Hint,
        )],
        Kind::Trigger => Vec::new(),
        _ => vec![Candidate::new("", d.plain, CandidateKind::Hint)],
    }
}

fn complete_eq(rest: &[&str], partial: &str, ctx: &Context) -> Vec<Candidate> {
    let lower = partial.to_ascii_lowercase();
    let keep = |c: &Candidate| c.value.to_ascii_lowercase().starts_with(&lower);

    let mut out = match rest.len() {
        0 => channel_candidates(ctx),
        1 => band_candidates(ctx),
        2 => by_path("eq.type").map(value_candidates).unwrap_or_default(),
        3 => vec![Candidate::new("", "frequency in Hz", CandidateKind::Hint)],
        4 => vec![Candidate::new("", "Q, default 0.707", CandidateKind::Hint)],
        5 => vec![Candidate::new("", "gain in dB", CandidateKind::Hint)],
        _ => Vec::new(),
    };
    out.retain(|c| c.kind == CandidateKind::Hint || keep(c));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::ctx;

    fn values(tokens: &[&str], partial: &str) -> Vec<String> {
        complete(tokens, partial, &ctx())
            .into_iter()
            .filter(|c| c.kind != CandidateKind::Hint)
            .map(|c| c.value)
            .collect()
    }

    fn hint(tokens: &[&str], partial: &str) -> Option<String> {
        complete(tokens, partial, &ctx())
            .into_iter()
            .find(|c| c.kind == CandidateKind::Hint)
            .map(|c| c.detail)
    }

    #[test]
    fn an_empty_line_offers_verbs_and_parameters() {
        let all = values(&[], "");
        assert!(all.contains(&"list".to_string()));
        assert!(all.contains(&"vol.user".to_string()));
        assert!(all.len() > 100, "every parameter should be reachable");
    }

    #[test]
    fn a_prefix_narrows_the_list() {
        let bass = values(&[], "bass.");
        assert!(bass.contains(&"bass.drive".to_string()));
        assert!(!bass.contains(&"vol.user".to_string()));
    }

    /// The point of the whole design: what can follow is derived from the
    /// parameter, so a user never has to know the valid words.
    #[test]
    fn a_boolean_offers_on_and_off() {
        assert_eq!(values(&["bass.on"], ""), vec!["on", "off"]);
    }

    #[test]
    fn a_choice_offers_its_variants() {
        let sources = values(&["in.source"], "");
        assert!(sources.contains(&"usb".to_string()));
        assert!(sources.contains(&"spdif".to_string()));
        assert!(sources.contains(&"adat".to_string()));
    }

    #[test]
    fn a_number_offers_its_range_as_a_hint() {
        assert_eq!(hint(&["bass.drive"], ""), Some("0 to 18 dB".into()));
        assert_eq!(hint(&["vol.user"], ""), Some("-60 to 0 dB".into()));
    }

    /// Channel suggestions come from the device, so a renamed channel completes
    /// under the name the user gave it.
    #[test]
    fn channels_complete_from_the_device() {
        let chans = values(&["ch.delay"], "");
        assert!(chans.contains(&"usb.1".to_string()));
        assert!(chans.contains(&"i2s.1.l".to_string()));
        assert!(chans.contains(&"pdm".to_string()));
    }

    #[test]
    fn a_channel_prefix_narrows_to_matching_channels() {
        let i2s = values(&["ch.delay"], "i2s");
        assert!(i2s.iter().all(|c| c.starts_with("i2s")));
        assert!(!i2s.is_empty());
    }

    #[test]
    fn the_second_index_offers_bands_including_crossovers() {
        let bands = values(&["eq.freq", "usb.1"], "");
        assert!(bands.contains(&"1".to_string()));
        assert!(bands.contains(&"20".to_string()), "crossover bands too");
    }

    #[test]
    fn completion_walks_all_the_way_to_the_value() {
        assert_eq!(
            hint(&["eq.freq", "usb.1", "3"], ""),
            Some("10 to 24000 Hz".into())
        );
    }

    #[test]
    fn get_shifts_the_frame_by_one() {
        let all = values(&["get"], "vol.");
        assert!(all.contains(&"vol.user".to_string()));
        // After a path, get takes indices, not a value.
        let chans = values(&["get", "ch.delay"], "");
        assert!(chans.contains(&"usb.1".to_string()));
    }

    #[test]
    fn the_compound_eq_form_completes_at_every_position() {
        assert!(values(&["eq"], "").contains(&"usb.1".to_string()));
        assert!(values(&["eq", "usb.1"], "").contains(&"1".to_string()));
        assert!(values(&["eq", "usb.1", "3"], "").contains(&"peak".to_string()));
        assert_eq!(
            hint(&["eq", "usb.1", "3", "peak"], ""),
            Some("frequency in Hz".into())
        );
    }

    #[test]
    fn a_trigger_offers_nothing_after_its_index() {
        assert!(complete(&["preset.save", "3"], "", &ctx()).is_empty());
    }

    #[test]
    fn an_unknown_path_offers_nothing_rather_than_guessing() {
        assert!(complete(&["nonsense.path"], "", &ctx()).is_empty());
    }

    #[test]
    fn preset_slots_are_bounded() {
        let slots = values(&["preset.name"], "");
        assert_eq!(slots.len(), 10);
    }

    #[test]
    fn shell_names_complete_for_the_completions_verb() {
        let shells = values(&["completions"], "");
        assert!(shells.contains(&"zsh".to_string()));
    }
}
