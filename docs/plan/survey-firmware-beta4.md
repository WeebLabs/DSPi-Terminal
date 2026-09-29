# Survey: DSPi firmware changes, v1.1.6-beta2 to beta4 (2026-09-29)

Read from `/Users/weeblabs/DSPi` on `release/v1.1.6`, from `112f35b` (the
baseline of `survey-firmware.md`, tagged `v1.1.6-beta2`) to `557bce7`
(beta4, untagged). Line numbers refer to `557bce7`; paths are relative to
`firmware/DSPi/` unless they start with `Documentation/`. This file only
records what changed; `survey-firmware.md` remains the reference for
everything else.

`v1.1.6-beta3` is tagged at `6f88ae1`: wire V30, CS caps 18, RTA protocol
V3, beta byte 3, no tube, no limiter, and an unpaced idle keep-alive.

| Commit | Wire | CS caps | Beta byte |
|---|---|---|---|
| 112f35b | 28 | 13 | none |
| 56b2ec8 | 29 | 14 | none |
| 6f90cb1 | 30 | 15 | none |
| 5ff3aa0 | 30 | 16 | none |
| 90ebe59 | 30 | 17 (never shipped) | none |
| 86f599a | 30 | 18 | none |
| 5145d1a | 30 | 18 | 3 |
| 7277c31 | 31 | 19 | 3 |
| 772061e | 32 | 19 | 3 |
| 6712c38 | 32 | 19 | 4 |
| 76c06bb | 32 | 20 | 4 |

## 1. Opcodes

`config.h` now has **244** `REQ_*` defines, up from 202: 42 new, none
removed, no duplicate numbers. `REQ_LIMITER` (0x81) is one define used in
both directions. Direction convention as in `survey-firmware.md`.

### 1.1 CS auxiliary outputs (caps v18), config.h:136-145

| Op | Name | Dir | wValue | Payload / response |
|---|---|---|---|---|
| 0x04 | REQ_SET_CS_AUX_STATE | OUT | slot 0-15 | 1 byte, non-zero = on. Immediate, runtime only, never flashed, never dirty |
| 0x05 | REQ_GET_CS_AUX_STATE | IN | slot | 1 byte; STALL unless the slot is an aux output that is up |
| 0x05 | REQ_GET_CS_AUX_STATE | IN | 0xFFFF | 48 bytes: `state[16]`, then `level_q8[16]` u16 LE; non-aux slots read 0 |
| 0x06 | REQ_SET_CS_AUX_LEVEL | OUT | slot | 2 bytes, 8.8 percent LE, clamped to 25600; AUX_PWM only |
| 0x07 | REQ_GET_CS_AUX_LEVEL | IN | slot | 2 bytes 8.8; 0 on AUX_OUT; STALL if not an aux that is up |

Handlers: vendor_commands.c:1496-1540 (SET), 2977-3009 (GET). A rejected
SET sets `cs_last_status` to `CS_STATUS_INVALID_AUX` (0x26) or
`INVALID_VALUE` (0x14), but USB still ACKs the data stage, so the host must
poll 0x87 to see the result. 0x00, 0x02 and 0x03 are unallocated and STALL.

### 1.2 Spectrum analyser (RTA), config.h:147-154

| Op | Name | Dir | wValue | Payload / response |
|---|---|---|---|---|
| 0x08 | REQ_RTA_SET_CONFIG | OUT | 0 | 12-byte `RtaConfig`, length exactly 12 |
| 0x09 | REQ_RTA_GET_CONFIG | IN | 0 | 12-byte applied `RtaConfig` |
| 0x0A | REQ_RTA_GET_CAPS | IN | 0 | 16-byte `RtaCaps` |
| 0x0A | REQ_RTA_GET_CAPS | IN | 1..255 | Band-centre chunk, up to 32 u16 LE Hz. Chunk 1 is 64 B (bands 0-31), chunk 2 is 10 B (32-36), later chunks STALL |
| 0x0B | REQ_RTA_GET_BANDS | IN | channel at current tap | 82-byte `RtaBandFrame`; STALL if channel >= tap width |
| 0x0C | REQ_RTA_GET_BINS | IN | byte offset | Up to wLength bytes of the bin frame; no lock taken |
| 0x0D | REQ_RTA_GET_STATUS | IN | 0 | 24-byte `RtaStatus`; does not count as a read |
| 0x0E | REQ_RTA_CONTROL | WAR | 0 stop, 1 start, 2 reset averaging | 1 byte (=1); unknown action STALLs |
| 0x0F | REQ_RTA_GET_BANDS_ALL | IN | 0 | Band frames of every selected and live channel, 82-byte strides, ascending; zero-length if none. USB only |

