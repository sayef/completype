use unicode_normalization::char::{decompose_canonical, is_combining_mark};

/// Splits text into lowercase terms: runs of letters and digits, each CJK character on its own.
#[derive(Clone, Debug)]
pub struct Analyzer {
    fold_accents: bool,
    max_len: usize,
}

impl Default for Analyzer {
    fn default() -> Self {
        Self {
            fold_accents: true,
            max_len: 40,
        }
    }
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xac00..=0xd7af | 0xf900..=0xfaff | 0x20000..=0x2fa1f)
}

impl Analyzer {
    /// Keeps accents, so `café` and `cafe` stay different terms.
    pub fn keep_accents(mut self) -> Self {
        self.fold_accents = false;
        self
    }

    /// Calls `emit` with each term of `text`, in order.
    pub fn analyze(&self, text: &str, mut emit: impl FnMut(&str)) {
        let mut term = String::new();
        let mut chars = 0;
        let mut flush = |term: &mut String, chars: &mut usize| {
            if !term.is_empty() && *chars <= self.max_len {
                emit(term);
            }
            term.clear();
            *chars = 0;
        };
        for c in text.chars() {
            if is_cjk(c) {
                flush(&mut term, &mut chars);
                term.push(c);
                chars = 1;
                flush(&mut term, &mut chars);
            } else if c.is_alphanumeric() {
                for lower in c.to_lowercase() {
                    if self.fold_accents && !lower.is_ascii() {
                        decompose_canonical(lower, |d| {
                            if !is_combining_mark(d) {
                                term.push(d);
                            }
                        });
                    } else {
                        term.push(lower);
                    }
                }
                chars += 1;
            } else if !(is_combining_mark(c) && !term.is_empty()) {
                flush(&mut term, &mut chars);
            } else if !self.fold_accents {
                term.push(c);
            }
        }
        flush(&mut term, &mut chars);
    }

    pub fn terms(&self, text: &str) -> Vec<String> {
        let mut out = Vec::new();
        self.analyze(text, |t| out.push(t.to_owned()));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_lowercased_split_and_folded() {
        let a = Analyzer::default();
        assert_eq!(
            a.terms("Crème Brûlée, NAÏVE café-au-lait 3.14!"),
            ["creme", "brulee", "naive", "cafe", "au", "lait", "3", "14"]
        );
        assert_eq!(a.terms("Straße Größe"), ["straße", "große"]);
        assert_eq!(
            a.terms("東京タワー visit"),
            ["東", "京", "タ", "ワ", "ー", "visit"]
        );
        assert_eq!(a.terms("cafe\u{301} x"), ["cafe", "x"]);
        assert_eq!(Analyzer::default().keep_accents().terms("Café"), ["café"]);
        assert!(a.terms(&"a".repeat(41)).is_empty());
    }
}
