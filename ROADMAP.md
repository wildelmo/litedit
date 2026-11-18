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

## Epic 5 — Everyday parity (owner review after 1.0)

Owner asked for tabs/compare/split "like other code editors" (VS Code is the
reference for these; lite-xl for the rest). Adversarial review findings are
fixed first (S5.0).

- [x] S5.0 Fix confirmed findings from the end-to-end adversarial review
       (remaining cosmetic items: D-026)
- [x] S5.1a Tree drag-to-move, F2/Delete in tree, unsaved dot in tab close slot
- [x] S5.1 Tabs like VS Code: shrink when crowded (to a readable minimum,
       names end in "…"), then scroll (wheel, active tab kept in view, overflow
       bar); drag to reorder; right-click menu (Close, Close Others, Close to
       the Right, Close Saved, Close All, Copy Path, Reveal in File Tree, Show
       in Explorer) asking about each unsaved tab; Ctrl+Shift+T reopens the
       last closed tab. (Double-click empty bar = new file moved to S5.4.)
- [x] S5.2 Compare: tree/tab "Select for Compare" → "Compare with Selected",
       and "Compare with Saved" (tab menu, palette) for an edited file.
       Side-by-side read-only diff tab with syntax colours: removed / added
       lines tinted, changed characters highlighted, aligned rows, one scroll,
       change markers on the right edge, F7 / Shift+F7 (Alt+F5) next/previous
       change, change count in status; recomputes when either file changes
- [x] S5.3 Split right: two editor groups side by side, each with its own tabs
       (Ctrl+Alt+Right/Left moves the tab across, Ctrl+1/2 focus a side, tab
       menu "Move to Right/Left Side", palette); one tab per file; a side
       closes when its last tab does (D-027)
- [x] S5.4 Everyday editing gaps vs lite-xl: Ctrl+N new file (named in the
       current folder; system dialog without one; also double-click on the
       empty tab bar), Shift+click extends, Ctrl+click adds/removes a caret,
       Ctrl+D next occurrence, Ctrl+Shift+L all occurrences, Esc back to one
       caret, Alt+Up/Down move lines, Ctrl+/ toggle comment (per-language
       table), right-click menu in text views, Ctrl+wheel zoom

- [x] S5.5a Workspaces (lite-xl workspace plugin + VS Code workspace files):
       every folder remembers its tabs and split; switching folders saves the
       old folder's and restores the new one's (unsaved tabs always stay);
       File: Save Workspace As… / Open Workspace… (`.litedit-workspace`,
       relative paths, network paths ignored); split restored next start
- [x] S5.5b Several folders in the tree (lite-xl "add directory"): File: Add
       Folder to Workspace… (palette, tree menu), Remove Folder from Workspace
       (tree menu on an added folder); quick open, watching, find in folder,
       session and workspace files cover every folder
- [x] S5.7 Small lite-xl/VS Code editing commands: Ctrl+Enter / Ctrl+Shift+Enter
       new line below/above, Ctrl+J join lines, Ctrl+Shift+Up/Down add caret,
       upper/lower case, trim trailing whitespace (+ `trim_trailing_whitespace`
       setting, never markdown), convert indentation, LF/CRLF, F11 full screen
- [x] S5.6 Full lite-xl command audit → docs/LITE-XL-COMPARISON.md

## Epic 6 — Reader first (owner: reading files, many files, compare, small edits, small memory)

- [x] S6.1 Hidden tabs free their rendered caches (30 viewed markdown tabs:
       272 MB → 36 MB peak on Linux; 30 code tabs 69 → 55 MB)
- [x] S6.2 Go to heading (Ctrl+Shift+G): list of the document's headings
- [x] S6.3 Click a task checkbox in the preview to tick / untick it (Ctrl+Z / Ctrl+Y
       work in the preview)
- [x] S6.4 Find in the preview (Ctrl+F stays in the preview; F3 / Shift+F3,
       Alt+C / Alt+R, count in the bar and status; Ctrl+H still goes to the source)
