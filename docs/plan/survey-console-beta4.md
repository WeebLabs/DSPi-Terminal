# Survey: DSPi Console changes, 75793c3 to 9dbb07a (2026-09-29)

Read from `/Users/weeblabs/DSPi Console` on `release/v1.1.6`, from `75793c3`
(the baseline of `survey-console.md`) to `9dbb07a` ("Version 1.1.6-beta4"),
138 commits later. Paths are under `DSPi Console/` unless stated; line
numbers are at `9dbb07a`, and the hash after an item is the commit that
introduced it. This file only records what changed.

The governing change: `WIRE_FORMAT_VERSION` 28 to 32, `BULK_PARAMS_SIZE`
5944 to 6136 (`Constants.swift:684,689`), with an exact-match parser
(`Commands.swift:3558`). The Console now only fully works with V32
firmware. It bundles `DSPi-RP2040-v1.1.6-beta4.uf2` and
`DSPi-RP2350-v1.1.6-beta4.uf2`.

## 1. New windows, panels and Settings pages

### 1.1 Subharmonic Synthesizer (4fc9918; layout 2b2a550, 0fe2a8e, 99bbfaf, b1a900e)

Tools > "Subharmonic Synthesizer..." **Cmd-Shift-S**
(`DSPi_ConsoleApp.swift:11028-11031`). No sidebar icon. Window 780x620,
minimum 740x612 (`SubharmonicSynthView.swift:5-62`). Closing the window
turns solo off. Requires wire >= 29, otherwise "Requires firmware with wire
format V29 or newer."; the top band, selectivity, ceiling, link, solo and
meter need wire >= 30 and are hidden otherwise.

Header: title "Subharmonic Synthesizer", subtitle "Generates a subharmonic
at half the frequency of the source's bass", a latching **SOLO** button
(orange when on, disabled unless enabled), and the master switch. Left
column: BANDS graph, LEVELS. Right column: SELECTIVITY, SUB CEILING, LF
BOOST, OUTPUTS.

| Control | Range / values | Default | Wire |
|---|---|---|---|
| BANDS graph, "Apply preset" menu | Subwoofer feed (-6/-6/0), Club / large PA (0/0/+3), Thin recordings (0/-6/+3), Cinema LFE (+3/-12/0); values are low/high/boost and the top band goes to the floor (`:77-82, 326-344`) | | |
| HEADROOM COST | "+x.x dB" in orange, or "none" | | 0x1A, re-read after each relevant SET |
| 24 - 36 Hz ("Derived from 48 - 72 Hz") | -30..+12 dB, step 0.5; -30 shows "Off" | 0 | 0x12 / 0x13 |
| 36 - 56 Hz ("Derived from 72 - 112 Hz") | same | 0 | 0x14 / 0x15 |
| 56 - 80 Hz ("Derived from 112 - 160 Hz") | same | -30 | 0x1B / 0x1C |
| Selectivity | All material / Percussive / Sustained | All | 0x1D / 0x1E |
| Depth (hidden in All) | 0..100 %, step 5 | 100 | 0xA9 / 0xAA |
| Hold (hidden in All) | 50..400 ms, step 10 | 150 | 0xAB / 0xAC |
| SUB CEILING Threshold | -40..0 dBFS, step 1; 0 shows "Off" | 0 | 0xAD / 0xAE |
| LF BOOST "70 Hz bell" | 0..6 dB, step 0.5 | 0 | 0x16 / 0x17 |
| OUTPUTS chips, "Presets" menu | Sub only (recommended, the PDM bit) / All outputs / None | 0xFFFF | 0x18 / 0x19 |
| Per-chip sub meter | 0..32767 | | 0x1F, polled at 10 Hz while visible and enabled |
| Link output pairs, "One sub per pair, from its mono sum." | bool | on | 0x2E / 0x2F |

Solo is read with 0x2D when the window opens. Control Surfaces nouns 57-67
form the category "Subharmonic Synth" (strip "Subharm"), for example
"Subharm 24-36 Hz Level" and "Subharm Solo".

### 1.2 Tube Modeller (2667bcc; 2b0b2a2, ec91f6d, 4bc9e00, 2096c5e, aadf5ff, b889582)

Tools > "Tube Modeller..." **Cmd-Shift-D**. Window 780x664, width-resizable,
height fits content. Requires wire >= 31. Header: a drawn tube that glows
while on, title "Tube Modeller", subtitle "Valve-style harmonic colour,
supply sag and a tube amplifier's output stage", a **Basic / Advanced**
switch (persisted, default Basic), and the master switch. The app mirrors
the firmware's type-row and Custom rules locally
(`Commands.swift:1414-1550`).

