# Phase B3 brief: small parity edits to existing screens

Read `docs/plan/archive/briefs/common-b.md` first. Phase B1 (protocol bump to V32)
is merged; `git log` shows its commits.

## Scope

1. **Rename Test Signals to Signal Generator** everywhere it is user
   visible: the tool title (`shell/mod.rs` `Tool::title`), the panel
   header, palette entries, help text, the quick strip and the Control
   Surfaces noun label (`settings/cs_model.rs`). The Console's panel
   header is "Signal Generator" with subtitle "Onboard test and
   measurement signals" (`TestSignalsView.swift:14-31`). Keep the key `G`.
   Internal identifiers (`Tool::Signals`, `signals.rs`) may stay.
2. **PEQ pass labels**: in the UI, PEQ low-pass and high-pass bands read
   **HC / LC / HC1 / LC1** (high cut = low pass, low cut = high pass).
   Crossover bands keep LP / HP. Filter files keep LP / HP / LP1 / HP1
   (`dspi-session/src/filterfile.rs` must not change). Find every place
   the band list, overview cards and band dialogs show a type code or
   name, and check the Console (`DSPMath.swift` around 168-180 and
   285-297, `DashboardView.swift:377`) for the long names too (for
   example whether "Low Pass" became "High Cut" in the type picker).
3. **Control Surfaces filter-type values**: "Low Pass" / "High Pass"
   become "High Cut" / "Low Cut" (`DSPi_ConsoleApp.swift:7426-7428`).
4. **Matrix input rows** use the sidebar channel names
   (`screens::channel_name`), truncated to fit. Upmix-derived rows keep C
   / Ls / Rs. Check the Console's `matrixRowFullName`
   (`DSPViewModel.swift:2067-2097`) for the exact rule.
5. **Output page routing names** come from the sidebar channel names,
   not 7.1 labels (Console `Components.swift:950`).
6. **Firmware version display**: use B1's beta-aware version ("1.1.6 beta
   4") in the title bar, Settings > About and Stats.
7. **No-device state**: with no device, the sidebar shows no channel rows
   and the graph shows its grid with no curves or legend. Losing the
   device returns the selection to the overview. Check what the Terminal
   does today; change only what differs.
8. **Tool keys** (decision 2): add `S`, `D` and `A` to the global tool
   keys, as the Console's mnemonics for Subharmonic Synthesizer, Tube
   Modeller and Spectrum Analyser. They must work from every screen, so
   the band list's `A` (Bypass All) and `D` (Clear All)
   (`screens/filters.rs:499,518`) and Test Signals' `S`
   (`screens/signals.rs:977`) must give them up. Bypass All and Clear All
   stay reachable from the footer buttons and the `;` command bar (check
   both exist and add the command-bar words if missing); Signal Generator
   keeps Space for Start and Stop. Update help text, key hints and the
   README key table.
   The three tools do not exist yet (phases B5, B6, B8 add them). Add the
   three `Tool` variants with their Console titles and keys, and have the
   factory in `live.rs` show them only when the device reports the
   feature; until their panels land, opening one should show the
   existing "unsupported" presentation used by other panels, so no stub
   screen is written. If that is not possible without a stub, stop and
   report instead of inventing one.
9. **Grey ghosts for bypassed bands** on the response graph: draw a
   bypassed band's own contribution dimmed in grey rather than hiding it,
   following the Console (e5f15d5), within the quiet design's
   one-curve-per-graph rule. If the Terminal does not draw per-band
   contributions at all, skip this item and say so.

## Exit

The common checks pass; golden frames updated where labels changed; tests
for the key bindings (`S`, `D`, `A` open tools from the band list and
from Signal Generator), the HC/LC labels and the matrix row names.
