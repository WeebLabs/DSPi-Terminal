# DSPi Terminal: Redesign Specification

*Status: implemented through M6. Revision 3, 2026-07-27.*

**Implementation notes.** Where the build diverged from this document, the
document is wrong and the code is right; the divergences are listed in section
18. The largest is section 3.2 mechanism 2: bulk writes turned out to be
strictly version-locked rather than tolerant, which the spec had backwards.

**Firmware baseline: `release/v1.1.5` @ `9776c2f` (2026-07-25), in sync with
`origin/release/v1.1.5`.** All protocol facts here were read from committed
content on that branch: 190 vendor opcodes, wire format **V26**,
`WireBulkParams` **5944 bytes**, CS caps v4, FW version 1.1.5.

We track the release branch, not `main`. Every future firmware bump re-pins this
line and re-runs the procedure in section 3.4.

Companion references, pinned:

| Repository | Branch | Commit | Used for |
|---|---|---|---|
| `WeebLabs/DSPi` | `release/v1.1.5` | `9776c2f` | **Protocol source of truth** |
| `WeebLabs/DSPi-Console` (macOS) | `release/v1.1.5` | working tree | Bode math, graph behaviour, Control Surfaces UX, AutoEQ pattern, multi-device model |
| `WeebLabs/DSPi-Console-Windows` | `master` | `81ae00b` (2026-07-27) | `.dspipreset` schema |

The Windows Console is under active development, so its pin is the one most
likely to move; re-check before M6.

Writing style note: this doc avoids em-dashes, per DSPi project convention.

---

## 0. Summary

`dspi-term` is a cross-platform, full-screen terminal application that exposes
**every** parameter of the DSPi firmware, graphs channel responses at parity with
the DSPi Console, and is usable by someone who has never read a command
reference.

Two ideas carry the design:

1. **A single declarative parameter registry** from which the completion engine,
   the palette, the panel widgets, the validators, the help text, the export
   schema, and the control-surface binding UI are all generated.
2. **Nothing about the device's shape is compiled in.** Channel counts, band
   counts, matrix dimensions, filter types, and feature availability are all
   discovered at connect. Section 3 is the foundational section of this document.

Decisions, all settled:

| Decision | Choice |
|---|---|
| Stack | Rust + `ratatui`, USB via `nusb` (no libusb dependency) |
| Interaction | Full-screen TUI, plus `Ctrl-P` palette and a `:` command line |
| Remote control | Local USB now; transport is a trait so remote is not a rewrite |
| Extensibility | Foundational, not deferred. See section 3 |
| AutoEQ | Bundle the database; user-supplied and online-update paths provisioned |
| Multi-device | Aware of all connected devices, one active at a time, switchable |
| File interchange | Import and export Console filter files and `.dspipreset` files |
| Repository | Clean `main` in this repo |
| Documentation | All notes stay in this repo. The firmware repo is not touched |
| Navigation | Ten panels plus two global overlays (Graph, Meters) |
| Invocation | `dspi` alone opens the TUI; `dspi <command>` runs one-shot and exits |
| Filter layout | Stacked, graph full-width above the band table, adjustable split |
| All-channel view | Small multiples grid, with overlay as a second mode |
| Meters | Compact always-on rail, expandable to a full meter bridge |

---

## 1. What exists today, and what survives

### 1.1 The current app

`dspi_cli.py` (868 lines) is a `prompt_toolkit` REPL. It is not merely outdated,
it is wrong against current firmware in ways that make it unusable:

| Defect | Current code | Reality |
|---|---|---|
| Wrong VID | `0x2e8a` | `0x2E8B` |
| Wrong interface | claims interface `0`, `wIndex=0` | vendor interface is `2`, `wIndex` must be `2` |
| Wrong band packing | `wValue = (ch<<8)\|(band<<4)\|param` | band field is 5 bits: `(band<<3)` |
| Channel model | 5 channels, `Master L/R` + 3 outputs | 7 (RP2040) or **17** (RP2350); there is no "master" |
| Band count | 2 bands on output channels | 10 active PEQ bands per channel, plus 4 crossover bands |
| Filter types | 6 | 12 PEQ types plus 32 crossover encodings |
| Write direction | all writes as OUT | 20+ opcodes are "write-as-read" and STALL as OUT |
| Deferred writes | none | flash and pipeline ops need backoff and retry |
| Feature coverage | ~10 opcodes | **190** opcodes defined in firmware |

The log file in this repo is 29 lines of `Could not detach kernel driver: Access
denied`, which is the honest summary.

**Verdict: rewrite. Nothing in `dspi_cli.py` is load-bearing.** Keep it in git
history only.

### 1.2 What is worth keeping

| Asset | Location | Use |
|---|---|---|
| **Firmware headers** | `DSPi/firmware/DSPi/{config,bulk_params,control_surfaces,psybass,upmix,siggen}.h` | **The authoritative source.** Vendored and parsed at build time (section 3.3). |
| **Feature specs** | `DSPi/Documentation/Features/*.md` | 43 specs. `control_surfaces_spec.md` (1339 lines), `upmixer_spec.md`, `psychoacoustic_bass_spec.md`, `test_signals_spec.md` carry semantics the headers do not. |
| **Vendor command reference** | `DSPi/Documentation/commands.md` | Useful for transport conventions, deferred-write behaviour, and the hazard analysis in §22. **Stale on specifics.** See 1.4. |
| **Rust core** | `DSPi Console for Linux/core/src/` | 2709 lines: `protocol.rs`, `dsp_math.rs`, `state.rs`, `preset.rs`. Seeds `dspi-proto`, but predates the V16 channel model, so treat the DSP math as the reusable part and re-derive the rest. |
| **Bode math** | `DSPi Console/DSPi Console/DSPMath.swift` (829 lines) | Reference for magnitude, phase, unwrap, and crossover pole prototypes (Butterworth / Bessel / Linkwitz-Riley). |
| **Graph behaviour** | `DSPi Console/DSPi Console/GraphView.swift` | 201 log points, 10 Hz to 20 kHz, output gain as constant offset, phase on a scaled right axis, identical-curve grouping. |
| **Control Surfaces UX** | `DSPi Console/DSPi_ConsoleApp.swift:1843` (`ControlSurfacesSettingsTab`) | The behavioural reference for that panel, including the caps-driven pickers and the IR learn loop. |
| **`.dspipreset` format** | `DSPi-Console-Windows/DSPiConsole/Models/PresetDocument.cs` + `Services/PresetFileService.cs` | The interchange schema we must read and write. Section 12. |
| **AutoEQ resolution** | `DSPi Console/AutoEQ/AutoEQManager.swift` | The bundled-with-user-override pattern to replicate. Section 12.3. |
| **Multi-device model** | `DSPi Console/USBDevice.swift` | `availableDevices`, identity by serial, hot-plug rescan. Section 4.6. |
| **Filter file format** | `DSPiFilters.txt`, `Console/FilterFile.swift` | Human-readable interchange. Must round-trip. |
| **Reference host impl** | `DSPi/tools/dspi_test/device.py` | Retry and backoff behaviour: 4 retries, 150 ms. |

### 1.3 Rule on `dspictl`

`dspictl` (`github.com/suhlig/dspi`) is someone else's project. It is a **guide
and an oracle, not a source**.

Permitted: cross-checking our understanding of an opcode's real behaviour against
its hardware tests when firmware source is ambiguous; noting *that* it hit a
device quirk, then confirming that quirk against firmware source or hardware and
documenting it in our own words.

Not permitted: copying code, type layouts, function decomposition, command
naming, or CLI grammar; adopting its abstractions for convenience; taking a
dependency on it, forking it, or tracking its releases.

Every protocol fact in our code cites firmware source, never `dspictl`.

### 1.4 Source-of-truth rule

**Firmware headers outrank documentation. Always.**

This is not pedantry, and it is not a branch mix-up. On `release/v1.1.5` itself,
at `9776c2f`, these files disagree:

| File | Last commit on the branch | States |
|---|---|---|
| `Documentation/commands.md` | `f7143dc`, 2026-07-12 | wire **V14**, 3664 bytes, 121 opcodes, "Master L / Master R" channels |
| `firmware/DSPi/bulk_params.h` | `1b84765`, 2026-07-19 | wire **V26**, **5944 bytes** |
| `firmware/DSPi/config.h` | `4ab5922`, 2026-07-19 | **190** opcodes |

The code moved on 19 July; the doc last moved on 12 July. That is normal and
expected in an actively developed firmware, and it is exactly why the doc cannot
be the source of truth.

The gap is not cosmetic. V16 removed the master channel concept entirely: inputs
are now first-class channels with their own PEQ and metering, and RP2350 carries
8 inputs, not 2, giving 17 channels rather than 11.

An app built from that document would need a rewrite on day one. So:

1. Protocol constants are **generated from vendored headers at build time**
   (3.3), never transcribed.
2. `commands.md` is consulted for *behaviour* (transport conventions, deferred
   writes, hazards) and never for *layout or counts*.
3. Where the two disagree, the header wins and we note the discrepancy in
   `docs/firmware-notes.md` **in this repo**. We do not modify the firmware repo.

---

## 2. Product goals

1. **Nothing is unreachable.** All 190 opcodes driveable, including control
   surfaces, IR learn, upmixer, psychoacoustic bass, signal generator, ADAT in
   and out, I2S slave mode, multi-input S/PDIF, UART and I2C control interfaces,
   and DAC hardware mute.
2. **Zero prior knowledge to start.** Arrow keys alone must reach every setting.
3. **Full speed for experts.** Typed equivalents with completion at every
   position, and the UI teaches them by echoing.
