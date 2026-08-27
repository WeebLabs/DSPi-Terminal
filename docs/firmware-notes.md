# Firmware notes

Observations about the DSPi firmware and its protocol that are not written down
elsewhere, or where the released documentation disagrees with the source.

**Kept here deliberately.** The firmware repository is not modified as part of
this work; these are our working notes.

Baseline: `WeebLabs/DSPi` `release/v1.1.6` @ `112f35b`.

---

## 1. `commands.md` is fourteen wire versions behind the headers

On `release/v1.1.6`, the documentation and the source disagree:

| File | States |
|---|---|
| `Documentation/commands.md` | wire **V14**, 3664 bytes, 121 opcodes, "Master L / Master R" |
| `firmware/DSPi/bulk_params.h` | wire **V28**, **5944 bytes** |
| `firmware/DSPi/config.h` | **202** opcodes |

Entirely normal for an actively developed firmware, and the reason this project
generates its constants from the headers rather than transcribing the document.
The gap widened at v1.1.6 rather than closing.

`commands.md` remains excellent and authoritative for **behaviour**: transport
conventions, the write-as-read opcode list, deferred writes and the busy window,
the flash blackout budget, and the hazard analysis in section 22. It is only
layout and counts that have drifted.

## 2. The V16 channel model change

V16 is described in the header as "unified channel model (inputs are first-class
channels with PEQ + metering; no master); matrix/preamp direct (8 inputs);
compat-breaking, no migration."

Consequences that bite a host:

- There is **no master channel**. Channel index space is
  `[inputs][outputs]`, so the first output is at index `num_input_channels`.
- That is **2 on RP2040 and 8 on RP2350**, so `channel = output + 2` from the
  older documentation is wrong on RP2350.
- RP2350 has **17 channels** (8 in, 9 out), not 11.
- The matrix is **8 x 9**, not 2 x 9.
- Every input channel has its own PEQ bank and peak meter.

This is why `ChannelMap` is the only code permitted to convert between index
spaces.

## 3. Bulk parameters are strict, not tolerant

`bulk_params.h` states that `bulk_params_apply()` rejects any payload whose
`format_version` is not current or whose length is not exactly
`sizeof(WireBulkParams)`. There are no legacy size anchors and no per-section
version gates.

So the usual host strategy of "send a shorter prefix and let older fields
default" does **not** apply, and `commands.md`'s description of accepting
versions 2 through 14 is obsolete. A host may only bulk-write a packet that
exactly matches the device's version and size, and must otherwise fall back to
individual `SET_*` opcodes, which remain version independent.

## 4. Reserved-byte sentinel encodings

Several V21-V24 fields were added by claiming previously reserved bytes, using
`0 = absent, keep the live value`. Where zero is itself a legal value the field
is stored **plus one**.

A host that writes a plain zero into these is not leaving them alone; depending
on the field it either changes nothing or disables an input. Full list in
`docs/wire-format.md` section 5.

`i2s_clock_mode` (V21) is the exception: it is a plain 0/1, because a pre-V21
reader seeing zero decodes it as master, which is the correct legacy default.

## 5. `mck_multiplier` is encoded differently in two places

- In the bulk packet's `i2s_config` section it is the **literal value**, 128 or 256.
- The vendor command `GET_MCK_MULTIPLIER` (0xC9) returns an **index**, 0 or 1.

Both are correct in their own context; conflating them yields a plausible but
wrong value.

## 6. IR learn is three-valued

`REQ_CS_IR_LEARN` (0x8F) takes `wValue = 1` to arm, `0` to cancel, and `2` to
**read an 8-byte result** carrying state, protocol, and the decoded code. The
constant list in `config.h` only hints at the third. The macOS Console polls
`wValue = 2` until the state leaves `ARMED`, which is the pattern to follow.

## 7. The macOS vendor interface is exclusive

On macOS, IOKit's interface open is exclusive, so **DSPi Console and this tool
cannot hold the device simultaneously**. Attempting it yields a permission-style
error from `nusb` that has nothing to do with permissions.

The error message is platform-specific for this reason: on Linux the same class
of failure almost always means a missing udev rule, which is a completely
different fix.

Worth confirming whether Windows and Linux behave the same way before relying on
concurrent access anywhere.

## 8. `SIGGEN_MULTITONE_MAX` is platform-conditional

8 on RP2040, 16 on RP2350. The build-time generator detects the conflicting
definition and refuses to emit it, so it cannot become a compiled-in assumption.
It must come from `SIGGEN_GET_CAPS` instead, which reports it per device.

---

## 9. `max_bands` is storage depth, not the number of bands

The bulk header's `max_bands` field reports **12**, and `bulk_params.h`
describes it as "bands per channel in this payload". It is the width of the
wire array, not a count of anything usable. `config.h` is explicit:

