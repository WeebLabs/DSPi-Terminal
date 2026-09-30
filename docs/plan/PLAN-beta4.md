# Plan: bring `console-parity` up to firmware and Console v1.1.6-beta4

*Written 2026-09-29. This plan follows `PLAN.md`, which is complete for its
own targets apart from the hardware checkpoints and the final deep audits.*

## 0. Where things stand

The `console-parity` branch did what `PLAN.md` asked. Phases 1 to 9 are
merged, the Opus audit and both fix rounds are merged, all 56 coverage rows
are ticked, and `cargo test --workspace` passes (974 tests, none ignored,
none failing, run on 2026-09-29). The branch is 141 commits ahead of
`main`, and `main` has nothing the branch lacks, so a merge is a
fast-forward with no conflicts.

Both targets have moved since the branch was planned:

| | Branch targets | Current | Change |
|---|---|---|---|
| Firmware (`/Users/weeblabs/DSPi`, `release/v1.1.6`) | `112f35b` (beta2) | `557bce7` (beta4) | 30 commits |
| Wire format | V28, 5944 bytes | V32, 6136 bytes | 4 appended sections, no offsets moved |
| Opcodes | 202 | 244 | 42 new, none removed |
| CS caps | v13 | v20 | 22 new nouns, 2 new component types |
| Console (`/Users/weeblabs/DSPi Console`) | `75793c3` | `9dbb07a` (beta4) | 138 commits, about 31,000 lines |

The consequence that matters most: `WireHeader::decode` accepts exactly
V28 (`crates/dspi-proto/src/wire.rs:91-96`), and the firmware's bulk apply
accepts exactly its own version. **The branch cannot talk to a device
running beta4 firmware**, which is the firmware the Console now bundles.
None of the hardware checkpoints HW-1 to HW-3 has been run yet, so every
hardware check still ahead would have to run against beta4.

What changed is recorded in two new surveys, which this plan rests on:

- `survey-firmware-beta4.md`: every host-visible firmware change.
- `survey-console-beta4.md`: every user-visible Console change.

## 1. Recommendation on ordering

Do the beta4 work on `console-parity` and merge afterwards, as you
suggested. The reason is hardware: the branch's code has never run against
a device, and after the protocol bump (phase B1) it can be checked against
the firmware you actually have. Merging first would put unverified V28 code
on `main` that no current device accepts.

Merging first remains possible at any time without conflicts, because
`main` has not moved. Merge first if you want `main` to carry the redesign
while the beta4 work happens elsewhere.

## 2. Decisions (settled 2026-09-29)

1. **Firmware versions:** exactly V32, as the Console. A beta2 or beta3
   device is refused with a message that names its firmware and says to
   update it.
2. **Tool keys:** the Console's mnemonics `S` (Subharmonic Synthesizer),
   `D` (Tube Modeller) and `A` (Spectrum Analyser), global like the other
   uppercase tool letters. The band list's Bypass All (`A`) and Clear All
   (`D`) move to the footer buttons and the `;` command bar; Test Signals
   keeps Space for Start and Stop and drops `S`.
3. **Spectrum:** the analyser engine and its own panel first. The overlay
   on the response graph and the RTA strip come later, off by default.
4. **Firmware update: out of scope.** No bundled images, no flashing
   command, no update wizard and no mismatch banner. The refusal message
   in decision 1 covers older firmware.
5. **On-graph editing: out of scope.** No mouse editing and no keyboard
   substitute (band multi-select and group gain offset are dropped).
   Drawing bypassed bands as grey ghosts is kept, because it is display,
   not editing.
6. **Limiter nouns 74 to 78:** included, although the Console lacks them.
7. **Also out of scope:** onboarding, What's New, the Help menu, window
   sizes and two-column layouts.
8. **Planning documents on `main`:** keep `PLAN.md`, `DESIGN.md`, this
   plan and the surveys; move `briefs/` and `audit.md` to
   `docs/plan/archive/` once the audit is annotated.

## 3. Phases

Each phase ends with `cargo fmt --check`, `cargo clippy --workspace
--all-targets -- -D warnings` and `cargo test --workspace` green, as
`PLAN.md` section 5 requires. The size column is a rough guide.

