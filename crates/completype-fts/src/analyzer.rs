use std::fmt;

use unicode_normalization::char::{decompose_canonical, is_combining_mark};

use crate::Language;

/// Splits text into lowercase terms: runs of letters and numbers, each CJK character on its own,
/// optionally stemmed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Analyzer {
    fold_accents: bool,
    max_len: usize,
    language: Option<Language>,
}

impl Default for Analyzer {
    fn default() -> Self {
        Self {
            fold_accents: true,
            max_len: 40,
            language: None,
        }
    }
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xac00..=0xd7af | 0xf900..=0xfaff | 0x20000..=0x2fa1f)
}

fn fold(word: &str, out: &mut String) {
    for c in word.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            decompose_canonical(c, |d| {
                if !is_combining_mark(d) {
                    out.push(d);
                }
            });
        }
    }
}

impl Analyzer {
    /// Keeps accents, so `café` and `cafe` stay different terms.
    pub fn keep_accents(mut self) -> Self {
        self.fold_accents = false;
        self
    }

    /// Reduces words to their Snowball stem, taken before accents are folded.
    pub fn stem(mut self, language: Language) -> Self {
        self.language = Some(language);
        self
    }

    pub fn language(&self) -> Option<Language> {
        self.language
    }

    /// Calls `emit` with each term of `text`, in order.
    pub fn analyze(&self, text: &str, mut emit: impl FnMut(&str)) {
        self.analyze_words(text, |_, term| emit(term));
    }

    /// Calls `emit` with each word before stemming and its term.
    pub(crate) fn analyze_words(&self, text: &str, mut emit: impl FnMut(&str, &str)) {
        let stemmer = self.language.map(|l| frostem::Stemmer::new(l.algorithm()));
        let (mut word, mut term) = (String::new(), String::new());
        let fold_into = |s: &str, out: &mut String| {
            out.clear();
            if self.fold_accents && !s.is_ascii() {
                fold(s, out);
            } else {
                out.push_str(s);
            }
        };
        let mut flush = |raw: &mut String, chars: &mut usize, cjk: bool| {
            if !raw.is_empty() && *chars <= self.max_len {
                fold_into(raw, &mut word);
                match stemmer.filter(|_| !cjk) {
                    Some(stemmer) => {
                        fold_into(&stemmer.stem(raw), &mut term);
                        emit(&word, &term);
                    }
                    None => emit(&word, &word),
                }
            }
            raw.clear();
            *chars = 0;
        };
        let mut raw = String::new();
        let mut chars = 0;
        for c in text.chars() {
            if is_cjk(c) {
                flush(&mut raw, &mut chars, false);
                raw.push(c);
                chars = 1;
                flush(&mut raw, &mut chars, true);
            } else if c.is_alphanumeric() {
                raw.extend(c.to_lowercase());
                chars += 1;
            } else if !(is_combining_mark(c) && !raw.is_empty()) {
                flush(&mut raw, &mut chars, false);
            } else {
                raw.push(c);
            }
        }
        flush(&mut raw, &mut chars, false);
    }

    pub fn terms(&self, text: &str) -> Vec<String> {
        let mut out = Vec::new();
        self.analyze(text, |t| out.push(t.to_owned()));
        out
    }

    /// The settings that change indexed terms, as an index manifest records them.
    pub(crate) fn spec(&self) -> String {
        let accents = if self.fold_accents { "fold" } else { "keep" };
        let stem = self.language.map_or("none", Language::name);
        format!("accents={accents} stem={stem}")
    }

    pub(crate) fn from_spec(spec: &str) -> Option<Self> {
        let mut analyzer = Self::default();
        for part in spec.split(' ') {
            match part.split_once('=')? {
                ("accents", "fold") | ("stem", "none") => {}
                ("accents", "keep") => analyzer.fold_accents = false,
                ("stem", name) => analyzer.language = Some(Language::from_name(name)?),
                _ => return None,
            }
        }
        Some(analyzer)
    }
}

impl fmt::Display for Analyzer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.spec())
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
        assert_eq!(
            Analyzer::default().keep_accents().terms("cafe\u{301}"),
            ["cafe\u{301}"]
        );
        assert!(a.terms(&"a".repeat(41)).is_empty());
    }

    #[test]
    fn stems_before_folding_and_round_trips_its_spec() {
        let english = Analyzer::default().stem(Language::English);
        assert_eq!(
            english.terms("Running dogs, 東京"),
            ["run", "dog", "東", "京"]
        );
        let french = Analyzer::default().stem(Language::French);
        assert_eq!(french.terms("Élégantes"), ["eleg"]);
        for a in [english, french.keep_accents(), Analyzer::default()] {
            assert_eq!(Analyzer::from_spec(&a.spec()), Some(a));
        }
        assert_eq!(Analyzer::from_spec("stem=klingon"), None);
    }
}
