# DSPi Terminal

Cross-platform terminal control for [DSPi](https://github.com/WeebLabs/DSPi)
audio processors: the DSPi Console's layout and every one of its controls,
in a full-screen terminal interface, plus a scriptable one-shot CLI.

**Status: Console-parity work in progress on the `console-parity` branch.**
The plan, the design and the surveys it rests on are in
[`docs/plan/`](docs/plan/). `docs/plan/PLAN.md` carries the coverage matrix
that says which Console feature lives where, and what is still open.

## The interface

```
 DSPi  RP2350 · fw 1.1.6 · A1B2C3D4                       ● Connected   Preset 3: Living Room *
╭──────────────────────╮╭─────────────────────────────────────────────────────────────────────╮
│ INPUTS               ││+20                     ┆                        ┆         ┆           │
│▍FL      ▓▓▓▓▓▓▓▌▏ IN1││    ⠉⠉⠓⠒⠒⠒⠦⠤⠤⠤⣄⣀⣀⣀⣀⡀                                 ⢀⣀⣀⡀   ┆       │
│▍FR      ▓▓▓▓▓▓▓▌▏ IN2││  0 ⠤⠤⠤──⠤⠤⠤──⠤⠤⠤──⠉⠉⠉⠉⠓⠒⠒⠦⠤⢤⣀⣀⡀──⠤⠤⠤──⠤⣀⣀⣀⣀⣀⣀⣠⠤⠤⠤⠤⠤⠤⠴⠒⠒⠚⠉⠉─⠤⠉⠓⢦⡀⠤⠤│
│ FC      ░░░░░░░░▏ IN3││-20                                                                  │
│ ...                  ││       20     50    100    200     500   1k    2k     5k   10k  20k  │
│ OUTPUTS              ││ ● IN1  ● IN2=  ● IN3  ● IN4=  ...  ● OUT1  ● OUT2=  ● OUT9          │
│ OUT L  ▓▓▓▓▓▓▓▌▏ OUT1││─ ● FL ──────────────────────────────────────────────────────────────│
│ OUT R  ▓▓▓▓▓▓▓▌▏ OUT2││ Link 1/2   Preamp -5.3 dB ━━━━━━━━━━━━━━━━●━━━━━━━━━━━━   Clear PEQ  │
│ Sub    ▓▓▓▓▓▓▓▓▏ OUT9││   # TYPE                    FREQ      GAIN   WIDTH                  │
│──────────────────────││ ● 1 Low Shelf 12 dB/oct   105 Hz   +8.8 dB                          │
│ M X● L○ V○ P● T , b○ ││ ● 2 Peaking                64 Hz   -6.2 dB     0.3                  │
│ Preset ‹3: Living …› ││ ● 3 Peaking              2856 Hz   -8.6 dB    3.58                  │
│ Source ‹USB›         ││   4 Off                                                             │
│ Volume User -12.0 dB ││                                                                     │
│ ━━━━━━━━━━━━━━━━━●━━ ││                                                                     │
│ C0 ▓░░ 31% C1 ▓▓░ 74%││                                                                     │
╰──────────────────────╯╰─────────────────────────────────────────────────────────────────────╯
 :eq in.1 3 freq 2856
 ↑ ↓ Select a channel · Enter Back to the overview · Space Show or hide its curve · ? help
```

The shape is the Console's: a channel sidebar with meters, clip cells and
visibility pills; the response graph with a legend; below it the overview,
an input page or an output page, switched by the selection. The Console's
tool windows are panels that replace the right pane; Settings replaces the
screen. Every channel keeps the Console's colour everywhere it appears.

### Keys

Uppercase letters open the Console's tools, with the Console's own
mnemonics (its Shift-Cmd-M is `M` here):

| Key | Opens |
|---|---|
| `M` | Matrix Mixer |
| `L` `X` `V` `P` `U` | Loudness, Crossfeed, Volume Leveller, Psychoacoustic Bass, Stereo Upmixer |
| `G` | Test Signals |
| `T` `I` | Stats for Nerbs, Interrupt Monitor |
| `B` | AutoEQ |
| `,` | Settings |

Everywhere:

| Key | Does |
|---|---|
| `Tab` | Next region: channels, footer, legend, detail |
| `↑ ↓` | Select a channel; `Enter` returns to the overview |
| `Space` | Toggle: a curve, a bypass, a feature in the quick strip, mute on the volume row |
| `Ctrl-P` | Search every parameter, tool and action |
| `:` | Command line, the same grammar as the shell |
| `Ctrl-S` | Commit Parameters to the active preset |
| `Ctrl-Z` `Ctrl-Y` | Undo, redo |
| `= g p + - h l` | Graph height, pop-out, phase, zoom, cursor |
| `b` `c` | Bypass Master EQ, clear clip latches |
| `?` | Help for whatever has focus |
| `q` | Quit, with the unsaved-changes prompt |

The echo line prints every change as the `:` command that would make it, so
anything done in the interface can be pasted into a script.

## Building

```sh
cargo build --release
cargo test --workspace
```

Requires Rust 1.90 or newer. There is no C toolchain dependency: USB access goes
through `nusb`, which talks WinUSB, IOKit and usbfs directly.

## One-shot commands

```sh
dspi                              # open the interface
dspi doctor                       # why can't it see my device?
dspi dump                         # the whole device state
dspi watch                        # notifications as they arrive

dspi get vol.user                 # read anything
dspi vol.user -18                 # write anything; `set` is optional
dspi eq in.1 3 peak 2856 3.58 -8.6
dspi mix 0 4 on -3 inv            # a crosspoint
dspi set cs.binding 3 type=encoder noun=user_volume action=step gpio=10,11
dspi params                       # every parameter this build knows

dspi export tuning.txt            # Console-compatible filter file
dspi export room.dspipreset       # whole-device document
dspi import tuning.txt            # apply one back
dspi -f evening.dspi              # a script, one command per line

dspi autoeq search hd600          # headphone corrections
dspi undo                         # put back what the last change replaced
dspi raw 0x87 41                  # any opcode, hex-dumped
dspi completions zsh              # shell completion
```

Every command takes `--json` for machine-readable output, `--dry-run` to change
nothing, `--quiet`, and `--device <serial>` to pick a unit. Exit codes
distinguish bad input (1) from no device (2) from a write the device refused
(3) from a transport failure (4).

## Device access

- **Windows**: nothing to install. The firmware ships MS OS 2.0 descriptors, so
  WinUSB binds automatically.
- **macOS**: nothing to install. The vendor interface is *exclusive*, so DSPi
  Console and this tool cannot hold the device at the same time.
- **Linux and Raspberry Pi**: `sudo dspi --install-udev`, then unplug and
  replug the device.

See [`docs/platforms.md`](docs/platforms.md) for what has been verified where.

## Protocol source of truth

The firmware headers in `crates/dspi-proto/firmware/` are vendored from
`WeebLabs/DSPi` at `release/v1.1.6` @ `112f35b` (wire format V28, 202 vendor
opcodes, control-surface caps v13) and parsed at build time to generate every
opcode, constant and struct offset. Nothing is transcribed by hand; a
coverage test refuses to build if an opcode has no registry row. To move to
a newer firmware, follow [`docs/firmware-bump.md`](docs/firmware-bump.md).

The device pushes its own changes over a notification endpoint, so a knob on
a control surface, a remote, or the operating system's volume slider shows up
in the interface without a refresh, with a note saying who changed it.

## Licence

GPL-3.0-or-later, matching the DSPi firmware and Console.
