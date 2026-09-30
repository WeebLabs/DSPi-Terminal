# Survey: DSPi Terminal current state (2026-08-27)

Produced by a read-only sweep of the workspace at commit 1a2ef21. Line
numbers refer to that commit. This is the baseline the phased plan in
`PLAN.md` works from.

## 1. Panels and overlays

Panel enum `crates/dspi-tui/src/app.rs:98-112`: Main, Input, Filters, Matrix,
Dynamics, Spatial, Surfaces, Presets, System, Raw. `Panel::group()`
(`app.rs:131-141`) maps only Input, Dynamics, Spatial, Presets, System,
Surfaces; Main, Filters, Matrix, Raw return None.

| Panel | Draw fn | Shows | Editable | Verdict |
|---|---|---|---|---|
| Main | `draw_dashboard` `app.rs:1391-1445` | graph + 3-line status box | nothing; `vol.*`, `meters.clear` never rendered | read-only |
| Input | `draw_fields` `app.rs:2184-2276` | registry rows | yes | works (8 rows Expert) |
| Filters | `draw_filters` `app.rs:1446-1483` | graph + band table, `=` split | PEQ bands only | works, PEQ only, no crossover bands |
| Matrix | `draw_matrix` `app.rs:2050-2183` | crosspoint grid + strip | read-only, reticle only | partial |
| Dynamics | `draw_fields` | 11 rows | yes | works |
| Spatial | `draw_fields` | 22 rows | yes | works |
| Surfaces | `draw_surfaces` `app.rs:1914-2049` | slot table | read-only; no write path | display-only |
| Presets | `draw_fields` | 3 rows Expert, 0 otherwise | PresetSlot targets filtered out | effectively empty |
| System | `draw_fields` | 12 rows Expert, 0 Advanced | yes at Expert | partial |
| Raw | `app.rs:1379-1389` | "Not built yet." | no | placeholder |

Structural gaps:
- `Targets::expand` (`fields.rs:130-135`) returns `vec![]` for every targeted
  parameter, so `preset.*`, `out.type`, `out.pin`, `in.spdif.*`, `in.i2s.pin`,
  `legacy.*` and all of Group::Matrix can never appear in a field panel.
- `nudge` (`fields.rs:142-186`) returns None for Kind::Trigger; every trigger
  row (dev.reset, dev.save, dev.bootloader, cs.save, cs.revert, cs.ir.learn,
  preset.save/load, diag.*.reset, vol.master.save) looks editable and is inert.
- `Group::Diagnostics` (11 rows: siggen, buffers, USB errors, core1, bulk) has
  no panel at all.
- Live-TUI wiring bug: `load_matrix` and `load_surfaces` are called only from
  `cmd_screenshot` (`main.rs:669-670`); `cmd_tui` (`main.rs:706-734`) calls
  only `load_bands`. In the interactive app Matrix always says "No routing
  read yet." and Surfaces says "This firmware reports no control surface
  support."
- Overlays: meter bridge `M` (`app.rs:2383-2411`) replaces whole body, no peak
  hold, no CPU rows, no CLEAR_CLIPS key. `G` toggles `graph_expanded` which is
  never read. Grid mode `m` works. `:` and Ctrl-P share `draw_input_overlay`
  (`app.rs:2451-2518`) and work well. Echo line works.
- Missing entirely: help overlay (`?`), device picker, undo consumer (journal
  is populated at `write.rs:381-388` and never read), legend (Curve::label is
  dead), frequency zoom.
- Advertised-but-unbound keys in Surfaces: n, l, s, r.
- No TODO/FIXME markers anywhere; gaps are absent code and prose strings.

## 2. Theme, layout, widgets

- `theme.rs`: ColorDepth detection from a captured env struct; Glyphs
  Braille/Blocks/Ascii; two palettes Amber (default) and Dark; 17 channel hues
  via `Theme::channel(i)`; Ansi16 amber gives only 2 channel colours; Mono has
  `needs_pattern_distinction()` with no caller.
- Layout `app.rs:1263-1327`: title / tabs / body [sidebar | main] / echo /
  keys. Minimum 60x12.
