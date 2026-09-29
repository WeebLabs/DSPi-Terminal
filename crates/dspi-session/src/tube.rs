//! The Tube Modeller's type and rectifier tables.
//!
//! Choosing a tube type makes the firmware copy four character values out of
//! a row, and the rectifier sets how deep and how fast the supply sags. Both
//! tables live only in the firmware source (`tube.c:49-75`), not in a header,
//! so they cannot be generated; they are transcribed here once, and the test
//! below holds the first row to the header's defaults, which are that row.
//!
//! The names are the Console's (`Constants.swift:364-405`), which match the
//! comments in `tube.c` except that the Console capitalises "Solid state".
//! The Terminal never applies a row itself: it writes the type and re-reads,
//! so the device's own copy is what a screen shows (DESIGN section 11).

use dspi_proto::generated::tube as t;

/// One tube style: the values a type write loads into the character knobs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TubeType {
    pub name: &'static str,
    pub bias_pct: f32,
    pub asym_db: f32,
    pub hardness_pct: f32,
    pub sag_pct: f32,
}

const fn row(name: &'static str, bias: f32, asym: f32, hardness: f32, sag: f32) -> TubeType {
    TubeType {
        name,
        bias_pct: bias,
        asym_db: asym,
        hardness_pct: hardness,
        sag_pct: sag,
    }
}

/// `tube_rows` (tube.c:49-66), indexed by `tube_type - 1`: type 0 is Custom
/// and has no row. The firmware never renumbers them.
pub const TYPES: [TubeType; t::TUBE_TYPE_MAX as usize] = [
    row("12AX7 / ECC83", 10.0, 3.0, 40.0, 15.0),
    row("5751", 8.0, 3.0, 35.0, 12.0),
    row("12AT7 / ECC81", 5.0, 2.0, 55.0, 10.0),
    row("12AY7", 7.0, 4.0, 25.0, 15.0),
    row("12AU7 / ECC82", 5.0, 5.0, 20.0, 8.0),
    row("6SN7", 7.0, 6.0, 15.0, 10.0),
    row("6SL7", 10.0, 3.0, 30.0, 15.0),
    row("6DJ8 / ECC88 / 6922", 3.0, 2.0, 60.0, 5.0),
    row("EF86 / 6267", 2.0, 0.0, 75.0, 12.0),
    row("6SJ7", 3.0, 1.0, 65.0, 15.0),
    row("EL84 / 6BQ5", 0.0, 0.0, 50.0, 25.0),
    row("EL34", 0.0, 0.0, 60.0, 30.0),
    row("6L6 / 5881", 0.0, 0.0, 55.0, 18.0),
    row("6V6", 0.0, 0.0, 35.0, 30.0),
    row("KT88 / 6550", 0.0, 0.0, 45.0, 10.0),
    row("300B / 2A3", 12.0, 6.0, 10.0, 12.0),
];

/// One rectifier style: how much of the sag setting it lets through, and the
/// supply's attack and release.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rectifier {
    pub name: &'static str,
    pub depth_scale: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
}

/// `rect_rows` (tube.c:70-75), indexed by `rectifier`, `0..=TUBE_RECT_MAX`.
/// Solid state switches sag off.
pub const RECTIFIERS: [Rectifier; t::TUBE_RECT_MAX as usize + 1] = [
    Rectifier {
        name: "Solid state",
        depth_scale: 0.0,
        attack_ms: 0.0,
        release_ms: 0.0,
    },
    Rectifier {
        name: "GZ34 / 5AR4",
        depth_scale: 0.6,
        attack_ms: 5.0,
        release_ms: 120.0,
    },
    Rectifier {
        name: "5U4",
        depth_scale: 1.0,
        attack_ms: 8.0,
        release_ms: 200.0,
    },
    Rectifier {
        name: "5Y3",
        depth_scale: 1.3,
        attack_ms: 10.0,
        release_ms: 300.0,
    },
];

/// The row a type loads, or `None` for Custom (0) and anything past the table.
pub fn type_row(tube_type: u8) -> Option<&'static TubeType> {
    (tube_type as usize)
        .checked_sub(1)
        .and_then(|i| TYPES.get(i))
}

/// The Console's `tubeTypeName` (Constants.swift:385-389).
pub fn type_name(tube_type: u8) -> String {
    if tube_type as u16 == t::TUBE_TYPE_CUSTOM {
        return "Custom".into();
    }
    type_row(tube_type).map_or_else(|| format!("Type {tube_type}"), |r| r.name.into())
}

/// The Console's `tubeRectifierName` (Constants.swift:407-409).
pub fn rectifier_name(rectifier: u8) -> String {
    RECTIFIERS
        .get(rectifier as usize)
        .map_or_else(|| format!("Rectifier {rectifier}"), |r| r.name.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dspi_proto::generated::ranges as r;

    /// The header's defaults are the 12AX7 row (tube.h:60-66), which is the
    /// one check against the firmware a hand-copied table can have.
    #[test]
    fn the_default_type_is_the_first_row_and_matches_the_header() {
        let row = type_row(t::TUBE_DEFAULT_TUBE_TYPE as u8).unwrap();
        assert_eq!(row.name, "12AX7 / ECC83");
        assert_eq!(row.bias_pct, r::TUBE_DEFAULT_BIAS);
        assert_eq!(row.asym_db, r::TUBE_DEFAULT_ASYM);
        assert_eq!(row.hardness_pct, r::TUBE_DEFAULT_HARDNESS);
        assert_eq!(row.sag_pct, r::TUBE_DEFAULT_SAG);
        assert_eq!(
            rectifier_name(t::TUBE_DEFAULT_RECTIFIER as u8),
            "GZ34 / 5AR4"
        );
    }

    #[test]
    fn every_row_sits_inside_the_parameter_ranges() {
        for row in TYPES {
            assert!((r::TUBE_BIAS_MIN..=r::TUBE_BIAS_MAX).contains(&row.bias_pct));
            assert!((r::TUBE_ASYM_MIN..=r::TUBE_ASYM_MAX).contains(&row.asym_db));
            assert!((r::TUBE_HARDNESS_MIN..=r::TUBE_HARDNESS_MAX).contains(&row.hardness_pct));
            assert!((r::TUBE_SAG_MIN..=r::TUBE_SAG_MAX).contains(&row.sag_pct));
        }
    }

    #[test]
    fn names_follow_the_console_including_custom_and_the_unknown() {
        assert_eq!(type_name(0), "Custom");
        assert_eq!(type_name(16), "300B / 2A3");
        assert_eq!(type_name(17), "Type 17");
        assert_eq!(rectifier_name(0), "Solid state");
        assert_eq!(rectifier_name(4), "Rectifier 4");
        assert!(type_row(0).is_none());
    }
}
