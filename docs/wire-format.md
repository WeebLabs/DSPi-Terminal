# `WireBulkParams` V32 wire format

*Derived from `crates/dspi-proto/firmware/bulk_params.h`, vendored from
`WeebLabs/DSPi` `release/v1.1.6` @ `557bce7` (v1.1.6 beta 4).*

This document exists because **no released documentation describes this layout**.
`DSPi/Documentation/commands.md` §13 documents wire format **V14 at 3664 bytes**
with a "Master L / Master R" channel model, which the firmware abandoned at V16.
It is eighteen versions stale. This file is derived from the header and is
checked by `dspi-proto`'s offset tests, so it cannot silently drift.

Writing style note: this doc avoids em-dashes, per project convention.

---

## 1. The compatibility rule (read this first)

`bulk_params.h` states it plainly:

> Backward compatibility is intentionally broken at V16 (unified channel model).
> Only the current full-size layout is accepted; there are no legacy size anchors
> or per-section version gates; every section is always present.
> `bulk_params_apply()` rejects any payload whose `format_version != current` or
> whose length `!= sizeof(WireBulkParams)`.

Consequences for a host, and they are strict:

- **`SET_ALL_PARAMS` (0xA1) is all-or-nothing.** You may only bulk-write a packet
  that is exactly V32 and exactly 6136 bytes. There is no "send a prefix and let
  older fields default" path, and no forward compatibility on write.
- **Never bulk-write to a device whose `format_version` you do not implement.**
  Fall back to individual `SET_*` opcodes, which are version-independent, or
  refuse with a clear message. Guessing here corrupts the whole device state.
- **On read, the header is authoritative.** Check `format_version` and
  `payload_length` before interpreting a single offset. A device reporting an
  unknown version must not be parsed against this table.

This inverts the tolerance strategy a host would normally use. The generated
section table (`SECTIONS` in `dspi-proto`) and the per-section field tables in
`wire.rs` still earn their place for notification-offset dispatch, for readable
field definitions, and for the next format change, but they do **not** buy us
cross-version bulk writes. Those do not exist. The Terminal checks the version
in the first chunk of a bulk read and refuses any other, naming the device's
firmware and the version to update to.

## 2. Sizing constants

| Constant | Value | Meaning |
|---|---:|---|
| `WIRE_MAX_INPUT_CHANNELS` | 8 | RP2350 max inputs |
| `WIRE_MAX_OUTPUT_CHANNELS` | 9 | RP2350 max outputs (8 S/PDIF + 1 PDM) |
| `WIRE_MAX_CHANNELS` | 17 | inputs + outputs |
| `WIRE_MAX_BANDS` | 12 | PEQ storage depth (bands 0-9 active today) |
| `WIRE_MAX_XOVER_BANDS` | 4 | crossover bands, vendor indices 20-23 |
| `WIRE_MAX_PIN_OUTPUTS` | 5 | 4 S/PDIF + 1 PDM |
| `WIRE_MAX_SPDIF_INSTANCES` | 4 | |
| `WIRE_NAME_LEN` | 32 | channel and preset names |
| `WIRE_FORMAT_VERSION` | **32** | |
| `sizeof(WireBulkParams)` | **6136** | asserted at bulk_params.c:50-66 |

Channel index space is `[inputs 0..7][outputs 8..16]` on RP2350 and
`[inputs 0..1][outputs 2..6]` on RP2040. **The first output index is
`num_input_channels`, not a constant.** Arrays are always sized at the RP2350
maximum and zero-padded; the header counts say how many entries are live.

## 3. Section offset table

All offsets are byte offsets from the start of the packet. Little-endian
throughout; floats are IEEE 754 single-precision at 4-byte-aligned offsets.

