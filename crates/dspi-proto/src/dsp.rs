//! Filter response maths, for the graph.
//!
//! A port of the DSPi Console's `DSPMath.swift`, kept deliberately faithful so
//! the two applications draw the same curve for the same settings. Divergence
//! here would be worse than a missing feature: a user would trust one of the two
//! and be wrong.
//!
//! Everything is computed in `f64` even though the device stores `f32`, matching
//! the Console, which uses `Double` throughout for the same reason: the cascade
//! accumulates error quickly at high Q.

use crate::enums::FilterType;

/// The device runs at 48 kHz; the response is drawn in that domain.
pub const SAMPLE_RATE: f64 = 48_000.0;

/// Points on the curve. The Console uses 201, and matching it means the two
/// applications can be compared point by point.
pub const POINTS: usize = 201;

/// The plotted range. Wider than the audible band at both ends, so shelves and
/// high-frequency filters are visible where they actually turn over.
pub const MIN_HZ: f64 = 10.0;
pub const MAX_HZ: f64 = 20_000.0;

/// One band's settings, as the graph needs them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    pub filter_type: FilterType,
    pub freq: f32,
    pub q: f32,
    pub gain_db: f32,
    pub bypass: bool,
}

impl Default for Band {
    fn default() -> Self {
        Self {
            filter_type: FilterType::Flat,
            freq: 1000.0,
            q: 0.707,
            gain_db: 0.0,
            bypass: false,
        }
    }
}

/// Normalised biquad coefficients, with `a0` divided out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coeffs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Coeffs {
    /// A filter that does nothing.
    pub const PASSTHROUGH: Coeffs = Coeffs {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };
}

/// RBJ cookbook coefficients.
///
/// Returns passthrough for anything that does not shape the response, including
/// **unrecognised types**: a filter this build does not understand is drawn as
/// no contribution rather than guessed at, and the caller is expected to say so
/// in the UI rather than let the curve quietly lie.
pub fn coefficients(b: &Band) -> Coeffs {
    if b.bypass {
        return Coeffs::PASSTHROUGH;
    }

    let freq = b.freq as f64;
    let q = (b.q as f64).max(1e-4);
    let gain = b.gain_db as f64;

    // Above Nyquist there is no meaningful response to draw.
    if freq <= 0.0 || freq >= SAMPLE_RATE / 2.0 {
        return Coeffs::PASSTHROUGH;
    }

    let w = 2.0 * std::f64::consts::PI * freq / SAMPLE_RATE;
    let sn = w.sin();
    let cs = w.cos();
    let alpha = sn / (2.0 * q);
    // A = 10^(gain/40) because the shelving and peaking forms work in amplitude,
    // and the response is squared later.
    let a = 10f64.powf(gain / 40.0);

    let (b0, b1, b2, a0, a1, a2) = match b.filter_type {
        FilterType::LowPass => (
            (1.0 - cs) / 2.0,
            1.0 - cs,
            (1.0 - cs) / 2.0,
            1.0 + alpha,
            -2.0 * cs,
            1.0 - alpha,
        ),
        FilterType::HighPass => (
            (1.0 + cs) / 2.0,
            -(1.0 + cs),
            (1.0 + cs) / 2.0,
            1.0 + alpha,
            -2.0 * cs,
            1.0 - alpha,
        ),
        FilterType::Peaking => (
            1.0 + alpha * a,
            -2.0 * cs,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cs,
            1.0 - alpha / a,
        ),
        FilterType::Notch => (1.0, -2.0 * cs, 1.0, 1.0 + alpha, -2.0 * cs, 1.0 - alpha),
        FilterType::AllPass => (
            1.0 - alpha,
            -2.0 * cs,
            1.0 + alpha,
            1.0 + alpha,
            -2.0 * cs,
            1.0 - alpha,
        ),
        FilterType::LowShelf => {
            let sq = 2.0 * a.sqrt() * alpha;
            (
                a * ((a + 1.0) - (a - 1.0) * cs + sq),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cs),
                a * ((a + 1.0) - (a - 1.0) * cs - sq),
                (a + 1.0) + (a - 1.0) * cs + sq,
                -2.0 * ((a - 1.0) + (a + 1.0) * cs),
                (a + 1.0) + (a - 1.0) * cs - sq,
            )
        }
        FilterType::HighShelf => {
            let sq = 2.0 * a.sqrt() * alpha;
            (
                a * ((a + 1.0) + (a - 1.0) * cs + sq),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cs),
                a * ((a + 1.0) + (a - 1.0) * cs - sq),
                (a + 1.0) - (a - 1.0) * cs + sq,
                2.0 * ((a - 1.0) - (a + 1.0) * cs),
                (a + 1.0) - (a - 1.0) * cs - sq,
            )
        }
        // First-order types. The RBJ cookbook only covers second order, so these
        // are the exact bilinear equivalents of the one-pole TPT state-variable
        // filter the firmware runs (`dsp_pipeline.c`, the `svf_first_order`
        // path). Deriving them independently would have drawn a curve the device
        // does not produce; the distinctive part is the prewarp, which divides
        // by A for a low shelf and multiplies by A for a high shelf, with `A*A`
        // as the shelf's linear gain.
        //
        // The one-pole TPT low pass is `g(1 + z^-1) / ((1+g) + (g-1)z^-1)`, and
        // each type is a fixed mix of that low pass with the input.
        FilterType::AllPass1 => {
            // out = 2*lp - in
            let g = w_over_two_tan(w);
            let k = (g - 1.0) / (g + 1.0);
            (k, 1.0, 0.0, 1.0, k, 0.0)
        }
        FilterType::LowShelf1 => {
            // out = in + (A^2 - 1) * lp, with g prewarped by 1/A.
            let g = w_over_two_tan(w) / a;
            let a2 = a * a;
            (1.0 + a2 * g, a2 * g - 1.0, 0.0, 1.0 + g, g - 1.0, 0.0)
        }
        FilterType::HighShelf1 => {
            // out = in + (A^2 - 1) * hp, with g prewarped by A.
            let g = w_over_two_tan(w) * a;
            let a2 = a * a;
            (a2 + g, g - a2, 0.0, 1.0 + g, g - 1.0, 0.0)
        }
        // Flat, the Linkwitz Transform (which needs its own four-parameter
        // treatment), crossover types, and anything unrecognised.
        _ => return Coeffs::PASSTHROUGH,
    };

    if a0.abs() < 1e-20 {
        return Coeffs::PASSTHROUGH;
    }

    Coeffs {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    }
}

