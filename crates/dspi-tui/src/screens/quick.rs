//! Parsing helpers for the page command bar (DESIGN 13).
//!
//! Every page grammar is built from the same three pieces, so the bar feels
//! like one language everywhere: channel lists (`1 3`, `1-4`, `all`),
//! numbers that take a `k` (`1k`, `2.5k`, `-6`), and verbs matched by any
//! unambiguous prefix (`ga` is `gain`).

/// One channel token as 0-based indices: `3` is the third channel, `1-4` a
/// range, `all` or `*` every one of `max`.
pub fn channel_token(tok: &str, max: usize) -> Option<Vec<usize>> {
    if tok == "all" || tok == "*" {
        return Some((0..max).collect());
    }
    if let Some((a, b)) = tok.split_once('-') {
        let (a, b) = (a.parse::<usize>().ok()?, b.parse::<usize>().ok()?);
        if a == 0 || b < a || b > max {
            return None;
        }
        return Some((a - 1..b).collect());
    }
    let n = tok.parse::<usize>().ok()?;
    if n == 0 || n > max {
        return None;
    }
    Some(vec![n - 1])
}

/// A run of channel tokens, deduplicated, in the order given.
pub fn channel_list(tokens: &[&str], max: usize) -> Option<Vec<usize>> {
    if tokens.is_empty() {
        return None;
    }
    let mut out: Vec<usize> = Vec::new();
    for t in tokens {
        for ch in channel_token(t, max)? {
            if !out.contains(&ch) {
                out.push(ch);
            }
        }
    }
    Some(out)
}

/// A number, with `k` for thousands: `1k`, `2.5k`, `-6`, `80`.
pub fn number(tok: &str) -> Option<f64> {
    if let Some(head) = tok.strip_suffix('k').or_else(|| tok.strip_suffix('K')) {
        return head.parse::<f64>().ok().map(|v| v * 1000.0);
    }
    tok.parse::<f64>().ok()
}

/// The one verb `tok` is an unambiguous prefix of, if any.
pub fn verb<'a>(tok: &str, verbs: &[&'a str]) -> Option<&'a str> {
    let mut hits = verbs.iter().filter(|v| v.starts_with(tok));
    match (hits.next(), hits.next()) {
        (Some(v), None) => Some(v),
        _ => None,
    }
}

/// The ghost completion for a first token: the rest of the verb it starts.
pub fn ghost(input: &str, verbs: &[&str]) -> Option<String> {
    let tok = input.trim_start();
    if tok.is_empty() || tok.contains(' ') || !tok.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let v = verb(&tok.to_ascii_lowercase(), verbs)?;
    v.strip_prefix(&tok.to_ascii_lowercase())
        .filter(|rest| !rest.is_empty())
        .map(|rest| rest.to_string())
}