4. **Graph parity with the Console.**
5. **Genuinely cross-platform.** One binary per target. Windows without Zadig,
   Linux with a shipped udev rule, macOS with no driver work, Raspberry Pi at a
   usable frame rate.
6. **Safe by construction.** Flash blackouts, silently-failing deferred writes, a
   bootloader jump with no undo, and an inviolable inter-slot phase alignment
   guarantee all have to be handled, not hoped about.
7. **Extensible without refactoring.** Section 3.

### 2.1 Non-goals

- Audio measurement (no capture, no REW replacement). The signal generator is
  exposed; analysis is not.
- Firmware building or flashing beyond `ENTER_BOOTLOADER` plus a UF2 copy helper.
- Replacing any Console. This is a peer.

---

## 3. Designing for firmware evolution

This is the foundational section. Everything downstream is shaped by it.

### 3.1 What actually changes, empirically

The requirement is not hypothetical. Here is what the firmware has done across
the last eleven wire versions, taken from the `WIRE_FORMAT_VERSION` history in
`bulk_params.h`:

| Change class | Example | What breaks if unprepared |
|---|---|---|
| **Whole-model change** | V16: unified channel model, inputs became first-class channels with PEQ and metering, "master" removed, 8 inputs on RP2350 | Everything. Hardcoded `master_l`, `ch - 2` output mapping, 2-row matrix |
| **New section appended** | V17 ADAT out, V23 psybass (24 B), V25 upmixer (44 B) | Fixed-size struct parse; packet size assertions |
| **Struct grew** | V18: leveller 16 to 20 bytes | Every offset after it |
| **Reserved bytes claimed** | V19 loudness mask, V20 crossfeed pair mask, V21 I2S clock mode, V22 Linkwitz `qp`, V24 ADAT input, V26 upmix presence | Silent misparse; writing zeros over real settings |
| **New opcodes** | 121 documented to 190 defined | Feature invisible in the app |
| **New enum values** | Filter types 6 to 12; crossover families | Unknown value clamped to Flat, **destroying a user's tuning** |
| **New capability tiers** | CS caps v1 to v4: 9 nouns to 49, 8 slots to 16, IR added | Hardcoded noun tables |
| **Platform divergence** | RP2040 7 channels / RP2350 17 | Arrays sized wrong |

An architecture that absorbs all eight classes without a refactor is the bar.

### 3.2 Nine mechanisms

**(1) No compiled-in topology.** There is no constant anywhere for channel count,
input count, output count, band count, matrix dimensions, pin-output count,
preset slot count, CS slot count, or IR command count. All are read at connect
from `WireHeader`, `GET_PLATFORM`, and the capability tables. A `ChannelMap`
built at connect is the only thing permitted to convert between index spaces:

```rust
pub struct ChannelMap { n_in: u8, n_out: u8, names: Vec<String> }

impl ChannelMap {
    fn inputs(&self)  -> Range<u8>;
    fn outputs(&self) -> Range<u8>;
    fn is_output(&self, ch: u8) -> bool;
    fn output_index(&self, ch: u8) -> Option<u8>;   // ch -> output idx
    fn channel_of(&self, out: u8) -> u8;            // output idx -> ch
    fn label(&self, ch: u8) -> &str;                // device-supplied name
}
```

`commands.md` says output index maps to `channel = output + 2`. That constant is
now `output + NUM_INPUT_CHANNELS`, which is 2 or 8 depending on platform. **No
call site does that arithmetic. Only `ChannelMap` does.** A future V27 that
reshapes the model again is a change to one struct.

A CI lint rejects suspicious numeric literals (2, 5, 7, 9, 10, 11, 16, 17) in
index arithmetic outside `ChannelMap` and the generated constants.

**(2) The bulk packet is parsed by a descriptor table, not a struct.** A
`#[repr(C)]` mirror of `WireBulkParams` would need editing for every one of the
eight change classes above. Instead:

```rust
struct FieldDesc {
    param:   ParamId,
    section: Section,
    offset:  u16,        // byte offset within the packet
    repr:    Repr,       // F32 | U8 | U16Le | Bool | Bits{byte,mask} | ...
    since:   WireVersion,
    until:   Option<WireVersion>,
}
```

Adding V26's upmix presence byte is one row with `since: 26`. Claiming a reserved
byte is one row. Appending a section is a set of rows plus a `Section` entry.
Nothing else moves.

Parsing rules that make this robust:

- Trust `header.payload_length`, not `size_of`. Read what is there.
- **Ignore trailing bytes you do not understand.** A newer device's longer packet
  parses fine; the unknown tail is retained verbatim.
- Skip any field whose `since` exceeds the device's `format_version`.
- On write, **round-trip the unknown tail unchanged** so we never zero out a
  field a newer firmware cares about.

**(3) Every enum is open.** No decode ever fails or clamps:

```rust
enum FilterType { Flat, Peaking, /* ... */ LinkwitzTransform, Unknown(u8) }
```

`Unknown(n)` renders as "Type 13 (unrecognised)", is **preserved byte-for-byte on
write**, and is excluded from the response curve with a visible note rather than
being treated as flat. Clamping an unknown filter type to Flat would silently
destroy a tuning made on newer firmware; that is the single worst failure mode
available to this app, and the type system prevents it.

**(4) Capability probing beats version comparison.** Prefer "does this opcode
answer?" over "is firmware >= X". Probe once at connect, cache per
(serial, firmware version). Version gates are used only where a probe is
destructive or ambiguous. This is already the house pattern: the Console's
Control Surfaces picker is built entirely from `csCaps` and `csNounDescs`
precisely so new firmware capabilities appear without an app change. **This spec
generalises that from one feature to the whole application.**

**(5) Panels declare fields, not widgets.** A panel is a list of `ParamId`s with
grouping and ordering hints. Widget selection comes from the registry's `Kind`.
A new registry row appears in its panel automatically, laid out correctly, with
completion, validation, and help, and with no panel code touched.

**(6) The registry is data.** Adding a firmware parameter is one row (section 5).
The coverage test (section 13) fails the build until every opcode in `config.h`
is either registered or explicitly excluded with a written reason.

**(7) The raw escape hatch.** An Expert-level "Raw" panel that can issue any
opcode by number with a hex payload and a chosen direction, plus a labelled hex
view of the bulk snapshot with unknown regions highlighted. **This guarantees
that a firmware feature shipping before app support is still reachable on day
one**, which is what actually removes the pressure to rush app releases. It is
also the fastest possible protocol debugging tool.

**(8) Deprecation without deletion.** Registry rows carry
`deprecated_since: Option<WireVersion>`. A removed firmware parameter keeps its
row, marked, so old `.dspipreset` files still import and the user is told what
was dropped rather than hitting a parse error.

**(9) Additive file formats.** Our own export schema is versioned, uses raw wire
values rather than enum names (following the `.dspipreset` precedent), and
readers ignore unknown keys while rejecting documents from a future major
version. Section 12.

### 3.3 Generated protocol constants

`build.rs` parses vendored copies of the firmware headers and emits Rust:

```
crates/dspi-proto/firmware/          vendored, with the source commit recorded
  config.h  bulk_params.h  control_surfaces.h  psybass.h  upmix.h  siggen.h
crates/dspi-proto/build.rs           parses them, emits opcodes + struct offsets
```

Emitted: all `REQ_*` opcode constants, `WIRE_FORMAT_VERSION`, section sizes and
offsets computed from the `_Static_assert`ed struct sizes, `CS_NOUN_*`,
`CS_TYPE_*`, `CS_ACT_*`, `CS_STATUS_*`, `PIN_CONFIG_*`, `PRESET_*`, filter type
enums, and all the `*_MIN` / `*_MAX` / `*_DEFAULT` range constants (psybass and
friends already define these, so ranges never need transcribing either).

The payoff: **a firmware bump is a file copy plus a build.** The compiler and the
coverage test then tell you exactly what is new and what moved. This converts
"keeping up with the firmware" from archaeology into a mechanical procedure.

### 3.4 The firmware bump procedure

Documented in `docs/firmware-bump.md` in this repo:

1. Copy the six headers into `crates/dspi-proto/firmware/`, record the commit.
2. `cargo build`. Generated constants update.
3. `cargo test`. The coverage test lists every new opcode; the offset test lists
   every moved or grown section.
4. For each new opcode: add a registry row, or add it to the exclusion list with
   a reason.
5. For each wire change: add `FieldDesc` rows with the correct `since`.
6. Add the parameter to a panel's field list if it needs a home, or let it live
   in the Raw panel until it does.
7. Update `docs/firmware-notes.md` with anything surprising.

Steps 4 and 5 are the only human work, and the tests tell you precisely what is
outstanding. **No step in this procedure involves changing existing code.**

### 3.5 What we accept

Honesty about the limits. A change that alters the *meaning* of an existing
parameter, rather than adding or moving one, still needs thought: V16 is the
example, where "master channel" stopped existing. The architecture above reduces
that to updating `ChannelMap`, the affected registry rows, and one panel's field
list, instead of a rewrite. That is the realistic target, and it is a large
improvement over any design that hardcodes topology.

---

## 4. Architecture