- [x] S6.5 Peek tab: single click in the tree reuses one temporary tab (italic
       title); double-click, Enter, editing, moving it across or "Keep Open" keeps it
- [x] S6.6 One window: a second launch opens its files in the running window
       (D-030; `--new-window`, `single_window` setting)
- [x] S6.7 "Open with" registration for markdown (explicit command with a
       confirmation; current user only; a remove command; D-006 updated)

## Epic 7 — Reader first, second batch (built by sub-agents, integrated on main)

- [x] S7.0 Fix confirmed findings from the independent review of Epic 6
- [x] S7.1 Big files: colour only as far as you've scrolled; lower memory peak
       while loading; read-only lock for code files
- [x] S7.2 Image files open in a viewer tab
- [x] S7.3 Back / forward (Alt+Left / Alt+Right); files reopen where you left off
- [x] S7.4 Compare extras: compare the two sides of a split, ignore whitespace,
       jump to a change in the file, compare with the clipboard
- [x] S7.5 Tree: multi-select, copy / paste / duplicate; file size and date in
       the status bar

## Epic 8 — Security scan findings (static scan of 2026-11-15, at dc5209f)

- [x] S8.1 CI screenshot decoder: names from the log cannot leave the output folder
- [x] S8.2 Line limit counts lone-CR line endings
- [x] S8.3 Markdown pictures: one budget per document, one spelling per picture
- [x] S8.4 Workspace restore: total size budget, one tab per file
- [x] S8.5 Markdown preview: budget for very large documents
- [x] S8.6 Links to network shares are not followed (a: gaps in the existing check; b: links, workspaces and tree entries)
- [x] S8.7 GitHub Actions pinned to commits
- [x] S8.8 One-window pipe: a launch sends its files only to a pipe owned by the same user
- [x] S8.9 One-window pipe: only the same user can connect; a silent launch is cut off after 1 s
- [x] S8.10 Folder listing: bounded work per folder
- [x] S8.11 Rename never replaces a dangling link
- [x] S8.12 Code renderer: long-line display and cache allocations are bounded
        by the 4,000-character display prefix
- [x] S8.13 Scan 72dfaf3b: bound file reads, watcher queues and workspace restores;
        guard copied paths, recycling and atomic document saves on Windows

## Epic 9 — Menus and folders you can see (owner request, 2026-11-16)

The owner found no way to close a folder, couldn't tell a folder from a
workspace, and missed a menu bar: everything was in shortcuts and the
palette.

- [x] S9.1 Close Folder (palette, File menu, tree header, main folder's
       right-click menu) and a start page: Open File / Folder / Workspace
       and the recent folders, workspaces and files, by mouse or keyboard
- [x] S9.2 Menu bar (File, Edit, View, Go, Help), drawn by us so it follows
       the theme; items replay their shortcuts; Alt+F/E/V/G/H and F10;
       `menu_bar` setting and View → Hide Menu Bar (D-040)
- [x] S9.3 Tree header: says FOLDER, FOLDERS or WORKSPACE · name; a click
       opens the folder / workspace menu (D-041)
- [x] S9.4 Help → About litedit: a tab with the version and who made the
       build (Created by / Company / Email from `assets/branding.txt`,
       embedded at build time; empty lines are not shown) (D-042)
- [x] S9.5 Hidden files are shown by default; Ctrl+Shift+H is remembered
       (`hidden_files` setting); Ctrl+P and find in folder follow it (D-043)

---

## Progress log

Newest first. One line per meaningful step.

- 2026-11-18 — Integrated the remaining findings from scan 72dfaf3b. Local
  document saves still replace the file atomically and check the identity of
  the original. If a path goes through a link or Windows cannot preserve the
  file safely, Save As to a new local file keeps the original intact.
