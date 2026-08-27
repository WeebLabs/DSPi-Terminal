//! The channel clipboard: the Console's `copyChannelParams` /
//! `pasteChannelParams`, reached from `y` and `Y` in the sidebar.
//!
//! A copy takes a snapshot of what makes a channel sound the way it does: its
//! PEQ bank, and for an output its crossover bank and its gain, delay and
//! mute. A paste writes that onto another channel as ordinary commands, so the
//! echo line shows exactly what was done and the same lines work in a script.

use dspi_proto::value::EqParamPacket;
use dspi_session::DeviceState;

use super::{band_command, channel_name, cleared_band, number, supports_crossover};

/// What a copy captured. The source name is kept so the paste can say where it
/// came from.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelClipboard {
    pub source: String,
    pub bands: Vec<EqParamPacket>,
    /// Outputs only: the crossover bank travels between outputs and is left
    /// alone when the clipboard came from an input.
    pub crossover: Option<Vec<EqParamPacket>>,
    /// Inputs only.
    pub preamp_db: Option<f32>,
    /// Outputs only: gain, delay, mute.
    pub output: Option<(f32, f32, bool)>,
}

/// Take a channel's parameters.
pub fn copy(state: &DeviceState, channel: usize) -> ChannelClipboard {
    let ni = state.caps.num_inputs as usize;
    let is_output = channel >= ni;
    ChannelClipboard {
        source: channel_name(state, channel),
        bands: state.bands(channel as u8),
        crossover: is_output.then(|| state.xover_bands(channel as u8)),
        preamp_db: (!is_output).then(|| state.preamp_db(channel)),
        output: is_output.then(|| {
            let o = state.output(channel - ni);
            (o.gain_db, o.delay_ms, o.mute)
        }),
    }
}

/// The commands that put a clipboard onto a channel, and onto its linked
/// partner when it has one.
pub fn paste_commands(
    clip: &ChannelClipboard,
    state: &DeviceState,
    channel: usize,
    mirror: Option<usize>,
) -> Vec<String> {
    let ni = state.caps.num_inputs as usize;
    let mut out = Vec::new();

    for target in std::iter::once(channel).chain(mirror) {
        for band in 0..state.caps.max_bands {
            // Bands the clipboard does not reach are cleared, so the
            // destination is the source rather than a merge of the two.
            let p = clip
                .bands
                .get(band as usize)
                .copied()
                .unwrap_or_else(|| cleared_band(target as u8, band));
            out.extend(band_command(state, target, band, &p));
        }
        if let Some(db) = clip.preamp_db
            && target < ni
        {
            out.push(format!("pre {} {}", target + 1, number(db)));
        }
    }

    if channel >= ni {
        let o = channel - ni;
        if let Some(xo) = &clip.crossover
            && supports_crossover(state)
        {
            for (i, p) in xo.iter().enumerate().take(4) {
                out.extend(band_command(state, channel, 20 + i as u8, p));
            }
        }
        if let Some((gain, delay, mute)) = clip.output {
            out.push(format!("out.gain {} {}", o + 1, number(gain)));
            out.push(format!("out.delay {} {}", o + 1, number(delay)));
            out.push(format!(
                "out.mute {} {}",
                o + 1,
                if mute { "on" } else { "off" }
            ));
        }
    }
    out
}

/// What the echo line says after a copy.
pub fn copied_message(clip: &ChannelClipboard) -> String {
    format!("Copied {} parameters", clip.source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::fixture;

    #[test]
    fn a_copy_takes_the_bank_and_what_the_channel_kind_carries() {
        let s = fixture::state();
        let input = copy(&s, 0);
        assert_eq!(input.source, "FL");
        assert_eq!(input.bands.len(), 10);
        assert!(input.crossover.is_none(), "inputs have no crossover");
        assert_eq!(input.preamp_db, Some(0.0));
        assert!(input.output.is_none());

        let output = copy(&s, 16);
        assert_eq!(output.source, "Sub");
        assert_eq!(output.crossover.as_ref().unwrap().len(), 4);
        assert_eq!(output.output, Some((-3.0, 0.0, false)));
        assert!(output.preamp_db.is_none());
    }

    #[test]
    fn a_paste_writes_every_band_and_the_channels_own_settings() {
        let s = fixture::state();
        let clip = copy(&s, 0);
        let cmds = paste_commands(&clip, &s, 1, None);
        assert_eq!(cmds[0], "eq in.2 1 lowshelf 105 0.707 8.8");
        assert_eq!(cmds[2], "eq in.2 3 peak 2856 3.58 -8.6");
        // Every band travels, including the ones the source leaves unset, so
        // the destination is the source rather than a merge of the two.
        assert_eq!(cmds[9], "eq in.2 10 flat 0 0 0");
        assert_eq!(cmds[10], "pre 2 0");
        assert_eq!(cmds.len(), 11);
    }

    #[test]
    fn a_paste_onto_an_output_carries_the_crossover_and_the_strip() {
        let s = fixture::state();
        let clip = copy(&s, 8);
        let cmds = paste_commands(&clip, &s, 16, None);
        assert!(
            cmds.iter().any(|c| c == "eq out.9 20 highpass 80 0.707 0"),
            "{cmds:?}"
        );
        assert!(cmds.iter().any(|c| c == "out.gain 9 0"), "{cmds:?}");
        assert!(cmds.iter().any(|c| c == "out.delay 9 0"), "{cmds:?}");
        assert!(cmds.iter().any(|c| c == "out.mute 9 off"), "{cmds:?}");
    }

    #[test]
    fn a_paste_mirrors_onto_a_linked_partner() {
        let s = fixture::state();
        let clip = copy(&s, 0);
        let cmds = paste_commands(&clip, &s, 0, Some(1));
        assert!(cmds.iter().any(|c| c.starts_with("eq in.1 1 ")), "{cmds:?}");
        assert!(cmds.iter().any(|c| c.starts_with("eq in.2 1 ")), "{cmds:?}");
        assert_eq!(copied_message(&clip), "Copied FL parameters");
    }
}
