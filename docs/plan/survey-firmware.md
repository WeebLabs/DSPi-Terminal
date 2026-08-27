# Survey: DSPi firmware v1.1.6 host-visible surface (2026-08-27)

Read from `/Users/weeblabs/DSPi` at `release/v1.1.6` @ `112f35b`. The
vendored headers in `crates/dspi-proto/firmware/` are at `9776c2f` (v1.1.5,
wire V26); this survey is the target. Line numbers refer to `112f35b`.

Direction convention: OUT = `0x41` host to device with payload; IN = `0xC1`;
WAR = "write-as-read", a mutation issued as a GET with parameters in `wValue`
and a status byte returned. All requests go to interface 2.

## 1. REQ_* table: 202 defines, no duplicate opcodes

### 1.1 Control Surfaces groups and macros (caps v9), config.h:135-145. NEW

| Op | Name | Dir | wValue | Payload / response |
|---|---|---|---|---|
| 0x20 | REQ_SET_CS_GROUP | OUT | group 0-7 | 40-byte CsGroup; all-zero clears |
| 0x21 | REQ_GET_CS_GROUP | IN | group 0-7 | 40-byte CsGroup |
| 0x22 | REQ_SET_CS_MACRO | OUT | macro 0-7 | 36-byte CsMacroHeaderWire |
| 0x23 | REQ_GET_CS_MACRO | IN | macro 0-7 | 132-byte CsMacro |
| 0x24 | REQ_SET_CS_MACRO_STEP | OUT | (step<<8)\|macro | 12-byte CsMacroStep; all-zero clears |
| 0x25 | REQ_CS_MACRO_FIRE | WAR | macro, 0xFFFF cancels | 1 status byte |
| 0x26 | REQ_GET_CS_EXT_STATUS | IN | 0 | 24-byte CsExtStatusPacket |

### 1.2 Control Surfaces I2C display (caps v10-v13), config.h:149-156. NEW

| Op | Name | Dir | wValue | Payload / response |
|---|---|---|---|---|
| 0x27 | REQ_SET_CS_DISPLAY_CFG | OUT | 0 | 12-byte CsDisplayCfg |
| 0x28 | REQ_GET_CS_DISPLAY_CFG | IN | 0 | 16 B: {max_pages, model_count, rsvd[2]} + CsDisplayCfg |
| 0x29 | REQ_SET_CS_DISPLAY_PAGE | OUT | page 0-15 | 4-byte CsDisplayPage; all-zero clears |
| 0x2A | REQ_GET_CS_DISPLAY_PAGE | IN | page 0-15 | 4-byte CsDisplayPage |
| 0x2B | REQ_GET_CS_DISPLAY_STATUS | IN | 0 | 8-byte CsDisplayStatus |

### 1.3 Psychoacoustic bass, config.h:159-172

0x30/31 enable u8; 0x32/33 cutoff Hz 30..300 def 80; 0x34/35 harmonics dB
-24..+12 def 0; 0x36/37 drive dB 0..18 def 6; 0x38/39 character % 0..100 def
50; 0x3A/3B original dB -60..0 def 0; 0x3C/3D mask u16 LE bit k = output k.

### 1.4 EQ / preamp / bypass / delay, config.h:175-182

0x42 SET_EQ_PARAM OUT EqParamPacket {channel, band, type, bypass, f32 freq, Q,
gain_db}; 0x43 GET_EQ_PARAM IN, band in wValue bits [7:3] (5-bit field);
0x44/45 preamp f32 dB; 0x46/47 bypass u8; 0x48/49 delay f32 ms per channel.

### 1.5 Stereo upmixer (RP2350 only; RP2040 SETs stall, GETs zero), config.h:187-191

0x4A/4B config 44-byte UpmixConfigPacket; 0x4C/4D param, wValue UPMIX_PARAM_*
0..13 f32; 0x4E status 16 bytes. Params: 0 ENABLED, 1 CENTER_MODE, 2
SURROUND_MODE, 3 STRENGTH, 4 CENTER_WIDTH, 5 THRESHOLD, 6 ATTACK, 7 RELEASE,
8 DET_HPF, 9 SUR_DELAY, 10 SUR_HPF, 11 SUR_LPF, 12 DECORR, 13 PRESENCE.

### 1.6 System / status / persistence, config.h:193-216

0x50 GET_STATUS IN wValue-multiplexed (see 6.4); 0x51 SAVE_PARAMS WAR (save to
active slot, deferred); 0x52 SAVE_OUTPUT_CONFIG WAR (repurposed from removed
LOAD_PARAMS); 0x53 FACTORY_RESET WAR; 0x54-57 legacy 3-ch gain/mute; 0x58/59
loudness u8; 0x5A/5B loudness ref dB SPL clamped to LOUDNESS_REF_SPL_MIN/MAX;
0x5C/5D loudness intensity % 0..200; 0x5E/5F crossfeed u8; 0x60/61 crossfeed
preset u8 0..3 (Default/Chu Moy/Meier/Custom); 0x62/63 freq f32; 0x64/65 feed
dB; 0x66/67 ITD u8.