- 2026-11-18 — S8.12 Long single-line documents no longer size the code
  renderer's two offset maps or its retained cache entry from the complete
  line. The maps and raw cache identity now follow the displayed prefix,
  first-line syntax probing is capped at the existing highlighting limit,
  and end-of-line hit testing uses the bounded source/display mapping.
  Regression assertions cover allocation bounds, ASCII and multibyte caret
  clamping, cache-prefix bounds and shebang syntax selection.
- 2026-11-16 — S9.5 Hidden files, at the owner's request (`.docs` was not
  in the tree and the switch was forgotten at every start). They are shown
  by default now, also without a settings file (headless, tests). The
  switch is saved as `hidden_files = on/off`, like the menu bar. Found
  while doing it: Ctrl+P and find in folder never listed hidden files,
  whatever the switch said; they now follow it, so what the tree shows is
  what they search (`index_files` and `SearchView::start` take the flag).
  The always-ignored names and the `ignore` setting are unchanged. One new
  app test, two extended unit tests; 9 mutation checks, all caught (one
  only after the test also covered a window without a settings file, where
  nothing else throws away Ctrl+P's file list). 448 tests pass on Windows
  (1 ignored).
- 2026-11-16 — S9.4 About, at the owner's request (the same idea as their
  Demo Studio app, without its update check: litedit stays off the network,
  D-042). Help → About litedit opens a tab with the compiled version, the
  description, and Created by / Company / Email from `assets/branding.txt`
  (`key = value`, embedded with `include_str!`, no new crate; values are cut
  to one plain line of 100 characters; without a name the author from
  `Cargo.toml` is shown). The checked-in file has the name only. Asking
  again shows the open tab; a file opened while About is the only tab takes
  its place; it is never saved in the session. 7 new tests (4 parser, 3 app).
  Six mutation checks: five caught; one was not a real mutation (a fixed
  background colour rightly passes a test about cached colours; caching the
  colour is caught). Looked at in both themes and at 330 px wide.
  447 tests pass on Windows (1 ignored).
- 2026-11-16 — Epic 9 (S9.1–S9.3), at the owner's request after trying the
  1.1 build. Close Folder remembers the folder's tabs first (reopening the
  folder brings them back), closes the saved tabs, keeps tabs with unsaved
  edits, and leaves the start page; the next launch starts without a
  folder. The start page lists Open File / Folder / Workspace and up to 8
  recent entries that still exist. The menu bar is a drawn row at the top
  with File, Edit, View, Go and Help; almost every item replays its
  shortcut, so menus and keys can't disagree. Hover switches menus, Left /
  Right walk them, Exit goes through the normal close (unsaved edits are
  asked about), and the bar does nothing under a question. The tree header
  names what is open, remembers a workspace file's name across restarts
  (only the name; the file is not read), and opens the same folder menu.
  Most older tests click at fixed positions, so the shared test app starts
  without the menu bar; four tree tests now offset their rows by the header.
  Edit-menu items move the keyboard to the open file first (the tree may
  have it), typing while a menu is open goes nowhere, and Close Folder is
  remembered even in a window started with a path (`litedit .`).
  15 new tests (`src/tests_menus.rs`, plus menu and menubar unit tests),
  13 mutation checks all caught. Windows-only parts not run here: whether
  Alt+F and F10 reach the window on a real desktop (read from winit's
  source: they should, with no beep; the new real-window CI step 9 checks
  Alt+F / F10 → Save and File → Exit). 440 tests pass on Windows (1 ignored).
- 2026-11-15 — S8.8 + S8.9 One-window pipe (sub-agent, reviewed, one change
  at review; the owner handed these back). The pipe that hands files to the
  running window now opens only for the current user (before, other accounts
  on the PC could read it and hold it). A later launch checks that the pipe
  is owned by the same user before sending any file path, so another account
  that created the name first gets nothing and the launch opens its own
  window. The window no longer waits on a stuck launch: the message must
  arrive within 1 s, and after "ok" it waits at most 1 s for the hang-up.
  At review: files are handed to the window before that wait, not after.
  If the access list cannot be built the window does not listen (D-039).
  Linux: logic tested with a fake pipe. Windows: the new calls are
  type-checked here (`cargo check --target x86_64-pc-windows-msvc`); CI's
  one-window test, now also with three launches at once, is the behavioural
  check. The hostile-account case itself has no automated test. This closes
  all eleven findings of the scan.
