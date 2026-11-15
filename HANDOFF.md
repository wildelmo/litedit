# Handoff — state of work when the session ended (2026-11-15)

Read this first, then `CLAUDE.md` and `ROADMAP.md`.

## Rules the owner set (keep following them)

- Commits are authored by **Joe Chun <wildelmo@users.noreply.github.com>**,
  with **no Co-Authored-By / attribution lines** and **no tags**.
- Commit to `main`; no pull requests unless asked. Never force-push `main`.
- The owner cannot test interactively: Windows CI (`.github/workflows/windows.yml`)
  is the acceptance gate. Record decisions in `DECISIONS.md`.

## What is on `main` and verified

- Version 1.1.0 (unreleased, no tag). Everything through Epic 6 is on `main`.
- Last fully green Windows CI run: commit `9395405` (all real-window tests
  pass, including one-window hand-over and the "Open with" registry test;
  exe 6.76 MB; one window 19 MB private; 12 viewed markdown tabs ≈ 22 MB).
- Linux: 241 tests pass. The Windows target type-checks from Linux with
  `cargo check --target x86_64-pc-windows-msvc`.

## In flight when the session ended

### 1. Independent review of Epic 6 (not finished — no findings received)

A reviewer was examining `git diff c7764b0..a4cf07e` (hidden-tab memory, go
to heading, checkbox clicks, find in preview, peek tab, one window, "Open
with"). **Its findings were never received.** Re-run it: an adversarial
review of that diff with no hints, then fix what it confirms (story S7.0)
before merging Epic 7 branches. The two earlier reviews each found
data-loss bugs, so do not skip this.

### 2. Epic 7 — five feature branches (work in progress, NOT reviewed, NOT merged)

Each was being built by a sub-agent from `main` at `23fe4a4`, following
`docs/AGENT-RULES.md`. The branches were pushed as snapshots; they may not
compile or pass tests. Treat them as drafts: finish, test, review, then
merge one at a time (expect small conflicts in `src/app.rs`, `src/main.rs`,
`src/palette.rs`).

| Branch | Story | Brief |
|---|---|---|
| `s7-bigfile` | S7.1 | Highlight only as far as the view needs (not the whole file); lower the memory peak while loading; read-only lock ("View: Toggle Read-Only", status shows it). Measured before: 6.9 MB/100k-line file = 59 MB peak just opened, 72 MB fully coloured (Linux release). |
| `s7-image` | S7.2 | `src/view/image.rs`: png/jpg/gif/webp/bmp open in a viewer tab (fit / actual size, pan, status with size); pixels freed in `on_hide`; reload on disk change. |
| `s7-nav` | S7.3 | Back / Forward (Alt+Left / Alt+Right) over a bounded location history; remember each file's position on close/reopen and across restarts (workspace + session files, old files must still load). |
| `s7-compare` | S7.4 | Compare Left ↔ Right of a split; ignore-whitespace toggle (Alt+W); double-click / Enter opens the file at that line; compare with clipboard. |
| `s7-tree` | S7.5 | Tree multi-select (Ctrl/Shift+click), multi delete / move with one confirmation, copy / paste / duplicate (never overwrite, bounded, no symlink following), file size + age in the status bar. |

Integration checklist per branch: read the diff; `cargo fmt`; `cargo test`;
`cargo clippy`; `cargo check --target x86_64-pc-windows-msvc`; mutation-check
its key tests; update `ROADMAP.md` (Epic 7 checkboxes + progress log),
`CHANGELOG.md`, `README.md` key table, `docs/LITE-XL-COMPARISON.md`,
`DECISIONS.md` if a choice was made; commit on `main`; push; confirm Windows
CI. Finish with an independent review of the whole Epic 7 diff.

## Open questions for the owner (also in DECISIONS.md)

- D-018 input methods (IME) untested; D-019 AltGr layouts unverified;
  D-025 title-bar theme and custom fonts only checkable on a real desktop.
- Release: `main` is 1.1.0. Tag `v1.1.0` on `main` (after dating the
  changelog) to publish; the session could not push tags. `v1.0.0` would have
  to point at commit `c8b735f`.
- Commits before `3c41dc5` are authored by the assistant with a co-author
  line; changing that needs a history rewrite and force-push by the owner.

## Useful facts

- Headless driving: `cargo run -- --script file` (commands at the top of
  `src/script.rs`); screenshots with `shot out.png`.
- Memory measurements: Linux release build, Python
  `resource.getrusage(RUSAGE_CHILDREN)` around `litedit --script`.
- Sub-agent worktrees used `cp -al target/debug` to share the build cache
  (builds in ~10 s each); the Agent tool's own worktree isolation failed in
  this repo (no `origin/main` ref), so worktrees were created by hand.
