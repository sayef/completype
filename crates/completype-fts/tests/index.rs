use completype_fts::{Index, Reordering, SearchHit};

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("completype-fts-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn ids(hits: &[SearchHit]) -> Vec<&str> {
    hits.iter().map(|h| h.id.as_str()).collect()
}

#[test]
fn commits_replacements_and_deletes_are_visible_to_new_readers() {
    let dir = temp("lifecycle");
    let index = Index::open(&dir).unwrap();
    let mut writer = index.writer().unwrap();
    writer.add("1", "The quick brown fox");
    writer.add("2", "A lazy brown dog");
    writer.add("3", "Brown bread");
    writer.commit().unwrap();
    let before = index.reader().unwrap();
    assert_eq!(before.len(), 3);

    writer.add("2", "A lazy grey dog");
    writer.delete("3");
    writer.add("4", "brown brown brown");
    writer.add("4", "only grey now");
    writer.commit().unwrap();
    let after = index.reader().unwrap();
    assert_eq!(after.len(), 3);
    assert_eq!(ids(&after.search("brown", 10)), ["1"]);
    assert_eq!(ids(&after.search("grey", 10)).len(), 2);
    assert_eq!(
        ids(&before.search("brown", 10)).len(),
        3,
        "an older reader keeps its snapshot"
    );
    assert!(index.writer().is_err(), "a second writer is refused");
    drop(writer);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn merging_keeps_live_documents_and_their_scores() {
    let dir = temp("merge");
    let index = Index::open(&dir).unwrap();
    let mut writer = index.writer().unwrap();
    for batch in 0..4 {
        for d in 0..300 {
            let text = format!(
                "common word{} batch{batch} {}",
                d % 17,
                if d % 5 == 0 { "rare" } else { "" }
            );
            writer.add(&format!("{batch}-{d}"), &text);
        }
        writer.delete(&format!("{batch}-0"));
        writer.commit().unwrap();
    }
    let before = index.reader().unwrap();
    assert_eq!(before.segment_count(), 4);
    let queries = ["rare common", "word3 batch2", "word16 rare batch0"];
    let expected: Vec<_> = queries.iter().map(|q| before.search(q, 10_000)).collect();
    writer.merge().unwrap();
    let after = index.reader().unwrap();
    assert_eq!(after.segment_count(), 1);
    assert_eq!(after.len(), before.len());
    for (q, want) in queries.iter().zip(expected) {
        let got = after.search(q, 10_000);
        let mut a: Vec<_> = got.iter().map(|h| (h.id.clone(), h.score)).collect();
        let mut b: Vec<_> = want.iter().map(|h| (h.id.clone(), h.score)).collect();
        a.sort_by(|x, y| x.0.cmp(&y.0));
        b.sort_by(|x, y| x.0.cmp(&y.0));
        assert_eq!(a.len(), b.len());
        for ((ia, sa), (ib, sb)) in a.iter().zip(&b) {
            assert_eq!(ia, ib);
            assert!((sa - sb).abs() <= 1e-4 * sb, "{q}: {sa} vs {sb}");
        }
    }
    drop(writer);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn many_commits_merge_automatically_and_reordering_changes_no_results() {
    let dir = temp("auto");
    let plain_dir = temp("auto-plain");
    let index = Index::open(&dir)
        .unwrap()
        .with_reordering(Reordering::Always);
    let plain = Index::open(&plain_dir)
        .unwrap()
        .with_reordering(Reordering::Never);
    let (mut w, mut p) = (index.writer().unwrap(), plain.writer().unwrap());
    for c in 0..14 {
        for d in 0..200 {
            let text = format!("alpha beta{} gamma{}", (c * 7 + d) % 13, d % 29);
            w.add(&format!("{c}/{d}"), &text);
            p.add(&format!("{c}/{d}"), &text);
        }
        w.commit().unwrap();
        p.commit().unwrap();
    }
    let (r, q) = (index.reader().unwrap(), plain.reader().unwrap());
    assert!(r.segment_count() <= 10);
    for text in ["beta3 gamma7", "alpha", "gamma28 beta12 alpha"] {
        let (a, b) = (r.search(text, 30), q.search(text, 30));
        assert_eq!(
            a.iter().map(|h| h.score).collect::<Vec<_>>(),
            b.iter().map(|h| h.score).collect::<Vec<_>>()
        );
    }
    drop((w, p));
    std::fs::remove_dir_all(dir).unwrap();
    std::fs::remove_dir_all(plain_dir).unwrap();
}

#[test]
fn pruned_search_over_segments_with_deletes_equals_exhaustive_scoring() {
    let dir = temp("exact");
    let index = Index::open(&dir).unwrap();
    let mut writer = index.writer().unwrap();
    let mut state = 17_u64;
    let mut next = |n: u64| {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 33) % n
    };
    // Segments of very different average lengths stress the bound correction.
    for (batch, max_len) in [(0, 5), (1, 80), (2, 20), (3, 200)] {
        for d in 0..1_500 {
            let len = 1 + next(max_len);
            let text: Vec<String> = (0..len)
                .map(|_| {
                    let r = next(300);
                    format!("t{}", next(1 + r))
                })
                .collect();
            writer.add(&format!("{batch}-{d}"), &text.join(" "));
            if next(10) == 0 {
                let b = next(batch + 1);
                writer.delete(&format!("{b}-{}", next(1_500)));
            }
        }
        writer.commit().unwrap();
    }
    let reader = index.reader().unwrap();
    assert_eq!(reader.segment_count(), 4);
    for _ in 0..200 {
        let terms: Vec<String> = (0..1 + next(8))
            .map(|_| {
                let r = next(300);
                format!("t{}", next(1 + r))
            })
            .collect();
        let query: Vec<(&str, f32)> = terms.iter().map(|t| (t.as_str(), 1.0)).collect();
        for k in [1, 10, 100] {
            assert_eq!(
                reader.search_terms(query.iter().copied(), k),
                reader.search_terms_exhaustive(query.iter().copied(), k),
                "{query:?} k {k}"
            );
        }
    }
    drop(writer);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn misspelled_queries_find_the_documents_they_meant() {
    let dir = temp("typos");
    let index = Index::open(&dir).unwrap();
    let mut writer = index.writer().unwrap();
    for d in 0..100 {
        writer.add(
            &format!("filler{d}"),
            "a recipe for bread with flour and water",
        );
    }
    writer.add("target", "Restaurant reviews in Zürich");
    writer.commit().unwrap();
    let reader = index.reader().unwrap();
    assert_eq!(ids(&reader.search("resturant zurich", 5)), ["target"]);
    assert_eq!(ids(&reader.search("rseTaurant", 5)), ["target"]);
    assert!(reader.search_exact("resturant", 5).is_empty());
    assert_eq!(reader.spellings("bread"), [("bread".to_owned(), 1.0)]);
    drop(writer);
    std::fs::remove_dir_all(dir).unwrap();
}