- 2026-11-15 — S8.1 follow-up, at the owner's request: screenshot names in
  `scripts/ci-shots.sh` may now contain dots and end in `.png` in any case
  (`markdown-1.5.png`, `x.PNG`); Windows device names (`con.png`,
  `nul.v2.png`) are refused. Still one plain file name: no separators, no
  colon, must start with a letter or digit. Checked with a stub `gh` over
  fake logs (19 good names written, 18 hostile ones refused).
- 2026-11-15 — S8.5 Markdown preview size budget (sub-agent branch, reviewed
  and merged). Files over 1 MiB, 100,000 lines or 30,000 blocks open in
  source view with "This file is too large to preview; showing the source".
  Bytes and lines are checked before parsing, the parser stops at the first
  block over the budget, and layout has a backstop. The same rule holds for
  edits, undo, reload, restore and headless runs; Ctrl+E previews again once
  the file is small enough. Measured on Linux (release): one million
  headings went from 8 s / 3.4 GB to 0.2 s in source; the worst file under
  the limits takes about 1 s and under 500 MB (D-038, with the figures and
  the choice of 1 MiB to review). Windows timings are not measured. This
  closes the scan's findings except S8.8 and S8.9, which are with the owner.
- 2026-11-15 — S8.4 + S8.6b (sub-agent branch, reviewed and merged, one
  correction). Restoring a session or a workspace opens each file once,
  however its path is spelled, and at most 512 MB of files: the active tab
  first, the rest in order; files left out are named in one message and stay
  in the saved workspace (D-037). Opening `x/../a.txt` (or `A.TXT` on
  Windows) focuses the tab that is already open. Links in markdown, tabs and
  folders from a workspace file, files sent by a later launch and tree
  entries that are links are checked without following any link and refused
  if they lead to a network share or device path; the reader's own opens
  (dialog, command line, drag and drop, saved session) are unchanged (D-036).
  Correction at review: the check judged the whole path's spelling, which
  would have blocked relative links and workspace tabs for a document the
  reader opened on a share; it now judges links only. Linux tests only: real
  Windows junctions and the absence of network traffic are not verified.
  S8.8 and S8.9 (the one-window pipe) are with the owner.
- 2026-11-15 — S8.3 Markdown pictures have a budget per document (sub-agent
  branch, reviewed and merged). Different spellings of one file (`./a.png`,
  `x/../a.png`, `A.PNG` on Windows) are one picture, decoded once. A document
  shows at most 1000 pictures and keeps at most 512 MB of decoded pixels;
  pictures past either limit show a short note and the decoder stops instead
  of working through the rest (D-035). Ordinary documents are unchanged.
  Checked on Linux with small injected limits; the Windows-only case folding
  is not run here.
- 2026-11-15 — Security scan triage, fixes S8.2, S8.6a, S8.7, S8.10 and
  S8.11. A file made only of lone CR bytes got past the line limit (only
  LF was counted, but the editor and Compare also split on CR) and could
  become hundreds of millions of lines; `doc::count_line_breaks` now counts
  LF, CRLF (as one) and lone CR on the decoded text, and Open and both
  Compare sides use it. Rename no longer replaces a dangling link (it counts
  as something there). Link chains longer than 16 hops, or loops, now count
  as unsafe instead of safe; image loading checks the folders on the way
  before looking at the file; the quick-open and folder-search index skips
  links to network or device paths. Folder listings read at most 50,000
  entries of one folder (the index no more than the overall cap still
  allows) and sort with one lowercase name per entry; the tree note says
  "more than N items" when reading stopped early. The GitHub Actions in the
  workflow are pinned to the commits their tags point at today (checked with
  `git ls-remote`). Checked by new tests for each point (including a small
  injected read limit instead of 50,000 files), `cargo fmt`, clippy with
  warnings as errors and the full test suite on Linux; the workflow itself
  has not been run on GitHub.
