# Changelog

## 1.1.0 — unreleased

- Workspaces: every folder remembers its open files and split, like lite-xl
  — switch folders and come back to where you were (files with unsaved
  edits always stay open). Save a workspace to a `.litedit-workspace` file
  (command palette: File: Save Workspace As…) and open it later or on
  another PC (paths inside the project are stored relative).
- Several folders in one workspace: File: Add Folder to Workspace… (command
  palette, or right-click the top of the tree); quick open, find in folder
  and workspace files include them all.
- Split view: Ctrl+Alt+Right moves the current tab to a right-hand side
  (with its own tabs); Ctrl+Alt+Left moves it back; Ctrl+1 / Ctrl+2 or a
  click switches sides. Also in the tab menu and command palette.
- Editing: Ctrl+D selects the next occurrence (multiple carets; type to edit
  them all), Ctrl+Shift+L all occurrences, Ctrl+click adds a caret,
  Shift+click extends the selection, Alt+Up/Down moves lines, Ctrl+/
  comments lines, Ctrl+N makes a new file, right-click menu in the editor,
  Ctrl+mouse wheel zooms.
- Safer saving (from an end-to-end review): a question bar always answers
  for the file it asked about; your own save no longer looks like an outside
  change; when another program really changed a file you are editing, the
  safe answer is the default and you can compare first; saving over another
  program's change asks first; files that can't be saved back exactly (odd
  encodings) always ask before saving, including Save As.
- File tree: drag a file or folder onto a folder to move it (asks first,
  never overwrites); F2 renames and Delete deletes the selected entry.
- Tabs with unsaved changes show a dot in place of the close button.
- Tabs: when many are open the bar scrolls (mouse wheel) instead of hiding
  tabs; the active tab is kept in view; long names end in "…". Drag tabs to
  reorder. Right-click a tab for Close, Close Others, Close to the Right,
  Close Saved, Close All, Copy Path, Reveal in File Tree, Show in Explorer.
  Ctrl+Shift+T reopens the last closed tab.
- Compare two files side by side: right-click a file → Select for Compare,
  then right-click another → Compare with Selected. "Compare with Saved"
  (tab menu or command palette) shows your unsaved edits against the file on
  disk. Removed lines red, added green, changed characters highlighted;
  F7 / Shift+F7 jump between changes.

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