| Phase | Name | Size | Depends on | Hardware |
|---|---|---|---|---|
| B1 | Protocol bump to V32, 244 opcodes, CS caps v20 | M | | HW-1 |
| B2 | Device state, presets and notifications for the new sections | M | B1 | HW-1 |
| B3 | Small parity edits to existing screens | S | B1 | |
| B4 | Output limiter on the output page | M | B2 | HW-2b |
| B5 | Subharmonic Synthesizer panel | M | B2 | HW-2b |
| B6 | Tube Modeller panel | M | B2 | HW-2b |
| B7 | Control Surfaces caps v14 to v20 and Auxiliary Outputs | M | B1 | HW-2 |
| B8 | Spectrum analyser | L | B1 | HW-2b |
| B9 | Graph, band list, stats and dashboard refinements | M | B3 | |
| B10 | Documentation, carried-over defects, audit, merge | M | all | HW-3 |
| B11 | `.dspipreset` interoperability with the Console | M | B2 | |

B3 to B8 are independent of each other once B1 and B2 land, so they can run
in parallel worktrees.

### B1: Protocol bump (M)

Follow `docs/firmware-bump.md`, with the corrections noted below.

- Vendor the six current headers from `557bce7`, and add `limiter.h`,
  `tube.h`, `subharm.h`, `rta.h`, `rta_fft.h`, `rta_bass.h` and
  `notify.h`. `build.rs` reads only `#define` lines, so their includes and
  platform-conditional types do not matter. Vendoring `notify.h` closes an
  audit item: event ids and `Source` are hand-typed today
  (`dspi-session/src/notify.rs:31-99`). Update `PROVENANCE.toml`,
  `SHA256SUMS` and `HEADERS` in `build.rs`.
- Add `SUBHARM_`, `TUBE_`, `LIMITER_` and `RTA_` to `DEFINE_GROUPS`, and to
  `FLOAT_PREFIXES` where the values are floats.
- Append three entries to the section table in `build.rs`
  (`emit_wire_layout`, after `("upmix", 44)`): `subharm` 36, `tube` 48 and
  `limiter` 108. Pin 32 and 6136 in `lib.rs`, and add offset tests for all
  three sections at 5944, 5980 and 6028.
- Add registry rows for the 42 new opcodes. Tube parameters follow the
  upmixer's `Wv::Fixed(index)` pattern, with an f32 payload for every
  parameter. The limiter needs a new `WValue` variant for `(output << 8) |
  index`, packed in `dspi-session/src/write.rs`, and a way to address
  output `0xFF`. The coverage test should report 244 of 244.
- Codecs in `packets.rs`: the CS caps header is now 52 bytes (check that
  the tail is found at `4 + 4 * type_count`, not a fixed offset);
  `CsBinding.extras` at byte 22; `CS_UNIT_MS_LOG` (plain integer ms);
  `CS_TARGET_AUX`; status 0x26; the 48-byte aux state block; `RtaConfig`,
  `RtaCaps`, `RtaBandFrame`, `RtaStatus` and the bin-frame header; the
  subharm meter; the limiter meter and status.
- Read GET_PLATFORM with length 7 and decode the beta ordinal, using the
  ordering rule in `firmware_versioning_spec.md`. Read `REQ_GET_BUILD_INFO`
  (0x80) and show it in `dspi dump` and Stats, tolerating a STALL.
- Make the notification reader tolerate reads that block for about 100 ms
  now that the endpoint is paced, and decode `NOTIFY_EVT_CS_AUX` (0x0C).
- Fix `docs/firmware-bump.md`. It refers to `FieldDesc` rows that do not
  exist and lists only six headers.

**Exit:** tests green; coverage 244 of 244; `dspi --version` prints V32 and
244 opcodes; `dspi dump` on a beta4 device reports the right topology,
version "1.1.6 beta 4", and build info (HW-1).

### B2: State, presets and notifications (M)

- `DeviceState` structs, decoders and accessors for subharm, tube and the
  per-output limiter records, on the pattern of `Psybass` and `Upmix` in
  `dspi-session/src/state.rs`.
- Runtime state outside the bulk image: subharm solo (read with 0x2D, with
  no notifications, so poll it while the panel is open), subharm headroom
  (0x1A), aux state and levels.
