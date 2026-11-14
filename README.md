# litedit

An ultralight, memory-safe text editor for Windows, written in Rust.

- **Markdown opens as a pretty preview by default.** Press `Ctrl+E` to edit.
- Code and text files are shown with syntax colours.
- File tree on the left.
- One small `.exe`: no installer, no browser engine, no plugins, no network.

Designed after [lite-xl](https://github.com/lite-xl/lite-xl) — see `NOTICE`.

> **Status:** Epics 1–3 complete: markdown preview (default) and editing,
> side-by-side live preview, tabs, find/replace, find in folder, command
> palette, quick open, file tree with file actions, safe saving, session
> restore. Epic 4 (polish) is next.
> See [ROADMAP.md](ROADMAP.md) and [DECISIONS.md](DECISIONS.md).

## Try it

Download `litedit.exe` from the latest successful
[windows workflow run](../../actions/workflows/windows.yml) (artifact
`litedit-windows`). Then open a file or folder:

```
litedit.exe notes\todo.md
litedit.exe C:\path\to\folder
```

or drag a file or folder onto the window.

## Keys

| Key | Action |
|---|---|
| `Ctrl+O` / `Ctrl+Shift+O` | Open file / open folder |
| `Ctrl+P` / `Ctrl+R` | Quick open a file in the folder / open recent |
| `Ctrl+Shift+P` | Command palette (every command, with its shortcut) |
| `Ctrl+Tab`, `Ctrl+PgUp/PgDn`, `Alt+1..9` | Switch tabs |
| `Ctrl+W` | Close tab (asks if there are unsaved changes) |
| `Ctrl+F` / `Ctrl+H` | Find / replace (`F3`, `Alt+C` case, `Alt+R` regex, `Ctrl+Enter` replace all) |
| `Ctrl+Shift+F` | Find in folder |
| `Ctrl+G` | Go to line |
| `Ctrl+Shift+V` | Markdown: side-by-side live preview |
| `Ctrl+E` | Markdown: switch preview ↔ editing |
| `Ctrl+S` / `Ctrl+Shift+S` | Save / Save as |
| `Ctrl+Z` / `Ctrl+Y` | Undo / redo |
| `Ctrl+C` / `Ctrl+X` / `Ctrl+V` | Copy / cut / paste (whole line when nothing is selected) |
| `Ctrl+A` / `Ctrl+L` | Select all / select line |
| `Ctrl+Shift+D` / `Ctrl+Shift+K` | Duplicate line / delete line |
| `Tab` / `Shift+Tab` | Indent / outdent |
| `Alt+Z` | Toggle word wrap (on by default for markdown) |
| `Ctrl+\` | Show / hide the file tree |
| `Ctrl+Shift+E` | Focus the file tree (arrows, Enter, Esc to leave) |
| `Ctrl+Shift+H` | Show / hide hidden files in the tree |
| Right-click in the tree | New file / folder, rename, delete (Recycle Bin), find in folder |
| `Ctrl +` / `Ctrl -` / `Ctrl 0` | Zoom in / out / reset |
| Wheel, `PgUp`/`PgDn`, `Home`/`End`, `Space` | Scroll |

## Build

```
cargo build --release
```

Headless render (used for testing):

```
litedit --screenshot out.png [--size 1280x800] [--scale 1.5] <file>
```

## License

MIT. See `LICENSE` and `NOTICE`.
