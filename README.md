# litedit

An ultralight, memory-safe text editor for Windows, written in Rust.

- **Markdown opens as a pretty preview by default.** Press `Ctrl+E` to edit.
- Code and text files are shown with syntax colours.
- File tree on the left.
- One small `.exe`: no installer, no browser engine, no plugins, no network.

Designed after [lite-xl](https://github.com/lite-xl/lite-xl) — see `NOTICE`.

> **Status:** Epic 1 (viewer) is complete: markdown preview, syntax-coloured
> code view, file tree, images, live reload. Editing arrives in Epic 2.
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
| `Ctrl+E` | Markdown: switch preview ↔ source |
| `Ctrl+\` | Show / hide the file tree |
| `Ctrl+Shift+E` | Focus the file tree (arrows, Enter, Esc to leave) |
| `Ctrl+Shift+H` | Show / hide hidden files in the tree |
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