- `PresetSnapshot::diff` blocks for Subharm and Tube, one line per field,
  and per-output limiter lines only when `output_config_mode` is
  WITH_PRESET. Exclude solo and headroom. In INDEPENDENT mode, limiter
  edits mark the output configuration unsaved instead, and Save Output
  Configuration (0x52) persists them.
- Feature probes in `probe.rs`: subharm (0x11, and 0x1C for the V30
  parts), tube (0x3F index 0), limiter (0x81 index 0x81) and RTA (0x0A).
- `.dspipreset` blocks matching the Console's `PresetDocument.swift`:
  top-level `subharm` and `tube`, and a `limiter` block inside each output
  entry. Import limiters only with the Hardware I/O option, and use the
  Console's skip reasons when the firmware lacks a feature. Test round
  trips against a file exported by the Console.
- Check that a PARAM_CHANGED offset the Terminal cannot patch in place
  triggers a re-read rather than being dropped. The Console changed to
  this behaviour.

**Exit:** state, diff and preset tests for all three sections; HW-1
extended with `dspi watch` showing a tube and a limiter change made from
the Console or a control surface.

### B3: Small parity edits (S)

- Rename Test Signals to **Signal Generator** everywhere: title, palette,
  help and the Control Surfaces noun label.
- PEQ pass labels become **HC / LC / HC1 / LC1**. Crossovers keep LP / HP,
  and filter files keep LP / HP / LP1 / HP1.
- Control Surfaces filter-type values become "High Cut" / "Low Cut".
- Matrix input rows use the sidebar channel names. Upmix rows stay C / Ls
  / Rs.
- Output routing names come from the sidebar channel names.
- Firmware version shows as "1.1.6 beta 4" in the title bar, About and
  Stats.
- Refuse a device whose wire version is not 32 with a message naming its
  firmware version and asking for an update (decision 1).
- With no device, the sidebar shows no channel rows and the graph shows no
  curves. Losing the device returns to the overview.
- Rebind the tool keys according to decision 2.

### B4: Output limiter (M)

On the output page, next to Mute, as the Console places it:

- A limiter indicator: off, on, and orange while gain reduction is at
  least 0.05 dB. Space or a key toggles it; Enter opens its settings.
- Settings: Threshold (-30..0 dBFS, step 0.5, default -1.0), Release
  (10..1000 ms, step 10, default 100), Link group (Off, 1 to 4) with the
  Console's summary text; settings dimmed while the limiter is off.
- Actions: "Copy to all outputs" (threshold, release and enable through
  output 0xFF, not the group), "Link all stereo pairs", "Unlink all
  outputs", and "Switch every limiter off".
- Poll the gain-reduction meter (0x81 index 0x80) only while the output
  page is showing and some limiter is on. Show a small gain-reduction
  reading on the output page, and consider a mark on the sidebar row.
- Do not mirror the firmware's ganging locally. The Terminal re-reads
  after every write (accepted deviation D37), which picks up the group's
  changes and avoids a second copy of the rules.
- Warn in the caption that turning on the first limiter adds 32 samples of
  latency to every output and fades all outputs for 8 ms.
- Page command grammar: `limit on`, `limit -3`, `release 200`, `link 1`.

### B5: Subharmonic Synthesizer panel (M)

A tool panel on the `psybass.rs` pattern, opened with `S`:

- Header with the master switch and **Solo**, and turn solo off when the
  panel closes, as the Console does.
- A BANDS graph (the three derived bands and the boost bell) with the four
  Console presets, and the HEADROOM COST reading.
- LEVELS: the three bands (-30..+12 dB, -30 shows "Off"), with the Console's
  "Derived from" captions.
- SELECTIVITY (All material / Percussive / Sustained, with Depth and Hold
  hidden in All), SUB CEILING, LF BOOST.
- OUTPUTS chips with the Sub only / All outputs / None presets, per-output
  sub meters polled while the panel is open and enabled, and Link output
  pairs.
- No V29 fallback is needed: decision 1 means every connected device has
  the V30 parts.

### B6: Tube Modeller panel (M)

A tool panel opened with `D`:

- Tube type menu grouped as in the Console, drive, mix, trim, bias,
  asymmetry, knee hardness, sag (dimmed on a solid-state rectifier),
  rectifier with its summary line, the output stage toggle with damping and
  resonance and their readout, and OUTPUTS with presets.
