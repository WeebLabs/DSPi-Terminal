# Phase B4 brief: the output limiter on the output page

Read `docs/plan/archive/briefs/common-b.md` first. Phases B1 (protocol, registry
rows `limit.*` on opcode 0x81) and B2 (`DeviceState` limiter records,
meters, snapshot diff, output-config dirty tracking) are merged. Read `git
log` for their commits.

Specification: `docs/plan/survey-firmware-beta4.md` sections 1.5, 2
(`WireLimiterParams`) and 3.4; `docs/plan/survey-console-beta4.md` section
1.3; the Console's `OutputLimiterView.swift` and `Components.swift:772-905`
for every string, range, step and state.

## Scope

On the output page (`crates/dspi-tui/src/screens/output.rs`), next to
Mute, as the Console places it. Shown only when the device reports the
limiter feature.

1. **Indicator** with three states: off (grey), on (accent), and orange
   while gain reduction is at least 0.05 dB. It is a row element that can
   take focus: Space toggles the limiter, Enter opens its settings.
2. **Settings** (a popup or an expanded section, whichever fits the
   Terminal's existing patterns for an inline editor on the output page):
   header "Output Limiter" plus the output name and an on/off switch;
   Threshold (-30..0 dBFS, step 0.5, 1 decimal, end labels "-30 dBFS" /
   "0 dBFS", default -1.0); Release (10..1000 ms, step 10, default 100);
   Link group (Off, 1, 2, 3, 4) with the Console's summary line ("Not
   linked." / "Linked with ..." / "No other outputs in group N."). All
   settings dim and are disabled while the limiter is off.
3. **Actions**: "Copy to all outputs" (threshold, release and enable via
   output 0xFF; not the link group); the "All outputs" menu with "Link
   all stereo pairs" (outputs 1+2 in group 1, 3+4 in group 2, and so on up
   to group 4; the PDM output stays unlinked; derive pairs and PDM from
   `DeviceState`, not constants), "Unlink all outputs" and "Switch every
   limiter off". Use the Console's exact labels.
4. **Gain reduction**: poll the meter (0x81 index 0x80) only while the
   output page is showing and at least one limiter is on, at the meter
   interval, through a function on the pattern of `poll_upmix_status` in
   `live.rs`. Show the current reduction for this output next to the
   indicator.
5. **Ganging**: do not reimplement the firmware's link-group rules. The
   Terminal re-reads after every write; rely on it, and test with the
   mock transport that a write on one linked output updates its partner
   in `DeviceState` after the re-read.
6. **Caption**: say, in one sentence in the Console's style, that turning
   on the first limiter adds 32 samples of latency to every output and
   briefly fades all outputs. Check whether the Console says anything
   similar and reuse its text if so.
7. **Page command bar** (`;` on the output page, DESIGN section 13): add
   `limit on|off`, `limit <dBFS>` (threshold), `release <ms>`, `link
   <0-4|off>`, with live hints, following how `gain -3` is implemented.
8. **Persistence wording**: in INDEPENDENT output-config mode the save bar
   must show the output configuration as unsaved after a limiter edit (B2
   provides the tracking); confirm on screen.

## Exit

The common checks pass; golden frames of the output page with the limiter
off, on, and reducing, and of the settings; tests for the command-bar
words, the stereo-pair linking on RP2350 (9 outputs) and RP2040 (5
outputs) fixtures, and the dimmed settings when off.
