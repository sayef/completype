use std::io::ErrorKind;

use completype_fts::{Analyzer, Index, Language, Query, Reader, SearchHit, StopWords};

fn temp(name: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("completype-fts-lang-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn ids(hits: &[SearchHit]) -> Vec<&str> {
    let mut ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
    ids.sort_unstable();
    ids
}

fn english() -> Analyzer {
    Analyzer::default().stem(Language::English)
}

fn snowball_english() -> StopWords {
    StopWords::snowball(Language::English).unwrap()
}

fn build(index: &Index, docs: &[(&str, &str)]) {
    let mut writer = index.writer().unwrap();
    for (id, text) in docs {
        writer.add(id, text);
    }
    writer.commit().unwrap();
}

#[test]
fn every_language_stems_and_snowball_lists_parse() {
    for &language in Language::ALL {
        assert_eq!(Language::from_name(language.name()), Some(language));
        assert_eq!(
            Analyzer::default().stem(language).terms("x").len(),
            1,
            "{language}"
        );
    }
    let with_lists = Language::ALL
        .iter()
        .filter_map(|&l| StopWords::snowball(l))
        .inspect(|list| assert!(list.words().len() > 20))
        .count();
    assert_eq!(with_lists, 17);
    let english = snowball_english();
    assert!(english.words().iter().any(|w| w == "what"));
    assert!(!english.words().iter().any(|w| w.contains('|')));
}

#[test]
fn stemming_applies_at_index_and_query_time() {
    let dir = temp("stem");
    let index = Index::open(&dir).unwrap().with_analyzer(english());
    build(
        &index,
        &[
            ("1", "The runners were running fast"),
            ("2", "One quick run"),
            ("3", "Dogs barking"),
        ],
    );
    let reader = index.reader().unwrap();
    assert_eq!(reader.document_frequency("run"), 2);
    assert_eq!(reader.document_frequency("running"), 0);
    assert_eq!(ids(&reader.search("runs", 10)), ["1", "2"]);
    assert_eq!(ids(&reader.search_exact("RUNNING", 10)), ["1", "2"]);
    assert_eq!(ids(&reader.search("dog barked", 10)), ["3"]);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_analyzer_persists_across_reopening_and_merging() {
    let dir = temp("persist");
    {
        let index = Index::open(&dir).unwrap().with_analyzer(english());
        build(&index, &[("1", "connected devices")]);
        build(&index, &[("2", "connecting cables")]);
        index.writer().unwrap().merge().unwrap();
    }
    let index = Index::open(&dir).unwrap();
    assert_eq!(index.analyzer(), &english());
    let reader = index.reader().unwrap();
    assert_eq!(reader.segment_count(), 1);
    assert_eq!(ids(&reader.search("connection", 10)), ["1", "2"]);
    build(&index, &[("3", "connects")]);
    assert_eq!(index.reader().unwrap().document_frequency("connect"), 3);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_different_analyzer_is_refused_once_documents_are_committed() {
    let dir = temp("mismatch");
    let fresh = Index::open(&dir).unwrap();
    assert!(fresh.clone().with_analyzer(english()).reader().is_ok());
    build(&fresh.with_analyzer(english()), &[("1", "running")]);
    for other in [
        Analyzer::default(),
        english().keep_accents(),
        Analyzer::default().stem(Language::German),
    ] {
        let index = Index::open(&dir).unwrap().with_analyzer(other);
        for error in [index.reader().err(), index.writer().err()] {
            let error = error.expect("a mismatched analyzer is refused");
            assert_eq!(error.kind(), ErrorKind::InvalidInput);
            assert!(error.to_string().contains("stem=english"), "{error}");
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn an_index_from_before_the_analyzer_was_recorded_still_opens() {
    let dir = temp("v1");
    build(&Index::open(&dir).unwrap(), &[("1", "plain words")]);
    let manifest = dir.join("manifest");
    let text = std::fs::read_to_string(&manifest).unwrap();
    let mut lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.remove(1), "analyzer accents=fold stem=none");
    lines[0] = "completype-fts 1";
    std::fs::write(&manifest, lines.join("\n")).unwrap();
    let reader = Index::open(&dir).unwrap().reader().unwrap();
    assert_eq!(ids(&reader.search("words", 10)), ["1"]);
    std::fs::write(&manifest, "completype-fts 2\nanalyzer stem=klingon\n").unwrap();
    assert_eq!(
        Index::open(&dir).err().unwrap().kind(),
        ErrorKind::InvalidData
    );
    std::fs::remove_dir_all(dir).unwrap();
}

/// A corpus where "what" is far more common than "covid" is rare, as in a question-heavy set.
fn questions(index: &Index) {
    let mut docs: Vec<(String, String)> = (0..60)
        .map(|d| (format!("q{d}"), format!("what is the answer number {d}")))
        .collect();
    docs.push(("c1".into(), "covid transmission in schools".into()));
    docs.push(("c2".into(), "what covid does to the lungs".into()));
    docs.push(("w".into(), "the who".into()));
    let docs: Vec<(&str, &str)> = docs.iter().map(|(i, t)| (i.as_str(), t.as_str())).collect();
    build(index, &docs);
}

fn scores(reader: &Reader, text: &str) -> Vec<(String, f32)> {
    let mut hits: Vec<(String, f32)> = reader
        .query(&Query::new(text).limit(100))
        .into_iter()
        .map(|h| (h.id, h.score))
        .collect();
    hits.sort_by(|a, b| a.0.cmp(&b.0));
    hits
}

#[test]
fn stop_words_leave_queries_but_stay_in_the_index() {
    let dir = temp("stop");
    let index = Index::open(&dir)
        .unwrap()
        .with_stop_words(snowball_english().with(["number"]));
    questions(&index);
    let plain = Index::open(&dir).unwrap().reader().unwrap();
    let reader = index.reader().unwrap();
    assert_eq!(reader.document_frequency("the"), 62);
    assert_eq!(scores(&reader, "What is COVID"), scores(&plain, "covid"));
    assert_eq!(scores(&reader, "number covid"), scores(&plain, "covid"));
    assert_eq!(
        ids(&reader.search_exact("what covid", 10)),
        ids(&plain.search_exact("covid", 10))
    );
    assert!(ids(&plain.search("what covid", 3)).contains(&"c2"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_query_of_only_stop_words_keeps_them() {
    let dir = temp("all-stop");
    let index = Index::open(&dir)
        .unwrap()
        .with_stop_words(snowball_english());
    questions(&index);
    let plain = Index::open(&dir).unwrap().reader().unwrap();
    let reader = index.reader().unwrap();
    assert_eq!(scores(&reader, "The Who"), scores(&plain, "the who"));
    assert_eq!(reader.search("the who", 1)[0].id, "w");
    assert_eq!(scores(&reader, "waht"), scores(&plain, "waht"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_correction_to_a_stop_word_is_dropped() {
    let dir = temp("correct-stop");
    let index = Index::open(&dir)
        .unwrap()
        .with_stop_words(snowball_english());
    questions(&index);
    let plain = Index::open(&dir).unwrap().reader().unwrap();
    let reader = index.reader().unwrap();
    assert_eq!(plain.spellings("waht"), [("what".to_owned(), 0.9)]);
    assert_eq!(scores(&reader, "waht covid"), scores(&plain, "covid"));
    assert_ne!(scores(&plain, "waht covid"), scores(&plain, "covid"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn stemmed_terms_are_corrected_against_stemmed_terms() {
    let dir = temp("stem-typo");
    let index = Index::open(&dir)
        .unwrap()
        .with_analyzer(english())
        .with_stop_words(snowball_english());
    let mut docs: Vec<(String, String)> = (0..30)
        .map(|d| (format!("c{d}"), format!("connecting cables {d}")))
        .collect();
    docs.push(("x".into(), "unrelated text".into()));
    let docs: Vec<(&str, &str)> = docs.iter().map(|(i, t)| (i.as_str(), t.as_str())).collect();
    build(&index, &docs);
    let reader = index.reader().unwrap();
    let typo = english().terms("conection");
    assert_eq!(typo, ["conect"]);
    assert_eq!(reader.spellings("conect"), [("connect".to_owned(), 0.9)]);
    assert_eq!(reader.search("the conections", 50).len(), 30);
    assert!(reader.search_exact("conection", 5).is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

struct Random(u64);

impl Random {
    fn below(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) % n as u64) as usize
    }
}

const WORDS: &[&str] = &[
    "the",
    "a",
    "what",
    "is",
    "of",
    "and",
    "not",
    "connect",
    "connected",
    "connecting",
    "connection",
    "run",
    "running",
    "runs",
    "runner",
    "study",
    "studies",
    "studied",
    "virus",
    "viruses",
    "viral",
    "infection",
    "infected",
    "school",
    "schools",
    "children",
    "child",
    "vaccine",
    "vaccines",
    "vaccinated",
    "mask",
    "masks",
    "masking",
    "test",
    "tests",
    "tested",
    "testing",
    "spread",
    "spreading",
    "lung",
    "lungs",
    "cell",
    "cells",
    "protein",
    "proteins",
];

#[test]
fn pruned_search_with_stemming_and_stop_words_equals_exhaustive() {
    let dir = temp("exact");
    let index = Index::open(&dir)
        .unwrap()
        .with_analyzer(english())
        .with_stop_words(snowball_english());
    let mut random = Random(11);
    let mut writer = index.writer().unwrap();
    for batch in 0..3 {
        for d in 0..1500 {
            let n = 1 + random.below(30);
            let text: Vec<&str> = (0..n)
                .map(|_| {
                    let skew = 1 + random.below(WORDS.len());
                    WORDS[random.below(skew)]
                })
                .collect();
            writer.add(&format!("{batch}-{d}"), &text.join(" "));
        }
        for _ in 0..50 {
            writer.delete(&format!("{batch}-{}", random.below(1500)));
        }
        writer.commit().unwrap();
    }
    drop(writer);
    let reader = index.reader().unwrap();
    for q in 0..200 {
        let n = 1 + random.below(4);
        let mut text: Vec<String> = (0..n)
            .map(|_| WORDS[random.below(WORDS.len())].to_owned())
            .collect();
        if q % 3 == 0 {
            text[0].insert(1, 'x');
        }
        for k in [1, 10, 100] {
            let query = Query::new(text.join(" ")).limit(k);
            assert_eq!(
                reader.query(&query),
                reader.query_exhaustive(&query),
                "{query:?}"
            );
            let exact = query.clone().exact();
            assert_eq!(reader.query(&exact), reader.query_exhaustive(&exact));
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}
