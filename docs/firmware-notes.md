# Firmware notes

Observations about the DSPi firmware and its protocol that are not written down
elsewhere, or where the released documentation disagrees with the source.

**Kept here deliberately.** The firmware repository is not modified as part of
this work; these are our working notes.

Baseline: `WeebLabs/DSPi` `release/v1.1.6` @ `557bce7` (v1.1.6 beta 4, wire
V32). Sections 1 to 27 were written against `112f35b` (beta 2, wire V28) and
still hold unless a later section says otherwise; sections 28 onwards cover
what beta 3 and beta 4 changed for a host.

---

## 1. `commands.md` is fourteen wire versions behind the headers

On `release/v1.1.6`, the documentation and the source disagree:

| File | States |
|---|---|
| `Documentation/commands.md` | wire **V14**, 3664 bytes, 121 opcodes, "Master L / Master R" |
| `firmware/DSPi/bulk_params.h` | wire **V32**, **6136 bytes** |
| `firmware/DSPi/config.h` | **244** opcodes |

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
therefore offers the user two bands per channel that silently do nothing, which
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
on hardware, that is **24 seconds**: a control transfer averages 3 ms, and the
firmware's own busy-window stalls push the total well past the arithmetic.

The `eq` section of the bulk packet is the same table, and the whole snapshot
(5944 bytes when this was measured, 6136 from V32) arrives in six chunked
transfers in **20 ms**. All 170 live bands decode
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
at `4 + 4*type_count`, which is the offset the header itself documents
(control_surfaces.h:644 at `557bce7`). A host that hardcodes 36 reads the
display type's descriptor as `max_ir_commands` and gets a plausible small
number. It grew again at caps v18, by two rows for the auxiliary outputs, to 52
bytes; see section 33.

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
---

## 22. Four packets are not the same size in both directions

Most vendor structures are one record used both ways. These four are not, and
each asymmetry is invisible to a size check that only looks at one direction:

| Opcode | SET takes | GET answers | The difference |
|---|---|---|---|
| `REQ_PRESET_SET/GET_STARTUP` (0x96/0x97) | 2 B | 3 B | the third byte is `last_active`, read-only |
| `REQ_SET/GET_CS_DISPLAY_CFG` (0x27/0x28) | 12 B | 16 B | the read prepends `{max_pages, model_count, rsvd[2]}` |
| `REQ_SET/GET_CS_MACRO` (0x22/0x23) | 36 B | 132 B | the SET is the header alone; steps go one at a time |
| `REQ_SET_EQ_PARAM` (0x42) | 16 or 18 B | 16 B | the 18-byte form carries the Linkwitz `qp` sidecar |

The display one is the trap: parsing the config from offset 0 of the *reply*
reads `max_pages` as the mode and `model_count` as the home page, which is a
plausible-looking config rather than an error. `CsDisplayCfgReply` exists so
that offset cannot be got wrong.

`max_pages` is also the only place the display's page count is published: it is
not in the CS caps header. A host that sizes its page loop by
`CS_MAX_DISPLAY_PAGES` reads sixteen slots from a device that may have fewer.

## 23. The display's deferred tag collides with its own page 0

`cs_last_slot` tags a display config SET as `0x50` and a page SET as
`0x50 | page` (control_surfaces.h:780-782), so the config and page 0 are the
same byte. Every other namespace is disjoint: bare `n` is a binding, `0x80|n`
an IR sub-slot, `0x40|n` a group, `0x60|n` a macro, `0xFF` a save or revert.

That is only safe because a host never has two display SETs in flight. The
macOS Console serialises them on a queue for exactly this reason, and
`surfaces.rs` writes them one at a time and waits for each.

## 24. Two delay scales, ten times apart

`CsBinding.on_delay` / `off_delay` are in **0.1 s** units
(control_surfaces.h:387-391) and `CsMacroStep.pre_delay` is in **10 ms** units
(control_surfaces.h:469). Both are `uint16` and both are called a delay, so
mixing them up is a factor of ten in either direction with nothing to notice it
by. One second is 10 in a binding and 100 in a macro step.

## 25. `SiggenTypeDesc.name` is NUL-padded, not NUL-terminated

The eight name bytes (siggen.h:166) may be entirely filled, so there is no
terminator to look for and a reader that insists on one runs into the
`timing_model` byte. Read it as "up to eight bytes, stop at the first NUL if
there is one".

The same struct's `SiggenParamDesc` entries are 13 bytes each and start at
offset 10, so every float in the table is unaligned. That is fine on the wire
and fatal to a `load::<f32>` on a platform that cares.

