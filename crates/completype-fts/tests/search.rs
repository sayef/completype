use completype_fts::{Searcher, Segment, SegmentBuilder};

struct Random(u64);

impl Random {
    fn below(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % n
    }
}

fn corpus(docs: usize, seed: u64) -> (Segment, Vec<String>) {
    corpus_with(docs, seed, false)
}

fn corpus_with(docs: usize, seed: u64, reorder: bool) -> (Segment, Vec<String>) {
    let mut random = Random(seed);
    let vocabulary: Vec<String> = (0..400).map(|i| format!("w{i}")).collect();
    let mut builder = SegmentBuilder::default();
    builder.reorder(reorder);
    for d in 0..docs {
        let len = 1 + random.below(60);
        // Zipf-like: low ranks are frequent, so lists range from dense to single postings.
        let tokens: Vec<&str> = (0..len)
            .map(|_| {
                let r = random.below(vocabulary.len());
                vocabulary[random.below(r + 1)].as_str()
            })
            .collect();
        builder.add(format!("doc{d}"), tokens);
    }
    (Segment::from_bytes(builder.finish()).unwrap(), vocabulary)
}

#[test]
fn pruned_search_equals_exhaustive_scoring() {
    for (docs, seed, reorder) in [
        (1, 1, false),
        (130, 2, true),
        (5_000, 3, false),
        (5_000, 3, true),
        (40_000, 4, true),
    ] {
        let (segment, vocabulary) = corpus_with(docs, seed, reorder);
        let mut searcher = Searcher::new(&segment);
        let mut random = Random(seed * 31);
        for _ in 0..300 {
            let query: Vec<(&str, f32)> = (0..1 + random.below(12))
                .map(|_| {
                    (
                        vocabulary[random.below(vocabulary.len())].as_str(),
                        [0.5, 1.0, 2.5][random.below(3)],
                    )
                })
                .collect();
            for k in [1, 10, 100, 1000] {
                let pruned = searcher.search(query.iter().copied(), k);
                let exhaustive = searcher.search_exhaustive(query.iter().copied(), k);
                assert_eq!(pruned, exhaustive, "{docs} docs, k {k}, query {query:?}");
            }
        }
    }
}

#[test]
fn bm25_uses_lucene_document_lengths() {
    let mut builder = SegmentBuilder::default();
    builder.add("a", ["apple", "apple"]);
    builder.add("b", ["apple", "pear"]);
    builder.add("c", ["pear"; 30]);
    let segment = Segment::from_bytes(builder.finish()).unwrap();
    let mut searcher = Searcher::new(&segment);
    let hits = searcher.search([("apple", 1.0)], 10);
    assert_eq!(
        hits.iter().map(|h| segment.id(h.doc)).collect::<Vec<_>>(),
        ["a", "b"]
    );
    let average = 34.0_f32 / 3.0;
    let idf = (1.0_f64 + (3.0 - 2.0 + 0.5) / (2.0 + 0.5)).ln() as f32;
    let expected = 2.2 * idf * 2.0 / (2.0 + 1.2 * (0.25 + 0.75 * 2.0 / average));
    assert!((hits[0].score - expected).abs() < 1e-6);
}

#[test]
fn unknown_terms_empty_queries_and_bad_weights_match_nothing() {
    let (segment, _) = corpus(300, 9);
    let mut searcher = Searcher::new(&segment);
    assert!(searcher.search([("missing", 1.0)], 10).is_empty());
    assert!(searcher.search([], 10).is_empty());
    assert!(searcher.search([("w0", 1.0)], 0).is_empty());
    assert!(searcher
        .search([("w0", -1.0), ("w0", f32::NAN)], 10)
        .is_empty());
}

#[test]
fn a_written_segment_reopens_memory_mapped_with_the_same_results() {
    let mut builder = SegmentBuilder::default();
    for d in 0..2_000 {
        builder.add(
            format!("doc{d}"),
            ["common", if d % 7 == 0 { "seven" } else { "other" }],
        );
    }
    let bytes = builder.finish();
    let path = std::env::temp_dir().join(format!("completype-fts-{}.seg", std::process::id()));
    std::fs::write(&path, &bytes).unwrap();
    let (owned, mapped) = (
        Segment::from_bytes(bytes).unwrap(),
        Segment::open(&path).unwrap(),
    );
    let query = [("common", 1.0), ("seven", 1.0)];
    let hits = Searcher::new(&mapped).search(query, 20);
    assert_eq!(hits, Searcher::new(&owned).search(query, 20));
    assert_eq!(mapped.id(hits[0].doc), "doc0");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn reordering_keeps_every_document_and_its_scores() {
    let (plain, vocabulary) = corpus_with(3_000, 11, false);
    let (reordered, _) = corpus_with(3_000, 11, true);
    let (mut a, mut b) = (Searcher::new(&plain), Searcher::new(&reordered));
    for term in vocabulary.iter().take(50) {
        let mut x: Vec<_> = a
            .search([(term.as_str(), 1.0)], 5_000)
            .iter()
            .map(|h| (plain.id(h.doc), h.score))
            .collect();
        let mut y: Vec<_> = b
            .search([(term.as_str(), 1.0)], 5_000)
            .iter()
            .map(|h| (reordered.id(h.doc), h.score))
            .collect();
        x.sort_by(|p, q| p.0.cmp(q.0));
        y.sort_by(|p, q| p.0.cmp(q.0));
        assert_eq!(x, y);
    }
}

#[test]
fn huge_term_frequencies_are_capped_consistently() {
    let mut builder = SegmentBuilder::default();
    let many = vec!["spam"; 70_000];
    builder.add("long", many.iter().copied());
    builder.add("short", ["spam", "ham"]);
    for d in 0..300 {
        builder.add(format!("filler{d}"), ["ham", "spam"]);
    }
    let segment = Segment::from_bytes(builder.finish()).unwrap();
    let mut searcher = Searcher::new(&segment);
    let pruned = searcher.search([("spam", 1.0)], 3);
    assert_eq!(pruned, searcher.search_exhaustive([("spam", 1.0)], 3));
}

#[test]
fn reordering_a_corpus_large_enough_to_run_in_parallel_keeps_every_document() {
    let mut builder = SegmentBuilder::default();
    builder.reorder(true);
    let docs = 80_000;
    for d in 0..docs {
        builder.add(
            format!("d{d}"),
            [
                format!("a{}", d % 97),
                format!("b{}", d % 389),
                "all".to_owned(),
            ]
            .iter()
            .map(String::as_str),
        );
    }
    let segment = Segment::from_bytes(builder.finish()).unwrap();
    let mut seen: Vec<bool> = vec![false; docs];
    for doc in 0..segment.len() as u32 {
        seen[segment.id(doc)[1..].parse::<usize>().unwrap()] = true;
    }
    assert!(seen.iter().all(|&s| s));
    let mut searcher = Searcher::new(&segment);
    assert_eq!(
        searcher.search([("a5", 1.0), ("b7", 1.0)], 50),
        searcher.search_exhaustive([("a5", 1.0), ("b7", 1.0)], 50)
    );
}
