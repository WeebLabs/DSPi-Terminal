# Moving to a newer firmware

The point of this procedure is that **no step involves changing existing code**.
The generator and the tests tell you exactly what is new; you add rows, not
edits. If you find yourself rewriting something, that is a signal the
architecture has sprung a leak, and it is worth fixing there instead.

## 1. Copy the headers

```sh
FW=/path/to/DSPi                     # on the release branch you are targeting
DST=crates/dspi-proto/firmware

for h in config.h bulk_params.h control_surfaces.h psybass.h upmix.h siggen.h; do
  cp "$FW/firmware/DSPi/$h" "$DST/$h"
done

( cd "$FW" && git rev-parse HEAD && git rev-parse --abbrev-ref HEAD )
```

Update `crates/dspi-proto/firmware/PROVENANCE.toml` with the new branch, commit,
and date, then refresh the checksums:

```sh
( cd crates/dspi-proto/firmware && shasum -a 256 *.h > SHA256SUMS )
```

Track the **release branch**, not `main`.

## 2. Rebuild

```sh
cargo build -p dspi-proto
```

Watch the build log. Any constant that became platform-conditional is reported
as `platform-conditional, not emitted: NAME`. That is the generator refusing to
let a per-platform value become a compiled-in assumption; the value must come
from a capability probe instead.

## 3. Let the tests tell you what moved

```sh
cargo test --workspace
```

Expect these to speak up first:

| Failing test | Means |
|---|---|
| `bulk_size_matches_firmware` | A section changed size, so every offset after it moved. Re-derive `docs/wire-format.md`. |
| `sections_are_contiguous_and_complete` | The section list in `build.rs` no longer matches the packet. Add or resize the entry. |
| `every_opcode_was_generated` | Opcodes were added or removed. |
| `wire_format_version_is_known` | The wire version bumped. Read the `WIRE_FORMAT_VERSION` comment in `bulk_params.h`, which documents every version's change in one line. |
| `refuses_an_unknown_version_rather_than_guessing` | Update the expected version once you have implemented the new layout. |

## 4. Handle each new opcode

For every opcode the coverage test reports, either:

- add a registry row (the normal case), or
- add it to the exclusion list **with a written reason**, for opcodes that are
  genuinely not host-driveable.

Never silence the test by loosening the assertion.

## 5. Handle each wire change

Read the new `WIRE_FORMAT_VERSION` comment carefully. Changes fall into classes,
and each has a standard response:

| Class | Response |
|---|---|
| Section appended | Add an entry to `sections` in `build.rs`, plus `FieldDesc` rows. |
| Struct grew | Change its size in `build.rs`; offsets after it shift automatically. |
| Reserved byte claimed | Add a `FieldDesc` row with `since`. **Check whether it uses a `+1` sentinel**, which several do. |
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

Confirm the channel count, wire version, and feature list match the device you
are holding. On macOS, quit DSPi Console first: the vendor interface is exclusive.

## 7. Write down anything surprising

Record it in `docs/firmware-notes.md`, **in this repository**. The firmware repo
is not modified as part of this work.
