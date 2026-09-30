# Phase 8 brief: Stats, the notification log, files, AutoEQ, tools

You are implementing **Phase 8** of the Console-parity plan, in a git
worktree. Read, in order: `docs/plan/PLAN.md` (Phase 8, section 5),
`docs/plan/DESIGN.md` 3, 6.11, 7.10 to 7.14, `docs/plan/survey-console.md`
sections 2.20 to 2.23, 3 and 4 (every section, string, dialog and menu
action, verbatim), `docs/plan/survey-firmware.md` 6.2 to 6.7, the shell,
screens and live runner, `crates/dspi-session/src/{state,notify,
preset_file,filterfile,autoeq,undo}.rs` and `crates/dspi-proto/src/
packets.rs` (`BufferStatsPacket`, `SpdifRxStatusPacket`, `LgSoundSyncStatus`,
`AdatStatus`, `I2sSlaveStatusPacket`), `crates/dspi/src/main.rs` (the
existing import / export / autoeq one-shot commands and the batching from
Phase 2C). The Console for behaviour, read-only: `StatsView.swift`,
`InterruptMonitor.swift`, `AutoEQ/AutoEQBrowser.swift`,
`AutoEQ/AutoEQManager.swift`, `DSPi_ConsoleApp.swift` lines 8665-9800 (file
actions, tools, alerts), `PresetDocumentTransfer.swift`, `FilterFile.swift`,
and the Console's `state_export_plan.md` for the Windows filter-file
dialect.

## File ownership

You own ONLY `crates/dspi-tui/src/screens/{stats,monitor,autoeq}.rs`,
`crates/dspi-tui/src/actions.rs` (new: the file and tools actions with
their dialogs), their `pub mod` lines, the `Tool::Stats | Monitor | AutoEq`
arms and the action routing in `live.rs` (the `:import`, `:export`,
`:import-config`, `:export-config`, `:autoeq`, `:save-master`,
`:save-output-config`, `:revert`, `:factory-reset`, `:bootloader`, `:undo`,
`:redo` verbs and the palette entries for them), `crates/dspi-session/src/
filterfile.rs` (the Windows dialect: `[Master L]` headers, `Crossover ...
Slope 24 dB/oct`, `BYP`, `NO`, comma decimals; read both dialects, write
ours), `crates/dspi-session/src/autoeq.rs` (Update Database: rebuild from
GitHub with `ureq` or the crate already in the tree, import file, reset to
built-in, favourites in the config file), `examples/gallery.rs`.

## What to build

Stats (survey 2.20): every section as label / value rows, badges as pills,
buffer fills as short meters with min-max watermarks, `r` resets
watermarks, 2 s refresh through a `Screen::tick` that asks the session for
the packets (add `Session` reads in `actions.rs`, called from `live.rs`
tick), the footer text. Interrupt Monitor (2.21): the `Notifications` log
as a table (time, seq, event, source, decoded field name from
`BulkPacket::section_at` and the field tables in `wire.rs`, value),
`Space` pause, `D` clear, the header state and count. AutoEQ browser
(2.23): search, source pills, favourites (`f`), Apply through the existing
`autoeq::apply`; the Favourites section; `:autoeq update` with the three
methods and the progress dialog and result strings. Files: `:import` and
`:export` (filter files, the single- and multi-channel channel-picker
dialogs, unsupported types skipped and reported), `:import-config` /
`:export-config` (`.dspipreset` with the options checklist showing
provenance and the cross-platform warning, the progress dialog, the
result report verbatim), path entry as a text dialog with Tab completion.
Tools: Commit (`Ctrl-S`, already there; add the `:commit` verb), Revert to
Saved, Factory Reset (critical), Firmware Update (critical; after
`dev.bootloader` wait for the device to vanish and the `RPI-RP2` volume to
appear, then tell the person where to copy the `.uf2`), Save Master
Volume, Save Output Configuration; the unsaved-changes prompt on device
switch (the device picker: `Ctrl-D`, listing `dspi_transport::list_devices`,
switching with the prompt); undo / redo through `Session::undo` with the
undone command on the echo line.

## Tests

Golden frames for the three panels; a decode test for the monitor's field
naming; filter-file dialect round-trips against fixtures you add under
`tests/fixtures/` (one macOS, one Windows, one REW); an import-report
wording test; a mock test for the bootloader handoff's waiting logic.

## Rules and report

As the Phase 4 brief. Commit per panel and per action group with `Phase 8:`
subjects. Report files, kit gaps, behaviours not reproduced, network use
(the GitHub rebuild must be opt-in and say what it fetches), test counts.
