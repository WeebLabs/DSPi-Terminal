//! Text helpers shared by the kit: truncation, alignment, and the number
//! formats the Console uses for each unit.

use unicode_width::UnicodeWidthStr;

/// Cut a string to `width` columns, ending in `…` when it had to be cut.
pub fn truncate(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw > width - 1 {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// Pad or cut to exactly `width` columns, left-aligned.
pub fn fit_left(s: &str, width: usize) -> String {
    let t = truncate(s, width);
    format!("{t:<w$}", w = width + (t.chars().count() - t.width()))
}

/// Pad or cut to exactly `width` columns, right-aligned.
pub fn fit_right(s: &str, width: usize) -> String {
    let t = truncate(s, width);
    format!("{t:>w$}", w = width + (t.chars().count() - t.width()))
}

/// Centre in `width` columns.
pub fn fit_centre(s: &str, width: usize) -> String {
    let t = truncate(s, width);
    let pad = width.saturating_sub(t.width());
    let left = pad / 2;
    format!("{}{}{}", " ".repeat(left), t, " ".repeat(pad - left))
}

/// Frequency as the Console prints it: an integer with the unit.
pub fn hz(v: f64) -> String {
    format!("{} Hz", v.round() as i64)
}

/// Gain with a sign, one decimal, as shown (three are kept when editing).
pub fn db(v: f64) -> String {
    format!("{v:+.1} dB")
}

/// Gain without forcing a sign, for levels and thresholds.
pub fn db_plain(v: f64) -> String {
    format!("{v:.1} dB")
}

/// Q to three decimals with trailing zeros stripped, as the Console's
/// `ValueField(stripTrailingZeros)` does.
pub fn q(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() { "0".into() } else { s.into() }
}

pub fn ms(v: f64) -> String {
    format!("{v:.1} ms")
}

pub fn pct(v: f64) -> String {
    format!("{}%", v.round() as i64)
}

/// A value with a unit, choosing the format by the unit string.
pub fn with_unit(v: f64, unit: &str, decimals: usize) -> String {
    match unit {
        "Hz" => hz(v),
        "dB" => {
            if decimals == 0 {
                format!("{:+.0} dB", v)
            } else {
                format!("{v:+.d$} dB", d = decimals)
            }
        }
        "Q" => q(v),
        "ms" => format!("{v:.d$} ms", d = decimals),
        "%" => {
            if decimals == 0 {
                pct(v)
            } else {
                format!("{v:.d$}%", d = decimals)
            }
        }
        "" => format!("{v:.d$}", d = decimals),
        other => format!("{v:.d$} {other}", d = decimals),
    }
}

/// Wrap a caption to `width` columns, at most `max_lines` lines.
pub fn wrap(s: &str, width: usize, max_lines: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in s.split_whitespace() {
        let need = if cur.is_empty() {
            word.width()
        } else {
            cur.width() + 1 + word.width()
        };
        if need > width && !cur.is_empty() {
            lines.push(std::mem::take(&mut cur));
            if lines.len() == max_lines {
                break;
            }
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() && lines.len() < max_lines {
        lines.push(cur);
    }
    let used: usize = lines.iter().map(|l| l.split_whitespace().count()).sum();
    if lines.len() == max_lines
        && s.split_whitespace().count() > used
        && let Some(last) = lines.last_mut()
    {
        *last = truncate(&format!("{last} …"), width);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_keeps_the_width_exact() {
        assert_eq!(truncate("Living Room", 6), "Livin…");
        assert_eq!(truncate("Sub", 6), "Sub");
        assert_eq!(fit_left("Sub", 6), "Sub   ");
        // A cut string ends in an ellipsis, which is one column and three
        // bytes; padding must count columns.
        assert_eq!(fit_left("Living Room", 6).chars().count(), 6);
        assert_eq!(fit_right("Living Room", 6).chars().count(), 6);
        assert_eq!(fit_right("Sub", 6), "   Sub");
        assert_eq!(fit_centre("ab", 6), "  ab  ");
    }

    #[test]
    fn numbers_follow_the_consoles_formats() {
        assert_eq!(hz(2856.4), "2856 Hz");
        assert_eq!(db(-8.6), "-8.6 dB");
        assert_eq!(db(3.6), "+3.6 dB");
        assert_eq!(q(0.707), "0.707");
        assert_eq!(q(3.58), "3.58");
        assert_eq!(q(1.0), "1");
        assert_eq!(ms(2.5), "2.5 ms");
        assert_eq!(pct(31.4), "31%");
    }

    #[test]
    fn captions_wrap_and_stop_at_the_line_limit() {
        let c = "Simulates head shadow lowpass cutoff. Lower = more bass crossfeed. Typical: 650-700 Hz.";
        let lines = wrap(c, 40, 2);
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| l.width() <= 40), "{lines:?}");
        assert!(lines[1].ends_with('…'), "{lines:?}");
        assert_eq!(wrap("short", 40, 2), vec!["short".to_string()]);
    }
}