/// The firmware's `g = tan(pi * f / fs)`, which is `tan(w/2)` for `w = 2*pi*f/fs`.
#[inline]
fn w_over_two_tan(w: f64) -> f64 {
    (w / 2.0).tan()
}

/// Magnitude of one biquad at a frequency, as a power ratio.
fn magnitude_squared(c: &Coeffs, freq: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * freq / SAMPLE_RATE;
    let (cos_w, sin_w) = (w.cos(), w.sin());
    let (cos_2w, sin_2w) = ((2.0 * w).cos(), (2.0 * w).sin());

    let num_r = c.b0 + c.b1 * cos_w + c.b2 * cos_2w;
    let num_i = -(c.b1 * sin_w + c.b2 * sin_2w);
    let den_r = 1.0 + c.a1 * cos_w + c.a2 * cos_2w;
    let den_i = -(c.a1 * sin_w + c.a2 * sin_2w);

    let den = den_r * den_r + den_i * den_i;
    if den < 1e-20 {
        return 1.0;
    }
    (num_r * num_r + num_i * num_i) / den
}

/// Combined response of a cascade at one frequency, in dB.
pub fn response_at(freq: f64, bands: &[Band]) -> f64 {
    let mut power = 1.0f64;
    for b in bands {
        if b.bypass || matches!(b.filter_type, FilterType::Flat) {
            continue;
        }
        power *= magnitude_squared(&coefficients(b), freq);
    }
    if power <= 0.0 {
        -200.0
    } else {
        10.0 * power.log10()
    }
}

/// The frequency of each plotted point, log-spaced.
pub fn frequencies() -> [f64; POINTS] {
    let mut out = [0.0; POINTS];
    let (lo, hi) = (MIN_HZ.log10(), MAX_HZ.log10());
    for (i, f) in out.iter_mut().enumerate() {
        *f = 10f64.powf(lo + (hi - lo) * i as f64 / (POINTS - 1) as f64);
    }
    out
}

