# Phase B9 brief: refinements to existing screens

Read `docs/plan/archive/briefs/common-b.md` first. Phases B1 to B8 and B11 are
merged; `git log --oneline -40` shows them.

## Scope

1. **Response magnitude.** The Console changed `DSPMath.magnitudeSquared`
   to the sin^2(w/2) form (Console commit 9c33a44) because narrow
   low-frequency peaks were drawn at 0 dB: the naive
   `|H(e^jw)|^2` evaluation loses precision when w is small. Find the
   Terminal's biquad magnitude code (`crates/dspi-tui/src/curves.rs` or
   the session crate) and test it with a narrow peak (for example 20 Hz,
   Q 10, +12 dB at 48 kHz and 96 kHz): if the drawn peak is wrong, adopt
   the Console's formulation (read it in `DSPMath.swift`) and pin it with
   the test. If it is already right, keep the test and say so.
2. **Graphing settings.** The Console added "Show Frequency Readout" and
   "Show Gain Readout" (default on) and "Grid Opacity" 0..200 % (default
   50 %, disabled when both grids are off). Find what the Terminal's
   graph shows at the cursor (`h` / `l`) and in its grid, and add the
   equivalents to Settings > Graphing, persisted in the config file: the
   two readout toggles if the Terminal has those readouts, and a grid
   choice of Off / Dim / Normal standing in for opacity (0 %, below 100 %,
   100 % and up), defaulting to Dim to match the Console's 50 %.
3. **Stats.** The Console's System Statistics is now three columns:
   device, system, audio output and PDM; then SPDIF DMA starvation and
   buffer fill levels; then S/PDIF input, LG Sound Sync, ADAT and I2S
   input when present. Buffer fill rows became history graphs of about 15
   s (256 samples). Read `StatsView.swift` (around 1266-1306 for the
   history). In the Terminal: use columns when the width allows (keep the
   single column at 80x24), show buffer fill history as a sparkline per
   row from the samples the Stats poll already reads (every 2 s, so keep
   the last 8 or so; say in the report how many), and show the firmware
   build info (B1 added `Capabilities::build_info`) in the device section.
   Use "-" rather than an em-dash for missing values, as the Console now
   does.
4. **Dashboard cards per row.** The Console's overview has a "DASHBOARD
   LAYOUT" choice: Auto, 1, 2, 3 cards per row (Auto fits as many as a
   440 pt minimum allows). Add the equivalent to the Terminal's overview
   grid as a Graphing setting (Auto default) if the grid has a column
   count to set; if the grid already adapts to width in a way that makes
   this meaningless, explain and skip.
5. **Write rate.** The Console caps drag traffic at 30 writes per second
   and commits on release. Check what happens in the Terminal when an
   arrow key is held on a gain, frequency, threshold or level: count the
   writes per second against the mock transport with key repeat at a
   typical 30 Hz. If writes can queue behind each other or exceed what
   the device should take, coalesce them (send the latest value at most
   every 33 ms, and always send the final value), with a test.
6. **Crossfeed output mask width.** `cf.outputs` is sent as 2 bytes; the
   header says a uint8 pair mask (config.h:582). The firmware reads only
   byte 0, so it works, but send 1 byte to match the header, with a test.

Grey ghosts for bypassed bands are dropped: the Terminal draws only a
channel's combined curve, not per-band contributions (B3 found this).

## Exit

The common checks pass; tests for each item done; golden frames updated
for Stats and Settings > Graphing at both sizes.