| Control | Range | Default | Notes |
|---|---|---|---|
| Tube menu | Custom; "Preamp triodes" 1-8, "Preamp pentodes" 9-10, "Power stages" 11-16 | 1 (12AX7) | Push-pull rows (11-15) add "Meant for use with the output stage on." |
| Drive | -30..+24 dB, step 0.5; Basic ends "Clean" / "Overdrive" | -12 | |
| Mix | 0..100 %; ends "Dry" / "All tube" | 100 | |
| Output Trim (Advanced) | -12..+12 dB | 0 | |
| Bias | -100..+100 % | 10 | |
| Asymmetry | -12..+12 dB | 3 | |
| Knee Hardness | 0..100 % | 40 | |
| Sag | 0..100 % | 15 | Dimmed when the rectifier is Solid state |
| Rectifier | Solid state / GZ34 / 5U4 / 5Y3 | GZ34 | Summary such as "Sag depth x0.6, 5 ms attack, 120 ms release." |
| OUTPUT STAGE ("A valve amplifier's loose grip on the speaker.") | bool | on | Rows below hidden while off |
| Damping Factor | 1..20; ends "1 (loose)" / "20 (tight)" | 2 | Readout "+x.x dB at resonance, +x.x dB at the top." |
| Speaker Resonance | 30..150 Hz | 95 | |
| OUTPUTS chips, Presets | All outputs / Exclude sub / None | 0xFFFF | |

Tube rows (name, style, bias/asym/hardness/sag) are at
`Constants.swift:364-395`; rectifier rows at `:400-405`. Advanced mode adds
a TRANSFER CURVE graph with presets Clean default, Warm hi-fi,
Single-ended sweetness, Guitar-amp style and Push-pull power (each also
sets mix 100 and trim 0), and an "AT FULL SCALE" 2nd/3rd harmonic readout.
Basic mode shows a tube illustration that flares with the masked outputs'
peak meters, a TUBE shelf of one-click chips, Drive, Mix and OUTPUTS.
Control Surfaces nouns 70-73 form the category "Tube Modeller". The
README says the output stage is off by default; the code says on.

### 1.3 Output Limiter (a426166; 0eb4972, 7d86acd, 9624658)

No window. A gauge icon sits under the MUTE button on each output channel
page (`Components.swift:772-792, 868-905`; `OutputLimiterView.swift`).
Requires wire >= 32. The icon is grey when off, accent when on and orange
while gain reduction is at least 0.05 dB. Click toggles; right-click opens
a 320-wide popover with an on/off switch; settings grey out while off.

| Control | Range | Default |
|---|---|---|
| Threshold | -30..0 dBFS, step 0.5 | -1.0 |
| Release | 10..1000 ms, step 10 | 100 |
| Link group | Off / 1 / 2 / 3 / 4; summary "Not linked." / "Linked with ..." / "No other outputs in group N." | Off |
| "Copy to all outputs" | copies threshold, release and enable via output 0xFF, not the link group | |
| "All outputs" menu | Link all stereo pairs (1+2 in group 1, and so on up to group 4; PDM stays unlinked) / Unlink all outputs / Switch every limiter off | |

The meter (0x81 index 0x80) is polled on the 60 ms timer only while the
icon is on screen and some limiter is on. The app mirrors ganging locally
(`LimiterGang`, `ToolParameters.swift:60-105`). In WITH_PRESET mode an edit
dirties the preset; in INDEPENDENT mode it marks the output configuration
unsaved, Revert restores it, and 0x52 saves it.

### 1.4 Spectrum analyser (4dfac1b and many follow-ups)

Available when `REQ_RTA_GET_CAPS` returns 16 bytes with version 3 and the
band-centre table reads (`SpectrumAnalyser.swift:536-583`). The engine
(`tick`, `:612-773`, on the 60 ms timer) pushes config only on change (3
retries at most), reads GET_BANDS_ALL for several channels or GET_BANDS for
one, reads bins every max(frame interval, 50 ms) only for a single
channel, reads status and config every 0.5 s, sends STOP when the last
viewer leaves, shows a frame older than max(500 ms, 4x refresh) as
silence, averages bins itself and applies 1/6-octave smoothing.

It appears in four places:

1. **Behind the response graph ("FFT Graph"):** a translucent fill in dBFS
   spanning the plot. One channel shows FFT bins blended from the bass
   bands; several show third-octave band curves. Peak-hold contour on top.
2. **"RTA Bars" strip** above the input page's band list, above the output
   page, and above the dashboard grid: one cell per channel, 1-4 columns
   (default 2), height 64..240 (default 96), with a gear for layout and
   "Open in Window".