```
dspi-term/
├── crates/
│   ├── dspi-proto/                no I/O
│   │   ├── firmware/              vendored headers (source of truth)
│   │   ├── build.rs               header parser -> generated constants
│   │   ├── wire.rs                FieldDesc table + tolerant bulk codec
│   │   ├── registry.rs            THE PARAMETER REGISTRY  (section 5)
│   │   ├── caps.rs                capability discovery + ChannelMap
│   │   ├── dsp_math.rs            biquad, Bode magnitude/phase, xover cascades
│   │   └── model.rs               DeviceState
│   ├── dspi-transport/
│   │   ├── lib.rs                 trait Transport
│   │   ├── usb.rs                 nusb; enumeration, hot-plug, by-serial open
│   │   └── mock.rs                record/replay device
│   ├── dspi-session/
│   │   ├── session.rs             connect, poll, write, readback-verify
│   │   ├── devices.rs             multi-device registry + switching (4.6)
│   │   ├── notify.rs              bulk IN 0x83 consumer
│   │   ├── journal.rs             undo/redo + command echo
│   │   └── files.rs               .dspipreset, filter files, JSON, AutoEQ
│   ├── dspi-cmd/                  ONE grammar, both front-ends
│   │   ├── grammar.rs             registry-driven parser
│   │   ├── complete.rs            completion at any argument position
│   │   ├── oneshot.rs             argv -> session -> stdout, exit codes
│   │   └── shellcomp.rs           bash/zsh/fish/pwsh generation
│   ├── dspi-tui/
│   │   ├── app.rs · theme.rs · palette.rs · cmdline.rs (uses dspi-cmd)
│   │   ├── widgets/               bode.rs, meter.rs, matrix.rs, ...
│   │   └── panels/                one module per panel, each a field list
│   └── dspi/                      the binary; no args -> TUI, args -> one-shot
└── xtask/                         packaging, udev rule, release builds
```

**Rule: `dspi-tui` never touches `wire.rs` directly.** It reads `DeviceState` and
issues registry-addressed writes. That is what makes the coverage guarantee
mechanical rather than aspirational.

### 4.1 Dependencies

| Crate | Why |
|---|---|
| `ratatui` + `crossterm` | TUI; crossterm is the only backend with solid Windows console support |
| `nusb` | Pure-Rust USB, talks WinUSB / IOKit / usbfs directly. **No libusb, so no Zadig on Windows** (the firmware ships MS OS 2.0 descriptors), and trivial cross-compilation to `aarch64` |
| `tokio` (rt, sync, time) | Poll loop, notification reader, debounced writes. Single-threaded runtime suffices |
| `serde`, `serde_json`, `toml` | Config, export, session files |
| `nucleo` | Fuzzy matching for the palette |
| `clap` | The handful of CLI flags |

Nothing requiring a C toolchain, so cross-compiling to a Pi stays a one-liner.

### 4.2 Transport seam

```rust
pub trait Transport: Send {
    fn control_in (&mut self, req: u8, value: u16, len: u16) -> Result<Vec<u8>>;
    fn control_out(&mut self, req: u8, value: u16, data: &[u8]) -> Result<()>;
    fn notifications(&mut self) -> Option<NotifyStream>;   // bulk IN 0x83
    fn descriptor(&self) -> DeviceDescriptor;
}
```

`wIndex` is always `2` and is not a trait parameter, so no call site can get it
wrong. `MockTransport` backs the tests. A future `RemoteTransport` needs no
change above this line; that plus a documented frame format in
`docs/remote-protocol.md` is the whole "leave the seam" commitment.

### 4.3 The write path

One function, because every subtlety lives there:

```
write(param_id, target, value)
  1. Registry lookup; capability gate (platform, wire version, caps bit)
     -> reject with a human explanation, never a STALL.
  2. Validate and clamp against the registry range.
  3. Direction from ParamDesc.dir:
       Out         -> control_out(0x41, ...)
       In          -> control_in (0xC1, ...)
       WriteAsRead -> control_in (0xC1, ...)   the 20+ mutating IN opcodes
  4. Optimistic DeviceState update; field marked Pending.
  5. If deferred: busy-window protocol. Expect STALL or timeout, back off
     150 ms, retry up to 4x, poll GET_PLATFORM (0x7F) for liveness,
     reacquire by serial on re-enumeration.
  6. Readback and compare.
       match  -> Confirmed
       differ -> Rejected(actual): toast what the device actually did,
                 repaint with the true value.
  7. Journal for undo and command echo.
```

Step 6 is not optional. Deferred writes return "accepted" before validation runs,
and a bad DAC mute config is dropped silently. Three field states are always
visible:

```
  Freq   2856 Hz            confirmed
  Freq   2856 Hz  ·         pending
  Freq   2856 Hz  ⚠ 1000    rejected; device kept 1000
```

### 4.4 The read path

1. **Bulk snapshot.** `GET_ALL_PARAMS` (0xA0), or `GET_ALL_PARAMS_CHUNK` (0xA2)
   on Windows where WinUSB caps transfers at 4 KB. At 5944 bytes the chunked path
   is mandatory there, and 0xA2 snapshots under the bulk lock at offset 0.
2. **Targeted reads** for state outside the bulk packet: control surfaces,
   siggen, ADAT status, I2S slave status, UART and I2C config, S/PDIF RX status,
   buffer stats, preset directory.
3. **Notifications** on bulk IN `0x83`. `PARAM_CHANGED` carries the byte offset
   into the bulk packet, which the `FieldDesc` table maps straight to a
   `ParamId`. No per-opcode switch. `BULK_INVALIDATED`, `PRESET_LOADED`, or a
   `seq` gap triggers a full re-read.

Notifications are the primary freshness mechanism; polling is the fallback.
Meters are the exception and are always polled via `GET_STATUS` `wValue=9`, which
returns per-channel peaks, both CPU loads, and sticky clip flags in one transfer.
20 Hz desktop, 10 Hz Pi, 2 Hz when unfocused.

### 4.5 Capability discovery

Run once at connect, before any panel is built:

| Probe | Yields |
|---|---|
| `GET_PLATFORM` (0x7F) | Platform, firmware major/minor BCD, output count |
| `GET_SERIAL` (0x7E) | Identity for device selection and per-device config |
| `GET_ALL_PARAMS` header | `format_version`, `payload_length`, `num_channels`, `num_input_channels`, `num_output_channels`, `max_bands`. **This builds `ChannelMap`.** |
| `GET_CHANNEL_NAME` (0x9C) per channel | Device-supplied labels, used everywhere |
| `GET_CS_CAPS` (0x86) | Caps version, slot and IR command counts, per-type action masks, then per-noun descriptors with kind, unit, min, max, targets, and availability |
| `SIGGEN_GET_CAPS` (0xA8) | Type count, multitone max, valid channel mask, per-type parameter descriptors |
| Probe-and-catch | Opcodes with no caps bit: issue the GET once, treat STALL as absent, cache |

### 4.6 Multi-device

Mirrors the Console's model (`USBDevice.swift`): aware of every connected DSPi,
exactly one active.

- Enumerate all `2E8B:FEAA` devices, identified by **serial**, with hot-plug
  rescan. `nusb` provides a hotplug watcher on all three platforms.
- A device picker (`Ctrl-D`, and automatically at startup when more than one is
  present) lists serial, short name (last 8 of serial, as the Console does),
  platform, and firmware version. A user-assigned nickname is stored per serial
  in config and shown in preference to the serial.
- **One open session at a time.** Switching closes cleanly, discards nothing
  unsaved without asking, and re-runs capability discovery, because the other
  device may be a different platform entirely.
- Per-device state is keyed by serial: nickname, last active panel, graph channel
  visibility, and unsaved-change warnings.
- `--device <serial-or-nickname>` selects at launch. With one device connected
  and no flag, connect automatically and say nothing.
- Hot-unplug of the active device shows a clear disconnected state and
  auto-reconnects by serial when it returns. Hot-plug of an additional device
  never steals focus, it just appears in the picker.

---

## 5. The parameter registry

One table, roughly 300 rows, from which everything is generated.

```rust
pub struct ParamDesc {
    pub id:        ParamId,
    pub path:      &'static str,   // "bass.drive"  -- CLI/palette address
    pub label:     &'static str,   // "Drive"
    pub plain:     &'static str,   // one plain-language line
    pub group:     Group,          // which panel owns it
    pub level:     Level,          // Simple | Advanced | Expert
    pub kind:      Kind,           // Float{unit,min,max,step,log} | Bool
                                   //   | Enum{variants} | Text{max}
                                   //   | Trigger | Packet{codec}
    pub target:    TargetKind,     // None | InputCh | OutputCh | DspCh
                                   //   | DspBand | Slot | PinOut | PresetSlot
    pub set:       Option<Opcode>,
    pub get:       Option<Opcode>,
    pub dir:       Dir,            // Out | In | WriteAsRead
    pub wvalue:    WValueRule,     // how target/index pack into wValue
    pub bulk:      Option<FieldRef>, // link into the FieldDesc table
    pub requires:  Requires,        // Platform | WireVersion | CapsBit | Probe
    pub hazard:    Hazard,          // None | Audible | Deferred | Flash
                                    //   | Reconfig | Irreversible
    pub persists:  Persistence,     // LiveOnly | WritesFlash | DeviceGlobal
    pub deprecated_since: Option<WireVersion>,
    pub help:      &'static str,
    pub see_also:  &'static [ParamId],
}
```

Ranges come from the generated constants where the firmware defines them
(`PSYBASS_DRIVE_MIN` and friends), so they are never transcribed and never drift.

### 5.1 What the registry generates

| Consumer | Derived how |
|---|---|
| **Command grammar** | `path` is the command; `target` and `kind` give the argument shapes |
| **Completion** | At argument position *n*, the legal value set: enum variants, channel names from `ChannelMap`, valid band indices, range hints |
| **Palette** | One entry per row, fuzzy-matched over path, label, plain, help |
| **Widgets** | `Kind` picks the control. Panels declare fields, not widgets |
| **Validation** | Clamp and reject before the wire, with the range in the message |
| **Help** | `?` renders plain, help, range, unit, hazard, persistence, see-also |
| **Export schema** | JSON keys are `path` |
| **CS binding UI** | CS nouns map onto registry rows, so binding a knob browses the same tree the user already knows |
| **Coverage test** | Every opcode in `config.h` is registered or explicitly excluded |

### 5.2 Path naming

