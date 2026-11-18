//! Reading navigation: the Back / Forward history (browser semantics) and
//! the remembered position of files that were closed.
//!
//! Pure data — the app decides what a jump is and how a location is shown.

use std::path::{Path, PathBuf};

use crate::view::ViewPos;

/// Most locations kept in each direction.
pub const MAX_HISTORY: usize = 100;
/// Most closed files whose position is remembered.
pub const MAX_REMEMBERED: usize = 500;

/// A place the reader was: a file and where in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loc {
    pub path: PathBuf,
    pub pos: ViewPos,
    /// It was showing in the peek tab (and is shown there again).
    pub peek: bool,
}

impl Loc {
    /// The same place (whichever kind of tab showed it).
    pub fn same(&self, other: &Loc) -> bool {
        self.path == other.path && self.pos == other.pos
    }
}

#[derive(Default)]
pub struct History {
    /// Oldest first; the last entry is where Back goes.
    back: Vec<Loc>,
    /// The last entry is where Forward goes.
    forward: Vec<Loc>,
}

fn push_capped(list: &mut Vec<Loc>, loc: Loc) {
    if list.last().is_some_and(|l| l.same(&loc)) {
        return;
    }
    list.push(loc);
    if list.len() > MAX_HISTORY {
        list.remove(0);
    }
}

impl History {
    /// The reader jumped away from `from`: Back returns there. A new jump
    /// drops everything Forward would have gone to.
    pub fn jumped(&mut self, from: Loc) {
        push_capped(&mut self.back, from);
        self.forward.clear();
    }

    /// The next place going back (or forward) that `usable` accepts and
    /// that isn't `here`; entries that fail are dropped. `here` is kept
    /// for the way back. None: nowhere to go (nothing changes for `here`).
    pub fn step(
        &mut self,
        back: bool,
        here: Option<&Loc>,
        mut usable: impl FnMut(&Loc) -> bool,
    ) -> Option<Loc> {
        let (from, to) = if back {
            (&mut self.back, &mut self.forward)
        } else {
            (&mut self.forward, &mut self.back)
        };
        while let Some(loc) = from.pop() {
            if here.is_some_and(|h| h.same(&loc)) || !usable(&loc) {
                continue;
            }
            if let Some(h) = here {
                push_capped(to, h.clone());
            }
            return Some(loc);
        }
        None
    }

    /// (back, forward) entry counts.
    #[cfg(test)]
    pub fn lens(&self) -> (usize, usize) {
        (self.back.len(), self.forward.len())
    }
}

/// Where the reader was in files that are closed now (most recent last).
#[derive(Default)]
pub struct Remembered {
    list: Vec<(PathBuf, ViewPos)>,
}

impl Remembered {
    pub fn put(&mut self, path: &Path, pos: ViewPos) {
        self.list.retain(|(p, _)| p != path);
        self.list.push((path.to_path_buf(), pos));
        if self.list.len() > MAX_REMEMBERED {
            self.list.remove(0);
        }
    }

    pub fn get(&self, path: &Path) -> Option<ViewPos> {
        self.list
            .iter()
            .rev()
            .find(|(p, _)| p == path)
            .map(|(_, pos)| *pos)
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.list.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc(name: &str, line: usize) -> Loc {
        Loc {
            path: PathBuf::from(name),
            pos: ViewPos {
                line,
                ..ViewPos::default()
            },
            peek: false,
        }
    }

    #[test]
    fn back_and_forward_walk_the_same_places() {
        let mut h = History::default();
        h.jumped(loc("a", 1));
        h.jumped(loc("b", 2));
        // Now at c:3.
        let here = loc("c", 3);
        assert_eq!(h.step(true, Some(&here), |_| true), Some(loc("b", 2)));
        assert_eq!(
            h.step(true, Some(&loc("b", 2)), |_| true),
            Some(loc("a", 1))
        );
        assert_eq!(h.step(true, Some(&loc("a", 1)), |_| true), None);
        assert_eq!(h.lens(), (0, 2), "a failed step changes nothing");
        assert_eq!(
            h.step(false, Some(&loc("a", 1)), |_| true),
            Some(loc("b", 2))
        );
        assert_eq!(h.step(false, Some(&loc("b", 2)), |_| true), Some(here));
        assert_eq!(h.step(false, Some(&loc("c", 3)), |_| true), None);
        assert_eq!(h.lens(), (2, 0));
    }

    #[test]
    fn a_new_jump_drops_the_forward_entries() {
        let mut h = History::default();
        h.jumped(loc("a", 1));
        h.jumped(loc("b", 2));
        h.step(true, Some(&loc("c", 3)), |_| true);
        assert_eq!(h.lens(), (1, 1));
        h.jumped(loc("b", 2));
        assert_eq!(h.lens(), (2, 0));
        assert_eq!(h.step(false, Some(&loc("d", 4)), |_| true), None);
    }

    #[test]
    fn duplicates_collapse_and_the_history_is_bounded() {
        let mut h = History::default();
        h.jumped(loc("a", 1));
        h.jumped(loc("a", 1));
        h.jumped(Loc {
            peek: true,
            ..loc("a", 1)
        });
        assert_eq!(h.lens(), (1, 0));
        h.jumped(loc("a", 2));
        assert_eq!(h.lens(), (2, 0));
        for i in 0..MAX_HISTORY * 3 {
            h.jumped(loc("f", i));
        }
        assert_eq!(h.lens(), (MAX_HISTORY, 0));
        // The newest survive.
        let top = h.step(true, None, |_| true).unwrap();
        assert_eq!(top.pos.line, MAX_HISTORY * 3 - 1);
        assert_eq!(h.lens(), (MAX_HISTORY - 1, 0), "nowhere to come back to");
    }

    #[test]
    fn unusable_entries_and_the_current_place_are_skipped() {
        let mut h = History::default();
        h.jumped(loc("a", 1));
        h.jumped(loc("gone", 5));
        h.jumped(loc("c", 3));
        let here = loc("c", 3);
        let got = h.step(true, Some(&here), |l| l.path != Path::new("gone"));
        assert_eq!(got, Some(loc("a", 1)));
        assert_eq!(h.lens(), (0, 1));
    }

    #[test]
    fn remembered_positions_are_bounded_and_updated() {
        let mut r = Remembered::default();
        for i in 0..MAX_REMEMBERED + 50 {
            r.put(Path::new(&format!("f{i}")), loc("", i).pos);
        }
        assert_eq!(r.len(), MAX_REMEMBERED);
        assert_eq!(r.get(Path::new("f0")), None, "oldest forgotten");
        assert_eq!(r.get(Path::new("f60")).map(|p| p.line), Some(60));
        r.put(Path::new("f60"), loc("", 7).pos);
        assert_eq!(r.len(), MAX_REMEMBERED);
        assert_eq!(r.get(Path::new("f60")).map(|p| p.line), Some(7));
    }
}
