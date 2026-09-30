# Phase B10 brief: documentation and carried-over defects

Read `docs/plan/briefs/common-b.md` first. Phases B1 to B8 and B11 are
merged. Phase B9 (refinements: graph magnitude, Graphing settings, Stats,
dashboard, write rate) is running in parallel; avoid `curves.rs`,
`screens/stats.rs`, `screens/overview.rs` and `settings/graphing` code.

## Part 1: documentation

1. **`docs/plan/DESIGN.md` section 11 (accepted deviations).** Add one
   entry for each decision below, in the section's existing style (what
   the Console does, what the Terminal does, why):
   - Limiter dirtiness in INDEPENDENT mode is compared, not flagged, so an
     edit put back by hand reads as saved (B2).
   - Limiter settings are an expanded section on the output page, not a
     popover; the latency caption is the Terminal's own sentence (B4).
   - Subharm solo is polled once a second while the panel is open; the
     Console reads it once (B5).
   - The subharm band graph draws the firmware's filter shapes, not the
     Console's flat blocks (B5).
   - The Tube Modeller shows one layout (the Console's Advanced rows) with
     the graph first; Mix keeps its Basic-mode end labels (B6).
   - The spectrum analyser's strength maps to normal or dim drawing; peak
     hold is a contour or a bar cap; the overlay on the response graph
     and the RTA strip are not built (B8, decision 3).
   - Aux live switches and levels move on the device's echo, not before;
     aux SETs read status 0x87 where the Console fires and forgets;
     aux nouns are hidden until an aux output exists (B7).
   - Cards on every Control Surfaces card page follow the device whenever
     they have no staged edit (B7).
   - The Console's strips apply to every noun category (B7).
   - `S`, `D` and `A` are global; Bypass All and Clear All moved to the
     `;` bar with the Console's dialogs; the matrix's `D` Clear is gone
     and `;clear` runs without a dialog as the Console's button does; the
     Interrupt Monitor's Clear moved to Backspace (B3).
   - Old RP2350 Terminal preset files are recognised and read, but the
     Consoles misplace them (B11).
   Update section 3's key table and section 13's command-bar rows (B3
   added `bypass` and `clear`; B4 already updated the output row).
2. **`docs/plan/audit.md`.** Annotate every defect with its resolution:
   fixed (commit), accepted (DESIGN section 11 entry) or open. Use
   `git log --grep` and the commit subjects. Then move `audit.md` and
   `briefs/` to `docs/plan/archive/`, fixing any links to them.
3. **`docs/plan/PLAN.md`.** Mark it superseded for firmware targets by
   `PLAN-beta4.md` (one line at the top), fix exit condition 1's firmware
   pin, remove the stale `Targets::expand` criterion, and remove the
   duplicate D41. Add the section 5 coverage rows from `PLAN-beta4.md` to
   its section 4 matrix, ticking the ones now merged (W1-W3, T1-T4, O1,
   C6, C7, R1-R3) and leaving T5, R4, R5 open.
4. **`docs/plan/PLAN-beta4.md`.** Add a Progress section in the style of
   PLAN.md's, listing what merged, what is open, and the hardware
   checkpoints still to run.
5. **`README.md`.** Replace "nothing is transcribed by hand" with an
   accurate statement (generated from headers where the generator can
   lift them; hand tables checked against the headers by tests), and
   "refuses to build" with "fails its tests". Add the new tools to the
   key table and the interface description, and the Spectrum Analyser and
   Auxiliary Outputs Settings pages.
6. **Stale code comments:** the `ConsoleScreens` doc comment
   (`crates/dspi-tui/src/live.rs`, around line 108), and remove
   `PlaceholderScreens` if only a test uses it (port the test to
   `ConsoleScreens` or a minimal local stand-in).
7. **Em-dashes:** grep code, strings and docs (excluding the vendored
   firmware headers, which are verbatim copies) and remove any.

## Part 2: open defects from the first audit

For each, fix it or add a DESIGN section 11 entry explaining why it stays:

- D36: the `ADAT_STATE` notification is only shown in the monitor log.
  Let it update the Stats ADAT section if Stats is open, if cheap.
- D40: Firmware Update is always offered. Firmware update is out of scope
  (decision 4), so check what the Terminal's existing bootloader handoff
  offers and make sure it is labelled accurately; record the decision.
- D57: the conditional "Reset to Built-in" button makes button indices
  fragile. Replace index matching with named actions.
- D60: AutoEQ result rows show the form factor as text; accept.
- D69, D70: Control Surfaces label wording differs from the Console
  (`surfaces.rs`, `groups.rs`, `cs_model.rs` action names Next/Increase
  vs Up/Down). Adopt the Console's wording where it has one.
- D71: the macro Run/Stop button: check the Console; keep it if the
  Console has an equivalent, otherwise record it as an addition.
- D72: the EDIT_GATED warning condition: check against the Console and
  the firmware, and fix or record.
- D73: `used_by` counts bindings only; include IR commands, macros and
  display pages if the Console does.
- The golden-frame tests are `contains` assertions rather than stored
  frames: record this as an accepted deviation with the reason.

## Exit

The common checks pass; every item above done or recorded; no em-dashes
outside the vendored headers.
