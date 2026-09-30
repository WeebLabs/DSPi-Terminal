# Phase 10 brief: the audit

You are auditing the DSPi Terminal Console-parity work on branch
`console-parity`, in a git worktree. You are not implementing; you are
checking, and writing `docs/plan/archive/audit.md`. Read, in order: `docs/plan/
PLAN.md` (the exit conditions in section 0, the per-phase exit criteria in
section 2, the coverage matrix in section 4, the working agreements in
section 5), `docs/plan/DESIGN.md` (the specification, including section 11),
`docs/plan/survey-console.md` (every Console control), and then the code.

## What to check, and how

For every row of the coverage matrix: find the Terminal code that
implements it, run the gallery command that shows it (`cargo run -q -p
dspi-tui --example gallery -- 120 40 console rp2350 --screen <s>` or
`--settings <page>`), and compare against the Console's survey entry
control by control: every control present, every label, caption, warning
and button verbatim (hyphens for em-dashes are the one allowed change),
every range and step, every conditional (platform, feature, firmware age).
Record each row as `pass`, `partial` (with what is missing) or `fail`.

For every phase's exit criteria in `PLAN.md` section 2: state whether each
is met, with the test names that prove it (`cargo test --workspace` lists
them) or the command output that shows it.

For the working agreements: grep for em-dashes in code, strings and docs;
check that no channel count, band count, filter-type table or feature
presence is compiled in (search for literals such as `17`, `9`, `12`,
`0x1FF`, `Rp2350` in the tui crate and judge each); check every key named
on a key line or in a `keys()` table is handled (the tests exist; confirm
they cover every screen); check every screen has golden frames at 80x24
and 120x40.

Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --
-D warnings`, `cargo test --workspace`, `cargo build --workspace
--all-targets`, and `cargo test --workspace -- --ignored` if any tests are
ignored, and report the numbers.

Then read, with particular care, three components the plan's owner will
audit in depth after you, and write down anything you find so that audit
can start from your notes: (1) `crates/dspi-session/src/{state.rs,
notify.rs}` and the notification path through `crates/dspi-tui/src/
live.rs` `tick`; (2) `crates/dspi-tui/src/settings/{surfaces,groups,
macros,cs_model}.rs` against `docs/plan/survey-firmware.md` section 3 and
`crates/dspi-proto/firmware/control_surfaces.h`; (3) `crates/dspi-tui/src/
{theme.rs,graph.rs,widgets/}` against `DESIGN.md` sections 4 to 6.

## What to produce

`docs/plan/archive/audit.md` with: a verdict per exit condition; the matrix with a
verdict per row and a one-line reason for anything not `pass`; a list of
defects found, each with file:line, what is wrong, what the Console or
header says, and a severity (blocks parity / wrong but usable / cosmetic);
the verification numbers; and the notes for the three deep audits. Do not
fix anything; do not commit anything but `audit.md`. Subject `Phase 10:`;
no trailers; no push.