### 1.7 ADAT input (RP2350), config.h:229-236

0x68/69 enable WAR/IN; 0x6A/6B pin WAR/IN (wValue GPIO; 0xFF clears to unset
only while disabled; GET returns 0xFF when unset); 0x6C/6D clock mode 0
master / 1 slave; 0x6E status 20-byte AdatInputStatusPacket.

### 1.8 Matrix mixer / outputs, config.h:239-248

0x70/71 route: MatrixRoutePacket {input, output, enabled, phase_invert, f32
gain_db}; 0x72/73 output enable (Core-1 interlock; blocked enable is silently
skipped, read back); 0x74/75 output gain f32; 0x76/77 output mute u8;
0x78/79 output delay f32 ms.

### 1.9 Core 1 / pins / identification / clips, config.h:251-264

0x7A GET_CORE1_MODE (0 IDLE / 1 PDM / 2 EQ_WORKER); 0x7B GET_CORE1_CONFLICT;
0x7C SET_OUTPUT_PIN WAR wValue (GPIO<<8)|output_index, 0xFF = reset default;
0x7D GET_OUTPUT_PIN; 0x7E GET_SERIAL 16 ASCII-hex; 0x7F GET_PLATFORM 4 bytes
{platform_id, fw_major, fw_minor_patch_bcd, NUM_OUTPUT_CHANNELS}; 0x83
CLEAR_CLIPS WAR read-then-clear, returns the 32-bit clip flags.

### 1.10 Control Surfaces bindings / names / IR, config.h:268-282

| Op | Name | Dir | wValue | Payload / response |
|---|---|---|---|---|
| 0x84 | SET_CS_BINDING | OUT | slot 0-15 | 24-byte CsBinding; deferred preview |
| 0x85 | GET_CS_BINDING | IN | slot 0-15 | 24-byte CsBinding |
| 0x86 | GET_CS_CAPS | IN | 0xFFFF / noun idx | 44-byte CsCapsHeader (was 40) or 12-byte CsNounDesc |
| 0x87 | GET_CS_STATUS | IN | 0 | 41-byte CsStatusPacket (was 22) |
| 0x8B | SET_CS_NAME | OUT | slot | 1-32 byte name; single NUL clears |
| 0x8C | GET_CS_NAME | IN | slot | 32 bytes |
| 0x8D | SET_CS_IR_CMD | OUT | sub-slot 0-15 (was 0-7) | 16-byte IrCommand |
| 0x8E | GET_CS_IR_CMD | IN | sub-slot 0-15 | 16-byte IrCommand |
| 0x8F | CS_IR_LEARN | WAR | 1 arm / 0 cancel / 2 read | 1 status, or 8 B {state, protocol, 0, 0, code_le32} |

### 1.11 I2S clock mode, config.h:478-480

0x88 SET_I2S_CLOCK_MODE OUT u8 0 master / 1 slave, deferred; 0x89 GET live
mode (pending not reflected); 0x8A GET_I2S_SLAVE_STATUS 16 bytes.

### 1.12 Presets / modes / names / CS persistence, config.h:285-305

0x90 PRESET_SAVE WAR slot 0-9; 0x91 PRESET_LOAD WAR deferred, SPDIF-safe;
0x92 DELETE; 0x93 GET_NAME 32 B; 0x94 SET_NAME 32 B; 0x95 GET_DIR 7 bytes
{occupied_LE16, startup_mode, default_slot, last_active, output_config_mode,
master_volume_mode} (spec says 6; firmware returns 7); 0x96 SET_STARTUP
{mode, slot}; 0x97 GET_STARTUP {startup, default_slot, last_active}; 0x98/99
OUTPUT_CONFIG_MODE u8; 0x9A GET_ACTIVE; 0x9B/9C channel name 32 B; 0x9D
CS_SAVE WAR (persists all CS state in one write); 0x9E CS_REVERT WAR.

### 1.13 Bulk params, config.h:308-320

0xA0 GET_ALL_PARAMS 5944 B; 0xA1 SET_ALL_PARAMS; 0xA2 GET_ALL_PARAMS_CHUNK
(wValue offset, wLength chunk; USB only; offset 0 snapshots under lock); 0xA3
SET_ALL_PARAMS_CHUNK (sequential from 0; deferred apply on last byte).

### 1.14 Test signal generator, config.h:325-329

0xA4/A5 config 36-byte SiggenConfig; 0xA6 CONTROL WAR wValue STOP 0 / START 1
/ STOP_NOW 2; 0xA7 status 16 B; 0xA8 caps: 0xFFFF gives 8-byte header, else
62-byte SiggenTypeDesc. 15 signal types (siggen.h:36-56).

### 1.15 Buffer / USB error statistics, config.h:357-360

0xB0 GET_BUFFER_STATS 44 B; 0xB1 RESET WAR bit0 = reset watermarks; 0xB2
GET_USB_ERROR_STATS 24 B, always zero under TinyUSB; 0xB3 reset no-op.

### 1.16 Volume leveller, config.h:363-376