0x0F takes the bulk lock, so it STALLs while a 0xA0/0xA2/0xA3 session holds
`bulk_param_buf` (vendor_commands.c:168-172, 1784, 4257-4356).

### 1.3 Subharmonic synthesizer, config.h:184-210

| Op | Name | Dir | Payload |
|---|---|---|---|
| 0x10 / 0x11 | SET / GET_SUBHARM | OUT / IN | u8 enable |
| 0x12 / 0x13 | SET / GET_SUBHARM_LOW | OUT / IN | f32 dB, 24-36 Hz sub, -30..+12, -30 = off |
| 0x14 / 0x15 | SET / GET_SUBHARM_HIGH | OUT / IN | f32 dB, 36-56 Hz sub, -30..+12 |
| 0x16 / 0x17 | SET / GET_SUBHARM_BOOST | OUT / IN | f32 dB, 70 Hz bell, 0..+6 |
| 0x18 / 0x19 | SET / GET_SUBHARM_MASK | OUT / IN | u16 LE output mask |
| 0x1A | GET_SUBHARM_HEADROOM | IN | f32 dB worst-case gain; 0 while disabled; not on the wire |
| 0x1B / 0x1C | SET / GET_SUBHARM_TOP | OUT / IN | f32 dB, 56-80 Hz sub, -30..+12 |
| 0x1D / 0x1E | SET / GET_SUBHARM_SELECT | OUT / IN | u8 0 all, 1 percussive, 2 sustained; >2 clamps |
| 0x1F | GET_SUBHARM_METER | IN | `NUM_OUTPUT_CHANNELS` x u16 LE 0..32767 (18 B RP2350, 10 B RP2040) |
| 0x2C / 0x2D | SET / GET_SUBHARM_SOLO | OUT / IN | u8; runtime only, no wire offset, **no notification**, never persisted |
| 0x2E / 0x2F | SET / GET_SUBHARM_LINK | OUT / IN | u8 pair link |
| 0xA9 / 0xAA | SET / GET_SUBHARM_DEPTH | OUT / IN | f32 %, 0..100 |
| 0xAB / 0xAC | SET / GET_SUBHARM_HOLD | OUT / IN | f32 ms, 50..400 |
| 0xAD / 0xAE | SET / GET_SUBHARM_CEILING | OUT / IN | f32 dBFS, -40..0, 0 = off |

Handlers: vendor_commands.c:938-1064, 2140-2217. Every SET except SOLO
emits PARAM_CHANGED at its `WireBulkParams.subharm` offset.

### 1.4 Tube preamp, config.h:230-231

| Op | Name | Dir | wValue | Payload |
|---|---|---|---|---|
| 0x3E | REQ_SET_TUBE_PARAM | OUT | low byte = `TUBE_PARAM_*` 0..13 | f32 LE for **every** parameter, including bools, enums and the mask |
| 0x3F | REQ_GET_TUBE_PARAM | IN | low byte = index | f32 LE; bad index STALLs (probe with index 0) |

A bad index, a short payload or NaN is a silent no-op. Bools are
non-zero-is-true; mask and enums round to nearest, then clamp (tube.c:122-212).

### 1.5 Output limiter, config.h:236

`REQ_LIMITER` = 0x81, `wValue = (output << 8) | index` (limiter.h:12-21).

- **OUT:** f32 LE. Index 0 ENABLED, 1 THRESHOLD_DB, 2 RELEASE_MS, 3
  LINK_GROUP. `output = 0xFF` applies to every output. Bad output, index or
  NaN is a silent no-op.