| # | Offset | Size | Section | Contents |
|--:|-------:|-----:|---------|----------|
| 1 | 0 | 16 | `header` | see §4 |
| 2 | 16 | 16 | `global` | f32 preamp_gain_db; u8 bypass; u8 loudness_enabled; u16 loudness_output_mask (V19+); f32 loudness_ref_spl; f32 loudness_intensity_pct |
| 3 | 32 | 16 | `crossfeed` | u8 enabled; u8 preset; u8 itd_enabled; u8 output_pair_mask (V20+); f32 custom_fc; f32 custom_feed_db; u32 rsv |
| 4 | 48 | 16 | `legacy` | f32 gain_db[3]; u8 mute[3]; u8 rsv |
| 5 | 64 | 68 | `delays` | f32 delay_ms[17] |
| 6 | 132 | 576 | `crosspoints` | `WireCrosspoint[8][9]`, input-major, 8 B each |
| 7 | 708 | 108 | `outputs` | `WireOutputChannel[9]`, 12 B each |
| 8 | 816 | 8 | `pins` | u8 num_pin_outputs; u8 pins[5]; u8 rsv[2] |
| 9 | 824 | 3264 | `eq` | `WireBandParams[17][12]`, channel-major, 16 B each |
| 10 | 4088 | 544 | `channel_names` | `char[17][32]`, NUL-padded |
| 11 | 4632 | 16 | `i2s_config` | see §5.1 |
| 12 | 4648 | 20 | `leveller` | u8 enabled; u8 speed; u8 lookahead; u8 rsv; f32 amount; f32 max_gain_db; f32 gate_threshold_db; u8 detector_mask (V18+); u8 apply_mask (V18+); u8 rsv2[2] |
| 13 | 4668 | 32 | `preamp` | f32 preamp_db[8] |
| 14 | 4700 | 16 | `master_volume` | f32 master_volume_db (-128 = mute); u8 rsv[12] |
| 15 | 4716 | 16 | `input_config` | see §5.2; **shape changed at V28, size did not** |
| 16 | 4732 | 16 | `lg_sound_sync` | u8 enabled; u8 present (ro); u8 volume (ro); u8 muted (ro); u8 rsv[12] |
| 17 | 4748 | 16 | `user_volume` | f32 user_volume_db; u8 user_mute; u8 rsv[11] |
| 18 | 4764 | 16 | `dac_hw_mute` | u8 enabled; u8 active_low; u8 pin (0xFF = none); u8 rsv0; u16 hold_ms; u16 release_ms; u8 rsv[8] |
| 19 | 4780 | 1088 | `crossovers` | `WireBandParams[17][4]`, channel-major; input rows unused |
| 20 | 5868 | 8 | `adat_config` | u8 enabled; u8 pin (0 = platform default); u8 rsv[6] |
| 21 | 5876 | 24 | `psybass` | see §5.3 (V23+) |
| 22 | 5900 | 44 | `upmix` | see §5.4 (V25+) |
| 23 | 5944 | 36 | `subharm` | see §5.5 (V29+, 36 bytes from V30) |
| 24 | 5980 | 48 | `tube` | see §5.6 (V31+) |
| 25 | 6028 | 108 | `limiter` | see §5.7 (V32+) |
| | **6136** | | **total** | |

V29 to V32 only appended sections; no offset before 5944 moved. The sizes on the
way were V29 5960, V30 5980, V31 6028 and V32 6136.

### 3.1 `WireBandParams` (16 B, used by `eq` and `crossovers`)

| Offset | Type | Field |
|---:|---|---|
| 0 | u8 | `type` (FilterType) |
| 1 | u8 | `bypass` (1 = bypassed, anything else = active) |
| 2 | u16 LE | **V22+**: Linkwitz Transform `qp` as `Q*512` when `type == 11`; **must be 0 for every other type** |
| 4 | f32 | `freq` (Hz; `f0` for LT) |
| 8 | f32 | `q` (`Q0` for LT) |
| 12 | f32 | `gain_db` (`fp` in Hz for LT) |

Band index is implicit in array position: row = channel, column = band.

### 3.2 `WireCrosspoint` (8 B) and `WireOutputChannel` (12 B)

`WireCrosspoint`: u8 enabled; u8 phase_invert; u8 rsv[2]; f32 gain_db.
`WireOutputChannel`: u8 enabled; u8 mute; u8 rsv[2]; f32 gain_db; f32 delay_ms.

## 4. `WireHeader` (16 B)

| Offset | Type | Field |
|---:|---|---|
| 0 | u8 | `format_version` (32) |
| 1 | u8 | `platform_id` (0 = RP2040, 1 = RP2350) |
| 2 | u8 | `num_channels` (7 or 17) |
| 3 | u8 | `num_output_channels` (5 or 9) |
| 4 | u8 | `num_input_channels` (2 or 8) |
| 5 | u8 | `max_bands` (12) |
| 6 | u16 | `payload_length` |
| 8 | u16 | `fw_version_major` |
| 10 | u16 | `fw_version_minor` |
| 12 | u32 | reserved |

## 5. Sentinel encodings (the dangerous part)

Several V21-V24 fields were added by **claiming previously reserved bytes**. To
stay safe against old hosts that write zeros there, the firmware uses a
`0 = absent, keep the live value` convention, and where `0` is itself a
meaningful value the field is stored **plus one**.

**A host that writes a plain `0` into these fields is not "leaving them alone" in
the way it might expect, and a host that reads them without subtracting one will
report the wrong thing.** Getting this wrong silently disables inputs.

