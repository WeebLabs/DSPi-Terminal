# `WireBulkParams` V26 wire format

*Derived from `crates/dspi-proto/firmware/bulk_params.h`, vendored from
`WeebLabs/DSPi` `release/v1.1.5` @ `9776c2f`.*

This document exists because **no released documentation describes this layout**.
`DSPi/Documentation/commands.md` §13 documents wire format **V14 at 3664 bytes**
with a "Master L / Master R" channel model, which the firmware abandoned at V16.
It is twelve versions stale. This file is derived from the header and is
regenerated-checked by `dspi-proto`'s offset test, so it cannot silently drift.

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
  that is exactly V26 and exactly 5944 bytes. There is no "send a prefix and let
  older fields default" path, and no forward compatibility on write.
- **Never bulk-write to a device whose `format_version` you do not implement.**
  Fall back to individual `SET_*` opcodes, which are version-independent, or
  refuse with a clear message. Guessing here corrupts the whole device state.
- **On read, the header is authoritative.** Check `format_version` and
  `payload_length` before interpreting a single offset. A device reporting an
  unknown version must not be parsed against this table.

This inverts the tolerance strategy a host would normally use. The
`FieldDesc` table still earns its place for notification-offset dispatch, for
readable field definitions, and for the next format change, but it does **not**
buy us cross-version bulk writes. Those do not exist.

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
| `WIRE_FORMAT_VERSION` | **26** | |
| `sizeof(WireBulkParams)` | **5944** | |

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
| 15 | 4716 | 16 | `input_config` | see §5.2 |
| 16 | 4732 | 16 | `lg_sound_sync` | u8 enabled; u8 present (ro); u8 volume (ro); u8 muted (ro); u8 rsv[12] |
| 17 | 4748 | 16 | `user_volume` | f32 user_volume_db; u8 user_mute; u8 rsv[11] |
| 18 | 4764 | 16 | `dac_hw_mute` | u8 enabled; u8 active_low; u8 pin (0xFF = none); u8 rsv0; u16 hold_ms; u16 release_ms; u8 rsv[8] |
| 19 | 4780 | 1088 | `crossovers` | `WireBandParams[17][4]`, channel-major; input rows unused |
| 20 | 5868 | 8 | `adat_config` | u8 enabled; u8 pin (0 = platform default); u8 rsv[6] |
| 21 | 5876 | 24 | `psybass` | see §5.3 (V23+) |
| 22 | 5900 | 44 | `upmix` | see §5.4 (V25+) |
| | **5944** | | **total** | |

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
| 0 | u8 | `format_version` (26) |
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
| `input_config` | `spdif_rx_pin_ext[2]` | 0 = absent, keep live |
| `input_config` | `spdif_rx_enabled_ext_p1` | 0 = absent; 1 = both disabled; 2 = S/PDIF 2; 3 = both |
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

### 5.2 `input_config` (16 B @ 4716)

u8 input_source; u8 spdif_rx_pin; u8 i2s_rx_pin (pair 0); u8 i2s_input_rate
(0 = 44100, 1 = 48000, 2 = 96000); u8 i2s_input_channels; u8 i2s_rx_pin_ext[3];
u8 spdif_rx_pin_ext[2]; u8 spdif_rx_enabled_ext_p1; u8 i2s_clock_mode;
u8 adat_input_pin; u8 adat_input_enabled_p1; u8 adat_clock_mode_p1; u8 rsv[1].

### 5.3 `psybass` (24 B @ 5876, V23+)

u8 enabled; u8 rsv0; u16 output_mask; f32 cutoff_hz; f32 harmonics_db;
f32 drive_db; f32 character_pct; f32 original_db.

### 5.4 `upmix` (44 B @ 5900, V25+)

u8 enabled; u8 center_mode; u8 surround_mode; **i8 presence_q1** (V26+: presence
bell in dB times 2; was reserved); then f32 strength_pct, center_width_pct,
corr_threshold_pct, attack_ms, release_ms, detector_hpf_hz, surround_delay_ms,
surround_hpf_hz, surround_lpf_hz, decorr_pct.

RP2040 zeroes this section on collect and ignores it on apply.

## 6. Read-only fields

On bulk SET the firmware ignores these; they are produced by the device:

- `lg_sound_sync.present`, `.volume`, `.muted` (only `.enabled` is honoured)
- The `legacy` section is retained for wire parity and is superseded by the
  matrix mixer for per-output control.

## 7. Practical notes

- **Transfer size.** At 5944 bytes this exceeds the 4 KB WinUSB cap, so on
  Windows the chunked opcodes `GET_ALL_PARAMS_CHUNK` (0xA2) and
  `SET_ALL_PARAMS_CHUNK` (0xA3) are mandatory rather than optional. `0xA2` at
  offset 0 snapshots under the bulk lock; read sequentially from there.
- **`SET_ALL_PARAMS` writes RAM only.** It persists nothing. A preset save is a
  separate step. This is the most commonly misunderstood part of the protocol.
- **Crossover rows for input channels are zeroed on collect and skipped on
  apply.** Storage symmetry with `eq` is a convenience, not a usable slot.
- **Notification dispatch.** `PARAM_CHANGED` on the bulk IN endpoint carries the
  byte offset into this packet, so this table doubles as the notification
  routing table.
