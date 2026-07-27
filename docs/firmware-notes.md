# Firmware notes

Observations about the DSPi firmware and its protocol that are not written down
elsewhere, or where the released documentation disagrees with the source.

**Kept here deliberately.** The firmware repository is not modified as part of
this work; these are our working notes.

Baseline: `WeebLabs/DSPi` `release/v1.1.5` @ `9776c2f`.

---

## 1. `commands.md` is twelve wire versions behind the headers

On `release/v1.1.5` itself, the documentation and the source disagree:

| File | Last commit | States |
|---|---|---|
| `Documentation/commands.md` | `f7143dc`, 2026-07-12 | wire **V14**, 3664 bytes, 121 opcodes, "Master L / Master R" |
| `firmware/DSPi/bulk_params.h` | `1b84765`, 2026-07-19 | wire **V26**, **5944 bytes** |
| `firmware/DSPi/config.h` | `4ab5922`, 2026-07-19 | **190** opcodes |

The code moved on 19 July; the doc last moved on 12 July. Entirely normal for an
actively developed firmware, and the reason this project generates its constants
from the headers rather than transcribing the document.

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