- **IN:** index 0-3 returns f32 for that output; a bad output or index
  STALLs. Index 0x80 (`LIMITER_GET_METER`) returns `NUM_OUTPUT_CHANNELS` x
  u16 LE gain reduction in 0.01 dB (0 none, 12000 floor). Index 0x81
  (`LIMITER_GET_STATUS`) returns `{engaged, lookahead=32, block=16,
  num_outputs}` and is the feature probe.

0x81 was briefly `REQ_GET_TUBE_METER` in untagged commits; that is gone.

### 1.6 Build info, config.h:326

`0x80 REQ_GET_BUILD_INFO` (IN, 64 bytes): `[0..47]` `git describe --always
--dirty`, NUL-padded; `[48..59]` `YYYY-MM-DD`; `[60..63]` zero.
Informational only; nothing may gate on it. Older firmware STALLs.

### 1.7 Changed opcodes

- **0x7F GET_PLATFORM** offers 7 bytes: `[0]` platform, `[1]` major, `[2]`
  legacy `(minor<<4)|patch`, `[3]` NUM_OUTPUT_CHANNELS, `[4]` minor, `[5]`
  patch, `[6]` beta ordinal (0 = final). Truncated to wLength. Host rule
  (Documentation/Features/firmware_versioning_spec.md:67-111): ask for 7;
  use bytes 4/5 if at least 6 arrive; beta from byte 6 if 7 arrive, else 0.
  Order by `(major, minor, patch, beta == 0 ? 256 : beta)`. beta2 returns 4
  bytes and so reads as final 1.1.6; the Console treats a short reply that
  claims 1.1.6 or later as "early beta".
- **0x86 GET_CS_CAPS:** header is 52 bytes (`type_count` 11, tail at
  `4 + 4*11 = 48`), `caps_version` 20, `noun_count` 79.
- **0x84/0x85 CsBinding:** byte 22 is now `extras`; byte 23 `reserved2`
  must be 0.
- **0xA0-0xA3 bulk:** 6136 bytes at V32.
- **0x52 SAVE_OUTPUT_CONFIG** also saves limiter settings; **0x98/0x99
  OUTPUT_CONFIG_MODE** now governs limiter persistence (section 3.4).
- **Presets** carry subharm (slot V36/V37), tube (V38) and limiter (V39).
- **0x53 FACTORY_RESET** clears subharm solo, thaws a parked host volume,
  and leaves the limiter alone in INDEPENDENT mode.

## 2. WireBulkParams V28 to V32

`WIRE_FORMAT_VERSION` = 32 (bulk_params.h:34). All sections are appended;
**no existing offset moved**. Sizes: V29 5960, V30 5980, V31 6028, V32
**6136** (asserted at bulk_params.c:50-66). Apply still requires an exact
version and length (bulk_params.c:418-420). `WIRE_BULK_BUF_SIZE` now equals
`sizeof(WireBulkParams)`.

- V29: subharm section, 16 bytes.
- V30: subharm grows to 36 bytes (top band, selectivity, ceiling, link).
- V31: tube section, 48 bytes.
- V32: limiter section, 108 bytes.

### `WireSubharmParams`, 36 B at 5944 (bulk_params.h:378-400)

| Abs | Rel | Field |
|---|---|---|
| 5944 | +0 | enabled u8 |
| 5945 | +1 | reserved0 |
| 5946 | +2 | output_mask u16 |
| 5948 | +4 | low_db f32 |
| 5952 | +8 | high_db f32 |
| 5956 | +12 | boost_db f32 |
| 5960 | +16 | top_db f32 |
| 5964 | +20 | select_depth f32 |
| 5968 | +24 | select_hold_ms f32 |
| 5972 | +28 | ceiling_db f32 |
| 5976 | +32 | select_mode u8 |
| 5977 | +33 | link_pairs u8 |
| 5978 | +34 | reserved1[2] |

Level -30 means band off; ceiling 0 means off. Solo is not on the wire.
Bulk apply clamps only `select_mode`; floats clamp later at recompute. The
header comments still say "-30..+6"; the firmware limit is +12.

### `WireTubeParams`, 48 B at 5980 (bulk_params.h:403-427)

