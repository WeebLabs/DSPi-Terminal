//! Panels that come from the registry rather than from hand-written layout.
//!
//! A panel here is a list of parameter paths. Which control each one gets, what
//! it is called, what range it accepts, whether it is available on this device
//! and what it does when written are all already described by the registry, so a
//! new firmware parameter appears in its panel with no work in this file.
//!
//! That is the whole point of mechanism 5 in the design: the alternative is ten
//! hand-built panels that drift out of step with the protocol one parameter at a
//! time.

use dspi_proto::registry::{Group, Kind, Level as RegLevel, ParamDesc, REGISTRY, Target};
use dspi_proto::value::Value;

use crate::app::Level;

/// One editable row.
#[derive(Debug, Clone)]
pub struct Field {
    pub path: &'static str,
    /// Index arguments, e.g. the channel for a per-channel parameter.
    pub indices: Vec<u8>,
    /// The last value read or written; `None` until it has been read once.
    pub value: Option<Value>,
    pub state: FieldState,
    /// Why this row cannot be used, if it cannot.
    pub unavailable: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FieldState {
    /// Read from the device and believed current.
    Settled,
    /// Written, awaiting confirmation.
    Pending,
    /// The device accepted the write and kept something else.
    Rejected(Value),
    /// Never successfully read.
    Unknown,
}

impl Field {
    pub fn new(path: &'static str, indices: Vec<u8>) -> Self {
        Self {
            path,
            indices,
            value: None,
            state: FieldState::Unknown,
            unavailable: None,
        }
    }

    pub fn desc(&self) -> Option<&'static ParamDesc> {
        dspi_proto::registry::by_path(self.path)
    }

    pub fn is_editable(&self) -> bool {
        self.unavailable.is_none()
            && self
                .desc()
                .map(|d| d.is_writable() && !matches!(d.kind, Kind::Status))
                .unwrap_or(false)
    }

    /// The value as text, resolving a choice to its name.
    pub fn display(&self) -> String {
        let Some(d) = self.desc() else {
            return String::new();
        };
        let Some(v) = &self.value else {
            return "-".into();
        };
        display_value(d, v)
    }
}

/// Render a value the way its parameter means it, not the way it is stored.
pub fn display_value(d: &ParamDesc, v: &Value) -> String {
    if let (Kind::Choice(variants), Some(n)) = (d.kind, v.as_u8())
        && let Some((_, name)) = variants.iter().find(|(raw, _)| *raw == n)
    {
        return (*name).to_string();
    }
    v.display(d.kind.unit())
}

/// Which parameters a panel shows, at a given disclosure level.
///
/// The order is the registry's own, which is arranged so a user meets the
/// parameters in the order they would think about them.
pub fn fields_for(group: Group, level: Level, targets: &Targets) -> Vec<Field> {
    let max = match level {
        Level::Simple => RegLevel::Simple,
        Level::Advanced => RegLevel::Advanced,
        Level::Expert => RegLevel::Expert,
    };

    REGISTRY
        .iter()
        .filter(|d| d.group == group && d.level <= max)
        // Structured reads and packets need bespoke rendering: a status packet
        // shown as a scalar reads as "1 bytes", which looks like data and is
        // worse than not showing it. They are covered by their own views.
        .filter(|d| !matches!(d.kind, Kind::Status | Kind::Packet))
        // Untargeted parameters are one row; targeted ones would be a row per
        // channel, which belongs in a dedicated panel rather than a flat list.
        .filter(|d| d.target == Target::None || targets.expand(d.target).len() == 1)
        .flat_map(|d| {
            targets
                .expand(d.target)
                .into_iter()
                .map(move |ix| Field::new(d.path, ix))
        })
        .collect()
}

/// How many of each index space this device has.
#[derive(Debug, Clone, Default)]
pub struct Targets {
    pub inputs: u8,
    pub outputs: u8,
    pub channels: u8,
}

impl Targets {
    /// The index arguments to use for a parameter in a flat panel.
    ///
    /// A flat list shows the untargeted parameters; anything addressing a
    /// channel gets its own panel where the channel is chosen explicitly.
    fn expand(&self, target: Target) -> Vec<Vec<u8>> {
        match target {
            Target::None => vec![vec![]],
            _ => vec![],
        }
    }
}

