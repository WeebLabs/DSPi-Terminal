# DSPi Terminal: interface design

*Revision 1, 2026-08-27. This document is the specification for Phase 3 and
the input to Phases 4 through 9. Where it disagrees with `REDESIGN_SPEC.md`
sections 6, 7 and 9, this document wins; those sections are superseded.*

The Console is the reference. When this document is silent, do what the
Console does (see `survey-console.md` for what that is), translated with
the conventions in sections 4 and 5.

## 1. Principles

1. **Same shape as the Console.** A channel sidebar on the left with
   meters and visibility pills; a response graph on the top right with a
   legend; a detail region beneath it that shows the overview, an input, or
   an output. Tool windows become full-pane panels; Settings becomes a
   full-screen page with its own sidebar. Someone who knows the Console
   should find everything where they expect it.
2. **Same words as the Console.** Titles, captions, warnings, preset names,
   type codes and button labels are the Console's strings, copied not
   paraphrased. The surveys carry them.
3. **Same colours as the Console.** Each channel owns a hue that follows it
   everywhere: pill, meter, curve, matrix label, card border, type code.
   Semantic colours are reserved: red for mute, clip, danger; orange for
   warning, phase invert, unsaved; green for live and confirmed; blue for
   output enable and the focus accent.
4. **Data is bright, chrome is dim.** Borders, section headers and captions
   sit low-contrast; values, curves, meters and pills carry the colour.
5. **The terminal's background is the background.** We never paint a page
   background. Filled colour is used only for pills, chips, selection bars,
   status pills and the focus highlight, which is how the Console uses
   fills too.
6. **Everything is reachable three ways.** Navigate with arrows and
   mnemonic keys; search with `Ctrl-P`; type with `:`. Every change echoes
   its command on the echo line.
7. **Motion is functional.** Meters and gauges move because they are live;
   a value that changes from the device eases over 120 ms so a knob turn is
   visible; the curve draws itself on connect. Nothing else animates.
8. **Absent features are removed, not disabled.** Present-but-too-old
   features show the Console's banner. Disconnected controls dim.

## 2. The shell

### 2.1 At 120x40 (the reference layout)

```
 DSPi  RP2350 · fw 1.1.6 · A1B2C3D4                         ● Connected   Preset 3: Living Room *
┌ INPUTS ─────────────────┐┌ Filter Response ────────────────────────────────────────────────── ⤢ ┐
│▍FL        ▓▓▓▓▓▓▓░░░▏IN1││ +15                                                                   │
│ FR        ▓▓▓▓▓▓░░░░▏IN2││                                                                       │
│ FC        ░░░░░░░░░░▏IN3││  +5           ⢀⡠⠤⠤⠤⢄⡀                                                  │
│ LFE       ░░░░░░░░░░▏IN4││   0 ────────⠔⠁──────⠈⠢⡀────────────────⢀⡠⠤⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒⠒│
│ OUTPUTS                 ││  -5                   ⠈⠢⡀           ⢀⡠⠊                             │
│ OUT L     ▓▓▓▓▓▓▓░░░▏OUT1││ -15                     ⠈⠑⠒⠒⠒⠒⠒⠒⠒⠊                                  │
│ OUT R     ▓▓▓▓▓▓▓░░░▏OUT2││      20        100        500   1k         5k      10k     20k     │
│ Sub       ▓▓▓▓▓▓▓▓▓█▏OUT9││ ● IN1  ● IN2  ○ IN3  ○ IN4  ● OUT1  ● OUT2  ● OUT9   φ phase        │
│                         │└───────────────────────────────────────────────────────────────────────┘
│                         │┌ ● FL ───────────────────────────────────────────────────────────────┐
│                         ││ Link 1/2 ⛓   Preamp  -5.3 dB ━━━━━━━━━━●━━━━━━━━━━━━━━━   Clear PEQ  │
│                         ││─────────────────────────────────────────────────────────────────────│
│                         ││    #  TYPE                 FREQ        GAIN        WIDTH            │
│                         ││ ●  1  Low Shelf 12 dB/oct   105 Hz     +8.8 dB     0.707            │
│                         ││ ●  2  Peaking                64 Hz     -6.2 dB     0.300            │
│                         ││ ●▸ 3  Peaking             [2856 Hz]    -8.6 dB     3.580            │
│                         ││ ○  4  Peaking              1880 Hz     +3.6 dB     1.690            │
│─────────────────────────││    5  Off                                                           │
│ M Matrix  X Xfeed ● L Loud ○ V Lev ○  P Bass ●  T Stats  , Settings  b Bypass ○                 │
│ Preset    ‹ 3: Living Room * ›                                                                  │
│ Source    ‹ USB ›                                                                               │
│ Volume    User  -12.0 dB  ━━━━━━━━━━━━━━━━━━●━━━━━━                                             │
│ C0 ▓▓▓░░░░░ 31%   C1 ▓▓▓▓▓▓░░ 74%                                                               │
└─────────────────────────┘└─────────────────────────────────────────────────────────────────────┘
 :eq in.1 3 freq 2856                                                    Enable All│Bypass All  Clear
 ↑↓ band · ←→ field · Enter edit · Space bypass · Tab region · ? help · : command · ^P search
```