5980 enabled, 5981 tube_type (0 custom, 1..16), 5982 rectifier (0..3),
5983 xfmr_enabled, 5984 output_mask u16, 5986 reserved[2], then f32 at
5988 drive_db, 5992 bias_pct, 5996 asym_db, 6000 hardness_pct, 6004
sag_pct, 6008 xfmr_damping, 6012 xfmr_res_hz, 6016 mix_pct, 6020 trim_db,
6024 reserved_f. Apply copies verbatim, clamps only type and rectifier, and
never runs the tube-type row lookup.

### `WireLimiterParams`, 108 B at 6028 (bulk_params.h:430-446)

Nine 12-byte records at `6028 + 12k`: `{enabled u8, link_group u8 (0..4),
reserved[2], threshold_db f32, release_ms f32}`. Records past
`num_output_channels` are zero on GET and ignored on SET. The section is
**applied only in OUTPUT_CONFIG_MODE = WITH_PRESET** (bulk_params.c:1008);
in INDEPENDENT mode it is silently ignored.

## 3. Features

### 3.1 Subharmonic synthesizer (both platforms)

Three bands: input 48-72 Hz makes a 24-36 Hz sub, 72-112 makes 36-56,
112-160 makes 56-80. Also a 70 Hz boost bell, a selectivity gate, a sub
ceiling, pair link, solo, a meter and a headroom reading. Defaults
(subharm.h:85-94): enabled 0, low 0, high 0, top -30 (off), boost 0, mask
0xFFFF, select 0, depth 100, hold 150, ceiling 0 (off), link on, solo off.
Chain: matrix, crossfeed, **subharm**, psybass, tube, crossover. The sub
lags the dry path by 6 samples at 48 kHz. RAW, muted and disabled outputs
are skipped. No capability bit: use wire >= 29/30 or probe 0x11 / 0x1C.

### 3.2 Spectrum analyser (polled over EP0, no push)

Off at boot, never persisted, not in presets or bulk. Any 0x0B/0x0C/0x0F
read auto-starts it; it stops 5000 ms after the last such read.
`RTA_FLAG_MANUAL` (0x01) disables auto-start and auto-stop so CONTROL owns
the run state (rta.h:27-44). Default config: tap OUTPUT, all outputs, order
10 (RP2350) / 9 (RP2040), avg 250 ms, peak decay 20 dB/s.

- **Taps:** INPUT (0) is after the per-input PEQ, before the matrix, and
  includes upmix-derived rows while the upmixer runs; OUTPUT (1) is after
  gain and delay. Only matrix-enabled outputs are live.
- **Rotation:** one FFT rotates through selected and live channels, about
  21 ms per channel at 1024 points and 48 kHz. A 14-band bass bank (10-200
  Hz) runs continuously per selected channel.
- **`RtaConfig`, 12 B:** version (=3), tap, channel_mask u16, fft_order
  8..10, rsvd, avg_ms u16 (0..10000), peak_decay_db_s u8 (0..100, 0 = hold
  off), flags, rsvd u16.
- **`RtaCaps`, 16 B:** version, input_channels, output_channels, order_min
  8, order_max 10, order_default, bass_bands 14, max_bands 37, level_zero
  243, dynamic_range_db (120 RP2350 / 78 RP2040), idle_timeout_ms 5000,
  max_bin_frame 529, bass_dynamic_range_db 70.
- **`RtaBandFrame`, 82 B:** version, channel, seq, n_bands (34 at 44.1/48
  kHz, 37 at 96 kHz), age_ms u16 (0xFFFF never), rsvd u16, avg[37],
  peak[37].
- **Bin frame:** 16-byte header (version, channel, seq, fft_order,
  sample_rate_hz u32, n_bins u16 = N/2, rsvd[3]), then `n_bins` level
  bytes, then one byte repeating seq. The device writes the tail as 0xFF
  first and seq skips 0xFF; chunked readers must compare head and tail seq.
- **`RtaStatus`, 24 B:** version, state (0 idle, 1 capturing, 2
  transforming), tap, channel (0xFF idle), rsvd, live_count, live_mask u16,
  frames_per_s, busy_us_per_s, last_frame_us, idle_ms (0xFFFF never),
  sample_rate_hz u32, first_band (0xFF on unsupported rate), rsvd,
  bass_busy_us_per_s.