- 2026-11-15 — Security scan triage, fix 1 of the findings (the one rated
  high): `scripts/ci-shots.sh` took the file name of each screenshot from
  the CI log, so a log line could make it write anywhere (`../`, absolute
  paths). Names must now be plain `.png` names (letters, digits, `-`, `_`),
  the destination must stay inside the output folder (a link there is
  refused), the base64 is strict and at most 16 MiB per picture. Checked
  with a stub `gh` and fake logs; not against a real run. The other ten
  findings are triaged but not fixed yet.
- 2026-11-15 — Epic 7 wrap-up: Windows smoke script now drives the picture
  tab, read-only lock, Back / Forward, Compare with Clipboard and tree
  Duplicate in the release exe; a 3000-entry tree delete is timed through
  the real flow (ask 46 ms, delete 53 ms, debug build, stand-in bin); the
  six S7.0 regression tests were run against the pre-fix code (all fail
  there); kept colours measured at ~90 KB per hidden 2000-line tab;
  "Remove from Open with" reports registry errors. Windows numbers after
  Epic 7: exe 6.89 MB, one window 21.6 MB private. 394 tests.
- 2026-11-15 — Epic 7 independent review, part 2 (tree file operations,
  image viewer): no way found to lose edits or overwrite a file; image
  decoding survived 12,000 damaged files. Fixed: a folder with a workspace
  folder somewhere inside it could be deleted / moved / renamed (now
  refused, for one entry or many); file details in the status bar were
  read through links (`symlink_metadata` now); a quit that was called off
  silently stopped a running copy (the copy stops when the window really
  closes: `take_requests`); selecting thousands of entries froze Delete
  (`without_nested` was quadratic); open tabs are matched to tree paths
  ignoring case on Windows (`fsops::is_under`, `rebase`). 393 tests.
- 2026-11-15 — Epic 7 independent review, part 1 (editing / navigation /
  compare): no data loss found (4,000-file open-and-save fuzz matched the
  old decoder exactly; read-only lock and Back / Forward held). Fixed: a
  comparison could keep rows of the old text after one file changed while
  the other was briefly unreadable, then panic when drawn (`recompute` now
  replaces a side only together with a fresh diff; drawing no longer
  indexes); Compare with Clipboard on text that is mostly line breaks used
  hundreds of MB before being refused (line breaks are counted first).
  Recorded D-034 (hidden tabs keep colours of the first 2000 lines).
- 2026-11-15 — S7.1 Big files + read-only lock (sub-agent branch, reviewed
  and merged): syntax colouring runs only as far as the view (or a jump
  target) needs, in compact span blocks; hidden code / compare tabs drop
  colours past 2000 lines. Load path: valid UTF-8 becomes the text without
  a copy and without the re-encode check (exact by construction; UTF-16 and
  invalid files still round-trip), lines copied at exact size. Measured
  (Linux release, 100k lines / 8 MB): just opened 59.6 → 36.7 MB peak,
  after Ctrl+End 60.6 → 43.7 MB. "View: Toggle Read-Only": every editing
  path is refused at `EditDoc` and at the view (typing, paste, cut,
  replace, multi-cursor, undo / redo, line commands, checkbox clicks);
  status shows "Read-only". Fixed on the way: moving lines in a locked
  file panicked. 386 tests.