(The footer strip in the sidebar is shown flattened above for space; see
2.3 for its real shape.)

Regions, top to bottom and left to right:

- **Title bar** (1 row): app name, platform, firmware, serial short; on the
  right the connection indicator and the active preset with the `*` dirty
  marker. The connection indicator is focusable (`Ctrl-D`) and opens the
  device picker when more than one device is present.
- **Sidebar** (24 columns; 22 at 80 columns): INPUTS and OUTPUTS sections,
  then the footer block.
- **Graph** (top of the detail pane): the Bode plot with axes, then one
  legend row of pills.
- **Detail** (rest of the detail pane): Overview, Input page or Output
  page, switched by the sidebar selection. Tool panels replace the whole
  detail pane including the graph.
- **Echo line** (1 row): the canonical command of the last change, dimmed;
  status messages override it for 3 s. On the right, contextual actions for
  the focused region.
- **Key line** (1 row): the keys that matter for the focused region.

### 2.2 At 80x24 (the minimum)

Same two columns. The sidebar is 22 wide, the graph is 7 rows tall and
`=` collapses it to a single legend row, the detail gets the rest. Below
80x24 the app draws "Terminal too small: needs 80x24" and nothing else.

### 2.3 The sidebar footer

Mirrors the Console's bottom inset, top to bottom:

```
│─────────────────────────│
│ M  X● L○ V○ P● T  ,  b○ │   the quick strip: one cell per tool
│ Preset  ‹3: Living Rm*› │
│ Source  ‹USB›           │
│ Volume  User   -12.0 dB │
│ ━━━━━━━━━━━━━━━●━━━━━━━ │
│ C0 ▓▓▓░░ 31% C1 ▓▓▓▓░ 74%│
```

The quick strip is one focusable row. Each cell is a key letter and, for
the four toggleable features, a dot: `●` on, `○` off. With the strip
focused, `←`/`→` move between cells, `Space` toggles the feature (Console
left-click), `Enter` opens its panel (Console right-click). The letters are
also global keys (section 3). Bypass Master EQ is the `b` cell.

Preset and Source are picker rows: `←`/`→` cycle, `Enter` opens the list.
The Preset row's context actions (Save, Rename, Set as Default, Copy to,
Clear, Clear All Slots) are on `Enter` in a second column of the popup,
and in the palette.

Volume shows the mode word (`User` / `Master`) which `Enter` on the label
switches, the readout, and the slider. The master slider is red. `Backspace`
resets to 0 dB (Console right-click). The tapers are the Console's.

### 2.4 Focus model

`Tab` cycles the regions: sidebar list, sidebar footer, graph legend,
detail. `Shift-Tab` goes back. Within a region, arrows move. The focused
region's border is drawn in the accent colour; unfocused borders are
chrome. Inside the focused region, the focused row is marked with a `▸`
and its label is in the accent colour.

`Esc` goes back: closes a popup, then a dialog, then a tool panel, then
returns focus to the sidebar. `Esc` in the sidebar with a channel selected
returns to the Overview (Console: tapping the selected row again).

`Enter` on a value arms it for editing; typing replaces it; `Enter` commits
and `Esc` reverts. While a numeric field is armed, `←`/`→` nudge by the
field's step, `Shift` by 10x, and the change is sent live (Console: slider
drag sends live, release commits). `Backspace` on an unarmed field resets it
to its default where the Console offers a right-click reset.

Selection in the sidebar follows the cursor: moving the cursor onto a
channel selects it (Console: single click). This keeps the detail region
and the graph focus in step with the arrow keys without an extra press.

## 3. Keys