```c
#define MAX_BANDS        12
// MAX_BANDS (12) is the PEQ storage depth; only bands 0..9 are active today.
// ...
// Band indices in [channel_band_counts[ch] .. XOVER_BAND_BASE) are rejected
// by the vendor handlers today.
```

So the live count is **10**, held per channel in `channel_band_counts[]`, and
`REQ_GET_EQ_PARAM` stalls for bands 10 and 11. Confirmed on hardware: every
channel of an RP2350 at 1.1.5 answers for bands 0-9 and stalls for 10-11.

There is no opcode that reports the count. `channel_band_counts` is firmware
internal and is not carried in the bulk packet. Taking the header at face value
therefore offers the user two bands per channel that silently do nothing — which
this app did until it was measured.

`probe_band_count()` measures it instead: the boundary is monotonic, so a binary
search finds it in four transfers. That also means a firmware that grows its PEQ
(the comment notes room to reach 20 bands) is picked up without a code change.
`Capabilities::max_bands` is the live count; `band_storage` is the wire depth,
which is what a bulk packet is indexed by.

---

## 10. The bulk packet is the only fast way to read the EQ

There is no whole-band read opcode: `REQ_GET_EQ_PARAM` returns one scalar per
transfer, so a band costs five transfers and a full RP2350 costs 850. Measured
on hardware, that is **24 seconds** — a control transfer averages 3 ms, and the
firmware's own busy-window stalls push the total well past the arithmetic.

The `eq` section of the bulk packet is the same table, and the whole 5944-byte
snapshot arrives in six chunked transfers in **20 ms**. All 170 live bands decode
identically to what the scalar path reports, so this is a strictly better read.

Anything that wants more than a band or two should use `Session::snapshot()`.
The same applies to the `crossovers` section, which mirrors the EQ layout at
four columns instead of twelve.

Note the addressing differs between the two paths, which is easy to get wrong:
crossover bands are wire indices **20-23** for `GET`/`SET_EQ_PARAM`, but columns
**0-3** of the `crossovers` section in the bulk packet.

---

## 11. V28 changed the wire's shape without changing its size

The change that matters most in the v1.1.5 to v1.1.6 bump is invisible to every
size check. `WireInputConfig` grew `spdif_rx_pin_ext` from two entries to three
for the fourth selectable S/PDIF input, so `spdif_rx_enabled_ext_p1`,
`i2s_clock_mode`, `adat_input_pin`, `adat_input_enabled_p1` and
`adat_clock_mode_p1` all moved down one byte, and the section's last reserved
byte was consumed. The section is still 16 bytes, the packet is still 5944, and
`bulk_size_matches_firmware` passes untouched.

`bulk_params.h:34` says so plainly, which is why the bump procedure's first step
is to read the `WIRE_FORMAT_VERSION` comment rather than to trust the tests. The
offsets are now pinned by name in `wire::INPUT_CONFIG_FIELDS` and its test, so
the next shift of this kind fails the build instead of misparsing a GPIO as a
clock mode.

## 12. The CS caps header grew because a component type was added

`REQ_GET_CS_CAPS` with `wValue = 0xFFFF` answers `CsCapsHeader`, whose type table
is `CS_TYPE_COUNT` rows of 4 bytes. Caps v10 added `CS_TYPE_DISPLAY`, taking the
count from 8 to 9 and the header from 40 bytes to 44. The four post-table maxima
(`max_ir_commands`, `max_groups`, `max_macros`, `max_macro_steps`) therefore sit
at `4 + 4*type_count`, which is the offset the header itself documents at line
568. A host that hardcodes 36 reads the display type's descriptor as
`max_ir_commands` and gets a plausible small number.

This is the mechanism the firmware calls "the documented self-describing
mechanism": the table is meant to grow, so nothing may index past it by a
constant.

## 13. `CsStatusPacket` grew for the same reason, one version earlier

Caps v6 doubled `CS_MAX_IR_COMMANDS` from 8 to 16. `ir_active_mask` widened from
one byte to two and `ir_cmd_status` from 8 entries to 16, taking the packet from
22 bytes to 41. Both counts come from the caps header, never from a constant:
`max_bindings` sizes `slot_status` and `max_ir_commands` sizes `ir_cmd_status`.

## 14. The S/PDIF enable mask's bit 0 is input 1, not input 2

`survey-firmware.md` section 1.22 says `REQ_GET_SPDIF_INPUT_CONFIG` (0xEF)
returns an enable mask whose "bit0 = input2". Both `config.h:456` and
`vendor_commands.c:3401-3411` say otherwise: the response is
`(spdif_rx_enabled_ext << 1) | 1`, so **bit 0 is input 1 and is always set**, and
bits 1..3 are the optional inputs 2..4.