- Choosing a type loads that type's bias, asymmetry, hardness and sag, and
  editing one of those sets the type to Custom. Let the device do this and
  re-read, rather than copying the rules.
- The type and rectifier tables exist only in `tube.c`, not in a header.
  Transcribe them into one table with a test, and cite `tube.c:49-66`.
- Show one layout, not Basic / Advanced, because a terminal panel has room
  for every row. Include the TRANSFER CURVE and the 2nd/3rd harmonic
  reading only if the transfer function can be derived from `tube.c` with
  confidence. Otherwise leave them for later rather than approximating.
  `PLAN.md` already asks for curves to come from firmware source.

### B7: Control Surfaces caps v14 to v20 and Auxiliary Outputs (M)

- Nouns 57 to 78 in `packets.rs` and `settings/cs_model.rs`, with the
  Console's categories and names (Subharmonic Synth, Tube Modeller,
  Auxiliary Outputs), plus the limiter nouns under decision 6. Handle
  `CS_UNIT_MS_LOG` for Limiter Release, and the read-only Limiter GR noun.
- Target kind 5 (aux): the picker lists only aux slots, and only dimmable
  ones for Aux Level, labelled "Auxiliary Output(s)".
- A new Settings > Control > **Auxiliary Outputs** page, shown when caps
  are at least 18 and types 9 and 10 exist. Each card has Output (a live
  switch through 0x04), Level (dimmable, 0x06), GPIO pin, Active-Low,
  Level Limit, Linear Response, turn-on and turn-off delays, At Power-On
  (Fixed or As Last Saved, Starts On, Starting Level), and Driven By.
  Exclude aux types from the Control Surfaces add menu.
- Aux SETs are not deferred and never make the configuration dirty, so
  read status 0x87 after each one to report a failure.
- Apply `NOTIFY_EVT_CS_AUX` live.

**Exit:** HW-2 (below), with a relay or LED on a spare GPIO.

### B8: Spectrum analyser (L)

This is the largest item, and the only one that adds continuous USB
traffic.

- **Engine** in `dspi-session`: read caps and band centres; push the
  config only when it changes; read band frames with 0x0F for several
  channels or 0x0B for one; read bins for a single channel, comparing
  head and tail sequence bytes and discarding torn frames; treat a frame
  older than max(500 ms, 4 x refresh) as silence; send STOP when the last
  view closes; tolerate a STALL on 0x0F during a bulk transfer.
- **Rate:** the Console polls every 60 ms. Measure the Terminal's tick
  and USB latency first, and make sure analyser reads never delay a
  parameter write or the meters.
- **Panel** (`A`): Inputs / Outputs, channel selection, Curves / Bars /
  Both, and the status line (running or idle, frames per second, transform
  time, device load, and the "Device refused this configuration" and
  "shaded bands below N Hz" notices).
- **Settings > Display > Spectrum Analyser:** Strength, Peak Hold,
  Smoothing, Floor, Ceiling, Transform Size, Averaging and Peak Decay,
  with the Console's defaults. Store them in the Terminal's config file
  and re-push them on connect.
- **Later:** the overlay on the response graph (decision 3) and an RTA
  strip above the pages. Both come after the panel is stable.

### B9: Refinements to existing screens (M)

- Graph: draw bypassed bands as grey ghosts.
- Graph: check the Terminal's magnitude calculation for the Console's
  fix. The Console switched to the sin^2(w/2) form because narrow
  low-frequency peaks were drawn at 0 dB. If the Terminal has the same
  weakness, fix it and pin it with a test.
- Graphing settings: frequency and gain readout toggles. The Console's
  grid opacity maps to a grid on / dim / off choice.
- Stats: the Console's three-column arrangement where the width allows,
  buffer-fill history as sparklines, and the build info.
- Dashboard: cards per row (Auto, 1, 2, 3).
- Write traffic: check that holding an arrow key on a gain or frequency
  cannot flood the device. The Console now caps drags at 30 writes per
  second and commits on release.

### B11: `.dspipreset` interoperability (M)

