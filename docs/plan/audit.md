# Phase 10 audit

*Opus, 2026-08-30, on `console-parity` (worktree at `c10230b`, phases 0
through 9 merged). This is the audit called for by `PLAN.md` section 2 phase
10: every exit condition, every coverage-matrix row, every phase exit
criterion and every working agreement, checked against `survey-console.md`,
`survey-firmware.md`, `DESIGN.md` and the vendored headers. Nothing was
fixed. HW-1, HW-2 and HW-3 have not been run, so anything only a device can
prove is recorded as `needs HW` rather than as a pass or a fail.*

Method: the code was read; `examples/gallery.rs` was run at 80x24, 120x40
and 200x60 for every screen, every Settings page and several expanded
cards; the Console survey was compared control by control, string by
string; and, for Settings, the Console source itself was read at
`/Users/weeblabs/DSPi Console` @ `75793c3`.

## 1. Verification numbers

| Command | Result |
|---|---|
| `cargo fmt --all --check` | clean, exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean, exit 0 |
| `cargo build --workspace --all-targets` | clean, exit 0 |
| `cargo test --workspace` | **911 passed, 0 failed, 0 ignored** |
| `cargo test --workspace -- --ignored` | not run: no test is `#[ignore]`d anywhere |

Per binary: `dspi-cmd` 64, `dspi-proto` 181 + `tests/coverage.rs` 5,
`dspi-session` 181, `dspi` 11, `dspi-transport` 10, `dspi-tui` 459.
No doc-tests.

`cargo test -p dspi-proto --test coverage -- --nocapture` reports
`registry: 135 rows referencing 202/202 opcodes (100%), 0 excluded`.

`dspi --version` prints
`dspi 2.0.0-dev (protocol from DSPi release/v1.1.6 @ 112f35b)` /
`wire format V28, 202 vendor opcodes`.

## 2. Verdict per exit condition (PLAN section 0)

**1. Firmware parity: pass, less the device check.** `PROVENANCE.toml`
pins `release/v1.1.6` @ `112f35b2db9cda4151ec8063dd3babe8fcfe9ea4`, all six
headers. Wire V28, 202/202 opcodes, an empty exclusion list. `dspi dump` on
a v1.1.6 device is **needs HW**.

**2. Feature parity: partial.** 40 of 55 matrix rows pass. 13 are partial,
1 fails (S8), 1 is a matrix row the plan never ticked (Q1, which passes).
See section 5.

**3. Layout parity: pass with cosmetic deviations.** The shell is the
Console's shape and the Console's mnemonics. Three specified pieces are
missing: the graph carries no `Filter Response` title or `⤢` affordance
outside the pop-out (`shell/mod.rs:912-916`), the echo line has no
right-hand contextual action strip (`shell/mod.rs:1033-1049`), and the
graph and detail share one box where `DESIGN.md` 2.1 draws two. None of
these were recorded in `DESIGN.md` section 11.

**4. Live parity: partial, and the worst area of the branch.** Every V28
event decodes and lands, but a lost packet never triggers the re-read that
is the whole point of the mechanism (D1), a device-side preset load leaves
a permanent false dirty marker (D2), a disconnect is un-observable (D5),
and the notification reader is never re-armed on a device switch and keeps
a claimed interface alive (D6). See section 8.1.

**5. Quality: partial.** fmt, clippy and tests are green; every screen and
every Settings page renders at 80x24 and 120x40 under test; every key
advertised in a `keys()` table is proven to do something. Two caveats: the
"golden frames" are `contains` assertions over a rendered frame, not stored
golden files, so a layout regression that keeps the asserted substrings
passes silently; and the hardware checkpoints have not been run.

**6. Audit: this document is the first half.** The three deep audits are
seeded in section 8.

## 3. Per-phase exit criteria (PLAN section 2)

Only phases 1, 2A, 2B, 2C and 3 carry an explicit `Exit:` line. Phases 4
through 9 are judged against their descriptions and against the matrix.

### Phase 1 (firmware bump): pass, less HW

- `cargo test --workspace` green. Yes.
- `tests/coverage.rs` 202/202, empty exclusion list. Yes.
- A test pins the V28 `input_config` offsets. Yes:
  `wire::tests::the_v28_input_config_offsets_are_where_the_header_puts_them`,
  plus `every_input_config_field_decodes_from_its_own_byte` and
  `the_v28_shift_would_be_invisible_to_a_size_check` (`wire.rs:633,715`).
- `dspi --version` prints V28 and 202. Yes.
- `dspi dump` on the device. **needs HW.**

### Phase 2A (device state and notifications): partial

- Mock-transport tests for **every** event type: **partial.**
  `notify::tests::discrete_events_decode_by_id` covers all nine ids but is a
  pure `decode()` test. Only two ids reach a `MockTransport`
  (`notify::tests::the_reader_delivers_events_and_flags_a_loss`).
  `state::tests::discrete_events_land_in_their_sub_state` applies only
  SIGGEN and IR_LEARN; ADAT_STATE, ADAT_INPUT_STATE and I2S_SLAVE_STATE
  have no apply test.
- Seq-gap recovery: **fail.** Detection is tested three ways;
  `DeviceState::stale` is set (`state.rs:676,689,694`) and then read by
  nothing outside its own tests. See D1.
- Input-config shift: **pass** (wire level).
- Snapshot diff categories: **partial.** One test
  (`state::tests::the_snapshot_ignores_runtime_fields_and_describes_real_changes`)
  covers Global / Matrix / Filters / Names and the "and N more" cap.
  Loudness, Crossfeed, Delays, Outputs, Hardware, Leveller, Psybass and
  Upmix are untested.
- `dspi watch` one-shot: **pass** (`main.rs:760-870`, plain and `--json`).
- HW: `user_volume` with source `UAC1`. **needs HW.**
- Two structural departures from the phase text, neither recorded in
  `DESIGN.md` 11: the trait is `Transport::notifications() ->
  Option<Box<dyn NotificationSource>>`, not `bulk_in(endpoint, len,
  timeout)`; and the "single typed model every screen reads from" is
  actually four models (`DeviceState`, `Capabilities`, `SettingsData`,
  `SharedState`), the last two living in the TUI crate. See section 8.1.

### Phase 2B (codecs and CS write path): pass

Every codec named in the phase has a size and a field-offset test in
`crates/dspi-proto/src/packets.rs`: `cs_binding_field_offsets_…`,
`ir_command_…`, `cs_group_…`, `macro_step_…`,
`the_macro_header_payload_is_name_then_count`, `display_cfg_…`,
`display_page_…`, `display_status_…`, `ext_status_…`, `cs_status_…`,
`caps_header_locates_its_maxima_past_the_type_table`,
`the_status_tables_are_sized_by_the_caps_header`, `noun_desc_…`,
`type_desc_…`, `matrix_route_…`, `upmix_config_…`, `upmix_status_…`,
`siggen_config_…`, `siggen_status_…`, `siggen_caps_header_…`,
`siggen_type_desc_…`, `dac_hw_mute_…`, `uart_config_…`, `i2c_config_…`,
`ctrl_iface_status_…`, `buffer_stats_…`, `spdif_rx_status_…`,
`adat_input_status_…`, `adat_output_status_…`, `i2s_slave_status_…`,
`lg_sound_sync_status_…`, `preset_directory_offsets_match_the_firmware`,
`preset_startup_is_asymmetric`. `explain_status` covers 0x00-0x04 and
0x10-0x25 (missing 0x05, D22). Writing a binding and seeing PENDING then OK
is tested (`settings::surfaces::tests::an_apply_waits_out_pending_and_reports_the_final_status`).
HW-1's write-and-read-back and IR arm: **needs HW.**

### Phase 2C (session fixes): partial

