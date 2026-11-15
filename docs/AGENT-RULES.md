# Rules for sub-agents working on litedit (read fully before starting)

litedit is an ultralight Windows-only editor/reader in Rust (developed and tested on Linux). The owner cares about: reading files, many files, comparing, small edits, and a very small memory/disk footprint. They cannot test interactively, so correctness rests on your tests.

## Where you work
- ONLY inside your own git worktree (given in your task), which is on its own branch. Its `target/` is already primed: `cargo test` builds in ~10 s. Do not set CARGO_TARGET_DIR.
- Never touch /home/user/litedit or any other /home/user/wt-* or /home/user/review-* directory. Other agents are working in parallel on other features; keep edits to shared files (src/app.rs, src/palette.rs, src/menu.rs, src/view/mod.rs, src/main.rs) minimal, additive and localized so they merge cleanly.
- Scratch files: `D=$(mktemp -d)` and delete only that directory. Never glob-delete in the system temp dir.

## Read first
- CLAUDE.md (architecture rules — follow them exactly), your story in ROADMAP.md (Epic 7), and the code you will change. docs/LITE-XL-COMPARISON.md and DECISIONS.md give background.

## Hard rules
- Never lose or damage user data: unsaved edits are never dropped; files are never overwritten without the existing confirmation flows; deletes go to the Recycle Bin; a question bar ("nag") is answered for the tab it asked about; nothing switches tabs under an open question.
- No panics reachable from user input or file content (release builds use panic=abort: a panic loses unsaved edits). No unwrap/expect/indexing on such data.
- No new crates. No network, no process spawning. Windows-only code must sit behind cfg(windows) with a Linux fallback, and must type-check: `cargo check --target x86_64-pc-windows-msvc`.
- Colours only via `theme::p()`. Views free rebuildable caches in `View::on_hide`. Headless modes never read user settings. Bounded work: anything driven by file content or folder size needs caps and/or time budgets.
- Do NOT edit ROADMAP.md, CHANGELOG.md, README.md, DECISIONS.md, docs/, .github/ or ci/. Put suggested text for them in your final report instead.

## Tests
- Put app-level tests in a NEW file `src/tests_<yourtopic>.rs`, declared in src/main.rs next to `mod tests;` as `#[cfg(test)] mod tests_<yourtopic>;`. Reuse helpers from `crate::tests` (new_app, key, mouse, click_tree, run_command, run_menu_item, render_hash, settled_hash, tmp_dir, many_files, fixture, press, type_text, ...). Unit tests go in the module they test. Do not append to src/tests.rs.
- Test the behaviour a user relies on, including the unhappy paths. For each important test, do a mutation check: temporarily break the feature, confirm the test fails, restore.
- Headless driving: `cargo run -- --script file` (commands documented at the top of src/script.rs); `shot out.png` then view the PNG with the Read tool to check visuals in both themes (`theme light`).

## Before you finish
- `cargo fmt`, `cargo test` (everything passes), `cargo clippy` (no new warnings in files you touched), `cargo check --target x86_64-pc-windows-msvc`, `cargo tree -i onig_sys` must find nothing.
- `git status` clean except your intended changes; commit on your branch in one or a few commits with plain descriptive messages. Do NOT add Co-Authored-By or any attribution line. No tags. Do NOT push.

## Final report (under 400 words)
Commit hash(es); what you built and the keys/commands; measured numbers if any; tests added and which mutation checks you ran; anything unfinished, uncertain or deliberately left out; suggested README key-table rows, one CHANGELOG bullet, and any decision the owner should know about.
