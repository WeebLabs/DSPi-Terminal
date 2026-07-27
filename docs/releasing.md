# Releasing

## What CI proves

`.github/workflows/ci.yml` runs on every push. It does three things the local
loop cannot:

- **Builds on all three desktop platforms**, not just the developer's. `cargo
  check` proves code compiles; only a real build proves it links, and until CI
  ran, nothing but macOS had ever been linked.
- **Cross-compiles both Raspberry Pi targets** with `cross`, which links against
  a real ARM sysroot rather than type-checking against a target spec.
- **Parses the generated shell completions** with `bash -n`, `zsh -n` and
  `fish --no-execute`. A completion script that fails to parse is worse than
  none, because it breaks the user's shell startup.

Clippy runs with `-D warnings`, so a lint is a build failure rather than
something to scroll past.

## What CI still does not prove

**No test touches hardware.** Every USB path is exercised against
`MockTransport`, which models stalls, timeouts and the flash blackout, but a
mock cannot be wrong in the same way a device can. The hardware smoke suite
(`--features hardware`, still to be written) is the missing piece, and it needs
a device attached to a runner.

Until then, `dspi doctor` on the target machine is the real check. It
enumerates, opens, claims the interface and probes the firmware, which is
exactly the sequence that fails when a platform assumption is wrong.

## Cutting a release

```sh
git tag v2.0.0
git push origin v2.0.0
```

`release.yml` builds seven targets and uploads an archive for each, carrying the
binary, the README and the udev rule. The rule ships in every archive, not only
the Linux ones: it costs nothing and means the fix is always to hand when
someone unpacks the wrong archive on a Pi.

## Version numbers

The application version is independent of the firmware version. `dspi --version`
prints both, plus the wire format:

```
dspi 2.0.0 (protocol from DSPi release/v1.1.5 @ 9776c2f)
wire format V26, 190 vendor opcodes
```

That pairing is what makes a bug report actionable: "wire V26" says more about
what the app was talking to than an app version ever could.
