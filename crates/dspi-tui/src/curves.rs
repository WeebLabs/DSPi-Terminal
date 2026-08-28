//! The maths behind the three small graphs in the DSP tool panels.
//!
//! Ported from the Console, formula for formula, so the terminal draws the
//! same picture: the complementary crossfeed pair from `CrossfeedView.swift`,
//! the ISO 226:2003 loudness compensation from `LoudnessView.swift` (the
//! table at `:6-44`, the computation at `:85-107`), and the schematic
//! psychoacoustic-bass spectrum from `PsychoacousticBassView.swift`.
//!
//! Everything here is a pure function of the parameters, evaluated on
//! `dspi_proto::dsp`'s 201-point log frequency grid so the results can be
//! handed straight to the shared [`crate::graph::Graph`].

use dspi_proto::dsp;

/// The rate the crossfeed one-pole is designed at. The Console hard-codes it
/// rather than reading the device's rate, and the shape barely moves with the
/// rate, so the picture is stable while the device retunes.
pub const CROSSFEED_SAMPLE_RATE: f64 = 48_000.0;

/// The playback level the compensation curve is drawn at, in dB below the
/// reference. `CompensationCurveView.visualizationVolumeDB`.
pub const LOUDNESS_VISUALIZATION_DB: f64 = -40.0;

// ---------------------------------------------------------------- crossfeed

/// The crossfeed and direct paths at one frequency, in dB.
///
/// The crossfed path is a one-pole lowpass at `fc` scaled by the
/// complementary gain `G = 1 / (1 + 10^(feed/20))`; the direct path is
/// `1 - H(z)` of that same filter, so the two always sum to unity and a
/// mono signal comes out untouched.
pub fn crossfeed_at(fc: f64, feed_db: f64, hz: f64) -> (f64, f64) {
    let fs = CROSSFEED_SAMPLE_RATE;
    let level_ratio = 10f64.powf(feed_db / 20.0);
    let g = 1.0 / (1.0 + level_ratio);
    let x = (-2.0 * std::f64::consts::PI * fc / fs).exp();
    let a0 = g * (1.0 - x);

    let omega = 2.0 * std::f64::consts::PI * hz / fs;
    let den_re = 1.0 - x * omega.cos();
    let den_im = x * omega.sin();
    let den2 = den_re * den_re + den_im * den_im;
    let lp_re = a0 * den_re / den2;
    let lp_im = -a0 * den_im / den2;

    let lp = (lp_re * lp_re + lp_im * lp_im).sqrt();
    let direct_re = 1.0 - lp_re;
    let direct_im = -lp_im;
    let direct = (direct_re * direct_re + direct_im * direct_im).sqrt();
    (db(lp), db(direct))
}

fn db(mag: f64) -> f64 {
    20.0 * mag.max(1e-10).log10()
}

/// The two crossfeed curves on `dsp::frequencies()`: crossfed first, direct
/// second.
pub fn crossfeed_curves(fc: f64, feed_db: f64) -> (Vec<f64>, Vec<f64>) {
    let mut cross = Vec::with_capacity(dsp::POINTS);
    let mut direct = Vec::with_capacity(dsp::POINTS);
    for hz in dsp::frequencies() {
        let (c, d) = crossfeed_at(fc, feed_db, hz);
        cross.push(c);
        direct.push(d);
    }
    (cross, direct)
}

// ------------------------------------------------------------------ ISO 226

/// One row of the ISO 226:2003 equal-loudness-contour table.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Iso226 {
    /// Centre frequency, Hz.
    pub f: f64,
    /// Exponent of loudness perception at this frequency.
    pub af: f64,
    /// Magnitude of the linear transfer function, dB.
    pub lu: f64,
    /// Threshold of hearing, dB.
    pub tf: f64,
}

