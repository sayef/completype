use std::cmp::Ordering;
use std::collections::HashMap;

mod pruned;
use pruned::Term;

const K1: f64 = 1.2;
const B: f64 = 0.75;

#[derive(Default)]
pub struct Builder {
    ids: Vec<String>,
    lengths: Vec<usize>,
    postings: HashMap<String, Vec<Posting>>,
}

struct Posting {
    doc: u32,
    tf: u32,
}

impl Posting {
    fn doc(&self) -> usize {
        self.doc as usize
    }
}

pub struct Index {
    ids: Vec<String>,
    norm: Vec<f64>,
    terms: HashMap<String, Term>,
}

#[derive(Debug, PartialEq)]
pub struct Hit<'a> {
    pub id: &'a str,
    pub score: f64,
}

impl Builder {
    pub fn add<'a>(&mut self, id: String, terms: impl IntoIterator<Item = &'a str>) {
        let doc = u32::try_from(self.ids.len()).expect("document ID fits posting layout");
        self.ids.push(id);
        let mut counts: HashMap<&str, usize> = HashMap::new();
        let mut length = 0;
        for term in terms {
            *counts.entry(term).or_insert(0) += 1;
            length += 1;
        }
        self.lengths.push(length);
        for (term, tf) in counts {
            let tf = u32::try_from(tf).expect("term frequency fits u32");
            match self.postings.get_mut(term) {
                Some(list) => list.push(Posting { doc, tf }),
                None => {
                    self.postings
                        .insert(term.to_owned(), vec![Posting { doc, tf }]);
                }
            }
        }
    }

    pub fn finish(self) -> Index {
        let total = self.lengths.iter().sum::<usize>();
        let scale = if total == 0 {
            0.0
        } else {
            B * self.ids.len() as f64 / total as f64
        };
        let norm = self
            .lengths
            .into_iter()
            .map(|len| K1 * (1.0 - B + len as f64 * scale))
            .collect::<Vec<_>>();
        let terms = self
            .postings
            .into_iter()
            .map(|(term, postings)| (term, Term::new(postings, &norm)))
            .collect();
        Index {
            ids: self.ids,
            norm,
            terms,
        }
    }
}

impl Index {
    fn numerator(&self, frequency: usize, weight: f64) -> f64 {
        let df = frequency as f64;
        let idf = ((self.ids.len() as f64 - df + 0.5) / (df + 0.5)).ln_1p();
        weight * idf * (K1 + 1.0)
    }

    #[doc(hidden)]
    pub fn search_exhaustive<'a>(
        &self,
        weighted_terms: impl IntoIterator<Item = (&'a str, f64)>,
        limit: usize,
    ) -> Vec<Hit<'_>> {
        if limit == 0 {
            return Vec::new();
        }
        let mut scores = vec![0.0; self.ids.len()];
        for (term, weight) in weighted_terms {
            let Some(term) = self.terms.get(term) else {
                continue;
            };
            let list = &term.postings;
            let numerator = self.numerator(list.len(), weight);
            for posting in list {
                let doc = posting.doc();
                let tf = posting.tf as f64;
                scores[doc] += numerator * tf / (tf + self.norm[doc]);
            }
        }
        let mut ranked: Vec<_> = scores
            .into_iter()
            .enumerate()
            .filter(|&(_, score)| score > 0.0)
            .collect();
        fn rank(a: &(usize, f64), b: &(usize, f64)) -> Ordering {
            b.1.total_cmp(&a.1).then(a.0.cmp(&b.0))
        }
        if ranked.len() > limit {
            ranked.select_nth_unstable_by(limit, rank);
            ranked.truncate(limit);
        }
        ranked.sort_unstable_by(rank);
        ranked
            .into_iter()
            .map(|(doc, score)| Hit {
                id: &self.ids[doc],
                score,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bm25_matches_the_formula_and_breaks_top_k_ties_by_document_order() {
        let mut builder = Builder::default();
        builder.add("first".into(), ["apple", "apple"]);
        builder.add("second".into(), ["apple", "apple"]);
        builder.add("mixed".into(), ["apple", "pear"]);
        builder.add("empty".into(), []);
        let index = builder.finish();

        let hits = index.search([("apple", 1.0)], 1);
        assert_eq!(hits[0].id, "first");
        let idf = (1.5_f64 / 3.5).ln_1p();
        let expected = idf * 2.2 * 2.0 / (2.0 + 1.2 * (0.25 + 0.75 * 2.0 / 1.5));
        assert!((hits[0].score - expected).abs() < 1e-12);
        let mixed = index.search([("apple", 1.0), ("pear", 0.5)], 3);
        assert_eq!(
            mixed.iter().map(|hit| hit.id).collect::<Vec<_>>(),
            ["mixed", "first", "second"]
        );
    }

    #[test]
    fn matches_require_a_posting_and_a_positive_score() {
        let mut builder = Builder::default();
        builder.add("empty".into(), []);
        let index = builder.finish();
        assert!(index.search([("apple", 1.0)], 10).is_empty());
        assert!(index.search([], 10).is_empty());
        assert!(index.search([("apple", 1.0)], 0).is_empty());
    }

    #[test]
    fn block_max_matches_exhaustive_for_weighted_queries_across_blocks() {
        let words = [
            "common", "data", "title", "answer", "query", "rare", "unused",
        ];
        let mut random = 7_u64;
        let mut next = |len| {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            ((random >> 32) as usize) % len
        };
        let mut builder = Builder::default();
        for i in 0..2300 {
            let mut terms = vec!["common", "data"];
            if i % 3 == 0 {
                terms.extend((0..next(24)).map(|_| words[next(words.len() - 2)]));
            }
            if i % 197 == 0 {
                terms.extend(["rare"; 15]);
            }
            builder.add(i.to_string(), terms);
        }
        let index = builder.finish();
        for query in std::iter::once(vec![("common", 1.0)]).chain((0..150).map(|_| {
            let (len, weights) = (next(10), [0.7, 0.9, 1.0, 2.5]);
            (0..len)
                .map(|_| (words[next(words.len())], weights[next(weights.len())]))
                .collect()
        })) {
            for k in [1, 10, 100, 4000] {
                assert_eq!(
                    index.search(query.iter().copied(), k),
                    index.search_exhaustive(query.iter().copied(), k)
                );
            }
        }
        let negative = [("common", -1.0), ("rare", 1.0)];
        assert_eq!(
            index.search(negative, 10),
            index.search_exhaustive(negative, 10)
        );
    }
}
