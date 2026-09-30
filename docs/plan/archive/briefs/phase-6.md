# Phase 6 brief: DSP tool panels and Test Signals

You are implementing **Phase 6** of the Console-parity plan, in a git
worktree. Read, in order: `docs/plan/PLAN.md` (Phase 6, and section 5),
`docs/plan/DESIGN.md` sections 3, 5, 6, 7.8, 7.9 and 8,
`docs/plan/survey-console.md` sections 2.14 to 2.19 (every control, range,
caption and preset of the five DSP windows and Test Signals, verbatim),
`docs/plan/survey-firmware.md` sections 1.3, 1.5, 1.6, 1.14, 1.16, 1.26 and
6.4, the shell and screens code (`crates/dspi-tui/src/{shell,screens,
live.rs,widgets,graph.rs}`), `crates/dspi-session/src/state.rs` (the
`Crossfeed`, `Global`, `Leveller`, `Psybass`, `Upmix` views and the
`siggen_state` sub-state), `crates/dspi-proto/src/packets.rs` (the
`SiggenConfig`, `SiggenStatus`, `SiggenCapsHeader`, `SiggenTypeDesc`,
`UpmixStatus` codecs from Phase 2B) and `crates/dspi-proto/src/registry.rs`
(`cf.*`, `loud.*`, `lev.*`, `bass.*`, `up.*`, `sig.*` paths). The Console for
behaviour, read-only: `/Users/weeblabs/DSPi Console/DSPi Console/
{CrossfeedView,LoudnessView,VolumeLevellerView,PsychoacousticBassView,
UpmixerView,TestSignalsView}.swift` (the ISO 226 table and the curve maths
in `LoudnessView.swift:6-107`, the crossfeed curve in `CrossfeedView.swift`,
the psybass spectrum in `PsychoacousticBassView.swift`).

## File ownership

You own ONLY: `crates/dspi-tui/src/screens/{crossfeed,loudness,leveller,
psybass,upmixer,signals}.rs` (new), a shared `screens/panel.rs` for the
tool-panel template (header toggle, scrolling section column, unsupported
banner), their `pub mod` lines, the matching `Tool::*` arms of the
`ConsoleScreens` factory in `live.rs`, `examples/gallery.rs`
(`--screen crossfeed|loudness|leveller|psybass|upmixer|signals`), and a
new `crates/dspi-tui/src/curves.rs` for the three small graphs' maths
(crossfeed response, ISO 226 compensation at -40 dB, psybass spectrum),
ported from the Console with tests against values you compute by hand from
its formulas.

## What to build

The template (DESIGN 7.8) and then each panel exactly as survey 2.14 to
2.19 lists them: header title and subtitle, master toggle first in focus
order, the panel's graph (8 rows, `Disabled` placeholder when off), the
chip rows with their `Presets ▾` menus (`p`) and captions, the radio /
segmented pickers, every `ParamRow` with its range, step, unit, decimals,
caption and end labels, the unsupported banner when `state.caps.features`
lacks the feature (`psychoacoustic_bass`, `upmixer`, `test_signals`) with
the Console's text, the upmixer's live gauges from `REQ_UPMIX_GET_STATUS`
(read through `ScreenEvent::Command("get ...")` is wrong for a 1 Hz
refresh; instead add the status to `DeviceState` only if Phase 2B put a
codec there, else show the config-derived state and report the gap).

Test Signals (DESIGN 7.9): the 4x4 tile grid built from `state.caps.siggen`
and the type descriptors, OUTPUTS chips with the three-state polarity,
LEVEL, the per-type PARAMETERS, TIMING by model, OPTIONS, and the transport
row with `Space` bound to Start / Stop, `Stop now` on `S`, start blockers
shown as the reason text, and the running state from
`state.siggen_state`. Writes go through `sig.config` (key=value form from
Phase 2B) and `sig.control`.

## Tests

Golden frames for every panel at 80x24 and 120x40; key tests for every
key in each `keys()`; a curve test per graph; a test that each panel's
unsupported banner appears when the feature is absent.

## Rules and report

As the Phase 4 brief. Commit per panel with `Phase 6:` subjects. Report
files, kit gaps, behaviours not reproduced, gallery commands, test counts.
