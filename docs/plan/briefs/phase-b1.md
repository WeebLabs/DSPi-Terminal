# Phase B1 brief: protocol bump to firmware v1.1.6-beta4 (wire V32)

You are working on DSPi Terminal, a Rust workspace, in a git worktree of
branch `console-parity`. Read first, in order:

1. `docs/plan/PLAN-beta4.md`: the plan; you are doing phase B1. Section 2
   holds settled decisions (V32 only; firmware update and on-graph
   editing are out of scope).
2. `docs/plan/survey-firmware-beta4.md`: every host-visible firmware
   change since the vendored baseline. This is your specification, but
   **every protocol fact you encode must cite a firmware header line**, so
   confirm each against the headers in `/Users/weeblabs/DSPi/firmware/DSPi/`
   (commit `557bce7`, branch `release/v1.1.6`). Where the survey and the
   headers disagree, the headers win; note the disagreement in your
   report.
3. `docs/firmware-bump.md`: the bump procedure. Two parts of it are
   wrong: it lists only six headers, and it refers to `FieldDesc` rows,
   which do not exist in this codebase. Fix the document as part of this
   phase.
4. `docs/plan/PLAN.md` section 5: working agreements. In particular, no
   em-dashes anywhere (code, strings, docs); nothing about device shape
   compiled in; protocol facts cite header lines.

Do not modify anything in `/Users/weeblabs/DSPi` or `/Users/weeblabs/DSPi
Console`. They are read-only references.

## Scope

### 1. Vendor the headers

From `/Users/weeblabs/DSPi/firmware/DSPi/` at `557bce7`, copy into
`crates/dspi-proto/firmware/`: the six existing headers (`config.h`,
`bulk_params.h`, `control_surfaces.h`, `psybass.h`, `upmix.h`,
`siggen.h`) and the new `limiter.h`, `tube.h`, `subharm.h`, `rta.h`,
`rta_fft.h`, `rta_bass.h` and `notify.h`. Update `PROVENANCE.toml`
(branch, commit `557bce7`, its commit date from `git -C
/Users/weeblabs/DSPi log -1 --format=%ci 557bce7`, vendored 2026-09-29, and
the file list) and regenerate `SHA256SUMS`.

### 2. Generator (`crates/dspi-proto/build.rs`)

- Add the new headers to `HEADERS`.
- Add `SUBHARM_`, `TUBE_`, `LIMITER_`, `RTA_` and any other prefixes the
  host needs to `DEFINE_GROUPS`, and float-valued ones to
  `FLOAT_PREFIXES`. Watch the build log for "platform-conditional, not
  emitted" and handle those as the bump document says (they must come
  from a probe, not a constant). `notify.h` event ids and `ParamSource`
  may be enums rather than defines; if the generator cannot lift them,
  keep them hand-typed in `dspi-session/src/notify.rs` but add a test
  that checks every value against the vendored `notify.h` text, as other
  hand tables do (look for existing examples of header-checked tables in
  `packets.rs` or its tests and follow that pattern).
- Append `subharm` (36), `tube` (48) and `limiter` (108) to the section
  table in `emit_wire_layout`, after `("upmix", 44)`.

### 3. Wire layer

- Pin `WIRE_FORMAT_VERSION` 32 and 6136 bytes in the tests in
  `crates/dspi-proto/src/lib.rs`, and update `WireHeader::decode`
  (`wire.rs:91-96`) so V32 is the accepted version.
- Add offset tests for the three new sections at 5944, 5980 and 6028,
  including field offsets within each (survey section 2), and a test that
  no earlier section moved.
- The refusal path for a non-V32 device must produce a user-facing
  message that names the device's firmware version (decoded from
  GET_PLATFORM, below) and says the firmware needs updating to v1.1.6
  beta 4. Find where the current version-refusal error surfaces in the
  session and TUI layers and make its wording follow this. Use the
  Console's phrasing style; check `/Users/weeblabs/DSPi Console` for an
  existing string about wire-format mismatch and reuse it if one exists.

### 4. Registry (`crates/dspi-proto/src/registry.rs`)

Add rows for all 42 new opcodes so `tests/coverage.rs` reports 244 of
244 with an empty exclusion list (or an exclusion with a written reason
for anything genuinely not host-driveable; runtime reads such as meters,
status and RTA frames are host-driveable reads, not exclusions).

- Subharm: 0x10-0x1F, 0x2C-0x2F, 0xA9-0xAE. Follow the psybass rows
  (`bass.*`) as the model; parameter names under `sub.*`. Ranges from
  `subharm.h` (levels -30..+12, not +6).
- Tube: 0x3E / 0x3F, indexed by `TUBE_PARAM_*` in wValue, **f32 payload
  for every parameter**, including enable, mask, type and rectifier.
  Follow the upmixer's `Wv::Fixed(index)` pattern (`up.strength`).
  Parameter names under `tube.*`.
- Limiter: one opcode 0x81 in both directions, `wValue = (output << 8) |
  index`. Add a `WValue` variant for this packing (see `Target`/`WValue`
  in registry.rs and the packing in `crates/dspi-session/src/write.rs`),
  targeted per output channel using the existing `ChannelMap` rules (the
  limiter's output index is the output number, not the unified channel
  index; confirm from `limiter.h` and `vendor_commands.c`). Support
  output `0xFF` (all outputs) for SET. Parameter names under `limit.*`.
- RTA 0x08-0x0F, CS aux 0x04-0x07, build info 0x80: registry rows of the
  appropriate kind (packet or read), with codecs from step 5.

Ranges are hand-typed in rows today; follow the existing convention but
take every number from a header line.

### 5. Codecs (`crates/dspi-proto/src/packets.rs`)

Each with size and offset tests, citing header lines:

