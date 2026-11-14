# Decisions log

Decisions made while the owner is away. Each has a recommendation and a
reason. Status: **decided** (implemented, review later) or **open** (needs the
owner). Newest first within each section.

## Open — please review

_None yet._

## Decided — review when convenient

### D-012 Opening a single file shows its folder in the tree
When you open a file and no folder is open yet (double-click a .md in
Explorer, drag a file in), the tree shows that file's folder with the file
selected — handy for notes folders. Open a different folder any time with
Ctrl+Shift+O. *Change if you prefer the tree to stay closed.*

### D-011 Preview size limits
For speed and safety the preview shows at most 3000 lines per code block,
30k characters per paragraph and 2000 rows × 64 columns per table, with a
note pointing to Ctrl+E (source view shows everything). Real documents
are far below these limits; generated or hostile files stay responsive.

### D-010 Control and bidi characters are shown, never obeyed
Control characters display as symbols (␀, ␛ …). Unicode bidi override /
embedding characters and paragraph separators display as �. This prevents
"Trojan Source" tricks (code that reads differently from how it runs) and
works around a crash in the text library. Display only — files are never
changed.

### D-009 SVG images are not rendered
Rendering SVG needs a large vector library (resvg, ~1–2 MB) and SVG is a
complex attack surface. PNG, JPEG, GIF, WebP and BMP are supported; SVGs
show a placeholder. Revisit if you use SVG diagrams a lot.

### D-008 Text is stored as a list of lines (`Vec<String>`), like lite-xl
lite-xl's `Doc` is a table of lines and its editing model (cursors as
line/column, per-line highlighting, undo records) is built on that. Copying the
model is simpler and fast for normal files (tens of thousands of lines). A rope
(`ropey`) wins only for huge files with large multi-line edits; the buffer is
kept behind one type so it can be swapped if S4.4 shows a need.

### D-007 Default theme is dark (lite-xl default palette)
lite-xl's default dark palette is the blueprint and looks good. A light theme
is planned in Epic 4 (S4.1). *Change if you prefer light by default.*

### D-006 No automatic file associations
litedit will not register itself as the `.md` handler. Doing it silently is
the kind of system change users dislike; it can be added as an explicit,
optional action later (S4.5). You can still right-click → Open with → litedit.

### D-005 Remote images are never loaded
Markdown images with `http(s)://` URLs show a placeholder with the alt text.
Fetching them would make the editor touch the network (tracking pixels, IP
leaks) — against the "no network" rule.

### D-004 Raw HTML in markdown is not rendered
`<br>` becomes a line break, HTML comments are hidden, other tags are stripped
and their text kept. Rendering HTML properly would need a browser engine.

### D-003 Software rendering, not GPU
Same approach as lite-xl. Measured on Windows CI: 2.4 MB exe, 21 MB working
set (6.7 MB private) with a file open. GPU toolkits typically cost far more
memory and add driver risk.

### D-002 Owner cannot test interactively — CI is the acceptance gate
S0.6 (owner tries the exe) is skipped. Every push must pass Windows CI:
tests, release build, static-CRT check, pure-Rust check, headless screenshots,
and a real window launched for 4 s without crashing.

### D-001 Windows fonts: Segoe UI (UI/prose), Cascadia Mono → Consolas (code)
Both ship with Windows; nothing to bundle. Only these files are loaded (not the
whole font folder) to keep startup and memory low.
