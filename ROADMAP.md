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
- [x] S2.5 Clipboard (arboard): copy, cut, paste (line-wise when no selection)
- [x] S2.6 Save / Save As (atomic, Windows edge cases), dirty marker, nag bar,
       single discard gate, external-change prompt for dirty docs
- [x] S2.7 Ctrl+E: markdown preview ↔ editable source (preview from buffer)
- [x] S2.8 IME (commit → text, candidate window at caret); unverifiable here,
       tracked in DECISIONS as open

## Epic 3 — Daily driver

Design: a reusable single-line input bar (`ui/input_bar`, lite-xl
CommandView) powers find/replace, go to line, rename/new file, quick open
and the command palette. Tabs move the discard gate to per-tab close; quit
asks for each dirty tab.

- [x] S3.0 Input bar widget (text field: caret, selection, clipboard, history)
- [x] S3.1 Tabs: tab bar, click/middle-click, Ctrl+Tab / Ctrl+PgUp/PgDn,
       Ctrl+W (guarded), dirty dot, reopen activates existing tab, quit asks
       per dirty tab, watcher covers all open files
- [x] S3.2 Find / replace in file: Ctrl+F (prefill from selection), F3 /
       Shift+F3, all matches highlighted, Ctrl+H replace one / all (one undo
       step), case toggle, regex toggle (fancy-regex, already a dependency)
- [x] S3.3 Side-by-side live preview for markdown (Ctrl+Shift+V), debounced
       re-parse, scroll follows the caret
- [x] S3.4 Command palette (Ctrl+Shift+P), quick open file (Ctrl+P, fuzzy),
       go to line (Ctrl+G)
- [x] S3.5 Tree actions (right-click menu): new file, new folder, rename,
       delete → Recycle Bin, Show in Explorer
- [x] S3.6 Find in folder (Ctrl+Shift+F): background search, results list,
       click opens at line
- [x] S3.7 Recent folders/files, restore last session when started without
       arguments (%APPDATA%\litedit)

## Epic 4 — Polish

- [x] S4.1 Light theme + theme setting (palette command; title bar follows)
- [x] S4.2 Settings file (theme, fonts, sizes, wrap, ignore list; D-024)
- [-] S4.3 Smooth scrolling — dropped (D-023); dirty-rect redraw not needed (bench: frames well under 16 ms)
- [x] S4.4 Large-file performance pass (litedit --bench; numbers in every Windows CI summary)
- [x] S4.5 App icon + version info (file associations: not done, D-006)
- [x] S4.6 Text selection + copy in preview
- [x] S4.7 Soft wrap in the editor (lite-xl linewrapping), on by default for
       markdown source and in side-by-side mode

---

## Progress log

Newest first. One line per meaningful step.

- 2026-11-14 — Tree: drag a file/folder onto a folder to move it (drop target
  outlined; asks first; never overwrites; refuses moves into itself and files
  with unsaved edits; open tabs follow). F2 renames and Delete deletes the
  selected tree entry. Unsaved tabs show a dot in place of the close button.
- 2026-11-14 — v1.0.0: author Joe Chun (LICENSE, Cargo.toml, exe version
  info), CHANGELOG.md, THIRD-PARTY-NOTICES.txt (generated by
  scripts/third-party-notices.py), tag-triggered release job that attaches the
  tested exe and licence files to a GitHub Release.
- 2026-11-14 — Review fixes: tab and palette widths grow with ui_font_size
  (renders checked at 8 and 40); missing font named in the status bar; live
  font-size change == fresh render test; real-window CI test for Ctrl+, and
  the theme toggle; Windows test for a non-default font family.
- 2026-11-14 — S4.1 + S4.2 done: colours are a runtime Palette (refactor
  committed alone; dark render hashes byte-identical). Light palette
  (GitHub-like); "View: Toggle Light / Dark Theme" switches every tab and
  rewrites only the `theme` line of settings.txt. `%APPDATA%\litedit\settings.txt`
  (Ctrl+, opens it, created from a commented template): theme, three font
  sizes, ui/code font, wrap_code, ignore list; saving it applies it; bad lines
  reported in the status bar. Headless modes never read it (`--theme` and
  script `theme` for light shots). Tests: switched render == fresh render
  (incl. inactive tab, split view) and back to dark == baseline; mutation-checked.
- 2026-11-14 — S4.5 done: app icon (assets/, cyan→pink rounded square with a page)
  embedded with version info via build.rs/winresource on Windows; window and
  taskbar icon set at runtime; CI checks icon + ProductName in the exe.
- 2026-11-14 — S4.6 done: select text in the markdown preview (drag, double-click
  a block, Ctrl+A) and copy it (Ctrl+C); list bullets excluded; selection survives
  relayouts that only change sizes. Windows bench: keystroke 5.5 ms avg (100k
  lines), 1.4 ms wrapped markdown, 13 ms with find bar; open 100k lines 52 ms;
  Ctrl+P index 20 ms; find in folder 881 ms / 25k files.
- 2026-11-14 — S4.4 done: `--bench` (open 100k-line file, keystroke→frame plain /
  with find bar / wrapped markdown, Ctrl+P index and find in folder over 25k
  files) runs in Windows CI. Linux: open 52 ms, keystroke avg 4.3 ms, find-bar
  keystroke 12→7 ms after deferring the match recount to idle time; quick open
  says when the 20k-file cap is hit. Smooth scrolling dropped (D-023).
