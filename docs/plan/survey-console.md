# Survey: DSPi Console UI inventory (2026-08-27)

Read from `/Users/weeblabs/DSPi Console` on `release/v1.1.6` @ `75793c3`. Paths
are relative to `DSPi Console/`. This is the feature checklist the Terminal
must cover; the coverage matrix in `PLAN.md` maps each item to a Terminal
location. Note `Commands.swift` is the wire layer, not menus; the menu bar is
in `DSPi_ConsoleApp.swift:9913-10092`.

## 1. Top-level navigation

Not tabbed. One fixed-size main window (950 x 813) with an HSplitView
(sidebar + detail), a Settings window, and free-floating tool windows.

### Main window (`ContentView.swift:303-855`)

Sidebar (min 220, max 260), top to bottom:
1. Section header INPUTS; one ChannelRow per live input (2/4/6/8).
2. Section header OUTPUTS; one OutputRow per visible + enabled output.
3. Bottom inset: quick-access icon strip (8 buttons); divider; Preset picker
   row; Source picker row (conditional); volume slider section (Master or
   User mode); divider; CPU section (C0/C1).

Detail pane, top to bottom:
1. Header: "Filter Response" + ConnectionStatusIndicator.
2. BodePlotView, height = settings.graphHeight (200..350, drag handle).
3. Dynamic region switched on SidebarSelection: .overview -> DashboardOverview;
   .input(ch) -> InputChannelHeader + FilterListView; .output(idx) ->
   OutputChannelDetail. Tapping the selected row again returns to overview.

### Settings window (`DSPi_ConsoleApp.swift:267-423`)

NavigationSplitView 665 x 580; Back/Forward toolbar; persisted selection.

| Group | Pages | Availability |
|---|---|---|
| Application | About, Advanced | always |
| Display | Graphing | always |
| System | Overview, Inputs, Outputs, I2S Configuration, Global Parameters | I2S hidden on STM32H723; Inputs requires inputSourceSupported |
| Control | Control Surfaces, Control Interfaces, Channel Groups, Macros | CS: controlSurfacesSupported or disconnected; Interfaces: controlInterfacesSupported; Groups: csGroupsSupported; Macros: csMacrosSupported |

A shared save bar sits at the bottom of every page.

### Tool windows

Matrix Mixer; System Statistics (320 x 620); Filter Response pop-out (800 x
400); Crossfeed (380 x 560); Loudness Compensation (380 x 600); Volume
Leveller (380 x 540); Psychoacoustic Bass (380 x 640); Stereo Upmixer (400 x
720); Test Signals (428 x 760); Interrupt Monitor (900 x 480); AutoEQ Browse
Profiles (600 x 500); Updating Database progress (340 x 180).

### Sheets and modals

Rename Preset sheet (TextField "Name", Cancel/Rename); configuration import
progress sheet ("Writing settings to the device..."); many NSAlerts (section
4).

## 2. Every screen, section and control

### 2.1 Sidebar INPUTS (`ContentView.swift:307-359`, `Components.swift:401-468`)

Per input: 3 pt accent selection bar; name label / inline rename field (width
80); HorizontalMeterBar (level = peaks[ch], colour = input colour, clip bit
from clipLatched, 3 pt red clip zone at right); ChannelVisibilityPill IN1..IN8
toggling the graph curve. Row height 23/27/28 by input count. Option-click =
rename; tap = select/deselect; linked pairs highlight together. Context menu:
Rename | Copy Parameters | Paste Parameters (disabled when clipboard empty).
Rename writes REQ_SET_CHANNEL_NAME 0x9B.

### 2.2 Sidebar OUTPUTS (`ContentView.swift:361-402`, `Components.swift:470-539`)

Same as inputs plus muted dimming (opacity 0.4). Pill OUT1..OUT9. Context
menu: Identify (connected and siggenSupported; fires channel-ID tone) |
Rename | Copy Parameters | Paste Parameters.

### 2.3 Sidebar quick-access strip (`ContentView.swift:416-488`)

| Icon | Tooltip | Left click | Right click |
|---|---|---|---|
| sliders | Matrix Mixer | toggle Matrix window | |
| headphones | Headphone Crossfeed | toggle crossfeed 0x5E | open Crossfeed |
| speaker.zzz | Loudness Compensation | toggle 0x58 | open Loudness |
| waveform | Volume Leveller | toggle 0xB4 | open Leveller |
| bass clef | Psychoacoustic Bass | toggle 0x30 | open Psybass |
| info | Stats for Nerbs | toggle Stats | |
| gear | Settings | open/close Settings | |
| xmark | Bypass Master EQ | toggle 0x46 | |

### 2.4 Sidebar Preset row (`ContentView.swift:497-660`)

