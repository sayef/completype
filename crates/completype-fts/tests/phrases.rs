use std::collections::{BTreeSet, HashMap};

use completype_fts::{
    Analyzer, Document, Filter, Index, Language, Query, Reader, Reordering, SearchHit, Searcher,
    Segment, SegmentBuilder, StopWords,
};

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "completype-fts-phrase-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn ids(hits: &[SearchHit]) -> Vec<&str> {
    hits.iter().map(|h| h.id.as_str()).collect()
}

fn id_set(hits: &[SearchHit]) -> BTreeSet<&str> {
    hits.iter().map(|h| h.id.as_str()).collect()
}

fn build(index: &Index, docs: &[(&str, Document)]) -> Reader {
    let mut writer = index.writer().unwrap();
    for (id, doc) in docs {
        writer.add_document(id, doc);
    }
    writer.commit().unwrap();
    index.reader().unwrap()
}

fn body(text: &str) -> Document {
    Document::new().text("body", text)
}

#[test]
fn exact_phrases_match_words_in_order_and_outrank_scattered_ones() {
    let dir = temp("exact");
    let index = Index::open(&dir).unwrap().with_positions(true);
    let reader = build(
        &index,
        &[
            ("a", body("pizza in new york tonight")),
            ("b", body("york is new to pizza")),
            ("c", body("new jersey and york")),
            ("d", body("new york new york pizza")),
        ],
    );
    let hits = reader.query(&Query::new(r#""new york""#));
    assert_eq!(id_set(&hits), BTreeSet::from(["a", "d"]));
    assert_eq!(ids(&hits)[0], "d", "two matches beat one");
    let hits = reader.query(&Query::new(r#""new york" pizza"#).limit(4));
    assert_eq!(ids(&hits)[..2], ["d", "a"]);
    assert_eq!(id_set(&hits), BTreeSet::from(["a", "b", "d"]));
    let built = reader.query(&Query::new("pizza").phrase("New York!").limit(4));
    assert_eq!(built, hits, "the builder equals the quoted form");
    assert!(reader.query(&Query::new(r#""pizza new""#)).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn slop_allows_gaps_and_transpositions_at_a_lower_frequency() {
    let dir = temp("slop");
    let index = Index::open(&dir).unwrap().with_positions(true);
    let reader = build(
        &index,
        &[
            ("gap", body("new big york")),
            ("swap", body("york new")),
            ("far", body("new a b c york")),
        ],
    );
    assert!(reader.query(&Query::new(r#""new york""#)).is_empty());
    assert_eq!(ids(&reader.query(&Query::new(r#""new york"~1"#))), ["gap"]);
    let two = reader.query(&Query::new("").phrase_slop("new york", 2));
    assert_eq!(
        ids(&two),
        ["gap", "swap"],
        "a gap is width 1, a swap width 2"
    );
    let wide = reader.query(&Query::new(r#""new york"~3"#));
    assert_eq!(id_set(&wide), BTreeSet::from(["gap", "swap", "far"]));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn repeated_words_need_their_own_positions() {
    let dir = temp("repeat");
    let index = Index::open(&dir).unwrap().with_positions(true);
    let reader = build(
        &index,
        &[
            ("twice", body("a b a b a")),
            ("once", body("a b a")),
            ("short", body("a b")),
            ("pair", body("a a")),
        ],
    );
    let hits = reader.query(&Query::new(r#""a b a""#));
    assert_eq!(ids(&hits), ["twice", "once"]);
    assert_eq!(ids(&reader.query(&Query::new(r#""a a""#))), ["pair"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn phrases_do_not_cross_fields_or_values_of_one_field() {
    let dir = temp("fields");
    let index = Index::open(&dir).unwrap().with_positions(true);
    let reader = build(
        &index,
        &[
            (
                "split",
                Document::new().text("title", "new").text("body", "york"),
            ),
            (
                "values",
                Document::new().text("body", "new").text("body", "york"),
            ),
            (
                "title",
                Document::new().text("title", "new york").text("body", "x"),
            ),
        ],
    );
    assert_eq!(ids(&reader.query(&Query::new(r#""new york""#))), ["title"]);
    let body_only = Query::new(r#""new york"~50"#).field("body", 1.0);
    assert!(reader.query(&body_only).is_empty());
    let boosted = reader.query(&Query::new(r#""new york""#).field("title", 3.0));
    let plain = reader.query(&Query::new(r#""new york""#).field("title", 1.0));
    assert!((boosted[0].score - 3.0 * plain[0].score).abs() < 1e-4);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stop_words_stay_in_phrases_and_stemming_applies_to_them() {
    let dir = temp("language");
    let english = Analyzer::default().stem(Language::English);
    let index = Index::open(&dir)
        .unwrap()
        .with_analyzer(english)
        .with_stop_words(StopWords::snowball(Language::English).unwrap())
        .with_positions(true);
    let reader = build(
        &index,
        &[
            ("band", body("The Who played loudly")),
            ("question", body("who was the singer")),
            ("dogs", body("running dogs bark")),
            ("runner", body("the dog was running")),
        ],
    );
    assert_eq!(ids(&reader.query(&Query::new(r#""the who""#))), ["band"]);
    assert_eq!(ids(&reader.query(&Query::new(r#""run dog""#))), ["dogs"]);
    assert_eq!(ids(&reader.query(&Query::new(r#""Runs DOG""#))), ["dogs"]);
    let hits = reader.query(&Query::new(r#"the "dog was""#).limit(4));
    assert_eq!(
        ids(&hits),
        ["runner"],
        "a stop word beside a phrase is still dropped"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn phrase_words_are_not_spelling_corrected() {
    let dir = temp("typos");
    let index = Index::open(&dir).unwrap().with_positions(true);
    let docs: Vec<(String, Document)> = (0..40)
        .map(|i| (format!("d{i}"), body("hello world again")))
        .collect();
    let docs: Vec<(&str, Document)> = docs.iter().map(|(i, d)| (i.as_str(), d.clone())).collect();
    let reader = build(&index, &docs);
    assert_eq!(reader.query(&Query::new("helo")).len(), 10);
    assert!(reader.query(&Query::new(r#""helo world""#)).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn deleted_and_filtered_documents_never_match_phrases() {
    let dir = temp("deletes");
    let index = Index::open(&dir).unwrap().with_positions(true);
    let doc = |text: &str, colour: &str| body(text).keyword("colour", colour);
    build(
        &index,
        &[
            ("red", doc("new york", "red")),
            ("blue", doc("new york", "blue")),
            ("gone", doc("new york", "red")),
        ],
    );
    let mut writer = index.writer().unwrap();
    writer.delete("gone");
    writer.commit().unwrap();
    drop(writer);
    let reader = index.reader().unwrap();
    let all = reader.query(&Query::new(r#""new york""#));
    assert_eq!(id_set(&all), BTreeSet::from(["red", "blue"]));
    let red = reader.query(&Query::new(r#""new york""#).filter(Filter::keyword("colour", "red")));
    assert_eq!(ids(&red), ["red"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_index_without_positions_refuses_phrases() {
    let dir = temp("off");
    let index = Index::open(&dir).unwrap();
    let reader = build(&index, &[("a", body("new york"))]);
    assert!(!reader.has_positions());
    let query = Query::new(r#""new york" pizza"#);
    assert!(reader.try_query(&query).is_err());
    assert!(
        reader.query(&query).is_empty(),
        "phrases match nothing there"
    );
    assert_eq!(reader.try_query(&Query::new(r#""york""#)).unwrap().len(), 1);
    assert!(Index::open(&dir)
        .unwrap()
        .with_positions(true)
        .writer()
        .is_err());
    assert!(!Index::open(&dir).unwrap().positions());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn version_4_segments_still_open_and_have_no_positions() {
    let dir = temp("v4");
    std::fs::create_dir_all(&dir).unwrap();
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v4-index");
    for entry in std::fs::read_dir(fixture).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.join(entry.file_name())).unwrap();
    }
    let index = Index::open(&dir).unwrap();
    let reader = index.reader().unwrap();
    assert_eq!(reader.len(), 3);
    assert!(!reader.has_positions());
    assert_eq!(
        id_set(&reader.query(&Query::new("jersey"))),
        BTreeSet::from(["b"])
    );
    assert!(reader.try_query(&Query::new(r#""new york""#)).is_err());
    let plain = reader.query(&Query::new("pizza"));
    assert_eq!(plain.len(), 2);

    let mut writer = index.writer().unwrap();
    writer.add_document("d", &body("new york deli"));
    writer.commit().unwrap();
    writer.merge().unwrap();
    drop(writer);
    let reader = index.reader().unwrap();
    assert_eq!((reader.segment_count(), reader.len()), (1, 4));
    assert_eq!(ids(&reader.query(&Query::new("pizza"))), ids(&plain));
    let _ = std::fs::remove_dir_all(&dir);
}

struct Random(u64);

impl Random {
    fn below(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) % n
    }

    fn words(&mut self, max: u64, vocabulary: u64) -> Vec<String> {
        let n = self.below(max + 1);
        (0..n)
            .map(|_| {
                let r = self.below(vocabulary);
                format!("w{}", self.below(r + 1))
            })
            .collect()
    }
}

/// Whether `words` occur next to each other, in order, within one value.
fn contains(values: &[Vec<String>], words: &[String]) -> bool {
    values
        .iter()
        .any(|v| v.windows(words.len()).any(|w| w == words))
}

#[test]
fn pruned_phrase_search_over_segments_deletes_merges_and_reordering_equals_exhaustive() {
    for (seed, reordering) in [(3, Reordering::Never), (11, Reordering::Always)] {
        let dir = temp(&format!("exact-{seed}"));
        let index = Index::open(&dir)
            .unwrap()
            .with_positions(true)
            .with_reordering(reordering);
        let mut random = Random(seed);
        let mut live: HashMap<String, Vec<Vec<String>>> = HashMap::new();
        let mut writer = index.writer().unwrap();
        for commit in 0..6 {
            for _ in 0..400 {
                let id = format!("d{}", random.below(1500));
                if random.below(8) == 0 {
                    writer.delete(&id);
                    live.remove(&id);
                    continue;
                }
                let mut values = vec![random.words(30, 40)];
                if random.below(4) == 0 {
                    values.push(random.words(6, 40));
                }
                let mut doc = Document::new().text("title", random.words(4, 40).join(" "));
                for v in &values {
                    doc = doc.text("body", v.join(" "));
                }
                writer.add_document(&id, &doc);
                live.insert(id, values);
            }
            writer.commit().unwrap();
            if commit == 3 {
                writer.merge().unwrap();
            }
        }
        drop(writer);
        let reader = index.reader().unwrap();
        assert!(reader.segment_count() > 1 && reader.has_positions());
        let mut sampled: Vec<&Vec<Vec<String>>> = live.values().collect();
        sampled.sort();
        for q in 0..60 {
            let values = sampled[random.below(sampled.len() as u64) as usize];
            let value = &values[0];
            if value.len() < 3 {
                continue;
            }
            let len = 2 + random.below(2) as usize;
            let start = random.below((value.len() - len + 1) as u64) as usize;
            let words = &value[start..start + len];
            let slop = [0, 0, 1, 3][q % 4];
            let phrase = words.join(" ");
            if slop == 0 {
                let only = Query::new("")
                    .phrase(&phrase)
                    .field("body", 1.0)
                    .limit(5000);
                let found: BTreeSet<String> =
                    reader.query(&only).into_iter().map(|h| h.id).collect();
                let expected: BTreeSet<String> = live
                    .iter()
                    .filter(|(_, v)| contains(v, words))
                    .map(|(id, _)| id.clone())
                    .collect();
                assert_eq!(found, expected, "phrase {phrase:?}");
            }
            let extra = random.words(2, 40).join(" ");
            for k in [1, 10, 100] {
                let query = Query::new(format!("{extra} \"{phrase}\"~{slop}"))
                    .field("body", 1.0)
                    .field("title", 2.0)
                    .exact()
                    .limit(k);
                assert_eq!(
                    reader.query(&query),
                    reader.query_exhaustive(&query),
                    "{query:?}"
                );
                let filtered = query.filter(Filter::keyword("none", "x").not());
                assert_eq!(reader.query(&filtered), reader.query_exhaustive(&filtered));
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn searcher_phrases_over_reordered_segments_equal_exhaustive_scoring() {
    let mut random = Random(5);
    let docs: Vec<Vec<String>> = (0..3000).map(|_| random.words(40, 60)).collect();
    for reorder in [false, true] {
        let mut builder = SegmentBuilder::default();
        builder.positions(true).reorder(reorder);
        for (i, doc) in docs.iter().enumerate() {
            builder.add(format!("d{i}"), doc.iter().map(String::as_str));
        }
        let segment = Segment::from_bytes(builder.finish()).unwrap();
        assert!(segment.has_positions());
        let mut searcher = Searcher::new(&segment);
        for q in 0..80 {
            let doc = &docs[random.below(docs.len() as u64) as usize];
            if doc.len() < 4 {
                continue;
            }
            let start = random.below(doc.len() as u64 - 3) as usize;
            let words: Vec<&str> = doc[start..start + 2 + q % 3]
                .iter()
                .map(String::as_str)
                .collect();
            let phrases = [(words.as_slice(), (q % 3) as u32, 1.5)];
            let extra = random.words(3, 60);
            let terms: Vec<(&str, f32)> = extra.iter().map(|w| (w.as_str(), 1.0)).collect();
            for k in [1, 10, 100] {
                let pruned = searcher.search_phrases(terms.iter().copied(), &phrases, k);
                let exhaustive =
                    searcher.search_phrases_exhaustive(terms.iter().copied(), &phrases, k);
                assert_eq!(pruned, exhaustive, "{words:?}");
                let alone = searcher.search_phrases([], &phrases, k);
                assert!(!alone.is_empty());
                assert_eq!(alone, searcher.search_phrases_exhaustive([], &phrases, k));
            }
        }
    }
}

#[test]
fn positions_leave_postings_untouched() {
    let mut random = Random(9);
    let docs: Vec<Vec<String>> = (0..2000).map(|_| random.words(30, 50)).collect();
    let segment = |positions: bool| {
        let mut builder = SegmentBuilder::default();
        builder.positions(positions);
        for (i, doc) in docs.iter().enumerate() {
            builder.add(format!("d{i}"), doc.iter().map(String::as_str));
        }
        Segment::from_bytes(builder.finish()).unwrap()
    };
    let (with, without) = (segment(true), segment(false));
    for ((name, a), (_, b)) in with.section_sizes().iter().zip(without.section_sizes()) {
        if !name.starts_with("position") {
            assert_eq!(*a, b, "{name}");
        } else {
            assert!(*a > 0 && b == 0, "{name}");
        }
    }
    let query = [("w1", 1.0), ("w3", 0.5)];
    assert_eq!(
        Searcher::new(&with).search(query, 10),
        Searcher::new(&without).search(query, 10)
    );
}