Found during B2: the Terminal numbers channel entries by its own unified
index, while the Consoles use their own ids with `eqChannel`,
`inputIndex` and `outputIndex`, so EQ, gain and delay from a Console file
land on the wrong channels, and the reverse. The import also drops the
psybass, crossfeed and loudness masks and the upmix modes. Establish the
format from the Console source, write it down, and make files move
correctly in both directions on both platforms. Brief:
`archive/briefs/phase-b11.md`.

### B10: Documentation, open defects, audit and merge (M)

- **Docs:** annotate `audit.md` with each defect's resolution; update
  `PLAN.md` (exit condition 1's firmware pin, the stale `Targets::expand`
  criterion, the duplicated D41, the matrix rows in section 4 below);
  correct the README's "nothing is transcribed by hand" and "refuses to
  build" claims; refresh `docs/platforms.md`, `docs/wire-format.md` and
  `docs/firmware-notes.md` (add the gotchas from the firmware survey);
  remove the three em-dashes (`probe.rs:391`, `DESIGN.md:997`,
  `firmware-notes.md`); fix the stale `ConsoleScreens` doc comment
  (`live.rs:108-113`) and remove `PlaceholderScreens` if only a test uses
  it.
- **Open defects from the first audit:** fix or formally accept D36, D40,
  D57, D60 and D69 to D73. The golden-frame tests are still `contains`
  assertions rather than stored frames; either store frames or record the
  deviation in `DESIGN.md` section 11.
- **Hardware:** HW-1, HW-2, HW-2b and HW-3 on beta4 (section 4).
- **Audit:** the deep audits that `PLAN.md` phase 10 still owes (device
  state and notifications, Control Surfaces, theme and graph), plus the
  analyser engine and the limiter.
- **Merge:** open a pull request from `console-parity` to `main` so CI
  runs, then fast-forward or merge.

### Progress (2026-09-30)

Merged on `console-parity`: B1 (wire V32, 244 of 244 opcodes, CS caps
v20), B2 (subharm, tube and limiter state, presets and diff), B3 (Signal
Generator, cut labels, tool keys `S` `D` `A`, the non-V32 refusal and the
no-device state), B4 (the output limiter), B5 (the Subharmonic
Synthesizer), B6 (the Tube Modeller), B7 (caps v14 to v20 and Auxiliary
Outputs), B8 (the spectrum analyser engine, panel and settings) and B11
(`.dspipreset` interoperability with both Consoles), B9 (Graphing readout,
grid and dashboard choices, Stats columns and fill history, held-key write
coalescing, a pinned magnitude test, a 1-byte crossfeed mask) and B10
(`DESIGN.md` section 11 records the beta4 deviations, `archive/audit.md`
gives every defect its resolution, D36, D40, D57 and D72 fixed). 1227
tests pass. R5 is done; R4 (grey ghosts) is dropped, recorded in
`DESIGN.md` section 11.

Open: T5, the analyser overlay on the response graph and the RTA strip,
deferred by decision 3; audit D50 and the graph height half of D66; the deep audits `PLAN.md` phase 10 still owes, plus
the analyser engine and the limiter; and the pull request to `main`.

### Hardware results (2026-09-30)

Run against an RP2350 board, serial `D1443D6A`, firmware 1.1.6 beta 4
(build `v1.1.6-beta3-16-g557bce7`, 2026-09-28), with DSPi Console closed.
Every value changed was restored; nothing was written to flash.

- `dspi doctor` and `dspi dump`: interface claimed; 8 inputs and 9 outputs
  (17 channels, 10 bands); wire V32; the beta-aware version; build info;
  all 20 feature probes answered, including subharm, tube, limiter and
  the analyser; CS caps v20, 16 slots, 79 nouns, 16 IR commands.
- GET_PLATFORM returned 7 bytes `01 01 16 09 01 06 04`: 1.1.6 beta 4.
- Tube: `tube.type 3` brought back the 12AT7 row's bias, asymmetry,
  hardness and sag (5, +2, 55, 10); a bias edit set the type to Custom
  (0); type 1 restored the 12AX7 row. The slot held pre-beta4 defaults
  (drive -6, output stage off, 85 Hz), as the survey predicts for a slot
  restored verbatim.
