# Moving to a newer firmware

The point of this procedure is that **no step involves changing existing code**.
The generator and the tests tell you exactly what is new; you add rows, not
edits. If you find yourself rewriting something, that is a signal the
architecture has sprung a leak, and it is worth fixing there instead.

## 1. Copy the headers

The vendored set is the `files` list in `crates/dspi-proto/firmware/PROVENANCE.toml`,
which is also the `HEADERS` list in `crates/dspi-proto/build.rs`. At `557bce7` it
is thirteen headers:

```sh
FW=/path/to/DSPi                     # on the release branch you are targeting
DST=crates/dspi-proto/firmware

for h in config.h bulk_params.h control_surfaces.h psybass.h upmix.h siggen.h \
         subharm.h tube.h limiter.h rta.h rta_fft.h rta_bass.h notify.h; do
  cp "$FW/firmware/DSPi/$h" "$DST/$h"
done

( cd "$FW" && git rev-parse HEAD && git rev-parse --abbrev-ref HEAD \
           && git log -1 --format=%ci HEAD )
```

A new firmware feature usually brings a new header. Add it to `HEADERS` in
`build.rs` and to the `files` list in `PROVENANCE.toml` together; a header that
is copied but not listed is never read.

Update `PROVENANCE.toml` with the new branch, commit, commit date and the date
you vendored, then refresh the checksums:

```sh
( cd crates/dspi-proto/firmware && shasum -a 256 *.h > SHA256SUMS )
```

Track the **release branch**, not `main`.

## 2. Rebuild

```sh
cargo build -p dspi-proto
```

The generator lifts three kinds of constant, each by name prefix:

| List in `build.rs` | Lifts | Lands in |
|---|---|---|
| `DEFINE_GROUPS` | integer `#define`s such as `REQ_*`, `CS_*`, `TUBE_*`, `RTA_*` | `generated::<group>`, e.g. `generated::tube` |
| `ENUM_PREFIXES` | `enum` members, explicit or implicit, such as `TUBE_PARAM_*`, `LIMITER_PARAM_*`, `PARAM_SRC_*`, `CS_TYPE_*`, `CS_NOUN_*` | the group whose prefix they match |
| `FLOAT_PREFIXES` | float `#define`s such as `SUBHARM_LEVEL_MAX` | `generated::ranges` |

A new header's constants appear only once their prefix is listed. Anything
parenthesised or computed is skipped rather than evaluated.

Watch the build log. Any constant that became platform-conditional is reported
as `platform-conditional, not emitted: NAME`. That is the generator refusing to
let a per-platform value become a compiled-in assumption; the value must come
from a capability probe instead. (`SIGGEN_MULTITONE_MAX` has been reported since
v1.1.6 and is read from the signal generator's caps.)

## 3. Let the tests tell you what moved

```sh
cargo test --workspace
```

Expect these to speak up first:

| Failing test | Means |
|---|---|
| `bulk_size_matches_firmware` | The packet changed size. Find the section that grew or was appended, fix `sections` in `build.rs`, and re-derive `docs/wire-format.md`. |
| `no_v28_section_moved` | A section moved rather than being appended. That is a model change; see step 5. |
| `sections_are_contiguous_and_complete` | The section list in `build.rs` no longer matches the packet. Add or resize the entry. |
| `every_opcode_was_generated` | Opcodes were added or removed. |
| `wire_format_version_is_known` | The wire version bumped. Read the `WIRE_FORMAT_VERSION` comment in `bulk_params.h`, which documents every version's change in one line. |
| `refuses_an_unknown_version_rather_than_guessing` | Update the expected version once you have implemented the new layout. |
| `the_noun_and_type_tables_match_the_vendored_header` | The control-surface nouns or component types changed. Extend `CS_NOUNS` / `CS_TYPES` in `packets.rs` and `CsNoun` / `CsType` in `enums.rs`. |
| `the_expected_firmware_is_the_vendored_one` | `FW_VERSION_*` moved, which is also the version a refused device is told to update to. |

Tests that pin a count or a version (`every_opcode_was_generated`,
`wire_format_version_is_known`, `bulk_size_matches_firmware`) are updated by hand
with a comment citing the header line, which is the point: someone has to read
what changed.

## 4. Handle each new opcode

For every opcode the coverage test reports, either:

- add a registry row (the normal case), or
- add it to the exclusion list **with a written reason**, for opcodes that are
  genuinely not host-driveable.

Never silence the test by loosening the assertion.

## 5. Handle each wire change

Read the new `WIRE_FORMAT_VERSION` comment carefully. Changes fall into classes,
and each has a standard response:

There is no field-descriptor table. What exists is:

- **`sections` in `emit_wire_layout` (`build.rs`)**, the one hand-sized list:
  `(name, size)` in declaration order, sizes computed from the `WIRE_MAX_*`
  constants where the header does. It generates `OFF_<SECTION>`,
  `LEN_<SECTION>`, `SECTIONS` and `BULK_SIZE`.
- **Field offset tables in `crates/dspi-proto/src/wire.rs`**, one per section
  whose fields someone has had to pin: `INPUT_CONFIG_FIELDS`, `SUBHARM_FIELDS`,
  `TUBE_FIELDS` and `LIMITER_OUTPUT_FIELDS`, each `(name, offset, length)` with a
  test that the fields are contiguous and at the header's offsets.
- **Section decoders in `crates/dspi-session/src/state.rs`** (`decode_global`,
  the `Psybass` and `Upmix` views and so on), which read fields from a section
  slice at fixed offsets and are what the screens use.

| Class | Response |
|---|---|
| Section appended | Add an entry to `sections` in `build.rs`, a field table in `wire.rs` with its offset test, and a decoder in `state.rs`. |
| Struct grew | Change its size in `build.rs`; offsets after it shift automatically. Extend the field table. |
| Reserved byte claimed | Add the field to the section's table and decoder. **Check whether it uses a `+1` sentinel**, which several do. |
| New enum value | Add a variant. The `Unknown` fallback means old builds already tolerated it. |
| Model change | The serious case. Update `ChannelMap` and the affected rows. |

**Sentinel encodings are the trap.** Fields added by claiming reserved bytes use
`0 = absent, keep the live value`, and where zero is meaningful they are stored
plus one. Writing a plain zero disables things silently. See
`docs/wire-format.md` section 5, and use `wire::decode_p1` / `wire::encode_p1`.

## 6. Verify against hardware

```sh
cargo run --bin dspi -- dump
```

Confirm the channel count, wire version, firmware version (with its beta),
build and feature list match the device you are holding. A device on another
wire version is refused with a message naming its firmware; that message is the
expected result for an older unit. On macOS, quit DSPi Console first: the vendor
interface is exclusive.

## 7. Write down anything surprising

Record it in `docs/firmware-notes.md`, **in this repository**. The firmware repo
is not modified as part of this work.
