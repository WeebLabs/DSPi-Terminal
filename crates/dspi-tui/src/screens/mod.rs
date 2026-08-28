//! The Console's core screens: the dashboard overview, the input page and the
//! output page, plus the filter list they share.
//!
//! Every screen here draws from `&DeviceState` each frame and keeps only its
//! own cursor and edit state, so a change made anywhere (a knob, a remote,
//! another host) is on screen the next tick without the screen being told. A
//! write is never issued from a screen: it is asked for as a
//! [`ScreenEvent::Command`](crate::shell::ScreenEvent::Command) in the shared
//! command grammar, which is also what the echo line shows, so anything the
//! interface can do a person can type.
//!
//! What the device does not store lives in [`SharedState`]: which input pairs
//! are linked (the Console's `linkedInputPairs`), the preset names read back
//! from the device, and the channel clipboard. The screen factory owns one and
//! hands every screen a handle to it.

pub mod clipboard;
pub mod crossfeed;
pub mod filters;
pub mod input;
pub mod linkwitz;
pub mod output;
pub mod overview;
pub mod panel;
pub mod presets;

use std::cell::RefCell;
use std::rc::Rc;

use dspi_proto::FilterType;
use dspi_proto::value::EqParamPacket;
use dspi_proto::xover;
use dspi_session::DeviceState;

pub use clipboard::ChannelClipboard;
pub use crossfeed::CrossfeedPanel;
pub use filters::{FilterList, FilterMode};
pub use input::InputPage;
pub use linkwitz::LinkwitzPanel;
pub use output::OutputPage;
pub use overview::Overview;
pub use presets::{PresetChoice, PresetMenu};

/// The application-side state the screens share.
#[derive(Debug, Default, Clone)]
pub struct SharedState {
    /// Linked input pairs, by pair index (0 is inputs 1 and 2). The Console
    /// keeps this in the app, not on the device, and so do we.
    pub linked_pairs: [bool; 4],
    /// Preset slot names, read back with `preset.name <slot>` and refreshed
    /// after a rename. Empty means the slot has no name.
    pub preset_names: Vec<String>,
    /// Bit N set means slot N holds a preset, from `REQ_PRESET_GET_DIR`
    /// (the Console's `isPresetOccupied`).
    pub occupied: u16,
    /// The slot the device loads at power on, when the startup mode is
    /// "specified".
    pub default_slot: Option<u8>,
    pub clipboard: Option<ChannelClipboard>,
}

impl SharedState {
    /// Is this input's adjacent pair linked?
    pub fn input_linked(&self, input: usize) -> bool {
        self.linked_pairs.get(input / 2).copied().unwrap_or(false)
    }

    /// The channel an edit on `input` mirrors onto, if the pair is linked and
    /// the partner is live.
    pub fn linked_partner(&self, input: usize, num_inputs: usize) -> Option<usize> {
        let partner = input ^ 1;
        // A pair is only available while both of its channels are live, which
        // is the Console's `pairAvailable`.
        (self.input_linked(input) && input.max(partner) < num_inputs).then_some(partner)
    }

    pub fn set_linked(&mut self, pair: usize, on: bool) {
        if let Some(slot) = self.linked_pairs.get_mut(pair) {
            *slot = on;
        }
    }
}

/// A handle every screen the factory makes holds a clone of.
pub type Shared = Rc<RefCell<SharedState>>;

pub fn shared() -> Shared {
    Rc::new(RefCell::new(SharedState::default()))
}

// ---------------------------------------------------------------------------
// What the device can do
// ---------------------------------------------------------------------------