- 2026-11-15 — S7.5 Tree multi-select + copy (sub-agent branch, reviewed and
  merged): Ctrl / Shift+click, Shift+Up/Down, Ctrl+A; Delete and drag-move
  act on the selection with one question (count + first names), never on a
  workspace folder, a folder and its contents count once, any open file
  with unsaved edits refuses the whole batch (checked when asking and
  again when answered), name clashes are skipped and reported. Copy /
  Paste / Duplicate (Ctrl+C / V / D with the tree focused, tree menu):
  `fsops::CopyJob` — `create_new` only ("name - Copy.ext"), links skipped,
  never into itself, caps on entries / bytes / depth 64, runs in slices
  with progress in the status bar, removes a half-written file on failure
  or exit. Status bar shows the selected file's size and age. Code in
  `src/app/treeops.rs`. 357 tests.
- 2026-11-15 — S7.2 Image viewer (sub-agent branch, reviewed and merged):
  `src/view/image.rs` — png / jpg / gif (first frame) / webp / bmp open in a
  viewer tab: fit (never enlarges) or 100% (`0` / `1`, double-click), pan by
  drag / wheel / arrows, checkerboard under transparency, status shows
  pixels, file size, zoom. Decoding in the background under the existing
  limits (40 MB file, 25 MP, 128 MB alloc); a row-at-a-time area-averaging
  downscaler replaces the `image` crate's filter (lower peak for markdown
  images too); pixels freed in `on_hide`; reloads on disk change; never
  "modified", save commands skip it (`View::can_save`). 323 tests.
- 2026-11-15 — S7.4 Compare extras (sub-agent branch, reviewed and merged):
  Compare Left ↔ Right of a split (palette + tab menu), Alt+W ignores
  whitespace (text shown is still the real text), Enter / double-click
  opens the file at that line (Shift+Enter: left side), Compare with
  Clipboard (clipboard = old side). Unsaved text is what gets compared; a
  comparison never starts under an open question. 299 tests.
- 2026-11-15 — S7.3 Back / Forward + remembered positions (sub-agent branch,
  reviewed and merged): `src/nav.rs` (100 places each way, adjacent
  duplicates collapsed, vanished files skipped), recorded on tab switches,
  opens, go to line / heading, `#heading` links, search results; nothing
  moves under an open question. `ViewPos` (lines + fractions, never pixels)
  via `View::position` / `restore_position`; in-session table capped at 500
  files; optional `pos` lines in session / workspace files (old files load,
  old versions ignore them, values capped). Also fixed: Ctrl+E in a preview
  whose editor was never drawn scrolled the source too far. 271 tests.
- 2026-11-15 — S7.0 Epic 6 review fixes (no lost edits or panics were found;
  eight confirmed defects): a second launch no longer reports "sent" for a
  path the window would drop (the sender checks the round trip, the listener
  only acknowledges a non-empty parsed list); busy pipe is retried (1 s);
  the client limits the server to identification-level impersonation; Space
  in the tree never moves the keyboard into an open file; no stale peek flag
  on the start-page slot; markdown tabs never shown decode no images and a
  hidden tab stops its image worker; Go to Heading reaches headings inside
  lists / quotes; find-in-preview highlight cost is per visible line; files
  from another launch wait for an open palette / menu / question and cancel
  a quit in progress; `%` in the exe path refuses "Open with". 6 regression
  tests (247 total).
- 2026-11-15 — S6.7 Open with: assoc.rs (pure list of registry entries with
  tests; Windows register / unregister under HKCU\Software\Classes), palette
  commands with a confirmation showing the exe path, result message tells the
  user the one Explorer step Windows requires. CI checks the registry.
- 2026-11-15 — S6.6 one window: single.rs (pure message encode/parse with
  fuzz test; Windows named-pipe client with a 1.5 s timeout and listener
  thread with no unwraps), UserEvent::OpenPaths, App::open_from_other_launch
  queues files while a question shows. Windows target type-checked from
  Linux (`cargo check --target x86_64-pc-windows-msvc`). CI: existing window
  launches use --new-window; new real-window test for the hand-over, a folder
  and --new-window.