- **Level byte:** `dBFS = (v - 243) * 0.5`; 255 is +6 dBFS, 0 is the floor.
- **Band centres** (rta_tables.h:818-822), 37 entries from 10 Hz to 40 kHz;
  bands 0-13 come from the bass bank. Bands above 200 Hz with no FFT bin at
  small sizes read 0.
- **Protocol V3** is incompatible with V2; reject any other version.

### 3.3 Tube preamp (both platforms)

| Index | Name | Range | Default |
|---|---|---|---|
| 0 | ENABLED | bool | 0 |
| 1 | OUTPUT_MASK | 0..0xFFFF | 0xFFFF |
| 2 | TUBE_TYPE | 0..16 | 1 (12AX7) |
| 3 | DRIVE_DB | -30..+24 | -12 |
| 4 | BIAS_PCT | -100..+100 | 10 |
| 5 | ASYM_DB | -12..+12 | 3 |
| 6 | HARDNESS_PCT | 0..100 | 40 |
| 7 | SAG_PCT | 0..100 | 15 |
| 8 | RECTIFIER | 0..3 | 1 (GZ34) |
| 9 | XFMR_ENABLED | bool | 1 |
| 10 | XFMR_DAMPING | 1..20 | 2 |
| 11 | XFMR_RES_HZ | 30..150 | 95 |
| 12 | MIX_PCT | 0..100 | 100 |
| 13 | TRIM_DB | -12..+12 | 0 |

Setting type 1..16 copies bias, asym, hardness and sag from the row (5
notifications). Changing any of indices 4-7 to a different value sets the
type to 0, Custom (2 notifications). The type rows and the rectifier table
live only in tube.c (49-66) and the spec, not in tube.h. Zero added
latency; after psybass, before the crossover; RAW siggen outputs bypass it.
Probe: wire >= 31, or 0x3F index 0 does not STALL.

### 3.4 Output limiter (both platforms)

Brickwall lookahead, block 16, delay 32 samples. After output gain and
loudness, before output delay and metering. **While any limiter is on,
every output carries +32 samples of latency**; enabling the first or
disabling the last switches the delay under an 8 ms soft-mute on all
outputs. Threshold -30..0 dBFS (default -1), release 10..1000 ms (default
100), link group 0..4 (0 unlinked), enabled off by default. RAW siggen
outputs are limited.

**Ganging** (limiter.c:143-194): a SET of enable, threshold or release on a
linked output writes the whole group; joining a group adopts the settings
of its lowest-numbered member; `link_group` with output 0xFF puts every
output in that group, adopting output 0. Recompute sanitises and re-gangs,
notifying each change.

**Persistence** (Documentation/Features/output_limiter_spec.md:136-150):

| Event | WITH_PRESET (1, default) | INDEPENDENT (0) |
|---|---|---|
| Boot | Startup slot, or defaults | Directory V22 `FlashLimiterConfig` |
| Preset load | Slot's settings | Untouched |
| Active preset deleted | Defaults | Untouched |
| Factory reset | Defaults | Untouched |
| Bulk SET (0xA1) | Applied | Ignored |
| Preset save | Stores live settings | Stores live settings |
| 0x52 | Saves device-wide | Saves device-wide |

### 3.5 Crossover indexing (e41335c)

Internal only. Host band indexing is unchanged.

### 3.6 Notifications on EP 0x83

- **Paced keep-alive** (dd5d24e): the endpoint NAKs while idle and sends a
  1-byte IDLE only after 100 ms without a packet. Reads may block about 100
  ms.
- **New `NOTIFY_EVT_CS_AUX` = 0x0C**, 9 bytes: ver 2, 0x0C, flags 0, seq,
  slot, state, level_q8 u16 LE, src. Sent only on a real change.
- Subharm (not solo), tube and limiter SETs emit PARAM_CHANGED at their
  offsets. A limiter 0xFF SET emits one per output.

### 3.7 Control Surfaces caps v14 to v20

