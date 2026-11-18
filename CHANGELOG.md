# Changelog

## 1.2.0 — 2026-11-18

The first release since 1.0.0 (1.1.0 was never published; its changes are
all here).

- Hardening from security scans: file reads, folder listings, the file
  watcher's queue, workspace restores and markdown pictures all have size
  limits; the one-window pipe only talks to the same Windows user; a rename
  never replaces a dangling link; very long lines can't make the code view
  use unbounded memory; saves on Windows still replace the file in one step
  and check it is the same local file.
- Hidden files and folders (`.docs`, `.github`, `.gitignore`, and what
  Windows marks hidden) are shown by default, in the tree and now also in
  Ctrl+P and find in folder. Ctrl+Shift+H (or View → Hidden Files) switches
  them off or on, and the choice is remembered (`hidden_files` in
  settings). `.git`, `node_modules` and the names in `ignore` stay out.
- Help → About litedit shows the version and who made the build: created
  by, company and email. Whoever builds litedit sets these in
  `assets/branding.txt`; lines left empty are not shown.
- Menu bar: File, Edit, View, Go and Help along the top, in the theme's
  colours, with each item's shortcut next to it. Alt+F / E / V / G / H or
  F10 opens it from the keyboard. View → Hide Menu Bar (or `menu_bar = off`
  in settings) hides it; Alt+F and F10 still open it.
- Close Folder (File menu, the tree header, or right-click the folder):
  its tabs are remembered for next time, tabs with unsaved edits stay, and
  the window shows a start page with Open File / Folder / Workspace and
  your recent folders, workspaces and files.
- The file tree has a header that says what is open — FOLDER, FOLDERS or
  WORKSPACE and its name — and a click on it opens the folder and workspace
  commands.
- Big files: text is coloured only as far as you read, and loading takes
  much less memory (a 100,000-line, 8 MB file: 60 MB → 37 MB at its peak).
- Read-only lock: "View: Toggle Read-Only" in the command palette protects
  a text tab from accidental edits (the status bar shows "Read-only").
- File tree: select several entries (Ctrl+click, Shift+click, Shift+Up /
  Down, Ctrl+A) and delete or drag-move them with one confirmation; Copy,
  Paste and Duplicate (Ctrl+C / Ctrl+V / Ctrl+D in the tree) that never
  overwrite anything; the status bar shows the selected file's size and
  when it last changed.
- Image files (png, jpg, gif, webp, bmp) open in a viewer tab: fit to the
  window or actual size (`0` / `1` or double-click), drag or scroll to pan,
  pixel size and file size in the status bar. Hidden picture tabs hold no
  pixels.
- Compare: the two sides of a split (Compare Left ↔ Right), Compare with
  Clipboard, Alt+W ignores whitespace differences, and Enter or a
  double-click on a line opens the file there.
- Back / Forward (Alt+Left / Alt+Right) through the places you've been,
  reopening closed files. Files reopen at the caret and scroll position you
  left them at, also after a restart.
- Fixes from an independent review of the newest features: a folder that
  contains one of your workspace folders can't be deleted, moved or renamed
  by accident; a comparison no longer crashes when one of its files is
  briefly unreadable; deleting a very large selection no longer freezes.
- Fixes from an independent review of the features below: a file opened from
  Explorer while litedit is running can no longer be silently dropped (odd
  paths open in their own window instead); Space on a tree entry never types
  into the file; a kept tab is never replaced by a single click; markdown
  tabs you haven't looked at yet hold no images; Go to Heading reaches
  headings inside lists and quotes; find in a huge preview stays fast.
- "Open with": a command adds litedit to Windows' "Open with" list for
  markdown files (and one removes it). It asks first and only changes your
  user's settings; you then pick litedit once in Explorer → Always.
- One window: opening a file while litedit is already running adds a tab to
  the running window instead of starting another copy (`--new-window` or
  `single_window = off` for the old behaviour).
- Peek tab: a single click on a file in the tree shows it in one reusable
  tab (italic title) instead of opening a new tab each time. Double-click,
  Enter, editing it, or "Keep Open" in the tab menu keeps it.
- Find in the markdown preview: Ctrl+F now searches the rendered page
  (matches highlighted, F3 / Shift+F3, count) instead of switching to the
  raw text. Ctrl+H (replace) still opens the source.
- Task lists: click a checkbox in the markdown preview to tick or untick
  it; Ctrl+Z / Ctrl+Y work in the preview.
- Go to heading (Ctrl+Shift+G): a searchable outline of the markdown
  document; jumps in the preview or the source.
- Much less memory with many tabs: tabs you aren't looking at free their
  rendered previews and images (30 viewed markdown tabs: 272 MB → 36 MB in
  a test) and rebuild instantly when you return.
- More editing commands: Ctrl+Enter / Ctrl+Shift+Enter (new line below /
  above), Ctrl+J (join lines), Ctrl+Shift+Up/Down (add caret), upper / lower
  case, trim trailing whitespace (also on save, setting
  `trim_trailing_whitespace`), convert indentation, LF / CRLF; F11 full
  screen.
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

Known open questions: input methods, AltGr layouts, title bar theme.
