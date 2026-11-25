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
    path: Vec<u8>,
    chars: Vec<char>,
    rows: Vec<Vec<u8>>,
    out: Vec<Match>,
}

/// Every term within `max` edits of `word`, by optimal string alignment.
pub(crate) fn within<D: AsRef<[u8]>>(fst: &Fst<D>, word: &str, max: u8) -> Vec<Match> {
    let word: Vec<char> = word.chars().collect();
    let first = (0..=word.len()).map(|j| j.min(255) as u8).collect();
    let mut walk = Walk {
        word: &word,
        max,
        path: Vec::new(),
        chars: Vec::new(),
        rows: vec![first],
        out: Vec::new(),
    };
    walk.visit(fst, fst.root(), 0, 0);
    walk.out
}

impl Walk<'_> {
    fn visit<D: AsRef<[u8]>>(&mut self, fst: &Fst<D>, node: Node<'_>, output: u64, pending: usize) {
        if node.is_final() && pending == 0 {
            let distance = self.rows.last().unwrap()[self.word.len()];
            if distance <= self.max {
                let term = String::from_utf8(self.path.clone()).expect("terms are UTF-8");
                self.out.push(Match {
                    term,
                    distance,
                    value: output + node.final_output().value(),
                });
            }
        }
        for t in node.transitions() {
            self.path.push(t.inp);
            let next = fst.node(t.addr);
            let output = output + t.out.value();
            let start = self.path.len() - pending - 1;
            match std::str::from_utf8(&self.path[start..]) {
                Ok(s) => {
                    let c = s.chars().next().unwrap();
                    if self.step(c) {
                        self.visit(fst, next, output, 0);
                        self.rows.pop();
                    }
                    self.chars.pop();
                }
                Err(e) if e.error_len().is_none() => self.visit(fst, next, output, pending + 1),
                Err(_) => {}
            }
            self.path.pop();
        }
    }

    /// Pushes the alignment row after `c`; false once no extension can come within `max`.
    fn step(&mut self, c: char) -> bool {
        self.chars.push(c);
        let depth = self.chars.len();
        let previous = &self.rows[depth - 1];
        let mut row = Vec::with_capacity(previous.len());
        row.push(depth.min(255) as u8);
        for j in 1..=self.word.len() {
            let mut cost = (previous[j] + 1)
                .min(row[j - 1] + 1)
                .min(previous[j - 1] + u8::from(self.word[j - 1] != c));
            if depth >= 2
                && j >= 2
                && self.word[j - 2] == c
                && self.word[j - 1] == self.chars[depth - 2]
            {
                cost = cost.min(self.rows[depth - 2][j - 2] + 1);
            }
            row.push(cost);
        }
        let alive = row.iter().min().is_some_and(|&m| m <= self.max);
        self.rows.push(row);
        if !alive {
            self.rows.pop();
        }
        alive
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