| Item | Verdict |
|---|---|
| `cmd_tui` loads matrix and surfaces at connect | pass by restructuring: the matrix now comes from the bulk shadow (`write::matrix_tests::the_matrix_comes_from_the_bulk_snapshot_not_per_crosspoint_reads`) and `cmd_tui`/`cmd_screenshot` build `DeviceState` identically. Surfaces load on Settings open (`live.rs:1443`), not at connect. |
| `Targets::expand` for every target kind | **not applicable / renamed.** No `Targets` type exists in the workspace. Expansion is `ChannelMap` plus `dspi_cmd::Context`. The plan owner should retire or restate this criterion. |
| Trigger rows fire with a confirm for `Hazard::Flash` / `Irreversible` | **partial.** Each Console-visible destructive action has a bespoke confirm, but there is no generic hazard gate: `:dev.reset` typed on the `:` line (`Hz::Irreversible`, `registry.rs:2083`) goes straight through `Live::set`. See D20. |
| `:undo` / `:redo` over the journal | pass (`undo.rs`, 8 tests; live params enforced at `write.rs:478`) |
| `core1_conflict` before an output enable, both Console dialogs | pass (`write.rs:1230-1335`, 9 `write::core1_tests::*`) |
| Preset apply of hardware I/O in the Console's order with the "skipped" report | pass (`preset_file::io_tests::*`) |
| Batching: `dspi -f script` and stdin | pass (`main.rs:55,1342,1434`; four tests) |
| The live app shows matrix and surfaces on the mock | pass |

### Phase 3 (design system): partial

- `theme.rs` with `console`, `amber`, `mono`, every table in section 4 as
  data, and a distinctness test: **pass.** `theme.rs:245-278` reproduces all
  17 channel triples and all 8 semantic rows exactly; `theme.rs:1094-1102`
  asserts 17 distinct 256 indices and `:1135-1141` that accent and warning
  avoid every channel. (A fourth palette, `dark`, exists beyond the three
  `DESIGN.md` names.)
- Every widget in section 6 with `handle()` and golden tests at truecolor,
  256, 16 and mono: **partial.** Every widget exists and has tests, but
  almost every widget test constructs `Theme::console(ColorDepth::TrueColor,
  …)`; only `legend.rs`, `chips.rs` and `meter.rs` branch on depth at all,
  and no widget has a per-depth golden.
- The shell, focus model, `Esc` stack, help overlay, golden tests at 80x24,
  120x40, 200x60: **pass.** Four densities exist (`Compact` /
  `Normal` / `Roomy` / `Wide`, `shell/layout.rs:13-58`), which is more than
  the checklist asked for and matches `DESIGN.md` section 8.
- Graph: legend pills, grouping, dashed unselected, cursor readout, phase,
  height cycling, pop-out: **pass**, except the pop-out's independent
  visibility map (D14).
- `examples/gallery.rs` renders every widget **in every colour depth**, and
  `dspi screenshot` uses the same fixture so screenshots need no device:
  **fail on both halves.** The gallery renders the shell and its screens
  only, never a widget catalogue, and it hard-codes `ColorDepth::TrueColor`
  unless `--ansi` is passed, in which case it detects rather than accepts a
  depth (`gallery.rs:69-73`). `dspi screenshot` calls `connect(serial)`
  (`main.rs:669`) and refuses without a device. See D15, D16.
- A test that every key named on the key line of every screen is handled:
  **pass** (`shell::tests::every_advertised_key_is_handled`,
  `settings::tests::every_advertised_key_is_handled_on_every_page`, and one
  per screen).

### Phases 4, 5, 6, 7A, 7B, 8, 9

No explicit exit lines; judged through the matrix in section 5. In summary:
4 partial (S3-S6, S8, I2, I3), 5 partial (M1), 6 partial (X1, X5, T1), 7A
partial (P1, P8, P9, P10), 7B partial (C1, C2, C3), 8 partial (A2, F2, K1,
K2), 9 pass.

## 4. `PLAN.md` is stale

`PLAN.md:286-289` still reads "Merged: Phases 0, 1, 2A, 2B, 2C, 3, 4, 5, 6,
7A and 8. Running: Phase 7B; Phase 9 under way." 7B and 9 are merged. The
matrix's `Done` column is still blank for C1-C5 and Q1. Cosmetic, but it is
the document the audit is measured against.

## 5. Coverage matrix

| # | Console | Verdict | Reason if not `pass` |
|---|---|---|---|
| S1 | Sidebar INPUTS rows | pass | |
| S2 | Sidebar OUTPUTS + identify + muted dimming | pass | the ID tone itself is needs HW |
| S3 | Quick-access strip (8 actions) | partial | `Space` is a no-op on the Matrix, Stats and Settings cells (D17) |
| S4 | Preset row and its six context actions | partial | no CRC path / "Preset data is corrupted."; Set as Default never disabled (D18, D19) |
| S5 | Source row | partial | `Enter` opens no list; only `←`/`→` cycle (D12) |
| S6 | Volume, both tapers, mode menu, reset | partial | the mode choice is read from the config file and never written back (D31) |
| S7 | CPU C0/C1 red > 90 % | pass | |
| S8 | Connection dot, "No Devices", picker, reconnect | **fail** | the dot is permanently green (D5); no force-reconnect (D11) |
| D1 | Overview cards | pass | |
| I1 | Input page: link, preamp, clear PEQ | pass | |
| I2 | Filter list | partial | no Q on 2nd-order shelves (D3); no footer action strip (D8); gain re-formatted at 1 dp on every nudge (D21) |
| I3 | Linkwitz popover | partial | Qp is read-only, so the fourth parameter is unreachable (D4) |
| O1 | Output page | pass | |
| O2 | PEQ / XO tabs, crossover rows | pass | |
| O3 | Enable All / Bypass All / Clear All | pass | |
| G1 | Graph | pass | |
| G2 | Legend, grouping, dashed unselected | pass | |
| G3 | Graph pop-out with independent visibility | partial | the pop-out shares the shell's visibility map; `popout_follows_selection` is dead (D14) |
| M1 | Matrix crosspoints, INV, trim, Direct 1:1, Clear | partial | Identify is a stub (D23); the conflict marker is one-sided; column names not in the output colour |
| M2 | Matrix ENABLE / GAIN / DELAY / MUTE, conflicts | pass | |
| X1 | Crossfeed | partial | PARAMETERS never dim outside Custom (D24); caption drawn above the chips |
| X2 | Loudness | pass | |
| X3 | Leveller | pass | |
| X4 | Psybass with starting points | pass | |
| X5 | Upmixer with live gauges | partial | "No device connected" status missing (D9); three gauges hidden when their engine is off |
| T1 | Test Signals | partial | the "No device connected" start blocker is missing (D10); output chips never dim though the caption promises it (D25) |
| N1 | Stats for Nerbs | pass | no Core 1 mode row; live values needs HW |
| N2 | Interrupt Monitor | pass | |
| A1 | AutoEQ browser | pass | |
| A2 | Favourites menu, Update Database | partial | no Clear Favourites; "Reset to Built-in" conditional; success string not verbatim |
| F1 | Import / Export Filters | pass | |
| F2 | Import / Export Device Configuration | partial | the progress dialog is never shown (D26) |
| F3 | Save Master Volume, Save Output Configuration | pass | effect needs HW |
| K1 | Commit, Revert, Factory Reset, Firmware Update | partial | no STM32 hiding, no Option-skip; the RPI-RP2 handoff needs HW |
| K2 | Unsaved-changes prompt | partial | not raised on copy-to, which then overwrites the saved source slot (D2) |
| K3 | Channel clipboard | pass | |
| P1 | Settings shell | partial | `Ctrl-S` in Settings saves a **preset**, not the page (D6) |
| P2 | Overview pin map | pass | |
| P3 | About | pass | |
| P4 | Graphing (persisted) | pass | |
| P5 | Advanced | pass | |
| P6 | Global Parameters | pass | |
| P7 | Outputs | pass | status row is optimistic; device outcome needs HW |
| P8 | Inputs | partial | Instances and Channels lists hardcoded rather than device-served (D27) |
| P9 | I2S Configuration | partial | Slave BCK Pin is not editable in Split mode (D7) |
| P10 | Control Interfaces | partial | four of the five status strings are unreachable (D13) |
| C1 | Control Surfaces slots | partial | the page never re-reads CS state after a write (D1b); band list hardcoded at 10 (D28) |
| C2 | IR remote with learn | partial | stale receiver-live gate (D1b); `arm_learn` discards its status (D29); capture needs HW |
| C3 | Display wiring, config, pages | partial | Panel State and the current-page marker freeze at open (D1b) |
| C4 | Channel Groups | pass | footer clipped, so "Match Members Exactly" is never visible (D30) |
| C5 | Macros | pass | firing needs HW |
| L1 | Notifications applied live | partial | seq-gap recovery dead (D1); device-side preset load leaves a false `*` (D2b); INPUT_FORMAT does not re-lay-out (D32) |
| L2 | Multi-device | **fail** | the reader is never re-armed on switch and holds the old interface (D6b) |
| V1 | Console palette everywhere | pass | but the default theme is `amber`, not `console` (D33) |
| V2 | Disabled / unsupported rules | partial | three desaturation rules unimplemented (crossfeed params, signal-generator chips, loudness/psybass chips) |
| Q1 | 80x24 / 200x60, help overlay, animation, `--lite` | pass | `--no-animation` from `DESIGN.md` 9 does not exist (D34) |

