# DSPi Terminal

Cross-platform terminal control for [DSPi](https://github.com/WeebLabs/DSPi)
audio processors: a full-screen TUI with response graphing and complete
parameter coverage, plus a scriptable one-shot CLI.

**Status: in development.** M1 (protocol foundations) is landing; there is no UI
yet. See [`REDESIGN_SPEC.md`](REDESIGN_SPEC.md) for the full design.

## What it will do

- Reach **every** firmware parameter: EQ, crossovers, matrix mixer, delays,
  loudness, crossfeed, leveller, upmixer, psychoacoustic bass, control surfaces
  and IR remotes, the signal generator, ADAT, I2S, presets, and physical I/O.
- Graph channel responses at parity with the DSPi Console: 201-point Bode plots,
  magnitude and phase, all channels at once.
- Meter every channel, with sticky clip latches.
- Run without any prior knowledge, via a browsable interface, or at full speed
  via a command palette and a typed command line.
- Work as a one-shot command for scripting: `dspi eq in.1 3 peak 2856 3.58 -8.6`.
- Run on Windows, macOS, Linux, and a Raspberry Pi.

## Building

```sh
cargo build --release
cargo test --workspace
```

Requires Rust 1.90 or newer. There is no C toolchain dependency: USB access goes
through `nusb`, which talks WinUSB, IOKit and usbfs directly.

## Current commands

```sh
dspi list        # every connected DSPi
dspi dump        # connect and print full discovered state
dspi dump --json # the same, machine readable
dspi --version   # app version plus the firmware revision the protocol came from
```

## Device access

- **Windows**: nothing to install. The firmware ships MS OS 2.0 descriptors, so
  WinUSB binds automatically. No Zadig.
- **macOS**: nothing to install. Note that the vendor interface is *exclusive*,
  so DSPi Console and this tool cannot hold the device at the same time.
- **Linux and Raspberry Pi**: install the udev rule.
  ```sh
  sudo cp xtask/70-dspi.rules /etc/udev/rules.d/
  sudo udevadm control --reload
  ```
  Then unplug and replug the device.

## Protocol source of truth

The firmware headers in `crates/dspi-proto/firmware/` are vendored from a pinned
DSPi commit and parsed at build time to generate every opcode, constant and
struct offset. Nothing is transcribed by hand.

This matters: the firmware's released `commands.md` documents wire format **V14
at 3664 bytes** with a channel model that was removed at V16, while the headers
on the same branch are at **V26, 5944 bytes**. Building from the documentation
would produce an app that is wrong on day one. See
[`docs/wire-format.md`](docs/wire-format.md), which is derived from the headers
and checked by tests.

To move to a newer firmware, follow `docs/firmware-bump.md`: copy the headers,
rebuild, and the tests report exactly what changed.

## Licence

GPL-3.0-or-later, matching the DSPi firmware and Console.
