# Console parity plan

*Written 2026-08-27. Owner of the plan and of every UX decision: Fable.
Implementation phases are assigned below.*

## 0. Goal and exit conditions

Bring DSPi Terminal in line with firmware `release/v1.1.6` and make every
feature that DSPi Console can configure configurable here, in a layout that
mirrors the Console's, in a terminal.

The work is done when all of the following hold:

1. **Firmware parity.** The vendored headers are `release/v1.1.6` @ `112f35b`
   (wire V28, 202 opcodes, CS caps v13), the coverage test reports 202/202,
   and `dspi dump` on a v1.1.6 device reports the right topology.
2. **Feature parity.** Every row of the coverage matrix in section 4 has a
   Terminal location and is checked off. The matrix is derived from
   `survey-console.md`, which lists every control in the Console.
3. **Layout parity.** The Terminal's shell is the Console's: a channel
   sidebar with meters and visibility pills, a graph with a legend, a detail
   region that switches between Overview, Input page and Output page, and
   the Console's tool windows and Settings pages reached with the same
   mnemonics the Console uses. `DESIGN.md` is the specification.
4. **Live parity.** Changes made on the device (knob, remote, another host,
   the OS volume slider) appear in the Terminal without a refresh, via the
   notification endpoint.
5. **Quality.** `cargo fmt --check`, `cargo clippy -D warnings`, and `cargo
   test --workspace` pass; every panel has golden-frame tests at 80x24 and
   120x40; every key hint on screen is a bound key (enforced by a test);
   the hardware checkpoints in section 3 have been run on a real device.
6. **Audit.** Each phase has been audited by Opus against its exit criteria;
   Fable has audited the three components named in section 5.

## 1. Baseline

Three surveys, all in this directory, are the factual basis:

- `survey-tui.md`: what the Terminal does today and where it is broken.
- `survey-firmware.md`: the complete host-visible surface of v1.1.6.
- `survey-console.md`: every screen and control in the Console.

The three biggest findings, because they shape the ordering:

- Bulk writes are strictly version-locked and the device is at V28, so
  nothing else is verifiable on hardware until the headers are bumped.
- Matrix and Surfaces are permanently empty in the live app (the loaders are
  only called from the screenshot path), no control-surface write path
  exists, and nothing reads the notification endpoint. The session layer
  needs real work before any screen can be built on it.
- `Targets::expand` returns nothing for every targeted parameter and every
  Trigger row is inert, so the "registry-driven field panel" approach that
  the old spec relied on never actually reached presets, pins, or triggers.
  The new screens are bespoke views over a typed device state, not generic
  field lists; the registry stays as the command grammar and write path.

## 2. Phases

| Phase | Name | Owner | Difficulty | Depends on | Hardware |
|---|---|---|---|---|---|
| 0 | Surveys, plan, design | Fable | | | |
| 1 | Firmware bump to v1.1.6 / V28 | Opus | Medium | 0 | checkpoint HW-1 |
| 2A | Device state model and notifications | Fable | High | 1 | HW-1 |
| 2B | Packet codecs and control-surface write path | Opus | High (well specified) | 1 | HW-1 |
| 2C | Session fixes: loaders, targets, triggers, undo, dirty snapshot | Opus | Medium | 1 | |
| 3 | Design system: theme, widget kit, shell, graph legend | Fable | High | 0 | |
| 4 | Core screens: sidebar, overview, input page, output page | Opus | Medium | 2A, 2C, 3 | |
| 5 | Matrix mixer | Opus | Medium | 2A, 2C, 3 | |
| 6 | DSP tool panels and test signals | Opus | Medium | 2A, 3 | |
| 7A | Settings: pages other than control surfaces | Opus | Medium | 2A, 2C, 3 | |
| 7B | Settings: control surfaces, groups, macros, display, IR learn | Opus | High | 2B, 3 | HW-2 |
| 8 | Stats, notification log, files, AutoEQ, tools menu, prompts | Opus | Medium | 2A, 3 | |
| 9 | Delight and density: animation, easing, 80x24, help, docs | Fable | Medium | 4-8 | |
| 10 | Audit | Opus, then Fable on three components | | 1-9 | HW-3 |