## 26. Two bytes of `SpdifRxStatusPacket` are documented as reserved and are not

`survey-firmware.md` 6.6 ends the 16-byte packet at `fifo_fill_pct` and calls
bytes 14 and 15 reserved. The Console reads them as the receiver library's
state and its callback count and shows both in Stats. They are decoded here
under the Console's names rather than dropped, since a reserved byte that
carries diagnostics is worth keeping.

## 27. The Console sends a 9-byte matrix route packet

`MatrixRoutePacket` is 8 bytes in `config.h:845-851` and the Console builds a
9-byte `Data` for `REQ_SET_MATRIX_ROUTE`, reading 9 back as well. The trailing
byte is zero and the firmware ignores the excess, so both work; this app sends
the 8 the header defines.

---

## 28. GET_PLATFORM grew to seven bytes, and beta 2 cannot say it is a beta

`REQ_GET_PLATFORM` (0x7F) offers seven bytes from beta 3: the historical four,
then full-width minor and patch, then a beta ordinal (config.h:664-667,
vendor_commands.c:2638-2655). The firmware truncates to `wLength`, so a host
asks for 7 and takes what comes (firmware_versioning_spec.md:109): minor and
patch from bytes 4 and 5 when six or more arrive, the nibbles otherwise, never
mixed; the beta from byte 6 when seven arrive.

Two consequences. First, a short reply is not an error, so the read goes through
`Transport::control_in_upto`, which every other read avoids because a short
reply there means a garbled answer. Second, beta 1 and beta 2 answer four bytes
and so look like the final 1.1.6. The Console reads a short reply that claims
1.1.6 or later as an "early beta"; this app does the same, and prints it as
"1.1.6 early beta". The spec says a short reply is beta 0, which is right only
for releases before 1.1.6.

Order versions by `(major, minor, patch, beta == 0 ? 256 : beta)`
(firmware_versioning_spec.md:111): final is encoded as 0 but comes after every
beta of its patch.

`REQ_GET_BUILD_INFO` (0x80) is new: 64 bytes of `git describe` and build date,
for people only (config.h:326). Older firmware stalls, which the probe treats
as "no build info".

## 29. Beta 2 and beta 3 are refused, by name

The firmware's bulk apply takes exactly its own version and length (section 3),
and the Terminal implements exactly V32, as the Console does. A beta 2 (V28) or
beta 3 (V30) device is therefore refused at connect with a message naming its
firmware and the version to update to. The check reads the version from the
first bulk chunk rather than after the whole read, because an older packet is
shorter and walking V32's length off its end fails with a stall or a short read
that says nothing useful.

## 30. The notification endpoint is paced

From beta 4 (dd5d24e) the endpoint NAKs while idle and sends the one-byte idle
packet only after 100 ms without any packet (`NOTIFY_IDLE_KEEPALIVE_US`,
usb_audio.c:1064-1080). Before, it re-armed idle packets back to back and a
reader spun on it. A read now blocks for up to about 100 ms. The reader's
timeout is 250 ms, longer than the keep-alive so a quiet read ends on the idle
packet rather than on a cancelled transfer, and short enough that shutdown stays
prompt. The mock endpoint can be paced the same way for tests.

`NOTIFY_EVT_CS_AUX` (0x0C, 9 bytes) is new: slot, state, level as 8.8 percent
and source (notify.h:78-83), sent only on a real change.

## 31. Tube, limiter and upmixer parameters are floats, even the booleans

`REQ_SET_TUBE_PARAM` (0x3E), `REQ_LIMITER` (0x81) and `REQ_UPMIX_SET_PARAM`
(0x4C) take a four-byte float for every parameter they address: the enables, the
masks and the enums too (config.h:228-236, upmix.h:185-186). A shorter payload
is a short payload: the upmixer stalls it (vendor_commands.c:1829), the tube and
limiter ignore it silently. Before this bump the registry sent the upmixer's
three switches as one byte; `ParamDesc::repr` now picks the float for all three
opcodes. Enums and masks are rounded to the nearest integer on the way in.

The tube's type table overwrites bias, asymmetry, hardness and sag when a type
is chosen, and editing any of those four sets the type to Custom. Bulk and
preset restore skip the lookup. The type and rectifier rows live only in
`tube.c`, not in a header.

Tube defaults changed between beta 3 and beta 4: drive -6 to -12 dB, the drive
floor -6 to -30, the output stage off to on, resonance 85 to 95 Hz.

## 32. The limiter moves in groups and its errors are silent

