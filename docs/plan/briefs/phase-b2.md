# Phase B2 brief: device state, presets and notifications

Read `docs/plan/briefs/common-b.md` first. Phase B1 (protocol bump to V32)
is merged: the bulk image has `subharm`, `tube` and `limiter` sections,
the registry has `sub.*`, `tube.*` and `limit.*` rows, and the probes
report the new features. Read `git log` for B1's commits to see what
exists.

## Scope

All in `crates/dspi-session` unless stated.

1. **`DeviceState` sections** (`state.rs`): typed structs, decoders and
   accessors for subharm (36 bytes at 5944), tube (48 at 5980) and the
   per-output limiter records (9 x 12 at 6028; records past the device's
   output count are ignored). Follow the `Psybass` / `Upmix` pattern.
   `apply()` must patch these sections from PARAM_CHANGED as it does for
   the others.
2. **Runtime state outside the bulk image:** subharm solo (0x2D; the
   firmware sends no notification, so the panel must poll it: provide a
   read function the TUI can call), subharm headroom (0x1A), and the
   subharm and limiter meters. Also the CS aux state and levels (0x05 with
   wValue 0xFFFF), updated from `NOTIFY_EVT_CS_AUX`.
3. **Unsaved-changes diff** (`PresetSnapshot::diff`): categories Subharm
   and Tube with one line per field, in the Console's wording (see
   `DSPi Console/DSPi Console/PresetSnapshot.swift:261-380`, for example
   "Subharm 24-36 Hz: off -> 0 dB" with the Console's arrow character if
   it is not an em-dash, and "Tube type: A -> B"). Per-output limiter lines
   ("<name> limiter threshold: ... dBFS") appear only when
   `output_config_mode` is WITH_PRESET. Solo and headroom are excluded.
4. **Output configuration in INDEPENDENT mode:** limiter edits mark the
   output configuration unsaved (find how the Terminal tracks the "output
   configuration" dirty category today; `PLAN.md` phase 7A mentions the
   save bar's three dirty categories), Revert restores them, and Save
   Output Configuration (0x52) persists them. In WITH_PRESET mode they
   dirty the preset.
5. **`.dspipreset` files** (`preset_file.rs`): optional top-level
   `subharm` and `tube` blocks and a `limiter` block inside each output
   channel entry, with exactly the Console's key names and value encodings
   (`DSPi Console/DSPi Console/PresetDocument.swift:45-47, 239-310, 379,
   408-435`). Limiters are applied on import only when the Hardware I/O
   option is chosen; its label becomes the Console's "Hardware I/O (GPIO
   pins, clocks, ADAT, inputs, output limiters)". Use the Console's skip
   reasons ("Output limiter (not supported by this firmware)",
   "Subharmonic synthesizer (not supported by this firmware)", "Tube
   preamp (not supported by this firmware)"; check
   `PresetDocumentTransfer.swift:435, 614, 618, 643` for the exact text).
   Write round-trip tests, and a test that parses a document written in
   the Console's format (construct it by hand from the Swift encoder if no
   sample file exists).
6. **Resync on unknown offsets:** a PARAM_CHANGED whose offset the
   Terminal cannot patch in place must trigger a re-read rather than be
   dropped. Check the current behaviour in `state.rs` `apply()` and fix it
   if needed, with a test.
7. **Tube and limiter write behaviour:** after a tube type write or a
   limiter write on a linked output, other values change on the device.
   The Terminal re-reads after every write (DESIGN section 11), so confirm
   that the re-read covers these sections and add a mock-transport test
   that a type change's four derived values arrive in `DeviceState`.

## Out of scope

Screens and panels (B4 to B8). Control Surfaces UI (B7).

## Exit

The common checks pass; new tests for each decoder, the diff lines in
both output-config modes, preset round trips, the Console-format parse,
the resync, and the CS_AUX state update.