/// The Console's table, `LoudnessView.swift:13-44`, verbatim.
pub const ISO226: [Iso226; 30] = [
    Iso226 {
        f: 20.0,
        af: 0.532,
        lu: -31.6,
        tf: 78.5,
    },
    Iso226 {
        f: 25.0,
        af: 0.506,
        lu: -27.2,
        tf: 68.7,
    },
    Iso226 {
        f: 31.5,
        af: 0.480,
        lu: -23.0,
        tf: 59.5,
    },
    Iso226 {
        f: 40.0,
        af: 0.455,
        lu: -19.1,
        tf: 51.1,
    },
    Iso226 {
        f: 50.0,
        af: 0.432,
        lu: -15.9,
        tf: 44.0,
    },
    Iso226 {
        f: 63.0,
        af: 0.409,
        lu: -13.0,
        tf: 37.5,
    },
    Iso226 {
        f: 80.0,
        af: 0.387,
        lu: -10.3,
        tf: 31.5,
    },
    Iso226 {
        f: 100.0,
        af: 0.367,
        lu: -8.1,
        tf: 26.5,
    },
    Iso226 {
        f: 125.0,
        af: 0.349,
        lu: -6.2,
        tf: 22.1,
    },
    Iso226 {
        f: 160.0,
        af: 0.330,
        lu: -4.5,
        tf: 17.9,
    },
    Iso226 {
        f: 200.0,
        af: 0.315,
        lu: -3.1,
        tf: 14.4,
    },
    Iso226 {
        f: 250.0,
        af: 0.301,
        lu: -2.0,
        tf: 11.4,
    },
    Iso226 {
        f: 315.0,
        af: 0.288,
        lu: -1.1,
        tf: 8.6,
    },
    Iso226 {
        f: 400.0,
        af: 0.276,
        lu: -0.4,
        tf: 6.2,
    },
    Iso226 {
        f: 500.0,
        af: 0.267,
        lu: 0.0,
        tf: 4.4,
    },
    Iso226 {
        f: 630.0,
        af: 0.259,
        lu: 0.3,
        tf: 3.0,
    },
    Iso226 {
        f: 800.0,
        af: 0.253,
        lu: 0.5,
        tf: 2.2,
    },
    Iso226 {
        f: 1000.0,
        af: 0.250,
        lu: 0.0,
        tf: 2.4,
    },
    Iso226 {
        f: 1250.0,
        af: 0.246,
        lu: -2.7,
        tf: 3.5,
    },
    Iso226 {
        f: 1600.0,
        af: 0.244,
        lu: -4.1,
        tf: 1.7,
    },
    Iso226 {
        f: 2000.0,
        af: 0.243,
        lu: -1.0,
        tf: -1.3,
    },
    Iso226 {
        f: 2500.0,
        af: 0.243,
        lu: 1.7,
        tf: -4.2,
    },
    Iso226 {
        f: 3150.0,
        af: 0.243,
        lu: 2.5,
        tf: -6.0,
    },
    Iso226 {
        f: 4000.0,
        af: 0.242,
        lu: 1.2,
        tf: -5.4,
    },
    Iso226 {
        f: 5000.0,
        af: 0.242,
        lu: -2.1,
        tf: -1.5,
    },
    Iso226 {
        f: 6300.0,
        af: 0.245,
        lu: -7.1,
        tf: 6.0,
    },
    Iso226 {
        f: 8000.0,
        af: 0.254,
        lu: -11.2,
        tf: 12.6,
    },
    Iso226 {
        f: 10000.0,
        af: 0.271,
        lu: -10.7,
        tf: 13.9,
    },
    Iso226 {
        f: 12500.0,
        af: 0.301,
        lu: -3.1,
        tf: 12.3,
    },
    Iso226 {
        f: 16000.0,
        af: 0.310,
        lu: -2.0,
        tf: 17.0,
    },
];

/// The sound pressure level of an equal-loudness contour at one table row.
///
/// `iso226SPL` in `LoudnessView.swift:87-95`.
pub fn iso226_spl(row: Iso226, phon: f64) -> f64 {
    let b = 0.4 * 10f64.powf((row.tf + row.lu) / 10.0 - 9.0);
    let threshold = b.powf(row.af);
    let mut af = 4.47e-3 * (10f64.powf(0.025 * phon) - 1.15) + threshold;
    if af < 1e-10 {
        af = 1e-10;
    }
    (10.0 / row.af) * af.log10() - row.lu + 94.0
}

/// The compensation this row needs at a given listening level.
///
/// `loudnessCompensationDB` in `LoudnessView.swift:97-107`: the difference
/// between the two contours minus the flat level difference, scaled by the
/// intensity percentage.
pub fn loudness_compensation_db(
    row: Iso226,
    ref_spl: f64,
    effective_phon: f64,
    intensity: f64,
) -> f64 {
    if effective_phon >= ref_spl {
        return 0.0;
    }
    let spl_ref = iso226_spl(row, ref_spl);
    let spl_eff = iso226_spl(row, effective_phon);
    let flat = effective_phon - ref_spl;
    let freq = spl_eff - spl_ref;
    (freq - flat) * (intensity / 100.0)
}

/// The phon the compensation curve is drawn at: the reference minus 40 dB,
/// with the Console's two clamps.
pub fn effective_phon(ref_spl: f64) -> f64 {
    let mut phon = ref_spl + LOUDNESS_VISUALIZATION_DB;
    if phon < 20.0 {
        phon = 20.0;
    }
    if phon > ref_spl {
        phon = ref_spl;
    }
    phon
}