| # | Noun | Kind | Unit | Range | Target | Caps |
|---|---|---|---|---|---|---|
| 57 | SUBHARM | bool | | | NONE | v14 |
| 58 | SUBHARM_LOW | cont | dB | -30..+12 | NONE | v14 |
| 59 | SUBHARM_HIGH | cont | dB | -30..+12 | NONE | v14 |
| 60 | SUBHARM_BOOST | cont | dB | 0..6 | NONE | v14 |
| 61 | SUBHARM_TOP | cont | dB | -30..+12 | NONE | v15 |
| 62 | SUBHARM_SELECT | enum | | 3 values | NONE | v15 |
| 63 | SUBHARM_DEPTH | cont | % | 0..100 | NONE | v15 |
| 64 | SUBHARM_HOLD | cont | ms (8.8) | 50..127 | NONE | v15 |
| 65 | SUBHARM_CEILING | cont | dB | -40..0 | NONE | v15 |
| 66 | SUBHARM_LINK | bool | | | NONE | v15 |
| 67 | SUBHARM_SOLO | bool | | | NONE | v15 |
| 68 | AUX | bool | | | AUX (5), 16 | v18 |
| 69 | AUX_LEVEL | cont | % | 0..100 | AUX (5), 16 | v18 |
| 70 | TUBE | bool | | | NONE | v19 |
| 71 | TUBE_DRIVE | cont | dB | -30..24 | NONE | v19 |
| 72 | TUBE_TYPE | enum | | 17 values | NONE | v19 |
| 73 | TUBE_MIX | cont | % | 0..100 | NONE | v19 |
| 74 | LIMITER | bool | | | OUTPUT_CH | v20 |
| 75 | LIMITER_THRESHOLD | cont | dB | -30..0 | OUTPUT_CH | v20 |
| 76 | LIMITER_RELEASE | cont | MS_LOG (6) | 10..1000 | OUTPUT_CH | v20 |
| 77 | LIMITER_LINK | enum | | 5 values | OUTPUT_CH | v20 |
| 78 | LIMITER_GR | cont, read-only | dB | 0..30 | OUTPUT_CH | v20 |

- v16 widened nouns 58, 59 and 61 from +6 to +12 dB.
- `CS_UNIT_MS_LOG` = 6 is new: `value`, `range`, `min_q` and `max_q` are
  plain integer ms (10 and 1000), and `step` is 8.8 octaves as for HZ.
- `CS_TARGET_AUX` = 5 addresses a binding slot and requires index 0.
- `CS_TYPE_AUX_OUT` = 9 and `CS_TYPE_AUX_PWM` = 10, both containers with
  no actions and one pin of class ANY; `CS_TYPE_COUNT` = 11.
- Aux containers: `gpio[0]` is the pin; `flags` may hold only INVERT;
  `on_delay`/`off_delay` act as TON/TOF; `base_bright` is the PWM ceiling;
  `value` is the PWM boot level (8.8, 0..25600); `extras` bits `BOOT_ON`
  0x01, `BOOT_SAVED` 0x02, `LINEAR` 0x04 (PWM only). noun, action, event,
  target, index, step and range must be 0.
- Aux slots load first and win GPIO conflicts; aux nouns reject GROUP; a
  binding cannot target its own slot; CYCLE_ALL display skips aux nouns;
  live aux values are never dirty.
- New status `CS_STATUS_INVALID_AUX` = 0x26.
- Directory V21 is byte-identical to V19; V22 adds the limiter block.
- Front-panel tube and limiter nouns dispatch through 0x3E and 0x81, so
  limiter nouns gang link groups exactly like host SETs.

### 3.8 User volume parking (3422c0c)

A UAC1 volume SET while the input is not USB is parked instead of applied.
`REQ_GET_USER_VOLUME`, bulk `user_volume` and notifications report only the
applied volume, and a parked write emits no PARAM_CHANGED. The parked value
is applied on a switch to USB unless a vendor, CS, preset or LG write
happened in between. Vendor `REQ_SET_USER_VOLUME` is unchanged.

## 4. Headers a host would vendor

