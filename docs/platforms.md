# Platform support

*Last verified 2026-07-27 against `release/v1.1.5` @ `9776c2f`.*

## Status

| Platform | Target triple | Type-checks | Linked | Run on hardware |
|---|---|---:|---:|---:|
| macOS (Apple silicon) | `aarch64-apple-darwin` | yes | yes | **yes** |
| macOS (Intel) | `x86_64-apple-darwin` | yes | no | no |
| Windows | `x86_64-pc-windows-msvc` | yes | no | no |
| Linux, Raspberry Pi 3/4/5 | `aarch64-unknown-linux-gnu` | yes | no | no |
| Linux, older Pi | `armv7-unknown-linux-gnueabihf` | yes | no | no |

**Be careful reading that table.** Everything type-checks on every target, and
the platform-specific *logic* is unit-tested for all of them, but only macOS on
Apple silicon has actually been linked and run against a device. `cargo check`
proves the code compiles; it does not prove it links, and it certainly does not
prove the USB backend behaves. Treat the other rows as "expected to work,
unverified" until someone runs `dspi doctor` on one.

The remaining verification needs either the hardware or a cross-linking
toolchain, neither of which was available here.

## What each platform needs

### Windows

Nothing to install. The firmware ships MS OS 2.0 descriptors, so Windows binds
WinUSB to the vendor interface automatically; there is no Zadig step.

Two constraints shaped the code:

- **WinUSB caps a control transfer at 4 KB**, and the bulk parameter packet is
  6136 bytes, so the chunked opcodes are used **on every platform**, not just
  Windows. One code path, exercised everywhere, rather than a Windows-only path
  that nobody tests.
- **`nusb` cannot do device-level control transfers on Windows**, and WinUSB
  additionally requires the low byte of `wIndex` to match the claimed interface.
  All transfers therefore go through a claimed interface, and `wIndex` is not a
  parameter anywhere in the transport API. The protocol already requires
  `wIndex == 2`, so these agree.

Terminal support differs sharply between the two consoles, and the code detects
which one it is in: Windows Terminal and ConEmu get truecolor and braille, while
the classic console gets 16 colours and block glyphs, because it renders braille
as boxes in most fonts. Windows sets neither `TERM` nor `COLORTERM`, so the usual
Unix sniffing would have reported 16 colours for Windows Terminal.

### macOS

Nothing to install. Interfaces 0 and 1 belong to the audio class driver;
interface 2 is free.

**The vendor interface is exclusive.** DSPi Console and this tool cannot hold the
same device at once, and the failure surfaces as a permission-style error that
has nothing to do with permissions. `dspi doctor` says so explicitly rather than
repeating the Linux advice.

### Linux and Raspberry Pi

Needs a udev rule for non-root access:

```sh
sudo dspi --install-udev
sudo udevadm control --reload
```

then unplug and replug the device. The rule also covers the RP2 bootloader the
device re-enumerates as during a firmware update; without that, an update appears
to hang at exactly the wrong moment.

`dspi doctor` checks for the rule and prints the exact fix when it is missing.

### Raspberry Pi specifically

`--lite` reduces the redraw rate: meters at 10 Hz instead of 20, and no
animation. It is chosen automatically on a single-core or ARMv6 part, and over
SSH, where the cost of a repaint is the link rather than the processor.

## Deliberately not assumed

- No C toolchain anywhere. USB goes through `nusb`, which talks WinUSB, IOKit and
  usbfs directly, so cross-compiling is a plain `cargo build --target`.
- No `std::os::unix`, no hardcoded paths outside the Linux-only udev installer,
  and no shelling out.
- `NO_COLOR` is honoured, and `TERM=dumb` degrades to plain ASCII, so output
  captured to a file or a CI log stays readable.

## Verifying a platform

```sh
cargo check --workspace --target <triple>   # compiles
cargo test --workspace                      # logic, including other platforms
dspi doctor                                 # the real answer, on the machine
```

`dspi doctor` is the one that counts: it enumerates, opens, claims the interface,
probes the firmware, and reports the terminal's capabilities, with a fix for each
failure written for the platform it is running on.