Paths read as English and complete well. Channel addresses accept device-supplied
names (slugified), canonical forms, and indices interchangeably.

```
vol.user · vol.master · vol.mute · bypass · pre.<in>
eq.<ch>.<band>.{type,freq,q,gain,bypass,qp}
xo.<ch>.<band>.{type,freq}
ch.<ch>.{delay,name}
out.<n>.{enable,gain,mute,delay,pin,type}
mix.<in>.<out>.{enable,gain,invert}
loud.{on,ref,intensity,mask} · cf.{on,preset,freq,feed,itd,outputs}
lev.{on,amount,speed,maxgain,lookahead,gate,detmask,applymask}
bass.{on,cutoff,harmonics,drive,character,original,mask}
up.{on,center_mode,surround_mode,strength,width,threshold,attack,release,
    det_hpf,sur_delay,sur_hpf,sur_lpf,decorr,presence}
sig.{type,level,channels,invert,duration,repeat,gap,p1..p4,raw,decorr,walk}
in.{source,rate,spdif_pin,spdif_enable,i2s_pin,i2s_channels,i2s_clock_mode,
    adat_pin,adat_enable}
i2s.{bck,bck_slave,mck,mck_pin,mck_mult,clock_pin_mode}
adat.{out_enable,out_pin}
cs.<slot>.{type,noun,action,event,target,index,pins,step,value,name}
cs.ir.<n>.{code,noun,action,value}
preset.<n>.{save,load,delete,name} · preset.{startup,active,mode}
dev.{serial,platform,uart,i2c,dac_mute,bootloader,factory_reset}
```

---

## 6. Interaction model

Three concentric layers, each a complete way to use the app.

### 6.1 Layer 1: navigate

Full-screen, tabbed, arrow-key navigable. Tab or `1`-`0` switches panel, arrows
move focus, `Enter` or `Space` activates, `+`/`-` and left/right nudge, `Shift`
coarse, `Alt` fine. Mouse works where crossterm supports it.

The global keys, which work from every panel:

| Key | Does |
|---|---|
| `Ctrl-P` | Palette |
| `:` | Command line |
| `Ctrl-D` | Device picker |
| `G` | Graph overlay; `m` cycles overlay and grid modes |
| `M` | Meters; cycles compact rail and full bridge |
| `=` | Cycle the graph/table split on Filters |
| `F2` | Disclosure level |
| `Ctrl-S` | Save to the active preset slot |
| `?` | Help for whatever is focused |

A permanent **context bar** under the focused field states in plain language what
it does, its range, and whether the change is live-only or written to flash.

**Disclosure levels** keep the first run calm. `F2` cycles them; they only hide
controls, never change behaviour, and the palette can always reach a hidden
control after a warning.

- **Simple**: volume, preset, bypass, a 3-band tone summary, loudness, crossfeed
  on/off, meters. About 20 controls.
- **Advanced**: full PEQ, crossovers, matrix, delays, leveller, upmixer,
  psychoacoustic bass, presets, signal generator.
- **Expert**: pins, output types, MCK, I2S clock mode, ADAT, DAC hardware mute,
  UART and I2C interfaces, buffer statistics, the Raw panel, bootloader.

### 6.2 Layer 2: palette (`Ctrl-P`)

Fuzzy search over the whole registry plus verbs. Typing `drive` finds
`bass.drive`; `knob` finds "Bind a control surface"; `sweep` finds the log sweep.
Selecting a parameter focuses it in its panel. The palette is the bridge between
the two audiences: search for a layman, launcher for an expert.

### 6.3 Layer 3: command line (`:`)

A typed grammar with completion at every position, driven by a registry walk, so
it always offers exactly the legal next tokens with meanings and ranges.

```
:eq in.1 3 peak 2856 3.58 -8.6
:xo out.1 20 lr4 80
:mix 0 4 on -3
:bass on ; bass.cutoff 90 ; bass.drive 12
:sig sweep 20 20000 --level -20 --channels 0,1 --duration 5s
:cs 2 encoder vol.user step 1
:preset save 3 "Living Room"
```

Beyond parameter setting: `:graph`, `:diff`, `:export`, `:import`, `:load`,
`:watch`, `:undo`, `:redo`, `:device`, `:doctor`, `:raw`, `:log`.

### 6.4 One-shot mode: the same grammar from a shell

`dspi` with arguments does not launch the TUI. It connects, performs the command,
prints the result, and exits. **It is the same grammar as the `:` command line**,
parsed by the same code in `dspi-cmd`, so there is one syntax to learn and one
place where it can drift.

```
$ dspi eq in.1 3 peak 2856 3.58 -8.6
in.1 band 3: Peaking 2856 Hz Q 3.58 -8.6 dB

$ dspi vol.user -18
$ dspi bass on
$ dspi get eq.in.1.3.freq
2856

$ dspi preset save 3 --name "Living Room"
$ dspi export tuning.json
$ dspi import tuning.json --dry-run
```

This matters for a Raspberry Pi install more than for a laptop: it makes the
device scriptable from cron, systemd units, home-automation hooks, and shell
one-liners without a terminal session at all.

Design rules:

- **Nothing is CLI-only or TUI-only.** Both front-ends drive `dspi-session`
  through the registry, so any parameter reachable in one is reachable in the
  other, by construction rather than by discipline.
- **Reads and writes are symmetric.** `dspi get <path>` prints a bare value for
  easy capture; `dspi get <path> --json` prints the value with its unit, range,
  and confirmation state.
- **Scripting output.** `--json` on any command emits a machine-readable result.
  `--quiet` suppresses everything but errors. Default output is a short
  human-readable confirmation.
- **Exit codes** are meaningful: `0` success, `1` usage or parse error, `2`
  device not found, `3` write rejected by the device on readback, `4` transport
  or busy-window failure after retries.
- **Batching.** `dspi -f script.dspi` and piped stdin run a sequence, aborting on
  first failure unless `--keep-going`. `:log --commands` from the TUI produces
  exactly this format, so a session can be replayed or committed to version
  control.
- **`--dry-run`, `--device <serial>`, and `--timeout` apply here too**, so a
  script can target a specific unit on a machine with several attached.
- **The busy window still applies.** A one-shot preset save waits out the flash
  blackout and verifies by readback before exiting `0`; it does not exit as soon
  as the device says "accepted". This is the difference between a script that
  works and one that races.

**Shell completions come free.** The registry generates static completion scripts
for bash, zsh, fish, and PowerShell via `dspi completions <shell>`, covering every
parameter path and enum value. The same table drives TUI completion, so the two
can never disagree.

The TUI's echo line (6.5) prints commands in this exact syntax, which means a
user who does something in the TUI can paste the echoed line straight into a
shell script. That is the payoff of keeping one grammar.

### 6.5 The echo line

Every change made by any means prints its canonical command, dimmed:

```
:eq in.1 3 freq 2856
```

Turn a knob in the UI, see the command. Highest-leverage feature for turning a
layman into an expert, and nearly free because the registry already knows how to
render any change as a command. `Ctrl-Y` copies the last one; `:log --commands`
exports the session as a replayable script.

### 6.6 Undo, and the two kinds of state

DSPi has a split that confuses people: most writes change **RAM only** and are
lost on reboot, while a specific set write flash. `SET_ALL_PARAMS` not persisting
anything is called out in the firmware docs as the most common misconception.

Made visible at all times: the title bar shows the active preset with a **dirty
marker**; control surfaces carry their own dirty flag from `CsStatusPacket`;
`Ctrl-S` is always "save to the active preset slot" with a confirm naming the
slot; `:diff` renders live versus stored field by field; quitting with unsaved
changes prompts once with a summary of what would be lost.

Undo journals live parameter writes with prior values. Flash operations, pin
moves, and the bootloader jump are non-undoable and are confirmed instead.

### 6.7 Guided flows

Four wizards, from the palette:

1. **Set up a crossover.** Pick outputs, frequency, and slope family with a
   plain-language note on the trade-offs; writes both channels' crossover bands,
   shows the summed response, offers to save.
2. **Wire a physical control.** Pick a component, pick what it should do by
   browsing the same parameter tree, pick GPIOs from a live pin map that greys
   out claimed pins with their owners; validates against caps before writing. IR
   runs the learn loop (section 8.8).
3. **Measure a channel.** Configure a log sweep on chosen outputs, arm it, track
   the live frequency on the graph's x-axis.
4. **First-run setup.** Name channels, choose input source, set output types, set
   master volume, save to a preset.

---

## 7. Panels and overlays

### 7.0 Navigation model

Ten panels, in signal-flow order, which is itself a teaching device:

```
Dashboard · Input · Filters · Matrix · Dynamics · Spatial · Surfaces ·
Presets · System · Raw
```

Plus **two global overlays** that any panel can summon, because they are things
you want *while working*, not destinations you travel to:

| Key | Overlay | Modes |
|---|---|---|
| `G` | **Graph** | `overlay` (channels on one axis) / `grid` (small multiples, all channels) |
| `M` | **Meters** | `compact` rail (always on) / `bridge` (full screen, every channel) |

Two consolidations keep the tab bar on one line at 80 columns:

- **Crossover folds into Filters** as a sub-tab. The Console already models it
  this way, with `FilterListTab(title: "PEQ")` and `FilterListTab(title: "XO")`
  in `ContentView.swift`. They are both "filters on a channel" and they share one
  graph, so splitting them across tabs would be arbitrary.
- **Output folds into Matrix**, which already carries the per-output strips
  below the crosspoint grid. Routing and per-output trim are one job.

Every panel is a declared list of `ParamId`s (mechanism 5 of section 3.2), so
adding a firmware parameter adds a row, not a layout change.

### 7.1 The Meters overlay