/// The whole curve for one channel.
///
/// `gain_offset_db` is the channel's output trim, which shifts the curve
/// bodily rather than shaping it. The Console applies it the same way.
pub fn curve(bands: &[Band], gain_offset_db: f64) -> Vec<f64> {
    frequencies()
        .iter()
        .map(|f| response_at(*f, bands) + gain_offset_db)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peaking(freq: f32, q: f32, gain: f32) -> Band {
        Band {
            filter_type: FilterType::Peaking,
            freq,
            q,
            gain_db: gain,
            bypass: false,
        }
    }

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    #[test]
    fn the_curve_has_the_expected_shape() {
        let f = frequencies();
        assert_eq!(f.len(), POINTS);
        assert!(close(f[0], 10.0, 1e-6));
        assert!(close(f[POINTS - 1], 20_000.0, 1e-6));
        // Log spacing means a constant ratio between neighbours.
        let r1 = f[1] / f[0];
        let r2 = f[100] / f[99];
        assert!(close(r1, r2, 1e-9), "spacing should be logarithmic");
    }

    #[test]
    fn no_filters_means_a_flat_curve() {
        let c = curve(&[], 0.0);
        assert!(c.iter().all(|db| close(*db, 0.0, 1e-9)));
    }

    /// A peaking filter's gain at its centre frequency is its gain, which is the
    /// single most load-bearing property of the whole graph.
    #[test]
    fn a_peaking_filter_hits_its_gain_at_centre() {
        for gain in [-12.0, -6.0, 3.0, 8.8, 12.0] {
            let db = response_at(1000.0, &[peaking(1000.0, 2.0, gain)]);
            assert!(
                close(db, gain as f64, 0.05),
                "expected {gain} dB at centre, got {db}"
            );
        }
    }

    #[test]
    fn a_peaking_filter_decays_away_from_centre() {
        let b = [peaking(1000.0, 4.0, 12.0)];
        assert!(response_at(1000.0, &b) > response_at(500.0, &b));
        assert!(response_at(500.0, &b) > response_at(100.0, &b));
        assert!(close(response_at(20.0, &b), 0.0, 0.2), "far away is flat");
    }

    #[test]
    fn higher_q_is_narrower() {
        let wide = [peaking(1000.0, 0.5, 12.0)];
        let narrow = [peaking(1000.0, 8.0, 12.0)];
        // Both reach 12 dB at centre, but the narrow one falls away sooner.
        assert!(close(response_at(1000.0, &wide), 12.0, 0.05));
        assert!(close(response_at(1000.0, &narrow), 12.0, 0.05));
        assert!(response_at(700.0, &wide) > response_at(700.0, &narrow));
    }

    /// A shelf reaches its full gain well past the corner, and none of it well
    /// before. This is the check that catches a transposed shelf formula.
    #[test]
    fn shelves_settle_at_their_gain_on_the_right_side() {
        let low = [Band {
            filter_type: FilterType::LowShelf,
            freq: 105.0,
            q: 0.707,
            gain_db: 8.8,
            bypass: false,
        }];
        assert!(
            close(response_at(20.0, &low), 8.8, 0.3),
            "low shelf lifts the bass"
        );
        assert!(
            close(response_at(10_000.0, &low), 0.0, 0.2),
            "and leaves the top alone"
        );

        let high = [Band {
            filter_type: FilterType::HighShelf,
            freq: 10_000.0,
            q: 0.707,
            gain_db: -6.0,
            bypass: false,
        }];
        assert!(
            response_at(19_000.0, &high) < -4.0,
            "high shelf cuts the top"
        );
        assert!(
            close(response_at(100.0, &high), 0.0, 0.2),
            "and leaves the bass alone"
        );
    }

    #[test]
    fn a_low_pass_is_minus_three_db_at_its_corner() {
        let b = [Band {
            filter_type: FilterType::LowPass,
            freq: 1000.0,
            q: 0.707,
            gain_db: 0.0,
            bypass: false,
        }];
        assert!(close(response_at(1000.0, &b), -3.0, 0.15));
        assert!(close(response_at(100.0, &b), 0.0, 0.2));
        assert!(response_at(8000.0, &b) < -30.0);
    }

    #[test]
    fn a_high_pass_mirrors_it() {
        let b = [Band {
            filter_type: FilterType::HighPass,
            freq: 80.0,
            q: 0.707,
            gain_db: 0.0,
            bypass: false,
        }];
        assert!(close(response_at(80.0, &b), -3.0, 0.15));
        assert!(response_at(20.0, &b) < -20.0);
        assert!(close(response_at(5000.0, &b), 0.0, 0.2));
    }

    #[test]
    fn a_notch_cuts_hard_at_its_frequency() {
        let b = [Band {
            filter_type: FilterType::Notch,
            freq: 1000.0,
            q: 10.0,
            gain_db: 0.0,
            bypass: false,
        }];
        assert!(response_at(1000.0, &b) < -40.0);
        assert!(close(response_at(200.0, &b), 0.0, 0.3));
    }

    /// An all-pass changes phase, not level. If it moves the magnitude curve the
    /// implementation is wrong.
    #[test]
    fn all_pass_filters_do_not_change_level() {
        for ty in [FilterType::AllPass, FilterType::AllPass1] {
            let b = [Band {
                filter_type: ty,
                freq: 1000.0,
                q: 2.0,
                gain_db: 0.0,
                bypass: false,
            }];
            for f in [50.0, 500.0, 1000.0, 5000.0, 15000.0] {
                assert!(
                    close(response_at(f, &b), 0.0, 0.05),
                    "{ty:?} moved the level at {f} Hz"
                );
            }
        }
    }

    #[test]
    fn cascaded_bands_add_in_decibels() {
        let two = [peaking(1000.0, 2.0, 6.0), peaking(1000.0, 2.0, 6.0)];
        assert!(close(response_at(1000.0, &two), 12.0, 0.1));
    }

    #[test]
    fn a_bypassed_band_contributes_nothing() {
        let mut b = peaking(1000.0, 2.0, 12.0);
        b.bypass = true;
        assert!(close(response_at(1000.0, &[b]), 0.0, 1e-9));
    }

    /// An unrecognised type must not be drawn as though it were flat *and* must
    /// not throw the curve off; it contributes nothing and the UI says so.
    #[test]
    fn an_unknown_filter_type_contributes_nothing() {
        let b = [Band {
            filter_type: FilterType::from_raw(200),
            freq: 1000.0,
            q: 2.0,
            gain_db: 12.0,
            bypass: false,
        }];
        assert!(close(response_at(1000.0, &b), 0.0, 1e-9));
    }

    #[test]
    fn output_gain_shifts_the_whole_curve() {
        let c = curve(&[peaking(1000.0, 2.0, 6.0)], -3.0);
        assert!(
            close(c[0], -3.0, 0.1),
            "the flat region moves with the trim"
        );
    }

    /// Nothing in the plotted range may produce a non-finite value; a NaN would
    /// propagate into the renderer and blank the graph.
    #[test]
    fn every_type_is_finite_across_the_whole_range() {
        for raw in 0u8..=12 {
            for q in [0.1f32, 0.707, 20.0] {
                for freq in [10.0f32, 1000.0, 23_999.0] {
                    let b = [Band {
                        filter_type: FilterType::from_raw(raw),
                        freq,
                        q,
                        gain_db: 12.0,
                        bypass: false,
                    }];
                    for db in curve(&b, 0.0) {
                        assert!(db.is_finite(), "type {raw} at {freq} Hz Q{q} produced {db}");
                    }
                }
            }
        }
    }

    #[test]
    fn frequencies_at_or_above_nyquist_are_ignored_safely() {
        let b = [peaking(24_000.0, 2.0, 12.0)];
        assert!(curve(&b, 0.0).iter().all(|db| db.is_finite()));
    }
}