0xB4/B5 enable; 0xB6/B7 amount f32 %; 0xB8/B9 speed 0/1/2; 0xBA/BB max_gain
dB; 0xBC/BD lookahead; 0xBE/BF gate dBFS; 0xDE/DF masks 2 bytes
[detector_mask, apply_mask].

### 1.17 I2S output / MCK, config.h:332-343

0xC0/C1 OUTPUT_TYPE WAR wValue (type<<8)|slot, deferred; 0xC2/C3 I2S_BCK_PIN
WAR wValue (role<<8)|GPIO, role 0 master/unified, 1 slave pair, LRCLK = BCK+1;
0xC4/C5 MCK_ENABLE; 0xC6/C7 MCK_PIN (must be a clk_gpout pin); 0xC8/C9
MCK_MULTIPLIER 0 = 128x, 1 = 256x.

### 1.18 ADAT output (RP2350), config.h:349-354

0xCA/CB enable; 0xCC/CD pin; 0xCE 8-byte AdatStatus.

### 1.19 Preamp / master volume / user volume, config.h:379-410

0xD0/D1 PREAMP_CH wValue input index f32 dB; 0xD2/D3 MASTER_VOLUME f32 dB,
-128 mute sentinel, -127..0; 0xD4/D5 MASTER_VOLUME_MODE 0 INDEPENDENT / 1
WITH_PRESET; 0xD6 SAVE_MASTER_VOLUME WAR; 0xD7 GET_SAVED_MASTER_VOLUME;
0xDA/DB USER_VOLUME f32 (same field as the OS UAC1 slider); 0xDC/DD USER_MUTE.

### 1.20 Band bypass, config.h:436-437

0xD8/D9 wValue (channel<<8)|band, payload u8 (1 = bypassed).

### 1.21 DAC hardware mute, config.h:440-442

0xEA/EB 16-byte DacHwMuteConfig {enabled, active_low, pin, rsvd0, hold_ms,
release_ms, rsvd[8]}; 0xEC TEST WAR, pulses ~1 s.

### 1.22 Input source / SPDIF RX, config.h:445-457

0xE0/E1 INPUT_SOURCE u8; 0xE2 SPDIF_RX_STATUS 16 B; 0xE3 SPDIF_RX_CH_STATUS 24
raw IEC 60958 bytes; 0xE4 SET_SPDIF_RX_PIN WAR wValue (index<<8)|GPIO, index
0..3 (was 0..2); 0xE5 GET index 0..3; 0xE9 SET_SPDIF_INPUT_ENABLE WAR
(index<<8)|enable, index 1..3 (was 1..2); 0xEF GET_SPDIF_INPUT_CONFIG 6 bytes
(was 5): {count, enable_mask bit0 = input2, gpio[0..3]}.

### 1.23 I2S input, config.h:462-471

0xED SET_INPUT_RATE u32 Hz (44100/48000/96000); 0xEE GET_INPUT_RATE 2x u32
{current, selected I2S}; 0xF1/F2 I2S_RX_PIN wValue (pair<<8)|GPIO pair 0..3;
0xF3/F4 I2S_INPUT_CHANNELS 2/4/6/8.

### 1.24 I2S clock-pin mode, config.h:487-488

0xFE SET WAR wValue 0 unified / 1 split; 0xFF GET.

### 1.25 LG Sound Sync, config.h:494-496

0xE6/E7 enable (live-only, persisted with preset save); 0xE8 16-byte status.

### 1.26 System / external control / masks, config.h:499-517

0xF0 ENTER_BOOTLOADER WAR (responds 1, 100 ms, reset_usb_boot); 0xF5/F6
UART_CONFIG 8 B (SET USB-only); 0xF7/F8 I2C_CONFIG 8 B (SET USB-only); 0xF9
CTRL_IFACE_STATUS 8 B; 0xFA/FB LOUDNESS_MASK u16; 0xFC/FD CROSSFEED_OUTPUTS
u8 pair mask.

### 1.27 Delta versus 9776c2f

New opcodes (12): 0x20-0x26, 0x27-0x2B.

Changed semantics: 0x87 response 22 -> 41 bytes; 0x8D sub-slot 0-7 -> 0-15;
0xE4/0xE5 index 0..2 -> 0..3; 0xE9 index 1..2 -> 1..3; 0xEF 5 -> 6 bytes;
0x86 header 40 -> 44 bytes (CS_TYPE_DISPLAY adds a CsTypeDesc row; locate
post-table fields at 4 + 4*type_count).

Other host-visible: FW 1.1.6; new PEQ types FILTER_LOWPASS1 = 12 and
FILTER_HIGHPASS1 = 13 accepted by SET_EQ_PARAM and WireBandParams.type;
Biquad struct renamed Filter (internal).

## 2. WireBulkParams V28

WIRE_FORMAT_VERSION = 28, total 5944 bytes, unchanged size from V26.
`bulk_params_apply()` rejects any payload whose format_version != 28 or whose
length != sizeof(WireBulkParams).