/// The compensation curve as the Console plots it: one point per table row.
pub fn loudness_points(ref_spl: f64, intensity: f64) -> Vec<(f64, f64)> {
    let phon = effective_phon(ref_spl);
    ISO226
        .iter()
        .map(|row| {
            (
                row.f,
                loudness_compensation_db(*row, ref_spl, phon, intensity),
            )
        })
        .collect()
}

/// The same curve resampled onto `dsp::frequencies()`, interpolated linearly
/// in dB against log frequency between table rows and held flat outside them,
/// which is what the Console's straight-line path between points looks like.
pub fn loudness_curve(ref_spl: f64, intensity: f64) -> Vec<f64> {
    let points = loudness_points(ref_spl, intensity);
    dsp::frequencies()
        .iter()
        .map(|hz| interpolate_log(&points, *hz))
        .collect()
}

/// Linear interpolation in log frequency over a sorted point list.
pub fn interpolate_log(points: &[(f64, f64)], hz: f64) -> f64 {
    match points {
        [] => 0.0,
        [(_, v)] => *v,
        _ => {
            let first = points[0];
            let last = points[points.len() - 1];
            if hz <= first.0 {
                return first.1;
            }
            if hz >= last.0 {
                return last.1;
            }
            let i = points
                .iter()
                .position(|(f, _)| *f >= hz)
                .unwrap_or(points.len() - 1);
            let (f0, v0) = points[i - 1];
            let (f1, v1) = points[i];
            let t = (hz.log10() - f0.log10()) / (f1.log10() - f0.log10());
            v0 + t * (v1 - v0)
        }
    }
}

// ------------------------------------------------------------------ psybass

/// How tall the original low band is drawn, 0 to 1.
///
/// `PsybassSpectrumView.originalFrac`: 0 dB fills the plot, -60 dB empties it.
pub fn psybass_original_fraction(original_db: f64) -> f64 {
    ((original_db + 60.0) / 60.0).clamp(0.0, 1.0)
}

