# Common rules for the beta4 phases (B2 to B10)

Every phase brief in this folder named `phase-b*.md` assumes these rules.

## Read first

1. `docs/plan/PLAN-beta4.md`, especially section 2 (settled decisions:
   V32 only; firmware update and on-graph editing are out of scope; tool
   keys `S`, `D`, `A`).
2. `docs/plan/survey-firmware-beta4.md` and
   `docs/plan/survey-console-beta4.md`: what changed upstream. They are
   the specification, but confirm protocol facts against the vendored
   headers in `crates/dspi-proto/firmware/` and cite header lines, and
   confirm Console wording against the Swift source.
3. `docs/plan/DESIGN.md`: the Terminal's design, especially section 11
   (accepted deviations: every write is followed by a re-read; own writes
   recognised by source), section 12 (the quiet redesign: calm palette, one
   curve per graph) and section 13 (the page command bar on `;`).
4. `docs/plan/PLAN.md` section 5: working agreements.

References, read-only, never modify: the firmware at `/Users/weeblabs/DSPi`
(commit `557bce7`) and the Console at `/Users/weeblabs/DSPi Console`
(commit `9dbb07a`).

## Rules

- No em-dashes anywhere: code, UI strings, docs, commit messages.
- UI strings follow the Console's wording verbatim where the Console has a
  string. Hyphens replace the Console's em-dashes.
- Nothing about device shape is compiled in. Channel counts, output
  counts, band counts and feature presence come from `DeviceState` and
  the probes.
- Every protocol fact cites a header line in a comment where it is
  encoded.
- Build new panels from the existing widget kit and patterns
  (`screens/panel.rs`, `screens/psybass.rs`, `screens/loudness.rs`), and
  follow the style of the surrounding code, including its comment density.
- Every new screen or page gets golden-frame tests at 80x24 and 120x40
  through `render_to_string`, in the style the existing screens use, and
  every key named on screen must be bound (the key-hint tests enforce
  this; extend them to new screens).
- The gallery example (`crates/dspi-tui/examples/gallery.rs`) and the
  fixture (`crates/dspi-tui/src/shell/fixture.rs`) should be able to show
  every new screen without a device.
- Only touch what your phase needs. Other phases are running in parallel
  worktrees; keep edits to shared files (`shell/mod.rs`, `live.rs`,
  `screens/mod.rs`, `settings/mod.rs`, `state.rs`) small and additive so
  merges stay easy.

## Finishing

Before your final commit, all of these must pass:

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Commit in small, coherent commits on your worktree branch. Subjects follow
this branch's style: a plain imperative sentence describing the behaviour
("Show the limiter on the output page"), no phase prefixes. End every
message with:

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>

Do not push and do not merge into other branches. Your final message is a
report with: worktree path and branch name; commits (hash and subject);
test counts before and after; every place where a survey disagreed with the
headers or the Console source; every decision you made that the brief did
not settle; anything left undone, with the reason.