| # | Field | Bytes | Contents |
|---|---|---|---|
| 1 | header | 16 | format_version, platform_id, num_channels, num_output_channels, num_input_channels, max_bands, payload_length u16, fw_major u16, fw_minor u16, reserved u32 |
| 2 | global | 16 | preamp_gain_db f32, bypass, loudness_enabled, loudness_output_mask u16, loudness_ref_spl f32, loudness_intensity_pct f32 |
| 3 | crossfeed | 16 | enabled, preset, itd_enabled, output_pair_mask, custom_fc f32, custom_feed_db f32, reserved u32 |
| 4 | legacy | 16 | gain_db[3] f32, mute[3], reserved |
| 5 | delays | 68 | delay_ms[17] f32 |
| 6 | crosspoints | 576 | [8][9] x 8 B {enabled, phase_invert, rsvd[2], gain_db f32} |
| 7 | outputs | 108 | [9] x 12 B {enabled, mute, rsvd[2], gain_db f32, delay_ms f32} |
| 8 | pins | 8 | num_pin_outputs, pins[5], rsvd[2] |
| 9 | eq | 3264 | [17][12] x 16 B {type, bypass, reserved[2] = LT target Q u16 LE Q*512, freq, q, gain_db} |
| 10 | channel_names | 544 | char[17][32] |
| 11 | i2s_config | 16 | output_types[4], bck_pin, mck_pin, mck_enabled, mck_multiplier, clock_pin_mode_p1, bck_pin_slave, rsvd[6] |
| 12 | leveller | 20 | enabled, speed, lookahead, rsvd, amount f32, max_gain_db f32, gate_threshold_db f32, detector_mask, apply_mask, rsvd[2] |
| 13 | preamp | 32 | preamp_db[8] f32 |
| 14 | master_volume | 16 | master_volume_db f32, rsvd[12] |
| 15 | input_config | 16 | see below; CHANGED at V28 |
| 16 | lg_sound_sync | 16 | enabled + read-only present, volume, muted; rsvd[12] |
| 17 | user_volume | 16 | user_volume_db f32, user_mute, rsvd[11] |
| 18 | dac_hw_mute | 16 | mirrors DacHwMuteConfig |
| 19 | crossovers | 1088 | WireBandParams[17][4]; input rows zeroed/skipped |
| 20 | adat_config | 8 | enabled, pin, rsvd[6] |
| 21 | psybass | 24 | enabled, rsvd0, output_mask u16, cutoff_hz, harmonics_db, drive_db, character_pct, original_db |
| 22 | upmix | 44 | enabled, center_mode, surround_mode, presence_q1 int8 dB*2, 10 floats |

WireInputConfig at V28 (bulk_params.h:203-233), no reserved bytes left:
0 input_source; 1 spdif_rx_pin; 2 i2s_rx_pin; 3 i2s_input_rate (0 44.1k, 1
48k, 2 96k); 4 i2s_input_channels; 5-7 i2s_rx_pin_ext[3]; 8-10
spdif_rx_pin_ext[3] (was [2]); 11 spdif_rx_enabled_ext_p1 (shifted +1); 12
i2s_clock_mode (shifted); 13 adat_input_pin (shifted); 14
adat_input_enabled_p1 (shifted); 15 adat_clock_mode_p1 (shifted).

V27: UPMIX_CENTER_OFF = 2 enum widening only. V28: the 5-byte shift above.
A host that version-gates on packet size alone will silently misparse.

## 3. Control Surfaces: shipping caps_version is 13

Hosts must build the UI from REQ_GET_CS_CAPS at runtime.

### 3.1 Component types (control_surfaces.h:104-117)

0 NONE; 1 BUTTON (1 GPIO, press/long/double); 2 SWITCH (1, level-follow); 3
POT (1, ADC GPIO 26-28); 4 ENCODER (2); 5 LED (1); 6 LED_PWM (1 + PWM slice);
7 IR (1, container, 16 sub-slots); 8 DISPLAY (2: SDA even, SCL odd; index =
model 1-8, value = 7-bit I2C addr, 0 = default).

CS_MAX_BINDINGS 16, CS_GPIO_UNUSED 0xFF, CS_MAX_IR_COMMANDS 16, CS_MAX_GROUPS
8, CS_MAX_MACROS 8, CS_MAX_MACRO_STEPS 8, CS_MAX_DISPLAY_PAGES 16, CS_NAME_LEN
32.

### 3.2 Nouns 0..56 (CS_NOUN_COUNT 57)

