//! The channel index space, and the only place allowed to convert between the
//! several index spaces the protocol uses.
//!
//! The firmware's V16 "unified channel model" made inputs first-class channels
//! with their own PEQ and metering, and removed the old master channels. The
//! index space is `[inputs 0..num_inputs-1][outputs num_inputs..num_channels-1]`,
//! so **the first output channel is `num_inputs`, which is 2 on RP2040 and 8 on
//! RP2350**. It is not a constant.
//!
//! Older documentation states that output index maps to `channel = output + 2`.
//! That was true before V16 and is now wrong on RP2350. Encoding it anywhere but
//! here is how an app acquires a silent, platform-specific bug, so no other
//! module performs this arithmetic. See REDESIGN_SPEC.md 3.2 mechanism 1.

/// Which index space a value lives in. These are not interchangeable, and mixing
/// them up is the most common integration bug against this protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Space {
    /// `0 .. num_channels`, used by EQ, per-channel delay, names, peak meters.
    Channel,
    /// `0 .. num_inputs`, used by preamp and matrix rows.
    Input,
    /// `0 .. num_outputs`, used by the matrix mixer and per-output commands.
    Output,
    /// `0 .. num_pin_outputs`, per physical output instance, not per channel.
    PinOutput,
}

/// The device's channel topology, built once at connect from the bulk header.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ChannelMap {
    num_inputs: u8,
    num_outputs: u8,
    names: Vec<String>,
}

impl ChannelMap {
    /// Build from the counts the device reported in its bulk-params header.
    ///
    /// Returns `None` if the counts are inconsistent, which would mean we
    /// misparsed the header and must not proceed to interpret offsets.
    pub fn new(num_inputs: u8, num_outputs: u8, num_channels: u8) -> Option<Self> {
        if num_inputs as u16 + num_outputs as u16 != num_channels as u16 {
            return None;
        }
        if num_channels == 0 {
            return None;
        }
        Some(Self {
            num_inputs,
            num_outputs,
            names: Vec::new(),
        })
    }

    /// Attach the device-supplied channel names, so the UI can show the user's
    /// own labels rather than our guesses.
    pub fn with_names(mut self, names: Vec<String>) -> Self {
        self.names = names;
        self
    }

    pub fn num_inputs(&self) -> u8 {
        self.num_inputs
    }

    pub fn num_outputs(&self) -> u8 {
        self.num_outputs
    }

    pub fn num_channels(&self) -> u8 {
        self.num_inputs + self.num_outputs
    }

    /// Channel indices of the input channels.
    pub fn inputs(&self) -> std::ops::Range<u8> {
        0..self.num_inputs
    }

    /// Channel indices of the output channels.
    pub fn outputs(&self) -> std::ops::Range<u8> {
        self.num_inputs..self.num_channels()
    }

    pub fn is_output(&self, ch: u8) -> bool {
        ch >= self.num_inputs && ch < self.num_channels()
    }

    pub fn is_input(&self, ch: u8) -> bool {
        ch < self.num_inputs
    }

    /// Channel index -> output index, or `None` if this channel is an input.
    pub fn output_index(&self, ch: u8) -> Option<u8> {
        self.is_output(ch).then(|| ch - self.num_inputs)
    }

    /// Output index -> channel index, or `None` if out of range.
    pub fn channel_of_output(&self, out: u8) -> Option<u8> {
        (out < self.num_outputs).then(|| out + self.num_inputs)
    }

    /// Input index -> channel index. Inputs lead the space, so this is identity,
    /// but callers should still go through it: if a future wire version reorders
    /// the space, this is the one place that changes.
    pub fn channel_of_input(&self, input: u8) -> Option<u8> {
        (input < self.num_inputs).then_some(input)
    }

    pub fn is_valid(&self, space: Space, index: u8) -> bool {
        match space {
            Space::Channel => index < self.num_channels(),
            Space::Input => index < self.num_inputs,
            Space::Output => index < self.num_outputs,
            // Pin outputs are a separate count reported by the pins section.
            Space::PinOutput => index < self.num_outputs,
        }
    }