Uppercase letters open the Console's tool windows and use the Console's
own mnemonics (Shift-Cmd-M in the Console is `M` here):

| Key | Opens | Console |
|---|---|---|
| `M` | Matrix Mixer | Shift-Cmd-M |
| `L` | Loudness Compensation | Shift-Cmd-L |
| `X` | Crossfeed | Shift-Cmd-X |
| `P` | Psychoacoustic Bass | Shift-Cmd-P |
| `U` | Stereo Upmixer | Shift-Cmd-U |
| `V` | Volume Leveller | Shift-Cmd-V |
| `G` | Test Signals | Shift-Cmd-G |
| `T` | Stats for Nerbs | Shift-Cmd-T |
| `I` | Interrupt Monitor | Shift-Cmd-I |
| `B` | AutoEQ Browse Profiles | Shift-Cmd-B |
| `,` | Settings | Cmd-, |

Pressing the same key again closes the panel (Console: the toggle
behaviour of the strip). Only one tool panel is open at a time.

Global keys that work everywhere:

| Key | Does |
|---|---|
| `Ctrl-P` | Palette: every parameter, every tool, every file action |
| `:` | Command line |
| `?` | Help for the focused region |
| `Ctrl-S` | Commit Parameters (save to the active slot), with confirm |
| `Ctrl-D` | Device picker |
| `Ctrl-Z` / `Ctrl-Y` | Undo / redo of live parameter writes |
| `Ctrl-C` / `q` | Quit, with the unsaved-changes prompt |
| `=` | Cycle graph height: small, medium, large, hidden |
| `g` | Graph pop-out: the graph fills the detail pane with its own legend |
| `p` | Toggle the phase overlay on the graph |
| `+` / `-` | Graph vertical zoom |
| `h` / `l` | Graph cursor |
| `b` | Bypass Master EQ |
| `c` | Clear clip latches |
| `F2` | Not used. Disclosure levels are dropped; the Console has none |

File actions are palette and command-line verbs, not keys: `:import`,
`:export`, `:import-config`, `:export-config`, `:autoeq`, `:save-master`,
`:save-output-config`, `:revert`, `:factory-reset`, `:bootloader`.

Digits are contextual: in a filter list they jump to a band; in the
sidebar they select an input.

## 4. Colour

### 4.1 Channel palette

The Console's `ChannelPalette.swift`, quantised. Truecolor values are the
Console's exactly. The 256-colour indices were chosen so that no two
channels that can appear together share an index; two Console pairs
collide after naive rounding (FC with OUT2, LFE with OUT4) and were moved
one cube step. At 16 colours identity relies on the pill label and the
curve-end label, and the mapping just keeps neighbours apart.

| Channel | Role | Truecolor | 256 | 16 |
|---|---|---|---|---|
| IN1 | FL | `#4A8FE3` | 68 | Blue |
| IN2 | FR | `#F57373` | 203 | Red |
| IN3 | FC | `#73C78C` | 78 | Green |
| IN4 | LFE | `#EDB34D` | 215 | Yellow |
| IN5 | BL | `#998CEB` | 104 | Magenta |
| IN6 | BR | `#E68CC7` | 176 | LightMagenta |
| IN7 | SL | `#66C7D1` | 80 | Cyan |
| IN8 | SR | `#CCB86B` | 179 | LightYellow |
| OUT1 | out 0 | `#45C2A3` | 79 | Cyan |
| OUT2 | out 1 | `#59D180` | 84 | LightGreen |
| OUT3 | out 2 | `#F0C459` | 221 | LightYellow |
| OUT4 | out 3 | `#F2A64D` | 209 | LightRed |
| OUT5 | out 4 | `#598CF2` | 33 | LightBlue |
| OUT6 | out 5 | `#8CB3F2` | 111 | LightCyan |
| OUT7 | out 6 | `#D9738C` | 168 | Magenta |
| OUT8 | out 7 | `#F299A6` | 217 | LightMagenta |
| PDM | sub | `#BA87F2` | 141 | Magenta |

On RP2040 the outputs are OUT1..OUT4 and PDM, using the first four output
colours and the sub colour. The Console's `outRight` quirk (output 2's
colour on the second output of the legacy model) is not carried; the
legacy model is gone.

### 4.2 Semantic and chrome

