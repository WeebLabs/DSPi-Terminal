# DSPi Terminal

Cross-platform terminal control for [DSPi](https://github.com/WeebLabs/DSPi)
audio processors: a full-screen TUI with response graphing and complete
parameter coverage, plus a scriptable one-shot CLI.

**Status: usable, incomplete.** The interface runs, reads and writes every
parameter, graphs responses, and exchanges files with the Console. See
[`REDESIGN_SPEC.md`](REDESIGN_SPEC.md) for the full design and
[`docs/platforms.md`](docs/platforms.md) for what has actually been verified
where.

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

## Commands

```sh
dspi                              # open the interface
dspi doctor                       # why can't it see my device?

dspi get vol.user                 # read anything
dspi vol.user -18                 # write anything; `set` is optional
dspi eq usb.1 3 peak 2856 3.58 -8.6
dspi params                       # every parameter this build knows

dspi export tuning.txt            # Console-compatible filter file
dspi export room.dspipreset       # whole-device document
dspi import tuning.txt            # apply one back

dspi autoeq search hd600          # headphone corrections
dspi autoeq apply <id>

dspi raw 0x87 32                  # any opcode, hex-dumped
dspi completions zsh              # shell completion
dspi screenshot 120 40 filters    # render a frame, for docs
```

Every command takes `--json` for machine-readable output, `--dry-run` to change
nothing, and `--device <serial>` to pick a unit. Exit codes distinguish bad
input (1) from no device (2) from a write the device refused (3) from a
transport failure (4).

### In the interface

| Key | Does |
|---|---|
| `Ctrl-P` | Search every parameter |
| `:` | Command line, same syntax as the shell |
| `Tab`, `1`-`0` | Panels |
| `G` / `M` | Graph and meters, `m` for the all-channel grid |
| `h` `l` | Graph cursor, with a per-channel readout |
| `+` `-` | Vertical zoom, shared across every view |
| `=` | Graph and table split |
| `F2` | Simple, Advanced or Expert controls |

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
on the same branch are at **V28, 5944 bytes**. Building from the documentation
would produce an app that is wrong on day one. See
[`docs/wire-format.md`](docs/wire-format.md), which is derived from the headers
and checked by tests.

Pinned at `WeebLabs/DSPi` `release/v1.1.6` @ `112f35b`: 202 vendor opcodes,
wire format V28, Control Surfaces caps v13, firmware 1.1.6. `dspi --version`
prints the pin the binary was built from.

To move to a newer firmware, follow `docs/firmware-bump.md`: copy the headers,
rebuild, and the tests report exactly what changed.

## Licence

GPL-3.0-or-later, matching the DSPi firmware and Console.
