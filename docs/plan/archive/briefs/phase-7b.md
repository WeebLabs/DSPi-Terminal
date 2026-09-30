# Phase 7B brief: Settings, Control Surfaces, Channel Groups, Macros

You are implementing **Phase 7B** of the Console-parity plan, in a git
worktree. This is the most intricate screen in the Console and the phase
the plan's owner audits in depth; precision over speed. Read, in order:
`docs/plan/PLAN.md` (Phase 7B, section 5), `docs/plan/DESIGN.md` 3, 5, 6,
7.13, `docs/plan/survey-console.md` sections 2.34, 2.35, 2.36 (every
control, name table and warning, verbatim) and 5, `docs/plan/survey-
firmware.md` section 3 in full (the caps-driven model, every struct, the
link laws, the apply model) and 7, `docs/firmware-notes.md` sections 11-16,
`crates/dspi-proto/src/packets.rs` and `crates/dspi-session/src/{surfaces,
pins,state}.rs` (the codecs, the write path with `wait_applied`, the pin
map, the `ir_learn` sub-state), the Settings shell from Phase 7A
(`crates/dspi-tui/src/settings/mod.rs`), the widget kit. The Console for
behaviour, read-only: `/Users/weeblabs/DSPi Console/DSPi Console/
DSPi_ConsoleApp.swift` lines 2094-6701 (the noun, action and type name
tables at the `+6185`, `+6253`, `+6323`, `+6586`, `+6595` offsets the survey
cites are the strings you must use).

## File ownership

You own ONLY `crates/dspi-tui/src/settings/{surfaces,groups,macros,
cs_model.rs}` (new; `cs_model.rs` holds the per-slot draft, the caps-driven
option lists and the name tables), their `pub mod` lines and the three
page arms in `settings/mod.rs`, `examples/gallery.rs` additions, and, if a
session helper is missing, `crates/dspi-session/src/surfaces.rs` additions
only (never edits to existing functions).

## What to build

Exactly survey 2.34 to 2.36: the placeholders and empty states; `Add
Control` as a type picker; one collapsible card per slot with type badge,
type change, name, status pill (from `slot_status[n]` through
`explain_status`), remove; the noun picker grouped by category with the
Console's names, filtered to nouns whose caps say `actions != 0` and
whose target kind the platform has; the action picker filtered by
`type.actions & noun.actions`; the event picker for buttons; the target
picker sectioned Channels / Groups (groups from the groups page's state);
the band / index picker; pin rows with the type's pin count and class
(pots ADC only, display SDA even / SCL odd) marking claimed pins; operand
rows by noun kind and unit (8.8 fixed point for dB / Q / percent / ms, log
steps for Hz and Q); brightness ceiling for LED_PWM; on / off delays with
the Console's warning; every flag toggle with the per-type Invert title
and detail; `Apply` (encode `CsBinding` and `write_binding`, then
`wait_applied`, showing PENDING / BUSY and the final status) and `Revert`;
the IR sub-section with learn (arm, `Waiting for a button... Cancel`, the
result from `state.ir_learn`, timeout) and per-command rows; the Display
sub-section with model, I2C pair, address, mode, home page, alignment,
seconds rows, brightness, flags with the edit-gated warning, and the pages
list with Large value and Level bar (bar rejected on bool / enum nouns
before the write); Channel Groups with the kind picker, member grid,
usage note and validation; Macros with ordered steps (move, delete), Add
Step, per-step fields, delay, the footer text; the CS-only save bar
subtitle; `Control-surface capability version N.`

## Tests

Golden frames per page and per card type at 120x40; encode tests that a
filled card produces the byte-exact `CsBinding` / `IrCommand` / `CsGroup` /
`CsMacroStep` / `CsDisplayCfg` / `CsDisplayPage` (mirror the Console's
`ControlSurfacesWireTests.swift` cases); a mock-transport test of Apply
seeing PENDING then OK; a test that unavailable nouns and disallowed
actions never appear; a test that a pot cannot pick a non-ADC pin.

## Rules and report

As the Phase 4 brief. Commit per page and per sub-section with `Phase 7B:`
subjects. Report files, every place the header and the Console disagreed
and which you followed, kit gaps, behaviours not reproduced, gallery
commands, test counts.