Meters for **every** channel, at parity with the Console. All of it comes from a
single `GET_STATUS` `wValue=9` poll returning per-channel peaks, both CPU loads,
and the sticky clip bitmask, so meters everywhere cost one transfer.

**Compact rail** (`M` cycles, on by default): a narrow always-visible rail on the
right edge showing the channels relevant to the current panel, plus CPU. It
follows context: on Filters it shows the selected channel and its pair, on Matrix
it shows the focused crosspoint's input and output, on Dashboard it shows all
enabled outputs.

```
  In 1 ▓▓▓▓▓▓░░ -6 ▌
  In 2 ▓▓▓▓▓░░░ -8 ▌      ▌ = clip latched
  SP1L ▓▓▓▓░░░░-11 ▐
  Sub  ▓▓▓▓▓▓▓░ -3 ▐
  CPU  ▓▓░ 34/8%
```

**Full bridge** (`M` again): every channel, full width, with peak hold, numeric
dB, and clip latch.

```
┌ Meters ── 17 channels ─────────────────── c clear clips · p peak hold ──────┐
│  In 1   ▓▓▓▓▓▓▓▓░░░░░░░░   -6.2   peak  -2.1   ● CLIP                       │
│  In 2   ▓▓▓▓▓▓▓░░░░░░░░░   -8.4   peak  -5.0                                │
│  In 3   ░░░░░░░░░░░░░░░░    -inf  peak  -inf                                │
│  …                                                                          │
│  SP1 L  ▓▓▓▓▓▓░░░░░░░░░░  -11.3   peak  -8.7                                │
│  SP1 R  ▓▓▓▓▓░░░░░░░░░░░  -13.1   peak  -9.2                                │
│  Sub    ▓▓▓▓▓▓▓▓▓░░░░░░░   -3.0   peak  -0.4   ● CLIP                       │
│                                                                             │
│  CPU 0  ▓▓▓░░░░░  34%        CPU 0  EQ worker                               │
│  CPU 1  ▓░░░░░░░   8%        CPU 1  PDM                                     │
└─────────────────────────────────────────────────────────────────────────────┘
```

Design details, all matching the Console's `HorizontalMeterBar`:

- A dedicated **clip zone** at the right end of each bar, driven by the per-channel
  bit in `clip_flags`, not by the bar reaching full scale.
- Clip is **sticky** until cleared. `c` issues `CLEAR_CLIPS` (0x83), which is
  read-and-clear in one transfer and returns the bitmask that was set.
- Channels group into **stereo pairs** where the device's naming implies one, and
  PDM stays mono.
- **Disabled outputs are dimmed, not hidden**, so the bridge is a stable shape;
  the Dashboard cards hide them, as the Console does.
- Peak hold with a configurable decay, off by default.
- Meter scale is logarithmic with the same -60 dB floor the firmware's peak
  encoding implies.

Polling: 20 Hz desktop, 10 Hz on a Pi, 2 Hz when the overlay is hidden and no
panel shows a meter. The rail being visible is what keeps the poll running, so
hiding it is a real power saving on a Pi.

### 7.2 The Graph overlay

`G` expands the graph to full screen and cycles its two modes. The same widget
renders inline on Dashboard and Filters, so there is one implementation.

**Overlay mode**: channels on one axis, legend pills toggling visibility, exactly
the Console's behaviour. Right for comparing a few channels closely: left against
right, or the two halves of a crossover.

**Grid mode (small multiples)**: one mini Bode plot per channel, each with its
name, a mini level meter, and its curve. This is what answers "which channel
looks wrong?" across 17 channels, which a 17-curve overlay cannot.

```
┌ Responses ── all 17 channels ──────────────────────── grid │ overlay ───────┐
│  In 1  ▪▃          In 2  ▪▃          In 3  ▫▁          In 4  ▫▁            │
│  ⠤⠔⠉⠑⠢⢄⡠⠔⠒⠤        ⠤⠔⠉⠑⠢⢄⡠⠔⠒⠤        ⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤        ⠤⠤⠤⠤⠤⠤⠤⠤⠤⠤          │
│                                                                             │
│  SP1 L ▪▅          SP1 R ▪▅          SP2 L ▪▄          SP2 R ▪▄            │
│  ⠉⠉⠒⠤⣀              ⠉⠉⠒⠤⣀              ⠉⠉⠒⠤⣀              ⠉⠉⠒⠤⣀              │
│                                                                             │
│  Sub   ▪▇                                                                   │
│  ⣀⡠⠔⠒⠉⠑⠢⢄                            ▸ Enter opens In 1 in Filters          │
└ m mode · f filter: all │ inputs │ outputs │ enabled · ? help ───────────────┘
```

Grid details:

- Cell size adapts to terminal width: 5 columns at 120, 3 at 80, 7 at 200. Rows
  scroll if the channel count exceeds the viewport.
- Each cell uses the **same dB and frequency window** as the main graph, so cells
  are comparable at a glance. A per-cell autoscale option exists but is off by
  default, because autoscaled small multiples lie about relative magnitude.
- **Flat channels are drawn flat and dimmed**, not omitted, so a channel with no
  EQ is visibly distinct from a channel that is missing.
- Filters: all / inputs only / outputs only / enabled outputs only.
- `Enter` on a cell opens that channel in the Filters panel. The grid is a
  navigation surface, not just a report.
- Curves are cached per channel and only recomputed on a band write, so grid mode
  costs nothing extra to display.

### 7.3 Dashboard

```
┌ DSPi ─ RP2350 · fw 1.1.5 · wire 26 · "Living Room" ── Preset 3 ● ───────────┐
│  ┌─ Response ─────────────────────────────────┐ ┌─ Levels ──────────────┐  │
│  │ +12                                        │ │ In 1  ▓▓▓▓▓▓░░ -6 ▌   │  │
│  │   0 ⠤⠤⠤⠤⠋⠁     ⠈⠑⠢⢄⣀⣀⡠⠔⠒⠉⠑⠒⠤⢄⣀          │ │ In 2  ▓▓▓▓▓░░░ -8 ▌   │  │
│  │ -12                          ⠉⠒⠤⣀         │ │ SP1 L ▓▓▓▓░░░░-11     │  │
│  │     20    100   1k    10k                  │ │ SP1 R ▓▓▓░░░░░-13     │  │
│  └─ ● In 1  ● In 2  ○ SP1 L  ○ Sub ──── G ────┘ │ Sub   ▓▓▓▓▓▓▓░ -3 ▐   │  │
│  ┌─ Now ──────────────────────────────────────┐ │ CPU   ▓▓░ 34/8%       │  │
│  │ Input   USB  48 kHz  streaming  ·  8 ch    │ └───────────────────────┘  │
│  │ Volume  -18.0 dB        Master  -20.0 dB   │ ┌─ Quick ───────────────┐  │
│  │ Outputs S/PDIF 1·2  I2S 3  PDM sub         │ │ [ ] EQ bypass         │  │
│  │ Active  Loudness · Leveller · Psycho bass  │ │ [x] Loudness          │  │
│  └────────────────────────────────────────────┘ │ [x] Psycho bass       │  │
│  :eq in.1 3 freq 2856                           └───────────────────────┘  │
└ ^P palette · : cmd · ^D device · G graph · M meters · F2 Advanced · ? ──────┘
```

Channel labels come from `GET_CHANNEL_NAME`, so a user who renamed a channel sees
their own name everywhere. The `●` marks unsaved changes.

### 7.4 Input

Source (USB / S/PDIF / I2S / ADAT), rate authority, per-input preamp, and live
receiver status. `SET_INPUT_SOURCE` shows the pipeline-reset warning before
committing, and `GET_INPUT_SOURCE` is polled after, since it reports *active*,
not *pending*.

`SpdifRxStatusPacket` rendered as a live lock indicator (state, detected rate,
lock and loss counts, parity errors, FIFO fill). IEC 60958 channel status decoded
into fields, not dumped as hex. Multi-input S/PDIF, I2S channel count, I2S clock
master/slave with `I2sSlaveStatusPacket`, ADAT input, and LG Sound Sync live here
too. This is where a user diagnoses a flaky optical cable, so it earns its
density.

### 7.5 Filters

The panel where most time is spent. Sub-tabs **PEQ** (bands 0-9) and **XO**
(bands 20-23), sharing one graph and one channel selector.

Layout is **stacked with an adjustable split**: the graph full-width on top, the
band table below with the focused band's detail beside it. A Bode plot spans
three frequency decades, so it wants width far more than height; full-width is
the single biggest legibility win available.

```
┌ Filters ── ◂ In 1 ▸ ──── [PEQ] XO ──────────────── EQ bypass [ ] ───────────┐
│ ┌─ Response ──────────────────────────────────────────────────────────────┐ │
│ │ +12                                                              +180°  │ │
│ │  +6    ⣀⡠⠔⠒⠑⠢⢄        ⡠⠒⠢⣀                                            │ │
│ │   0 ⠤⠔⠉        ⠑⠢⠤⠤⠤⠔⠉    ⠈⠒⠤⣀⡠⠔⠒⠢⢄⣀                            0°  │ │
│ │  -6              ▲             ⠉        ⠉⠒⠢⢄⣀                          │ │
│ │ -12              │band 3                       ⠈⠉                -180°  │ │
│ │     20    50   100  200   500   1k    2k    5k   10k    20k             │ │
│ └─────────────────────────────────────────────────────────────────────────┘ │
│ ┌─ Bands ─────────────────────────────────────────┐ ┌─ Band 3 ───────────┐ │
│ │  #  on  type       freq      gain     Q         │ │ Type   Peaking   ▾ │ │
│ │  1  ●   Low shelf   105 Hz   +8.8 dB    -       │ │ Freq   2856 Hz     │ │
│ │  2  ●   Peaking      64 Hz   -6.2 dB  0.30      │ │ Gain    -8.6 dB    │ │
│ │▸ 3  ●   Peaking    2856 Hz   -8.6 dB  3.58      │ │ Q        3.58      │ │
│ │  4  ●   Peaking    1880 Hz   +3.6 dB  1.69      │ │ Bypass    off      │ │
│ │  5  ●   High shelf 10000 Hz  -0.8 dB    -       │ │                    │ │
│ │  6  ○   Peaking     118 Hz   +0.4 dB  1.98      │ │ A narrow cut at    │ │
│ │  …                                              │ │ 2.9 kHz. Tames     │ │
│ └─────────────────────────────────────────────────┘ └────────────────────┘ │
│ :eq in.1 3 freq 2856                                                        │
└ c copy · v paste · b bypass · l link pair · A AutoEQ · = split · ? help ────┘
```