3. **Graph gear popover** (replacing the hover pop-out arrow): SPECTRUM
   section with an Inputs / Outputs switch, per-side channel chips, "Clear",
   and "FFT Graph" / "RTA Bars" switches (separate for the dashboard and
   channel pages). "Graph Setup" page: SCALE (frequency 10/15/20/50/100 Hz
   to 5/10/20 kHz, range 10..100 dB, centre -40..+20 dB), GRID & LABELS
   (frequency grid, frequency labels, dB grid, dB labels, frequency
   readout, gain readout, grid opacity 0..200 %), CURVES (line width 1..4,
   glow, phase, unwrap phase), and "Pop Out Graph".
4. **Tools > "Spectrum Analyser..." Cmd-Shift-A**: a window that mirrors the
   current page's selection, with per-channel show/hide for this window
   only, a Curves / Bars / Both switch, and a status bar (Running/Idle,
   frames/s, transform time, main-loop and bass load, and warnings such as
   "Device refused this configuration").

The dashboard selection persists and defaults to the first enabled output.
A channel page starts on its own channel (default on).

**Settings > Display > Spectrum Analyser** (`DSPi_ConsoleApp.swift:1620-1782`):
Spectrum Strength 30..100 % (100), Peak Hold (on), Smoothing (on); Floor
-60 / -90 / -120 dB (-90); Ceiling 0 / +6 / +12 dBFS (+6); Transform Size
from caps (1024); Averaging Off / 50 / 125 / 300 ms / 1 s / 3 s (300);
Peak Decay Off / 4 / 12 / 30 dB/s (12). The engine values are Console
preferences re-pushed on connect, because the firmware forgets them.

### 1.5 Settings > Control > Auxiliary Outputs (4fc9918, reworked 7a2cb82)

Shown when CS caps >= 18 and types 9/10 exist. Each output is a binding
slot of type "On/Off Output" (9) or "Dimmable Output" (10), excluded from
the Control Surfaces add menu. Empty state "No Auxiliary Outputs Set Up".
Card rows: Output (live switch, 0x04), Level (PWM, 0..100 %, 0x06), GPIO
pin, Active-Low Output (INVERT), Level Limit (PWM, 1..100 %,
`base_bright`), Linear Response (PWM, `LINEAR`), turn-on and turn-off
delays, At Power-On Fixed / As Last Saved (`BOOT_SAVED`), Starts On
(`BOOT_ON`) with Starting Level for PWM, and Driven By (the controls,
remote keys and macros that target the slot). Reads the 48-byte 0x05
block, handles `NOTIFY_EVT_CS_AUX` live, and names status 0x26. New nouns
68 "Aux Switch" and 69 "Aux Level" use target kind 5, whose picker lists
only aux slots (only PWM slots for Aux Level) and is labelled "Auxiliary
Output(s)".

### 1.6 Firmware Update window (79495c0, 05381a5, e7d8cca, e8bab31, f623734)