## 6. Working agreements

**Em-dashes.** Three, all ours: `crates/dspi-session/src/probe.rs:388` (a
doc comment), `docs/firmware-notes.md:133` and `:148`. The 31 others are in
the vendored `crates/dspi-proto/firmware/*.h`, which must not be edited, and
in the superseded `REDESIGN_SPEC.md`. No em-dash reaches a UI string.

**Nothing about device shape compiled in.** Broadly holds. Channel counts,
band counts, filter types, caps maxima and feature presence come from
`DeviceState` / `Capabilities`. The caps parse is v13, reads the four
post-table maxima at `4 + 4*type_count` (`probe.rs:312`) with a test that
varies `type_count` over 8/9/12, reads per-noun descriptors, and drops a
noun whose `actions == 0`. Exceptions found:

- `settings/cs_model.rs:1338` and `:1359` hardcode `(0..10)` PEQ bands where
  `caps.max_bands` (set at `probe.rs:184`) is available. D28.
- `settings/groups.rs:68` and `settings/macros.rs:69` declare `health: [u8;
  8]` and index it by the caps-served `max_groups` / `max_macros`. Latent
  out-of-bounds if a device reports more than 8. D35.
- `settings/inputs.rs:120` (S/PDIF instances 1..4) and `:234` (I2S channels
  2/4/6/8) are fixed lists where the Console uses `spdifInputCount` and
  `i2sMaxInputChannels`. D27.
- `screens/mod.rs:201` maps `Platform::Rp2040 => 42.0` ms. This is the
  decision recorded in `DESIGN.md` 11 and is fine.
- `0x1FF` appears only in fixtures and tests.

**Every protocol fact cites a firmware header line.** Mostly: 86 header
citations in `packets.rs` alone. Two classes do not:
`SpdifRxStatusPacket`, `AdatInputStatusPacket`, `AdatStatus` and
`I2sSlaveStatusPacket` have no struct in the vendored headers and their
offsets are pinned to `survey-firmware.md` section 6.6; and the whole
notification layer cites `notify.h`, which is not one of the six vendored
files (`notify.rs:29,44`). Either vendor `notify.h` or say in
`firmware-bump.md` that `survey-firmware.md` is the citation of record for
these. Their correctness is **needs HW**.

**Every key named on a key line or in a `keys()` table is handled.**
Covered for every screen and all 12 Settings pages. One gap: the Settings
key test walks only top-level rows, so keys that live inside an expanded
Control Surfaces / Groups / Macros card body are not exercised. And
`EVERY_PAGE` (`settings/mod.rs:2040`) is a hand-written array of 12; a new
page not added to it is silently skipped by both the key test and the
both-sizes test.

**Every screen has golden frames at 80x24 and 120x40.** Yes:
`screens::tests::every_screen_fills_the_detail_region_at_both_sizes`,
`settings::tests::every_page_fills_the_screen_at_both_sizes`,
`matrix::tests::the_panel_fills_the_pane_at_every_size` (which also does
200x60), and a `golden_frames_at_both_sizes` in each of crossfeed, loudness,
leveller, psybass, upmixer, signals, stats, monitor, autoeq. Caveat above:
these assert `contains` and a line count, not a stored frame.

**Console wording verbatim.** Overwhelmingly true, and checked string by
string across every panel and page. The deliberate re-wordings found are
listed as cosmetic defects; the significant cluster is the Control Surfaces
row labels (`Component` / `Controls` / `On Press` / `Gesture` / `GPIO` for
the Console's `Type` / `Parameter` / `Action` / `Event` / `Pins`) and the
Groups page's `Channel Type` for the Console's kind picker.

## 7. Defects

Severity: **B** blocks parity, **W** wrong but usable, **C** cosmetic.

### Blocks parity

**D1. A lost notification never triggers the re-read.** B.
`crates/dspi-session/src/state.rs:676` sets `stale = true` on `n.lost` (also
`:689`, `:694`), and `stale` is read by nothing outside its own tests.
`crates/dspi-tui/src/live.rs:1943-1955` re-reads only on
`Applied::NeedsReread` and `Applied::PresetLoaded`. A dropped packet
followed by a `ParamChanged` that patches cleanly returns
`Applied::Section`, so `reread` stays false and the shadow is permanently
wrong for whatever the lost packet carried. `PLAN.md` Phase 2A: "a `seq` gap
… trigger a full chunked re-read". This is the headline Phase 2A exit
criterion and it does not hold.

**D1b. The three Control pages never re-read control-surface state after a
write.** B. `crates/dspi-tui/src/settings/mod.rs:1671-1698`
(`session_result`) sets `cs_dirty` and forwards the reply; it never re-reads
`CsData`. `SettingsData::read` runs only when Settings opens
(`live.rs:1443`). `settings/surfaces.rs:200-205` (`adopt`) copies
`status` / `display_status` / `groups` / `macros` out of `cx.data.cs`, which
is the same frozen snapshot it was built from, so it is a no-op.
`survey-firmware.md` 3.14 is explicit: "Binding-config changes do NOT push
notifications; re-read after writing." Symptom: add a control, apply it
successfully, and the card renders the pill **Inactive** and the row **"Not
running: applied. Reassign the conflicting pin, then apply."**
(`surfaces.rs:1658-1662`, because `slot_status[slot]` is still 0 and
`explain_status(0)` is `applied`). Knock-ons: `receiver_live`
(`surfaces.rs:1196`) and the Learn gate (`surfaces.rs:2218`) refuse with
"No IR receiver is active - apply the receiver first."; Panel State and
`nak_count` (`surfaces.rs:1416-1441`) and the `◉` current-page marker
(`surfaces.rs:1514`) freeze; group and macro slot health
(`groups.rs:190`, `macros.rs:434`) and the macro Running badge
(`macros.rs:137`) freeze; and `SettingsData::claims` (`mod.rs:202-207`) is a
snapshot, so a pin claimed by a control added this session is still offered
as free to the next one. No test renders a card after a successful apply.

**D2. Copy-to-slot writes the live parameters over the stored source
slot.** B, and it loses data. `crates/dspi-tui/src/live.rs:1735-1750`
(`copy_preset_to`): after `preset.save dest` it issues `preset.save source`
to restore the firmware's "last active slot". The comment says "Data-wise
the second write is a no-op", which is only true when the preset is clean.
With unsaved changes the second write flashes them over the saved source
preset. It also never raises the unsaved-changes prompt, which
`survey-console.md` section 4 lists copy-to among the triggers for
precisely this reason.

**D2b. A device-side `PRESET_LOADED` leaves a false dirty marker.** B.
`crates/dspi-tui/src/live.rs:1946-1950` re-reads but never calls
`mark_saved()`; the host-initiated path does (`live.rs:1172`). Load a preset
from a control surface or the front panel and the app then compares the new
preset against the *previous* preset's baseline: `*` on the slot forever,
plus a spurious Unsaved Changes dialog on the next preset switch, device
switch or quit. Same omission on `BulkInvalidated { source: Preset }`
(`live.rs:1945`).

**D3. The WIDTH column is blank for 12 dB/oct shelves.** B.
`crates/dspi-proto/src/enums.rs:96-101`: `uses_q()` returns true only for
Peaking, Notch, AllPass, LowPass and HighPass, so `screens/filters.rs:206`
and `:340` draw nothing in WIDTH for Low Shelf / High Shelf and there is no
way to edit their Q from the filter list. `survey-console.md` 2.12 hides Q
only "for crossover and first-order types", and `DESIGN.md` 2.1's own
reference layout draws `Low Shelf 12 dB/oct   105 Hz   +8.8 dB   0.707`.
Confirmed in the gallery.

**D4. The Linkwitz target Qp is read-only.** B.
`crates/dspi-tui/src/screens/linkwitz.rs:298-304` renders Qp as a readout;
`:50` (`QP_NOTE`) says "the command grammar has no field for it". That is
not true: `screens/mod.rs:383-386` emits qp as the seventh `eq` token and
`dspi-cmd/src/lib.rs:464` parses it, which is the decision recorded in
`DESIGN.md` 11. The Console's popover has "Target fp **and Qp**" (step 0.01,
min 0.1). One of the four Linkwitz parameters is unreachable from the TUI.

