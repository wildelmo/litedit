//! Prints the licences of the syntax definitions bundled in litedit.exe
//! (they come from the `two-face` crate). Used by
//! `scripts/third-party-notices.py`; not part of the editor.

fn main() {
    print!("{}", two_face::acknowledgement::listing().to_md());
}
