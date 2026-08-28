//! Settings > About.
//!
//! The Console's About page, minus the icon: the app name, what it is, the
//! version, the credit, and the five support links. The Console opens each
//! link in a browser; a terminal shows the URL, which is the thing a person
//! can act on either way.

use crate::widgets::{Action, KeyHelp};

use super::{Cx, PageEvent, Row, SettingsPage};

/// The Console's `Links & Support` list, in its order
/// (`DSPi_ConsoleApp.swift:695-704`).
pub const LINKS: [(&str, &str); 5] = [
    ("YouTube", "https://youtube.com/weeblabs"),
    ("GitHub", "https://github.com/weeblabs"),
    ("Discord", "https://discord.gg/RCyqxAQ5xS"),
    ("Patreon", "https://patreon.com/weeblabs"),
    ("Ko-fi", "https://ko-fi.com/weeblabs"),
];

/// The Console's paragraph above the links.
pub const SUPPORT: &str = "DSPi Firmware and Console are free, open-source software that I \
                           develop in my spare time. Contributions of any kind - code, \
                           feedback, funding, or otherwise - are always immensely appreciated.";

#[derive(Debug, Default)]
pub struct AboutPage;

const KEYS: &[KeyHelp] = &[
    KeyHelp::new("↑ ↓", "Scroll"),
    KeyHelp::new("Tab", "Sidebar, page, save bar"),
    KeyHelp::new("[ ]", "Back, forward"),
    KeyHelp::new("Esc", "Leave Settings"),
];

impl SettingsPage for AboutPage {
    fn rows(&self, cx: &Cx<'_>) -> Vec<Row> {
        let mut rows = vec![
            Row::pair("DSPi Terminal", String::new()),
            Row::note("USB Audio DSP Controller"),
            Row::note(format!("Version {}", env!("CARGO_PKG_VERSION"))),
            Row::note("Made with love by Weeb Labs"),
            Row::Blank,
        ];
        if cx.connected {
            rows.push(Row::section("Device"));
            let caps = &cx.state.caps;
            rows.push(Row::pair("Platform", caps.platform.name()));
            rows.push(Row::pair("Firmware", caps.firmware.clone()));
            rows.push(Row::pair("Wire format", format!("V{}", caps.wire_format)));
            rows.push(Row::pair("Serial", caps.serial.clone()));
            rows.push(Row::Blank);
        }
        rows.push(Row::section("Links & Support"));
        rows.push(Row::note(SUPPORT));
        for (name, url) in LINKS {
            rows.push(Row::pair(name, url));
        }
        rows
    }

    fn act(&mut self, _index: usize, _action: Action, _cx: &Cx<'_>) -> PageEvent {
        PageEvent::Handled
    }

    fn cursor(&self) -> usize {
        0
    }

    fn set_cursor(&mut self, _i: usize) {}

    fn keys(&self) -> &'static [KeyHelp] {
        KEYS
    }
}

#[cfg(test)]
mod tests {
    use super::super::Page;
    use super::super::tests::{frame, screen};
    use super::*;

    #[test]
    fn about_names_the_app_the_version_and_every_link() {
        let (mut s, st) = screen(Page::About);
        let f = frame(&mut s, &st, 120, 40);
        assert!(f.contains("DSPi Terminal"), "{f}");
        assert!(f.contains("USB Audio DSP Controller"), "{f}");
        assert!(
            f.contains(&format!("Version {}", env!("CARGO_PKG_VERSION"))),
            "{f}"
        );
        assert!(f.contains("Made with love by Weeb Labs"), "{f}");
        assert!(f.contains("LINKS & SUPPORT"), "{f}");
        for (name, _) in LINKS {
            assert!(f.contains(name), "missing {name}\n{f}");
        }
        assert!(f.contains("https://ko-fi.com/weeblabs"), "{f}");
    }

    #[test]
    fn about_fits_the_minimum_frame_and_scrolls() {
        let (mut s, st) = screen(Page::About);
        let f = frame(&mut s, &st, 80, 24);
        assert_eq!(f.lines().count(), 24);
        assert!(f.contains("DSPi Terminal"), "{f}");
    }
}