| Name | Use | Truecolor | 256 | 16 |
|---|---|---|---|---|
| `accent` | focus, selection bar, armed bypass disc, output enable | `#3A8DFF` | 75 | LightBlue |
| `danger` | mute, clip zone, destructive, master slider | `#F5453C` | 203 | LightRed |
| `warning` | unsaved glyph, INV, conflict outline, banners | `#F0A030` | 214 | Yellow |
| `ok` | connected dot, live pill, applied check | `#5AC26B` | 78 | Green |
| `fg` | values, names | `#E6E6E6` | 252 | White |
| `dim` | captions, section headers, disabled | `#8C8C8C` | 245 | DarkGray |
| `chrome` | borders, grid, dividers | `#4A4A4A` | 238 | DarkGray |
| `chrome_faint` | minor grid | `#333333` | 236 | Black |

Grid lines: major frequency lines and the 0 dB line in `chrome`, minor in
`chrome_faint`, axis labels in `dim`. Phase overlay in `fg` dotted.

### 4.3 Depth fallbacks

- **256 colours**: the tables above.
- **16 colours**: the tables above; the graph labels every curve at its
  right end with its pill name because hues repeat.
- **Mono / `NO_COLOR` / `TERM=dumb`**: no colour. Selection is reverse
  video. Pills are `[IN1]` with `*` for visible. Curves: the selected
  channel is solid braille, others are drawn with every other column
  dropped (a dotted look), and every curve is labelled at its right end.
  Meters are `▓` / `░` with `!` in the clip zone. Chips selected are
  `[■1]`, unselected `[ 1]`.

### 4.4 Themes

`console` (default, this document), `amber` (the existing phosphor theme,
kept as a variant), and `mono`. `--theme` and the config file select them.
The Console is dark-only, so the terminal's own background is assumed
dark; on a light terminal the `console` theme still works because it
paints no backgrounds, but the palette is not re-tuned for it.

## 5. Text conventions

- **Section headers** are the Console's uppercase labels (`INPUTS`,
  `OUTPUTS`, `PARAMETERS`, `OUTPUT PAIRS`) in `dim`, as a row of their
  own, preceded by a blank row except at the top of a box.
- **Panel titles** are the Console's window titles in bold `fg`, with the
  subtitle after a `·` in `dim`: `Crossfeed · BS2B Bauer
  Stereophonic-to-Binaural`.
- **Captions** under parameters are the Console's, in `dim`, wrapped to the
  box width, at most two lines.
- **Numbers** carry their unit with a space: `2856 Hz`, `-8.6 dB`, `31%`.
  Decimals follow the Console's `ValueField` for that field (gain 1 dp
  displayed, 3 dp editable; Q 3 dp with trailing zeros stripped; Hz
  integer; ms 1 dp). `-inf` for the master volume floor.
- **Type codes** in the overview cards are the Console's: `OFF PK LS HS LP
  HP NO AP`, with `LS1 HS1 LP1 HP1 AP1 LT` for the first-order and Linkwitz
  types, and the crossover short labels (`LR4LP`, `BW2HP`, `BES4LP`).
- **Channel descriptors** are `IN1`..`IN8` and `OUT1`..`OUT9`; the PDM
  output is `OUT9` on RP2350 and `OUT5` on RP2040, as the Console numbers
  them.
- No em-dashes.

## 6. Widget kit

Every widget is a function of `(area, state, theme, focus)` in
`crates/dspi-tui/src/widgets/`, and every widget has a golden test in every
colour depth. Keys listed are handled by the widget's `handle(key) ->
Option<Action>` so screens do not re-implement them.

### 6.1 Section header

```
 PARAMETERS
```
`dim`. Optional trailing action on the right (`Presets ▾`).

### 6.2 Parameter row (slider + value field)

```
▸ Cutoff Frequency                                     700 Hz
  ━━━━━━━━━━━━━━━━━━━━━━●━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  Simulates head shadow lowpass cutoff. Lower = more bass crossfeed.
```
Three rows: label with the value right-aligned, the slider, the caption.
The slider is `━` in `chrome` with the knob `●` in the channel colour or
`fg`, and the filled part left of the knob in `fg`. Focused: label in
`accent`, knob in `accent`. Armed for typing: the value is shown as
`[700 Hz]` with a cursor. Keys: `←`/`→` step (sent live), `Shift` 10x,
`Enter` arm / commit, `Esc` revert, `Backspace` reset to default. Log-taper
fields (frequency, Q) step multiplicatively. Disabled rows draw in `dim`
with no knob.

Compact variant (one row, no slider, no caption) for tables and headers:
`Preamp  -5.3 dB`.

### 6.3 Toggle row

```
▸ Interaural Time Delay                                  ● On
  Simulates ~220 us path difference via all-pass filter
