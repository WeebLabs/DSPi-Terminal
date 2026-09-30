# Phase B7 brief: Control Surfaces caps v14 to v20 and Auxiliary Outputs

Read `docs/plan/archive/briefs/common-b.md` first. Phase B1 (protocol bump to V32)
is merged: the noun table in `packets.rs` has nouns 57 to 78, and the
codecs for `CsBinding.extras`, `CS_UNIT_MS_LOG`, `CS_TARGET_AUX`, the aux
component types, status 0x26 and the 48-byte aux state block exist. Read
`git log` for B1's commits.

Also read `crates/dspi-proto/firmware/control_surfaces.h` for everything
about aux containers, and `docs/plan/survey-firmware-beta4.md` section
3.7.

## Scope

1. **Nouns 57 to 78 in the UI** (`settings/cs_model.rs`): labels,
   categories and strips from the Console (`DSPi_ConsoleApp.swift` around
   6170-6199 for categories, 7156-7183 for names): "Subharmonic Synth"
   (strip "Subharm"), "Tube Modeller", "Auxiliary Outputs" (strip "Aux").
   Limiter nouns 74 to 78 are not in the Console; put them in a category
   "Output Limiter" with names "Limiter", "Limiter Threshold", "Limiter
   Release", "Limiter Link", "Limiter Gain Reduction" (B1 may already have
   placeholders; replace them). Limiter Release uses `CS_UNIT_MS_LOG`
   (plain ms). Limiter Gain Reduction is read-only: offer it only where a
   read-only noun makes sense (displays), following how the firmware
   marks it. Nouns whose `actions` are 0 in caps stay unavailable, as
   today.
2. **Target kind 5 (aux)**: the target picker lists only aux slots (only
   dimmable, type 10, for Aux Level), labelled "Auxiliary Output" /
   "Auxiliary Outputs" instead of "Channel(s)"; target names are the slot
   name or "Aux N". Aux nouns reject GROUP targets; a binding cannot
   target its own slot.
3. **Settings > Control > Auxiliary Outputs**, a new page shown when caps
   are at least 18 and the type table contains types 9 and 10 (Console
   `DSPViewModel.swift:1640`; page at `DSPi_ConsoleApp.swift:3114-3350`,
   dispatched around 4438). Reproduce the Console's page: the empty state
   ("No Auxiliary Outputs Set Up" and its sentence), "Add Output" (On/Off
   Output or Dimmable Output; "All N control slots are in use." when
   full), and per card: Output (live switch through 0x04, disabled until
   applied with the Console's caption), Level (dimmable only, 0..100 %,
   0x06, 8.8 LE clamped to 25600), GPIO pin (through the existing pin
   picker and ownership map), Active-Low Output (`CS_FLAG_INVERT`), Level
   Limit (dimmable, 1..100 %, `base_bright`), Linear Response (dimmable,
   `CS_AUX_X_LINEAR`), turn-on and turn-off delays, At Power-On (Fixed /
   As Last Saved via `BOOT_SAVED`; Fixed shows Starts On via `BOOT_ON`
   and, for dimmable, Starting Level in `value` as 8.8), and Driven By
   (the controls, remote keys and macros that target this slot, with the
   Console's empty text). Apply / Revert per slot as the Control Surfaces
   page does. Use the existing slot write path in
   `dspi-session/src/surfaces.rs`.
4. **Exclude aux types** (9, 10) from the Control Surfaces page's add
   menu and card list.
5. **Aux SETs are immediate** (0x04, 0x06): they are not deferred previews
   and never make the configuration dirty. Read status 0x87 after each
   one and show a failure with the Console's text for status 0x26.
6. **Live updates**: `NOTIFY_EVT_CS_AUX` updates the switch and level on
   the page. If phase B2 has not merged when you start, read the state
   with 0x05 wValue 0xFFFF when the page opens and on each Settings poll
   (the page already polls once a second while Settings is open), and
   leave a note in your report so the notification path can be joined up
   after B2 merges.
7. **Other v14 to v20 details**: the PWM conflict message now reads
   "...another dimmable LED or output" (check the Console for the full
   text); `CsBinding.extras` must be written as 0 on non-aux types; the
   display's CYCLE_ALL mode skips aux nouns (no UI change unless the
   Terminal lists cycled nouns).

8. **Preserve `extras` on every edit path.** Phase B1 noted that an aux
   slot edited through the existing Control Surfaces code may be
   re-encoded with `extras = 0`, which would clear its boot flags. Check
   every path that decodes, edits and re-encodes a `CsBinding` (including
   Revert, copy and the command line) and make sure `extras` survives,
   with a test.

## Exit

The common checks pass; golden frames for the Auxiliary Outputs page
(empty, one on/off output, one dimmable output) at both sizes; tests for
the aux target picker, the extras bits, the MS_LOG encoding in the noun
editor, and the status 0x26 message.