/// A firmware version string as a comparable triple; an unparseable version
/// reads as `(0, 0, 0)`, which is the Console's conservative default.
pub fn firmware_triple(version: &str) -> (u32, u32, u32) {
    let mut parts = version
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .map(|p| p.parse::<u32>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

/// Per-band bypass shipped in firmware 1.1.4 (`DSPViewModel.firmwareSupports
/// BandBypass`); older firmware stalls the opcode, so the disc is hidden.
pub fn supports_bypass(state: &DeviceState) -> bool {
    firmware_triple(&state.caps.firmware) >= (1, 1, 4)
}

/// Notch and all-pass shipped in the same release as bypass.
fn supports_notch(state: &DeviceState) -> bool {
    supports_bypass(state)
}

/// Crossovers are addressable from wire format V11 (bands 20 to 23).
pub fn supports_crossover(state: &DeviceState) -> bool {
    state.caps.wire_format >= 11
}

/// The Linkwitz Transform arrived at V22, and the Console offers it on outputs
/// only.
pub fn supports_linkwitz(state: &DeviceState) -> bool {
    state.caps.wire_format >= 22
}

/// The PEQ types this firmware will accept, in the Console's menu order.
///
/// `availableFilterTypes` in `ContentView.swift`: every gate is a firmware
/// version or a wire-format version, never a compiled-in list.
pub fn available_types(state: &DeviceState, include_linkwitz: bool) -> Vec<FilterType> {
    let wire = state.caps.wire_format;
    let mut out = vec![
        FilterType::Flat,
        FilterType::Peaking,
        FilterType::LowShelf,
        FilterType::HighShelf,
        FilterType::LowPass,
        FilterType::HighPass,
    ];
    if supports_notch(state) {
        out.push(FilterType::Notch);
        out.push(FilterType::AllPass);
    }
    if wire >= 13 {
        out.push(FilterType::AllPass1);
    }
    if wire >= 14 {
        out.push(FilterType::LowShelf1);
        out.push(FilterType::HighShelf1);
    }
    if wire >= 28 {
        out.push(FilterType::LowPass1);
        out.push(FilterType::HighPass1);
    }
    if include_linkwitz && supports_linkwitz(state) {
        out.push(FilterType::LinkwitzTransform);
    }
    out
}

/// The longest delay this platform accepts, in ms. `ChannelSettingsView`'s
/// `maxDelay`: 42 on RP2040, 85 elsewhere.
pub fn max_delay_ms(state: &DeviceState) -> f64 {
    match state.caps.platform {
        dspi_proto::Platform::Rp2040 => 42.0,
        _ => 85.0,
    }
}

// ---------------------------------------------------------------------------
// Names and numbers
// ---------------------------------------------------------------------------

/// The Console's full type name, `FilterType.name` in `DSPMath.swift`.
pub fn type_name(t: FilterType) -> String {
    match t {
        FilterType::Flat => "Off".into(),
        FilterType::Peaking => "Peaking".into(),
        FilterType::LowShelf => "Low Shelf (12dB)".into(),
        FilterType::HighShelf => "High Shelf (12dB)".into(),
        FilterType::LowPass => "High Cut (12dB)".into(),
        FilterType::HighPass => "Low Cut (12dB)".into(),
        FilterType::Notch => "Notch".into(),
        FilterType::AllPass => "All Pass (360°)".into(),
        FilterType::AllPass1 => "All Pass (180°)".into(),
        FilterType::LowShelf1 => "Low Shelf (6dB)".into(),
        FilterType::HighShelf1 => "High Shelf (6dB)".into(),
        FilterType::LowPass1 => "High Cut (6dB)".into(),
        FilterType::HighPass1 => "Low Cut (6dB)".into(),
        FilterType::LinkwitzTransform => "Linkwitz Transform".into(),
        other => match xover::meta(other.to_raw()) {
            Some(m) => format!(
                "{}{} {}",
                m.family.short().to_uppercase(),
                m.order,
                if m.high_pass { "High Pass" } else { "Low Pass" }
            ),
            None => other.label(),
        },
    }
}

/// The dashboard's compact code, `DashboardRow.typeCode`. The Console draws an
/// em-dash for an unset band; we draw `OFF`, which is what its own switch says.
pub fn type_code(t: FilterType) -> String {
    match t {
        FilterType::Flat => "OFF".into(),
        FilterType::Peaking => "PK".into(),
        FilterType::LowShelf => "LS".into(),
        FilterType::HighShelf => "HS".into(),
        FilterType::LowPass => "LP".into(),
        FilterType::HighPass => "HP".into(),
        FilterType::Notch => "NO".into(),
        FilterType::AllPass => "AP".into(),
        FilterType::AllPass1 => "AP1".into(),
        FilterType::LowShelf1 => "LS1".into(),
        FilterType::HighShelf1 => "HS1".into(),
        FilterType::LowPass1 => "LP1".into(),
        FilterType::HighPass1 => "HP1".into(),
        FilterType::LinkwitzTransform => "LT".into(),
        other => match xover::meta(other.to_raw()) {
            Some(m) => format!(
                "{}{}{}",
                m.family.short().to_uppercase(),
                m.order,
                if m.high_pass { "HP" } else { "LP" }
            ),
            None => format!("{}", other.to_raw()),
        },
    }
}

/// The token the command grammar takes for a filter type: the registry's name
/// for a PEQ type, the file code (`lr4lp`) for a crossover.
pub fn type_token(t: FilterType) -> String {
    if let Some(m) = xover::meta(t.to_raw()) {
        return format!(
            "{}{}{}",
            m.family.short(),
            m.order,
            if m.high_pass { "hp" } else { "lp" }
        );
    }
    match t {
        FilterType::Flat => "flat".into(),
        FilterType::Peaking => "peak".into(),
        FilterType::LowShelf => "lowshelf".into(),
        FilterType::HighShelf => "highshelf".into(),
        FilterType::LowPass => "lowpass".into(),
        FilterType::HighPass => "highpass".into(),
        FilterType::Notch => "notch".into(),
        FilterType::AllPass => "allpass".into(),
        FilterType::AllPass1 => "allpass1".into(),
        FilterType::LowShelf1 => "lowshelf1".into(),
        FilterType::HighShelf1 => "highshelf1".into(),
        FilterType::LinkwitzTransform => "linkwitz".into(),
        FilterType::LowPass1 => "lowpass1".into(),
        FilterType::HighPass1 => "highpass1".into(),
        other => other.to_raw().to_string(),
    }
}

/// How a command names a channel: the device's own slug, so a line copied off
/// the echo line works in a shell against this device.
pub fn channel_token(state: &DeviceState, channel: usize) -> String {
    if let Some(c) = state.caps.channels.get(channel)
        && !c.slug.is_empty()
    {
        return c.slug.clone();
    }
    let ni = state.caps.num_inputs as usize;
    if channel < ni {
        format!("in.{}", channel + 1)
    } else {
        format!("out.{}", channel - ni + 1)
    }
}

/// The channel a sidebar output index equalises. Outputs follow the inputs in
/// the unified channel space.
pub fn output_channel(state: &DeviceState, output: usize) -> usize {
    state.caps.num_inputs as usize + output
}

/// The Console's `formatTrimmed`: a fixed number of decimals with trailing
/// zeros stripped, but never down to a bare integer.
pub fn trimmed(value: f64, decimals: usize, signed: bool) -> String {
    let full = if signed {
        format!("{value:+.d$}", d = decimals)
    } else {
        format!("{value:.d$}", d = decimals)
    };
    let Some((head, tail)) = full.split_once('.') else {
        return full;
    };
    let tail = tail.trim_end_matches('0');
    if tail.is_empty() {
        format!("{head}.0")
    } else {
        format!("{head}.{tail}")
    }
}

/// A number as a command line would carry it: enough precision for every field
/// the device has, with no trailing noise.
pub fn number(v: f32) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" {
        "0".into()
    } else {
        s.to_string()
    }
}