- 2026-11-15 — S6.5 peek tab: Request::PeekPath from tree single click /
  Space, TabSlot.peek, App::open_peek (new tab added before the old peek is
  removed; never replaces a dirty tab; blocked while a question shows; not
  added to recents or the closed list until kept), italic title. Tab fuzz now
  clicks the tree and types into the editor (mutation-checked).
  Windows CI for S6.1: 12 viewed markdown tabs = 21.2 MB private.
- 2026-11-15 — S6.4 find in preview: PreviewFind (Finder + match list of
  element ranges, recomputed per pattern and per layout, time-budgeted and
  capped), highlights via LayoutRun::highlight, current match revealed;
  truncated code blocks say so in the bar. Preview selection now also
  survives a hidden tab's layout being dropped (last_shape).
- 2026-11-15 — S6.3 task checkboxes: parser keeps each marker's offset;
  click maps it to the buffer by line + character count and edits only if the
  bytes are `[ ]`/`[x]` (one undo step); undo/redo forwarded in preview mode.
- 2026-11-15 — S6.2 go to heading: parse::headings, View::headings /
  goto_heading, palette list indented by level; preview scroll resolved at
  the next draw (works without a layout); caret moves in source / split.
- 2026-11-15 — S6.1: View::on_hide; App::trim_hidden_tabs sweeps before every
  frame (covers tab switch, split moves, closes, workspace restore; the tab
  shown on the other side of a split is kept). Markdown drops layout, scaled
  and decoded images (sizes kept → no jump; decoded again in the background
  when shown, via View::has_background_work). Test with a cache hook,
  mutation-checked; Windows CI gate: 12 viewed markdown tabs under 80 MB.
- 2026-11-14 — Independent review of c8b735f..HEAD; fixed: (1) split view: a
  click on the other side that raised that tab's disk question could land on
  another tab and the answer went to it (now bound to its tab, click stops);
  (2) line commands with two carets on one line hit extra lines (one
  selection per line block); (3) save-time check treated "can't read the
  file" (now binary / too many lines) as unchanged and overwrote it (now
  Differs → asks); (4) link chains and linked folders mid-path to network
  paths (every hop checked; local junctions allowed). Also: size+mtime
  shortcut only on sub-second timestamps and taken before reading; replace
  all says when the time limit stopped it; CR-only files split in compare;
  a tree drag never replaces an open question; hiding the tree mid-drag ends
  the drag. Regression tests, the split one mutation-checked.
