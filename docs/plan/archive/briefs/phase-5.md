# Phase 5 brief: the Matrix Mixer

You are implementing **Phase 5: Matrix mixer** of the Console-parity plan, in
a git worktree. Read, in order: `docs/plan/PLAN.md` (Phase 5, and section 5),
`docs/plan/DESIGN.md` sections 3, 5, 6, 7.7 and 8, `docs/plan/survey-console.md`
section 2.13 (every control and string of the Console's Matrix Mixer), the
shell and screens code (`crates/dspi-tui/src/{shell,screens,live.rs,widgets}`)
and `crates/dspi-session/src/{state.rs,write.rs}` (the `Crosspoint` and
`Output` views, `Session::enable_output` and `EnableOutcome` from Phase 2C).
The Console for behaviour, read-only: `/Users/weeblabs/DSPi Console/DSPi
Console/MatrixMixerView.swift`.

## File ownership

You own ONLY `crates/dspi-tui/src/screens/matrix.rs` (new), its `pub mod`
line in `screens/mod.rs`, the `Tool::Matrix` arm of the `ConsoleScreens`
factory in `live.rs`, and `examples/gallery.rs` (add `--screen matrix`).
Nothing else.

## What to build

The panel from DESIGN 7.7, opened with `M`, filling the pane:

- Column headers in the output colour: the output name over its `OUTn`
  descriptor; only outputs visible for the platform. A horizontal scroll
  with the input label column pinned when columns exceed the width; at the
  Wide density every RP2350 column fits.
- The `ROUTING` band with, in 8-channel mode, `Direct 1:1` (`d`: route each
  input to the matching output and disable the PDM sub, exactly as the
  Console's `applyDirectRouting`) and `Clear` (`D`, confirm, disconnect
  every crosspoint).
- Input rows: the input name in its colour, and in 8-channel mode the
  input trim (`pre <input>`) as an editable value; a divider between stereo
  pairs.
- Crosspoint cells: `●`/`○` connect state and the gain; `INV` in the
  warning colour when inverted; cells of a disabled output dim; a cell that
  would collide with Core 1 carries a `!` in the warning colour.
- Output rows `ENABLE` (`⏻`, accent when on, dim when off, warning when it
  would conflict; `Space` toggles through `Session::enable_output`, raising
  the Console's two confirm dialogs verbatim when `NeedsConfirm` and then
  `enable_output_confirmed`), `GAIN`, `DELAY` (42 ms on RP2040, 85 ms
  otherwise), `MUTE` (danger reverse when muted).
- Keys: arrows move the reticle; `Space` connects / disconnects a
  crosspoint; `Enter` arms the gain for typing; `←`/`→` on an armed gain
  nudge 0.5 dB; `i` inverts; `r` renames the column's output; `y`/`Y`
  copy / paste the output's parameters through the Phase 4 clipboard; `I`
  identify (emit `ScreenEvent::Status("Identify arrives with Phase 6")`
  unless Phase 6 has landed).
- Writes are `mix <in> <out> ...` and `out.* <o>` commands through
  `ScreenEvent::Command`, except output enable, which needs the session's
  interlock: emit a new `ScreenEvent::EnableOutput(index, bool)` only if
  `shell/screen.rs` already has it; otherwise emit `Command("out.enable ..")`
  and note in your report that the reviewer should route it through
  `enable_output`.

## Tests

Golden frames at 80x24, 120x40 and 200x60 through the shell with
`shell::fixture::state`; key tests for every key in `keys()`; a test that a
disabled output's cells draw dim; a test that Direct 1:1 emits the right
commands for an 8-input device and nothing on a 2-input one.

## Rules and report

As the Phase 4 brief: Console strings verbatim, no em-dashes, nothing about
device shape compiled in, fmt / clippy / test / build --all-targets green,
one commit per bullet group with `Phase 5:` subjects, no trailers, no push.
Report: files, gaps in the kit, behaviours not reproduced, gallery command,
test counts.