/// How a command names a band: 1-based for the PEQ bank, and the firmware's
/// own 20 to 23 for the crossover bank.
pub fn display_band(band: u8) -> String {
    if (20..24).contains(&band) {
        band.to_string()
    } else {
        (band + 1).to_string()
    }
}

/// The commands that write one whole band.
///
/// The firmware stores a band as one 16-byte packet and the grammar's `eq`
/// verb sends exactly that, so a single field edit is still a whole-band
/// write. The packet's bypass byte is always sent clear by `eq`, so a bypassed
/// band needs the flag put back.
pub fn band_command(
    state: &DeviceState,
    channel: usize,
    band: u8,
    p: &EqParamPacket,
) -> Vec<String> {
    let ch = channel_token(state, channel);
    let n = display_band(band);
    let mut line = format!(
        "eq {ch} {n} {} {} {} {}",
        type_token(p.filter_type),
        number(p.freq),
        number(p.q),
        number(p.gain_db),
    );
    if let Some(qp) = p.qp.filter(|_| p.filter_type.is_linkwitz()) {
        line.push(' ');
        line.push_str(&number(qp));
    }
    let mut out = vec![line];
    if p.bypass && p.filter_type != FilterType::Flat && supports_bypass(state) {
        out.push(format!("eq.bypass {ch} {n} on"));
    }
    out
}