| Section | Field | Encoding |
|---|---|---|
| `i2s_config` | `clock_pin_mode_p1` | 0 = absent; 1 = unified; 2 = split |
| `i2s_config` | `bck_pin_slave` | 0 = absent, keep live |
| `input_config` | `i2s_input_channels` | 0 = absent; else 2/4/6/8 |
| `input_config` | `i2s_rx_pin_ext[3]` | 0 = unset (pairs 1..3) |
| `input_config` | `spdif_rx_pin_ext[3]` | 0 = absent, keep live (S/PDIF 2..4; **was `[2]` before V28**) |
| `input_config` | `spdif_rx_enabled_ext_p1` | 0 = absent; else mask + 1, bit 0 = S/PDIF 2 (1 = all disabled, 2 = S/PDIF 2, 8 = S/PDIF 2+3+4) |
| `input_config` | `adat_input_pin` | 0 = absent, keep live |
| `input_config` | `adat_input_enabled_p1` | 0 = absent; 1 = disabled; 2 = enabled |
| `input_config` | `adat_clock_mode_p1` | 0 = absent; 1 = master; 2 = slave |
| `adat_config` | `pin` | 0 = platform default on apply |
| `dac_hw_mute` | `pin` | 0xFF = no pin |
| `master_volume` | `master_volume_db` | -128.0 = mute sentinel, distinct from -127.0 |

`i2s_clock_mode` (V21) is a plain 0/1 with no `+1`, because a pre-V21 reader
seeing zero decodes it as master, which is the correct legacy default.

### 5.1 `i2s_config` (16 B @ 4632)

u8 output_types[4] (0 = S/PDIF, 1 = I2S); u8 bck_pin; u8 mck_pin; u8 mck_enabled;
u8 mck_multiplier (128 or 256, **not** an index); u8 clock_pin_mode_p1;
u8 bck_pin_slave; u8 rsv[6].

Note `mck_multiplier` here is the literal value 128 or 256 truncated into a byte,
whereas the vendor command `0xC9` returns an encoded 0 or 1. Do not confuse them.

### 5.2 `input_config` (16 B @ 4716) - changed at V28

**This is the V28 change, and it is the dangerous kind: the section stayed 16
bytes and the packet stayed 5944, so no size check can see it.** V28 added a
fourth selectable S/PDIF input, growing `spdif_rx_pin_ext` from two entries to
three. Every field below it moved down one byte and the section's last reserved
byte was consumed; there are no reserved bytes left.

| Offset | Type | Field | V26 offset |
|---:|---|---|---:|
| 0 | u8 | `input_source` | 0 |
| 1 | u8 | `spdif_rx_pin` | 1 |
| 2 | u8 | `i2s_rx_pin` (pair 0) | 2 |
| 3 | u8 | `i2s_input_rate` (0 = 44100, 1 = 48000, 2 = 96000) | 3 |
| 4 | u8 | `i2s_input_channels` (2/4/6/8; 0 = absent) | 4 |
| 5 | u8[3] | `i2s_rx_pin_ext` (pairs 1..3) | 5 |
| 8 | u8[**3**] | `spdif_rx_pin_ext` (S/PDIF 2..4) | 8, but **`[2]`** |
| 11 | u8 | `spdif_rx_enabled_ext_p1` | 10 |
| 12 | u8 | `i2s_clock_mode` (0 = master, 1 = slave) | 11 |
| 13 | u8 | `adat_input_pin` | 12 |
| 14 | u8 | `adat_input_enabled_p1` | 13 |
| 15 | u8 | `adat_clock_mode_p1` | 14 |
| | | *(V26 had one reserved byte at 15)* | 15 |

A pre-V28 decoder run against a V28 packet reads the S/PDIF enable mask as the
I2S clock mode, the clock mode as the ADAT pin, and so on down the section. The
offsets are pinned by name in `wire.rs`'s
`the_v28_input_config_offsets_are_where_the_header_puts_them`.

### 5.3 `psybass` (24 B @ 5876, V23+)

u8 enabled; u8 rsv0; u16 output_mask; f32 cutoff_hz; f32 harmonics_db;
f32 drive_db; f32 character_pct; f32 original_db.

### 5.4 `upmix` (44 B @ 5900, V25+)

u8 enabled; u8 center_mode; u8 surround_mode; **i8 presence_q1** (V26+: presence
bell in dB times 2; was reserved); then f32 strength_pct, center_width_pct,
corr_threshold_pct, attack_ms, release_ms, detector_hpf_hz, surround_delay_ms,
surround_hpf_hz, surround_lpf_hz, decorr_pct.

RP2040 zeroes this section on collect and ignores it on apply.

### 5.5 `subharm` (36 B @ 5944, V29+; bulk_params.h:378-400)