Replaces the "Reboot into Bootloader" alert. Rows "This Console" and
"Connected device"; step strip Prepare / Write / Verify / Done; buttons
"Export Configuration...", "Update Firmware" / "Downgrade" / "Try Again",
then "Update Another Board" / "Done"; footnote "Enter bootloader mode
without installing" (0xF0). A **mismatch banner** over the main window
reads "This device runs firmware X; DSPi Console expects Y." (or "...newer
than DSPi Console Y. Some of its features may not be shown.") with
"Update...", "Details..." and "Hide" (per launch).

### 1.7 Onboarding (3d58c2f ... 03130c2, def02d1, 481fc60)

A Getting Started wizard, a What's New window, and a developer section in
Settings > Advanced. The tour and first-open hints are compiled in but
disabled (`OnboardingCoordinator.postSetupOnboardingEnabled = false`).

## 2. Changes to existing screens

- **Connection:** channel rows, dashboard cards and graph curves fade in
  once the first connect tier completes; without a device the sidebar
  shows no channels and the graph no curves; losing the device returns to
  the overview.
- **CPU meter** observes the meter model directly, fixing frozen readings.
- **Output gain drag** updates only the graph during the drag; other
  views update on release.
- **Response graph editing (mouse):** double-click adds a Bell; Cmd-click
  opens a two-step shape card (shape, then 6/12 dB or 180/360 degrees);
  drag the curve to add a band (Low Shelf in the leftmost 12 %, High Shelf
  in the rightmost 12 %, else Bell); marquee select; Cmd-click toggles and
  Shift-click selects a frequency-ordered range; Option-click bypasses;
  drag modifiers for Q, fine and axis lock; Control-drag scales a
  selection's gains proportionally (default is an equal-dB offset); wheel
  for Q and Cmd-wheel for gain; right-click band and graph menus.
- **Graph keys:** Delete, Escape, Tab / Shift-Tab in frequency order, Left
  / Right move 1/12 octave (1/96 with Shift), Up / Down move 0.5 dB (0.1
  with Shift) or Q for gainless types, Cmd-A.
- **Graph drawing:** bypassed bands are grey ghosts; a selected dot has a
  centre in the background colour; a 10-colour band palette; hover
  frequency and gain readouts (toggles, default on); "All N bands in use";
  grid opacity 0..200 %, default 50 %; no PEQ editing while the XO tab is
  showing; a newly shown channel draws at once; `magnitudeSquared` uses the
  sin^2(w/2) form, fixing narrow low-frequency peaks drawn at 0 dB.
- **Band chip:** two-letter shape codes PK / LS / LC / HS / HC / NT / AP;
  typed frequencies accept "2k", "2.5 kHz", "A4", "C#2+13"; limits gain
  +/-30 dB, Q 0.1..20, frequency 10 Hz..0.45 fs.
- **Band list:** band-number click selects on the graph, Cmd-click
  toggles, Shift-click selects a run skipping Off rows; selected and
  hovered row highlight; auto-scroll to a band selected on the graph.
  **PEQ pass labels now read HC / LC / HC1 / LC1**; crossovers keep LP /
  HP; filter files keep LP / HP / LP1 / HP1.
- **Output page:** the MUTE column shares space with the limiter icon;
  routing input names come from the sidebar channel names; the RTA strip
  sits above the settings card.
- **Dashboard:** cards per row Auto / 1 / 2 / 3 from a hover gear
  ("DASHBOARD LAYOUT").
- **Matrix Mixer:** input rows use the sidebar channel names; upmix rows
  still read C / Ls / Rs.
- **Tool windows** were re-laid out in two columns (Crossfeed,
  Psychoacoustic Bass, Loudness, Signal Generator, Volume Leveller) with
  unchanged ranges. **"Test Signals" is renamed "Signal Generator".**
- **System Statistics** is now three columns: device, system, audio output
  and PDM; SPDIF DMA starvation and buffer fill history graphs (about 15 s,
  256 samples, 6 series); then S/PDIF input, LG Sound Sync, ADAT and I2S
  input when present. The firmware row reads "v1.1.6 beta 4".
- **Settings:** Display gains "Spectrum Analyser"; Control gains
  "Auxiliary Outputs"; Graphing gains the two readout toggles and grid
  opacity; the independent-mode save bar covers limiters; the Outputs pin
  assignment fixes the PDM retry keyed on the old row name.
- **Control Surfaces:** new categories Subharmonic Synth, Tube Modeller
  and Auxiliary Outputs. "Test Signal" is now "Signal Generator"; filter
  values "Low Pass" / "High Pass" are now "High Cut" / "Low Cut". The
  Console has **no** limiter nouns (74-78) yet.
- **Presets:** `.dspipreset` gains optional top-level `subharm` and `tube`
  blocks and a per-output `limiter` block inside channel entries
  (`PresetDocument.swift:45-47, 239-310, 379, 408-435`). Limiters import
  only with the "Hardware I/O (GPIO pins, clocks, ADAT, inputs, output
  limiters)" box. New skip reasons name the missing firmware support.

## 3. Wire handling

- GET_PLATFORM read with length 7; display "1.1.6 beta 4" or "1.1.6 early
  beta"; `firmwareMatch` returns match / deviceOlder / deviceNewer against
  the app's own version.
- Connect fetch in two tiers: input source, sample rate, user volume and
  presets first, then the rest.
- `PRESET_LOADED` and `BULK_INVALIDATED` schedule a coalesced resync (0.15
  s delay, one in flight, one queued). A PARAM_CHANGED offset that is not
  decoded in place triggers a 0.25 s resync instead of being dropped. The
  app's own preset load and factory reset suppress resyncs for 3 s.
- Unsaved-changes diff gains Subharm and Tube categories and per-output
  limiter lines (only in WITH_PRESET mode); solo and headroom are excluded.
- Drag traffic commits on release and is rate-limited to 30 writes/s.

## 4. Removed or renamed

"Test Signals" to "Signal Generator"; the bootloader alert to the Firmware
Update window; the graph pop-out arrow to the gear popover; the Matrix
Mixer's 7.1 row names; PEQ LP / HP to HC / LC in the UI. Several RTA
experiments came and went inside the range and need no parity work.

## 5. Menus and shortcuts

Tools adds "Subharmonic Synthesizer..." (Cmd-Shift-S), "Tube Modeller..."
(Cmd-Shift-D) and "Spectrum Analyser..." (Cmd-Shift-A), and renames "Test
Signals..." (Cmd-Shift-G unchanged). A new Help menu has Getting
Started, What's New, and links to the two GitHub repositories. Everything
else is unchanged.