"Owner: Opus" means a general-purpose agent on the Opus model, working in a
git worktree from a brief written by Fable, with the survey files and
`DESIGN.md` as inputs. Fable reviews the diff of every Opus phase before it
merges, and audits three components in depth at the end (section 5).

The difficulty column is Fable's estimate of how much judgement the phase
needs, not how much typing. Phases assigned to Opus are ones where the
inputs fully determine the output: a header to transcribe, a struct layout
to encode, a Console screen to reproduce with a widget kit that already
exists. Phases kept by Fable are ones where the design is the work.

### Phase 1: Firmware bump (Opus, Medium)

Follow `docs/firmware-bump.md` exactly. Copy the six headers from
`/Users/weeblabs/DSPi` at `112f35b`; update `PROVENANCE.toml` and
`SHA256SUMS`; rebuild; let the tests report what moved; then:

- Add registry rows for the 12 new opcodes (`0x20`-`0x2B`). They are all
  `Kind::Packet` rows for now; Phase 2B gives them codecs.
- Update decoders whose sizes changed: CS caps header 40 to 44 bytes (index
  post-table fields at `4 + 4*type_count`, never a fixed offset), CS status
  22 to 41, SPDIF input config 5 to 6, preset directory 6 to 7 (it was
  already 7 on the wire; the old spec was wrong), IR sub-slots 8 to 16
  (read `max_ir_commands` from caps).
- Handle the V28 `WireInputConfig` shift: `spdif_rx_pin_ext` grows to 3 and
  every field after it moves down one byte. Section and packet size are
  unchanged, so this must be caught by offset tests, not size tests.
- Add `FilterType::LowPass1 = 12` and `HighPass1 = 13` with their response
  math (first-order TPT one-pole; derive from `crossover.c` and
  `first_order.h` in the firmware, not from a textbook, per spec section
  18), and add them to the filter-file grammar.
- Add `UPMIX_CENTER_OFF = 2`.
- Update `SPDIF` index ranges (`0..3`, enable `1..3`) in validators.
- Regenerate `docs/wire-format.md` from the new headers; record surprises in
  `docs/firmware-notes.md`.
- Update `REDESIGN_SPEC.md`'s pin table and `README.md`'s version line.

Exit: `cargo test --workspace` green; `tests/coverage.rs` reports 202/202
with an empty exclusion list (or reasons for any exclusion); a new test
pins the V28 `input_config` offsets; `dspi --version` prints V28 and 202
opcodes. Hardware: `dspi dump` on the device at HW-1.

### Phase 2A: Device state model and notifications (Fable, High)

The single typed model every screen reads from and every write updates.

- `DeviceState` in `dspi-session`: the decoded bulk packet (V28) plus
  everything the Console tracks outside it: preset directory and names,
  active slot, startup mode, output config mode, master volume mode, saved
  master volume, CS caps / bindings / names / IR / groups / macros / display
  / status, siggen caps / config / status, buffer stats, SPDIF / ADAT / I2S
  / LG status, control-interface config and status, core1 mode, pin
  ownership map, meters, clip latch, connection and device list.
- `Transport::bulk_in(endpoint, len, timeout)` on the trait, implemented in
  `usb.rs` with `nusb` and in `mock.rs` with a scripted event queue.
- A reader task on endpoint `0x83` (64-byte reads, sized by actual length),
  decoding the v2 events into a channel the app drains each tick.
  `PARAM_CHANGED` is applied by copying bytes into the local bulk shadow at
  `wire_offset` and re-decoding that section; `BULK_INVALIDATED`, a `seq`
  gap, or a `PRESET_LOADED` trigger a full chunked re-read; `INPUT_FORMAT`
  re-lays-out the channel list; `SIGGEN_STATE`, `ADAT_*`, `I2S_SLAVE_STATE`
  and `CS_IR_LEARN` update their sub-states. Source tags are kept so a
  screen can say "changed by GPIO".
