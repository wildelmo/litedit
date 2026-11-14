# litedit — working notes for Claude

Read `ROADMAP.md` first: it is the source of truth for scope, story status and
progress. Update it (story checkboxes + progress log) in the same commit as the
work.

## Project

Ultralight Windows text editor in Rust. Markdown opens as a native preview by
default; code is syntax-highlighted. Blueprint: lite-xl (MIT) — a reference
clone lives outside the repo; copy architecture/behaviour, credit in `NOTICE`.

## Architecture decisions (keep unless a story says otherwise)

- **Rendering:** software, like lite-xl. `winit` window → `softbuffer` u32
  framebuffer → our own `Canvas` (rects, glyph masks, images, clip). No GPU.
- **Text:** `cosmic-text` (shaping, wrapping, swash rasterizing).
- **Render path is shared** between the window and headless
  `--screenshot <out.png>` mode. Use screenshot mode to verify visuals here.
- **Redraw on events only** (no busy loop).
- Markdown: `pulldown-cmark`. Syntax: `syntect` with fancy-regex (pure Rust).
  `cargo tree -i onig_sys` must stay empty.

## Rules

- Windows is the only target. Develop/test on Linux here, but CI on
  `windows-latest` is the real check. Static CRT (`.cargo/config.toml`).
- Never `fs::canonicalize` for display/compare (produces `\\?\` paths).
- File loading must tolerate CRLF, UTF-8 BOM, non-UTF-8 (lossy, never panic).
- Links: only http/https/mailto go to `open`. Images: local only, size-limited.
- No network, no plugins, no process spawning beyond the above.
- Keep dependencies few; justify each new crate in the commit message.

## Commands

- `cargo run -- <path>` — run (Linux needs a display; use `xvfb-run` if not).
- `cargo run -- --screenshot out.png [--size 1280x800] <path>` — headless render.
- `cargo test`