0 USER_VOLUME, 1 MASTER_VOLUME, 2 USER_MUTE, 3 LOUDNESS, 4 CROSSFEED, 5
LEVELLER, 6 PRESET, 7 INPUT_SOURCE, 8 CLIP, 9 EQ_BYPASS, 10 LG_SYNC, 11
CROSSFEED_PRESET, 12 CROSSFEED_ITD, 13 LEVELLER_AMOUNT, 14 LEVELLER_SPEED, 15
LEVELLER_LOOKAHEAD, 16 PREAMP, 17 OUTPUT_GAIN, 18 OUTPUT_MUTE, 19
OUTPUT_ENABLE, 20 FILTER_FREQ, 21 FILTER_GAIN, 22 FILTER_Q, 23 FILTER_TYPE, 24
FILTER_BYPASS, 25 SIGGEN, 26 DAC_MUTE_TEST, 27 CLIP_CH, 28 LEVEL, 29
SPDIF_LOCK, 30 SAMPLE_RATE, 31 USB_STREAMING, 32 ADAT_ACTIVE, 33 LG_PRESENT,
34 LG_MUTED, 35 UPMIX, 36 UPMIX_CENTER_MODE, 37 UPMIX_SURROUND_MODE, 38
UPMIX_STRENGTH, 39 UPMIX_WIDTH, 40 UPMIX_PRESENCE, 41 PSYBASS, 42
PSYBASS_CUTOFF, 43 PSYBASS_HARMONICS, 44 PSYBASS_DRIVE, 45 PSYBASS_CHARACTER,
46 PSYBASS_ORIGINAL, 47 OUTPUT_DELAY, 48 PRESET_RELOAD.

New since 9776c2f: 49 LOUDNESS_SPL (dB SPL 40..100); 50 LOUDNESS_INTENSITY (%
0..127); 51 INPUT_LEVEL_MAX (read-only dB -60..0); 52 MACRO (enum 0..7,
actions SET + IND_EQUALS only); 53 CPU_LOAD (read-only %); 54 DISPLAY_PAGE
(enum 0..15); 55 DISPLAY_EDIT (bool, auto-clears after edit_timeout); 56
PAGE_VALUE (virtual; STEP/INC/DEC/TOGGLE only; value/step/range must be 0;
rejected as macro step and as display page noun).

Kinds: CONTINUOUS 0 / BOOL 1 / ENUM 2. Units: NONE 0 / DB 1 (8.8 signed) / HZ
2 (int, log step) / Q 3 (8.8, log step) / PERCENT 4 / MS 5 (default step 0.1
ms). Target kinds: NONE 0 / INPUT_CH 1 / OUTPUT_CH 2 / DSP_CH 3 / DSP_BAND 4.
Noun flag CS_NDF_DEFERRED 0x01.

### 3.3 Actions (control_surfaces.h:227-241)

0 ADJUST (pot), 1 STEP (encoder), 2 INC, 3 DEC, 4 TOGGLE, 5 SET, 6 FOLLOW
(switch), 7 TRIGGER, 8 IND_EQUALS (LED), 9 MOMENTARY, 10 IND_ABOVE, 11
IND_LEVEL (PWM). Button events: 0 PRESS, 1 LONG (>= 500 ms), 2 DOUBLE (350
ms). Bindings may share a GPIO when their events differ.

### 3.4 Binding flags

INVERT 0x01, REVERSE 0x02, WRAP 0x04, ACCEL 0x08, REPEAT 0x10, GROUP 0x20,
LINK_ABS 0x40, GROUP_ALL 0x80. Byte is full.

### 3.5 CsBinding, 24 bytes (control_surfaces.h:370-395)

0 type; 1 noun; 2 action; 3 flags; 4 gpio[0]; 5 gpio[1] (0xFF unless
two-pin); 6 event; 7 target (channel, or group index when GROUP flag); 8 index
(band for DSP_BAND nouns; display model for DISPLAY); 9 base_bright (LED_PWM
ceiling % 1-100, 0 = full; other types write 0); 10 value i16 (SET/MOMENTARY
target, IND_EQUALS/IND_ABOVE comparand, I2C addr on DISPLAY); 12 step i16 (0
= per-unit default); 14 range_min i16; 16 range_max i16 (both 0 = full range);
18 on_delay u16 (0.1 s, LEDs); 20 off_delay u16; 22 reserved2[2] = 0.

### 3.6 IrCommand, 16 bytes (control_surfaces.h:403-414)

0 noun; 1 action; 2 flags (WRAP|REPEAT|GROUP); 3 target; 4 index; 5 protocol
(NONE 0 / NEC 1 / RC5 2 / RC6 3 / HASH 4); 6 value i16; 8 step i16; 10
reserved[2]; 12 code u32. Learn states IDLE 0, ARMED 1, DONE 2, TIMEOUT 3.
Learn results also push notification 0x0A.

### 3.7 CsGroup, 40 bytes (control_surfaces.h:440-445)

0 target_kind (INPUT_CH / OUTPUT_CH / DSP_CH; 0 = empty); 1 reserved[3]; 4
member_mask u32; 8 name[32]. Empty = all-zero. Link laws: STEP/INC/DEC step
each member from its own value; ADJUST moves the group mean keeping offsets
unless LINK_ABS (valid only on ADJUST); bool/enum groups follow the
lowest-numbered member (anchor). Grouped indicators OR by default, AND with
GROUP_ALL; IND_LEVEL follows the maximum and rejects GROUP_ALL.

### 3.8 Macros

