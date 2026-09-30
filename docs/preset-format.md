# The `.dspipreset` document

A `.dspipreset` is a whole-device configuration as JSON: schema 1 of the
Windows Console's `PresetDocument.cs`, extended additively by the macOS
Console (`PresetDocument.swift`, `PresetDocumentTransfer.swift`, DSPi Console
`9dbb07a`). The Terminal reads and writes it in
`crates/dspi-session/src/preset_file.rs`. The Linux Console
(`DSPi Console for Linux`, `fadeac4`) has no `.dspipreset` support at all: its
`core/src/preset.rs` only drives the device's own preset slots.

## Shape

Top-level keys: `schemaVersion` (1; a larger number is refused), `meta`,
`global`, `loudness`, `crossfeed`, `leveller`, `psybass`, `upmix`, `subharm`,
`tube`, `channels`, `matrix`, `io`. `psybass`, `upmix`, `subharm` and `tube`
are absent when the source device lacked the feature, which is different from
present and disabled. Values are the firmware's raw wire numbers (filter
types, masks, modes, pins), never names. Every reader is lenient: a missing
key takes the Console's default, which is the firmware's (for example
`loudness.outputMask` 0xFFFF and `crossfeed.outputPairMask` 0x01), never zero.

`meta` carries `name`, `savedUtc` (ISO-8601 UTC), `appVersion`, `platform`
(`RP2040` or `RP2350`), `firmwareVersion`, `wireFormatVersion`,
`inputChannelCount`, `outputChannelCount`, and the Console's additive
`masterVolumeMode` and `outputConfigMode`. Nothing in `meta` gates an import.

## Channel numbering

This is the part that decides where every EQ band, gain and delay lands.

Each entry in `channels` carries up to four identities:

| Key | Meaning | Written by |
|---|---|---|
| `channelId` | The Windows Console's channel id (table below) | Windows, macOS, Terminal |
| `eqChannel` | The firmware's unified channel index (config.h:788-790) | macOS, Terminal |
| `inputIndex` | Wire input index 0..7, on input entries only | macOS, Terminal |
| `outputIndex` | Matrix output index, on output entries only | macOS, Terminal |

The Windows ids were laid down when the device had two inputs; outputs grew in
the middle and the extra inputs went above them:

| Channel | Windows id |
|---|---|
| Input 0, 1 | 0, 1 |
| Input 2..7 | 11..16 |
| Output *o* | 2 + *o* (RP2040 PDM, output 4, is id 6; RP2350 PDM, output 8, is id 10) |

The unified index is inputs from 0 and outputs from `CH_OUT_1 =
NUM_INPUT_CHANNELS`. On RP2040 (2 inputs, 5 outputs) the two numberings
coincide. On RP2350 (8 inputs, 9 outputs) they agree only for inputs 0 and 1:
Windows id 2 is output 0, while unified index 2 is input 2.

**Placing an entry** (the macOS Console's `ChannelBlock.ref(platform:)`,
PresetDocument.swift:674-684), read against the connected device:

1. An output entry with `outputIndex` goes to that output; an input entry
   with `inputIndex` goes to that input.
2. Otherwise `channelId` is resolved through the table: 0..1 and 11..16 are
   inputs, anything else from 2 up is output `id - 2`.
3. A channel the device does not have is reported as "Not present on this
   device", never moved elsewhere. A duplicated channel takes the last entry.

`eqChannel` is written but no reader places a channel by it. `isOutput` is not
consulted by the Consoles when resolving `channelId`.

The matrix (`input`, `output`) and `global.inputPreampsDb` (by wire input) use
wire indices directly and need no translation.

## Files from older Terminal builds

Terminal builds before this format fix wrote the unified index as `channelId`
and none of the index fields. On RP2040 that is the same as the Windows id, so
those files were always correct. On RP2350 they are recognised by their
content: no entry carries `eqChannel`, `inputIndex` or `outputIndex`, every
entry agrees with the unified numbering (id below `meta.inputChannelCount`
exactly when it is an input), and at least one entry contradicts the Windows
table (an input at 2..10, or an output at 11..16). No Console-written file can
pass that test, because every Console entry agrees with the table. Such a file
is read by the unified numbering in the Terminal. The Consoles do not know this
rule: they will place an older Terminal RP2350 file by the Windows table, so
inputs 2..7 land on outputs 0..5 and outputs 8..16 on outputs 6..8 or nowhere.
Re-export it from the Terminal to fix it.

## What an import applies

In the Console's order (PresetDocumentTransfer.swift:295-667): output enables
(every disable, then every enable, an enable that would collide on Core 1
reported rather than forced); each channel's name, delay, output gain, mute and
`outputDelayMs`, EQ and crossover (a non-empty bank replaces the channel's,
the rest going flat; an empty one leaves it alone) and each input's preamp;
the matrix; bypass, LG Sound Sync; loudness, crossfeed, leveller, psybass,
subharm, tube and upmix, each with its parameters and mask before its switch.
Volume levels are opt-in, and master volume is written only when the device's
master-volume mode is with-preset. Hardware I/O (pins, clocks, inputs, the
limiters inside each output entry) is opt-in. The input source goes last.
`global.inputPairLinked` is app state; the Terminal applies it to its own pair
links.