```
`● On` in `ok`, `○ Off` in `dim`. `Space` or `Enter` toggles. Header
toggles (panel master switches) use the same widget on the title row.

### 6.4 Picker row

```
▸ Speed                                    ‹ Medium ›
```
`←`/`→` cycle; `Enter` opens a popup list under the value with the choices,
current one marked, arrows and `Enter` select, `Esc` closes. Segmented
variant draws all choices inline with the selected one in reverse video:
`Slow │ Medium │ Fast`. Hierarchical variant (filter type) draws the popup
as a flat list with group headers in `dim`.

### 6.5 Chip row

```
 OUTPUT PAIRS                                         Presets ▾
 [1] [2] [3] [4]
```
Selected chips are reverse video in the channel colour; unselected are
`chrome` outline. `←`/`→` move, `Space` toggles, `p` opens the presets
list. The test-signals variant has a third state, inverted polarity, drawn
`[ø1]` in `warning`. Disabled outputs draw in `dim`.

### 6.6 Status pill and banner

Pill: ` Active ` in reverse `ok`, ` Inactive ` in reverse `warning`,
` Disabled ` in reverse `dim`. Banner:
```
 ▲ Adjust only with audio stopped
   Changing these settings while audio is playing can send a loud pop.
```
in `warning`, two lines max. Info banner uses `ⓘ` in `dim`.

### 6.7 Card

A box with a `chrome` border; the title row holds `● Name` in the channel
colour and any right-aligned detail (`Delay: 0 ms`). A stereo card is one
box with a vertical divider; the left half of the top border is drawn in
the first channel's colour and the right half in the second's, which is
the terminal's version of the Console's gradient stroke.

### 6.8 Table

Header row in `dim` uppercase, fixed column widths given by the screen,
alternating rows unmarked (the Console's 3 % tint has no terminal
equivalent worth the noise). The focused row has `▸` in its margin and its
focused cell bracketed. Scrolls with a `▲`/`▼` hint in the border when
clipped.

### 6.9 Meter

```
 FL        ▓▓▓▓▓▓▓░░░▏
```
Log scale, -60 dB floor, `▓` filled in the channel colour, `░` in
`chrome_faint`, a final clip cell `▏` that turns `danger` reverse video
when the latch is set. Peak hold: a single `▌` at the 1 s peak, decaying.
The CPU meter is the same widget, 8 cells, `accent` fill turning `danger`
above 90 %, with the percentage after.

### 6.10 Pill (channel visibility)

`● IN1` in the channel colour when visible, `○ IN1` in `dim` when hidden.
`Space` toggles. In the legend row they are laid out left to right with
two spaces between.

### 6.11 Dialogs

Centered box, `accent` border when it is the focus, title bold, body
wrapped, buttons on the last row right-aligned with the default in reverse
video. `Tab`/arrows move between buttons; `Enter` activates; `Esc` is
Cancel; the first letter of a button also activates it. Variants:
- **confirm**: `Save` / `Discard` / `Cancel` and the like.
- **text**: one field (Rename Preset, channel name).
- **list**: a scrolling list with optional search (device picker, channel
  picker for filter import, target picker).
- **checklist**: the import-options prompt with two checkboxes.
- **progress**: a bar and a status line ("Writing settings to the
  device...").
- **report**: a scrolling text (import result).

### 6.12 Save bar

```
 ● Unsaved changes  Saving writes these settings to the device's flash.   Revert  [Save]