/// How tall the synthesised harmonic band is drawn, 0 to 1.
///
/// `PsybassSpectrumView.harmonicsFrac`: -24 dB is empty, +12 dB is full.
pub fn psybass_harmonics_fraction(harmonics_db: f64) -> f64 {
    ((harmonics_db + 24.0) / 36.0).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    /// At DC the one-pole passes its whole gain, so the crossfed path is
    /// exactly `20 log10 G` and the direct path `20 log10 (1 - G)`. With the
    /// feed at 0 dB the two halves are equal, which is -6.0206 dB each.
    #[test]
    fn the_crossfeed_pair_is_the_complementary_split_at_dc() {
        let (cross, direct) = crossfeed_at(700.0, 0.0, 0.0);
        assert!(close(cross, -6.0206, 1e-3), "{cross}");
        assert!(close(direct, -6.0206, 1e-3), "{direct}");

        // The Console's default voicing: feed 4.5 dB gives a level ratio of
        // 10^0.225 = 1.6788040, so G = 1/2.6788040 = 0.3733027 (-8.5588 dB)
        // and the direct path keeps 1 - G = 0.6266973 (-4.0588 dB).
        let (cross, direct) = crossfeed_at(700.0, 4.5, 0.0);
        assert!(close(cross, -8.5588, 1e-3), "{cross}");
        assert!(close(direct, -4.0588, 1e-3), "{direct}");
    }

    /// A quarter of the sample rate puts the pole at right angles, where the
    /// magnitude is `a0 / sqrt(1 + x^2)`. With fc = 700 Hz and fs = 48 kHz,
    /// x = exp(-0.0916298) = 0.9124430, a0 = 0.3733027 * 0.0875570 =
    /// 0.0326828, and sqrt(1 + x^2) = 1.3536806, so the crossfed path is
    /// -32.3436 dB.
    #[test]
    fn the_crossfeed_lowpass_rolls_off_at_a_quarter_of_the_rate() {
        let (cross, _) = crossfeed_at(700.0, 4.5, CROSSFEED_SAMPLE_RATE / 4.0);
        assert!(close(cross, -32.3436, 1e-3), "{cross}");
    }

    /// The two paths are complementary by construction, so their transfer
    /// functions sum to one at every frequency: where one is -3 dB the other
    /// cannot also be, and a summed mono signal is untouched.
    #[test]
    fn the_two_paths_stay_complementary_across_the_band() {
        for hz in [20.0, 100.0, 700.0, 5_000.0, 20_000.0] {
            let (cross, direct) = crossfeed_at(700.0, 4.5, hz);
            // Both are magnitudes of vectors summing to 1, so neither can
            // exceed unity and the larger is always at least -6 dB.
            assert!(cross <= 0.01, "{hz}: {cross}");
            assert!(direct <= 0.01, "{hz}: {direct}");
            assert!(cross.max(direct) >= -6.03, "{hz}: {cross} {direct}");
        }
        let (_, direct) = crossfeed_at(700.0, 4.5, 20_000.0);
        assert!(
            direct > -0.2,
            "the direct path is flat at the top: {direct}"
        );
    }

    #[test]
    fn the_crossfeed_curves_land_on_the_shared_frequency_grid() {
        let (cross, direct) = crossfeed_curves(700.0, 4.5);
        assert_eq!(cross.len(), dsp::POINTS);
        assert_eq!(direct.len(), dsp::POINTS);
        let f = dsp::frequencies();
        assert!(close(cross[0], crossfeed_at(700.0, 4.5, f[0]).0, 1e-9));
    }

    /// One kilohertz is where the ISO contours are defined, so the reference
    /// contour reads back its own phon value: 80 phon gives 80.0121 dB SPL
    /// and 40 phon gives 40.0100, worked out from the table row af 0.250,
    /// lu 0.0, tf 2.4 by hand.
    #[test]
    fn the_iso_contour_reproduces_its_phon_at_one_kilohertz() {
        let row = ISO226[17];
        assert_eq!(row.f, 1000.0);
        assert!(close(iso226_spl(row, 80.0), 80.0121, 1e-3));
        assert!(close(iso226_spl(row, 40.0), 40.0100, 1e-3));
        // And so the compensation there is nothing at all, which is what
        // makes 1 kHz the pivot of the whole curve.
        let c = loudness_compensation_db(row, 80.0, 40.0, 100.0);
        assert!(close(c, 0.0, 0.01), "{c}");
    }

    /// Twenty hertz, the bottom of the table: af 0.532, lu -31.6, tf 78.5.
    /// By hand, the 80 phon contour is 118.9900 dB SPL there and the 40 phon
    /// contour 99.8539, so listening 40 dB down needs 99.8539 - 118.9900 + 40
    /// = +20.8639 dB of bass back.
    #[test]
    fn the_compensation_lifts_twenty_hertz_by_the_hand_computed_amount() {
        let row = ISO226[0];
        assert_eq!(row.f, 20.0);
        assert!(close(iso226_spl(row, 80.0), 118.9900, 1e-3));
        assert!(close(iso226_spl(row, 40.0), 99.8539, 1e-3));
        let c = loudness_compensation_db(row, 80.0, 40.0, 100.0);
        assert!(close(c, 20.8639, 1e-3), "{c}");
        // Intensity scales it linearly, and zero bypasses the curve.
        assert!(close(
            loudness_compensation_db(row, 80.0, 40.0, 200.0),
            2.0 * c,
            1e-9
        ));
        assert_eq!(loudness_compensation_db(row, 80.0, 40.0, 0.0), 0.0);
    }

    #[test]
    fn the_compensation_vanishes_at_or_above_the_reference() {
        let row = ISO226[0];
        assert_eq!(loudness_compensation_db(row, 40.0, 40.0, 100.0), 0.0);
        assert_eq!(loudness_compensation_db(row, 40.0, 60.0, 100.0), 0.0);
        // At a 40 dB reference the visualisation volume would put the
        // effective phon at 0, so the Console clamps it to 20.
        assert_eq!(effective_phon(40.0), 20.0);
        assert_eq!(effective_phon(80.0), 40.0);
    }

    #[test]
    fn the_loudness_curve_is_a_polyline_through_the_table() {
        let points = loudness_points(80.0, 100.0);
        assert_eq!(points.len(), 30);
        let curve = loudness_curve(80.0, 100.0);
        assert_eq!(curve.len(), dsp::POINTS);
        // Below the table's first row the curve holds the 20 Hz value.
        assert!(close(curve[0], points[0].1, 1e-9));
        // Halfway in log frequency between two rows is halfway in dB.
        let mid = (20.0f64.log10() + 25.0f64.log10()) / 2.0;
        let want = (points[0].1 + points[1].1) / 2.0;
        assert!(close(interpolate_log(&points, 10f64.powf(mid)), want, 1e-9));
        // And bass gets more help than treble, which is the whole point.
        assert!(points[0].1 > points[17].1 + 10.0);
    }

    #[test]
    fn the_psybass_bars_scale_the_way_the_console_draws_them() {
        assert_eq!(psybass_original_fraction(0.0), 1.0);
        assert_eq!(psybass_original_fraction(-60.0), 0.0);
        assert!(close(psybass_original_fraction(-30.0), 0.5, 1e-12));
        assert_eq!(psybass_original_fraction(-90.0), 0.0);

        assert_eq!(psybass_harmonics_fraction(12.0), 1.0);
        assert_eq!(psybass_harmonics_fraction(-24.0), 0.0);
        assert!(close(psybass_harmonics_fraction(-6.0), 0.5, 1e-12));
        assert_eq!(psybass_harmonics_fraction(24.0), 1.0);
    }
}