- `widgets.rs`: Bode (braille, focused curve coloured, others drawn dim, no
  legend), db_axis helpers, frequency_axis, Meter (no peak hold), InlineMeter.
  Everything else is hand-rolled Paragraph/Line in app.rs; only `draw_bands`
  uses ratatui Table. No reusable slider, form row, list, or picker.

## 3. Registry coverage

`registry.rs` has 127 rows referencing 190/190 vendored opcodes; enforced by
`tests/coverage.rs`. Prefixes: up.* 16, in.* 16, dev.* 11, cs.* 8, preset.* 7,
lev.* 7, bass.* 7, out.* 6, diag.* 6, cf.* 6, vol.* 5, i2s.* 5, eq.* 5,
loud.* 4, sig.* 3, adat.* 3, pre.* 2, meters(.clear) 2, legacy.* 2, ch.* 2,
bulk(.chunk) 2, mix 1, bypass 1.

Kind::Packet rows with no encoder anywhere (writable only where preset_file
builds Value::Bytes): mix, cs.binding, cs.ir, up.config, sig.config,
dev.dacmute, dev.uart, dev.i2c, bulk, bulk.chunk, preset.startup.
`dspi_cmd::parse_value` (`lib.rs:317-344`) has no Packet arm.

## 4. Session layer

- `probe.rs:107-169`: platform, serial, chunked bulk read (0xA2, always),
  header, channel map and names, band-count binary search, 15 feature probes,
  CS caps two-pass (`probe.rs:288-311`; caps_version decoded then never used),
  siggen caps, active preset.
- `write.rs:140-195`: lookup, availability, index check against discovered
  topology, validate, read-before, dispatch, confirm-by-readback for
  Deferred/Flash/Reconfig hazards, journal. EQ is read-modify-write of a
  16-byte band (`write.rs:284-311`). `read_matrix` decodes from one bulk
  snapshot. `core1_conflict` never called from UI.
- `surfaces.rs`: decoders and readers only. No write_binding, write_name,
  write_ir_command. IR learn arm/cancel/read exists. explain_status covers 30
  codes.
- `preset_file.rs`: schema v1 pinned to DSPi-Console-Windows@81ae00b; capture,
  write, apply; hardware I/O apply refused at `:511-514`.
- `filterfile.rs`: FORMAT_VERSION 2, PEQ + crossover banks, round-trips
  `tests/fixtures/DSPiFilters.txt`.
- `autoeq.rs`: bundled DB ~5 MB, user override; no online update.
- Notifications: NOT implemented. Transport trait has only control_in and
  control_out. VENDOR_EP_IN 0x83 is never read. Two comments (`app.rs:2606`,
  `registry.rs:2716`) wrongly claim otherwise.
- Metering: constant-rate poll of REQ_GET_STATUS at 20 Hz (10 Hz lite).

## 5. Tests

386 tests pass. dspi-tui 151 (golden frames via TestBackend), dspi-proto 92,
dspi-session 83, dspi-cmd 45, dspi-transport 10, coverage 5. `crates/dspi`
main.rs has 0 tests; `run()` is untested; no test checks key hints against
bound keys; no hardware smoke suite. Only macOS aarch64 has run on hardware.

## 6. CLI

Subcommands in `main.rs:47-77`: list, params, completions, doctor,
--install-udev, raw (IN only), autoeq search/apply, screenshot, export,
import, dump, and the shared grammar via cmd_run. Flags --device --json --lite
--theme --dry-run --quiet (last two undocumented). No batching / stdin.

## Ranked gap list

1. cmd_tui never loads matrix/surfaces.
2. No notification endpoint support.
3. Targets::expand drops every targeted parameter.
4. Trigger rows inert.
5. No control-surface write path.
6. Matrix read-only; mix has no CLI encoder.
7. Diagnostics group has no panel.
8. No crossover UI.
9. Main/Filters group params orphaned (vol, mute, bypass, preamp, delay, name).
10. No help, device picker, undo, legend, peak hold, clear-clips, freq zoom.
11. Graph dims non-focused curves.
12. Mono pattern fallback absent.
13. Preset hardware I/O apply refused; core1_conflict unused; no AutoEQ update.
14. main.rs untested; no batching; raw is IN-only.
15. Only macOS/aarch64 hardware-verified.
