//! Crossover filter response.
//!
//! Ported from the firmware's `crossover.c` rather than from a textbook. The
//! families are standard, but the details that decide whether a curve matches
//! the hardware are not: the prewarp, the pole ordering, how a high pass
//! reciprocates its poles, and the fact that a Linkwitz-Riley of order 2N is a
//! Butterworth of order N cascaded with itself.
//!
//! The Bessel tables are copied verbatim from the firmware, including the
//! corrected eighth-order pair; the firmware's own comment records that the
//! original value was a typo caught by its test suite against scipy. Retyping
//! them from a handbook would risk reintroducing exactly that.

use crate::dsp::{Coeffs, SAMPLE_RATE};
use crate::enums::FilterType;

/// The crossover types occupy this range of the filter-type byte.
pub const XOVER_FIRST: u8 = 32;
pub const XOVER_LAST: u8 = 63;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    LinkwitzRiley,
    Butterworth,
    Bessel,
}

impl Family {
    pub fn label(self) -> &'static str {
        match self {
            Family::LinkwitzRiley => "Linkwitz-Riley",
            Family::Butterworth => "Butterworth",
            Family::Bessel => "Bessel",
        }
    }

    /// The short form used in commands and tables.
    pub fn short(self) -> &'static str {
        match self {
            Family::LinkwitzRiley => "lr",
            Family::Butterworth => "bw",
            Family::Bessel => "bes",
        }
    }
}

/// What a crossover type byte means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Meta {
    pub family: Family,
    pub order: u8,
    pub high_pass: bool,
}

impl Meta {
    /// A user-facing name: the family and order, never the raw number.
    pub fn label(self) -> String {
        format!(
            "{}{} {}",
            self.family.short().to_uppercase(),
            self.order,
            if self.high_pass { "HP" } else { "LP" }
        )
    }

    pub fn to_type(self) -> u8 {
        let base = match self.family {
            Family::LinkwitzRiley => 32 + (self.order / 2 - 1) * 2,
            Family::Butterworth => 40 + (self.order - 1) * 2,
            Family::Bessel => 56 + (self.order / 2 - 1) * 2,
        };
        base + self.high_pass as u8
    }
}

/// Decode a filter-type byte into a crossover description.
///
/// The layout mirrors the firmware's table exactly: LR2-LR8 at 32-39 stepping
/// by two orders, BW1-BW8 at 40-55 stepping by one, Bessel 2-8 at 56-63.
pub fn meta(filter_type: u8) -> Option<Meta> {
    if !(XOVER_FIRST..=XOVER_LAST).contains(&filter_type) {
        return None;
    }
    let idx = filter_type - XOVER_FIRST;
    let high_pass = idx % 2 == 1;
    let pair = idx / 2;

    Some(match filter_type {
        32..=39 => Meta {
            family: Family::LinkwitzRiley,
            order: (pair + 1) * 2,
            high_pass,
        },
        40..=55 => Meta {
            family: Family::Butterworth,
            order: pair - 3,
            high_pass,
        },
        _ => Meta {
            family: Family::Bessel,
            order: (pair - 11) * 2,
            high_pass,
        },
    })
}

/// A left-half-plane pole pair, normalised to unit cutoff.
#[derive(Debug, Clone, Copy)]
struct Pole {
    sigma: f64,
    omega: f64,
}

/// Bessel poles, copied from the firmware's verified tables.
fn bessel_poles(order: u8) -> &'static [(f64, f64)] {
    match order {
        2 => &[(1.10160, 0.63601)],
        4 => &[(1.37007, 0.41025), (0.99521, 1.25711)],
        6 => &[(1.57149, 0.32090), (1.38186, 0.97147), (0.93066, 1.66186)],
        8 => &[
            (1.75741, 0.27287),
            (1.63694, 0.82280),
            (1.37384, 1.38836),
            (0.89287, 1.99833),
        ],
        _ => &[],
    }
}