Label "Preset"; popup over slots 0..9 titled "Empty" / name / "Preset N";
active slot gets a trailing `*` when hasUnsavedChanges (the dirty marker).
Disabled when disconnected or switching. Selecting: if dirty, unsaved-changes
alert (Save / Discard / Cancel), then save 0x90 and/or load 0x91; failure
alerts "Load Failed" / "Save Failed"; CRC -> "Preset data is corrupted."
Context menu: Save; Rename...; Set as Default (0x96 mode 0; disabled when
already default); Copy to... submenu of the 9 other slots; Clear "<slot>"...
(destructive, confirm "Clear Preset?"); Clear All Slots... ("Clear All
Presets?").

### 2.5 Sidebar Source row (`ContentView.swift:663-690`)

Only when inputSourceSupported. Popup over 0 USB, 1 SPDIF, 2 I2S, 3 ADAT,
4/5/6 SPDIF 2/3/4; writes 0xE0.

### 2.6 Sidebar Volume section

Mode selector menu "User Volume" / "Master Volume" persisted in app settings.

UserVolumeSection: readout "%.1f dB"; slider 0..1 with square-root taper over
[-60, 0] dB; drag streams, release commits 0xDA; right-click resets to 0 dB.

MasterModeSection: red-tinted slider; MasterVolumeTaper piecewise: 0..-10 dB
in 0.1 steps (100 units), -10..-40 in 0.5 (60 units), -40..-128 in 1.0 (88
units) = 248 units; readout "-inf" at <= -128; 0xD2; right-click -> 0 dB.

### 2.7 Sidebar CPU (`Components.swift:135-144`)

C0: / C1: 40 x 6 bar, blue, red above 90 %, NN% monospaced; from 0x50.

### 2.8 Connection indicator (`Components.swift:149-196`)

6 pt dot green/red; tooltip "Connected" or connectionError or "Not connected.
Right-click the device name to retry." If no devices: red "No Devices". Else a
device popup enabled only with > 1 device; right-click forces reconnect.

### 2.9 Overview / Dashboard (`DashboardView.swift`)

Vertical stack of read-only cards: StereoDashboardCard "STEREO INPUT (USB)"
for inputs 0/1; per SPDIF pair a StereoOutputDashboardCard when both enabled
else single OutputDashboardCard; OutputDashboardCard for PDM when enabled.
Card header: colour dot, name, right-aligned "Delay: N ms". Body: 10
DashboardRows: band number, type code (OFF/PK/LS/HS/LP/HP/NO/AP or
shortLabel) coloured when active, freq Hz, +-gain dB (if usesGain), Q (peaking
only), "-" when inactive. Stereo cards have a left-to-right gradient stroke
blending the two channel colours.

### 2.10 Input channel page (`ContentView.swift:792-821`)

InputChannelHeader (56 pt), three sections: Link pill "Link 1/2".."Link 7/8"
(hidden when partner not live; mismatch alert "Inputs N and M don't match"
with Keep INn / Keep INm / Cancel); Preamp slider -60..+10 dB + ValueField
"dB" (drag streams, release commits 0xD0, right-click resets 0, mirrors to
linked partner); Clear PEQ button ("Clear 1/2 PEQ" when linked).
FilterListView: 10 PEQ bands; Linkwitz Transform hidden on inputs; edits
mirror to linked partner.

### 2.11 Output channel page (`ContentView.swift:959-1040`)

ChannelSettingsView (68 pt), four panels: InputRoutingPanel (per input: o/*
connect toggle + name coloured when connected; ValueField dB crosspoint gain
(scroll step 0.5, right-click 0); INV phase toggle orange when inverted; all
write 0x70); GAIN ValueField + slider -60..+10 dB (0x74; right-click 0);
DELAY ValueField ms + slider 0..maxDelay (42 on RP2040, 85 otherwise; 0x78);
MUTE toggle button red when muted (0x76).

Tabs PEQ | XO when firmwareSupportsCrossover. PEQ -> FilterListView on
channelData with onClear clearPEQBands; XO -> FilterListView on xoverData in
crossover mode, types [flat] + crossover types, onClear clearCrossoverBands.

### 2.12 FilterListView (`Components.swift:1249-1408`)

Header row: PEQ: # | TYPE (140) | FREQ (104) | GAIN (84) | WIDTH (74). XO: # |
FAMILY (110) | TYPE (84) | SLOPE (80) | FREQ (104). 18 pt leading column when
bypassSupported.

Footer: BypassAllControls split pill "Enable All" | "Bypass All" (each half
greys when no-op; off bands excluded; XO "Bypass All" confirms "Bypass this
output's crossovers?" / "...can damage unprotected drivers such as
tweeters."); "Clear All" (confirm "Clear All Bands?"); PEQ/XO toggle.

FilterRowView (24 pt): BypassCheckbox (solid disc armed, hollow ring bypassed
or Off; 0xD8; hidden when unsupported); index 1..10; Type menu (hierarchical:
Off, Peaking, Low Shelf > 6/12 dB/oct, High Shelf > ..., High Cut > ..., Low
Cut > ..., Notch, All Pass > 180/360, Linkwitz Transform); FREQ ValueField Hz
(width 80, scroll 10, min 10); GAIN dB (width 60, 3 dp; spacer for gainless
types); Q (width 50, min 0.1, 3 dp, trailing zeros stripped; hidden for
crossover and first-order types). Linkwitz row replaces numerics with a
config button opening a popover (width 300): Driver f0 (Hz, step 1, min 10)
and Q0 (step 0.01, min 0.1); Target fp and Qp; DC boost readout
40*log10(f0/fp) dB, orange above 15 dB with warning glyph; Revert / Apply
(staged draft); qp goes over the wire x512 in an 18-byte 0x42. XO row:
FAMILY popup (Off / Linkwitz-Riley / Butterworth / Bessel) | TYPE (LP/HP) |
SLOPE (order x 6 dB/oct) | FREQ.

FilterType gating: flat 0, peaking 1, lowShelf 2, highShelf 3, lowPass 4,
highPass 5 always; notch 6 (fw >= 1.1.4); allPass 7; allPass1 8 (wire V13);
lowShelf1 9 / highShelf1 10 (V14); linkwitzTransform 11 (V22, outputs only);
lowPass1 12 / highPass1 13 (V28); crossovers 32..63 (LR2/4/6/8, BW1..8,
BES2/4/6/8 x LP/HP) (V11+).

ValueField: click-to-edit numeric with scroll-wheel increment; params label,
width, scrollStep, minValue, maxDecimals, stripTrailingZeros.

### 2.13 Matrix Mixer (`MatrixMixerView.swift:119-833`)

Column headers per visible output: editable name + OUT n descriptor coloured;
context menu Identify | Rename | Copy Parameters | Paste Parameters. ROUTING
band: in 8-channel mode "Direct 1:1" (route each input to matching output and
disable the PDM sub) and "Clear" (disconnect every crosspoint). Input rows
(78 pt): name coloured; in 8-channel mode a CompactGainField input trim bound
to preampDB[input]; dividers group stereo pairs. Crosspoint cells:
connect/disconnect, gain value, INV phase; cells of a disabled output
desaturated; orange outline for a PDM/Core-1 conflict. OUTPUT rows (30 pt):
ENABLE (power icon; blue / orange-dim conflict / grey; 0x72), GAIN
(CompactGainField 0x74), DELAY (CompactDelayField, max 42/85; 0x78), MUTE
(red when muted; 0x76). Conflict alerts: enabling PDM -> "Outputs first-last
will be disabled. Are you sure?" Enable PDM / Cancel; enabling an EQ-worker
output -> "The PDM output will be disabled. Are you sure?" Disable PDM /
Cancel. Scroll-wheel adjust on gain and delay fields.

### 2.14 Crossfeed (`CrossfeedView.swift:62-395`)

Header: title "Crossfeed", subtitle "BS2B Bauer Stereophonic-to-Binaural",
master toggle 0x5E. FREQUENCY RESPONSE graph (CrossfeedCurveView; "Disabled"
when off). OUTPUT PAIRS chips 1..N on crossfeedOutputMask (0xFC); Presets
menu: All pairs / Pair 1 only (Headphones) / None; caption "Crossfeed only
the stereo output pairs feeding headphones. Speaker pairs stay bit-accurate.
The mono sub is never crossfed." PRESET radio rows: Default "700 Hz / 4.5 dB
- Balanced, most popular"; Chu Moy "700 Hz / 6.0 dB - Stronger spatial
effect"; Jan Meier "650 Hz / 9.5 dB - Natural speaker-like"; Custom
"User-defined parameters" (0x60). PARAMETERS (opacity 0.5 unless Custom):
Cutoff Frequency Hz 500..2000 (editing switches to Custom; 0x62; caption
"Simulates head shadow lowpass cutoff. Lower = more bass crossfeed. Typical:
650-700 Hz."); Feed Level 0..15 dB (0x64; caption "Crossfeed attenuation
below direct signal. Higher = more crossfeed. Typical: 4.5-9.5 dB.").
Interaural Time Delay toggle "Simulates ~220 us path difference via all-pass
filter" (0x66).

### 2.15 Loudness (`LoudnessView.swift:111-350`)

Header "Loudness Compensation" / "ISO 226:2003 Fletcher-Munson", toggle 0x58.
COMPENSATION CURVE graph (legend "Curve at -40dB"; "Disabled" placeholder;
ISO 226 table at :6-44, computation :85-107). OUTPUTS chips on
loudnessOutputMask (0xFA); Presets: All outputs / Slot 1 only (Headphones)
(0x0003) / None; caption "Compensate only the outputs feeding your low-level
listening chain...". PARAMETERS: Reference SPL dB 40..100 (0x5A; "SPL at 1
kHz when USB volume is 0 dB. Lower = more compensation per dB of volume
reduction."); Intensity % 0..200 (0x5C; "Scales the ISO 226 compensation.
100% = standard curve. 0% = bypassed. >100% = exaggerated."). CustomSlider
defined here (:355-396) and reused by every DSP window.

### 2.16 Volume Leveller (`VolumeLevellerView.swift:44-345`)

Header "Volume Leveller" / "Upward Dynamic Range Compression", toggle 0xB4.
CHANNELS chips (when showMasks); Presets: All channels (Night mode) / Center
only (Dialog boost) / Front L / R only (0xDE). PARAMETERS: Amount % 0..100
(0xB6; "Compression strength. Higher values reduce dynamic range more
aggressively."); Speed segmented Slow / Medium / Fast (0xB8; dynamic
caption); Max Gain dB 0..35 (0xBA; "Maximum boost for quiet passages. Higher
values risk amplifying noise."); Gate Threshold dB -96..0 (0xBE; "Silence
gate. Signals below this level are not boosted..."); Lookahead toggle "Adds
5ms latency. Improves transient handling." (0xBC).

### 2.17 Psychoacoustic Bass (`PsychoacousticBassView.swift:65-441`)

Header "Psychoacoustic Bass" / "Phantom fundamental bass enhancement", toggle
0x30; unsupported banner "Requires firmware with wire format V23 or newer."
SPECTRUM graph (marks fc and 4fc; "Disabled"). STARTING POINTS "Apply preset"
menu: Bookshelf speakers (Gentle low-end help: 60, 0, 6, 50, 0); Small
Bluetooth (Portable speaker: 100, 3, 9, 40, -12); Laptop / tablet (Tiny
drivers, protect them: 180, 6, 12, 50, -24); Headphone bass feel (Extra sub
sensation: 45, -3, 6, 30, 0) as cutoff, harmonics, drive, character,
original. OUTPUTS chips; Presets: All outputs / Exclude sub (recommended) /
None (0x3C). PARAMETERS: Cutoff Frequency Hz 30..300 step 1 (0x32);
Harmonics dB -24..+12 step 0.5 (0x34); Drive dB 0..18 step 0.5 (0x36);
Character % 0..100 with end labels "Warm" / "Aggressive" (0x38); Original
Bass dB -60..0 step 1 (0x3A).

### 2.18 Stereo Upmixer (`UpmixerView.swift:58-467`)

Header "Stereo Upmixer" / "Derive Centre and Surround from stereo", toggle;
gates "Requires an RP2350 device with firmware wire format V25 or newer." and
"The upmixer runs on stereo input at 48 kHz or below." STATUS: dot + text "No
device connected" / "Active - processing audio" / "Idle: upmixer disabled" /
"Idle: input is not stereo" / "Idle: sample rate above 48 kHz"; when active
live gauges Correlation (%+.2f), Centre gain (%), Ls gain (%), Rs gain (%)
from 0x4E. ENGINES: Centre segmented Off (2) / Sinner (0) / Logician (1);
Surround Off (0) / Sinner (1) / Logician (2); caption on PLII vs Hafler.
CENTRE: Strength % 0..100; Centre Width % 0..100; Presence dB -12..+12 step
0.5; Logician only: Correlation Threshold % 0..95, Attack ms 1..500, Release
ms 5..2000 step 5, Detector HPF Hz 20..1000 step 5. SURROUND: Delay ms 0..20
step 0.5; Band-limit HPF Hz 20..2000 step 5; Band-limit LPF Hz 1000..20000
step 100; Decorrelation % 0..100. ROUTING note: "row 2 = Centre, row 3 =
Left Surround, row 4 = Right Surround ... a centre crosspoint gain of -3 dB
is a safe start". All via 0x4C param ids 0-13.

### 2.19 Test Signals (`TestSignalsView.swift:273-1057`)

Header "Test Signals" / "Onboard measurement signal generator"; unsupported
notice. SIGNAL grid of 16 tiles with waveform glyphs:

| Tile | Name | Blurb | Params (unit, range, default) | Timing |
|---|---|---|---|---|
| Sine | Sine | Pure tone, THD approx -139 dB | Frequency Hz 1..30000 (1000) | continuous |
| Square | Square wave | Band-limited (polyBLEP) square | Frequency Hz (100) | continuous |
| White | White noise | Uniform white noise | - | continuous |
| Pink | Pink noise | -3 dB/oct, level-safe normalized | - | continuous |
| Log Swp | Log sweep | Exponential sweep for room measurement | Start / End Hz (20 / 20000) | sweep |
| Lin Swp | Linear sweep | Linear frequency sweep | Start / End Hz | sweep |
| Step Swp | Stepped sweep | Discrete tones stepping up the band | Start, End, Steps/octave 1..24 (3), Dwell ms 20..10000 (250) | sweep |
| Impulse | Impulse | Single-sample unit impulses | Period ms 10..60000 (500) | pattern |
| Clicks | Alternating clicks | Clicks with alternating polarity | Period ms | pattern |
| Polarity | Polarity pulse | Positive half-sine lobe per period | Pulse width ms 1..100 (5), Period ms | pattern |
| Burst | Tone burst | Sine bursts with raised-cosine edges | Frequency, On cycles 1..1000 (8), Off cycles 0..1000 (8), Edge cycles 0..100 (2) | pattern |
| 2-Tone | Tone pair | IMD test pair (SMPTE / CCIF) | Tone 1 (60), Tone 2 (7000), Ratio A1/A2 x0.1..10 (4) | continuous |
| Multi | Multitone | Log-spaced tones, Schroeder phases | Tones 2..16 (10), Low, High | continuous |
| ISP | ISP test | Inter-sample-peak over patterns | Pattern 0..1 | continuous |
| Chan ID | Channel ID | Counted pentatonic blips per channel | Blip length ms 30..1000 (120) | pattern |

OUTPUTS: All / None buttons; chips per output; click selects, click again
inverts polarity (o with slash); caption "Click to select, click again to
invert polarity. Dimmed outputs are disabled in the matrix mixer and stay
silent." LEVEL: dB -80..0 dBFS; caption "Peak level in dBFS. Output trim,
master volume and mute still apply downstream." PARAMETERS: one ValueField
per used param with unit (Hz, ms, cyc, x); ISP uses segmented "fs/4 - +3.01
dBTP" / "fs/6 - +1.25 dBTP"; Tone pair adds presets "SMPTE 60/7k" and "CCIF
19k/20k"; Multitone adds "Up to N tones on this device." TIMING: sweep: Sweep
length (s, min 0.01), Repeat ("0 = repeat forever"), Gap between sweeps (ms);
pattern: Repeat ("Pattern periods. 0 = repeat forever", or for Channel ID
"Passes over the selected outputs. 0 = forever"), Extra gap per period (ms);
continuous + walk: Dwell per channel ("0 = 2 s default"), Passes;
continuous: Duration ("0 = play until stopped"). OPTIONS: Bypass output EQ
(RAW) "Skips crossover and PEQ on the selected outputs. Trim, master volume,
mute and delay still apply." (FLAG_RAW); Decorrelate channels (noise only;
FLAG_DECORR); Walk outputs one at a time (FLAG_WALK; forced on for Channel
ID). Transport: status title/detail; Stop-immediate ("Stop immediately, no
fade") + Stop (red) while running, else Start; Space toggles. Start blockers:
"No device connected" / "Firmware has no signal generator" / "Select at least
one output" / "Sweep length must be greater than 0".

### 2.20 Stats for Nerbs (`StatsView.swift:649-1060`)

Sections: Device Information (Platform, Firmware, Serial, Reconnects); System
Information (4 rows: sample rate, core1 mode, etc.); S/PDIF Input (State
badge, Active Source, Sample Rate, Lock Count, Loss Count, Parity Errors,
FIFO Fill, RX Pin; Channel Status: Format, Audio, Category, Word Length,
Copy; Debug: Library State, Stable Callbacks, Lost Callbacks); LG Sound Sync
(Enabled, Present, TV Volume, TV Mute); ADAT Bulk Output (RP2350: State,
Streaming, Rate Supported, Data Pin, Resync Count, Slip Count); I2S Input
(Slave Clock) (State, Detected Rate, Measured Rate, Lock Count, Loss Count);
PDM (Subwoofer) over/under; Audio Output; SPDIF DMA Starvation (Total +
delta badge, per-instance, "Time since last event", "Time between last
two"); Buffer Fill Levels (Audio Streaming / PDM Active indicators, per
buffer fill % and min-max watermark band); "Reset Watermarks" (0xB1);
footer "Updated every 2 seconds". Sources 0xB0, 0xE2, 0xE3, 0xE8, 0xCE, 0x8A.

### 2.21 Interrupt Monitor (`InterruptMonitor.swift:1044-1124`)

Toolbar Pause/Resume, Clear, status Inactive / Paused / Listening, "N
events". Body: decoded event log with offset-to-field-name decoder.

### 2.22 Graph pop-out

BodePlotView + GraphLegend; optional independent visibility map.

### 2.23 AutoEQ browser (`AutoEQ/AutoEQBrowser.swift`)

Search field "Search headphones..."; rows: form-factor icon, name, source
capsule (oratory1990 orange, crinacle purple, rtings blue, innerfidelity
green, else grey), form factor, heart favourite. Empty states "Loading
headphone database..." / "No headphones found matching ..." / error. Bottom
bar: selection summary or "Select a headphone to apply its EQ profile";
Cancel (Esc) / Apply (Return).

### 2.24 Settings: Overview (pin map) (`DSPi_ConsoleApp.swift:439-637`)

Disconnected placeholder "No Device Connected" / "Pin assignments live on
the device. Connect a DSPi to see which GPIOs are in use." Empty: "No GPIOs
are currently claimed." Summary: "N of M GPIOs in use", "N free"; 9-column
grid of every valid GPIO chip, role-tinted when claimed, grey when free,
tooltip owner or "GPn - available". One section per role: Outputs / Clocks /
Inputs / Control / Other, 2-column grid of GPn chips + owner label. Role
tints: output #0278c7, clock #ba3822, input #04856f, control #9543a7, utility
#807701.

### 2.25 Settings: About

App name, "USB Audio DSP Controller", version, "Made with love by Weeb
Labs"; Links & Support: YouTube, GitHub, Discord, Patreon, Ko-fi.

### 2.26 Settings: Graphing (app-side)

Graph Appearance: Graph Line Glow; Show Phase Response; Unwrap Phase
(disabled unless phase on). Response Curve: Line Width 1.0..4.0 step 0.5;
Animation Speed 0.1..0.5 step 0.05. Scale & Grid: Show Frequency Grid / Show
Frequency Labels / Show dB Grid / Show dB Labels; Vertical Range 10..100 dB;
Center -40..20 dB; Min Frequency 10/15/20/50/100 Hz; Max Frequency 5/10/20
kHz. Pop-out Window: "Pop-out graph follows channel selection". Defaults:
line width 2.0, animation 0.2, phase off, unwrap off, range 50, center 0,
height 250, min 15 Hz, max 20 kHz.

### 2.27 Settings: Advanced

Channel Names: "Reset all channel names to factory defaults." + Reset.
Diagnostics: Show Debug Information toggle.

### 2.28 Settings: Global Parameters (staged draft, applied on Save)

Startup Preset: Mode Specified Default (0) / Last Used (1); Default Preset
picker "n: <name|Empty>"; caption "Choose which preset loads when the device
powers on." (0x96). External Mute Control (when dacHwMuteSupported): orange
warning "Adjust only with audio stopped" / "Changing these settings while
audio is playing can send a loud pop or full-level transient..."; Enable
Automatic Mute ("Briefly mute an external DAC or amplifier to suppress loud
pops during system state changes."; auto-assigns first free GPIO); Polarity
Active Low / Active High; Mute Pin picker (free GPIOs); Hold Time 5/10/20/
50/100 ms; Release Time 0/5/10/20/50/100 ms; Test row + Start ("Toggle
automatic mute for one second to confirm hardware configuration."; disabled
when no pin / disabled / draft dirty; tooltip "Save your changes first to
test the mute pin."; 0xEC); config 0xEA. Master Volume: Mode Independent /
With Preset + caption (0xD4). Hardware Configuration: Mode Independent / With
Preset + caption (0x98).

### 2.29 Settings save bar

While hasPendingChanges: orange glyph, "Unsaved changes", subtitle "Your
controls are live now; saving keeps them across a reboot." (CS-only) or
"Saving writes these settings to the device's flash."; progress while
csBusy; Revert and Save. Three dirty categories: global draft, output config
live edits not yet flashed (only in OUTPUT_CONFIG_MODE_INDEPENDENT), Control
Surfaces live preview (device-reported). Save issues startup / master volume
mode / output config mode / DAC mute config, then 0x52, then 0x9D. Revert
re-applies the captured OutputConfigSnapshot and issues 0x9E.

### 2.30 Settings: Outputs

Slots: per pin output: colour dot, title OUT 1/2 .. OUT 7/8 or Sub, "Default"
capsule when type and pin are factory, Type picker S/PDIF (0) / I2S (1) (PDM
row fixed PDM; STM32 constraints), 0xC0; Pin picker GPIO n (hidden on STM32),
0x7C. Bulk Output (when adatSupported): Enable ADAT ("Stream all 8 output
channels as one optical ADAT lightpipe (44.1/48 kHz, 24-bit). Runs alongside
the existing outputs; drive a TOSLINK transmitter from the data pin.") 0xCA;
Serial Data pin ("GPIO driving the ADAT optical output. Default GPIO 12.")
0xCC. Reset Pins button. Inline statusRow for PIN_CONFIG_* results.
validPins = 0..22, 26, 27, 28. Default data pins [6,7,8,9].

### 2.31 Settings: Inputs

S/PDIF Input: Instances picker 1..4 ("N selectable input(s) sharing one
receiver"); per instance row "S/PDIF n" (or "SPDIF RX"), "GPIO pin for S/PDIF
input n (TOSLINK RX module or comparator)." + pin picker (0xE4 / 0xE9;
defaults 5, 20, 21, 22); LG Sound Sync toggle ("Decode the LG TV's TOSLINK
volume + mute signaling and apply it as the host volume - TV remote becomes
the volume control. Per-preset; saved with the active preset."; unsupported
note) 0xE6. I2S Input: Clock Mode Master / Slave ("Master: DSPi drives
BCK/LRCLK. Slave: an external master drives the clocks and the rate is
auto-detected."; confirm alert Change Clock Mode / Cancel) 0x88; Lock Status
row (slave); Channels 2/4/6/8 ("N stereo pair(s) of 24-bit audio,
sample-aligned") 0xF3; Serial Data n pin per pair ("GPIO data pin for input
channels 2n-1..2n"; defaults 4, 16, 17, 18) 0xF1. ADAT Input: Enable ADAT
Input ("Receive 8 channels of 24-bit audio (44.1/48 kHz) from one TOSLINK
optical input into input channels 1-8. Assign a data pin below, then select
ADAT as the input source.") 0x68; Serial Data picker with "Not set" (0xFF)
0x6A; Clock Mode Master / Slave 0x6C; warning "Clock is free-running" with
inline Enable ADAT Output button; Lock Status row.

### 2.32 Settings: I2S Configuration (hidden on STM32)

BCK Pin picker + note "LRCK: GPIO bck+1 (BCK + 1)" (0xC2 role 0); Clock Pins
Unified / Split ("Unified: Master and Slave modes share pins." / "Split:
Separate pins for Master and Slave modes.") 0xFE; Slave BCK Pin (Split; 0xC2
role 1); Master Clock (MCK) enable 0xC4; MCK Pin restricted to CLK_GPOUT pins
0xC6; MCK Multiplier 128x / 256x with "Locked to 128x at <rate>" note 0xC8;
Input Sample Rate 44.1 / 48 / 96 kHz 0xED.

### 2.33 Settings: Control Interfaces

UART: header "Enable UART", "Asynchronous 3.3V serial link, fixed 8N1
framing.", status pill Active / Inactive / Disabled, toggle; warning when
enabled but not live ("Enabled in flash but not running - its pins likely
collide with the current output wiring..."); TX Pin (pin % 4 == 0, free), RX
Pin (pin % 4 == 1, free), Baud Rate over UART_CTRL_BAUD_CHOICES ("9600 -
1000000. Must match the controller."), Push Notifications toggle ("Stream
live parameter/preset/format changes to the controller (type-0x40 frames)
instead of polling."), Apply / Revert with "Unapplied changes" (0xF5). I2C:
"Enable I2C Target", "Device acts as an I2C slave; the controller is bus
master. Poll-only (no async notifications)."; SDA Pin (even), SCL Pin (next
odd), Target Address hex + stepper 0x08..0x77, Apply / Revert (0xF7). Footer
about USB-only configuration and pull-ups; "External control protocol version
N." Status strings: "<IFACE> configuration applied and saved" / "A pin is
out of range or lacks the required <IFACE> mux function" / "A pin is already
claimed by another output or interface" / "Baud rate is out of range (9600 -
1000000)" / "Address is out of range (0x08 - 0x77)".

### 2.34 Settings: Control Surfaces (`DSPi_ConsoleApp.swift:2094-6701`)

Entirely caps-driven; 16 slots. Placeholders: "Reading control-surface
capabilities from the device..." / "No Device Connected" / "No Controls
Configured" ("Wire a button, switch, knob, encoder, or LED to a spare GPIO
and bind it to a device function.") + Add Control menu; "All N control slots
are in use." Types: Push Button, Toggle Switch, Potentiometer / Fader, Rotary
Encoder, Indicator LED, Dimmable LED, IR Remote, Display.

Per-slot card (collapsible): header with type badge, type-change menu, name
field, status pill, remove. Parameter (noun) menu grouped by category with
names: Volume, Master Volume, Mute, Loudness, Crossfeed, Volume Leveller,
Preset, Input Source, Clipping / Clear Clipping, EQ Bypass, LG Sound Sync,
Crossfeed Preset, Crossfeed ITD, Leveller Amount/Speed/Lookahead, Input
Preamp, Output Gain/Mute/Enable/Delay, Filter Frequency/Gain/Q/Type/Bypass,
Test Signal, DAC Mute Test, Channel Clipping, Channel Level, S/PDIF Lock,
Sample Rate, USB Streaming, ADAT Active, LG Source Present, LG Muted, Upmixer
+ Centre Mode/Surround Mode/Strength/Width/Presence, Psychoacoustic Bass +
Cutoff/Harmonics/Drive/Character/Original Level, Preset Reload, Loudness
Reference SPL, Loudness Intensity, Input Signal Level, CPU Load, Show Page,
Allow Editing, Browse/Adjust. Action picker: Adjust, Step, Up, Down, Toggle,
Set value, Follow position, Trigger, Indicate, Hold, Indicate above, Show
level. Event picker (buttons): Press / Long press / Double press. Target
picker sectioned Channels / Groups; Band/Index picker (Band 1.., Crossover
1..4). Pin rows (pots ADC 26/27/28; encoders two pins) with per-type detail.
Operand rows: value / bool / enum / span (Lo & Hi) / step / brightness
ceiling with units. Delay rows on/off with warning "Applying, reverting, or
rebooting briefly releases the pin and restarts the timing from off. Driving
an amplifier trigger, that is a power cycle." Flags: Reverse, Acceleration,
Wrap Around, Repeat While Held, Link Absolute, Group All (Match Members
Exactly), Invert with per-type title (Active-Low LED / Pull-Down Wiring /
Idle-Low Receiver / Active-High Wiring). Apply / Revert per slot (0x84,
0x8B).

IR Remote sub-section under an IR card: "Remote Buttons" n/max; "Apply the
receiver above before learning remote buttons."; Add Remote Button; per
command: code chip "<PROTOCOL> 0xNNNNNNNN" or "Not learned"; Learn with
Cancel; noun / action / target / operand / step; flags Wrap Around, Repeat
While Held; remove (0x8D, 0x8F).

Display sub-section under a Display card: model picker, I2C pair picker
(GPIO n / n+1), address (Default or 0xNN); mode One page / Cycle Dashboard /
Cycle All; home page; alignment Left / Centre / Right for label and value;
seconds rows (dwell, overlay hold, edit timeout); brightness; flags
OVERLAY_ANY, EDIT_GATED with warning "Nothing can arm editing, so a
Browse/Adjust control can only browse pages. Bind a button or remote key to
Allow Editing."; Pages list with Add Page, per page noun, Large value, Level
bar, target (Channels / Groups), delete; "All N page slots are in use.";
Panel State status row. Footer wiring rules and "Control-surface capability
version N."

### 2.35 Settings: Channel Groups

Empty: "No Channel Groups Configured" / "Name a set of channels so one
control can drive them together - a stereo pair, a zone, every output at
once." + Add Group. Per group: name, kind Inputs / Outputs / All Channels,
Members toggle grid ("No channels of this type on the connected device."),
"Used by N control(s). Emptying this group or changing its channel type
deactivates them until it fits again.", "Pick at least one channel.", Revert
/ Apply, remove. "All N group slots are in use." Footer on relative vs
absolute moves and "Match Members Exactly".

### 2.36 Settings: Macros

Empty: "No Macros Configured" / "Run a short sequence of changes from a
single press: select an input and load a preset, switch monitors, mute after
a delay." + Add Macro. Per macro: name, ordered numbered step rows (move,
delete), Add Step ("A macro holds up to N steps."), Revert / Apply, remove.
Per step: noun, action, target (Channels / Groups), index, operand, delay,
Wrap Around. Footer: "One macro runs at a time - firing another cancels the
first at its current step."

## 3. Menus and shortcuts

App: About; Settings... (Cmd-,). Edit: Cut/Copy/Paste/Select All; Copy/Paste
fall through to channel parameter clipboard. File: Import Filters... (Cmd-I);
Export Filters... (Cmd-E; default "DSPi Filters.txt"); Import Device
Configuration... (.dspipreset); Export Device Configuration... (default "DSPi
Configuration.dspipreset"); Save Master Volume (0xD6); Save Output
Configuration (0x52). AutoEQ: Browse Profiles... (Shift-Cmd-B); Favorite
Profiles submenu (+ Clear Favorites); Update Database... Tools: Commit
Parameters... (confirm "Save current parameters to preset slot N?"); Revert
to Saved... ; Factory Reset... (critical confirm; 0x53); Firmware Update...
(critical confirm; Option skips; 0xF0; hidden on STM32); Matrix Mixer
(Shift-Cmd-M); Loudness (Shift-Cmd-L); Crossfeed (Shift-Cmd-X); Psybass
(Shift-Cmd-P); Upmixer (Shift-Cmd-U); Leveller (Shift-Cmd-V); Test Signals
(Shift-Cmd-G); Stats (Shift-Cmd-T); Interrupt Monitor (Shift-Cmd-I). Space =
Start/Stop in Test Signals. Return/Esc default/cancel in sheets.

## 4. Cross-cutting behaviours

Multi-device: availableDevices; picker enabled with > 1; right-click
reconnect; switchToDevice prompts unsaved changes; async flows scoped by a
generation counter; Settings drafts discarded on a different serial, kept on
re-plug of the same.

Connect/disconnect: dot + "No Devices"; controls dim (0.4) and disable;
Settings falls back to About; a disabled output selection falls back to
overview.

Notifications: always-on bulk endpoint reader; applyNotifiedParamChange
decodes EQ bands, crossover bands, channel names, dac_hw_mute, user_volume
(this is how the OS volume slider syncs into the app via PARAM_SRC_UAC1),
lg_sound_sync.enabled, input_config.input_source (switch-complete trigger).

Dirty state: (1) preset dirty from PresetSnapshot diff, shown as `*` on the
active slot; (2) Settings save bar; (3) CS device-owned dirty. Unsaved
Changes alert: "The current preset has unsaved changes:\n\n<summary>\n\nSave
before continuing?" Save / Discard / Cancel; summary is a bulleted diff
capped with "and N more"; triggered on preset switch, copy-to, device
switch, quit, window close.

Undo: none. Confirmations state "This cannot be undone"; staged drafts with
Revert.

.dspipreset: export default "DSPi Configuration.dspipreset"; import shows
provenance (platform, firmware, date), cross-platform warning "This file
came from a X device and you are connected to Y. Anything the connected
device doesn't have will be skipped.", two checkboxes off by default
"Volume levels (master and listening volume)" and "Hardware I/O (GPIO pins,
clocks, ADAT, inputs)"; progress sheet; result "Applied N channels, N EQ
bands, N crossover bands, N crosspoints." + "Not present on this device:
..." + "Skipped: ..." + "These changes are live but not yet stored on the
device. Save them to a preset slot to keep them."

Filter files: import REW or DSPi format; single-channel picker "Found N
filter(s)[ and a +-X.X dB preamp]. Select which channel(s) to apply them
to:"; multi-channel picker "This file contains filter settings for multiple
channels. Select which channels to import:"; unsupported types skipped;
export via generateExportString.

AutoEQ: browse, favourite, apply; Favorite Profiles menu; Update Database
with Rebuild from GitHub (confirm, progress window, "Database rebuilt
successfully!\nEntries: N") / Import File... (.json) / Reset to Built-in.

Clipping: firmware sticky flags OR'd into clipLatched; red zone at meter
end; auto-clear after timeout then 0x83.

Errors: no toasts; modal alerts (Error / Success / Information), inline
status rows (green check / orange triangle / "Unapplied changes"),
persistent orange banners.

Channel clipboard: copy/paste channel params from row context menus, matrix
column menus, Cmd-C/V.

## 5. Visual design

Colour: ChannelPalette.swift single source. Inputs 0-7: FL (0.29,0.56,0.89)
blue; FR (0.96,0.45,0.45) red; FC (0.45,0.78,0.55) green; LFE
(0.93,0.70,0.30) amber; BL (0.60,0.55,0.92) violet; BR (0.90,0.55,0.78) pink;
SL (0.40,0.78,0.82) teal; SR (0.80,0.72,0.42) olive. Outputs 0-7: teal
(0.27,0.76,0.64); green (0.35,0.82,0.50); amber (0.94,0.77,0.35); orange
(0.95,0.65,0.30); blue (0.35,0.55,0.95); light blue (0.55,0.70,0.95); rose
(0.85,0.45,0.55); pink (0.95,0.60,0.65). PDM purple (0.73,0.53,0.95). Legacy
quirk: outRight maps to output(2).

Settings badge tints in OKLCH L 0.52-0.57, chroma <= 0.125. Pin-role tints
five hues ~72 degrees apart, L 0.53-0.56. AutoEQ source capsules as above.
Semantic: red = mute / clip / master slider / destructive; orange = warning,
INV, conflict, unsaved glyph; green = active / live; blue = output enable;
accent = selection and armed bypass.

The same colour follows a channel everywhere: pill, meter, curve, matrix
labels, dashboard border and dot, type-code text, Settings output dot.

Typography: dark only. 9 pt captions / axis / section labels; 10 pt bold
section headings (PARAMETERS, OUTPUTS, ...); 10 pt mono dashboard rows; 11 pt
matrix labels; 12 pt medium parameter titles; headline for window titles;
monospaced digits for every numeric readout. Cards: control background 0.6
opacity, radius 10, 1 pt grey 0.2 stroke.

Graph: log-x min..max; linear-y center +- range/2; major freq grid at 100 /
1k / 10k (white 0.15), minor (0.06); dB grid step 1/3/5/10 by span; 0 dB at
0.3; labels 9 pt white 0.4; phase overlay dotted light grey on +-180 deg
scaled with vertical zoom; glow option; spring animation; scroll-wheel
vertical zoom 10..100; height 200..350.

Identical-curve grouping: channels with bit-identical magnitude arrays
(output gain folded in first) collapse into one stroke drawn as a gradient
of member colours at 1.25x width; groups without the selected channel are
dashed [6, 4].

Disabled / unsupported: absent features are removed, not disabled (Settings
pages, filter types, XO tab, bypass checkbox, Source picker, ADAT/LG/multi
SPDIF sections); present-but-old firmware shows a banner; disconnected =
opacity 0.4 + no hit testing + placeholder cards; inactive-but-present =
desaturated (disabled outputs saturation 0 opacity 0.3; muted meters 0.4;
non-Custom crossfeed params 0.5); claimed GPIO = saturated tint + white
text, free = secondary 0.12 fill; status pills Active (green) / Inactive
(orange) / Disabled (secondary); "Default" capsule on factory hardware rows.