- 2026-11-14 — S4.7 done: soft wrap. view/wrap.rs: word wrap by display columns
  (no shaping; 170k lines mapped fast) + Rows line↔row mapping (identity when off).
  Render, hit-testing, caret, selections, find highlights, scroll, go to line and
  split sync all use it; visual Up/Down/PgUp/PgDn; Alt+Z; on by default for
  markdown (D-022); fuzzes run with wrap at narrow widths. Fixed: row mapping now
  valid before the first frame. Windows CI (hardening): Recycle Bin verified by
  item count; all real-window tests PASS; exe 6.39 MB.
- 2026-11-14 — Epic 3 hardening (advisor review): folder search made linear
  (one-pass lowercase map; per-line cancel; huge lines skipped; timed test);
  starting with a file no longer overwrites the saved workspace (only recents and
  preferences are written); cancelling a quit midway keeps every edit; find count
  cached; whole-app fuzz now covers Epic 3 features, prompts, resizes to 1×1;
  Recycle Bin verified on Windows by item count. Epic 3 CI: exe 6.39 MB, 10 tabs
  32.3 MB WS / 14.4 MB private, all real-window tests PASS.
- 2026-11-14 — Epic 3 complete. S3.7: session restore (folder, tabs, active tab,
  tree width/visibility, zoom) when started without arguments; recent folders and
  files (Ctrl+R); stored in %APPDATA%\litedit\session.txt (tolerant text format,
  atomic save).
- 2026-11-14 — S3.6 done: find in folder (Ctrl+Shift+F or tree menu): background
  search with tree rules, binary/huge/cache files skipped, 5000-result cap,
  results tab with highlighted matches, Enter/click opens at the line, cancelled
  when the tab closes.
- 2026-11-14 — S3.5 done: tree right-click menu (new file / folder, rename, delete
  to Recycle Bin with Windows' own warning when it would be permanent, Show in
  Explorer, copy path); names validated as one Windows-legal component; never
  overwrites; dirty open files block rename/delete; clean tabs follow renames.
- 2026-11-14 — S3.4 done: command palette (Ctrl+Shift+P; commands replay their
  shortcuts), quick open (Ctrl+P, lite-xl fuzzy match, tree rules + CACHEDIR.TAG
  skip, 20k cap, rebuilt after changes), go to line (Ctrl+G). Windows CI (find/
  replace commit): exe 6.29 MB, 10 tabs 31.5 MB WS / 13.6 MB private, 4/4 real-
  window tests PASS.
- 2026-11-14 — S3.3 done: side-by-side markdown (Ctrl+Shift+V): editor left, live
  preview right, re-parse debounced (180 ms) for large docs, preview follows the
  caret, wheel/links work on the preview half. Added S4.7 soft wrap.
- 2026-11-14 — S3.2 done: find/replace bar (Ctrl+F / Ctrl+H, incremental, F3,
  Enter/Shift+Enter, Tab between fields, Enter replaces one, Ctrl+Enter all as one
  undo step, Alt+C case, Alt+R regex with $1 groups, all matches highlighted,
  count). fancy-regex with backtrack limit; empty matches skipped. Fixed CI: smoke
  script still expected the pre-tabs prompt; `cargo test` now runs the smoke
  script so this cannot recur.
- 2026-11-14 — S3.0/S3.1 done: input bar widget (reuses EditDoc); tabs (tab bar with
  hover ×, click/middle-click, Ctrl+Tab/Ctrl+PgUp/PgDn/Alt+1..9, Ctrl+W asks when
  dirty, quit asks per dirty tab, opening never discards, background tabs reload
  or queue a disk prompt, multiple CLI paths). Real-window CI (4 tests) now
  gating after two green runs; CI reports memory with 10 tabs.
- 2026-11-14 — Hardening (advisor review): save never deletes the only good copy
  (ReplaceFileW partial failures, verified in-place overwrite); app-level fuzz
  (random keys/typing/clicks/drags through App, render each step, grapheme
  invariants, undo-all byte-exact) → fixed word moves / double-click splitting
  clusters; lines keep their colours while re-highlighting (no flash); undoing
  back to the saved text clears the modified marker; real-window CI tests for
  Alt+F4 (clean/dirty) and the Ctrl+O dialog. AltGr open item D-019.
- 2026-11-14 — Epic 2 complete. S2.7: markdown owns the editor; Ctrl+E edits the
  source and the preview re-parses from it; save/dirty/prompts work for .md;
  link clicks on a dirty doc ask first. S2.8: IME commit → text, candidate
  window at the caret (inline preedit open: D-018). Windows CI: real-window
  SendKeys typing + Ctrl+S PASS; held-open/locked/attribute save tests PASS.
- 2026-11-14 — S2.6 done: atomic save (temp + ReplaceFileW keeping ACLs/attributes,
  overwrite-in-place fallback D-017, read-only refused, symlinks followed),
  Ctrl+S / Ctrl+Shift+S, dirty marker, in-window nag bar (D-013), one discard
  gate (open/drop/tree/link/dialogs/close), external change → prompt when dirty,
  deleted-on-disk keeps text; lossy files need "Save anyway". Script `expect
  file/nag`, real-window SendKeys CI step (report-only).
- 2026-11-14 — S2.5 done: clipboard trait (arboard on Windows, memory in scripts/tests),
  Ctrl+C/X/V + Ctrl/Shift+Insert, Shift+Delete, lite-xl line-mode copy/paste.
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