/// A band reset to its default (flat) state, as the Console's Clear does.
pub fn cleared_band(channel: u8, band: u8) -> EqParamPacket {
    EqParamPacket {
        channel,
        band,
        filter_type: FilterType::Flat,
        bypass: false,
        freq: 1000.0,
        q: 0.707,
        gain_db: 0.0,
        qp: None,
    }
}

/// The Console's name for a channel, falling back to its descriptor when the
/// device has none.
pub fn channel_name(state: &DeviceState, channel: usize) -> String {
    let name = state.channel_name(channel);
    if !name.is_empty() {
        return name;
    }
    crate::theme::ChannelRole::of(channel as u8, state.caps.num_inputs, state.caps.num_outputs)
        .descriptor(state.caps.num_outputs)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::shell::{Shell, fixture};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    /// Turn a key-line token into the key events it stands for, so a screen
    /// can prove that every key it advertises is bound. The shell has the same
    /// helper for its own regions.
    pub(crate) fn keys_for(token: &str) -> Vec<KeyEvent> {
        let plain = |c: KeyCode| KeyEvent::new(c, KeyModifiers::NONE);
        token
            .split_whitespace()
            .map(|k| match k {
                "↑" => plain(KeyCode::Up),
                "↓" => plain(KeyCode::Down),
                "←" => plain(KeyCode::Left),
                "→" => plain(KeyCode::Right),
                "Enter" => plain(KeyCode::Enter),
                "Space" => plain(KeyCode::Char(' ')),
                "Backspace" => plain(KeyCode::Backspace),
                "Esc" => plain(KeyCode::Esc),
                "PgUp" => plain(KeyCode::PageUp),
                "PgDn" => plain(KeyCode::PageDown),
                "1-9,0" => plain(KeyCode::Char('4')),
                other => {
                    let c = other.chars().next().unwrap();
                    assert_eq!(other.chars().count(), 1, "unknown key token {other}");
                    KeyEvent::new(
                        KeyCode::Char(c),
                        if c.is_ascii_uppercase() {
                            KeyModifiers::SHIFT
                        } else {
                            KeyModifiers::NONE
                        },
                    )
                }
            })
            .collect()
    }

    /// A whole shell with one of the Console's screens in its detail region,
    /// so the golden frames are what a person actually sees.
    fn shell(detail: Box<dyn crate::shell::Screen>, selection: crate::shell::Selection) -> Shell {
        let theme = crate::theme::Theme::console(
            crate::theme::ColorDepth::TrueColor,
            crate::theme::Glyphs::Braille,
        );
        let mut model = fixture::rp2350(&theme);
        model.selection = selection;
        let mut shell = Shell::new(model, theme, detail);
        shell.focus = crate::shell::Focus::Screen;
        shell
    }

    fn frame(shell: &mut Shell, w: u16, h: u16) -> String {
        let state = fixture::state();
        let mut term =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).expect("backend");
        term.draw(|f| shell.draw(f.area(), f.buffer_mut(), &state))
            .expect("draw");
        let buf = term.backend().buffer();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn screens_under_test() -> Vec<(
        &'static str,
        Box<dyn crate::shell::Screen>,
        crate::shell::Selection,
    )> {
        let state = fixture::state();
        vec![
            (
                "overview",
                Box::new(Overview::new(shared())) as Box<dyn crate::shell::Screen>,
                crate::shell::Selection::Overview,
            ),
            (
                "input",
                Box::new(InputPage::new(0, shared(), &state)),
                crate::shell::Selection::Input(0),
            ),
            (
                "output",
                Box::new(OutputPage::new(0, shared(), &state)),
                crate::shell::Selection::Output(0),
            ),
        ]
    }

    #[test]
    fn every_screen_fills_the_detail_region_at_both_sizes() {
        for (name, detail, selection) in screens_under_test() {
            let mut s = shell(detail, selection);
            for (w, h) in [(120u16, 40u16), (80, 24)] {
                let f = frame(&mut s, w, h);
                assert_eq!(f.lines().count(), h as usize, "{name} at {w}x{h}");
                assert!(f.contains("INPUTS"), "the sidebar is still there: {name}");
                assert!(
                    !f.contains("arrives in Phase"),
                    "{name} at {w}x{h} is still a placeholder:\n{f}"
                );
            }
        }
    }

    #[test]
    fn the_overview_draws_the_consoles_card_through_the_shell() {
        let state = fixture::state();
        let _ = &state;
        let mut s = shell(
            Box::new(Overview::new(shared())),
            crate::shell::Selection::Overview,
        );
        let f = frame(&mut s, 120, 40);
        assert!(f.contains("STEREO INPUT (USB)"), "{f}");
        assert!(f.contains("Delay: 0 ms"), "{f}");
        assert!(f.contains("Move between cards"), "the key line: {f}");
        let f = frame(&mut s, 80, 24);
        assert!(f.contains("STEREO INPUT (USB)"), "{f}");
    }

    #[test]
    fn the_input_page_draws_its_header_and_list_through_the_shell() {
        let state = fixture::state();
        let mut s = shell(
            Box::new(InputPage::new(0, shared(), &state)),
            crate::shell::Selection::Input(0),
        );
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            let f = frame(&mut s, w, h);
            assert!(f.contains("Link 1/2"), "{w}x{h}: {f}");
            assert!(f.contains("Preamp"), "{w}x{h}: {f}");
            assert!(f.contains("Clear PEQ"), "{w}x{h}: {f}");
            assert!(f.contains("TYPE"), "{w}x{h}: {f}");
            assert!(f.contains("● FL"), "the title row names the channel: {f}");
        }
        // The wider frame has room for the type names in full, and for the
        // whole key line.
        let f = frame(&mut s, 120, 40);
        assert!(f.contains("Low Shelf (12dB)"), "{f}");
        assert!(f.contains("105 Hz"), "{f}");
        assert!(f.contains("Enable All"), "the key line: {f}");
    }

    #[test]
    fn the_output_page_draws_routing_gain_delay_mute_and_the_tabs() {
        let state = fixture::state();
        let mut s = shell(
            Box::new(OutputPage::new(0, shared(), &state)),
            crate::shell::Selection::Output(0),
        );
        for (w, h) in [(120u16, 40u16), (80, 24)] {
            let f = frame(&mut s, w, h);
            assert!(f.contains("GAIN"), "{w}x{h}: {f}");
            assert!(f.contains("DELAY"), "{w}x{h}: {f}");
            assert!(f.contains("MUTE"), "{w}x{h}: {f}");
            assert!(f.contains("PEQ") && f.contains("XO"), "{w}x{h}: {f}");
            assert!(f.contains("INV"), "{w}x{h}: {f}");
        }
    }

    /// The shell hands the key to the focused screen before it looks at it
    /// itself, so a screen's own letters have to survive the trip.
    #[test]
    fn a_screens_keys_reach_it_through_the_shell() {
        let state = fixture::state();
        let mut s = shell(
            Box::new(InputPage::new(0, shared(), &state)),
            crate::shell::Selection::Input(0),
        );
        let events = s.handle(
            KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT),
            &state,
        );
        assert!(
            events.is_empty(),
            "D opened the Clear All dialog, not a tool"
        );
        assert!(s.dialog.is_some());
    }

    #[test]
    fn firmware_and_wire_gates_follow_the_console() {
        let s = fixture::state();
        assert_eq!(firmware_triple("1.1.6"), (1, 1, 6));
        assert_eq!(firmware_triple(""), (0, 0, 0));
        assert!(supports_bypass(&s));
        assert!(supports_crossover(&s));
        assert!(supports_linkwitz(&s));
        let types = available_types(&s, false);
        assert!(!types.contains(&FilterType::LinkwitzTransform));
        assert!(
            types.contains(&FilterType::LowPass1),
            "V28 first-order pass"
        );
        assert!(available_types(&s, true).contains(&FilterType::LinkwitzTransform));

        let mut old = fixture::state();
        old.caps.firmware = "1.1.3".into();
        old.caps.wire_format = 10;
        assert!(!supports_bypass(&old));
        assert!(!supports_crossover(&old));
        let types = available_types(&old, true);
        assert!(!types.contains(&FilterType::Notch));
        assert!(!types.contains(&FilterType::LowPass1));
        assert!(!types.contains(&FilterType::LinkwitzTransform));
    }

    #[test]
    fn names_codes_and_tokens_match_the_consoles_tables() {
        assert_eq!(type_name(FilterType::LowShelf), "Low Shelf (12dB)");
        assert_eq!(type_name(FilterType::from_raw(34)), "LR4 Low Pass");
        assert_eq!(type_code(FilterType::Notch), "NO");
        assert_eq!(type_code(FilterType::from_raw(34)), "LR4LP");
        assert_eq!(type_token(FilterType::Peaking), "peak");
        assert_eq!(type_token(FilterType::from_raw(35)), "lr4hp");
        assert_eq!(type_token(FilterType::from_raw(56)), "bes2lp");
    }

    #[test]
    fn trimmed_keeps_one_decimal_and_drops_the_rest() {
        assert_eq!(trimmed(-8.6, 2, true), "-8.6");
        assert_eq!(trimmed(8.0, 2, true), "+8.0");
        assert_eq!(trimmed(3.58, 3, false), "3.58");
        assert_eq!(trimmed(0.707, 3, false), "0.707");
        assert_eq!(trimmed(1.0, 3, false), "1.0");
    }

    #[test]
    fn channel_tokens_come_from_the_device() {
        let s = fixture::state();
        assert_eq!(channel_token(&s, 0), "in.1");
        assert_eq!(channel_token(&s, 8), "out.1");
        assert_eq!(output_channel(&s, 8), 16);
        assert_eq!(channel_name(&s, 0), "FL");
        assert_eq!(channel_name(&s, 16), "Sub");
    }

    #[test]
    fn a_linked_pair_names_its_partner_only_while_it_is_live() {
        let mut st = SharedState::default();
        assert_eq!(st.linked_partner(0, 8), None);
        st.set_linked(0, true);
        assert_eq!(st.linked_partner(0, 8), Some(1));
        assert_eq!(st.linked_partner(1, 8), Some(0));
        assert_eq!(st.linked_partner(1, 1), None, "partner is not live");
    }
}