- 2026-11-14 — S5.7 editing commands (EditDoc replace_all / map_lines keep
  each line's ending; one undo step each), View::run_command for palette
  commands, F11, trim-on-save setting. Esc now keeps the first caret (VS Code).
- 2026-11-14 — S5.5b several workspace folders: Tree.roots, root_of,
  add_root/remove_root; quick open indexes all folders (labels prefixed with
  the folder name); session `folder` lines; workspace files keep all folders.
- 2026-11-14 — S5.5a workspaces: workspace.rs (format, relative paths,
  bounds, network paths skipped, per-folder store in %APPDATA%), App
  apply_workspace / open_folder / save_workspace_as / persist_workspace;
  session keeps tab sides. Tests: per-folder memory with split (mutation-
  checked), unsaved tabs kept, workspace file save/reopen/moved/hostile.
- 2026-11-14 — Fixes from the final review of Epic 5: the unfocused split
  side now ticks (highlighting, images, search results; pixel test,
  mutation-checked); Ctrl+/ token table matches exact grammar names (Plain
  Text no longer gets `%`); uncommenting lines shorter than both tokens can't
  panic; Alt+Down with a selection ending at column 0 of the last line kept a
  caret past the end (found by the extended editing fuzz, now with Ctrl+D /
  Ctrl+Shift+L / Alt+Up/Down / Ctrl+/ / Esc / Ctrl+click / Shift+click); a
  new quit forgets an interrupted quit's "Don't save" answers; Close Others /
  to the Right / Saved / All act on one side of a split; Ctrl+D scrolls to the
  new selection; disk checks use size + mtime first (D-029); D-028 recorded.
- 2026-11-14 — S5.3 split view: flat tab list with a group tag per tab,
  focused group's view in App.view (all save/close/quit paths unchanged), the
  other side drawn from its slot; per-side tab bars, scroll and reveal; clicks
  in the other side focus it, the wheel scrolls it in place. Tests: split /
  focus / merge flow, quit with unsaved tabs on both sides, randomized
  drag/close/reopen/move-between-sides run (now asserts >200 steps run;
  mutation-checked).
- 2026-11-14 — S5.4 everyday editing: modifier state reaches the app
  (Input::Modifiers → Ctx.mods; script `mods`), multi-caret commands, move
  lines keeping each line's ending, comment toggle (line and block styles),
  Ctrl+N, editor right-click menu (drawn above the status bar), Ctrl+wheel.
  Windows CI green for all of S5.0 (200 tests incl. the Windows font test).
- 2026-11-14 — S5.0 part 4: whole-document searches stop after 300 ms (3 s
  for replace all) and say "pattern too slow" (two patterns that froze for
  minutes now take ~0.3 s); backtrack limit 50k; files over 4M lines refused;
  images over 25 MP refused from the header; code lines starting with RTL
  text drawn in view (row shift bug); wrapped tab-indented lines and lines
  past 4000 chars wrap as drawn; quick open / find in folder see past 5000
  files per folder; binary files aren't opened as text; Go to line with a
  huge number goes to the end; Save As onto a file open in another (clean)
  tab closes it; symlink chains followed on save; long names save; "1 result".
- 2026-11-14 — S5.0 part 3 (security): markdown paths using NT/device forms
  (`\\??\\`, `\\\\?\\`, `\\\\.\\`, `/??/`) or alternate data streams are blocked
  (they could reach UNC shares and bypass the network-path rule); symlinked
  images are refused; the tree never stats a link whose target is a network
  or device path; COM¹-³/LPT¹-³ and trailing-dot device names are reserved;
  LRM/RLM/ALM, zero-width space/word joiner/ZWNBSP, C1 controls and tag
  characters are shown as � (D-010 extended; ZWJ/ZWNJ kept).
- 2026-11-14 — S5.0 part 2: delete/duplicate line respect a selection ending
  at column 0 (Ctrl+L); Tab on any selection indents lines (never replaces
  text); undo history overflow can't fake the saved state; case-only rename
  never overwrites a different file (exact-name check, Unicode case); zoom
  to the limit with small fonts is a regression test (old code panicked).
- 2026-11-14 — S5.0 part 1 (data safety, from the adversarial review): a
  question bar answers for the tab it asked about (owner recorded; drops and
  link opens wait); "changed on disk" compares disk with what we last
  read/wrote (hash), so our own save or a touch is no conflict, and its safe
  answer (Keep my version) is the default, plus Compare; saving re-checks the
  disk and asks Compare / Save mine over it / Cancel; any file that won't
  encode back to its exact bytes (BOM + invalid UTF-8, broken UTF-16) asks
  before saving, Save As included; the theme toggle leaves non-UTF-8
  settings files alone. Tests mutation-checked.
- 2026-11-14 — S5.2 compare view (diff.rs model on the `similar` crate with a
  1.5 s deadline and 20 MB / 500k-line caps; view/diff.rs). Tests: diff rows
  cover every line once, inline highlights on char boundaries, 60k-line
  unrelated files finish in budget, tree-menu flow, F7 wrap-around, refresh on
  disk change, compare-with-saved, huge/missing/binary files, theme switch ==
  fresh (mutation-checked). CI smoke script takes a compare screenshot.
- 2026-11-14 — S5.1 tabs: scrolling tab bar (min width, ellipsis labels,
  wheel, active kept in view, overflow bar), drag to reorder, tab menu with
  Close Others/Right/Saved/All going through per-tab save prompts (Don't save
  closes that tab, Cancel stops), Ctrl+Shift+T. Randomized reorder/close test
  (mutation-checked) asserts unsaved tabs are never closed.
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
