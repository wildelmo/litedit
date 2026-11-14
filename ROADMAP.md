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
- [x] S1.14 Hostile-file guards: reject UNC/`file://host` paths for images and
        links, safe link launching (no `cmd /c start`), cap long-line shaping
        and highlighting, cap markdown nesting depth, never-panic fuzz test over
        fixtures (every prefix, tiny sizes)
- [x] S1.1 Markdown parse (pulldown-cmark, GFM: tables, tasks, strikethrough)
- [x] S1.2 Preview layout: headings, paragraphs, bold/italic/code/strike, links
- [x] S1.3 Preview: lists (nested, numbered, tasks), block quotes, rules
- [x] S1.4 Preview: fenced code blocks with syntax colors
- [x] S1.5 Preview: tables
- [x] S1.6 Preview: local images (size-limited decode, no remote fetch)
- [x] S1.7 Links: click opens http/https/mailto only; local .md opens in litedit
- [x] S1.8 Code view: syntect highlighting (fancy-regex, pure Rust), line numbers,
       lazy highlighting, scrolling
- [x] S1.9 Theme: lite-xl default dark palette for UI + syntax
- [x] S1.10 File tree: open folder, lazy expand/collapse, icons, ignore list,
        hidden toggle, keyboard nav, resize, highlight current file, Ctrl+\
- [x] S1.11 Open file / folder: CLI arg, Ctrl+O, Ctrl+Shift+O, drag-drop
- [x] S1.12 File watching (notify): tree refresh, reload open file
- [x] S1.13 Status bar; zoom (Ctrl +/-); DPI awareness

## Epic 2 — Editor

Design (reviewed with advisor): `editor/buffer.rs` (lines + per-line EOLs),
`editor/doc.rs` (buffer, selections, undo, change ids, path, encoding),
CodeView grows into the editor; MarkdownView owns the editor and re-parses
the preview from its text. Prompts use an in-app nag bar (D-013).

Acceptance criteria that protect the user's text:
- **Byte-exact round trip:** open → save with no edits is byte-identical for
  every fixture (CRLF, LF, mixed, BOM, UTF-16, no final newline, empty).
  Lossy-decoded files need a second Ctrl+S (warned in the status bar).
- **One discard gate:** every path that replaces or closes a dirty document
  (open, drop, tree click, link click, dialogs, window close, external
  change) goes through one function that raises the nag bar. Script-tested.
- **Model-based fuzz:** random insert/delete/undo/redo vs a String model,
  undo-all restores the original, render after each step. Caret moves by
  grapheme cluster.
- **Windows save:** atomic temp + replace (ReplaceFileW keeps ACLs and
  attributes); read-only target → error; file held open → fallback/clear
  error; symlink → write the target. Failure keeps the buffer dirty and
  removes the temp file. Own saves don't trigger "changed on disk".
  Script `expect file` asserts bytes.
- **Key translation is a pure, tested function:** AltGr (Ctrl+Alt) produces
  text, not shortcuts. One real-window SendKeys CI step (report-only).
- Reserved Windows device names (CON, NUL, COM1…) rejected everywhere.

- [x] S2.1 Buffer: lines + per-line EOLs, positions, insert/remove, exact
       load/serialize for all encodings, undo/redo with merge; fuzz tests
- [x] S2.2 Cursor + selection (keyboard, mouse, word/line), grapheme-aware;
       one col↔x mapping shared by caret drawing and hit-testing
- [x] S2.3 Typing, Backspace/Delete (word), Enter auto-indent, Tab/Shift+Tab;
       pure key-translation function with AltGr tests
- [x] S2.4 Undo/redo wiring (Ctrl+Z / Ctrl+Y / Ctrl+Shift+Z)
- [ ] S2.5 Clipboard (arboard): copy, cut, paste (line-wise when no selection)
- [ ] S2.6 Save / Save As (atomic, Windows edge cases), dirty marker, nag bar,
       single discard gate, external-change prompt for dirty docs
- [ ] S2.7 Ctrl+E: markdown preview ↔ editable source (preview from buffer)
- [ ] S2.8 IME (commit → text, candidate window at caret); unverifiable here,
       tracked in DECISIONS as open

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

- 2026-11-14 — S2.2–S2.4 done: editor/doc.rs (commands on all selections, grapheme
  caret moves, lite-xl word rules, smart Home, auto-indent, indent detection,
  Tab/Shift+Tab, word delete, duplicate/delete line, undo/redo); CodeView is
  now the editor (content-keyed line cache, shared col↔x mapping, selection,
  caret, mouse click/drag/double/triple, gutter line select); translate_key
  pure fn with AltGr tests. Markdown source stays read-only until S2.7.
- 2026-11-14 — S2.1 done: buffer with per-line EOLs (byte-exact round trip for all
  fixtures/encodings), ops + position transforms, undo/redo with typing merge,
  model-based fuzz (40 seeds × 300 ops) incl. undo-all.
- 2026-11-14 — Epic 1 complete. S1.10–S1.13: file tree (lazy, sorted, ignore
  list, hidden toggle Ctrl+Shift+H, symlinks never expanded, keyboard nav,
  draggable divider, Ctrl+\ / Ctrl+Shift+E), native open dialogs (rfd, Windows
  only), folder watching (notify, reads filtered, 150 ms batches), status bar
  + zoom. Windows (prev. commit): 5.9 MB exe, 23.6 MB working set, 8.5 MB private.
- 2026-11-14 — S1.6/S1.7/S1.14 done: local images (background decode, size and
  allocation limits, downscaled storage); links open via ShellExecuteW only
  after re-validation; hostile-file test found a cosmic-text assertion crash
  (bidi paragraph separators) → display sanitization (D-010), and a 200 ms/frame
  slowdown on huge code blocks → chunked layout + preview limits (D-011).
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