/// Step a value by one notch, respecting how the quantity is perceived.
///
/// Frequency and Q move by a proportion, because a 10 Hz step is enormous at
/// 30 Hz and invisible at 10 kHz. Everything else moves linearly.
pub fn nudge(d: &ParamDesc, current: &Value, up: bool, coarse: bool) -> Option<Value> {
    let dir = if up { 1.0 } else { -1.0 };

    Some(match d.kind {
        // Directional rather than a toggle: right means on, left means off, so
        // holding a key does not flap the value back and forth.
        Kind::Bool => Value::Bool(up),

        Kind::Choice(variants) => {
            let now = current.as_u8()?;
            let i = variants.iter().position(|(raw, _)| *raw == now)?;
            let next = if up {
                (i + 1) % variants.len()
            } else {
                (i + variants.len() - 1) % variants.len()
            };
            Value::Choice(variants[next].0)
        }

        Kind::Float { unit, min, max } => {
            let v = current.as_f32()?;
            let next = if unit.is_logarithmic() {
                // A twelfth of an octave, or a whole one when coarse.
                let factor = if coarse {
                    2.0f32
                } else {
                    2f32.powf(1.0 / 12.0)
                };
                if up { v * factor } else { v / factor }
            } else {
                let step = if coarse { 6.0 } else { 0.5 };
                v + dir * step
            };
            Value::Float(next.clamp(min, max))
        }

        Kind::Int { min, max, .. } => {
            let v = current.as_f32()? as i64;
            let step = if coarse { 10 } else { 1 };
            Value::Int((v + dir as i64 * step).clamp(min, max))
        }

        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dspi_proto::registry::by_path;

    fn targets() -> Targets {
        Targets {
            inputs: 8,
            outputs: 9,
            channels: 17,
        }
    }

    #[test]
    fn a_panel_is_built_from_the_registry() {
        let f = fields_for(Group::Dynamics, Level::Advanced, &targets());
        let paths: Vec<&str> = f.iter().map(|x| x.path).collect();
        assert!(paths.contains(&"loud.on"));
        assert!(paths.contains(&"lev.amount"));
        // Belongs to another panel.
        assert!(!paths.contains(&"bass.drive"));
    }

    /// Disclosure only hides; it never changes what a control does.
    #[test]
    fn levels_reveal_progressively() {
        let simple = fields_for(Group::Dynamics, Level::Simple, &targets()).len();
        let advanced = fields_for(Group::Dynamics, Level::Advanced, &targets()).len();
        let expert = fields_for(Group::Dynamics, Level::Expert, &targets()).len();
        assert!(simple < advanced, "advanced should reveal more");
        assert!(advanced < expert, "expert should reveal more still");
    }

    #[test]
    fn a_simple_panel_still_offers_the_obvious_controls() {
        let f = fields_for(Group::Dynamics, Level::Simple, &targets());
        let paths: Vec<&str> = f.iter().map(|x| x.path).collect();
        assert!(paths.contains(&"loud.on"), "loudness is a Simple control");
        assert!(!paths.contains(&"loud.mask"), "masks are for Expert");
    }

    #[test]
    fn status_rows_are_not_editable() {
        let mut f = Field::new("in.spdif.status", vec![]);
        assert!(!f.is_editable());
        f = Field::new("loud.on", vec![]);
        assert!(f.is_editable());
    }

    #[test]
    fn an_unavailable_field_cannot_be_edited() {
        let mut f = Field::new("bass.on", vec![]);
        assert!(f.is_editable());
        f.unavailable = Some("this firmware has no psychoacoustic bass".into());
        assert!(!f.is_editable());
    }

    #[test]
    fn a_field_with_no_reading_yet_shows_a_placeholder() {
        assert_eq!(Field::new("vol.user", vec![]).display(), "-");
    }

    #[test]
    fn choices_display_as_names() {
        let mut f = Field::new("in.source", vec![]);
        f.value = Some(Value::Choice(1));
        assert_eq!(f.display(), "spdif");
    }

    /// Right means on and left means off, rather than both toggling: a held key
    /// should settle on a value, not oscillate.
    #[test]
    fn a_boolean_follows_the_direction_pressed() {
        let d = by_path("loud.on").unwrap();
        assert_eq!(
            nudge(d, &Value::Bool(false), true, false),
            Some(Value::Bool(true))
        );
        assert_eq!(
            nudge(d, &Value::Bool(true), true, false),
            Some(Value::Bool(true))
        );
        assert_eq!(
            nudge(d, &Value::Bool(true), false, false),
            Some(Value::Bool(false))
        );
        assert_eq!(
            nudge(d, &Value::Bool(false), false, false),
            Some(Value::Bool(false))
        );
    }

    #[test]
    fn a_choice_wraps_in_both_directions() {
        let d = by_path("lev.speed").unwrap();
        assert_eq!(
            nudge(d, &Value::Choice(2), true, false),
            Some(Value::Choice(0))
        );
        assert_eq!(
            nudge(d, &Value::Choice(0), false, false),
            Some(Value::Choice(2))
        );
    }

    /// Frequency steps proportionally: a fixed step is enormous at 30 Hz and
    /// invisible at 10 kHz.
    #[test]
    fn frequency_steps_by_a_ratio_not_an_amount() {
        let d = by_path("bass.cutoff").unwrap();
        let low = nudge(d, &Value::Float(40.0), true, false)
            .unwrap()
            .as_f32()
            .unwrap();
        let high = nudge(d, &Value::Float(200.0), true, false)
            .unwrap()
            .as_f32()
            .unwrap();
        assert!(
            (high - 200.0) > (low - 40.0) * 3.0,
            "the step should scale with frequency: {low} vs {high}"
        );
        // A semitone up, near enough.
        assert!((low / 40.0 - 2f32.powf(1.0 / 12.0)).abs() < 1e-4);
    }

    #[test]
    fn decibels_step_linearly() {
        let d = by_path("bass.drive").unwrap();
        let v = nudge(d, &Value::Float(6.0), true, false).unwrap();
        assert_eq!(v, Value::Float(6.5));
        let coarse = nudge(d, &Value::Float(6.0), true, true).unwrap();
        assert_eq!(coarse, Value::Float(12.0));
    }

    /// Nudging must never leave the range the device will accept, or every
    /// keypress at the limit would produce a rejection.
    #[test]
    fn nudging_stops_at_the_limits() {
        let d = by_path("bass.drive").unwrap();
        let mut v = Value::Float(17.0);
        for _ in 0..20 {
            v = nudge(d, &v, true, true).unwrap();
        }
        assert_eq!(v, Value::Float(18.0));

        let mut v = Value::Float(1.0);
        for _ in 0..20 {
            v = nudge(d, &v, false, true).unwrap();
        }
        assert_eq!(v, Value::Float(0.0));
    }

    #[test]
    fn a_trigger_has_nothing_to_nudge() {
        let d = by_path("preset.save").unwrap();
        assert!(nudge(d, &Value::Trigger, true, false).is_none());
    }
}

#[cfg(test)]
mod structured_tests {
    use super::*;

    fn targets() -> Targets {
        Targets {
            inputs: 8,
            outputs: 9,
            channels: 17,
        }
    }

    /// A status packet rendered as a scalar reads as "1 bytes", which looks like
    /// a value and is worse than showing nothing. Those rows are excluded until
    /// they have a view that can decode them.
    #[test]
    fn structured_reads_stay_out_of_flat_panels() {
        for group in [Group::Input, Group::Presets, Group::System, Group::Spatial] {
            for f in fields_for(group, Level::Expert, &targets()) {
                let kind = f.desc().unwrap().kind;
                assert!(
                    !matches!(kind, Kind::Status | Kind::Packet),
                    "{} is structured and needs its own view",
                    f.path
                );
            }
        }
    }

    /// The exclusion must not swallow the ordinary controls with it.
    #[test]
    fn the_editable_controls_are_still_there() {
        let input: Vec<&str> = fields_for(Group::Input, Level::Expert, &targets())
            .iter()
            .map(|f| f.path)
            .collect();
        assert!(input.contains(&"in.source"));
        assert!(input.contains(&"in.rate"));
        assert!(!input.contains(&"in.spdif.status"));

        let spatial: Vec<&str> = fields_for(Group::Spatial, Level::Advanced, &targets())
            .iter()
            .map(|f| f.path)
            .collect();
        assert!(spatial.contains(&"bass.drive"));
        assert!(spatial.contains(&"cf.freq"));
    }
}
