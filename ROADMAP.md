# litedit roadmap

litedit is an ultralight, memory-safe text editor for Windows, written in Rust.
It is built around one core idea: **markdown opens as a pretty preview by
default**, and everything else (code, text) is shown with syntax colors. It uses
[lite-xl](https://github.com/lite-xl/lite-xl) as its design blueprint — we copy
its architecture and behaviour, not its Lua code.

Status legend: `[ ]` todo · `[~]` in progress · `[x]` done · `[-]` dropped

## Product rules (non-negotiable)

- Single `.exe`, no installer, no runtime, no browser engine.
- No plugins, no network access, no launching programs (except opening
  http/https/mailto links in the user's browser, and "Show in Explorer").
- Pure-Rust dependencies where possible (memory safety covers the whole stack).
- Markdown preview never executes HTML and never fetches remote images.
- Target: idle RAM in the tens of MB, exe in the single-digit-to-low-teens MB.

## Decided v1 interaction model

- `.md` opens in preview. `Ctrl+E` toggles to the editor. Optional side-by-side
  live preview. (Typora-style inline editing: out of scope.)
- File tree on the left. Open a folder via `Ctrl+Shift+O`, CLI arg, or drag-drop.

---

## Epic 0 — Foundation (thin slice, proves the stack on Windows)

- [x] S0.1 Cargo project, MIT license, lite-xl credit (NOTICE)
- [x] S0.2 Window (winit) + software canvas (softbuffer) + text (cosmic-text)
- [x] S0.3 Headless `--screenshot out.png` mode using the same render path
- [x] S0.4 Windows CI: release build, tests, screenshot, upload exe + PNG;
       log exe size, startup time, peak memory
- [x] S0.5 Windows hardening: static CRT, panic hook → log file, no `\\?\` paths,
       CRLF / BOM / non-UTF-8 tolerant file loading
- [-] S0.6 User runs the CI exe — dropped: owner can't test (see D-002);
       CI screenshots + scripted input tests are the gate instead

## Epic 1 — Viewer (useful daily for reading)

- [x] S1.0 Test infrastructure: app-level input events (winit only translates),
       `--script` mode (events + screenshots), Windows CI prints screenshots
       as base64 so they can be inspected, EventLoopProxy wake-ups,
       time-budgeted background work, view abstraction
- [ ] S1.14 Hostile-file guards: reject UNC/`file://host` paths for images and
        links, safe link launching (no `cmd /c start`), cap long-line shaping
        and highlighting, cap markdown nesting depth, never-panic fuzz test over
        fixtures (every prefix, tiny sizes)
- [x] S1.1 Markdown parse (pulldown-cmark, GFM: tables, tasks, strikethrough)
- [x] S1.2 Preview layout: headings, paragraphs, bold/italic/code/strike, links
- [x] S1.3 Preview: lists (nested, numbered, tasks), block quotes, rules
- [x] S1.4 Preview: fenced code blocks with syntax colors
- [x] S1.5 Preview: tables
- [ ] S1.6 Preview: local images (size-limited decode, no remote fetch)
- [~] S1.7 Links: click opens http/https/mailto only; local .md opens in litedit
- [x] S1.8 Code view: syntect highlighting (fancy-regex, pure Rust), line numbers,
       lazy highlighting, scrolling
- [x] S1.9 Theme: lite-xl default dark palette for UI + syntax
- [ ] S1.10 File tree: open folder, lazy expand/collapse, icons, ignore list,
        hidden toggle, keyboard nav, resize, highlight current file, Ctrl+\
- [ ] S1.11 Open file / folder: CLI arg, Ctrl+O, Ctrl+Shift+O, drag-drop
- [ ] S1.12 File watching (notify): tree refresh, reload open file
- [ ] S1.13 Status bar; zoom (Ctrl +/-); DPI awareness

## Epic 2 — Editor

- [ ] S2.1 Text buffer (ropey), load/save preserving line endings + encoding
- [ ] S2.2 Cursor, selection (mouse + keyboard), multi-cursor (later)
- [ ] S2.3 Typing, delete, indent/outdent, auto-indent
- [ ] S2.4 Undo/redo (lite-xl doc/init.lua model)
- [ ] S2.5 Clipboard (arboard)
- [ ] S2.6 Save, Save As, dirty marker, confirm on close
- [~] S2.7 Ctrl+E preview/edit toggle (preview ↔ read-only source done)
- [ ] S2.8 IME / non-English input

## Epic 3 — Daily driver

- [ ] S3.1 Tabs
- [ ] S3.2 Find / replace (in file)
- [ ] S3.3 Side-by-side live preview
- [ ] S3.4 Tree actions: new, rename, delete→Recycle Bin, Show in Explorer
- [ ] S3.5 Find in folder
- [ ] S3.6 Recent folders / files, restore session
- [ ] S3.7 Command palette (lite-xl commandview)

## Epic 4 — Polish

- [ ] S4.1 Light theme + theme setting
- [ ] S4.2 Settings file (fonts, sizes, ignore list)
- [ ] S4.3 Smooth scrolling; dirty-rect redraw (lite-xl rencache) if needed
- [ ] S4.4 Large-file performance pass
- [ ] S4.5 App icon, version info, file associations (optional)
- [ ] S4.6 Text selection + copy in preview

---

## Progress log

Newest first. One line per meaningful step.

- 2026-11-14 — S1.1–S1.5 done: markdown preview (headings, inline styles, lists,
  tasks, quotes, GFM callouts, code blocks with highlighting, tables with
  alignment, rules, footnotes, front matter hidden, HTML stripped). Links
  classified safely (links.rs); clicking works (web → request, local .md →
  opens, #anchor → scrolls, blocked → status notice). Ctrl+E toggles source
  with scroll sync. Memory note: regex compile per grammar used (~7 MB for
  Rust); preview mode never runs the markdown grammar.
- 2026-11-14 — S1.8/S1.9 done: syntect + two-face (fancy-regex), lite-xl colours,
  budgeted incremental highlighting with checkpoints. 173k-line file: ~17k
  lines/s, 66 MB peak. Linux exe grew 4.7 → 7.5 MB (grammar set).
  Gap: no PowerShell grammar in two-face (S4.x: consider adding one).
- 2026-11-14 — S1.0 done: Input enum, View trait, per-line cached CodeView,
  `--script` mode with assertions, CI smoke script + base64 screenshots
  (`scripts/ci-shots.sh`), budgeted tick + EventLoopProxy. 22 tests.
- 2026-11-14 — Windows CI green. Windows numbers: exe 2.43 MB, startup 178 ms
  (fonts 152 ms), window working set 21.4 MB (6.7 MB private). Static CRT and
  pure-Rust checks pass. Waiting on user feedback (S0.6).
- 2026-11-14 — Thin slice works on Linux: 4.7 MB exe, ~110 ms startup, ~14 MB RSS
  in window mode. Pushing with Windows CI to get real Windows numbers.
- 2026-11-14 — Project started. Roadmap written. Working on Epic 0.
