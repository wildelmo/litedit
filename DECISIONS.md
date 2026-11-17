# Decisions log

Decisions made while the owner is away. Each has a recommendation and a
reason. Status: **decided** (implemented, review later) or **open** (needs the
owner). Newest first within each section.

## Open — please review

### D-018 Typing with an input method (IME) is untested
If you type Chinese, Japanese, Korean or use another input method: litedit
accepts the finished text and places the candidate window at the caret, but
the in-progress (not yet committed) text is not drawn inline. I can't verify
IME behaviour in the build environment. **Question:** do you use an IME? If
yes, tell me what you see and I'll finish inline composition.

### D-019 AltGr typing is assumed, not yet verified on a real keyboard layout
On German, French and other layouts, AltGr+key types characters like @, {
and €. Windows reports AltGr as Ctrl+Alt; litedit treats Ctrl+Alt with
typed text as typing (unit-tested), but this depends on how the window
library reports it. **Question:** if you use such a layout, check that @ and
{ type correctly.

### D-025 Two things only a real Windows desktop can confirm
(1) The title bar follows the theme (dark title bar on the dark theme, light
on light). (2) `ui_font` / `code_font` in settings pick a font that isn't one
of the bundled-with-Windows defaults (a unit test checks Arial on CI, but not
your installed fonts). **Question:** if either looks wrong, tell me what you
see.

## Decided — review when convenient
### D-043 Hidden files are shown by default, and searched when shown
Names starting with a dot (`.docs`, `.github`, `.gitignore`) and files
Windows marks hidden or system are listed in the tree unless you switch
them off (Ctrl+Shift+H, View → Hidden Files); the choice is saved as
`hidden_files` in settings. Ctrl+P and find in folder follow the same
switch: what the tree shows is what they look through. Before, they never
looked at hidden files. `.git`, `node_modules` and similar, and the names
in the `ignore` setting, are left out either way. **If a large dot-folder
gets in the way** (`.venv`, a tool's cache), add its name to `ignore`.

### D-042 About shows the version and who built it; no update check
Help → About litedit opens a tab with the version compiled into the
program and three lines from `assets/branding.txt`: Created by, Company
and Email. The file is embedded when litedit is built (edit it, then
build); it is not read when the program runs, and a line left empty is not
shown. The copy in the repository has the name only. The Windows file
properties (`build.rs`) are separate. The owner's other app also checks a
release page for a newer version; that was considered here and **dropped**:
litedit makes no network requests at all, and that promise is worth more
than an update notice. New versions are found on the releases page by hand.

### D-041 The tree header names a workspace file, which is still a snapshot
The header says FOLDER (one folder), FOLDERS (folders added but not saved
as a workspace) or WORKSPACE · name (opened from, or saved as, a
`.litedit-workspace` file). The file is not updated by itself: as before,
each folder's tabs are remembered automatically, and the file changes only
when you choose Save Workspace As… (which now suggests the same file).
Writing to it on every change would surprise anyone who keeps it in git or
shares it. The name is remembered across restarts; the file is not read
again for that, and a missing file drops the name.

### D-040 Menu bar: drawn by litedit, on by default, Alt+letter and F10
A row at the top with File, Edit, View, Go and Help. It is drawn like the
rest of the window, not the standard Windows menu bar, which stays light
in the dark theme and is invisible to the screenshot tests. Almost every
item replays its keyboard shortcut (as the command palette does), so a
menu can't do something different from the keys. Alt + the first letter
opens a menu; Alt+Z, Alt+W, Alt+C, Alt+R and Alt+digits keep their jobs,
and AltGr (Ctrl+Alt) stays typing. Pressing Alt alone does nothing: telling
an Alt tap from Alt+Tab needs a focus event the app doesn't get. Hiding the
bar (View → Hide Menu Bar, `menu_bar = off`) is saved in settings; Alt+F
and F10 still show it while a menu is open. Headless screenshots show the
bar like the window does; most older unit tests start without it because
they click at fixed positions. **To check on a real desktop:** Alt+F and
F10 open the menu without a beep (winit keeps those keys from Windows'
own menu handling; CI step 9 checks they arrive).

### D-039 One-window pipe: who may connect, and who a launch trusts
The pipe is created owned by the current user with an access list that
names only that user (`O:<user SID>D:P(A;;GA;;;<user SID>)`); nobody else
can open it, even to read. The user is named explicitly so a window started
as administrator stays reachable from normal launches. If the access list
cannot be built, the window does not listen (launches open their own
windows); there is no fallback to the Windows default. The pipe's name can
be guessed, so a launch trusts the other end only by the pipe's owner: it
reads the owner before writing anything and sends its file paths only if
the owner is the same user. Only that user, or an administrator, can create
an object owned by them. The window reads a launch's message within 1 s and
waits at most 1 s more for it to hang up; the files open as soon as "ok"
is written. Cost: if another account has taken the name, one window is off
for you until they stop (each launch opens its own window; nothing leaks).
Not verified on a real multi-user PC.

### D-038 Markdown preview: only up to 1 MiB, 100,000 lines, 30,000 blocks
Larger markdown files open in source view with a status-bar notice; Ctrl+E
previews again once the file is small enough. Blocks are counted at every
level (list items and table cells included). Why: the preview shapes the
whole document at once, which costs about 300 bytes of memory per byte of
text plus about 3.5 KB per block (Linux, release). Without a limit one
million headings took 8 s and 3.4 GB, and a 16 MB file of headings ran out
of memory. Just under the limits the worst files take about 1 s and under
500 MB (29,000 list items: 1.05 s / 477 MB; 1 MB of prose: 0.8 s / 320 MB).
A book of about 1 MB still previews; a 3 MB one, or a changelog with more
than about 15,000 entries, opens as source. **To review:** raising the byte
limit to 2 MiB would preview 2 MB of prose at about 600–850 MB. The real
fix is to shape only the part near the screen, which is not built.

### D-037 Restoring tabs: one per file, at most 512 MB together
A session or workspace restore opens each file once (paths compared as text:
`.` and `..` resolved, case and slash style ignored on Windows) and at most
512 MB of files in total, the active tab first. Files that do not fit are
not opened; one message says how many, and they stay in the workspace so the
next restore tries again. Why: a workspace file naming one large file 200
ways made the editor allocate tens of gigabytes. Side effect: opening a file
under another spelling now focuses the tab that is already open.

### D-036 Which opens are checked for links to network shares
When content asks for a file, every link along its path is checked (never
followed) and it is refused if one leads to a network share or device path:
file links in markdown, tabs and folders from a workspace file, paths sent
by a later launch, tree entries that are links, and Compare from the tree.
The reader's own choices are never checked: file dialog, command line, drag
and drop, Save / Save As, the saved session and the remembered workspace of
a folder. A document opened on a share on purpose keeps working, including
the files next to it; only a link that leads somewhere else on the network
is refused. `Document::open` itself is not checked. Known cost: inside a
folder reached through your own link to a NAS, markdown file links and
pictures are refused.

### D-035 Markdown pictures: a budget per document, no eviction
One markdown preview keeps track of at most 1000 different pictures and holds
at most 512 MB of decoded pixels. A picture is filed under one spelling of
its path (`.` and `..` resolved as text, case ignored on Windows, never
`canonicalize`). The first picture that does not fit, and every one after it,
shows "not shown: the pictures on this page already use 512 MB", and decoding
stops. Why: one small document naming the same picture a few hundred ways
made the editor hold gigabytes and abort. Not done: loading only the pictures
near the screen (a document with more than about 22 full-size photos loses
the later ones; say if that matters to you). Worst case for one visible
document is about 0.7 GB including the picture being decoded.

### D-034 Hidden tabs keep the colours of their first 2000 lines
The rule is that a tab you aren't looking at holds nothing it can rebuild.
One exception: syntax colours for up to 2000 lines stay (measured: about
90 KB per fully read 2000-line tab), so switching back to an ordinary file doesn't flash uncoloured
text. Longer files drop all colours when hidden and recolour as you read.

### D-033 Read-only lock: what it blocks and what it doesn't
A locked tab refuses everything that changes its text, including undo and
redo. Cut copies without removing. Save As, and reloading when another
program changes the file, still work (they don't change what you see by
accident). The lock is per tab and is not remembered after closing.

### D-032 Copying in the tree: litedit's own list, strict limits
Ctrl+C in the tree remembers the entries inside litedit only (it does not
put files on the Windows clipboard, so you can't paste them into Explorer
or the other way round). A copy never replaces anything ("name - Copy"),
skips shortcuts / links instead of following them, and is refused up front
when it is very large (entry and size caps) — use Explorer for those.
Duplicate (Ctrl+D) doesn't ask, since it can't overwrite. If litedit is
killed mid-copy one partly written file can remain; a normal exit or a
failure removes it.
Deleting many entries asks Windows to recycle them one at a time; a
selection of thousands may take a while (not measurable from the build
machine's tests, which use a stand-in for the Recycle Bin).

### D-031 Pictures: fit view keeps a reduced copy; 100% decodes everything
A picture tab keeps at most 2400 pixels on the long side while fitted to the
window (plenty for any screen litedit targets) and decodes the full picture
only at 100%. Limits are the same as for images in markdown: 40 MB file, 25
megapixels. Animated GIFs show their first frame. Zoom is fit or 100% only
(Ctrl+wheel stays text zoom). The pixels are freed when you switch tabs.

### D-030 One window: a second launch opens its files in the running window
Double-clicking another file while litedit is open adds a tab to the open
window instead of starting a second copy (less memory, tabs in one place).
It works through a local named pipe on your PC — not the network: remote
clients are rejected, only your own user can write to it, and the only thing
sent is a list of file paths. If the running window doesn't answer within
1.5 seconds the new launch just opens its own window. Folders and workspace
files always open a new window (they would switch the running window's
workspace). `litedit --new-window file` or `single_window = off` in settings
gives the old behaviour.
Known limits (accepted): another program running as you could create the
pipe first and receive the file *paths* a launch sends (never file
contents; it could already read your files). litedit lets such a program
identify the sender but not act as it. A program that connects and then
sends nothing delays hand-over for other launches until it disconnects;
they open their own window after 1.5 s, so nothing is lost.

### D-028 Files with NUL bytes don't open as text
A NUL byte in the first 8 KB (and no UTF-16 byte-order mark) means an image,
program or archive; opening it as text shows garbage and saving would damage
it, so litedit shows a message instead. Rare text files with NULs (some logs,
UTF-16 without a BOM) are refused too. Tell me if you hit one; an
"open anyway" option is possible.

### D-029 Disk-change checks trust size + modified time first
Before saving, litedit checks whether another program changed the file. If
the size and modification time are exactly as when litedit last read or
wrote it, the file isn't re-read (saving a large file stays instant);
otherwise its text is compared. The shortcut is only used when the time has
sub-second precision (NTFS); on FAT/exFAT USB drives (2 s / 10 ms times) the
text is always compared. A file that can no longer be read as text counts as
changed, so saving asks first.

### D-027 Split view: two sides, Ctrl+Alt+arrows, not saved in the session
VS Code uses Ctrl+\ to split, but in litedit Ctrl+\ already shows/hides the
file tree (lite-xl's habit), so moving a tab across is Ctrl+Alt+Right/Left
(VS Code's "move editor to next group") and Ctrl+1/2 focus a side. At most
two sides; a file is open in only one tab (two live copies of one file would
fight over who saves last). Since S5.5 the split is restored with the tabs.

### D-026 Known cosmetic limitations left after the 1.1 review
Not worth the risk or size right now; tell me if any bothers you:
- Soft wrap counts every character as one column, so CJK lines can run past
  the right edge, and a break can fall between a letter and its accent.
- Home on a wrapped row goes to the start of the whole line.
- Regex search skips empty matches (`^`, `\b`), so "add text at every line
  start" with `^` is not possible; use Alt+Shift selection later (S5.4).
- Path comparisons for open tabs are exact (case-sensitive). On Windows two
  spellings of one path (`Notes.md` / `notes.md`) could open twice.


### D-024 Settings are a small text file, applied when saved
`%APPDATA%\litedit\settings.txt`, `key = value` lines (lite-xl uses a Lua
user module; litedit runs no user code, so it is plain data). Ctrl+, or
"Preferences: Open Settings" opens it, creating a commented template the
first time. Saving it from litedit applies theme, font sizes, wrap default
and ignore list at once; font *families* apply at next start (reloading fonts
while running is not worth the complexity). Unknown keys or bad values are
named in the status bar and the defaults are used — never an error dialog.
The theme toggle command edits only the `theme` line, so comments and other
settings are kept. Edits made in another program apply at next start.

### D-023 No smooth (animated) scrolling
Animating scrolls means redrawing for many frames per wheel notch, which
costs CPU for no functional gain; litedit jumps directly and stays idle.
Measured frames are already a few milliseconds.

### D-022 Markdown wraps lines; code does not (Alt+Z toggles)
Prose is easier to edit when long lines wrap at the window edge; code reads
better unwrapped with horizontal scrolling. Alt+Z switches either way.
Wrapping is visual only — the file's lines are unchanged.

### D-021 Starting litedit without a file restores the last session
Like VS Code: the last folder, open tabs (that still exist), active tab, tree
width and zoom come back. Opening a specific file (double-click, drag onto
the exe, command line) opens just that file. Stored in
`%APPDATA%\litedit\session.txt`; delete it to start fresh.

Updated in 1.1: each folder also remembers its own tabs and split
(`%APPDATA%\litedit\workspaces`), and workspaces can be saved to files.

### D-020 Delete from the tree goes to the Recycle Bin, with Windows' warning
litedit asks "Move X to the Recycle Bin?" first. Windows CI verifies the item
really lands in the Recycle Bin (item count goes up). Where Windows can't
recycle (network drives, very large items), litedit passes the flag that
makes Windows show its own "delete permanently?" warning — that case can't
be reproduced on CI, so it rests on Windows' documented behaviour. Files
with unsaved edits must be saved or closed before rename/delete.

### D-017 Saving a file another program holds open
Saves normally write a temporary copy and swap it in (so a crash mid-save
can't leave a half-written file). If another program holds the file open in
a way that blocks the swap (common with sync tools), litedit overwrites the
file in place instead of refusing. If the file is fully locked, the save
fails with a message and your edits stay unsaved in the editor.

### D-016 The caret does not blink
A blinking caret means waking the CPU twice a second forever. litedit
redraws only when something happens, so the caret is steady. *Easy to add
as an option if you miss it.*

### D-015 Very long lines: editing works past the display cut
Lines longer than 4000 characters are drawn up to the cut (…). The text
beyond is still there and editable; the caret is drawn at the cut when it
is past it. Real files almost never have such lines (minified code does).

### D-013 Prompts appear as a bar inside the window, not as pop-up dialogs
"Save changes?", "File changed on disk — reload?" and similar questions
appear as a bar at the top of the editor (like lite-xl's NagView), answered
with a click or the keyboard. Native pop-ups can't be exercised by the
automated tests, and the in-window bar never hides behind other windows.

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

Extended in 1.1: the invisible marks that also reorder text (LRM, RLM,
ALM), zero-width space / word joiner / ZWNBSP, C1 controls and tag
characters are shown as �. ZWJ and ZWNJ are kept because emoji and many
scripts need them.

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
exists (palette: "Toggle Light / Dark Theme", or `theme = light` in settings;
D-024). *Say if you want light as the default.*

### D-006 No automatic file associations
litedit will not register itself as the `.md` handler. Doing it silently is
the kind of system change users dislike; it can be added as an explicit,
optional action later (S4.5). You can still right-click → Open with → litedit.

Updated in 1.1: the command palette has "Preferences: Add litedit to "Open
with" for Markdown…" (asks first, shows the program path, changes only your
user's settings) and a matching Remove. Windows does not allow a program to
make itself the default app, so after adding, choose litedit once in
Explorer: right-click a .md file → Open with → Choose another app → litedit →
Always.

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
