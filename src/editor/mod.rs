//! Editing core: buffer, history (undo/redo), and their tests.

pub mod buffer;
pub mod doc;
pub mod history;

#[cfg(test)]
mod tests {
    use super::buffer::{Buffer, Chunk, Eol, Pos};
    use super::history::{GroupKind, History, Op};
    use crate::doc::{Encoding, decode, encode};

    /// Open → save with no edits must be byte-identical for every fixture
    /// (except files that were not valid UTF-8, which cannot round-trip).
    #[test]
    fn fixtures_round_trip_byte_exact() {
        let mut checked = 0;
        for path in crate::tests::fixtures() {
            let bytes = std::fs::read(&path).unwrap();
            let (text, enc) = decode(&bytes);
            if enc == Encoding::Utf8Lossy {
                continue;
            }
            let buf = Buffer::from_text(&text);
            assert_eq!(encode(&buf.to_text(), enc), bytes, "{}", path.display());
            checked += 1;
        }
        assert!(checked >= 6);
        for raw in [
            &b"no final newline"[..],
            b"a\r\nb\nc\rd",
            b"\xEF\xBB\xBFbom\r\n",
            b"",
            b"\n",
        ] {
            let (t, e) = decode(raw);
            assert_eq!(encode(&Buffer::from_text(&t).to_text(), e), raw);
        }
    }

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n.max(1) as u64) as usize
        }
    }

    /// Byte offset of a (clamped) position in the exact text.
    fn offset(buf: &Buffer, p: Pos) -> usize {
        let p = buf.clamp(p);
        let text = buf.to_text();
        let chunk = buf.slice(Pos::new(0, 0), p);
        let off = chunk.to_text().len();
        assert!(off <= text.len());
        off
    }

    /// Model-based fuzz: random edits, undo and redo on the buffer and on a
    /// plain String must always agree; undoing everything restores the start.
    #[test]
    fn fuzz_buffer_and_history_against_string_model() {
        let pieces = [
            "a",
            "é",
            "👨\u{200d}👩\u{200d}👧",
            "\t",
            "\n",
            "\r\n",
            "word ",
            "x\ny\nz",
            "",
            "日本",
        ];
        for seed in 1..40u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1);
            let start = ["", "hello\r\nworld\r\n", "a\nb\rc\r\nd", "\u{feff}x"][seed as usize % 4];
            let mut buf = Buffer::from_text(start);
            let mut hist = History::default();
            let mut model = start.to_string();
            let mut undo_models: Vec<String> = Vec::new();
            let mut redo_models: Vec<String> = Vec::new();
            for _ in 0..300 {
                match rng.below(10) {
                    0..=3 => {
                        let line = rng.below(buf.line_count() + 1);
                        let col = rng.below(buf.line(line.min(buf.line_count() - 1)).len() + 2);
                        let at = buf.clamp(Pos::new(line, col));
                        let text = pieces[rng.below(pieces.len())];
                        let chunk = Chunk::typed(text, buf.default_eol());
                        if chunk.is_empty() {
                            continue;
                        }
                        let off = offset(&buf, at);
                        model.insert_str(off, &chunk.to_text());
                        undo_models.push(model.clone());
                        let inv = Op::Insert { at, chunk }.apply(&mut buf);
                        hist.record(vec![inv], vec![], vec![], GroupKind::Other);
                        redo_models.clear();
                    }
                    4..=6 => {
                        let a = buf.clamp(Pos::new(rng.below(buf.line_count()), rng.below(20)));
                        let b = buf.clamp(Pos::new(rng.below(buf.line_count()), rng.below(20)));
                        let (a, b) = super::buffer::order(a, b);
                        if a == b {
                            continue;
                        }
                        let (oa, ob) = (offset(&buf, a), offset(&buf, b));
                        model.replace_range(oa..ob, "");
                        undo_models.push(model.clone());
                        let inv = Op::Remove {
                            at: a,
                            chunk: buf.slice(a, b),
                        }
                        .apply(&mut buf);
                        hist.record(vec![inv], vec![], vec![], GroupKind::Other);
                        redo_models.clear();
                    }
                    7 | 8 => {
                        if hist.undo(&mut buf).is_some() {
                            let m = undo_models.pop().unwrap();
                            redo_models.push(m);
                            model = undo_models
                                .last()
                                .cloned()
                                .unwrap_or_else(|| start.to_string());
                        }
                    }
                    _ => {
                        if hist.redo(&mut buf).is_some() {
                            let m = redo_models.pop().unwrap();
                            undo_models.push(m.clone());
                            model = m;
                        }
                    }
                }
                assert_eq!(buf.to_text(), model, "seed {seed}");
            }
            while hist.undo(&mut buf).is_some() {}
            assert_eq!(
                buf.to_text(),
                start,
                "undo-all must restore the original (seed {seed})"
            );
        }
    }

    #[test]
    fn typed_text_uses_default_ending() {
        let mut b = Buffer::from_text("x\r\ny");
        let c = Chunk::typed("1\n2\r3", b.default_eol());
        b.insert(Pos::new(0, 1), &c);
        assert_eq!(b.to_text(), "x1\r\n2\r\n3\r\ny");
        assert_eq!(Eol::CrLf, b.default_eol());
    }
}