#[cfg(test)]
mod first_order_tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    /// A first-order low shelf must reach its full gain at DC and none of it at
    /// the top, exactly like the second-order form but with a gentler slope.
    #[test]
    fn first_order_low_shelf_settles_correctly() {
        for gain in [-9.0f32, -3.0, 6.0] {
            let b = [Band {
                filter_type: FilterType::LowShelf1,
                freq: 200.0,
                q: 0.707,
                gain_db: gain,
                bypass: false,
            }];
            let bottom = response_at(10.0, &b);
            let top = response_at(19_000.0, &b);
            assert!(
                close(bottom, gain as f64, 0.5),
                "low shelf {gain} dB: bass settled at {bottom}"
            );
            assert!(
                close(top, 0.0, 0.5),
                "low shelf {gain} dB: top moved to {top}"
            );
        }
    }

    #[test]
    fn first_order_high_shelf_settles_correctly() {
        for gain in [-9.0f32, -3.0, 6.0] {
            let b = [Band {
                filter_type: FilterType::HighShelf1,
                freq: 4000.0,
                q: 0.707,
                gain_db: gain,
                bypass: false,
            }];
            let bottom = response_at(20.0, &b);
            let top = response_at(19_000.0, &b);
            assert!(
                close(bottom, 0.0, 0.5),
                "high shelf {gain} dB: bass moved to {bottom}"
            );
            assert!(
                close(top, gain as f64, 0.5),
                "high shelf {gain} dB: top settled at {top}"
            );
        }
    }

    /// The midpoint of a shelf sits at half its gain. This distinguishes a
    /// correct shelf from one that merely settles at the right endpoints.
    #[test]
    fn first_order_shelves_cross_half_gain_at_the_corner() {
        let b = [Band {
            filter_type: FilterType::LowShelf1,
            freq: 200.0,
            q: 0.707,
            gain_db: 12.0,
            bypass: false,
        }];
        // The A prewarp is what puts the midpoint at the stated corner; without
        // it the shelf would cross well off frequency.
        let mid = response_at(200.0, &b);
        assert!(
            close(mid, 6.0, 0.6),
            "corner should sit near half the 12 dB gain, got {mid}"
        );
    }
}