CsMacroStep 12 B: 0 noun; 1 action (SET/TOGGLE/INC/DEC/TRIGGER only); 2 flags
(WRAP|GROUP); 3 target; 4 index; 5 reserved; 6 value i16; 8 step i16; 10
pre_delay u16 (10 ms units). CsMacro 132 B: name[32] + step_count +
reserved[3] + steps[8]. CsMacroHeaderWire 36 B. Write steps first, header
last. One macro runs at a time; firing cancels the running one at a step
boundary. MACRO and PAGE_VALUE are rejected as step nouns.

### 3.9 CsDisplayCfg, 12 bytes (control_surfaces.h:500-509)

0 mode (FIXED 0 / CYCLE_SELECTED 1 / CYCLE_ALL 2); 1 home_page; 2 dwell u16
(0.1 s, min 10 in cycle modes); 4 overlay_hold u16 (0 = off); 6 brightness
0-255 (applies on next attach); 7 flags; 8 edit_timeout u16 (0 = manual
only); 10 reserved[2].

Flags: OVERLAY_ANY 0x01; EDIT_GATED 0x02 (arm-before-edit; ON by default from
page seeding at 112f35b); LABEL_ALIGN 0x0C (shift 2); VALUE_ALIGN 0x30 (shift
4), values LEFT 0 / CENTRE 1 / RIGHT 2, 3 rejected.

Seeded defaults: pages 0-3 = volume (LARGE), preset, input source, sample
rate; overlay hold 2 s, dwell 3 s, edit timeout 10 s, EDIT_GATED set.

### 3.10 CsDisplayPage, 4 bytes

{noun, target, index, flags}. Flags: ACTIVE 0x01, GROUP 0x02, LARGE 0x04, BAR
0x08 (continuous nouns only; INVALID_PAGE on bool/enum).

Display models 0-8: 1 LCD1602, 2 LCD2004, 3/4/5 char OLED 16x2/20x2/20x4,
6/7 SSD1306 128x64/128x32, 8 SH1106 128x64.

### 3.11 Status packets

CsStatusPacket 41 B: 0 last_status; 1 last_slot; 2 max_bindings; 3 dirty; 4
active_mask u16; 6 slot_status[16]; 22 ir_active_mask u16; 24 ir_learn_state;
25 ir_cmd_status[16]. last_slot tags: bare n = binding, 0x80|n = IR sub-slot,
0x40|n = group, 0x60|n = macro, 0x50 = display cfg, 0x50|page = display page,
0xFF = save/revert.

CsExtStatusPacket 24 B: {max_groups, max_macros, max_macro_steps,
macro_running (0xFF idle), macro_step, reserved[3], group_status[8],
macro_status[8]}.

CsDisplayStatus 8 B: {init_state (0 down / 1 init / 2 live / 3 error),
current_page (0xFF none), flags (bit0 overlay showing, bit1 edit armed),
model, nak_count u16, reserved[2]}.

### 3.12 Caps descriptors

CsTypeDesc 4 B {actions u16, pin_count, pin_class}. CsCapsHeader 44 B at v10+:
0 caps_version; 1 max_bindings; 2 type_count; 3 noun_count; 4 types[type_count]
(9 x 4 = 36); then max_ir_commands, max_groups, max_macros, max_macro_steps at
4 + 4*type_count. CsNounDesc 12 B {kind, enum_count, actions u16, min_q i16,
max_q i16, unit, target_kind, target_count, dflags}; actions == 0 means the
noun is unavailable on this platform.

### 3.13 Status codes (control_surfaces.h:607-637)

0x00-0x05 reuse PIN_CONFIG_*. 0x10 INVALID_SLOT, 0x11 INVALID_TYPE, 0x12
INVALID_NOUN, 0x13 INVALID_ACTION, 0x14 INVALID_VALUE, 0x15 PIN_NOT_ADC, 0x16
PENDING, 0x17 INVALID_TARGET, 0x18 INVALID_EVENT, 0x19 PWM_CONFLICT, 0x1A
EVENT_IN_USE, 0x1B BUSY, 0x1C FLASH_ERROR, 0x1D IR_IN_USE, 0x1E NO_IR, and new
0x1F INVALID_GROUP, 0x20 INVALID_MACRO, 0x21 INVALID_STEP, 0x22
DISPLAY_IN_USE, 0x23 PIN_NOT_I2C, 0x24 I2C_IN_USE, 0x25 INVALID_PAGE.

### 3.14 Host apply model

Every CS SET is a deferred, single-deep, live-only preview. Poll 0x87 until
PENDING resolves; BUSY means retry shortly. Binding-config changes do NOT push
notifications; re-read after writing. CS_SAVE persists everything; CS_REVERT
discards. `dirty` in the status packet is the unsaved-preview flag.

## 4. Notifications v2

Endpoint VENDOR_EP_IN 0x83, size 64, interval 10. It is a BULK IN endpoint
(not interrupt) and is always armed: when idle it returns a 1-byte IDLE packet,
so the host sees short reads. Size the read by actual_length.

Header: 0 version (2); 1 event_id; 2 flags; 3 seq (u8, wraps; gap = loss).