```
Only while dirty; `warning` glyph; progress spinner while busy. `Ctrl-S`
saves from anywhere in Settings.

### 6.13 Help overlay

`?` draws a centered box listing the focused region's keys, then the
global keys. `Esc` or `?` closes it.

### 6.14 Pin grid

A 9-column grid of `GP0`..`GP28` cells; claimed cells in reverse video of
their role tint (`accent` outputs, `danger` clocks, `ok` inputs, `IN5`
violet control, `IN8` olive utility), free in `chrome` outline, with the
owner shown on the echo line as the cursor moves.

## 7. Screens

Each screen is described as: layout, what each control does, and the keys
beyond the kit's. Strings come from `survey-console.md` section 2.

### 7.1 Sidebar list

- Row: selection bar `▍` (accent, only on the selected channel), name
  (left, 9 columns, truncated with `…`), meter (10 cells + clip cell), the
  descriptor pill. Linked input pairs both show the bar when either is
  selected. A muted or disabled output dims the row.
- Keys: `↑`/`↓` move and select; `Enter` on the selected row toggles back
  to Overview; `r` rename (text dialog); `y` copy parameters; `Y` paste
  parameters (disabled when the clipboard is empty); `i` identify (outputs,
  when the generator exists); `Space` toggles the pill.
- Sections show only what exists: 2, 4, 6 or 8 inputs; enabled outputs
  only.

### 7.2 Graph and legend

Ported from the Console's `GraphView.swift` behaviour: log-x
`min_freq`..`max_freq`, linear-y `center ± range/2`, adaptive dB step
(1/3/5/10), 0 dB line, axis labels; phase overlay dotted on a ±180 deg axis
scaled with the range; identical-curve grouping (output gain folded in;
bit-identical magnitude arrays collapse into one curve drawn in the first
member's colour with the other members' pills marked `=` after their
descriptor); curves not containing the selected channel are drawn dotted
(every other braille column) when a channel is selected, all solid on the
Overview; the selected channel's curve is drawn last so it is on top.
Legend pills toggle visibility. The graph's own keys: `h`/`l` cursor with
a per-visible-channel readout on the echo line, `+`/`-` range, `p` phase,
`=` height, `g` pop-out. Settings > Graphing values apply.

### 7.3 Overview

The Console's dashboard cards, stacked and scrolling: a stereo card for
IN1/IN2, a stereo card per enabled S/PDIF pair, a single card for PDM.
Each card: header with dot, name, `Delay: N ms`; ten rows of `#`, type
code, freq, gain, Q as the Console shows them, `-` for inactive. Cards are
read-only; `Enter` on a card selects that channel.

### 7.4 Input page

