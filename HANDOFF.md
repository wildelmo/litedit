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
- Linux: 386 tests pass. The Windows target type-checks from Linux with
  `cargo check --target x86_64-pc-windows-msvc`.

## In flight when the session ended

### 1. Independent review of Epic 6 — done

Findings fixed as story S7.0 (see the ROADMAP progress log).

### 2. Epic 7 — merged

All five stories (S7.1–S7.5) were finished by sub-agents, reviewed, and
squash-merged into `main`; see the ROADMAP progress log. The `s7-*` remote
branches are stale snapshots and can be deleted.

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