/// Butterworth pole pair `k` of `order`, in ascending Q.
fn butterworth_pole(order: u8, pair_index: u8) -> Pole {
    let theta = if order % 2 == 1 {
        std::f64::consts::PI * (pair_index as f64 + 1.0) / order as f64
    } else {
        std::f64::consts::PI * (2.0 * pair_index as f64 + 1.0) / (2.0 * order as f64)
    };
    Pole {
        sigma: theta.cos(),
        omega: theta.sin(),
    }
}

/// A second-order section built from a normalised analog pole.
fn second_order(mut p: Pole, omega_a: f64, high_pass: bool) -> Coeffs {
    // A high pass reflects each pole through the unit circle. Skipping this
    // gives a curve that looks plausible and is wrong everywhere but the corner.
    if high_pass {
        let r2 = p.sigma * p.sigma + p.omega * p.omega;
        if r2 > 0.0 {
            p.sigma /= r2;
            p.omega /= r2;
        }
    }
    let sigma = p.sigma * omega_a;
    let omega = p.omega * omega_a;

    let w0 = (sigma * sigma + omega * omega).sqrt();
    let q = w0 / (2.0 * sigma);
    // The firmware's `g = w0 / (2*Fs)` is already the prewarped tangent, so the
    // matching digital corner is `atan(g) * Fs / pi`.
    let g = w0 / (2.0 * SAMPLE_RATE);
    let f0 = g.atan() * SAMPLE_RATE / std::f64::consts::PI;

    crate::dsp::coefficients(&crate::dsp::Band {
        filter_type: if high_pass {
            FilterType::HighPass
        } else {
            FilterType::LowPass
        },
        freq: f0 as f32,
        q: q as f32,
        gain_db: 0.0,
        bypass: false,
    })
}

/// A first-order section from a real pole.
fn first_order(mut sigma_n: f64, omega_a: f64, high_pass: bool) -> Coeffs {
    if high_pass && sigma_n > 0.0 {
        sigma_n = 1.0 / sigma_n;
    }
    let g = sigma_n * omega_a / (2.0 * SAMPLE_RATE);

    // One-pole TPT low pass, or its complement for a high pass.
    let (num0, num1) = if high_pass { (1.0, -1.0) } else { (g, g) };
    let den = 1.0 + g;
    Coeffs {
        b0: num0 / den,
        b1: num1 / den,
        b2: 0.0,
        a1: (g - 1.0) / den,
        a2: 0.0,
    }
}

/// Every biquad section a crossover band decomposes into.
pub fn sections(filter_type: u8, freq: f32) -> Vec<Coeffs> {
    let Some(m) = meta(filter_type) else {
        return Vec::new();
    };

    // The firmware clamps before prewarping; matching it keeps the curve honest
    // for a band set implausibly high.
    let fc = (freq as f64).clamp(1.0, SAMPLE_RATE * 0.45);
    let omega_a = 2.0 * SAMPLE_RATE * (std::f64::consts::PI * fc / SAMPLE_RATE).tan();

    let mut out = Vec::new();
    match m.family {
        Family::Butterworth => {
            push_butterworth(&mut out, m.order, omega_a, m.high_pass);
        }
        Family::LinkwitzRiley => {
            if m.order == 2 {
                // LR2 is a single biquad with a double real pole: Q = 0.5.
                out.push(second_order(
                    Pole {
                        sigma: 1.0,
                        omega: 0.0,
                    },
                    omega_a,
                    m.high_pass,
                ));
            } else {
                // LR(2N) is BW(N) cascaded with itself.
                let mut half = Vec::new();
                push_butterworth(&mut half, m.order / 2, omega_a, m.high_pass);
                out.extend(half.iter().copied());
                out.extend(half);
            }
        }
        Family::Bessel => {
            for (sigma, omega) in bessel_poles(m.order) {
                out.push(second_order(
                    Pole {
                        sigma: *sigma,
                        omega: *omega,
                    },
                    omega_a,
                    m.high_pass,
                ));
            }
        }
    }
    out
}