- `PresetSnapshot` and `diff()` mirroring the Console's
  `unsaved_changes_spec.md`: captured on connect, preset load and preset
  save; `has_unsaved_changes` drives the `*` marker and the Save / Discard /
  Cancel prompt.
- The write path updates `DeviceState` on `Outcome::Accepted` so the UI
  never waits on a re-read for its own writes, and suppresses the echo of
  its own `HOST_SET` notifications by sequence.

Exit: mock-transport tests for every event type, for seq-gap recovery, for
the input-config shift, and for snapshot diff categories; a `dspi watch`
one-shot that prints decoded events (this is also the Interrupt Monitor's
data source). Hardware at HW-1: turn the OS volume slider and see
`user_volume` arrive with source `UAC1`.

### Phase 2B: Packet codecs and control-surface write path (Opus, High)

Typed structs with `encode` / `decode` and offset tests against the header
for every `Kind::Packet` row: `MatrixRoutePacket`, `CsBinding` (24),
`IrCommand` (16), `CsGroup` (40), `CsMacroStep` (12), `CsMacroHeaderWire`
(36), `CsMacro` (132), `CsDisplayCfg` (12), `CsDisplayPage` (4),
`CsDisplayStatus` (8), `CsExtStatusPacket` (24), `CsStatusPacket` (41),
`CsCapsHeader` (44 + variable), `CsNounDesc` (12), `UpmixConfigPacket` (44),
`UpmixStatus` (16), `SiggenConfig` (36), `SiggenStatus` (16),
`SiggenCapsHeader` (8), `SiggenTypeDesc` (62), `DacHwMuteConfig` (16),
`UartCtrlConfig` (8), `I2cCtrlConfig` (8), `CtrlIfaceStatus` (8),
`BufferStatsPacket` (44), `SpdifRxStatusPacket` (16),
`AdatInputStatusPacket` (20), `AdatStatus` (8), `I2sSlaveStatusPacket`
(16), `LgSoundSyncStatus` (16), preset directory (7), startup (2 / 3).

Then in `surfaces.rs`: `write_binding`, `write_name`, `write_ir_command`,
`write_group`, `write_macro` (steps first, header last), `write_display_cfg`,
`write_display_page`, `fire_macro`, and a `wait_applied()` that polls
`GET_CS_STATUS` until `PENDING` clears, retrying on `BUSY`, and returns the
decoded status code with `explain_status` extended to `0x1F`-`0x25`. Full
caps parse at v13 including per-noun descriptors, `actions == 0` meaning
unavailable, and the four post-table maxima. A pin-ownership map builder
that reads every pin-bearing setting and reports who owns each GPIO, with
the SPLIT-clock rule (both pairs claimed).

Also `dspi-cmd`: a `Packet` arm in `parse_value` so the `:` line can set
these via named fields (`:cs 3 encoder vol.user step 1 pins 10,11`), and
`:raw` gains an OUT form.

Exit: every codec has a size test and a field-offset test; a mock-transport
test writes a binding, sees PENDING then OK, reads it back; the CLI can
create a binding end to end against the mock. Hardware at HW-1: write and
read back one binding on the device; arm IR learn and press a remote.

### Phase 2C: Session fixes (Opus, Medium)

- `cmd_tui` loads matrix and surfaces (and everything else `DeviceState`
  needs) at connect, not only `cmd_screenshot`.
- `Targets::expand` expands every target kind from discovered topology.
- Trigger rows fire (Enter) with a confirm for `Hazard::Flash` and
  `Irreversible`.
- Undo consumer: `:undo` / `:redo` over the journal, live params only.
- `core1_conflict` is consulted before an output enable and surfaces the
  Console's two confirm dialogs.
- Preset apply of hardware I/O blocks is implemented (pins, types, clocks,
  ADAT, inputs) in the Console's order, with the "skipped" report.
- Batching: `dspi -f script` and stdin.

Exit: tests for each; the live app shows the matrix and surfaces on the
mock.

### Phase 3: Design system (Fable, High)

