# litedit vs lite-xl — feature comparison

Checked entry by entry against lite-xl's default key map
(`data/core/keymap.lua`) and the plugins it ships enabled. Status as of
litedit 1.1.0 (unreleased).

**Legend:** ✅ same · 🔁 same feature, different key or behaviour · ❌ missing
in litedit · ➕ litedit only

## Files, folders and workspaces

| lite-xl | litedit | Notes |
|---|---|---|
| New document (Ctrl+N) | ✅ Ctrl+N | litedit asks for the file name right away (in the current folder) |
| Open file / folder, recent | ✅ Ctrl+O, Ctrl+Shift+O, Ctrl+R | |
| Change project folder (Ctrl+Shift+C) | 🔁 Ctrl+Shift+O | Opening a folder switches to its workspace |
| Workspace per project (tabs + splits restored) | ✅ | Per folder, automatic |
| Add / remove project directories | ✅ | Palette or tree menu: Add Folder to Workspace…; tree menu: Remove |
| — | ➕ Save / Open Workspace file | VS Code-style `.litedit-workspace` |
| Tree: new file / folder, rename, delete | ✅ (+ F2, Delete keys) | litedit deletes to the Recycle Bin; lite-xl deletes permanently |
| — | ➕ Drag to move in the tree | Asks first, never overwrites |
| — | ➕ Multi-select in the tree; copy / paste / duplicate | One confirmation; copies never overwrite |
| — | ➕ Show in Explorer, Copy path, Find in folder from the tree | |
| Auto-reload changed files | ✅ | litedit asks if you have unsaved edits (Keep / Compare / Reload) |
| Toolbar above the tree (icons: new, open, find…) | ❌ | Everything is in the command palette / menus |

## Editing

| lite-xl | litedit | Notes |
|---|---|---|
| Undo/redo, cut/copy/paste, select all, select line | ✅ | |
| Duplicate / delete lines, indent / unindent | ✅ | |
| New line below / above (Ctrl+Enter / Ctrl+Shift+Enter) | ✅ | |
| Join lines (Ctrl+J) | ✅ | |
| Toggle line comment (Ctrl+/) | ✅ | |
| Toggle block comment (Ctrl+Shift+/) | ❌ | HTML/CSS use block comments with Ctrl+/ |
| Move lines (Ctrl+Up / Ctrl+Down) | 🔁 Alt+Up / Alt+Down | VS Code keys; Ctrl+Up/Down scroll |
| Multiple cursors: Ctrl+click, Ctrl+Shift+Up/Down, Ctrl+D, Ctrl+Shift+L | ✅ | |
| Select next / previous occurrence without adding (Ctrl+F3) | ❌ | |
| Block navigation (Ctrl+[ / Ctrl+]) | 🔁 | In litedit these indent / outdent (VS Code) |
| Overwrite mode (Insert) | ❌ | |
| Autocomplete (words from open files) | ❌ | |
| Upper / lower case, trim whitespace | ✅ palette | Trim on save is a setting (off by default, like lite-xl) |
| Reflow paragraph (Ctrl+Shift+Q) | ❌ | |
| Quote string (Ctrl+') | ❌ | |
| Tabularize (align on a character) | ❌ | |
| Record / play macro (Ctrl+Shift+; / Ctrl+;) | ❌ | |
| Indentation detection | ✅ | + convert to spaces / tabs (palette) |
| — | ➕ Line endings LF / CRLF (palette) | |

## Search

| lite-xl | litedit | Notes |
|---|---|---|
| Find / replace, F3 / Shift+F3 | 🔁 Ctrl+F / Ctrl+H | lite-xl replace is Ctrl+R (litedit: Ctrl+R = recent) |
| Case / regex toggles (Ctrl+I / Ctrl+Shift+I) | 🔁 Alt+C / Alt+R | |
| Go to line, command palette, find file (Ctrl+P) | ✅ | |
| Project search | 🔁 Ctrl+Shift+F | litedit: plain text only; lite-xl also regex and fuzzy |
| — | ➕ All matches highlighted, match count, replace-all is one undo | |
| — | ➕ Find in the rendered markdown preview, go to heading (outline) | |
| — | ➕ Back / Forward (Alt+Left / Alt+Right); files reopen where you left off | |

## View

| lite-xl | litedit | Notes |
|---|---|---|
| Tabs: Ctrl+Tab, Alt+1…9, middle-click close, wheel scrolls tabs | ✅ | |
| Move tab (Ctrl+PgUp / PgDn) | 🔁 drag the tab | Ctrl+PgUp/PgDn switch tabs (VS Code) |
| — | ➕ Tab menu: Close Others / to the Right / Saved / All, Ctrl+Shift+T | |
| Splits in any direction (Alt+Shift+J/L/I/K), switch (Alt+J/L/I/K) | 🔁 two sides, left/right | Ctrl+Alt+Right/Left move a tab across, Ctrl+1/2 switch |
| Word wrap | ✅ Alt+Z | |
| Zoom | ✅ Ctrl+±/0, Ctrl+wheel | |
| Full screen (F11, Alt+Enter) | ✅ F11 | |
| Draw whitespace (plugin, off by default) | ❌ | |
| Line guide at a column (plugin, off by default) | ❌ | |
| Right-click menu in the editor | ✅ | |
| Log view | ❌ | Crash log file only |
| Blinking caret | 🔁 steady | D-016 |
| — | ➕ Markdown preview (default), side-by-side live preview | The reason litedit exists |
| — | ➕ Compare two files / with saved / left ↔ right of a split / with the clipboard; ignore whitespace | |
| Image view (plugin-free: lite-xl has none built in) | ➕ Picture files open in a viewer tab | png, jpg, gif (first frame), webp, bmp |
| — | ➕ Light theme switch | |

## Customisation

| lite-xl | litedit | Notes |
|---|---|---|
| Lua user config, key rebinding, plugins | 🔁 settings.txt | By design: no code runs from config (security) |
| 4 colour themes | 🔁 dark + light | |

## Safety (litedit only)

Atomic saves that never lose the only copy; exact round-trip of odd
encodings (BOM, UTF-16, CRLF, invalid UTF-8 asks first); "changed by another
program" check before saving; no network; network paths, device paths and
symlinked images in documents are never opened; invisible bidi characters are
shown; hard limits on file, image and search cost.