- CS caps header grows from 44 to 52 bytes (`type_count` 11). Confirm the
  decoder finds the post-table fields at `4 + 4 * type_count`, never at a
  fixed offset; add a test with type_count 11.
- `CsBinding` byte 22 is `extras` (byte 23 must be 0). Constants
  `CS_AUX_X_BOOT_ON`, `CS_AUX_X_BOOT_SAVED`, `CS_AUX_X_LINEAR`.
- `CS_UNIT_MS_LOG` (6): value, range, min_q and max_q are plain integer
  ms; step is 8.8 octaves as for HZ.
- `CS_TARGET_AUX` (5), `CS_TYPE_AUX_OUT` (9), `CS_TYPE_AUX_PWM` (10),
  `CS_STATUS_INVALID_AUX` (0x26) with the Console's wording for it ("The
  target isn't an auxiliary output, or a level control needs a dimmable
  one", check the Console source).
- CS noun table: add nouns 57 to 78 to the hand-maintained noun table in
  `packets.rs` with their kind, unit, range and target from
  `control_surfaces.h` and the survey. (The UI labels are phase B7; you
  only need the protocol table here, but keep any existing
  table-completeness tests passing, which may mean adding placeholder
  labels in `crates/dspi-tui/src/settings/cs_model.rs`; if so, use the
  Console's names from `DSPi_ConsoleApp.swift` around lines 7156-7183,
  and for limiter nouns 74-78, which the Console lacks, use "Limiter",
  "Limiter Threshold", "Limiter Release", "Limiter Link", "Limiter Gain
  Reduction").
- Aux state block (0x05 wValue 0xFFFF): 48 bytes, `state[16]` then
  `level_q8[16]` u16 LE.
- RTA: `RtaConfig` (12), `RtaCaps` (16), band-centre chunks,
  `RtaBandFrame` (82), bin-frame header (16) with the tail sequence byte
  rule, `RtaStatus` (24). Level byte to dBFS: `(v - level_zero) * 0.5`,
  with `level_zero` from caps. Reject protocol versions other than 3.
- Subharm meter: `n_outputs` x u16 LE (length from the reply, not a
  constant).
- Limiter meter (index 0x80): `n_outputs` x u16 LE in 0.01 dB; limiter
  status (index 0x81): 4 bytes.
- Build info (0x80): 64 bytes; describe string, date.
- GET_PLATFORM (0x7F): request 7 bytes; decode minor and patch from bytes
  4-5 when at least 6 arrive, else from the byte-2 nibbles; beta from byte
  6 when 7 arrive, else 0. A version type with ordering `(major, minor,
  patch, beta == 0 ? 256 : beta)` and display "1.1.6 beta 4" (final:
  "1.1.6"). Follow the Console's `FirmwareVersion.swift` for display and
  the "early beta" case (a short reply that claims 1.1.6 or later).

### 6. Session layer (only what B1 needs)

- `crates/dspi-session/src/probe.rs`: read GET_PLATFORM with length 7 and
  use the new version type; read build info (tolerate a STALL); parse CS
  caps v20. Add feature probes for subharm (0x11), tube (0x3F index 0),
  limiter (0x81 index 0x81) and RTA (0x0A), on the pattern of the
  existing `probe_features` rows.
- `crates/dspi-session/src/notify.rs`: decode `NOTIFY_EVT_CS_AUX` (0x0C,
  9 bytes: ver, evt, flags, seq, slot, state, level_q8 u16 LE, src) into
  a typed event. `DeviceState` handling of it is phase B2; for now store
  it or pass it through as the other runtime events are.
- The notification reader: the endpoint now NAKs while idle and sends a
  1-byte IDLE only every 100 ms. Make sure the reader's timeout and
  shutdown logic tolerate a read that blocks about 100 ms, and that the
  mock transport tests cover it.
- Mock transport and fixtures (`crates/dspi-tui/src/shell/fixture.rs`,
  test fixtures under `tests/fixtures/`, and anything that builds a V28
  bulk image) must produce V32 images so every existing test passes.

### 7. CLI and docs

- `dspi --version` prints wire V32 and 244 opcodes; `dspi dump` shows the
  beta-aware firmware version and the build info.
- Regenerate or update `docs/wire-format.md` for V32; add the gotchas in
  survey-firmware-beta4.md section 6 that affect a host to
  `docs/firmware-notes.md`, and move its baseline line to `557bce7`;
  update `docs/firmware-bump.md` (header list; remove `FieldDesc`
  references and describe what actually exists); update README's
  version line (V28/202/v13 claims) to V32/244/v20.

## Out of scope for B1

The `DeviceState` sections, snapshot diff and preset-file blocks for
subharm, tube and limiter (B2); any new screen or panel (B4 to B8); UI
labels beyond placeholders needed to keep tests passing (B7).

## Exit criteria

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --
  -D warnings`, `cargo test --workspace` and `cargo build --workspace
  --release` all pass.
- Coverage test reports 244/244.
- New tests: V32 size and version; section and field offsets for subharm,
  tube and limiter; every new codec's size and offsets; GET_PLATFORM with
  4-, 6- and 7-byte replies and the version ordering; CS caps with
  type_count 11; the CS_AUX event; the MS_LOG unit; the refusal message
  for a V28 device.
- No em-dashes introduced (`grep -rn $'\u2014'` over changed files).

## How to work and report

Commit in small, coherent commits on your worktree branch, with subjects
in the style of `git log` on this branch (plain sentences, imperative, no
phase prefixes needed) and ending with the line:

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>

Do not push. When done, report: the commits (hash and subject), the test
counts before and after, the coverage number, every place where the
survey and the headers disagreed, anything you had to decide that the
brief did not settle, and anything you left undone with the reason.