| Offset | Type | Field |
|---:|---|---|
| 0 | u8 | `enabled` |
| 1 | u8 | reserved |
| 2 | u16 | `output_mask` |
| 4 | f32 | `low_db` (24-36 Hz sub) |
| 8 | f32 | `high_db` (36-56 Hz sub) |
| 12 | f32 | `boost_db` (70 Hz bell, 0..+6) |
| 16 | f32 | `top_db` (56-80 Hz sub; V30+) |
| 20 | f32 | `select_depth` (0..100 %) |
| 24 | f32 | `select_hold_ms` (50..400) |
| 28 | f32 | `ceiling_db` (-40..0 dBFS, 0 = off) |
| 32 | u8 | `select_mode` (0 all, 1 percussive, 2 sustained) |
| 33 | u8 | `link_pairs` |
| 34 | u8[2] | reserved |

A band level of -30 dB turns that band off. The header's comments say the band
levels run to +6 dB; the firmware limit is `SUBHARM_LEVEL_MAX`, +12
(subharm.h:64-65). Solo is runtime only and never on the wire. Bulk apply
clamps only `select_mode`; the floats are clamped later, so a GET straight
after a bulk apply can show unclamped values.

### 5.6 `tube` (48 B @ 5980, V31+; bulk_params.h:403-427)

| Offset | Type | Field |
|---:|---|---|
| 0 | u8 | `enabled` |
| 1 | u8 | `tube_type` (0 custom, 1..16) |
| 2 | u8 | `rectifier` (0..3) |
| 3 | u8 | `xfmr_enabled` (output stage) |
| 4 | u16 | `output_mask` |
| 6 | u8[2] | reserved |
| 8 | f32 | `drive_db` (-30..24) |
| 12 | f32 | `bias_pct` (-100..100) |
| 16 | f32 | `asym_db` (-12..12) |
| 20 | f32 | `hardness_pct` (0..100) |
| 24 | f32 | `sag_pct` (0..100) |
| 28 | f32 | `xfmr_damping` (1..20) |
| 32 | f32 | `xfmr_res_hz` (30..150) |
| 36 | f32 | `mix_pct` (0..100) |
| 40 | f32 | `trim_db` (-12..12) |
| 44 | f32 | reserved |

Apply copies the character fields verbatim and does not run the tube-type row
lookup, so a saved Custom voicing survives.

### 5.7 `limiter` (108 B @ 6028, V32+; bulk_params.h:430-446)

Nine 12-byte `WireLimiterOutput` records, output `k` at `6028 + 12k`:

| Offset | Type | Field |
|---:|---|---|
| 0 | u8 | `enabled` |
| 1 | u8 | `link_group` (0 unlinked, 1..4) |
| 2 | u8[2] | reserved |
| 4 | f32 | `threshold_db` (-30..0 dBFS) |
| 8 | f32 | `release_ms` (10..1000) |

Records past the device's output count read zero and are ignored on a write.
**The section is applied only in `OUTPUT_CONFIG_MODE` WITH_PRESET**
(bulk_params.h:497-499, bulk_params.c:1005-1008); in INDEPENDENT mode a bulk write ignores it, yet
`BULK_INVALIDATED` still fires.

## 6. Read-only fields

On bulk SET the firmware ignores these; they are produced by the device:

- `lg_sound_sync.present`, `.volume`, `.muted` (only `.enabled` is honoured)
- The `legacy` section is retained for wire parity and is superseded by the
  matrix mixer for per-output control.

## 7. Practical notes

- **Transfer size.** At 6136 bytes this exceeds the 4 KB WinUSB cap, so on
  Windows the chunked opcodes `GET_ALL_PARAMS_CHUNK` (0xA2) and
  `SET_ALL_PARAMS_CHUNK` (0xA3) are mandatory rather than optional. `0xA2` at
  offset 0 snapshots under the bulk lock; read sequentially from there.
- **`SET_ALL_PARAMS` writes RAM only.** It persists nothing. A preset save is a
  separate step. This is the most commonly misunderstood part of the protocol.
- **Crossover rows for input channels are zeroed on collect and skipped on
  apply.** Storage symmetry with `eq` is a convenience, not a usable slot.
- **Notification dispatch.** `PARAM_CHANGED` on the bulk IN endpoint carries the
  byte offset into this packet, so this table doubles as the notification
  routing table. Subharm (except solo), tube and limiter writes are notified at
  their offsets here; a limiter write to output 0xFF sends one per output.
- **The analyser shares the bulk buffer.** `REQ_RTA_GET_BANDS_ALL` (0x0F) builds
  its reply in the same buffer, so it stalls while a chunked bulk transfer holds
  it (vendor_commands.c:4334-4355).