`REQ_LIMITER` packs `(output << 8) | index` (limiter.h:11). Output 0xFF is every
output, on a SET only; a GET of 0xFF stalls, so the session refuses it before the
wire. A SET with a bad output, index or a NaN is ignored silently; a GET with
either stalls. Index 0x80 is the gain-reduction meter and 0x81 the status block,
which doubles as the feature probe; both ignore the output byte.

A write to one member of a link group moves the whole group (limiter.c:143-194),
and a write to 0xFF sends one notification per output. The first enable and the
last disable switch a 32-sample lookahead into or out of **every** output under
an 8 ms fade. The limiter follows `OUTPUT_CONFIG_MODE`: in INDEPENDENT mode a
bulk write ignores the limiter section while `BULK_INVALIDATED` still fires, and
the settings persist through `REQ_SAVE_OUTPUT_CONFIG` (0x52) instead. Raw
signal-generator output is limited.

## 33. Control surfaces caps v14 to v20

- The caps header is 52 bytes: `type_count` is 11 with `CS_TYPE_AUX_OUT` (9) and
  `CS_TYPE_AUX_PWM` (10), and `noun_count` is 79 (control_surfaces.h:141-145,
  251, 652).
- `CsBinding` byte 22 is `extras` (control_surfaces.h:468-470): the
  `CS_AUX_X_*` boot and response flags on an aux slot, 0 on every other type.
  Byte 23 stays reserved.
- `CS_UNIT_MS_LOG` (6) is plain integer milliseconds for the value, range and
  `min_q`/`max_q`, and steps in 8.8 octaves like Hz (control_surfaces.h:268-
  269). It exists for the limiter release, 10 to 1000 ms, which 8.8 cannot hold.
- `CS_TARGET_AUX` (5) addresses a binding slot and needs index 0.
- An aux SET (0x04, 0x06) applies at once and never makes the configuration
  dirty; a refusal still ACKs the data stage and shows only in
  `REQ_GET_CS_STATUS` as `CS_STATUS_INVALID_AUX` (0x26).
- `REQ_GET_CS_AUX_STATE` with `wValue = 0xFFFF` answers 48 bytes: sixteen states,
  then sixteen levels as 8.8 percent (config.h:138-140).

## 34. The spectrum analyser is polled and tears

- Off at boot and never persisted. Any band or bin read (0x0B, 0x0C, 0x0F)
  starts it and it stops 5 s after the last one; polling status (0x0D) does not
  keep it alive (rta.h:28, vendor_commands.c:4313-4321).
- `REQ_RTA_SET_CONFIG` (0x08) is validated and applied later. The header says it
  stalls on an invalid config (config.h:147), and the handler clears `handled`
  (vendor_commands.c:1784-1790), but on USB the data stage has already been
  acknowledged by then, so read the config back with 0x09 to confirm.
- 0x0C takes no lock, so a bin frame read in chunks can tear. The frame ends
  with a copy of its sequence number, written as 0xFF first, and sequence
  numbers skip 0xFF (rta.c:128-131, 157-161, 254-256): compare head and tail,
  and discard the frame when they differ or the tail is 0xFF.
- 0x0F builds its reply in the bulk buffer, so it stalls while a chunked bulk
  transfer holds it (vendor_commands.c:4334-4355).
- Level bytes are 0.5 dB steps from `level_zero` in the caps (243 is 0 dBFS,
  rta_fft.h:44-45). Protocol version 3 is incompatible with 2; every analyser
  record is refused unless its version byte is 3.

## 35. Subharm solo is invisible, and the meter scale is 0..32767

`REQ_SET_SUBHARM_SOLO` (0x2C) is runtime only: never persisted, not on the wire
and never notified (config.h:201), so a panel that shows it has to poll 0x2D.
The subharm and limiter meters carry one `u16` per output, 18 bytes on an
RP2350 and 10 on an RP2040, so their length comes from the reply. The subharm
meter's full scale is 32767, the same as the status peaks
(vendor_commands.c:2206-2216, audio_pipeline.c:424).

The header's section comment still gives the band levels as -30..+6 dB
(bulk_params.h:388-392); the firmware limit is +12 (subharm.h:64-65), and the
registry follows the latter.

## 36. A UAC1 volume change on a non-USB input is parked

A volume SET from the operating system while the input is not USB is parked
rather than applied (3422c0c). It emits no PARAM_CHANGED, and
`REQ_GET_USER_VOLUME` and the bulk `user_volume` report only the applied value,
so the change is invisible until the input switches to USB, when it is applied
unless a vendor, control-surface, preset or LG write came in between.