Specified in `DESIGN.md`. Delivers `theme.rs` (Console palette, semantic
colours, 256-colour and 16-colour quantisation, mono patterns), `widgets/`
(section header, form row with slider and value field, toggle row, picker
row with popup list, chip row with presets menu, status pill, banner, card,
table, dialog kit: confirm / text input / list picker / progress, save bar,
help overlay, legend pills, meter with peak hold and clip zone, CPU meter,
pin grid), the shell layout (sidebar + graph + detail, and full-pane tool
panels and Settings), the navigation model and key map, and the graph
legend with identical-curve grouping.

Exit: `examples/gallery.rs` renders every widget in every colour depth;
golden-frame tests; `DESIGN.md` section 8 checklists satisfied.

### Phase 4: Core screens (Opus, Medium)

Sidebar (INPUTS / OUTPUTS rows with meter, clip zone, visibility pill,
selection bar, rename, copy / paste, identify, linked-pair highlight; icon
strip; Preset row with every context action; Source row; Volume section
with both modes and both tapers; CPU section), connection indicator and
device picker, Overview cards, Input page (link pill with mismatch prompt,
preamp, clear PEQ, filter list with every type incl. first-order and the
Linkwitz popover), Output page (routing panel, gain, delay, mute, PEQ / XO
tabs, bypass-all with the tweeter warning, clear all).

### Phase 5: Matrix mixer (Opus, Medium)

Full grid with crosspoint enable / gain / INV, input trim in 8-channel mode,
Direct 1:1 and Clear, output rows ENABLE / GAIN / DELAY / MUTE, conflict
outline and the two confirm dialogs, column rename / identify / copy /
paste.

### Phase 6: DSP tool panels and test signals (Opus, Medium)

Crossfeed, Loudness, Leveller, Psybass, Upmixer, each with its header
toggle, its graph (crossfeed curve, ISO 226 curve, psybass spectrum,
upmixer live gauges), chips with presets menu, parameters with captions,
and its unsupported banner. Test Signals with the 16-tile grid, outputs with
polarity, level, per-type params, timing by model, options, transport with
Space, and start blockers.

### Phase 7A: Settings, general pages (Opus, Medium)

Settings shell with grouped sidebar, availability gating, save bar with the
three dirty categories, Revert. Pages: Overview pin map, About, Graphing
(app settings persisted to the config file), Advanced, Global Parameters
(staged draft), Outputs, Inputs, I2S Configuration, Control Interfaces.

### Phase 7B: Settings, control surfaces (Opus, High)

Control Surfaces page with caps-driven type / noun / action / event / target
/ index / pins / operands / delays / flags per slot, Apply / Revert per
slot, status pills and reasons; IR remote sub-section with learn loop;
Display sub-section with wiring, config, alignment, flags, pages; Channel
Groups page; Macros page. Fable audits this phase in depth.

### Phase 8: Stats, log, files, AutoEQ, tools, prompts (Opus, Medium)

Stats panel (2 s refresh, every section), Notification log panel (Interrupt
Monitor), Import / Export Filters with REW, DSPi and Windows dialects and
the channel pickers, Import / Export Device Configuration with the options
prompt, progress and result report, AutoEQ browser with favourites and
Update Database (GitHub rebuild, import file, reset), Tools actions (commit,
revert, factory reset, firmware update with the bootloader handoff and
`RPI-RP2` detection), Save Master Volume and Save Output Configuration,
the unsaved-changes prompt on preset switch / device switch / quit, channel
clipboard.

### Phase 9: Delight and density (Fable, Medium)

Connect animation, remote-change easing, 80x24 single-column mode, 200x60
wide mode, `?` help overlay per screen, README and screenshots, `--lite`
verification, mono and 16-colour passes.

### Phase 10: Audit

Opus audits every phase against its exit criteria and the coverage matrix,
producing `docs/plan/audit.md`. Fable then audits: (1) `DeviceState` and
the notification path, (2) the Control Surfaces pages, (3) the theme, graph
and legend. Then HW-3.

### Progress (2026-08-30)