- Limiter: linking outputs 1 and 2 in group 1 and writing output 1's
  threshold moved output 2's; `limit.release all 250` reached output 9.
  Switching output 1 on read the meter at 0 and status engaged, and off
  again restored not-engaged. An output that has never engaged reads
  12000 (120 dB) on the meter (limiter.c:217); the Terminal shows
  reduction only for outputs whose limiter is on, so it is not drawn.
- Analyser: caps v3, 120 dB range, 70 dB bass range, orders 8 to 10. Every
  analyser transfer (82-byte bands, 738-byte all-bands, 529-byte bins,
  24-byte status) costs well under a millisecond, lost in process-start
  noise against a 7-byte read, so the per-tick budget is ample. In the
  running interface (a pseudo-terminal, 120x40) `A` started it ("1
  channel, each refreshed every 21 ms"), the Console's default config was
  pushed and confirmed (output tap, 1024 points, 300 ms, 12 dB/s), and
  closing the panel sent STOP (state idle).
- The Tube Modeller and Spectrum Analyser panels render from the device's
  own values.

Found and fixed from these runs: `dspi get` printed "1 bytes" for the
runtime status rows, and `dspi params` ran long value lists into their
descriptions.

Still to run by hand, because they need a physical control, a second
host or wiring: notifications from a knob, remote, preset change or the OS
volume (`dspi watch`; this Mac's default output is not the DSPi); HW-2's
control surfaces with real buttons, an encoder, an LED, IR and a display,
and an aux output driven from a button; audible checks of the tube,
subharm and limiter; and an RP2040 board. Linux and Windows are covered
by CI on the pull request.

## 4. Hardware checkpoints (all on beta4)

On macOS, quit DSPi Console first, because the vendor interface is
exclusive.

- **HW-1** (after B1 and B2): the original HW-1 list from `PLAN.md`, plus
  version "1.1.6 beta 4", build info, a tube or limiter change seen live
  by `dspi watch`, and the refusal message on a beta2 or beta3 device if
  one is available.
- **HW-2** (after B7): the original HW-2 list, plus an aux output driven
  from a button and switched from the Terminal.
- **HW-2b** (after B4, B5, B6 and B8): limiter engage with gain reduction
  shown and a link group moving together; subharm with solo and meters;
  tube type change re-reading the four character values; analyser on
  input and output taps, one and several channels, with no stalls in
  parameter writes while it runs.
- **HW-3** (after B10): a full coverage walkthrough on RP2350 and, if
  available, RP2040 (meter sizes and mask bits differ).

## 5. Coverage matrix additions

These rows join `PLAN.md` section 4.

| # | Console | Terminal | Phase |
|---|---|---|---|
| W1 | Wire V32, GET_PLATFORM beta ordinal, build info | Protocol, title bar, Stats | B1 |
| W2 | Resync on unknown PARAM_CHANGED; `CS_AUX` event | Session | B1, B2 |
| W3 | Subharm, tube and limiter in snapshot diff and `.dspipreset` | Session | B2 |
| T1 | Subharmonic Synthesizer window | Tool panel `S` | B5 |
| T2 | Tube Modeller window | Tool panel `D` | B6 |
| T3 | Spectrum Analyser window | Tool panel `A` | B8 |
| T4 | Spectrum Analyser settings page | Settings > Display | B8 |
| T5 | FFT Graph overlay and RTA Bars strip | Graph, pages | B8 (later) |
| O1 | Output limiter icon, popover and "All outputs" menu | Output page | B4 |
| C6 | Auxiliary Outputs page | Settings > Control | B7 |
| C7 | Nouns 57 to 78, target kind 5, MS_LOG | Control Surfaces | B7 |
| R1 | Signal Generator rename; HC / LC labels; High Cut / Low Cut | Everywhere | B3 |
| R2 | Matrix and routing names from channel names | Matrix, output page | B3 |
| R3 | Refusal of non-V32 firmware; no-device empty state | Shell | B3 |
| R4 | Bypassed bands as grey ghosts | Graph | B9 |
| R5 | Stats three columns and buffer history; dashboard cards per row | Stats, overview | B9 |

Not carried over, with reasons: on-graph band editing (decision 5); the
Firmware Update window, bundled images and the mismatch banner (decision
4); onboarding, What's New and the Help menu (disabled or macOS-specific in
the Console); Basic / Advanced tube modes (a terminal panel shows every
row).
