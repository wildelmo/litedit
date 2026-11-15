# Handoff — state of work (2026-11-15)

Read this first, then `CLAUDE.md` and `ROADMAP.md`.

## Rules the owner set (keep following them)

- Commits are authored by **Joe Chun <wildelmo@users.noreply.github.com>**,
  with **no Co-Authored-By / attribution lines** and **no tags**.
- Commit to `main`; no pull requests unless asked. Never force-push `main`.
- The owner cannot test interactively: Windows CI (`.github/workflows/windows.yml`)
  is the acceptance gate. Record decisions in `DECISIONS.md`.

## What is on `main` and verified

- Version 1.1.0 (unreleased, no tag). Everything through Epic 7 is on `main`.
- Windows CI (all real-window tests, one-window hand-over, "Open with"
  registry test) is green; numbers from the run on `c9001e3`: exe 6.89 MB,
  one window 21.6 MB private, 10 tabs 22.4 MB private, first render 89 ms.
- Linux: 394 tests pass. The Windows target type-checks from Linux with
  `cargo check --target x86_64-pc-windows-msvc`.
- Only checked from Linux, not on a real Windows desktop: skipping
  junctions in tree copies (tests are unix symlinks), case-insensitive
  matching of open tabs to tree entries, how long recycling thousands of
  entries takes.

## Nothing is in flight

Epics 0–7 are on `main`. Epic 7 (S7.0–S7.5) was built on branches by
sub-agents, reviewed, squash-merged, then reviewed again independently
(two reviewers); every confirmed finding is fixed with a regression test.
See the ROADMAP progress log. The `s7-*` branches on the remote are stale
snapshots and can be deleted.

Not addressed (recorded, low risk): on Windows a tab opened under a
differently-cased path is now matched to tree entries, but this could only
be unit-tested from Linux (`fsops::is_under`).

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