**The split is one mechanism, not three breakpoint layouts.** `=` cycles
graph-only / split / table-only, and the boundary is draggable with the mouse,
mirroring the Console's `GraphResizeHandle`. At 24 rows the app opens in
table-only with a one-line curve strip; at 40 rows it opens split; the user's
choice is remembered per device. This is why the stacked layout does not need a
separate small-terminal design.

Editing behaviour:

- The focused band is marked on the graph with a caret at its centre frequency,
  so the link between table row and curve is never ambiguous.
- Left and right adjust the focused field live, with the curve animating; the
  device write is debounced so a held key does not flood EP0.
- Every channel has PEQ, inputs included; that is the V16 model.
- All 12 filter types, including notch, both all-pass orders, first-order
  shelves, and Linkwitz Transform.
- **Linkwitz Transform** needs the 18-byte payload carrying `qp` as `Q*512` and
  reinterprets the fields as `f0` / `Q0` / `fp`. The UI relabels all four fields
  when LT is selected, or it is baffling. A 16-byte write preserves stored `qp`.
- **Crossover sub-tab** picks family and order as a unit (LR2/4/6/8, BW1-8,
  Bessel 2/4/6/8) rather than the raw 32-63 type encoding, which no user should
  see. It overlays the low and high sections plus their sum. Crossover bands are
  output-channels-only and the sub-tab is hidden on inputs.
- Per-band bypass, distinct from master EQ bypass.
- Channel copy and paste including the crossover bank, matching the Console.
- **Input pair linking** is an app-side concept the firmware has no notion of,
  but `.dspipreset` carries it, so we implement and preserve it for interchange
  fidelity.
- AutoEQ browser (section 12.3).

### 7.6 Matrix

The crosspoint grid, sized from `ChannelMap`. **On RP2350 this is 8 inputs by 9
outputs**, which needs scrolling and a focus reticle rather than a fixed layout.
Per-output strips sit below: enable, gain, mute, delay, pin, and output type.

```
┌ Matrix ── 8 in × 9 out ─────────────────────────────────────────────────────┐
│           SP1L  SP1R  SP2L  SP2R  SP3L  SP3R  SP4L  SP4R   Sub              │
│  In 1    ▣0.0    ·   ▣0.0    ·   ▣-3.0   ·     ·     ·   ▣-6.0              │
│  In 2      ·   ▣0.0    ·   ▣0.0    ·   ▣-3.0   ·     ·   ▣-6.0              │
│  In 3      ·     ·     ·     ·     ·     ·   ▣0.0    ·     ·                │
│  In 4      ·     ·     ·     ·     ·     ·     ·   ▣0.0    ·                │
│  In 5-8    ·     ·     ·     ·     ·     ·     ·     ·     ·                │
│  ▣ on   ▨ inverted   · off      ▸ In 1 → Sub   gain -6.0 dB   phase normal  │
├─ Outputs ───────────────────────────────────────────────────────────────────┤
│  #  name    on   gain    mute  delay    pin   type                          │
│  0  SP1 L   ●    +0.0     -    0.0 ms   6     S/PDIF                        │
│  1  SP1 R   ●    +0.0     -    0.0 ms   6     S/PDIF                        │
│  8  Sub     ●    -6.0     -    4.2 ms   10    PDM                           │
└─────────────────────────────────────────────────────────────────────────────┘
```

The Core 1 interlock is enforced before the fact: query `GET_CORE1_CONFLICT` and
grey out the option with the reason, rather than letting the write fail silently.

### 7.7 Dynamics

Volume leveller (including the V18 detector and apply masks) and loudness
compensation (including the V19 output mask). Both get live visualisation: the
leveller shows applied gain as a scrolling sparkline, loudness draws its
compensation curve as a ghost overlay on the graph keyed to user volume, so the
interaction with the volume control is visible rather than theoretical.

### 7.8 Spatial

Crossfeed, upmixer, psychoacoustic bass.

- **Crossfeed**: presets, frequency, feed, ITD, and the V20 output pair mask.
  Selecting a named preset shows the values it implies rather than hiding them.
- **Upmixer** (RP2350): all 14 `UPMIX_PARAM_*` individually, or the 44-byte
  config packet for atomic multi-parameter changes, plus live `UpmixStatus`.
  Detector and surround filters behind Expert level.
- **Psychoacoustic bass**: enable, cutoff, harmonics, drive, character, original
  level, output mask, with ranges from the generated `PSYBASS_*_MIN/MAX`
  constants. A small diagram of the even/odd blend beats prose.

### 7.9 Surfaces

The Console covers this fully in **Settings > Control Surfaces**
(`DSPi_ConsoleApp.swift:1843`), including all slots, caps-driven pickers,
targets, events, the apply/save/revert preview model, and IR learn. **Treat that
tab as the behavioural reference**; matching its semantics means a user moving
between apps is never surprised. What the terminal adds is density: the Console
edits one slot at a time in a form, whereas every slot fits in one table here,
which is the right shape for auditing a rig.

```
┌ Control Surfaces ──────────────────── 16 slots · caps v4 · unsaved ● ───────┐
│  #  component      does                          pins      status           │
│  0  Encoder        Volume            step 1 dB   27, 28    ● live           │
│  1  Button         Preset            next, wrap  22        ● live           │
│  2  Switch         Loudness          follow      21        ● live           │
│  3  Pot            Psycho bass drive 0-18 dB     26 (ADC)  ● live           │
│  4  LED (PWM)      In 1 level        brightness  20        ● live           │
│  5  IR receiver    8 commands        -           19        ● live           │
│  6  Button         Bass cutoff       +5 Hz, hold 18        ⚠ pin in use     │
│  ┌─ IR commands (slot 5) ────────────────────────────────────────────────┐  │
│  │  0  0x20DF40BF   Volume  +1 dB     learned                            │  │
│  │  1  0x20DF10EF   Mute    toggle    learned                            │  │
│  │  2  -            -       -         [ learn ]                          │  │
│  └───────────────────────────────────────────────────────────────────────┘  │
└ n new · l learn IR · s save to flash · r revert · ? help ───────────────────┘
```

- Bindings and IR commands **apply live but do not persist** until `CS_SAVE`;
  `CS_REVERT` restores the stored config. The `dirty` flag drives the marker.
  This preview model is a real feature: a user can try an assignment and back out.
- The "does" column is generated from the registry, so a knob bound to
  `bass.drive` reads as "Psycho bass drive, 0-18 dB", not as noun 44.
- The pin picker shows a live GPIO map with claimed pins greyed and labelled with
  their owner. GPIO 12 and 23-25 permanently excluded. ADC-capable pins marked.
- All `CS_STATUS_*` codes get a human sentence.
- The app never offers an impossible combination, because the action list is
  `CsTypeDesc.actions & CsNounDesc.actions`.
- **IR learn is three-valued** on `0x8F`: `wValue=1` arms, `0` cancels, `2` reads
  an 8-byte result carrying state, protocol, and code. Poll `wValue=2` until the
  state leaves `ARMED`, with a visible countdown and a cancel key.

### 7.10 Presets

Ten slots: occupancy, names, active slot, startup policy, and the two persistence
modes (output config and master volume, device-global versus per-preset). Those
modes are the subtlest part of the protocol and get an explanatory paragraph in
the panel, not a tooltip. Import and export in section 12.

### 7.11 System

Physical I/O (output pins and types, I2S BCK and LRCLK, MCK with the CLK_GPOUTn
constraint enforced in the picker, I2S clock pin mode), DAC hardware mute with
its audible test behind a confirm and mandatory readback, UART and I2C control
interfaces, diagnostics (buffer statistics with watermarks, overrun and underrun
counters, DMA starvations per instance, clock, core voltage, temperature), and
firmware version with `ENTER_BOOTLOADER` behind a two-step typed confirm.

### 7.12 Raw

Expert only. Issue any opcode by number with a hex payload and chosen direction;
view the bulk snapshot as labelled hex with unknown regions highlighted; watch a
live wire transcript. This is mechanism (7) from section 3.2: **a firmware
feature that ships before app support is still reachable today.**

## 8. The graph

### 8.1 Math

Parity with the Console, so this is a port, not a reimplementation.

- **201 log-spaced points, 10 Hz to 20 kHz.** Computed over the full data range
  and mapped to the visible window, so zoom needs no recompute.
- **Magnitude**: cascade of biquads, `10*log10(|H|²)`. Skip bypassed bands and
  bypassed channels.
- **Phase**: per-band `atan2` accumulation with an unwrap option, drawn dotted on
  a right axis whose range scales with the dB zoom (±180° at the default 50 dB).
- **Crossover cascades**: port `crossoverSections`, `prototypePoles`,
  `butterworthPoles`, `besselPoles`, and `designSection` from `DSPMath.swift`.
  The existing Rust `dsp_math.rs` has the simple types but not these or Linkwitz
  Transform.