**D5. A disconnect is un-observable.** B.
`crates/dspi-tui/src/live.rs:561` sets `m.connected = true;`
unconditionally inside `sync_model`, and `sync_model()` is the last thing
`tick` does (`live.rs:1975`), after `live.rs:1958` sets it false on
`is_disconnected()`. The flag is therefore always true. `m.devices` is
populated only in `shell/fixture.rs:74`, so `shell/mod.rs:810`'s
`!m.connected && m.devices.is_empty()` branch is unreachable: the dot is
permanently green and the Console's "No Devices" and the 0.4-opacity dim
(`survey-console.md` section 4) can never appear. Compounding,
`session.meters()` failure is swallowed (`if let Ok(m)`, `live.rs:1902`), so
the control path never reports the disconnect either.

**D6. `Ctrl-S` inside Settings saves a preset, not the page.** B.
`crates/dspi-tui/src/settings/mod.rs` has no handler for `Char('s')`
anywhere, so `SettingsScreen::handle` returns `Unhandled` and
`shell/mod.rs:344-356` falls through to `ShellEvent::SavePreset`.
`DESIGN.md` 6.12: "`Ctrl-S` saves from anywhere in Settings." Today it
issues an unrelated device write.

**D6b. The notification reader is never re-armed on a device switch.** B.
`crates/dspi-tui/src/live.rs:2060-2064` builds `notifications` once, from
the first session, outside the loop; `live.rs:2091-2098` replaces `*session`
on a switch and calls `adopt`, leaving `notifications` pointing at the old
device. `UsbNotifications` holds a clone of the claimed `nusb::Interface`
(`dspi-transport/src/usb.rs:200-207`), so dropping the old `UsbTransport`
does not release it: the reader keeps delivering the *old* device's
`PARAM_CHANGED` frames into the *new* device's bulk shadow at the same
offsets. On macOS the old vendor interface also stays claimed for the life
of the process, so switching back fails with the "another application is
holding it" message about itself. (Worth double-checking nusb's clone
semantics before treating the silent-corruption half as fact; the
never-re-armed half follows from the code alone.)

**D7. Slave BCK Pin is not editable.** B.
`crates/dspi-tui/src/settings/i2s.rs:149` builds the row with `enabled:
false` in both Unified and Split. The Console renders the same row and
enables it in Split (`DSPi_ConsoleApp.swift:7302-7322`, `setI2SSlaveBckPin`,
0xC2 role 1). `settings/mod.rs:989-992` concedes there is no registry path
for role 1, which also means Revert cannot restore the slave pair.