The bulk packet's `spdif_rx_enabled_ext_p1` is the *other* mask: it carries only
`spdif_rx_enabled_ext`, so its bit 0 **is** S/PDIF 2. The two are one bit apart
and both are called "the enable mask".

## 15. The first-order pass filters follow the firmware's one-pole

`FILTER_LOWPASS1` (12) and `FILTER_HIGHPASS1` (13) arrived at v1.1.6. Like the
first-order shelves before them they are a one-pole TPT state-variable section,
not a degenerate RBJ biquad: `dsp_pipeline.c` sets `g = tan(pi*f/fs)` with no
prewarp and takes `lp` for the low pass and `in - lp` for the high pass.

The biquad fallback the same file uses above `fs/7.5` is the identical transfer
function once `1 + cos(w)` is divided out, so one set of coefficients draws both
paths. Unlike the shelves, these two need no `A` prewarp, because they have no
gain; they also read neither `Q` nor `gain_db`, so offering either in a UI would
show a control that does nothing.

## 16. The Console writes these as `LP1` and `HP1`

`DSPi Console/DSPMath.swift` gives every filter type a `shortLabel` and writes it
verbatim into exported filter files. The two new types are `LP1` and `HP1`.

Reading that file also settled an older bug here: `filterfile.rs` mapped `LSC`
and `HSC` to the *first-order* shelves, while the Console and `autoeq.rs` both
read them as REW's spelling of the second-order ones. A REW file's shelves were
being imported at half their slope. They now agree.

## 17. The Core 1 EQ-worker range is derivable, so nothing needs a platform test

`CORE1_EQ_FIRST_OUTPUT` and `CORE1_EQ_LAST_OUTPUT` (config.h:764-770) are
platform-conditional, so `build.rs` refuses to emit them and no generated
constant exists. They do not need one. The first is 2 on both platforms, and the
last is always the output below the PDM sub: 7 of nine outputs on RP2350, 3 of
five on RP2040. `num_outputs - 2` gives both, from the topology the probe already
discovered, which is what the Console computes from `platformName` instead
(`DSPViewModel.swift:2124-2126`).

The same holds for the pin outputs: `NUM_PIN_OUTPUTS` (config.h:667-673) is one
per S/PDIF slot plus PDM, and a slot carries a stereo pair, so
`(num_outputs - 1) / 2` slots and one more for PDM covers 5 on RP2350 and 3 on
RP2040 without naming either.

## 18. A blocked output enable answers success

`REQ_SET_OUTPUT_ENABLE` reports no failure when Core 1 is already running the
other side of the PDM / EQ-worker interlock; the firmware skips the enable and
says nothing (survey-firmware 6.8). The status byte is not evidence, and neither
is the absence of a stall. Only reading the enable back distinguishes "on" from
"politely ignored", which is why `enable_output` reports `Rejected` rather than
trusting the write, and why the plain `out.enable` write path asks
`REQ_GET_CORE1_CONFLICT` (0x7B) first.

## 19. The Console's own preset apply moves the MCK pin while MCK is running

`PresetDocumentTransfer.swift:618-619` writes the MCK enable and then the MCK
pin, in that order. When a document asks for MCK enabled on a different pin, the
enable lands first and the pin move then hits `PIN_CONFIG_OUTPUT_ACTIVE`
(config.h:611), because the firmware refuses to move a clock output that is
running (survey-firmware 6.7). The Console records the refusal and carries on, so
the pin silently stays where it was.

Our apply drops MCK first when the pin has to move, moves the pin, and applies
the document's enable state last. This is a deliberate departure from the
reference implementation, and the only one in that sequence.

## 20. Two opcodes cannot be reached through a registry row

`REQ_SET_I2S_BCK_PIN` (0xC2) packs a *role* into wValue's high byte
(config.h:334-336): 0 is the unified or master pair, 1 the SPLIT-mode slave pair.
The registry addresses the master pair only, and the GET (0xC3) documents no role
at all, so the slave pin can be written but not read back scalar-wise; it lives
in the bulk packet as `bck_pin_slave` (bulk_params.h:165).

`REQ_GET_DAC_HW_MUTE_CONFIG` (0xEB) answers 16 bytes, but the registry types
`dev.dacmute` as a packet whose `Repr::Raw` reads one. The write path's own
readback therefore cannot confirm it, and reports a correct write as a
rejection. Both are read directly from the transport in `preset_file.rs` until
Phase 2B gives them codecs.

## 21. The `.dspipreset` master-clock field is the multiple, not the wire value

The shared schema stores `mckMultiplier` as 128 or 256
(`PresetDocument.swift:394`). The wire carries a selector: 0 is 128x and 1 is
256x (`REQ_SET_MCK_MULTIPLIER`, config.h:342-343). Writing the document's number
through would be out of range for the choice; the two are mapped explicitly.