- **Output gain** applies as a constant dB offset on output channels only.
- **Identical-curve grouping**: channels whose 201-point vectors match are drawn
  once in a blended colour. With up to 17 channels this matters more than it did
  at 11.
- **Unknown filter types** are excluded from the curve with a visible note, never
  silently treated as flat.
- `f64` throughout, cached per channel, invalidated on any band write.

### 8.2 Rendering

Resolution comes from Unicode braille: each cell is a 2 x 4 dot matrix, so a
60 x 12 area becomes a 120 x 48 plot grid.

| Tier | Glyphs | When |
|---|---|---|
| `braille` | `⠀-⣿` | Default |
| `blocks` | `▁▂▃▄▅▆▇█` plus box drawing | Fallback |
| `ascii` | `.-*#\|+` | `TERM=dumb`, `--no-unicode`, CI |

Colour tiers: truecolor, 256, 16, monochrome. In monochrome, channels differ by
line pattern rather than hue, which also serves colour-blind users. Braille
cannot carry per-dot colour, so overlaps resolve by z-order with the legend
marking occluded channels.

With 17 possible channels the legend needs to wrap and to support "show only
inputs" / "show only outputs" / "show enabled outputs" quick filters.

**Small multiples** (grid mode, 7.2) reuse the same renderer at a smaller cell
size. Below roughly 12 columns by 3 rows a braille curve stops being readable, so
cells at or under that threshold fall back to a **sparkline**: one row of block
glyphs conveying shape and rough magnitude only. The grid tells you *which*
channel to look at; the overlay tells you *what* it is doing. Keeping that
division of labour is what stops grid mode from becoming an unreadable wall.

### 8.3 Interaction

Legend pills toggle visibility. A vertical **cursor** moved with `h`/`l` or the
mouse reads out frequency and each visible channel's dB, which is something a
terminal does better than a GUI because the readout is text. Zoom with `+`/`-`
on dB range and `[`/`]` on frequency, `0` to reset. Toggleable overlays: phase,
loudness compensation at the current volume, crossover sum, and a saved reference
curve for A/B. A caret on the x-axis marks the focused band's centre frequency.

`G` expands to full screen, `m` cycles overlay and grid modes, `f` cycles the
channel filter, and `Enter` on a grid cell opens that channel in Filters. Zoom
and dB window are **shared** between the inline graph, the overlay, and every
grid cell, so a comparison never silently changes scale underneath you.

### 8.4 Export

`:graph export <file>` writes SVG or PNG, so a Pi-over-SSH user can produce
something to share.

---

## 9. Visual design

### 9.1 Principles

1. **Data is bright, chrome is dim.** Values, curves, and meters carry the
   colour; borders and labels sit low-contrast.
2. **Colour means something.** Each channel owns a hue used consistently across
   graph, legend, meters, and matrix. Never decorative.
3. **One accent**, for focus and nothing else.
4. **Semantic colour is reserved**: amber pending, red clip and hazard, green
   confirmed.
5. **Motion is functional.** Meters and the leveller sparkline move because they
   are live; values ease over ~120 ms when changed remotely so a physical knob
   turn is visible. Nothing else animates.
6. **Density adapts.** 80x24 single column with a collapsible graph; 120x40 the
   two-column layout above; 200x60 a dedicated graph pane beside a full band
   table. Hard minimum 80x24 with a clear message below that.

### 9.2 Themes

Four shipped: dark default, a real light theme, high contrast, and monochrome.
TOML in the config directory so users can write their own. `NO_COLOR` respected.

The channel palette needs **17** distinguishable hues surviving 256-colour
quantisation and common colour-vision deficiencies. That is a real constraint at
that count; do the validation once and record it in `docs/palette.md`.

### 9.3 The one moment of delight

On connect, the response curve draws itself left to right over about 400 ms as
state loads, with chrome fading in behind. It makes the device feel like it is
waking up, doubles as an honest progress indicator for a 5944-byte bulk read, and
costs nothing. Skipped under `--no-animation` and `TERM=dumb`.

Not doing: ASCII art banners, spinners that outlast their operation, gradients
for their own sake.

---

## 10. Cross-platform

| Platform | Access | Notes |
|---|---|---|
| **Windows 10/11** | WinUSB via `nusb`, bound automatically | MS OS 2.0 descriptors mean no Zadig. **4 KB transfer cap makes `GET_ALL_PARAMS_CHUNK` mandatory** at 5944 bytes. Windows Terminal supported; conhost gets `blocks`. |
| **Linux x86_64** | usbfs via `nusb` | Ship `70-dspi.rules`; `--install-udev` installs it; `:doctor` detects its absence and prints the fix. |
| **macOS 12+** | IOKit via `nusb` | Interfaces 0 and 1 belong to the audio class driver; interface 2 is free. Universal binary. |
| **Raspberry Pi** | usbfs | Section 10.1. |

### 10.1 Raspberry Pi

- **Targets**: `aarch64-unknown-linux-gnu` and
  `armv7-unknown-linux-gnueabihf`, both cross-compiled in CI.
- **Adaptive frame rate**: up to 30 fps while animating, 4 fps idle, 1 fps
  unfocused. Never repaint an unchanged frame. Over SSH the bottleneck is bytes
  on the wire, so damage-tracked partial repaints matter more than render speed.
- **`--lite`**: 10 Hz meters, no animation, `blocks` rendering, graph recompute
  throttled to 5 Hz. Auto-enabled on single-core or ARMv6 parts and on slow SSH
  links.
- **USB power**: a Pi's ports can brown out on re-enumeration after
  `ENTER_BOOTLOADER`. Handle disappearance and return gracefully.

### 10.2 `:doctor`

```
$ dspi-term --doctor
DSPi Terminal 2.0.0 · linux aarch64

  ✓ USB subsystem reachable
  ✓ Device found: 2E8B:FEAA at bus 001 addr 007  (SN 4E3A91C7)
  ✗ Permission denied opening interface 2
      → udev rule not installed.
        Fix:  sudo dspi-term --install-udev && sudo udevadm control --reload
              then unplug and replug the DSPi.
  ✓ Terminal: xterm-256color, 120x40, truecolor, braille OK
  ✓ Firmware 1.1.5, RP2350, wire 26, 17 channels, CS caps 4
```

---

## 11. Safety

| Hazard | Handling |
|---|---|
| **Busy window** | Automatic: 150 ms backoff, 4 retries, liveness poll, reacquire by serial. Subtle status indicator, no modal. |
| **Flash blackout** (~45 ms/sector, two sectors for a preset save) | Progress indicator; writes queued, never concurrent with a pending flash op. |
| **Silent validation failure** | Universal readback-and-compare. The only defence. |
| **Audible operations** | Confirm with "this will interrupt audio for about N ms". |
| **Pin conflicts** | Prevented in the picker via a live claimed-pin map. |
| **Core 1 interlock** | Queried before offering an enable. |
| **`ENTER_BOOTLOADER`** | Two-step typed confirm, Expert only, never in default palette results. |
| **Factory reset** | Confirm with an accurate explanation: it resets live state and **keeps stored presets**, the opposite of what the name suggests. |
| **Unknown enum values** | Preserved, never clamped. |
| **Inter-slot phase alignment** | The app never issues partial reconfiguration. All pin, type, and source changes go through the documented opcodes exactly as specified. Hard product invariant, marked as such in code. |
| **`--dry-run`** | Connect, read, render, refuse all writes, log what would have been sent. |

---

## 12. Files and interchange

### 12.1 `.dspipreset` (whole-device configuration)

Defined by the Windows Console
(`DSPiConsole/Models/PresetDocument.cs`, `Services/PresetFileService.cs`).
We must **read and write it faithfully**, since it is destined for the macOS
Console too. It is JSON, camelCase, schema-versioned.

Structure, in brief: `schemaVersion`, `meta` (name, savedUtc, appVersion,
platform, firmwareVersion, wireFormatVersion, input and output counts), `global`
(8 input preamps, bypass, master and user volume, input source, LG sync, input
pair linking), feature blocks `loudness` / `crossfeed` / `leveller`, **nullable**
`psybass` and `upmix` (null when the source device lacked them), `channels` (per
channel: id, name, isOutput, delay, gain, mute, enable, `eq[]`, `crossover[]`),
`matrix` (crosspoints), and `io` (pins, slot types, I2S and MCK, S/PDIF and I2S
RX pins, ADAT, DAC hardware mute).

Design decisions in that format that we adopt wholesale, because they are correct
and because divergence would break interchange:

- **Raw wire values, not enum names.** Wire values are the firmware's own and
  stay stable; a renamed enum member would silently break old files.
- **Nullable capability blocks**, so a document from a device without a feature is
  distinguishable from one with the feature disabled.
- **Schema version rejects documents from the future** rather than guessing.
- **Three-way apply options**: audio processing (always), volume levels (off by
  default, since a document from another system would otherwise change how loud
  the room gets), and hardware I/O (off by default, since GPIO assignments
  describe a board rather than a listening setup).
- **An apply report**: channels and bands applied, channels the device does not
  have, blocks skipped for lack of a feature. The user is told, not left to infer.

Our import shows the report as a **preview diff before applying**, which the TUI
can do more comfortably than a dialog. Import applies through the ordinary
registry write path, so every value gets the same clamping, platform gating, and
dirty tracking as a user edit.

**Open item.** The format's `meta.wireFormatVersion` and its 8-input `global`
block imply a V16+ device. Behaviour when importing onto an RP2040 (2 inputs, 7
channels) needs confirming against the Windows implementation's channel-mapping
logic so we match it exactly rather than inventing a rule. Flagged in section 15.

### 12.2 Filter files