**D8. The echo line has no contextual action strip.** B for I2.
`crates/dspi-tui/src/shell/mod.rs:1033-1049` (`draw_echo`) draws only the
left-hand text. `DESIGN.md` 2.1 puts "contextual actions for the focused
region" on the right, and 7.6 names them: `Enable All │ Bypass All   Clear
All   PEQ │ XO`, mirroring the Console's `BypassAllControls` split pill
whose halves grey when they would be a no-op. The actions exist only as the
unlabelled keys `a` / `A` / `D` / `x` on the key line.

**D9. The upmixer's "No device connected" status is missing.** B.
`crates/dspi-tui/src/screens/upmixer.rs:68-87` produces four of the five
STATUS strings in `survey-console.md` 2.18 plus an invented `"Idle"`
fallback (`:85`). The string `No device connected` appears nowhere in
`crates/`.

**D10. Test Signals is missing the "No device connected" start blocker.** B.
`crates/dspi-tui/src/screens/signals.rs:380-391` returns three of the four
blockers in `survey-console.md` 2.19. A disconnected device falls through to
`Ready · Sine` with Start enabled.

**D11. There is no force-reconnect.** B for S8. The Console's right-click on
the device name forces a reconnect, and its tooltip says "Not connected.
Right-click the device name to retry." Nothing in `dspi-tui` offers it
(`grep reconnect` finds only a stats counter at `actions.rs:104`).

**D12. `Enter` on the Source row opens nothing.** W/B.
`crates/dspi-tui/src/live.rs:1250` is `ShellEvent::Source(None) => {}`.
`DESIGN.md` 2.3: "Preset and Source are picker rows: `←`/`→` cycle, `Enter`
opens the list." The Console's Source is a popup over the seven sources.
Cycling works, so the sources are reachable.

**D13. Control Interfaces never reports the device's real status.** B.
`crates/dspi-tui/src/settings/interfaces.rs:543` and `:554` set
`status_message(0, …)` unconditionally instead of the `PIN_CONFIG_*`
result the Console maps (`DSPi_ConsoleApp.swift:1948-1978`). Four of the
five status strings in `survey-console.md` 2.33 ("A pin is out of range or
lacks the required <IFACE> mux function", "A pin is already claimed by
another output or interface", "Baud rate is out of range (9600 - 1000000)",
"Address is out of range (0x08 - 0x77)") are therefore unreachable outside
tests. `CtrlIfaceStatus.uart_last_status` / `i2c_last_status` are read into
`SettingsData` and never used. The same optimistic pattern appears on the
pin writes in `outputs.rs:186-193,207,216`, `inputs.rs:427,436,472,489,502`
and `i2s.rs:238,249,271,283`.

**D14. The graph pop-out has no independent visibility.** B for G3.
`crates/dspi-tui/src/shell/mod.rs:923-933` reuses the shell's single
visibility map. `survey-console.md` 2.22 specifies an "optional independent
visibility map", and `settings/config.rs:40`'s `popout_follows_selection` is
written and read only by its own Settings row (`settings/graphing.rs:229`);
nothing consults it.

**D15. `dspi screenshot` needs a device.** B for the Phase 3 exit
checklist. `crates/dspi/src/main.rs:669` calls `connect(serial)` before
anything else, so without hardware it prints `dspi: no DSPi device found`.
`DESIGN.md` section 10: "`dspi screenshot` uses the same fixture so
documentation screenshots need no device." The subcommand is also absent
from `--help` (`main.rs:139-166`) while `main.rs:699` refers the user to a
`dspi screenshot --help` that does not exist.

### Wrong but usable

**D16. The gallery renders no widget catalogue and no chosen colour
depth.** W. `crates/dspi-tui/examples/gallery.rs:69-73` fixes
`ColorDepth::TrueColor` unless `--ansi`, and there is no widget mode.
Phase 3's exit line asks for "every widget in every colour depth".

**D17. `Space` is a no-op on three quick-strip cells.** W.
`crates/dspi-tui/src/live.rs:1212-1217` and `:1179-1188`: `strip_path`
returns `None` for cell 0 (Matrix), 5 (Stats) and 6 (Settings), so `Space`
does nothing there. `DESIGN.md` 2.3 maps `Space` to the Console's left
click, which opens those windows. `Enter` works.

**D18. No CRC path on preset load.** W.
`crates/dspi-tui/src/live.rs:1166-1177`. The Console distinguishes a corrupt
preset with "Preset data is corrupted."; that string appears nowhere in the
workspace, so a corrupt payload reads as "Load Failed".

**D19. "Set as Default" is never disabled.** W.
`crates/dspi-tui/src/screens/presets.rs:64` always offers it; the Console
disables it when the slot is already the default, and `shared.default_slot`
is on hand (`live.rs:1710,1787`).

**D20. No generic hazard confirm.** W. `Hazard::Flash` and
`Hazard::Irreversible` rows are not gated: `:dev.reset` on the `:` line
(`registry.rs:2083`, `Hz::Irreversible`) reaches `Session::write` through
`live.rs:870-875` with no confirm, as does the same command on the CLI.
Every Console-visible destructive button does have its own confirm.

**D21. Band gain is seeded and re-formatted at 1 dp.** W.
`crates/dspi-tui/src/screens/filters.rs:337`, `:537`, `:743` use `{:+.1}`
and `format!("{:.1}")` when arming and nudging. The Console's GAIN
ValueField is 3 dp (`survey-console.md` 2.12), and `DESIGN.md` 5 says "gain
1 dp displayed, 3 dp editable". Arming a band whose gain is 8.875 seeds
`8.9` and any nudge writes the quantised value back.

**D22. `explain_status` is missing 0x05.** W.
`crates/dspi-session/src/surfaces.rs:96-126` covers 0x00-0x04 and 0x10-0x25.
`control_surfaces.h:607` says "0x00..0x05 reuse the shared PIN_CONFIG_*
namespace" and `config.h:612` is `PIN_CONFIG_INVALID_PARAM 0x05`, reachable
on a display record with a bad address or brightness. It renders as
`refused with status 0x05`. `surfaces.rs:1240`
(`every_status_code_explains_itself`) does not include it.

**D23. Matrix Identify is a stub.** W.
`crates/dspi-tui/src/screens/matrix.rs:864`:
`ScreenEvent::Status("Identify arrives with Phase 6")`. The sidebar's `i`
already implements it (`live.rs:1379-1396`).

**D24. Crossfeed PARAMETERS never dim outside Custom.** W.
`crates/dspi-tui/src/screens/crossfeed.rs:172-189` builds both rows fully
enabled. `survey-console.md` 2.14 says "PARAMETERS (opacity 0.5 unless
Custom)" and section 5 repeats it. `Param::enabled()` exists and `ParamRow`
honours it (`widgets/param.rs:195,246,253`); it is simply never called here.

**D25. Output chips never dim, though the caption promises they will.** W.
`crates/dspi-tui/src/screens/signals.rs:590-611` builds every chip with
`enabled: true` (explicit comment at `:607`) directly under the caption
"Dimmed outputs are disabled in the matrix mixer and stay silent." Same
hard-coded `true` at `loudness.rs:113` and `psybass.rs:160`.

**D26. Config import skips the progress dialog.** W.
`crates/dspi-tui/src/live.rs:1516-1533` applies synchronously and goes
straight to the report. `Dialog::progress(…, "Writing settings to the
device...")` exists (`widgets/dialog.rs:163`) and is exercised only by a test
(`dialog.rs:660`). `survey-console.md` section 4 and `DESIGN.md` 7.14 both
put a progress sheet between the options and the result.

**D27. Two Inputs choice lists are fixed where the Console reads the
device.** W. `crates/dspi-tui/src/settings/inputs.rs:120` offers Instances
1..4 always; the Console uses `1...vm.spdifInputCount`.
`crates/dspi-tui/src/settings/inputs.rs:234` offers Channels 2/4/6/8
always; the Console strides `2...vm.i2sMaxInputChannels` and shows a static
"2" on a stereo-only part. The Terminal offers counts the device will
refuse.

**D28. The CS band picker is hardcoded at ten bands.** W, and a
working-agreement violation. `crates/dspi-tui/src/settings/cs_model.rs:1338`
and `:1359`: `(0..10).collect()`. `caps.max_bands` is served by the device
(`probe.rs:184`) and `state.bands()` already uses it. The `20..=23`
crossover range beside it is a frozen firmware constant and is fine.

**D29. `arm_learn` discards the device's status byte.** W.
`crates/dspi-session/src/surfaces.rs:624-627`:
`control_in(REQ_CS_IR_LEARN, ARM, 1)` returns `PIN_CONFIG_SUCCESS` or
`CS_STATUS_NO_IR` (`control_surfaces.h:798`) and the result is dropped, so a
refused arm shows "Waiting for a button..." until Cancel or a device-side
timeout.

**D30. Footer notes are clipped at four wrapped lines.** W.
`crates/dspi-tui/src/settings/mod.rs:362` (`Row::Note`) wraps to 4 and
ellipsises. At every width tested: the Surfaces footer (`surfaces.rs:41`)
loses "…use Save to keep them across a reboot, or Revert to discard them.";
the Groups footer (`groups.rs:32`) is cut before **"Match Members
Exactly"**, which `survey-console.md` 2.35 names explicitly; the Macros
footer (`macros.rs:32`) loses its Save/Revert sentence; and the Control
Interfaces footer (`interfaces.rs:54-60`) loses "Fit external pull-ups
(2.2k - 4.7k) on the I2C bus."

**D31. The volume-mode choice is never persisted.** W.
`crates/dspi-tui/src/live.rs:1269-1275` flips `model.volume_mode` and never
writes it back to `AppConfig`; it is only read at startup (`live.rs:511`).
The Console persists it in app settings (`survey-console.md` 2.6).

**D32. `INPUT_FORMAT` does not re-lay-out the channel list.** W.
`crates/dspi-tui/src/live.rs:1951-1953` prints a note. `PLAN.md` Phase 2A:
"`INPUT_FORMAT` re-lays-out the channel list." `sync_model` builds the input
rows from `caps.num_inputs` with `inactive` hardcoded false
(`live.rs:591-599`), and `Meters::active_inputs` (`probe.rs:686`) is decoded
and read by nothing.

**D33. The default theme is `amber`, not `console`.** W.
`crates/dspi/src/main.rs:45`: `None => Palette::Amber`. `DESIGN.md` 4.4 names
`console` the default. `main.rs:164`'s help line also says
`--theme amber|dark  colour scheme; amber is the default` while
`Palette::NAMES` is `console, amber, dark, mono`. A user who opens the app
without `--theme` does not get the Console palette that V1 is about.

**D34. `--no-animation` does not exist.** W.
`DESIGN.md` 9 says the connect animation is "Skipped with `--lite`,
`--no-animation`, `TERM=dumb`". Only `--lite` and the glyph/depth detection
are implemented (`main.rs:81`, `perf.rs:33-48`).

**D35. `health: [u8; 8]` indexed by a caps-served count.** W (latent
panic). `crates/dspi-tui/src/settings/groups.rs:68` and
`crates/dspi-tui/src/settings/macros.rs:69`; indices come from
`caps.max_groups` / `caps.max_macros` via `drafts.len()`, used at
`groups.rs:190` and `macros.rs:434`.

**D36. `ADAT_STATE` (0x08) is stored and never displayed.** W.
`crates/dspi-session/src/state.rs:710-717` keeps `adat_state`; it has zero
readers. The ADAT-output status the Console shows has no source.

**D37. Every write costs a full bulk re-read.** W.
`crates/dspi-tui/src/live.rs:761`, `:771`, `:911` call `self.refresh(session)`
after each successful write, and `refresh` (`live.rs:726-732`) is a chunked
5944-byte read. `PLAN.md` Phase 2A says the write path should update
`DeviceState` on `Outcome::Accepted` "so the UI never waits on a re-read for
its own writes". `run_commands` (`live.rs:786-800`) does one write and one
full re-read per line, so a multi-line gesture is many bulk reads.

**D38. No sequence-based suppression of our own `HOST_SET` echo.** W.
Suppression is by source (`Source::is_ours()`, `notify.rs:83-85`, used at
`live.rs:1936`) and suppresses only the echo text. A second host (the
Console open at the same time) writing a value is reported as changed by
this host. `PLAN.md` Phase 2A asks for suppression "by sequence".

**D39. `SettingsData` is not reset on a device switch.** W.
`crates/dspi-tui/src/live.rs:1653-1676` (`adopt`) clears `stats`,
`preset_names`, `occupied` and `default_slot` but not
`self.screens.settings`, so the CS model, pin claims, UART/I2C config and
preset directory hold the previous device's data until Settings is reopened.
The Console's rule ("Settings drafts discarded on a different serial, kept
on re-plug of the same") is not implemented in either direction.

**D40. Firmware Update is offered unconditionally.** W.
`crates/dspi-tui/src/live.rs:203,942`: the survey's "hidden on STM32" and
"Option skips the confirm" are both absent. Note `dspi_proto::Platform`
(`lib.rs:60`) has only Rp2040 / Rp2350 / Unknown, so the STM32 rule is
currently unreachable rather than wrong.

**D41. No Clear Favourites.** W. `dspi-session/src/autoeq.rs` has
`save_favourites` but nothing calls it with an empty list; the verb is not
in `APP_VERBS` (`live.rs:192-204`) nor in the AutoEQ panel's keys
(`screens/autoeq.rs:28-32`). `survey-console.md` section 3 lists Favorite
Profiles + Clear Favorites.

**D42. Stats has no Core 1 mode row.** W.
`crates/dspi-tui/src/screens/stats.rs:210-227` shows Clock Frequency / Core
Voltage / Sample Rate / Temperature; `survey-console.md` 2.20 names "sample
rate, core1 mode, etc." `diag.core1` (0x7A) is in the registry
(`registry.rs:2711`).

### Cosmetic

**D43.** The graph pane carries no title. `shell/mod.rs:912-916` passes `""`
unless `graph_popout`. `DESIGN.md` 2.1 draws `┌ Filter Response ── ⤢ ┐` and
`survey-console.md` 2.9 makes "Filter Response" the detail-pane header. The
`⤢` pop-out affordance is absent too, and the graph and detail share one box
where the mockup draws two. Not recorded in `DESIGN.md` 11.

**D44.** Mono presentation is incomplete. `widgets/legend.rs:21-29`
(`pill_text`) keys only on `Glyphs`, so pills read `● IN1` / `* IN1`, never
`DESIGN.md` 4.3's `[IN1]` with `*` for visible. `widgets/chips.rs:129-134`
renders unselected chips `[2]` rather than `[ 1]`, and emits the non-ASCII
`■` and `ø` under `Glyphs::Ascii`.

**D45.** Panel captions are drawn above the chip row, not below it:
`crossfeed.rs:142-151`, `loudness.rs:95-120`, `psybass.rs:140-167`,
`signals.rs:584-617`. `DESIGN.md` 7.8's template is header, chips, caption.

**D46.** PRESET radio rows are two lines each (`panel.rs:296-303,836-874`)
where `DESIGN.md` 7.8 shows one: `● Default    700 Hz / 4.5 dB - Balanced,
most popular`. Costs 8 rows where 4 were specified, with the width to spare.

**D47.** Unsigned dB fields carry a `+`. `widgets/text.rs:83-89` formats
every `dB` field `{:+}`, so Crossfeed Feed Level (0..15) reads `+4.5 dB`,
Loudness Reference SPL (40..100) `+80 dB`, Leveller Max Gain (0..35)
`+12.0 dB`, Psybass Drive (0..18) and Original Bass (-60..0) likewise.

**D48.** The upmixer hides three of its four gauges when the matching
engine is off (`upmixer.rs:117-138`); `survey-console.md` 2.18 shows all
four whenever the status is active.

**D49.** "Stop immediately, no fade" appears nowhere; the transport draws
`Stop` / `Stop now` (`panel.rs:1015-1019`).

**D50.** The tile blurb is a caption row under the grid (`signals.rs:576`);
`DESIGN.md` 7.9 puts it on the echo line.

**D51.** The matrix conflict marker is one-sided: `matrix.rs:503` draws `!`
only on the PDM column, though `would_conflict` (`matrix.rs:123-135`) is
symmetric and the ENABLE row does colour both (`matrix.rs:569`).

**D52.** Matrix column names are drawn in `theme.label()` (`matrix.rs:425`);
`DESIGN.md` 7.7 says "Column headers in the output colour". Only the `OUT n`
descriptor row is coloured.

**D53.** Sidebar meters are 8 cells at the reference width
(`shell/sidebar.rs:72`); `DESIGN.md` 7.1 and 6.9 specify 10 plus the clip
cell. The 24-column sidebar cannot hold `DESIGN.md`'s own name-9 +
meter-10 + clip + pill, so either the widths or the sidebar width needs
restating.

**D54.** The Linkwitz row shows only `f0`, `fp` and `⚙`
(`filters.rs:326-332`); `DESIGN.md` 7.6 specifies `f0 40 Hz Q0 0.5 → fp 25
Hz Qp 0.71  +8.2 dB  ⚙`.

**D55.** FREQ nudges by a semitone ratio rather than the Console's 10 Hz
`scrollStep` (`filters.rs:763-775`). Deliberate and commented; min 10 is
honoured.

**D56.** `"Database rebuilt successfully!  Entries: {n}"` with two spaces
(`live.rs:2014`); the Console's string has a newline. Same at `live.rs:1052`.

**D57.** "Reset to Built-in" is added only when `has_user_database()`
(`live.rs:1107`), and the button index mapping in `finish_dialog`
(`live.rs:1602`) shifts when it is absent. Harmless today (Cancel falls
through `_ => {}`), fragile.

**D58.** `Row::info("r", "Reset Watermarks")` (`stats.rs:482`) renders the
key as the label and the button text as the value, i.e. backwards. The
action works.

**D59.** Starvation "Time since last event" / "Time between last two" count
2 s polls rather than a wall clock (`stats.rs:494-512`), so they read in
whole 2 s steps.

**D60.** AutoEQ result rows omit the Console's leading form-factor icon
(`screens/autoeq.rs:305-336`).

**D61.** Hold / Release pickers fall back to index 0 for an off-list device
value, e.g. 7 ms shows as "5 ms" (`settings/global.rs:266-269,279-282`); the
Console appends the out-of-list value.

**D62.** Macros availability is `max_macros > 0` (`settings/mod.rs:788`);
the Console is `maxMacros > 0 && maxMacroSteps > 0`.

**D63.** Inputs availability is `spdif_multi_input || i2s_input_channels ||
adat_input || lg_sound_sync` (`settings/mod.rs:778-783`); the Console uses
`inputSourceSupported`, which `probe.rs:258-270` does not probe. Firmware
with input-source support but none of those four loses the page.

**D64.** "Locked to 128x at <rate>" is driven by the configured
`input_config.i2s_input_rate` (`settings/i2s.rs:50-55,181`); the Console
uses the device's *running* `sampleRateHz` (`mck256UnsupportedAtCurrentRate`).
Divergence is **needs HW** to see, but the source differs.

**D65.** The I2S Input section footer ("Wire one ADC serial-data line per
stereo pair…", `DSPi_ConsoleApp.swift:7748-7752`) is not carried
(`settings/inputs.rs:261`).

**D66.** Graphing carries an extra "Volume / Slider" section
(`settings/graphing.rs:236-251`) that the Console keeps in the sidebar's
volume menu, and the Console's persisted `graphHeight` (default 250) has no
counterpart; `GraphHeight` (`shell/model.rs:99`) is session-only.

**D67.** Settings Revert and Advanced Reset raise confirms the Console does
not have (`settings/mod.rs:1798-1808`, `settings/advanced.rs:112-119`).
Additions, not omissions.

**D68.** Advanced derives factory channel names from the live slot types
(`settings/advanced.rs:75-102`); the Console passes `slotTypes: [0,0,0,0]`
and always writes "SPDIF n L/R". Arguably better, but different.

**D69.** Control Surfaces row labels are re-worded: `Component` /
`Controls` / `On Press` / `Gesture` / `GPIO` for the Console's `Type` /
`Parameter` / `Action` / `Event` / `Pins`; Groups uses `Channel Type` for
the Console's kind picker. Against the "same words as the Console"
agreement, though each re-wording is defensible on its own.

**D70.** Action names `Next` / `Increase` and `Previous` / `Decrease`
(`cs_model.rs:293-296`) where `survey-console.md` 2.34 lists "Up, Down";
`LINK_ABS` is titled "Match Members Exactly" and `GROUP_ALL` "Require Every
Member" (`surfaces.rs:854-871`) where the survey reads
"Link Absolute, Group All (Match Members Exactly)". The survey's 2.35 footer
ties "Match Members Exactly" to relative-vs-absolute, which is `LINK_ABS`,
so the code is probably right and the survey line condensed. One Console
lookup at `DSPi_ConsoleApp.swift:6323` settles both.

**D71.** The macro card carries a `Run` / `Stop` button
(`macros.rs:412-420`, `REQ_CS_MACRO_FIRE`) the survey does not list. An
addition; confirm the Console has it.

**D72.** The EDIT_GATED warning is shown only when `uses_page_value() &&
!can_arm_editing()` (`surfaces.rs:1470-1479`); the survey states it with the
flag unconditionally. `can_arm_editing` (`surfaces.rs:343-357`) also mixes
draft bindings and IR with *live* macros.

**D73.** `used_by` counts bindings only (`groups.rs:141-147`); IR commands
and macro steps also address groups with `CS_FLAG_GROUP` and are also
deactivated by an empty or re-kinded group, so "Used by N control(s)…" and
the Remove confirm can read 0 while remote keys and macro steps break. Low
confidence.

**D74.** Digit keys index CS slots, not visible cards
(`surfaces.rs:2557`, `groups.rs:470`, `macros.rs`), so with controls in
slots 0 and 5 the key `2` does nothing. Inconsistent with the 1-based
band-jump convention pinned in `cd86123`.

**D75.** The remote-change announcement lands on the status line, not the
echo line, and only the last change per tick survives
(`live.rs:1922,1937,1964-1966`). Wording is `changed by a control surface`
where `DESIGN.md` 9 says `changed by control surface`. The test
`live::tests::a_notification_from_elsewhere_lands_in_the_model_and_the_echo_line`
asserts the status field, locking the divergence in.

**D76.** `DESIGN.md` 9's easing and label flash are implemented only for the
sidebar volume readout, and only for the `user_volume` / `master_volume`
sections (`live.rs:45,655-664,1938-1942`). No other value eases and there is
no flash-once-in-`ok` anywhere.

**D77.** `MockNotifications` spins the reader thread at 100 % CPU
(`dspi-transport/src/mock.rs:31-38` returns `Ok(vec![])` immediately and
`notify.rs:348-350` `continue`s with no sleep). Test-only; the USB path
blocks 250 ms.

**D78.** `has_unsaved_changes()` clones and blanks the whole 5944-byte
packet on every call (`state.rs:636-641`), and `sync_model` calls it every
frame (`live.rs:566`): two 6 KB allocations per frame at 20 Hz.

**D79.** The theme is not selectable from the config file. `DESIGN.md` 4.4
says "`--theme` and the config file select them"; `settings/config.rs` has
no theme key.

**D80.** `actions.rs:288` `reset_watermarks()` is dead code; the panel routes
through `ScreenEvent::Command("diag.buffers.reset")`.

**D81.** The input-mismatch alert is ordered by the page you are on rather
than by the pair (`screens/input.rs:221-234`): from IN2 it reads "Inputs 2
and 1 don't match" with `Keep IN2` first. Low confidence; the Console source
for this alert was not read.

## 8. Notes for the three deep audits

### 8.1 `DeviceState`, `notify.rs`, and `live.rs` `tick`

Start at **D1**: `stale` being write-only is a one-line grep that
invalidates the marquee Phase 2A exit criterion. Decide whether recovery
belongs in `DeviceState` (an `apply` that returns `NeedsReread` whenever
`n.lost`) or in the consumer; the former is testable without a TUI and lets
`dspi watch` report it too.

**The "one typed model" claim is the real finding.** `DeviceState`
(`state.rs:588-601`) holds `caps`, `bulk`, `meters`, `clip_latched`, `saved`,
`siggen_state`, `adat_state`, `i2s_slave_state`, `adat_input_state`,
`ir_learn`, `upmix_status`, `stale`. Roughly a third of Phase 2A's list. The
rest is spread across `Capabilities` (active slot, siggen caps, CS caps),
`SettingsData` in the *TUI* crate (`settings/mod.rs:74-92`: preset
directory, startup mode, CS bindings/names/IR/groups/macros/display/status,
UART/I2C config and status, pin claims, S/PDIF status), and `SharedState`
(`screens/mod.rs:64-91`: preset names, occupancy, default slot, buffer
stats, AutoEQ). Output config mode, master volume mode, saved master volume
and core1 mode are registry-only, read on demand. Consequences: nothing
notified can invalidate `SettingsData` or `SharedState`; a `PARAM_CHANGED`
into a CS or control-interface field has nowhere to go; `SettingsData` loads
lazily on Settings open and is never reset on a device switch (D39); and a
second front end (the CLI) can reuse none of it. Ask whether this was a
deliberate revision: if so, `DESIGN.md` 11 should record it and Phase 2A's
wording should be amended rather than ticked.

**D6b is the L2 audit in one bug.** The `nusb::Interface` clone inside
`UsbNotifications` is the subtle half, and it is why the failure is silent
corruption rather than a clean disconnect. Verify nusb's clone semantics
before writing that half up as fact; the never-re-armed half follows from
`live.rs:2060` alone.

**D5 is trivial and trivially missed.** `m.connected = true` at
`live.rs:561` is unconditional. Whoever fixes it should decide whether
`connected` belongs on `DeviceState`, as `PLAN.md` says, rather than on the
shell model.

Two things this audit could not settle. (a) The Console's
`unsaved_changes_spec.md` is not in this repo, so it is unknown whether
`user_volume` belongs in `PresetSnapshot`. It is there
(`state.rs:398-400`, category "Global: Volume"), which means turning the OS
volume slider dirties the preset and raises Unsaved Changes on quit. If the
Console does not do that, HW-1's `UAC1` check will show it immediately.
(b) For crossfeed / leveller / psybass / upmix, when `enabled` flips the
diff emits only the on/off line and never the accompanying "parameters"
line (`state.rs:232-243` and the three parallel blocks); check that against
the spec.

The rest of the path is sound. `BulkPacket::decode` enforces the exact
`BULK_SIZE` and the stated `payload_length` (`wire.rs:140-158`), so every
`sec()` slice in `state.rs` is provably in bounds; `notify.rs::decode` guards
every field with `need()`; `patch` uses `checked_add` and refuses the header
and any run past the end. The only `expect` on device-adjacent data is
`section()` (`state.rs:454-459`), keyed by a compile-time name. No panics
found.

### 8.2 `settings/{surfaces,groups,macros,cs_model}.rs`

**`adopt()` is the structural hole in all three pages.** `surfaces.rs:200`,
`groups.rs:107` and `macros.rs:124` all guard on `drafts.len() !=
cs.<x>.len()`, which can only fire if the device changes shape mid-session,
which it cannot: `cx.data` is a clone frozen when Settings opened. Everything
else in `adopt` copies a snapshot onto itself. Either `SettingsData` needs a
refresh hook after a CS write, or `cs_model::run` (`cs_model.rs:990`), which
already receives the settled `CsStatusPacket` and discards everything but
`last_status`, should hand it back through `SessionReply` so each page can
update its own `status` / `ext` / `display_status`. Fixing this one thing
clears every symptom under D1b.

**`surfaces.rs` is three editors in one 3419-line file.** `Item` is a
40-variant enum indexed positionally through `build()` →
`filter(focusable)` → `nth(index)`. Every `act` rebuilds the whole row list,
and `popup_result` rebuilds it again to find the index: correct, but O(rows)
per keystroke and fragile. `Item::Pin` and `Item::DispPins` both need a
manual `offset` correction (`surfaces.rs:2093`, `:2372`) because the row
keeps the current pin at the head of its choice list. The IR and Display
sub-editors are the obvious split.

**`self.live` in `SurfacesPage` means two different things.** For bindings,
names and IR it is "what the device is believed to hold", advanced on Apply.
For `display_cfg` and `pages` it is the *draft*, written through immediately
(`write_display_cfg` / `write_page` mutate `self.live` before the session
request), so a failed display write leaves the local copy already moved.
`display_status` is only ever cleared, never re-read.

**`cs_model.rs` is the right shape and the least risky part.** Name tables,
link laws and unit codecs are cleanly separated and well tested (8.8
encoding, the two ten-times-apart delay scales, group-flag sweeping,
`PAGE_VALUE` carrying no operands). The `demo` fixture deliberately zeroes
`ADAT_ACTIVE`'s action mask to prove the unavailable-noun path.

**Host-side name tables the firmware cannot serve** are all in `cs_model.rs`
and are known drift risk: `INPUT_SOURCES` (7 entries, `:947`),
`enum_value_label`'s filter-type / crossfeed / upmix / leveller-speed tables
(`:966-1023`), `display_model_name` 1-8 (`:426`), `DISPLAY_ADDRESSES`
(`:196`), `display_default_address`. The counts are device-served; only the
labels are compiled in, which is unavoidable.

**Two flattening conventions on one page.** `target_choices`
(`cs_model.rs:1341`) merges Channels and Groups into one list with a
`"Group: "` prefix, while the noun picker uses `"{category} / {noun}"`.
`DESIGN.md` 6.4 sanctions a flat hierarchical popup, but not two dialects of
it. Relatedly, display page nouns use the flat `display_page_nouns`
(`surfaces.rs:1519`) and skip the "target space is non-empty" filter that
`valid_nouns` applies to binding and IR nouns.

Test coverage is genuinely good: 30 surfaces tests, 8 groups, 8 macros, all
passing, with golden frames at both sizes and wire-encoding assertions for
the binding, IR and display-page records, and a PENDING/BUSY wait-out. The
one visible gap is any test that renders a card *after* a successful apply,
which is exactly where D1b lives.

### 8.3 `theme.rs`, `graph.rs`, `widgets/`

This is the strongest area of the branch. `theme.rs:245-278` reproduces
`DESIGN.md` 4.1 and 4.2 exactly, all 17 channel truecolor/256/16 triples and
all 8 semantic rows, with `theme.rs:1094-1102` asserting 17 distinct 256
indices and `:1135-1141` that accent and warning avoid every channel.
`graph.rs` implements every decision in `DESIGN.md` 11: dashes 6-on-4-off
(`:557`), minor grid only at 2 and 5 per decade and only at
`plot.width >= 100` (`:327-332`), dB grid gated at 1.5 rows per step
(`:288`) with label steps doubled until two rows apart (`:481-483`),
curve-end labels at Ansi16 and Mono and for grouped curves with no selection
(`:394-405`) with colliders stacked (`:408-421`), and the legend wrapping to
a second row (`legend.rs:118-126`).

What the deep audit should look at:

- **Depth is not the same thing as palette, and the widgets disagree about
  which one governs.** `Theme::mono(glyphs)` forces `ColorDepth::Mono`
  (`theme.rs:331`), so `--theme mono` does reach the mono branches. But
  `legend.rs::pill_text` keys on `Glyphs` and `chips.rs` keys on
  `ColorDepth`, and neither produces `DESIGN.md` 4.3's `[IN1]` / `[ 1]`
  forms. Decide once whether the mono presentation rules hang off depth,
  palette or glyphs, and make every widget agree. D44.
- **No widget has a per-depth golden.** Almost every widget test builds
  `Theme::console(ColorDepth::TrueColor, …)`. Phase 3's exit line asks for
  goldens at truecolor, 256, 16 and mono, and the gallery cannot even be
  asked for a depth (D16). The 256-index distinctness test is the only thing
  standing between the palette and a regression.
- **`widgets/text.rs:83-89` signs every dB field.** D47. The formatter needs
  to know whether a field's range crosses zero.
- **`widgets/param.rs` has an `enabled()` path that two panels never
  use.** D24, D25. The disabled-row rendering exists and is correct; the
  screens simply hardcode `true`.
- **`DESIGN.md` 6.9's ten-cell meter does not fit the 24-column sidebar.**
  D53. Either the sidebar widths in `DESIGN.md` 2.1 / 8 or the meter width in
  6.9 has to give; the code chose 8 cells silently.
- **`Row::Note`'s four-line clip** (`settings/mod.rs:362`) is a widget-kit
  decision that silently truncates Console prose, including a string the
  survey names explicitly. D30.
- **The pane block** (`shell/mod.rs:902-960`) merges the graph and detail
  boxes and drops the `Filter Response` title and the `⤢` affordance. D43.

## 9. Needs hardware

Nothing below is a pass or a fail; it is what HW-1, HW-2 and HW-3 exist to
settle.

- HW-1: `dspi doctor`; `dspi dump` reporting V28 and 1.1.6; `dspi watch`
  showing `user_volume` with source `UAC1` from the OS slider and a
  `PRESET_LOADED` from `:preset load`; writing and reading back a CS
  binding; arming IR learn. Also: whether the firmware emits a seq gap in
  practice, which sets D1's severity in the field; whether `user_volume`
  belongs in `PresetSnapshot`; and the four status packets whose offsets are
  pinned to the survey rather than to a vendored header
  (`SpdifRxStatusPacket`, `AdatInputStatusPacket`, `AdatStatus`,
  `I2sSlaveStatusPacket`).
- HW-2: the Control Surfaces pages against real hardware; learning an
  actual IR code (`observe_learn`, `surfaces.rs:207-247`); a panel lighting
  up and `CsDisplayStatus.init_state` transitioning; a macro firing with
  `macro_running` / `macro_step` advancing; whether a real device's
  `slot_status` produces the reasons `inactive_reason` formats. D1b will make
  every one of these look broken until it is fixed, so fix D1b first.
- HW-3: the full matrix walkthrough on RP2350 and RP2040; the Identify
  blip melody; the 18-byte 0x42 Linkwitz write; the commit / revert /
  factory-reset / save-master / save-output-config effects; the bootloader
  handoff (`actions.rs:1352-1381`); live Stats values and the feature-gated
  ADAT / LG / I2S-slave / S/PDIF sections appearing and being removed;
  whether pin and interface writes are rejected the way D13 and D9 assume;
  and `dspi doctor` on Windows and Linux.

## 10. Summary

The branch is close. Firmware parity is complete, the design system is
excellent, the Settings pages and the DSP panels reproduce the Console
almost string for string, and 911 tests pass with fmt and clippy clean.

Five things stand between it and the exit conditions, in order:

1. **The notification path.** D1 (seq-gap recovery is dead), D2b (a
   device-side preset load leaves a false dirty marker), D5 (a disconnect is
   invisible) and D6b (the reader is never re-armed and holds the old
   interface). Exit condition 4 does not hold today.
2. **D1b**, which makes every Control Surfaces page report failure after a
   successful write, and which will invalidate HW-2 if it is not fixed
   first.
3. **D2**, which loses a saved preset.
4. **Three controls that are simply not reachable**: shelf Q (D3),
   Linkwitz Qp (D4) and the Slave BCK pin (D7).
5. **D33**, which means the Console palette is not what the app opens with.

Everything else is a caption, a colour, a step size or a string.
