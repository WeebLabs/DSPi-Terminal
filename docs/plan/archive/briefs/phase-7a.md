# Phase 7A brief: Settings, general pages

You are implementing **Phase 7A** of the Console-parity plan, in a git
worktree. Read, in order: `docs/plan/PLAN.md` (Phase 7A, section 5),
`docs/plan/DESIGN.md` sections 3, 5, 6 (especially 6.12 the save bar and
6.14 the pin grid) and 7.13, `docs/plan/survey-console.md` sections 1
(Settings window), 2.24 to 2.33 (every page, control, caption, warning and
status string, verbatim) and 4, `docs/plan/survey-firmware.md` sections 1.7,
1.9, 1.12, 1.17 to 1.26, 6.1, 6.6, 6.7, 6.10, 6.12 and 7, the shell and
screens code, `crates/dspi-session/src/{state.rs,pins.rs,write.rs,
preset_file.rs}` (the `PinMap` from Phase 2B; `enable_output` and the
hardware apply order from Phase 2C), `crates/dspi-proto/src/packets.rs`
(`DacHwMuteConfig`, `UartCtrlConfig`, `I2cCtrlConfig`, `CtrlIfaceStatus`,
`PresetDirectory`, `PresetStartup`, `SpdifInputConfig`) and the registry
paths (`preset.startup`, `preset.iomode`, `vol.master.mode`, `dev.dacmute`,
`dev.dacmute.test`, `dev.uart`, `dev.i2c`, `out.type`, `out.pin`, `adat.*`,
`in.spdif.*`, `in.i2s.*`, `in.adat.*`, `in.lg`, `i2s.*`, `dev.save.io`).
The Console for behaviour, read-only: `/Users/weeblabs/DSPi Console/DSPi
Console/DSPi_ConsoleApp.swift` lines 267-1609 and 6704-8408.

## File ownership

You own ONLY `crates/dspi-tui/src/settings/` (new): `mod.rs` (the Settings
shell: grouped sidebar with availability gating, `[`/`]` history, the save
bar with the Console's three dirty categories and Revert, page switching),
`overview.rs` (pin map), `about.rs`, `graphing.rs`, `advanced.rs`,
`global.rs`, `outputs.rs`, `inputs.rs`, `i2s.rs`, `interfaces.rs`, a
`config.rs` for the app-side settings file (`~/.config/dspi/config.toml` or
the platform equivalent via `dirs`, holding Graphing and the volume-mode
choice; add `dirs` and `toml` to `crates/dspi-tui/Cargo.toml`), plus `pub
mod settings;` in `lib.rs`, the `settings()` arm of the `ConsoleScreens`
factory in `live.rs`, and `examples/gallery.rs` (`--settings <page>`).
Leave `settings/surfaces.rs`, `groups.rs`, `macros.rs` to Phase 7B: your
sidebar lists them as pages that say "Control surfaces arrive with Phase
7B" until then.

## What to build

Every page as the survey lists it, in the Console's order, with its
captions, pickers (pin pickers list `GPIO n` for free pins and mark claimed
ones with their owner from `PinMap`; role-constrained candidates from
`pins.rs`), toggles, warnings and inline status rows (`PIN_CONFIG_*` and
the interface status strings verbatim). Global Parameters is a staged draft
applied on Save; Outputs / Inputs / I2S write live and, in
`OUTPUT_CONFIG_MODE_INDEPENDENT`, mark the output-config dirty category
until `dev.save.io`; Control Interfaces has per-interface Apply / Revert.
The About page shows the app name, "USB Audio DSP Controller", the version,
"Made with love by Weeb Labs" and the five link URLs as text. Graphing
writes `config.rs` and the live runner reads it on start (add a
`GraphSettings::from_config` only in your files; the reviewer wires the
load).

## Tests

Golden frames for every page at 80x24 and 120x40; key tests for every key
in `keys()`; a test that the save bar appears only while dirty and that
Revert restores the draft; a test that a claimed pin is marked in a picker;
a config round-trip test.

## Rules and report

As the Phase 4 brief. Commit per page with `Phase 7A:` subjects. Report
files, kit gaps, behaviours not reproduced, gallery commands, test counts.