The Console's human-readable `.txt` export (as in `DSPiFilters.txt`). Must
round-trip, including preamp, crossovers, and every PEQ type. Import maps
`[Master L]`-style section headers from older files onto the current channel
model via `ChannelMap`, reporting anything it cannot place.

### 12.3 AutoEQ

**Bundle the database**, following the Console's pattern
(`AutoEQManager.swift`): a user-supplied database takes precedence, otherwise the
bundled copy is used.

```
1. $CONFIG_DIR/dspi-term/autoeq_database.json      user-supplied or updated
2. embedded copy (include_bytes!)                   shipped in the binary
```

Provisioned now, implemented later: an `:autoeq update` command that fetches a
newer database into path 1, and a `--autoeq-db <path>` flag. The loader already
reads `generatedAt` and displays the database date, so a stale bundle is visible
rather than silent. Source priority (oratory1990, crinacle, Rtings,
Innerfidelity, headphone.com legacy) is carried in the database and honoured in
search ranking, as the Console does.

The embedded copy costs binary size; if that becomes a problem on the armv7
target, gate it behind a `bundled-autoeq` cargo feature that is on by default.

### 12.4 Native export

`:export` writes JSON keyed by registry `path`: human-diffable, forward
compatible, and trivially scriptable. This is our own format, complementary to
`.dspipreset` rather than a replacement, and it is the one to use for version
control of a tuning.

---

## 13. Testing

1. **Wire codec tests.** Every field round-trips, with fixtures at several
   `format_version` values, including a synthetic "future" packet with an unknown
   trailing section that must survive a read-modify-write untouched.
2. **Registry coverage test.** Every `REQ_*` in `config.h` is registered or
   excluded with a reason. Parses the vendored header, so a firmware bump fails
   the build until handled. **This is the mechanism behind "controls every
   setting".**
3. **Offset test.** Section offsets computed by `build.rs` are asserted against
   the firmware's own `_Static_assert` sizes, so a struct that grows is caught at
   build time rather than by misparsing at runtime.
4. **DSP math golden tests.** Magnitude and phase for every filter type at 201
   points against vectors generated from `DSPMath.swift`. The graph-parity
   guarantee.
5. **Mock device.** Replay of recorded sessions including STALLs, timeouts, and a
   simulated busy window, so retry logic is tested without hardware.
6. **Golden-frame snapshot tests** at 80x24, 120x40, 200x60.
7. **Interchange tests.** Read every `.dspipreset` and filter file we can obtain
   from both Consoles and assert a faithful round-trip.
8. **Hardware smoke suite.** Opt-in `--features hardware`, round-tripping every
   writable parameter against a real device and restoring prior state.
9. **Fuzz the decoders.** Malformed and truncated packets must never panic; a
   marginal cable produces short reads.

---

## 14. Distribution

Prebuilt binaries for `x86_64-pc-windows-msvc`, `x86_64-apple-darwin`,
`aarch64-apple-darwin` (or universal), `x86_64-unknown-linux-gnu`,
`x86_64-unknown-linux-musl`, `aarch64-unknown-linux-gnu`, and
`armv7-unknown-linux-gnueabihf`. `cargo install dspi-term`, a Homebrew tap, and a
`.deb` carrying the udev rule.

Version the app independently of firmware. Print app version, firmware version,
and wire format in `--version` and the title bar. Support the current firmware and
older ones by capability probe, degrading rather than refusing to connect.

---

## 15. Milestones

| # | Deliverable |
|---|---|
| **M1** | `dspi-proto` foundations: vendored headers, `build.rs` generator, `FieldDesc` bulk codec with version tolerance, open enums, `ChannelMap`, capability probe. `dspi-transport` with `nusb`, enumeration, hot-plug. Connects and dumps full state as JSON. No UI. |
| **M2** | The registry, the write path with readback verification and busy-window handling, the journal, and the coverage, offset, and codec tests. |
| **M2.5** | `dspi-cmd`: the grammar, parser, completer, one-shot runner, exit codes, and shell-completion generation. **Ships a genuinely useful CLI before any TUI exists**, and de-risks the grammar early since both front-ends depend on it. |
| **M3** | TUI shell: event loop, theme, panel routing, status bar, help, command line, palette, echo line, device picker, and the **overlay mechanism**. Dashboard, Filters, and the Meters overlay (rail plus bridge). |
| **M4** | Graph: DSP math port including crossovers and Linkwitz Transform, braille renderer, cursor, zoom, phase and loudness overlays, legend, and **grid mode with small multiples**. |
| **M5** | Remaining panels: Input, Matrix, Dynamics, Spatial, Presets, System, Raw. |
| **M6** | Control Surfaces and IR learn. Files: `.dspipreset`, filter files, AutoEQ, native export. |
| **M7** | Guided flows, disclosure levels, `:doctor`, Pi `--lite`, packaging, docs. |

M1 and M2 carry the difficulty and the entire extensibility bet. If they are
right, M3 through M7 are largely mechanical.

---

## 16. Settled decisions

1. **AutoEQ**: bundle the database as the Console does; user-supplied path takes
   precedence; online update provisioned but not implemented now. (12.3)
2. **Multi-device**: aware of all connected devices, one active at a time,
   switchable, mirroring the Console. (4.6)
3. **Interchange**: import and export Console filter files and `.dspipreset`
   files. (12.1, 12.2)
4. **Repository**: clean `main` in this repo. Keep `REDESIGN_SPEC.md` and
   `DSPiFilters.txt` as a test fixture; `dspi_cli.py` and `dspi_cli.log` survive
   in git history only.
5. **Documentation**: all notes stay here, in `docs/`. **The firmware repository
   is not modified.** Discrepancies between `commands.md` and the headers are
   recorded in `docs/firmware-notes.md` for our own use.
6. **Navigation**: ten panels, with Crossover folded into Filters as a sub-tab
   and Output folded into Matrix. Graph and Meters are global overlays, not
   destinations. (7.0)
7. **Filter editing**: stacked layout, graph full-width above the band table,
   with an adjustable split that `=` cycles. The split replaces per-breakpoint
   layouts. (7.5)
8. **All-channel responses**: a small-multiples grid, with a full overlay as the
   second mode of the same panel. Not the default view; reached with `G`. (7.2)
9. **Meters**: every channel, from one `GET_STATUS` poll. Compact context-aware
   rail on by default, full meter bridge on `M`. Sticky clip latch cleared with
   `CLEAR_CLIPS`. (7.1)
10. **One-shot CLI**: `dspi <command>` executes without launching the TUI, using
    the same grammar as the `:` line, with `--json`, meaningful exit codes,
    batching, and generated shell completions. Binary is named `dspi`. (6.4)

## 17. Open items

1. **`.dspipreset` cross-platform import.** How the Windows implementation maps an
   8-input document onto a 2-input RP2040 needs reading before we implement, so we
   match rather than invent. (12.1)
2. **Wire format V26 field map.** `commands.md` documents V14. The full V26
   section table must be derived from `bulk_params.h` during M1 and written up in
   `docs/wire-format.md` here. Budget real time for this; it is the single largest
   unknown in M1.
3. **`.dspipreset` schema drift.** Pinned at `DSPi-Console-Windows@81ae00b`
   (`master`, 2026-07-27). That repo is under active development, so re-check
   before M6 and record any schema change in `docs/interchange-notes.md`.
4. **Console branch tracking.** This spec and the macOS Console both sit on
   `release/v1.1.5`. If the Consoles and the firmware ever diverge onto different
   release branches, decide explicitly which `dspi-term` follows. The answer
   should be the firmware, with interchange formats tracked separately.

---

## 18. Where the implementation diverged

Recorded as they were found, because each was a spec assumption that did not
survive contact with the firmware.

**Bulk writes are version-locked, not tolerant (3.2 mechanism 2).**
`bulk_params_apply()` rejects any payload whose `format_version` or length is
not exactly current. The "send a shorter prefix and let older fields default"
strategy the spec described does not exist; it was inherited from `commands.md`,
which documents a V14 behaviour the firmware abandoned. `BulkPacket::decode`
refuses an unknown version outright, and individual `SET_*` opcodes are the
version-independent fallback.

**EQ bands are written whole, not field by field.** `SET_EQ_PARAM` takes a
16-byte descriptor and reads nothing from `wValue`. Changing one field is a
read-modify-write of the band. A plain write stays 16 bytes; padding to 18 would
reset the stored Linkwitz `Qp` on every ordinary edit.

**The combined status packet is `channels*2 + 7` bytes**, not the `+ 4` in
`commands.md`: the clip mask widened to 32 bits and a live input count was
appended. A 16-bit mask cannot represent channel 17's clip flag, which is the
PDM subwoofer.

**First-order shelves and crossovers follow the firmware, not the textbook.**
The device runs a one-pole TPT state-variable filter prewarping by `A` rather
than `sqrt(A)`; an independently derived shelf settled at half the requested
gain. Crossovers likewise: the prewarp, pole ordering and high-pass pole
reciprocation all had to come from `crossover.c`.

**`.dspipreset` channels match by id and are never remapped** (open item 1). The
reference implementation reports what a device lacks rather than translating an
eight-input document onto a two-input part. Pair-link state applies before the
channels, because a linked pair mirrors writes to its partner.

**Structured reads do not belong in flat field panels.** Rendering a status
packet as a scalar produces "1 bytes", which looks like data. They are excluded
until they have views that can decode them.

**The macOS vendor interface is exclusive.** DSPi Console and this tool cannot
hold a device simultaneously, and the failure looks like a permission error.
Every permission message is platform-specific for this reason.

**Not built:** guided flows (6.7), remote transport beyond the trait seam, SVG
and PNG graph export, applying hardware I/O blocks from a preset, and a hardware
smoke suite. The Raw panel exists as a command rather than a panel.