Header row: `Link 1/2` pill (`ok` when linked, `dim` when not; hidden when
the partner is not live; toggling with mismatched pairs raises the "Inputs
N and M don't match" confirm with Keep INn / Keep INm / Cancel), the
Preamp compact slider, `Clear PEQ` (or `Clear 1/2 PEQ`). Then the filter
list (7.6) in PEQ mode with Linkwitz hidden. Edits mirror to the linked
partner.

### 7.5 Output page

Header rows: the routing panel (one line per input up to the base count:
`● FL  -3.0 dB  INV` with `●`/`○` connect, gain field, INV in `warning`
when inverted), then `GAIN -12.0 dB ━━━●━━`, `DELAY 0.0 ms ━●━━━`, `MUTE ○`
(`danger` reverse when muted). Then the `PEQ │ XO` tab strip (only when
crossovers are supported) and the filter list in the chosen mode.

### 7.6 Filter list

```
    #  TYPE                 FREQ        GAIN        WIDTH
 ●  1  Low Shelf 12 dB/oct   105 Hz     +8.8 dB     0.707
 ●▸ 3  Peaking             [2856 Hz]    -8.6 dB     3.580
    5  Off
 ●  6  Linkwitz Transform   f0 40 Hz Q0 0.5 → fp 25 Hz Qp 0.71  +8.2 dB  ⚙
```
Columns as the Console's headers, PEQ mode `# TYPE FREQ GAIN WIDTH`, XO
mode `# FAMILY TYPE SLOPE FREQ`. The bypass disc: `●` armed, `○`
bypassed, blank for Off; hidden when bypass is unsupported. Type is a
hierarchical picker with the Console's groups. Gain and Q hide for types
that do not use them. The Linkwitz row shows its four values inline and
`Enter` on `⚙` opens a dialog with the Driver / Target fields, the DC
boost readout (`warning` above 15 dB), and Revert / Apply (staged). XO
rows: Family picker (Off / Linkwitz-Riley / Butterworth / Bessel), Type
(LP / HP), Slope (order x 6 dB/oct, only the orders the family allows),
Freq.

Footer actions (on the echo line's right side while the list is focused):
`Enable All │ Bypass All   Clear All   PEQ │ XO`. Keys: `a` enable all,
`A` bypass all (XO mode confirms with the tweeter warning), `D` clear all
(confirm), `x` toggle PEQ / XO, `1`-`9`,`0` jump to band, `Space` bypass,
`↑`/`↓` band, `←`/`→` field, `Enter` edit.

### 7.7 Matrix Mixer panel (`M`)

Replaces the detail pane:

```
 Matrix Mixer                                     Direct 1:1   Clear
                 OUT1      OUT2      OUT3      OUT4      OUT9
 ROUTING
 FL   -5.3 dB   ● 0.0    ○  -      ○  -      ○  -      ● -6.0 INV
 FR   -5.3 dB   ○  -     ● 0.0     ○  -      ○  -      ● -6.0
 ...
 ENABLE          ⏻ on     ⏻ on     ⏻ off     ⏻ off     ⏻ on
 GAIN            0.0 dB   0.0 dB   0.0 dB    0.0 dB   -3.0 dB
 DELAY           0.0 ms   0.0 ms   0.0 ms    0.0 ms    2.5 ms
 MUTE            ○        ○        ○         ○         ●
```
Column headers in the output colour; input labels in the input colour;
the input trim column only in 8-channel mode. A cell shows `●`/`○` and
the gain; `INV` in `warning` when inverted. Cells of a disabled output in
`dim`. A cell that would conflict with Core 1 has a `warning` outline
marker `!`. Keys: arrows move the reticle; `Space` connects / disconnects;
`Enter` edits the gain; `i` inverts; `←`/`→` on a gain nudges by 0.5 dB;
in the ENABLE row `Space` toggles with the conflict confirms; `d` Direct
1:1; `D` Clear (confirm); `r` rename the column's output; `y`/`Y` copy /
paste; `I` identify. Scrolls horizontally when there are more columns than
fit, with the input label column pinned.

### 7.8 Tool panel template (`L` `X` `V` `P` `U`)

```
 Crossfeed · BS2B Bauer Stereophonic-to-Binaural                   ● On
 FREQUENCY RESPONSE
 [ small graph, 8 rows, or "Disabled" ]
 OUTPUT PAIRS                                              Presets ▾
 [1] [2] [3] [4]
 Crossfeed only the stereo output pairs feeding headphones. ...
 PRESET
 ● Default    700 Hz / 4.5 dB - Balanced, most popular
 ○ Chu Moy    700 Hz / 6.0 dB - Stronger spatial effect
 ○ Jan Meier  650 Hz / 9.5 dB - Natural speaker-like
 ○ Custom     User-defined parameters
 PARAMETERS
 ▸ Cutoff Frequency ...
```
Sections scroll as one column. The header toggle is first in focus order.
The unsupported banner replaces the body. Each panel's graph: crossfeed
response, ISO 226 curve at -40 dB, psybass spectrum with `fc` and `4fc`
marks, the upmixer's live gauges (four one-line meters). The leveller has
no graph (the Console has none).

### 7.9 Test Signals panel (`G`)

The 16 tiles as a 4x4 grid of `[ Sine ]` cells, selected in reverse video,
with the blurb on the echo line; OUTPUTS chips with polarity; LEVEL row;
PARAMETERS rows for the type; TIMING rows per model; OPTIONS toggles; the
transport row `Start` / `Stop` / `Stop now` with `Space` bound and the
blockers shown as the reason text. A running generator shows its state
line from `SIGGEN_STATE` notifications.

### 7.10 Stats panel (`T`)

The Console's sections as `Label ......... value` rows under section
headers, badges as status pills, the buffer fill rows as short meters with
the min-max watermark drawn `▏ ▕`. `r` resets watermarks. Refresh every
2 s, the footer says so.

### 7.11 Interrupt Monitor panel (`I`)

A log table: time, seq, event, source, decoded field and value. `Space`
pauses, `D` clears, the header shows Listening / Paused / Inactive and the
count. This is `dspi watch` in a panel.

### 7.12 AutoEQ browser (`B`)

Search field on top; results table with name, source pill (oratory1990
`warning`, crinacle `IN5`, rtings `accent`, innerfidelity `ok`, other
`dim`), form factor, `♥` favourite; bottom line with the selection summary
and `Apply` / `Cancel`. `f` toggles favourite. The favourites appear at
the top of the list under a FAVOURITES header when the search is empty
(the terminal's version of the Favorite Profiles submenu). `:autoeq
update` runs the Update Database flow with its three methods.

### 7.13 Settings (`,`)

Full screen:

```
 Settings                                                                 Esc back
┌──────────────────┐┌ Global Parameters ─────────────────────────────────────────────┐
│ APPLICATION      ││ STARTUP PRESET                                                  │
│   About          ││ ▸ Mode                                   ‹ Specified Default › │
│   Advanced       ││   Default Preset                         ‹ 3: Living Room ›    │
│ DISPLAY          ││   Choose which preset loads when the device powers on.         │
│   Graphing       ││                                                                 │
│ SYSTEM           ││ EXTERNAL MUTE CONTROL                                           │
│   Overview       ││ ▲ Adjust only with audio stopped                                │
│ ▸ Inputs         ││   Changing these settings while audio is playing can send ...  │
│   Outputs        ││   Enable Automatic Mute                                  ○ Off  │
│   I2S Config.    ││ ...                                                             │
│   Global Params  ││                                                                 │
│ CONTROL          ││                                                                 │
│   Control Surf.  ││                                                                 │
│   Control Interf.││                                                                 │
│   Channel Groups ││                                                                 │
│   Macros         ││                                                                 │
└──────────────────┘└─────────────────────────────────────────────────────────────────┘
 ● Unsaved changes  Saving writes these settings to the device's flash.   Revert  [Save]
```
The sidebar lists only available pages. `[`/`]` go back and forward
through visited pages. Every page is a scrolling column of the kit's rows
in the Console's order with the Console's captions. The save bar follows
the Console's three dirty categories. Pin pickers list `GPIO n` for free
pins and mark claimed ones with their owner.

Control Surfaces: one collapsible card per slot, `Add Control` as a
picker of types, the card body as rows in the Console's order (type,
name, parameter, action, event, target, index, pins, operands, delays,
flags, Apply / Revert). IR commands and display pages are nested cards
inside their parent. Learn shows `Waiting for a button... Cancel` and the
result chip. Channel Groups and Macros pages follow the same card
pattern.

### 7.14 Dialog flows

- **Unsaved changes**: the Console's text with the bulleted diff, Save /
  Discard / Cancel, raised on preset switch, device switch and quit.
- **Import Filters**: file path text dialog (with tab completion), then the
  single-channel or multi-channel checklist.
- **Import Device Configuration**: path, then the options checklist with
  provenance and the cross-platform warning, then progress, then the
  report.
- **Firmware Update**: the critical confirm, then a progress dialog that
  waits for the device to disappear and the `RPI-RP2` volume to appear, and
  tells the user where to copy the `.uf2`.

## 8. Density rules

| Size | Sidebar | Graph | Detail | Notes |
|---|---|---|---|---|
| 80x24 | 22 | 7 rows, `=` hides | rest | captions hidden; one-line param rows |
| 120x40 | 24 | 12 rows | rest | the reference layout |
| 160x50+ | 26 | 16 rows | rest | captions always shown; overview cards two abreast |
| 200x60 | 28 | 20 rows | rest | the matrix shows every column without scrolling |

Param rows drop their caption first, then their slider, before anything
truncates. Tables truncate the TYPE column with `…` last.

## 9. Motion

- On connect: chrome first, then the curves draw left to right over 400 ms
  while the bulk read completes; the sidebar meters fade in after. Skipped
  with `--lite`, `--no-animation`, `TERM=dumb`.
- A value changed by the device (source not `HOST_SET`) eases over 120 ms
  on its slider and flashes its label once in `ok`. A change with source
  `GPIO` also prints `changed by control surface` on the echo line.
- Meters: 20 Hz, peak hold 1 s with linear decay, clip latch sticky until
  `c` or the 3 s auto-clear the Console uses.
- Nothing else animates. No spinners that outlast their operation.

## 10. Phase 3 exit checklist

- `theme.rs`: `console`, `amber`, `mono`; every table in section 4 as
  data; a test that the 17 channel indices are distinct at 256 colours and
  that no two channels that can be shown together share an index.
- `widgets/`: every widget in section 6 with `handle()` and golden tests at
  truecolor, 256, 16 and mono.
- The shell: title bar, sidebar with footer, graph with legend, detail
  region switch, tool panel swap, Settings full screen, echo and key
  lines, focus model, `Esc` stack, help overlay; golden tests at 80x24,
  120x40, 200x60.
- Graph: legend pills, grouping, dotted unselected, cursor readout, phase,
  height cycling, pop-out.
- `examples/gallery.rs` renders every widget and the shell with fixture
  data for design review, and `dspi screenshot` uses the same fixture so
  documentation screenshots need no device.
- A test that every key named on the key line of every screen is handled.
