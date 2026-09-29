# Phase B5 brief: the Subharmonic Synthesizer panel

Read `docs/plan/briefs/common-b.md` first. Phases B1 (protocol, registry
rows `sub.*`), B2 (`DeviceState` subharm section, solo and headroom
reads, meters) and B3 (the `Tool` variant for `S`) are merged. Read `git
log` for their commits.

Specification: `docs/plan/survey-firmware-beta4.md` sections 1.3, 2
(`WireSubharmParams`) and 3.1; `docs/plan/survey-console-beta4.md` section
1.1; the Console's `SubharmonicSynthView.swift` for every string, range,
step, end label, caption, tooltip and preset value; `ToolParameters.swift:
174-207` for defaults.

## Scope

A tool panel `screens/subharm.rs` on the pattern of `screens/psybass.rs`,
opened with `S`, titled "Subharmonic Synthesizer", shown only when the
device reports the subharm feature.

1. **Header**: the master switch and a **Solo** toggle (disabled unless
   the module is enabled; drawn in the warning colour while on). Solo is
   runtime-only and not notified: poll 0x2D while the panel is open, and
   send solo off when the panel closes if it was on, as the Console does.
2. **Graph**: the three derived bands and the boost bell, as a
   `PanelGraph` in the style of the psybass spectrum. Derive shapes from
   the firmware's band edges and filters (`subharm.h`, `subharm.c`), not
   from a guess; if the response cannot be derived with confidence, draw
   the bands as level blocks over their frequency ranges and say so in
   the report. "Apply preset" with the Console's four presets and their
   values (the top band goes to the floor).
3. **HEADROOM COST**: "+x.x dB" in the warning colour, or "none"; re-read
   0x1A after every write that can change it.
4. **LEVELS**: 24 - 36 Hz, 36 - 56 Hz, 56 - 80 Hz, each -30..+12 dB, step
   0.5, 1 decimal, -30 shown as "Off", with captions "Derived from 48 - 72
   Hz" and so on, and the Console's end labels.
5. **SELECTIVITY**: All material / Percussive / Sustained; Depth (0..100
   %, step 5) and Hold (50..400 ms, step 10) hidden in All material.
6. **SUB CEILING**: Threshold -40..0 dBFS, step 1, 0 shown as "Off".
7. **LF BOOST**: "70 Hz bell", 0..6 dB, step 0.5.
8. **OUTPUTS**: a chip per output, presets "Sub only" (the PDM output's
   bit, derived from `DeviceState`), "All outputs", "None"; a per-output
   sub meter (0x1F) polled while the panel is open and the module is
   enabled, at most 10 times a second; "Link output pairs" with its
   caption "One sub per pair, from its mono sum."
9. **Page command bar**: words for enable, solo, the three levels
   (`low`, `high`, `top`), `boost`, `ceiling`, `select`, `depth`, `hold`,
   `link`, with live hints, following `psybass.rs` or whichever panel
   already implements a command grammar.
10. **Quick strip and palette**: add the panel to Ctrl-P search. Do not
    add it to the sidebar quick strip (the Console has no sidebar icon
    for it).

## Exit

The common checks pass; golden frames (disabled, enabled with All
material, enabled with Percussive, solo on) at both sizes; tests for the
presets, the Off rendering at -30 and 0, the hidden Depth and Hold, solo
off on close, and the Sub only preset on RP2350 and RP2040 fixtures.