    /// The device's own name for a channel, falling back to a generated label.
    pub fn label(&self, ch: u8) -> String {
        if let Some(name) = self.names.get(ch as usize)
            && !name.is_empty()
        {
            return name.clone();
        }
        match self.output_index(ch) {
            Some(out) => format!("Out {}", out + 1),
            None => format!("In {}", ch + 1),
        }
    }

    /// A stable, lowercase, shell-safe address for a channel, used by the
    /// command grammar (`eq in.1 ...`). Device names are slugified so a user's
    /// own label works as a command argument.
    pub fn slug(&self, ch: u8) -> String {
        if let Some(name) = self.names.get(ch as usize)
            && !name.is_empty()
        {
            let slug: String = name
                .trim()
                .to_lowercase()
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '.' })
                .collect();
            let slug = slug.trim_matches('.').to_string();
            // Collapse runs produced by punctuation, e.g. "S/PDIF 1 L" -> "s.pdif.1.l".
            let collapsed = slug
                .split('.')
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(".");
            if !collapsed.is_empty() {
                return collapsed;
            }
        }
        match self.output_index(ch) {
            Some(out) => format!("out.{}", out + 1),
            None => format!("in.{}", ch + 1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rp2350() -> ChannelMap {
        ChannelMap::new(8, 9, 17).unwrap()
    }

    fn rp2040() -> ChannelMap {
        ChannelMap::new(2, 5, 7).unwrap()
    }

    #[test]
    fn rejects_inconsistent_counts() {
        assert!(ChannelMap::new(8, 9, 11).is_none());
        assert!(ChannelMap::new(0, 0, 0).is_none());
    }

    /// The regression this whole module exists to prevent. Pre-V16 code assumed
    /// `channel = output + 2` on every platform.
    #[test]
    fn output_mapping_is_platform_dependent() {
        assert_eq!(rp2040().channel_of_output(0), Some(2));
        assert_eq!(rp2350().channel_of_output(0), Some(8));
        assert_ne!(
            rp2350().channel_of_output(0),
            Some(2),
            "the +2 mapping from pre-V16 docs is wrong on RP2350"
        );
    }

    #[test]
    fn round_trips_both_directions() {
        for map in [rp2040(), rp2350()] {
            for out in 0..map.num_outputs() {
                let ch = map.channel_of_output(out).unwrap();
                assert_eq!(map.output_index(ch), Some(out));
                assert!(map.is_output(ch));
                assert!(!map.is_input(ch));
            }
            for inp in 0..map.num_inputs() {
                let ch = map.channel_of_input(inp).unwrap();
                assert!(map.is_input(ch));
                assert_eq!(map.output_index(ch), None);
            }
        }
    }

    #[test]
    fn ranges_partition_the_space() {
        let map = rp2350();
        let all: Vec<u8> = map.inputs().chain(map.outputs()).collect();
        assert_eq!(all, (0..17).collect::<Vec<u8>>());
    }

    #[test]
    fn out_of_range_is_rejected_not_wrapped() {
        let map = rp2040();
        assert_eq!(map.channel_of_output(5), None);
        assert_eq!(map.output_index(7), None);
        assert!(!map.is_output(7));
    }

    #[test]
    fn device_names_win_over_generated_labels() {
        let map = rp2350().with_names(
            (0..17)
                .map(|i| if i == 0 { "Turntable".into() } else { String::new() })
                .collect(),
        );
        assert_eq!(map.label(0), "Turntable");
        assert_eq!(map.slug(0), "turntable");
        // Empty names fall back rather than rendering blank.
        assert_eq!(map.label(1), "In 2");
    }

    #[test]
    fn slugs_are_shell_safe() {
        let mut names = vec![String::new(); 17];
        names[8] = "S/PDIF 1 L".into();
        let map = rp2350().with_names(names);
        assert_eq!(map.slug(8), "s.pdif.1.l");
    }
}
