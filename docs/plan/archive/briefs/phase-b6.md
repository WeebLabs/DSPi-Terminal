# Phase B6 brief: the Tube Modeller panel

Read `docs/plan/archive/briefs/common-b.md` first. Phases B1 (protocol, registry
rows `tube.*` with f32 payloads on 0x3E / 0x3F), B2 (`DeviceState` tube
section) and B3 (the `Tool` variant for `D`) are merged. Read `git log`
for their commits.

Specification: `docs/plan/survey-firmware-beta4.md` sections 1.4, 2
(`WireTubeParams`) and 3.3; `docs/plan/survey-console-beta4.md` section
1.2; the Console's `TubeModellerView.swift` for every string, range, step,
end label, caption and preset, and `Constants.swift:267-410` for the tube
and rectifier rows.

## Scope

A tool panel `screens/tube.rs` on the pattern of `screens/psybass.rs`,
opened with `D`, titled "Tube Modeller", shown only when the device
reports the tube feature. One layout (no Basic / Advanced switch): the
Advanced rows, in the Console's section order.

1. **Header**: master switch; subtitle as the Console.
2. **STAGE**: Tube menu grouped "Preamp triodes" (1-8), "Preamp pentodes"
   (9-10), "Power stages" (11-16), plus Custom (0), with the row's style
   as caption and, for push-pull rows, "Meant for use with the output
   stage on."; Drive (-30..+24 dB, step 0.5, default -12); Mix (0..100 %,
   end labels "Dry" / "All tube"); Output Trim (-12..+12 dB).
3. **CHARACTER**: Bias (-100..+100 %), Asymmetry (-12..+12 dB), Knee
   Hardness (0..100 %), Sag (0..100 %, dimmed when the rectifier is Solid
   state), Rectifier (Solid state / GZ34 / 5U4 / 5Y3) with its summary
   line ("Sag depth x0.6, 5 ms attack, 120 ms release." and so on, from
   the rectifier table).
4. **OUTPUT STAGE**: toggle with caption "A valve amplifier's loose grip
   on the speaker."; Damping Factor (1..20, step 0.5, end labels "1
   (loose)" / "20 (tight)"), Speaker Resonance (30..150 Hz), and the
   readout "+x.x dB at resonance, +x.x dB at the top." (derive the
   formula from the Console source and the firmware's `tube.c`). Rows
   hidden while the stage is off.
5. **OUTPUTS**: chips per output, presets "All outputs", "Exclude sub"
   (clear the PDM output's bit, derived from `DeviceState`), "None".
6. **Type rows**: choosing a type loads bias, asymmetry, hardness and sag
   on the device; editing one of those sets the type to Custom. Do not
   copy these rules into the Terminal: write, then rely on the re-read.
   The type and rectifier tables (names, styles, and the rectifier
   summary values) live only in `tube.c` (49-66) and the Console; put them
   in one table with a comment citing both, and a test that the table's
   length matches `TUBE_TYPE_MAX` and `TUBE_RECT_MAX` from the vendored
   `tube.h`.
7. **Transfer curve and harmonics**: include the TRANSFER CURVE graph and
   the "AT FULL SCALE" 2nd / 3rd harmonic readout only if the transfer
   function can be derived from the firmware source (`tube.c`) with
   confidence, as `PLAN.md` requires for all curves. Also include the
   Console's five curve presets (Clean default, Warm hi-fi, Single-ended
   sweetness, Guitar-amp style, Push-pull power; each also sets mix 100
   and trim 0). If the curve cannot be derived exactly, leave the graph
   out, keep the presets menu, and explain in the report.
8. **Page command bar**: words for enable, `type`, `drive`, `mix`,
   `trim`, `bias`, `asym`, `hardness`, `sag`, `rect`, `stage`, `damping`,
   `resonance`, with live hints.
9. **Palette**: add the panel to Ctrl-P search; not to the quick strip.

## Exit

The common checks pass; golden frames (disabled; enabled with 12AX7;
Custom; output stage off; solid-state rectifier) at both sizes; tests for
the type menu grouping, the table lengths, the dimmed Sag, the hidden
output-stage rows and the Exclude sub preset on both fixtures.