/// `IN1 IN3` for a hint, from 0-based indices.
pub fn names(prefix: &str, list: &[usize]) -> String {
    list.iter()
        .map(|i| format!("{prefix}{}", i + 1))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The one-switch grammar the tool panels share: `on` and `off` drive the
/// panel's enable path; anything else shows the summary and falls through
/// to the shared `:` grammar.
pub fn toggle(line: &str, path: &str, what: &str) -> crate::shell::Quick {
    let tok = line.trim().to_ascii_lowercase();
    let (hint, commands) = match verb(&tok, &["on", "off"]) {
        Some(v) if !tok.is_empty() => (format!("{what} {v}"), vec![format!("{path} {v}")]),
        _ => (
            "on · off · anything else runs the : grammar".to_string(),
            Vec::new(),
        ),
    };
    crate::shell::Quick {
        hint,
        ghost: ghost(&tok, &["on", "off"]),
        commands,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_input_pages_grammar_writes_preamp_bands_delay_and_names() {
        use crate::screens::{InputPage, shared};
        use crate::shell::{Screen, fixture};
        let state = fixture::state();
        let p = InputPage::new(0, shared(), &state);
        let q = |line: &str| p.quick(line, &state).unwrap();
        assert_eq!(q("pre -4").commands, vec!["pre 0 -4"]);
        assert_eq!(q("3 peak 1k -2").commands, vec!["eq in.1 3 peak 1000 1 -2"]);
        assert_eq!(
            q("3 ls 100 4").commands,
            vec!["eq in.1 3 lowshelf 100 0.707 4"]
        );
        assert_eq!(q("3 off").commands, vec!["eq in.1 3 flat 1000 0.707 0"]);
        assert_eq!(q("delay 2.5").commands, vec!["ch.delay in.1 2.5"]);
        assert_eq!(q("name Front L").commands, vec!["ch.name 0 Front L"]);
        assert!(!q("clear").commands.is_empty());
        for partial in ["", "pre", "3", "3 peak", "3 peak 1k", "99 off", "de"] {
            let r = q(partial);
            assert!(r.commands.is_empty(), "{partial:?} ran {:?}", r.commands);
            assert!(!r.hint.is_empty());
        }
        assert_eq!(q("de").ghost.as_deref(), Some("lay"));
    }

    #[test]
    fn the_output_pages_grammar_covers_its_strip_and_the_crossover() {
        use crate::screens::{OutputPage, shared};
        use crate::shell::{Screen, fixture};
        let state = fixture::state();
        let p = OutputPage::new(0, shared(), &state);
        let q = |line: &str| p.quick(line, &state).unwrap();
        assert_eq!(q("gain -3").commands, vec!["out.gain 0 -3"]);
        assert_eq!(q("delay 2.5").commands, vec!["out.delay 0 2.5"]);
        assert_eq!(q("mute").commands, vec!["out.mute 0 on"]);
        assert_eq!(q("unmute").commands, vec!["out.mute 0 off"]);
        assert_eq!(q("off").commands, vec!["out.enable 0 off"]);
        assert_eq!(q("xo hp 80").commands, vec!["eq out.1 20 lr4hp 80 0.707 0"]);
        assert_eq!(
            q("xo lp 120 bw2").commands,
            vec!["eq out.1 20 bw2lp 120 0.707 0"]
        );
        assert_eq!(q("xo off").commands.len(), 4);
        assert_eq!(
            q("3 peak 1k -2").commands,
            vec!["eq out.1 3 peak 1000 1 -2"]
        );
        for partial in ["", "gain", "xo", "xo hp", "3"] {
            let r = q(partial);
            assert!(r.commands.is_empty(), "{partial:?} ran {:?}", r.commands);
            assert!(!r.hint.is_empty());
        }
    }

    #[test]
    fn the_panel_toggle_is_two_words() {
        assert_eq!(
            toggle("on", "cf.on", "Crossfeed").commands,
            vec!["cf.on on"]
        );
        assert_eq!(
            toggle("of", "lev.on", "Leveller").commands,
            vec!["lev.on off"]
        );
        assert!(
            toggle("o", "cf.on", "Crossfeed").commands.is_empty(),
            "ambiguous"
        );
        assert!(toggle("", "cf.on", "Crossfeed").commands.is_empty());
    }

    #[test]
    fn channel_tokens_cover_singles_ranges_and_all() {
        assert_eq!(channel_token("3", 9), Some(vec![2]));
        assert_eq!(channel_token("1-4", 9), Some(vec![0, 1, 2, 3]));
        assert_eq!(channel_token("all", 3), Some(vec![0, 1, 2]));
        assert_eq!(channel_token("0", 9), None, "channels are 1-based");
        assert_eq!(channel_token("10", 9), None, "out of range");
        assert_eq!(channel_token("4-2", 9), None, "backwards");
        assert_eq!(
            channel_list(&["1", "3-4", "1"], 9),
            Some(vec![0, 2, 3]),
            "deduplicated, in order"
        );
    }

    #[test]
    fn numbers_take_the_k_suffix() {
        assert_eq!(number("1k"), Some(1000.0));
        assert_eq!(number("2.5K"), Some(2500.0));
        assert_eq!(number("-6"), Some(-6.0));
        assert_eq!(number("x"), None);
    }

    #[test]
    fn verbs_match_by_unambiguous_prefix() {
        let verbs = ["gain", "delay", "direct", "mute"];
        assert_eq!(verb("ga", &verbs), Some("gain"));
        assert_eq!(verb("d", &verbs), None, "delay or direct");
        assert_eq!(verb("de", &verbs), Some("delay"));
        assert_eq!(ghost("ga", &verbs), Some("in".into()));
        assert_eq!(ghost("gain 1", &verbs), None);
    }
}