| ID | Name | Packet | Host action |
|---|---|---|---|
| 0x00 | IDLE | 1 B | discard |
| 0x01 | MASTER_VOLUME | 8 B [1,0,0,0,f32] | legacy; ignore |
| 0x02 | PARAM_CHANGED | 12 + size: [2,2,0,seq, off u16, size u16, src, 0,0,0, value] | copy into the local WireBulkParams at wire_offset and apply |
| 0x03 | BULK_INVALIDATED | 8 B [..., src, 0,0,0] | re-read GET_ALL_PARAMS |
| 0x04 | PRESET_LOADED | 8 B [..., slot] | toast; always followed by 0x03 |
| 0x05 | INPUT_FORMAT | 8 B [..., channels] | re-lay-out for 2/4/6/8 inputs |
| 0x07 | SIGGEN_STATE | 8 B [..., state, reason, signal_type, channel] | update generator UI |
| 0x08 | ADAT_STATE | 8 B [..., enabled, active, pin, 0] | RP2350 |
| 0x09 | I2S_SLAVE_STATE | 9 B [..., state, rate u32] | rate 0 until LOCKED |
| 0x0A | CS_IR_LEARN | 12 B [..., state, protocol, 0, 0, code u32] | learn result |
| 0x0B | ADAT_INPUT_STATE | 10 B [..., state, rate u32, clock_mode] | RP2350 |

Classify by event ID only. wire_offset = offsetof(WireBulkParams, field) or
array base + index * element size. Max notifiable size 52 B; larger falls back
to BULK_INVALIDATED. Sources: 0 UNKNOWN, 1 HOST_SET, 2 BULK_SET, 3 PRESET, 4
FACTORY, 5 GPIO (every CS dispatch), 6 INTERNAL, 7 UAC1, 8 UART, 9 I2C; treat
unknown as "someone else". Loss detected by seq gap; recover with a full bulk
read. USB backlog is dropped on bus reset.

## 5. Platform differences

Discovery: GET_PLATFORM (0x7F) {platform_id 0 RP2040 / 1 RP2350, fw_major,
fw_minor_patch_bcd, NUM_OUTPUT_CHANNELS}; WireHeader counts; CsNounDesc
actions == 0 marks unavailable nouns.

| Aspect | RP2040 | RP2350 |
|---|---|---|
| Inputs | 2 | 8 (active 2/4/6/8 by USB alt) |
| Outputs | 5 (4 SPDIF + PDM) | 9 (8 SPDIF + PDM) |
| NUM_CHANNELS | 7 | 17 |
| SPDIF TX | 2 | 4 |
| Pin outputs | 3 | 5 |
| I2S RX pairs | 1 | 4 |
| Max delay | 1024 samples | 2048 |
| Core 1 EQ outputs | 2..3 | 2..7 |
| Multitone oscillators | 8 | 16 |
| DSP path | Q28 fixed | float |
| MCK default pin | 21 | 13 (15, 21 possible) |
| Slave clock pair default | BCK 12 / LRCLK 13 | BCK 26 / LRCLK 27 |
| Upmixer | absent (SET stalls) | present |
| ADAT out | absent | present |
| ADAT in | round-trips, inert | present |

Channel space: [0..NUM_INPUT_CHANNELS-1] inputs (PEQ + metering, no
crossover), then outputs (PEQ + crossover + gain/delay/mute). CH_OUT_SUB =
NUM_CHANNELS - 1 is the PDM sub. Band space: PEQ storage MAX_BANDS 12 (0..9
active); crossover bands at wire 20..23; 10..19 rejected.

## 6. Other host-visible features

### 6.1 Presets

10 slots, 32-char names. Status PRESET_OK 0 / INVALID_SLOT 1 / SLOT_EMPTY 2
(reserved; load-on-empty applies factory defaults) / CRC 3 / FLASH_WRITE 4.
Startup modes SPECIFIED 0 / LAST_ACTIVE 1. Load is deferred and pushes
PRESET_LOADED then BULK_INVALIDATED.

Master volume mode: 0 INDEPENDENT (default; stored in directory, saved via
0xD6, read via 0xD7) / 1 WITH_PRESET. Output config mode: 1 WITH_PRESET
(default) / 0 INDEPENDENT (pins, output types, I2S MCK+BCK, SPDIF RX pin
become device-global, persisted by 0x52). Input source is per-preset always.

### 6.2 Firmware update

0xF0 responds 1, waits 100 ms, reset_usb_boot. Re-enumerates as 2E8A:0003
(RP2040) / 2E8A:000F (RP2350), drive RPI-RP2. Host copies the .uf2. No
firmware-side confirmation; the host owns the UX.

### 6.3 Identification

Serial 0x7E = USB iSerialNumber (flash unique ID). No user-settable device
name; names exist per preset, channel, CS slot, group and macro.

### 6.4 Buffer stats and metering

0xB0 BufferStatsPacket 44 B {num_spdif, flags (bit0 PDM active, bit1
streaming), sequence u16, SpdifBufferStats[4], PdmBufferStats}.

