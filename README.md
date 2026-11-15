# litedit

An ultralight, memory-safe text editor for Windows, written in Rust.

- **Markdown opens as a pretty preview by default.** Press `Ctrl+E` to edit.
- Code and text files are shown with syntax colours.
- File tree on the left.
- One small `.exe`: no installer, no browser engine, no plugins, no network.

Designed after [lite-xl](https://github.com/lite-xl/lite-xl) — see `NOTICE`, and
[docs/LITE-XL-COMPARISON.md](docs/LITE-XL-COMPARISON.md) for what is the same and what differs.

> **Version 1.0.0.** Markdown preview (default) and editing, side-by-side
> live preview, tabs, find/replace, find in folder, command palette, quick
> open, file tree with file actions, safe saving, session restore, light and
> dark themes, settings file. See [CHANGELOG.md](CHANGELOG.md),
> [ROADMAP.md](ROADMAP.md) and [DECISIONS.md](DECISIONS.md).

## Try it

Download `litedit.exe` (or the zip with the licence files) from the
[latest release](../../releases/latest). Builds of every commit are also
attached to each [windows workflow run](../../actions/workflows/windows.yml)
(artifact `litedit-windows`). Then open a file or folder:

```
litedit.exe notes\todo.md
litedit.exe C:\path\to\folder
```

or drag a file or folder onto the window. To open markdown files with a
double-click, run **Preferences: Add litedit to "Open with" for Markdown…** from
the command palette (`Ctrl+Shift+P`), then pick litedit once in Explorer's
"Open with" → "Always". If litedit is already running, opening
another file adds a tab to the running window (`--new-window` starts a second one).

## Keys

| Key | Action |
|---|---|
| `Ctrl+O` / `Ctrl+Shift+O` | Open file / open folder |
| `Ctrl+P` / `Ctrl+R` | Quick open a file in the folder / open recent |
| `Ctrl+Shift+P` | Command palette (every command, with its shortcut) |
| `Ctrl+Tab`, `Ctrl+PgUp/PgDn`, `Alt+1..9` | Switch tabs |
| `Ctrl+W` | Close tab (asks if there are unsaved changes) |
| `Ctrl+F` / `Ctrl+H` | Find / replace (`F3`, `Alt+C` case, `Alt+R` regex, `Ctrl+Enter` replace all). In the markdown preview `Ctrl+F` searches the rendered page |
| `Ctrl+Shift+F` | Find in folder |
| `Ctrl+G` | Go to line |
| `Ctrl+Shift+G` | Markdown: go to heading (outline list) |
| `Alt+Left` / `Alt+Right` | Back / forward through the places you've been |
| `Ctrl+Shift+V` | Markdown: side-by-side live preview |
| `Ctrl+E` | Markdown: switch preview ↔ editing |
| Click a task checkbox in the preview | Tick / untick it (`Ctrl+Z` undoes; `Ctrl+S` saves) |
| `Ctrl+S` / `Ctrl+Shift+S` | Save / Save as |
| `Ctrl+W` / `Ctrl+Shift+T` | Close tab / reopen the last closed tab |
| Right-click a file → Select for Compare, then Compare with Selected | Side-by-side comparison (`F7` / `Shift+F7` next / previous change); tab menu or palette: Compare with Saved |
| Tabs | Drag to reorder, wheel to scroll, middle-click to close, right-click for Close Others / Close All / Copy Path… |
| `Ctrl+Z` / `Ctrl+Y` | Undo / redo |
| `Ctrl+C` / `Ctrl+X` / `Ctrl+V` | Copy / cut / paste (whole line when nothing is selected) |
| `Ctrl+A` / `Ctrl+L` | Select all / select line |
| `Ctrl+Shift+D` / `Ctrl+Shift+K` | Duplicate line / delete line |
| `Alt+Up` / `Alt+Down` | Move line(s) up / down |
| `Ctrl+Enter` / `Ctrl+Shift+Enter` | New line below / above |
| `Ctrl+J` | Join lines |
| `Ctrl+Shift+Up` / `Ctrl+Shift+Down` | Add a caret above / below |
| Palette: Upper/Lower Case, Trim Trailing Whitespace, Convert Indentation, Line Endings | Text commands |
| `F11` | Full screen |
| `Ctrl+/` | Comment / uncomment line(s) |
| `Ctrl+D` / `Ctrl+Shift+L` | Select next / all occurrences (type to edit them all; `Esc` for one caret) |
| `Ctrl+click` / `Shift+click` | Add a caret / extend the selection |
| `Ctrl+N` | New file (named in the current folder) |
| `Ctrl+Shift+O` / `Ctrl+R` | Open folder / open recent file, folder or workspace |
| Palette: File: Save Workspace As… / Open Workspace… | Save or open a `.litedit-workspace` (folders + open files + split) |
| Palette or tree menu: Add Folder to Workspace… | Show another folder in the tree (right-click it → Remove Folder from Workspace) |
| `Ctrl+Alt+Right` / `Ctrl+Alt+Left` | Split: move the tab to the right / left side |
| `Ctrl+1` / `Ctrl+2` | Focus the left / right side |
| Right-click in the editor | Cut, Copy, Paste, Select All, Find, Toggle Comment, Compare with Saved |
| `Tab` / `Shift+Tab` | Indent / outdent |
| `Alt+Z` | Toggle word wrap (on by default for markdown) |
| `Ctrl+\` | Show / hide the file tree |
| `Ctrl+Shift+E` | Focus the file tree (arrows, Enter, Esc to leave) |
| `Ctrl+Shift+H` | Show / hide hidden files in the tree |
| Click a file in the tree | Peek: shows it in one reusable tab (italic title). Double-click, `Enter` or editing keeps it open |
| Right-click in the tree | New file / folder, rename, delete (Recycle Bin), find in folder |
| Drag in the tree | Move a file or folder into another folder (asks first) |
| `F2` / `Delete` in the tree | Rename / delete (Recycle Bin, asks first) |
| `Ctrl +` / `Ctrl -` / `Ctrl 0`, `Ctrl+wheel` | Zoom in / out / reset |
| `Ctrl+,` | Settings (theme, font sizes, fonts, wrap, ignored folders) |
| Wheel, `PgUp`/`PgDn`, `Home`/`End`, `Space` | Scroll |

## Build

```
cargo build --release
```

Headless render (used for testing):

```
litedit --screenshot out.png [--size 1280x800] [--scale 1.5] [--theme light] <file>
```

## License

MIT © 2026 Joe Chun. See `LICENSE`, `NOTICE` (lite-xl, the design this
follows) and `THIRD-PARTY-NOTICES.txt` (the open-source libraries and syntax
definitions built into the exe; regenerate with
`python3 scripts/third-party-notices.py` after changing dependencies).