Merged on `console-parity`: every implementation phase, 0 through 9, and
the Opus audit (`audit.md`, 81 defects). Fixed by Fable: D1, D2, D2b, D5,
D6, D6b, D8, D11, D12, D14, D15, D16, D17, D18, D20, D32, D33, D34, D39,
D41, D43, D44, D56, D75, D77, D78. Both Opus fix rounds are merged
(Settings; screens and panels). The quiet redesign of `DESIGN.md` section
12 landed on 2026-08-30 (calm palette, one curve per graph, the overview
grid), and the sidebar, matrix and page-command-bar rounds that followed
(DESIGN 13), 972 tests. Waiting: HW-1 and HW-2 (a device), then Fable's
three deep audits and HW-3.

## 3. Hardware checkpoints

The Terminal will pause and ask for a device at these points. Everything
between them runs against the mock transport.

- **HW-1** (after 1, 2A, 2B, 2C): `dspi doctor`, `dspi dump` reports V28
  and 1.1.6; `dspi watch` shows `user_volume` from the OS slider and a
  `PRESET_LOADED` from `:preset load`; write and read back a CS binding;
  arm IR learn. On macOS, quit DSPi Console first; the vendor interface is
  exclusive.
- **HW-2** (after 7B): the Control Surfaces pages against a device with a
  button, an encoder, an LED, an IR receiver and, if available, an I2C
  display; macro fire; display page cycling.
- **HW-3** (after 10): a full walkthrough of the coverage matrix on RP2350
  and, if available, RP2040; firmware update handoff; Windows and Linux
  `dspi doctor` if machines are available.

## 4. Coverage matrix

Console item (from `survey-console.md`) to Terminal location and phase.
Tick when merged and golden-tested.