| Header | Host needs | Caveats |
|---|---|---|
| subharm.h | Limits, defaults, `SUBHARM_SELECT_*`, band edges (15-98) | Includes config.h; Q28 helpers are platform-conditional |
| tube.h | `TUBE_PARAM_*`, `TUBE_TYPE_MAX` 16, `TUBE_RECT_MAX` 3, limits, defaults (16-73) | Type and rectifier tables are only in tube.c |
| limiter.h | `LIMITER_PARAM_*`, 0x80/0x81/0xFF, block, delay, limits, defaults (12-36) | `lm_sample_t` is platform-conditional |
| rta.h | Wire structs and protocol constants (27-129) | Includes rta_fft.h and rta_bass.h; `RTA_MAX_TRACKED` is platform-derived |
| rta_fft.h | `RTA_ORDER_MIN/MAX`, `RTA_MAX_POINTS`, `RTA_MAX_BANDS`, `RTA_LEVEL_ZERO_DBFS` | Kernel API is internal |
| rta_bass.h | `RTA_BASS_BANDS` (also in caps) | |
| notify.h | Event ids including `NOTIFY_EVT_CS_AUX`, `ParamSource` | Not vendored today; the Terminal hand-types these |
| rta_tables.h, rta_bass_tables.h, build_info.h | Nothing | Generated tables; band centres come from caps |

psybass.h, upmix.h and siggen.h are unchanged.

## 5. Platform differences

Every new feature exists on both RP2040 and RP2350.

| Area | RP2350 | RP2040 |
|---|---|---|
| Subharm and limiter meters | 18 B | 10 B |
| Limiter `num_outputs` | 9 | 5 |
| Mask bits used | 0-8 (bit 8 = PDM) | 0-4 (bit 4 = PDM) |
| RTA dynamic range | 120 dB | 78 dB |
| RTA default order | 10 | 9 |
| RTA input tap width | 8 | 2 |

PDM full scale is not referenced to the same 0 dBFS on the two platforms
(output_limiter_spec.md:95). Probes: limiter 0x81 IN index 0x81; tube 0x3F
index 0; subharm 0x11 / 0x1C; RTA 0x0A; build info 0x80.

## 6. Gotchas

1. `REQ_RTA_SET_CONFIG` never STALLs on USB and is applied later; re-read
   0x09 to confirm.
2. The RTA stops 5 s after the last data read; polling 0x0D does not keep
   it alive.
3. Chunked 0x0C reads can tear; compare head and tail seq.
4. 0x0F STALLs while a bulk session is open.
5. Tube SETs are always f32, even for bools, enums and the mask.
6. A tube type overwrites bias, asym, hardness and sag; editing any of
   those sets the type to Custom.
7. Bulk and preset restore skip the tube row lookup.
8. Tube defaults changed in this range: drive -6 to -12 dB, drive floor -6
   to -30, output stage off to on, resonance 85 to 95 Hz.
9. Subharm solo is invisible to other hosts; poll 0x2D.
10. A GET after a subharm bulk apply may show unclamped floats.
11. The limiter section of a bulk push is ignored in INDEPENDENT mode, yet
    BULK_INVALIDATED still fires.
12. Limiter SET errors are silent; GET errors STALL; 0xFF is SET-only.
13. Writing one member of a limiter link group moves the whole group.
14. The first enable and last disable of a limiter cause an 8 ms fade and
    a 32-sample latency step on every output.
15. RAW signal-generator output is limited.
16. The CS caps header is 52 bytes; `noun_count` is 79.
17. `CS_UNIT_MS_LOG` values are plain ms, not 8.8.
18. Aux SETs apply immediately and never dirty the config; failures show
    only through 0x87.
19. `CsBinding.extras` must be 0 on non-aux types.
20. Ask GET_PLATFORM for 7 bytes; a 4-byte reply means beta 0.
21. EP 0x83 NAKs while idle.
22. A UAC1 volume change on a non-USB input no longer appears anywhere
    until the input switches to USB.

Documentation that disagrees with the firmware (trust the firmware):
commands.md says 0x7F returns 6 bytes (it returns 7);
control_surfaces_spec.md section 2.4 says caps 19 and 74 nouns (20 and 79);
the subharm level comments say +6 (the limit is +12);
firmware_versioning_spec.md shows beta 0 (HEAD is 4); config.h:147 says
0x08 STALLs on invalid input (not on USB).
