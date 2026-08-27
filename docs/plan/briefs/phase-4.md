# Phase 4 brief: core screens

You are implementing **Phase 4: Core screens** of the Console-parity plan, in
a git worktree of the DSPi Terminal repo (a Rust workspace). Read, in order:

1. `docs/plan/PLAN.md` (your phase is "Phase 4"; section 5 "Working
   agreements" binds you).
2. `docs/plan/DESIGN.md` in full. Sections 2, 5, 6 and 7.1 to 7.6 are your
   specification; the widget kit in section 6 already exists.
3. `docs/plan/survey-console.md` sections 2.1 to 2.12 (every control on the
   screens you are building, with the Console's exact strings) and section
   5 (visual rules).
4. The code you build on: `crates/dspi-tui/src/shell/{mod.rs,screen.rs,
   model.rs,fixture.rs}` (the shell, the `Screen` trait, the fixtures),
   `crates/dspi-tui/src/live.rs` (the runner and the `Screens` factory),
   `crates/dspi-tui/src/widgets/` (the kit; read every file's doc comment
   and tests), `crates/dspi-tui/src/graph.rs`, `crates/dspi-tui/src/theme.rs`,
   `crates/dspi-session/src/state.rs` (`DeviceState`, the typed views you
   read from), `crates/dspi-session/src/write.rs` (`Session::write`,
   `write_band`, `read_band`), `crates/dspi-proto/src/{registry.rs,dsp.rs,
   enums.rs,value.rs}` (parameter paths, filter types, the `EqParamPacket`).
5. The Console for behaviour, read-only, never copied:
   `/Users/weeblabs/DSPi Console/DSPi Console/{ContentView.swift,
   Components.swift,DashboardView.swift,DSPMath.swift}`.

Run `cargo run -q -p dspi-tui --example gallery -- 120 40` to see the shell
as it stands. Your screens fill its detail region.

## File ownership

You own and may create or edit ONLY:
- `crates/dspi-tui/src/screens/` (new): `mod.rs`, `overview.rs`,
  `input.rs`, `output.rs`, `filters.rs`, `linkwitz.rs`, `clipboard.rs`,
  `presets.rs`, plus `pub mod screens;` in `crates/dspi-tui/src/lib.rs`.
- `crates/dspi-tui/src/live.rs`: only (a) a `ConsoleScreens` factory
  replacing `PlaceholderScreens` as the default, (b) the preset row's
  context actions and the copy / paste / rename handlers, (c) anything the
  brief below names. Keep the diff to those.
- `crates/dspi-tui/src/widgets/`: you may ADD a widget file if a screen
  genuinely needs one the kit lacks, with tests in the kit's style. Do not
  change existing widgets' behaviour; if one is wrong, say so in the report.
- `crates/dspi-tui/examples/gallery.rs`: extend it to render your screens.

Do NOT edit `shell/`, `graph.rs`, `theme.rs`, `app.rs`, `fields.rs`, or
anything outside `crates/dspi-tui`. Another agent owns `crates/dspi/src/
main.rs` right now; the reviewer switches the binary to the live runner.

## What to build

Every screen implements `shell::Screen`: it draws from `&DeviceState` each
frame (never caching device values), keeps only its own cursor and edit
state, and asks for writes with `ScreenEvent::Command("<shared grammar>")`
so the echo line shows exactly what a person could type. Use registry paths
from `cargo run -q --bin dspi -- params` (for example `pre <input>`,
`out.gain <o>`, `out.delay <o>`, `out.mute <o>`, `out.enable <o>`,
`ch.name <ch>`, `eq.type <ch> <band>`, `eq.freq`, `eq.q`, `eq.gain`,
`eq.bypass`, `mix <in> <out> ...`) and the `eq` band command
(`eq in.1 3 peak 2856 3.58 -8.6`, see `dspi-cmd`). When a write needs a
form the grammar lacks, add it to `dspi-cmd` only if trivial and say so;
otherwise report it.

### 1. `screens/overview.rs`: the Console's dashboard (survey 2.9)

Stacked cards in a scrolling column: a stereo `Card` for IN1 / IN2 titled
`STEREO INPUT (USB)`, a stereo card per enabled S/PDIF output pair (single
when only one of the pair is enabled), a single card for the PDM output when
enabled. Card header: the channel name and `Delay: N ms` from
`state.delay_ms` / `state.output(o).delay_ms`. Body: ten rows `#`, type
code (`OFF PK LS HS LP HP NO AP LS1 HS1 LP1 HP1 AP1 LT` and crossover short
labels; DESIGN 5), `freq Hz`, `+-gain dB` only for types with gain, `Q` only
for peaking, `-` for inactive; the type code in the channel colour when
active. `Enter` on a card selects that channel (`ScreenEvent::Command` is
not right for that; add a `ScreenEvent::Select(Selection)` variant to
`screen.rs`? No: you do not own `shell/`. Instead the overview returns
`ScreenEvent::Status` for now and the reviewer wires selection; note it in
the report). `↑`/`↓` move between cards, `PgUp`/`PgDn` scroll. At Roomy and
Wide densities two cards sit abreast (DESIGN 8).

### 2. `screens/input.rs`: the input page (survey 2.10)

Header row (DESIGN 7.4): the `Link n/m` pill (green when linked, dim when
not, hidden when the partner input is not live; the link state is
application-side like the Console's `linkedInputPairs`, keep it in the
screen factory's shared state), `Preamp` as a compact `ParamRow`
(-60..+10 dB, step 0.5, `pre <input>`, Backspace resets to 0), and
`Clear PEQ` (`Clear 1/2 PEQ` when linked) which confirms with the Console's
"Clear All Bands?" and then writes every band Off. Below it the filter list
in PEQ mode with Linkwitz hidden. Toggling the link when the two inputs'
bands or preamp differ raises the Console's "Inputs N and M don't match"
dialog with `Keep INn` / `Keep INm` / `Cancel`, copying the kept side to the
other. While linked, every write mirrors to the partner.

### 3. `screens/output.rs`: the output page (survey 2.11)

Header rows (DESIGN 7.5): the routing panel (one line per input up to the
base input count: `●`/`○` connect, the crosspoint gain as an editable value
with 0.5 dB nudges, `INV` in warning colour when inverted; all via `mix`),
then `GAIN` (-60..+10 dB), `DELAY` (0..42 ms on RP2040, 0..85 otherwise;
use `state.caps.platform`), `MUTE` (danger reverse when muted). Then the
`PEQ │ XO` tab strip, shown only when `state.caps` reports crossover
support (V11+ is always true at V28, but gate it anyway), and the filter
list in the chosen mode.

### 4. `screens/filters.rs`: the filter list (survey 2.12, DESIGN 7.6)

A `Table` with the Console's columns. PEQ mode: `#`, `TYPE` (the
hierarchical type picker as a `PopupList` with group headers: Off, Peaking,
Low Shelf > 6 dB/oct and 12 dB/oct, High Shelf > ..., High Cut > 6/12
(that is LP1 / LP), Low Cut > 6/12 (HP1 / HP), Notch, All Pass > 180 / 360,
Linkwitz Transform; gate each by `FilterType` availability as the Console's
`availableFilterTypes` does), `FREQ` (10 Hz minimum, log nudges by the
ratio 2^(1/12), scroll step 10), `GAIN` (hidden for gainless types; 1 dp
shown, 3 dp typed), `WIDTH` (Q, 3 dp with trailing zeros stripped, 0.1
minimum, hidden for crossover and first-order types). The bypass disc `●`
armed / `○` bypassed / blank for Off, hidden when bypass is unsupported.
XO mode: `#`, `FAMILY` (Off / Linkwitz-Riley / Butterworth / Bessel),
`TYPE` (LP / HP), `SLOPE` (only the orders the family allows, shown as
order x 6 dB/oct), `FREQ`; encode with `dspi_proto::xover`. Keys: `↑`/`↓`
band, `←`/`→` field, `Enter` arm and commit typed entry (`NumberEdit`),
`Space` bypass, `1`-`9`,`0` jump to band, `a` Enable All, `A` Bypass All
(XO mode confirms with the Console's tweeter warning, verbatim), `D` Clear
All (confirm "Clear All Bands?"), `x` toggle PEQ / XO. The footer actions
appear on the key line through `Screen::keys`. Writes: a band edit is one
`eq ...` command (the whole band, since the firmware writes bands whole).

### 5. `screens/linkwitz.rs`: the Linkwitz Transform dialog (survey 2.12)

Opened from the `⚙` on a Linkwitz row. A `Dialog` variant is not enough;
draw your own centred box with Driver `f0` / `Q0` and Target `fp` / `Qp`
fields, the DC boost readout `40*log10(f0/fp)` in warning colour above
15 dB with a `▲`, and `Revert` / `Apply`. Edits are staged; `Apply` writes
the 18-byte band form (`qp` x512) through `Session::write_band` via a
command the grammar can express, or report that it cannot.

### 6. `screens/clipboard.rs` and `screens/presets.rs`

Channel clipboard: copy the selected channel's bands, crossover bands,
preamp or gain / delay / mute, paste onto another (the Console's
`copyChannelParams` / `pasteChannelParams`), from `y` / `Y` in the sidebar
(the `ShellEvent::CopyParams` / `PasteParams` events already exist; handle
them in `live.rs`). Preset row actions in the `Preset(None)` popup: the ten
slots, then a divider and `Save`, `Rename...`, `Set as Default`, `Copy
to...`, `Clear "<slot>"...`, `Clear All Slots...` with the Console's
confirmations and error strings (survey 2.4). Preset names come from
`preset.name <slot>` reads (cache them in the factory's shared state and
refresh after a rename); the active slot shows `*` when
`state.has_unsaved_changes()`.

### 7. Tests

Golden-frame tests for every screen at 80x24 and 120x40 through a `Shell`
built from `shell::fixture::rp2350` and `shell::fixture::state`, in the
style of `shell/mod.rs`'s tests (`frame()`), asserting the Console's strings
are on screen. Key tests asserting that arrows, Enter, Space and the letter
keys produce the expected `ScreenEvent::Command` text. A test that every
key in each screen's `keys()` is handled (see `every_advertised_key_is_
handled` in `shell/mod.rs` for the pattern). Extend `examples/gallery.rs`
with a `--screen overview|input|output` argument.

## Rules

- The Console's strings verbatim; no em-dashes anywhere.
- Nothing about device shape compiled in: channel counts, band counts,
  filter-type availability and platform come from `DeviceState`.
- Finish with `cargo fmt --all`, `cargo clippy --workspace --all-targets --
  -D warnings`, `cargo test --workspace` green. Commit per numbered section
  with subjects starting `Phase 4:`. Do not push. No `Co-Authored-By` or
  `Claude-Session` trailers.

## Report

Worktree path, branch, commits; every file added; every place the widget
kit or the shell lacked something (with what you did about it); every
Console behaviour you could not reproduce and why; the gallery commands
that show each screen; test counts before and after.
