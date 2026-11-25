//! Terms of a dictionary within an edit distance, counting adjacent transpositions as one edit.

use fst::raw::{Fst, Node};

pub(crate) struct Match {
    pub term: String,
    pub distance: u8,
    pub value: u64,
}

struct Walk<'w> {
    word: &'w [char],
    max: u8,
    keep_first: bool,
    path: Vec<u8>,
    chars: Vec<char>,
    /// Alignment rows, `word.len() + 1` cells each, one per character of `chars` plus the first.
    rows: Vec<u8>,
    out: Vec<Match>,
}

/// Every term within `max` edits of `word`, by optimal string alignment.
#[cfg(test)]
pub(crate) fn within<D: AsRef<[u8]>>(fst: &Fst<D>, word: &str, max: u8) -> Vec<Match> {
    within_from(fst, word, max, false)
}

/// [`within`], optionally only among terms starting with the same character as `word`.
pub(crate) fn within_from<D: AsRef<[u8]>>(
    fst: &Fst<D>,
    word: &str,
    max: u8,
    keep_first: bool,
) -> Vec<Match> {
    let word: Vec<char> = word.chars().collect();
    let mut walk = Walk {
        word: &word,
        max,
        keep_first: keep_first && !word.is_empty(),
        path: Vec::with_capacity(64),
        chars: Vec::with_capacity(48),
        rows: (0..=word.len()).map(|j| j.min(255) as u8).collect(),
        out: Vec::new(),
    };
    walk.visit(fst, fst.root(), 0, 0);
    walk.out
}

impl Walk<'_> {
    fn stride(&self) -> usize {
        self.word.len() + 1
    }

    fn visit<D: AsRef<[u8]>>(&mut self, fst: &Fst<D>, node: Node<'_>, output: u64, pending: usize) {
        if node.is_final() && pending == 0 {
            let distance = *self.rows.last().unwrap();
            if distance <= self.max {
                let term = String::from_utf8(self.path.clone()).expect("terms are UTF-8");
                let value = output + node.final_output().value();
                self.out.push(Match {
                    term,
                    distance,
                    value,
                });
            }
        }
        for t in node.transitions() {
            self.path.push(t.inp);
            let output = output + t.out.value();
            let start = self.path.len() - pending - 1;
            match std::str::from_utf8(&self.path[start..]) {
                Ok(s) => {
                    let c = s.chars().next().unwrap();
                    if self.step(c) {
                        self.visit(fst, fst.node(t.addr), output, 0);
                    }
                    self.rows.truncate(self.rows.len() - self.stride());
                    self.chars.pop();
                }
                Err(e) if e.error_len().is_none() => {
                    self.visit(fst, fst.node(t.addr), output, pending + 1)
                }
                Err(_) => {}
            }
            self.path.pop();
        }
    }

    /// Appends the alignment row after `c`; false once no extension can come within `max`.
    fn step(&mut self, c: char) -> bool {
        self.chars.push(c);
        let (depth, stride) = (self.chars.len(), self.stride());
        if depth == 1 && self.keep_first && c != self.word[0] {
            self.rows.resize(self.rows.len() + stride, 0);
            return false;
        }
        let at = self.rows.len();
        self.rows.resize(at + stride, 0);
        let (before, row) = self.rows.split_at_mut(at);
        let previous = &before[at - stride..];
        row[0] = depth.min(255) as u8;
        let mut best = row[0];
        for j in 1..stride {
            let mut cost = previous[j]
                .saturating_add(1)
                .min(row[j - 1].saturating_add(1))
                .min(previous[j - 1] + u8::from(self.word[j - 1] != c));
            if depth >= 2
                && j >= 2
                && self.word[j - 2] == c
                && self.word[j - 1] == self.chars[depth - 2]
            {
                cost = cost.min(before[at - 2 * stride + j - 2] + 1);
            }
            row[j] = cost;
            best = best.min(cost);
        }
        best <= self.max
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_typos_including_swapped_letters_and_accented_terms() {
        let words = ["apple", "apply", "maple", "pale", "paple", "über", "uber"];
        let mut sorted = words.to_vec();
        sorted.sort();
        let fst = fst::Map::from_iter(sorted.iter().enumerate().map(|(i, w)| (*w, i as u64)))
            .unwrap()
            .into_fst();
        let mut got: Vec<(String, u8)> = within(&fst, "aplpe", 1)
            .into_iter()
            .map(|m| (m.term, m.distance))
            .collect();
        got.sort();
        assert_eq!(got, [("apple".to_owned(), 1)]);
        let near: Vec<String> = within(&fst, "apple", 2)
            .into_iter()
            .map(|m| m.term)
            .collect();
        for expected in ["apple", "apply", "maple", "paple", "pale"] {
            assert!(near.contains(&expected.to_owned()), "{expected}");
        }
        let accented: Vec<String> = within(&fst, "ubre", 1)
            .into_iter()
            .map(|m| m.term)
            .collect();
        assert_eq!(accented, ["uber"]);
        let m = within(&fst, "über", 0);
        assert_eq!(
            (m[0].term.as_str(), m[0].value),
            (
                "über",
                sorted.iter().position(|w| *w == "über").unwrap() as u64
            )
        );
    }
}
