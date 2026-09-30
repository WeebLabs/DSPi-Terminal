# Phase B11 brief: `.dspipreset` interoperability with the Console

Read `docs/plan/archive/briefs/common-b.md` first. Phases B1 and B2 are merged;
B2 added subharm, tube and limiter blocks to `.dspipreset`
(`crates/dspi-session/src/preset_file.rs`).

## The defect

Phase B2 reported that the Terminal writes and matches a channel entry's
`channelId` as its own unified channel index, while the Consoles (macOS at
`/Users/weeblabs/DSPi Console`, and the Linux/Windows Console at
`/Users/weeblabs/DSPi Console for Linux`, if it has the same format) use
a different numbering (reported as the Windows ids: outputs 2..10, extra
inputs 11..16) together with `eqChannel`, `inputIndex` and `outputIndex`.
If so, EQ, gain and delay from a Console file land on the wrong channels
in the Terminal, and the reverse. B2 also reported that the import skips
several masks and modes: the psybass mask, the crossfeed and loudness
masks, and the upmix modes.

## Scope

1. **Establish the format exactly** from the Console source
   (`PresetDocument.swift`, `PresetDocumentTransfer.swift`, and the
   encoder around `PresetDocument.swift:705-711`): how `channelId` is
   numbered on RP2040 and RP2350, what `eqChannel`, `inputIndex` and
   `outputIndex` mean, which are written and which are read, and how the
   Console places a channel entry on import. Check the Linux Console too
   and note any difference. Write this down as a short section in
   `docs/wire-format.md` or a new `docs/preset-format.md`.
2. **Fix export and import** so a file written by the Terminal places
   every channel correctly in the Console, and a file written by the
   Console places every channel correctly in the Terminal, on both
   platforms. Write the same keys the Console writes.
3. **Carry every field the Console carries**: the psybass output mask,
   the crossfeed and loudness masks, the upmix modes, and anything else
   the Console reads that the Terminal drops. List each gap you find and
   close it.
4. **Files written by older Terminal builds**: if the old numbering can be
   told apart from the Console's (for example by `meta` fields), keep
   reading it; otherwise say so in the report and state what happens.
5. **Tests**: hand-built Console-format documents for RP2040 and RP2350
   (as B2 did for the new blocks) that import to the right channels;
   Terminal exports that match the Console's key set and numbering; a
   round trip on both platforms.

## Exit

The common checks pass; the tests above; the format written down.
