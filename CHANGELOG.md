# Changelog

## 1.0.0 — 2026-11-14

First release. One 6.5 MB `litedit.exe`, no installer, no runtime; about
15 MB of memory with a file open.

- **Markdown** opens as a rendered preview: headings, lists, task lists,
  tables, quotes, callouts, code blocks with syntax colours, local images,
  safe links. `Ctrl+E` edits; `Ctrl+Shift+V` shows source and live preview
  side by side. Select and copy text in the preview.
- **Code and text**: syntax colours for ~200 languages,
  undo/redo, soft wrap (`Alt+Z`), go to line, indent detection. Tolerates
  CRLF, BOM and non-UTF-8 files and saves them back byte for byte.
- **Find / replace** in a file (case, regex, replace all) and **find in
  folder** (`Ctrl+Shift+F`).
- **File tree** with new/rename/delete (to the Recycle Bin), quick open
  (`Ctrl+P`), command palette (`Ctrl+Shift+P`), tabs, drag and drop.
- **Safe saving**: atomic writes, unsaved-changes prompts, external-change
  detection, session restore.
- **Light and dark themes**, settings file (`Ctrl+,`): font sizes, fonts,
  wrap, ignored folders.
- **Security**: no network, no plugins, no scripts; only http/https/mailto
  links open; control and bidi characters are shown, never obeyed.

Known open questions (input methods, AltGr layouts, title bar theme): see
`DECISIONS.md`.
