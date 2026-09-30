# Phase B8 brief: the spectrum analyser

Read `docs/plan/archive/briefs/common-b.md` first. Phase B1 (protocol) is merged:
the RTA codecs (`RtaConfig`, `RtaCaps`, band-centre chunks,
`RtaBandFrame`, the bin-frame header, `RtaStatus`) and the RTA feature
probe exist. Read `git log` for its commits.

Specification: `docs/plan/survey-firmware-beta4.md` sections 1.2, 3.2 and
6 (gotchas 1 to 4); `docs/plan/survey-console-beta4.md` section 1.4; the
Console's `SpectrumAnalyser.swift` (engine, especially `tick` at
612-773), `SpectrumAnalyserView.swift` (window at 1488-1902) and the
settings page `DSPi_ConsoleApp.swift:1620-1782`; the firmware's `rta.h`
and `rta.c` in `/Users/weeblabs/DSPi/firmware/DSPi/`.

This is the only feature that adds continuous USB traffic. The Terminal's
loop is single-threaded and does USB synchronously inside `Live::tick`
(`crates/dspi-tui/src/live.rs`, `run` around line 2246; timing in
`perf.rs`). Parameter writes and meters must not be slowed by the
analyser.

## Scope

### 1. Engine (`crates/dspi-session/src/rta.rs`, new)

A transport-driven engine that the TUI calls from its tick, testable
against the mock transport:

- On connect, read caps (reject a version other than 3) and the band
  centres. No caps means no feature.
- Hold the wanted configuration (tap, channel mask, FFT order, averaging,
  peak decay, flags) and push it only when it differs from the applied
  one, re-reading 0x09 to confirm (a SET never STALLs over USB); give up
  after 3 failed attempts per configuration and report "Device refused
  this configuration".
- Read band frames with 0x0F when several channels are selected and 0x0B
  for one; a STALL on 0x0F during a bulk session is a skipped frame, not
  an error.
- Read bins only when a single channel is selected, at most every
  max(frame interval, 50 ms), in chunks, comparing the head sequence
  byte with the tail byte and discarding a torn frame.
- Read status and config every 0.5 s.
- A frame older than max(500 ms, 4 x refresh interval) is shown as
  silence.
- Keep the analyser alive by reading data; when the last view closes,
  send STOP (0x0E wValue 0).
- Time-average bins and apply 1/6-octave smoothing on the host, as the
  Console does (`GraphSpectrumOverlay.swift:483`); level bytes convert
  with `(v - level_zero) * 0.5` dBFS using caps.
- A time budget: measure how long each tick's analyser transfers take
  with the real transport, and cap the analyser at a share of the tick
  (for example one band read and at most one bin chunk per tick) so a
  parameter write is never queued behind a frame read. Note the numbers
  you measure against the mock and state in your report what should be
  measured on hardware (HW-2b).

### 2. Panel (`screens/spectrum.rs`, key `A`)

Titled "Spectrum Analyser", shown when the RTA feature is present.

- Inputs / Outputs switch; channel selection chips in each channel's
  colour; the Console's summary ("Spectrum hidden" / "1 channel" / "N
  channels") and Clear.
- Curves / Bars / Both. Curves: one channel shows smoothed FFT bins
  blended from the bass bands below 200 Hz; several channels show the
  third-octave band curves. Bars: third-octave bars per channel. Peak
  hold drawn as a contour or marker when on. Use the existing braille
  graph (`graph.rs`) with a dB axis from the Floor and Ceiling settings.
- The status line: Running / Idle, refresh description, "N frames/s",
  "transform N us", device load, and the notices "Device refused this
  configuration" / "shaded bands below N Hz need a larger transform size
  in Settings" / "N/M dB range", in the Console's words.
- Notices: "Spectrum analyser unavailable", "No channels selected".

### 3. Settings > Display > Spectrum Analyser

A new page after Graphing, always listed (showing the Console's orange
notice when the firmware has no analyser): Spectrum Strength (30..100 %,
step 5, default 100, mapped to the Terminal's dim / normal rendering if a
percentage has no terminal meaning; say how in the report), Peak Hold
(on), Smoothing (on), Floor (-60 / -90 / -120 dB, default -90), Ceiling (0
/ +6 / +12 dBFS, default +6), Transform Size (from caps, default the caps
default), Averaging (Off / 50 / 125 / 300 ms / 1 s / 3 s, default 300),
Peak Decay (Off / 4 / 12 / 30 dB/s, default 12, disabled without Peak
Hold). The footer reads "This device reports N dB of usable range and
transforms up to N points." Store these in the Terminal's config file
alongside the Graphing settings and push the engine values on connect.

### 4. Not in this phase

The overlay behind the response graph and the RTA strip above the pages
(decision 3: later, off by default). Structure the engine so they can be
added without changing it.

## Exit

The common checks pass; engine tests against the mock transport for caps,
config push and confirmation, the 3-retry refusal, band frames via 0x0B and
0x0F, a 0x0F STALL, torn-bin rejection, staleness, and STOP on close;
golden frames for the panel (unavailable, no channels, curves one channel,
curves several, bars) and the settings page at both sizes.