GET_STATUS wValue = 9 (canonical): peaks[NUM_CHANNELS] u16 LE, cpu0, cpu1,
clip_flags u32, active_input_channels => NUM_CHANNELS*2 + 7 bytes (41 on
RP2350, 21 on RP2040). The peak_clip_metering_spec is stale; trust
vendor_commands.c:1972-1991. wValue 0..2 legacy peaks; 3..8 PDM/SPDIF
over/underruns; 10..23 usb_audio_packets, alt setting, mounted, clk_sys, core
mV, sample rate, temperature centi-C, SPDIF DMA starvations total and per
instance 0..3, USB ring overruns, active input count; 24/25 loopback only,
STALL on release builds.

Peaks are raw block peaks with no ballistics; the host owns decay and hold.
Clip flags are a sticky 32-bit latch cleared by 0x83 (read-then-clear).
cpu1_load meaning depends on core1 mode.

### 6.5 USB error stats

Always zero under TinyUSB; hide or label unavailable.

### 6.6 Input switching

InputSource: 0 USB, 1 SPDIF1 (always enabled), 2 I2S, 3 ADAT (RP2350), 4
SPDIF2, 5 SPDIF3, 6 SPDIF4. Optional SPDIF inputs share one RX state machine;
a disabled input's pin is a stored preference only. Defaults: SPDIF RX GPIO
5/20/21/22 (21 clashes with RP2040 MCK default); I2S RX data pins 1/2/3/4.
SpdifRxStatusPacket 16 B {state (INACTIVE 0/ACQUIRING 1/LOCKED 2/RELOCKING
3), input_source, lock_count, loss_count, sample_rate u32, parity_errors u32,
fifo_fill_pct u16, reserved}.

### 6.7 SPDIF / ADAT / I2S / clocks

- Output type per slot is deferred. BCK default GPIO 14, LRCLK = BCK+1.
- I2S clock mode SET is deferred; GET returns live mode only. In slave mode
  SET_INPUT_RATE is stored + notified, no live effect.
- Clock-pin mode UNIFIED vs SPLIT; slave pair set via 0xC2 with role = 1. In
  SPLIT both pairs count as claimed for every pin validation.
- MCK uses CLK_GPOUT pins: RP2040 GPIO 21 only; RP2350 13 (default), 15, 21.
  128x or 256x. Forced off while I2S slave clocking is live. Pin change while
  enabled returns OUTPUT_ACTIVE 0x04; disable first.
- ADAT out (RP2350) runs at 44.1/48 kHz only, auto-suspends above; show
  rate_ok / active and event 0x08.
- ADAT in (RP2350) master/slave; default pin unset so reset clears.
- PIN_RESET_TO_DEFAULT = 0xFF on every single-pin SET; GPIO 0 is always GPIO 0.
- PIN_CONFIG_* codes: SUCCESS 0, INVALID_PIN 1, PIN_IN_USE 2, INVALID_OUTPUT
  3, OUTPUT_ACTIVE 4, INVALID_PARAM 5.

### 6.8 Core 1

Modes IDLE / PDM / EQ_WORKER. A blocked output enable is silently skipped;
read back after writing and explain via 0x7B.

### 6.9 LG Sound Sync

Live-only flag persisted on preset save. Status {enabled, present, volume
0..100 (0xFF never decoded), muted}.

### 6.10 DAC hardware mute

Config {enabled, active_low (def 1), pin (def 11; 0xFF none), hold_ms 1..500
def 5, release_ms 0..500 def 0}. Test pulses ~1 s.

### 6.11 Silent state changes

Preset load/save/delete, bulk SET, factory reset and stream resyncs are silent
by construction; nothing is host-configurable. The DAC mute is reserved for
hardware reconfiguration.

### 6.12 UART / I2C control interfaces

Both ship disabled. UartCtrlConfig 8 B {enabled, tx_pin (pin%4==0), rx_pin
(pin%4==1), notify_enable, baud 9600..1000000 def 115200}. I2cCtrlConfig 8 B
{enabled, sda (even), scl (odd), address 0x08..0x77 def 0x42, rsvd[4]}.
Defaults UART 16/17, I2C 18/19. SETs are USB-only. CtrlIfaceStatus 8 B
{uart_last_status, uart_live, i2c_last_status, i2c_live, proto_version,
rsvd[3]}. Chunked bulk opcodes are refused over UART/I2C.

## 7. Gotchas checklist for the host

1. CS caps header is 44 bytes; read type_count and index post-table fields.
2. CS status is 41 bytes.
3. SPDIF input config is 6 bytes.
4. Preset directory is 7 bytes.
5. GET_STATUS is NUM_CHANNELS*2 + 7 with 32-bit clip flags.
6. WireInputConfig offsets 11-15 shifted at V28; size unchanged.
7. Filter types 12 and 13 are new.
8. EDIT_GATED is set by default; clear it for direct adjust.
9. All CS SETs are deferred previews; poll 0x87; CS_SAVE to persist.
10. Macros: steps before header.
11. CS_MAX_IR_COMMANDS is 16; read from caps.
12. Upmix center_mode may be 2.
13. USB error stats are permanently zero.
14. GET_STATUS 24/25 stall on release builds.
15. GET_I2S_CLOCK_MODE does not reflect a pending SET.
