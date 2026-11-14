# litedit

An ultralight, memory-safe text editor for Windows, written in Rust.

- **Markdown opens as a pretty preview by default.** Press `Ctrl+E` to edit.
- Code and text files are shown with syntax colours.
- File tree on the left.
- One small `.exe`: no installer, no browser engine, no plugins, no network.

Designed after [lite-xl](https://github.com/lite-xl/lite-xl) — see `NOTICE`.

> **Status:** early. The current build is a thin slice: it opens one file and
> displays it with line numbers. See [ROADMAP.md](ROADMAP.md) for the plan.

## Try it

Download `litedit.exe` from the latest successful
[windows workflow run](../../actions/workflows/windows.yml) (artifact
`litedit-windows`), then:

```
litedit.exe path\to\file.md
```

or drag a file onto the window. Scroll with the mouse wheel, PageUp/PageDown,
arrows, Home/End.

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