| # | Console | Terminal | Phase | Done |
|---|---|---|---|---|
| S1 | Sidebar INPUTS rows: meter, clip, pill, rename, copy/paste | Sidebar | 4| yes |
| S2 | Sidebar OUTPUTS rows + identify + muted dimming | Sidebar | 4| yes |
| S3 | Quick-access strip (8 actions) | Sidebar footer key row | 4| yes |
| S4 | Preset row: pick, `*` dirty, save/rename/default/copy-to/clear/clear-all | Sidebar | 4, 8| yes |
| S5 | Source row | Sidebar | 4| yes |
| S6 | Volume: user (sqrt taper) and master (piecewise taper), mode menu, reset | Sidebar | 4| yes |
| S7 | CPU C0/C1 red > 90 % | Sidebar | 4| yes |
| S8 | Connection dot, "No Devices", device picker, reconnect | Title bar | 4| yes |
| D1 | Overview cards (stereo gradient, delay, 10 rows, type codes) | Detail: Overview | 4| yes |
| I1 | Input page: link pill + mismatch prompt, preamp, clear PEQ | Detail: Input | 4| yes |
| I2 | Filter list: all PEQ types, bypass, ValueField semantics, header/footer | Detail | 4| yes |
| I3 | Linkwitz Transform popover with DC boost warning, staged Apply | Detail | 4| yes |
| O1 | Output page: routing panel, gain, delay (42/85), mute | Detail: Output | 4| yes |
| O2 | PEQ / XO tabs, crossover rows (family / type / slope / freq) | Detail | 4| yes |
| O3 | Enable All / Bypass All (tweeter warning), Clear All | Detail | 4| yes |
| G1 | Graph: grid, labels, adaptive dB step, phase overlay, zoom, height | Graph | 3| yes |
| G2 | Legend pills, identical-curve grouping, dashed unselected | Graph | 3| yes |
| G3 | Graph pop-out with independent visibility | Graph panel `g` | 3| yes |
| M1 | Matrix: crosspoints, INV, trim, Direct 1:1, Clear | Matrix panel | 5| yes |
| M2 | Matrix output rows ENABLE / GAIN / DELAY / MUTE, conflicts | Matrix panel | 5| yes |
| X1 | Crossfeed panel | Tool panel | 6| yes |
| X2 | Loudness panel | Tool panel | 6| yes |
| X3 | Leveller panel | Tool panel | 6| yes |
| X4 | Psybass panel with starting points | Tool panel | 6| yes |
| X5 | Upmixer panel with live gauges | Tool panel | 6| yes |
| T1 | Test Signals: tiles, outputs/polarity, level, params, timing, options, transport | Tool panel | 6| yes |
| N1 | Stats for Nerbs, every section, 2 s refresh, reset watermarks | Tool panel | 8| yes |
| N2 | Interrupt Monitor: pause, clear, decoded log | Tool panel | 2A, 8| yes |
| A1 | AutoEQ browser: search, source capsule, favourites, apply | Tool panel | 8| yes |
| A2 | AutoEQ favourites menu, Update Database (3 methods) | Palette / panel | 8| yes |
| F1 | Import / Export Filters (REW, DSPi, Windows dialect), channel pickers | `:import` `:export` + dialogs | 8| yes |
| F2 | Import / Export Device Configuration with options and report | same | 8| yes |
| F3 | Save Master Volume, Save Output Configuration | Tools | 8| yes |
| K1 | Commit, Revert to Saved, Factory Reset, Firmware Update (bootloader) | Tools | 8| yes |
| K2 | Unsaved-changes prompt on preset switch / device switch / quit | Everywhere | 2A, 8| yes |
| K3 | Channel clipboard | Sidebar, Matrix | 4, 5| yes |
| P1 | Settings shell: groups, gating, back/forward, save bar, revert | Settings | 7A| yes |
| P2 | Overview pin map with roles | Settings | 7A| yes |
| P3 | About | Settings | 7A| yes |
| P4 | Graphing settings (persisted) | Settings | 7A| yes |
| P5 | Advanced: reset names, debug info | Settings | 7A| yes |
| P6 | Global Parameters: startup, DAC mute (+test), master mode, hw mode | Settings | 7A| yes |
| P7 | Outputs: slots type/pin, Default capsule, ADAT out, reset pins | Settings | 7A| yes |
| P8 | Inputs: SPDIF instances/pins, LG sync, I2S clock/channels/pins, ADAT in | Settings | 7A| yes |
| P9 | I2S Configuration: BCK, clock pins, slave BCK, MCK, multiplier, rate | Settings | 7A| yes |
| P10 | Control Interfaces: UART, I2C, apply/revert, status pills | Settings | 7A| yes |
| C1 | Control Surfaces slots: full caps-driven editor | Settings | 7B| yes |
| C2 | IR remote commands with learn | Settings | 7B| yes |
| C3 | Display wiring, config, pages | Settings | 7B| yes |
| C4 | Channel Groups | Settings | 7B| yes |
| C5 | Macros | Settings | 7B| yes |
| L1 | Notifications applied live (EQ, names, volume, source, and all of V28) | Session | 2A| yes |
| L2 | Multi-device: list, switch with prompt, generation scoping | Session | 2A, 4| yes |
| V1 | Console palette; a hue on the swatch and the selection (DESIGN 12), the Console's full colouring as `--theme console` | Theme | 3, 12| yes |
| V2 | Disabled / unsupported presentation rules | Widgets | 3| yes |
| Q1 | 80x24 and 200x60 layouts, help overlay, animation, `--lite` | Shell | 9| yes |

Not carried over, with reasons: the `STM32H723` platform (a separate
firmware repository not in scope; the platform id is decoded generically so
it will not crash); macOS-only menu items (About panel links are shown in
Settings > About instead); window sizes.

## 5. Working agreements

- Branch `console-parity` off `main`. Each phase merges as one or more
  commits with the phase number in the subject. No pushes without asking.
- Opus phases run in a worktree; the brief names the exact files, the exit
  criteria, and the tests to write. A phase is not done until `cargo fmt
  --check`, `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo test --workspace` are green.
- No em-dashes anywhere in code, UI strings, or docs (DSPi convention).
- Every protocol fact cites a firmware header line. `dspictl` is not a
  source.
- Every screen gets golden-frame tests at 80x24 and 120x40 through
  `render_to_string`, and a key-hint test that every key named in the
  footer is bound.
- UI strings follow the Console's wording verbatim where the Console has a
  string; captions and warnings are copied, not paraphrased.
- Nothing about device shape is compiled in; channel counts, band counts,
  caps and feature presence come from `DeviceState`.