fn push_butterworth(out: &mut Vec<Coeffs>, order: u8, omega_a: f64, high_pass: bool) {
    // An odd order contributes one real pole, emitted first as the firmware does.
    if order % 2 == 1 {
        out.push(first_order(1.0, omega_a, high_pass));
    }
    for p in 0..order / 2 {
        out.push(second_order(butterworth_pole(order, p), omega_a, high_pass));
    }
}

/// Response of one crossover band at a frequency, in dB.
pub fn response_at(freq_hz: f64, filter_type: u8, corner_hz: f32) -> f64 {
    let mut power = 1.0f64;
    for c in sections(filter_type, corner_hz) {
        power *= crate::dsp::magnitude_squared_of(&c, freq_hz);
    }
    if power <= 0.0 {
        -200.0
    } else {
        10.0 * power.log10()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    #[test]
    fn type_bytes_decode_to_the_firmware_table() {
        assert_eq!(
            meta(32).unwrap(),
            Meta {
                family: Family::LinkwitzRiley,
                order: 2,
                high_pass: false
            }
        );
        assert_eq!(
            meta(39).unwrap(),
            Meta {
                family: Family::LinkwitzRiley,
                order: 8,
                high_pass: true
            }
        );
        assert_eq!(
            meta(40).unwrap(),
            Meta {
                family: Family::Butterworth,
                order: 1,
                high_pass: false
            }
        );
        assert_eq!(
            meta(55).unwrap(),
            Meta {
                family: Family::Butterworth,
                order: 8,
                high_pass: true
            }
        );
        assert_eq!(
            meta(56).unwrap(),
            Meta {
                family: Family::Bessel,
                order: 2,
                high_pass: false
            }
        );
        assert_eq!(
            meta(63).unwrap(),
            Meta {
                family: Family::Bessel,
                order: 8,
                high_pass: true
            }
        );
        assert!(meta(31).is_none());
        assert!(meta(64).is_none());
    }

    #[test]
    fn every_type_in_the_range_round_trips() {
        for t in XOVER_FIRST..=XOVER_LAST {
            let m = meta(t).unwrap();
            assert_eq!(m.to_type(), t, "{m:?} did not round-trip from {t}");
        }
    }

    #[test]
    fn section_counts_match_the_firmware_table() {
        // LR6 is BW3 squared: two first-order plus two biquads.
        assert_eq!(sections(36, 1000.0).len(), 4);
        assert_eq!(sections(32, 1000.0).len(), 1); // LR2
        assert_eq!(sections(34, 1000.0).len(), 2); // LR4
        assert_eq!(sections(38, 1000.0).len(), 4); // LR8
        assert_eq!(sections(40, 1000.0).len(), 1); // BW1
        assert_eq!(sections(48, 1000.0).len(), 3); // BW5
        assert_eq!(sections(54, 1000.0).len(), 4); // BW8
        assert_eq!(sections(62, 1000.0).len(), 4); // Bessel 8
    }

    /// The defining property: a Linkwitz-Riley is 6 dB down at its corner, where
    /// a Butterworth is 3 dB down. Getting this wrong means every crossover the
    /// app draws is misleading.
    #[test]
    fn linkwitz_riley_is_six_db_down_at_the_corner() {
        for t in [32u8, 34, 36, 38] {
            let db = response_at(1000.0, t, 1000.0);
            assert!(
                close(db, -6.0, 0.35),
                "{} should be -6 dB at its corner, got {db}",
                meta(t).unwrap().label()
            );
        }
    }

    #[test]
    fn butterworth_is_three_db_down_at_the_corner() {
        for t in [40u8, 42, 44, 46, 48, 50, 52, 54] {
            let db = response_at(1000.0, t, 1000.0);
            assert!(
                close(db, -3.0, 0.35),
                "{} should be -3 dB at its corner, got {db}",
                meta(t).unwrap().label()
            );
        }
    }

    /// A low pass and its matching high pass must be mirror images about the
    /// corner; this is what catches a missing pole reciprocation.
    #[test]
    fn high_pass_mirrors_low_pass() {
        for lp in [32u8, 34, 44, 52, 56, 62] {
            let hp = lp + 1;
            let below = response_at(100.0, lp, 1000.0);
            let above = response_at(10_000.0, hp, 1000.0);
            assert!(close(below, 0.0, 0.6), "LP should pass 100 Hz: {below}");
            assert!(close(above, 0.0, 0.6), "HP should pass 10 kHz: {above}");

            let lp_stop = response_at(10_000.0, lp, 1000.0);
            let hp_stop = response_at(100.0, hp, 1000.0);
            assert!(lp_stop < -20.0, "LP should stop 10 kHz: {lp_stop}");
            assert!(hp_stop < -20.0, "HP should stop 100 Hz: {hp_stop}");
        }
    }

    /// Order sets the slope: each order is another 6 dB per octave.
    #[test]
    fn slope_follows_the_order() {
        // One octave below the corner for a high pass.
        let at = |t: u8| response_at(500.0, t, 1000.0);
        let bw2 = at(43);
        let bw4 = at(47);
        let bw8 = at(55);
        assert!(bw2 > bw4 && bw4 > bw8, "steeper orders must cut harder");
        // BW2 is about 12 dB down an octave out, BW4 about 24.
        assert!(close(bw2, -12.3, 1.5), "BW2 one octave out: {bw2}");
        assert!(close(bw4, -24.1, 2.0), "BW4 one octave out: {bw4}");
    }

    /// A Linkwitz-Riley pair sums flat, which is the entire reason the family
    /// exists and the strongest single check on the implementation.
    #[test]
    fn a_linkwitz_riley_pair_sums_flat() {
        for (lp, hp) in [(34u8, 35u8), (38, 39)] {
            for f in [100.0, 500.0, 900.0, 1000.0, 1100.0, 2000.0, 8000.0] {
                let a = 10f64.powf(response_at(f, lp, 1000.0) / 20.0);
                let b = 10f64.powf(response_at(f, hp, 1000.0) / 20.0);
                // LR sums in phase to unity magnitude.
                let sum_db = 20.0 * (a + b).log10();
                assert!(
                    close(sum_db, 0.0, 0.6),
                    "{} + {} at {f} Hz summed to {sum_db} dB",
                    meta(lp).unwrap().label(),
                    meta(hp).unwrap().label()
                );
            }
        }
    }

    #[test]
    fn bessel_is_gentler_than_butterworth_at_the_same_order() {
        // Bessel trades slope for phase behaviour, so it is shallower.
        let bes4 = response_at(2000.0, 58, 1000.0);
        let bw4 = response_at(2000.0, 46, 1000.0);
        assert!(
            bes4 > bw4,
            "Bessel should roll off more gently: {bes4} vs {bw4}"
        );
    }

    #[test]
    fn every_type_is_finite_across_the_band() {
        for t in XOVER_FIRST..=XOVER_LAST {
            for fc in [20.0f32, 80.0, 1000.0, 18_000.0, 30_000.0] {
                for f in [10.0, 100.0, 1000.0, 20_000.0] {
                    let db = response_at(f, t, fc);
                    assert!(db.is_finite(), "type {t} fc {fc} at {f} Hz gave {db}");
                }
            }
        }
    }

    #[test]
    fn labels_name_the_family_and_order() {
        assert_eq!(meta(34).unwrap().label(), "LR4 LP");
        assert_eq!(meta(55).unwrap().label(), "BW8 HP");
        assert_eq!(meta(56).unwrap().label(), "BES2 LP");
    }
}
