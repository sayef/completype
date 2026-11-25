use std::collections::{BTreeSet, HashMap};

use completype_fts::{
    Document, Filter, Index, Query, Reader, Reordering, SearchHit, DEFAULT_FIELD,
};

fn temp(name: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("completype-fts-doc-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn ids(hits: &[SearchHit]) -> Vec<&str> {
    hits.iter().map(|h| h.id.as_str()).collect()
}

fn id_set(hits: &[SearchHit]) -> BTreeSet<String> {
    hits.iter().map(|h| h.id.clone()).collect()
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

    fn word(&mut self, vocabulary: u64) -> String {
        let r = self.below(vocabulary);
        format!("w{}", self.below(r + 1))
    }

    fn words(&mut self, max: u64, vocabulary: u64) -> String {
        let n = self.below(max + 1);
        (0..n)
            .map(|_| self.word(vocabulary))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// What a test expects a live document to hold.
#[derive(Clone, Debug)]
struct Model {
    brand: String,
    tags: Vec<String>,
    price: f64,
    stock: i64,
}

impl Model {
    fn matches(&self, filter: &Filter) -> bool {
        use std::ops::Bound;
        let within = |x: f64,
                      lower: &Bound<completype_fts::Number>,
                      upper: &Bound<completype_fts::Number>| {
            let n = |v: &completype_fts::Number| match v {
                completype_fts::Number::Int(i) => *i as f64,
                completype_fts::Number::Float(f) => *f,
            };
            let low = match lower {
                Bound::Included(v) => x >= n(v),
                Bound::Excluded(v) => x > n(v),
                Bound::Unbounded => true,
            };
            let high = match upper {
                Bound::Included(v) => x <= n(v),
                Bound::Excluded(v) => x < n(v),
                Bound::Unbounded => true,
            };
            low && high
        };
        match filter {
            Filter::Keyword { field, values } => match field.as_str() {
                "brand" => values.contains(&self.brand),
                "tags" => self.tags.iter().any(|t| values.contains(t)),
                _ => false,
            },
            Filter::Range {
                field,
                lower,
                upper,
            } => match field.as_str() {
                "price" => within(self.price, lower, upper),
                "stock" => within(self.stock as f64, lower, upper),
                _ => false,
            },
            Filter::And(all) => all.iter().all(|f| self.matches(f)),
            Filter::Or(any) => any.iter().any(|f| self.matches(f)),
            Filter::Not(f) => !self.matches(f),
        }
    }
}

fn random_filter(random: &mut Random, depth: u32) -> Filter {
    let choice = if depth == 0 {
        random.below(4)
    } else {
        random.below(7)
    };
    match choice {
        0 => Filter::keyword("brand", format!("b{}", random.below(6))),
        1 => Filter::keyword_in(
            "tags",
            (0..1 + random.below(3)).map(|_| format!("t{}", random.below(12))),
        ),
        2 => {
            let low = random.below(100) as f64 + 0.5;
            Filter::range("price", low..low + random.below(60) as f64)
        }
        3 => {
            let low = random.below(50) as i64;
            Filter::range("stock", low..=low + random.below(30) as i64)
        }
        4 => random_filter(random, depth - 1).and(random_filter(random, depth - 1)),
        5 => random_filter(random, depth - 1).or(random_filter(random, depth - 1)),
        _ => random_filter(random, depth - 1).not(),
    }
}

fn random_document(random: &mut Random) -> (Document, Model) {
    let model = Model {
        brand: format!("b{}", random.below(6)),
        tags: (0..random.below(4))
            .map(|_| format!("t{}", random.below(12)))
            .collect(),
        price: (random.below(16_000) as f64) / 100.0,
        stock: random.below(80) as i64,
    };
    let mut document = Document::new()
        .text("title", random.words(6, 150))
        .text("description", random.words(60, 300))
        .keyword("brand", model.brand.clone())
        .number("price", model.price)
        .number("stock", model.stock);
    if random.below(3) == 0 {
        document = document.text("aliases", random.words(4, 150));
    }
    for tag in &model.tags {
        document = document.keyword("tags", tag.clone());
    }
    (document, model)
}

/// Every live document a filter allows, from a query without terms.
fn filtered(reader: &Reader, filter: Filter) -> BTreeSet<String> {
    id_set(&reader.query(&Query::new("").filter(filter).limit(usize::MAX)))
}

#[test]
fn pruned_search_with_weighted_fields_filters_and_deletes_equals_exhaustive() {
    for reordering in [Reordering::Never, Reordering::Always] {
        let dir = temp(&format!("exact-{reordering:?}"));
        let index = Index::open(&dir).unwrap().with_reordering(reordering);
        let mut writer = index.writer().unwrap();
        let mut random = Random(5);
        let mut models: HashMap<String, Model> = HashMap::new();
        for batch in 0..4 {
            for d in 0..1_200 {
                let id = format!("{batch}-{d}");
                let (document, model) = random_document(&mut random);
                writer.add_document(&id, &document);
                models.insert(id, model);
                if random.below(8) == 0 {
                    let victim = format!("{}-{}", random.below(batch + 1), random.below(1_200));
                    writer.delete(&victim);
                    models.remove(&victim);
                }
                if random.below(12) == 0 {
                    let victim = format!("{}-{}", random.below(batch + 1), random.below(1_200));
                    let (document, model) = random_document(&mut random);
                    writer.add_document(&victim, &document);
                    models.insert(victim, model);
                }
            }
            writer.commit().unwrap();
        }
        let reader = index.reader().unwrap();
        assert_eq!(reader.segment_count(), 4);
        assert_eq!(reader.len(), models.len());
        let mut nonempty = 0;
        for round in 0..300 {
            let text = (0..1 + random.below(5))
                .map(|_| random.word(150))
                .collect::<Vec<_>>()
                .join(" ");
            let mut query = Query::new(text).exact();
            for (field, boost) in [("title", 3.0), ("aliases", 2.0), ("description", 0.7)] {
                if random.below(4) != 0 {
                    query = query.field(field, boost * (1.0 + random.below(3) as f32));
                }
            }
            let filter = (round % 4 != 0).then(|| random_filter(&mut random, 2));
            if let Some(filter) = &filter {
                query = query.filter(filter.clone());
            }
            for k in [1, 10, 100] {
                let query = query.clone().limit(k);
                let pruned = reader.query(&query);
                assert_eq!(pruned, reader.query_exhaustive(&query), "{query:?}");
                for hit in &pruned {
                    let model = &models[&hit.id];
                    assert!(
                        filter.as_ref().is_none_or(|f| model.matches(f)),
                        "{hit:?} {filter:?}"
                    );
                }
                nonempty += usize::from(!pruned.is_empty());
            }
        }
        assert!(nonempty > 400, "{nonempty}");
        for _ in 0..100 {
            let filter = random_filter(&mut random, 3);
            let expected: BTreeSet<String> = models
                .iter()
                .filter(|(_, m)| m.matches(&filter))
                .map(|(id, _)| id.clone())
                .collect();
            assert_eq!(filtered(&reader, filter.clone()), expected, "{filter:?}");
        }
        drop(writer);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

fn catalog(dir: &std::path::Path) -> Index {
    let index = Index::open(dir).unwrap();
    let mut writer = index.writer().unwrap();
    let products = [
        (
            "p1",
            "Apple iPhone 15",
            "apple",
            &["phone", "5g"][..],
            799.0,
            12,
        ),
        (
            "p2",
            "Samsung Galaxy S24",
            "samsung",
            &["phone", "5g"][..],
            699.5,
            0,
        ),
        (
            "p3",
            "Apple MacBook Air",
            "apple",
            &["laptop"][..],
            1099.0,
            4,
        ),
        (
            "p4",
            "Lenovo ThinkPad",
            "lenovo",
            &["laptop", "business"][..],
            949.99,
            30,
        ),
        ("p5", "Phone case", "generic", &[][..], 9.99, 500),
    ];
    for (i, (id, title, brand, tags, price, stock)) in products.into_iter().enumerate() {
        let mut document = Document::new()
            .text("title", title)
            .keyword("brand", brand)
            .number("price", price)
            .number("stock", stock);
        for tag in tags {
            document = document.keyword("tags", *tag);
        }
        writer.add_document(id, &document);
        if i == 2 {
            writer.commit().unwrap();
        }
    }
    writer.commit().unwrap();
    index
}

#[test]
fn filters_select_keywords_numeric_ranges_and_combinations() {
    let dir = temp("filters");
    let index = catalog(&dir);
    let reader = index.reader().unwrap();
    assert_eq!(reader.segment_count(), 2);
    let set = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>();
    let f = |filter: Filter| filtered(&reader, filter);

    assert_eq!(f(Filter::keyword("brand", "apple")), set(&["p1", "p3"]));
    assert_eq!(f(Filter::keyword("tags", "laptop")), set(&["p3", "p4"]));
    assert_eq!(
        f(Filter::keyword_in("brand", ["lenovo", "samsung", "nobody"])),
        set(&["p2", "p4"])
    );
    assert_eq!(f(Filter::range("price", 700.0..1000.0)), set(&["p1", "p4"]));
    assert_eq!(f(Filter::range("price", ..=699.5)), set(&["p2", "p5"]));
    assert_eq!(f(Filter::range("price", 700..)), set(&["p1", "p3", "p4"]));
    assert_eq!(f(Filter::range("stock", 1..=30)), set(&["p1", "p3", "p4"]));
    assert_eq!(f(Filter::range("stock", 0.5..12.5)), set(&["p1", "p3"]));
    assert_eq!(f(Filter::equals("stock", 0)), set(&["p2"]));
    assert_eq!(
        f(Filter::keyword("tags", "phone").and(Filter::range("stock", 1..))),
        set(&["p1"])
    );
    assert_eq!(
        f(Filter::keyword("brand", "apple").or(Filter::range("price", ..10))),
        set(&["p1", "p3", "p5"])
    );
    assert_eq!(
        f(Filter::keyword("tags", "phone").not()),
        set(&["p3", "p4", "p5"])
    );
    assert_eq!(
        f(Filter::keyword("brand", "apple")
            .not()
            .and(Filter::keyword("tags", "5g").not())),
        set(&["p4", "p5"])
    );

    for empty in [
        Filter::keyword("brand", "nobody"),
        Filter::keyword("colour", "red"),
        Filter::range("weight", 0..),
        Filter::range("price", 10.0..5.0),
        Filter::range("stock", i64::MAX..),
        Filter::range("price", f64::NAN..),
        Filter::Or(vec![]),
        Filter::keyword("brand", "apple").and(Filter::keyword("brand", "lenovo")),
    ] {
        assert!(f(empty.clone()).is_empty(), "{empty:?}");
        assert!(reader.query(&Query::new("apple").filter(empty)).is_empty());
    }
    assert_eq!(f(Filter::And(vec![])).len(), 5);

    let hits = reader.query(&Query::new("apple phone").filter(Filter::range("price", ..1000)));
    assert_eq!(id_set(&hits), set(&["p1", "p5"]));
    let hits = reader.query(&Query::new("phone").filter(Filter::keyword("brand", "generic").not()));
    assert!(hits.is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn boosts_weight_fields_and_default_field_documents_mix_with_named_ones() {
    let dir = temp("boosts");
    let index = Index::open(&dir).unwrap();
    let mut writer = index.writer().unwrap();
    for d in 0..50 {
        writer.add_document(
            &format!("filler{d}"),
            &Document::new()
                .text("title", "plain item")
                .text("description", "nothing to see"),
        );
    }
    writer.add_document(
        "in-title",
        &Document::new()
            .text("title", "Lantern")
            .text("description", "a camping light"),
    );
    writer.add_document(
        "in-description",
        &Document::new()
            .text("title", "Camping light")
            .text("description", "works like a lantern"),
    );
    writer.add("legacy", "an old lantern from Zürich");
    writer.commit().unwrap();
    let reader = index.reader().unwrap();
    let title_first = reader.query(
        &Query::new("lantern")
            .field("title", 5.0)
            .field("description", 1.0),
    );
    assert_eq!(ids(&title_first), ["in-title", "in-description"]);
    let description_first = reader.query(
        &Query::new("lantern")
            .field("title", 0.1)
            .field("description", 5.0),
    );
    assert_eq!(ids(&description_first), ["in-description", "in-title"]);
    assert_eq!(
        ids(&reader.query(&Query::new("lantern").field(DEFAULT_FIELD, 1.0))),
        ["legacy"]
    );
    assert_eq!(reader.search("lantern", 10).len(), 3);
    assert_eq!(ids(&reader.search("zurch", 10)), ["legacy"]);
    assert_eq!(
        ids(&reader.query(&Query::new("lanetrn").field("title", 1.0))),
        ["in-title"]
    );
    assert!(reader
        .query(&Query::new("lanetrn").field("title", 1.0).exact())
        .is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

fn payload(id: &str, version: u32) -> String {
    format!(
        "{{\"id\":\"{id}\",\"version\":{version},\"pad\":\"{}\"}}",
        "x".repeat(300 + id.len() * 7)
    )
}

/// Checks every field, filter value and payload of the live documents against `live`.
fn check(reader: &Reader, live: &HashMap<String, u32>, deleted: &[String]) {
    assert_eq!(reader.len(), live.len());
    for (id, &version) in live {
        let n: i64 = id[1..].parse().unwrap();
        assert_eq!(
            reader.document(id),
            Some(payload(id, version).into_bytes()),
            "{id}"
        );
        let unique = format!("u{n}v{version}");
        assert_eq!(
            ids(&reader.query(&Query::new(&unique).field("title", 2.0).exact())),
            [id.as_str()]
        );
        assert_eq!(
            filtered(reader, Filter::keyword("key", id.clone())),
            BTreeSet::from([id.clone()])
        );
        let value = n * 10 + i64::from(version);
        assert_eq!(
            filtered(reader, Filter::equals("value", value)),
            BTreeSet::from([id.clone()]),
            "{id}"
        );
        assert_eq!(
            filtered(reader, Filter::equals("ratio", value as f64 / 4.0)),
            BTreeSet::from([id.clone()])
        );
    }
    for id in deleted {
        assert_eq!(reader.document(id), None);
        assert!(filtered(reader, Filter::keyword("key", id.clone())).is_empty());
    }
    let all: Vec<SearchHit> = reader.query(&Query::new("shared").limit(usize::MAX));
    assert_eq!(all.len(), live.len());
    let payloads = reader.documents(&all);
    for (hit, payload_bytes) in all.iter().zip(payloads) {
        assert_eq!(
            payload_bytes,
            Some(payload(&hit.id, live[&hit.id]).into_bytes())
        );
    }
}

#[test]
fn merges_replacements_deletes_and_reordering_keep_everything_with_its_id() {
    for reordering in [Reordering::Never, Reordering::OnMerge, Reordering::Always] {
        let dir = temp(&format!("lifecycle-{reordering:?}"));
        let index = Index::open(&dir).unwrap().with_reordering(reordering);
        let mut writer = index.writer().unwrap();
        let mut live: HashMap<String, u32> = HashMap::new();
        let mut deleted = Vec::new();
        let mut random = Random(9);
        let add =
            |writer: &mut completype_fts::Writer, id: &str, version: u32, random: &mut Random| {
                let n: i64 = id[1..].parse().unwrap();
                let value = n * 10 + i64::from(version);
                let document = Document::new()
                    .text(
                        "title",
                        format!("shared u{n}v{version} {}", random.words(4, 40)),
                    )
                    .text("description", random.words(30, 200))
                    .keyword("key", id)
                    .keyword("group", format!("g{}", n % 7))
                    .number("value", value)
                    .number("ratio", value as f64 / 4.0)
                    .stored(payload(id, version));
                writer.add_document(id, &document);
            };
        for batch in 0..3 {
            for d in 0..400 {
                let id = format!("d{}", batch * 400 + d);
                add(&mut writer, &id, 0, &mut random);
                live.insert(id, 0);
            }
            for _ in 0..40 {
                let id = format!("d{}", random.below((batch + 1) * 400));
                if random.below(2) == 0 {
                    if live.remove(&id).is_some() {
                        writer.delete(&id);
                        deleted.push(id);
                    }
                } else if let Some(version) = live.get_mut(&id) {
                    *version += 1;
                    add(&mut writer, &id, *version, &mut random);
                }
            }
            writer.commit().unwrap();
        }
        let before = index.reader().unwrap();
        assert_eq!(before.segment_count(), 3);
        check(&before, &live, &deleted);
        writer.merge().unwrap();
        let after = index.reader().unwrap();
        assert_eq!(after.segment_count(), 1);
        check(&after, &live, &deleted);
        let group = Filter::keyword("group", "g3").and(Filter::range("value", ..5_000));
        assert_eq!(filtered(&before, group.clone()), filtered(&after, group));
        drop(writer);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn stored_payloads_span_blocks_and_documents_without_one_have_none() {
    let dir = temp("stored");
    let index = Index::open(&dir).unwrap();
    let mut writer = index.writer().unwrap();
    for d in 0..3_000 {
        let mut document = Document::new().text("title", format!("item number{d}"));
        if d % 3 != 0 {
            document = document.stored(format!("{d}:{}", "é".repeat(d % 50)));
        }
        writer.add_document(&format!("{d}"), &document);
    }
    writer.add("bare", "item");
    writer.commit().unwrap();
    let reader = index.reader().unwrap();
    for d in (0..3_000).step_by(7) {
        let expected = (d % 3 != 0).then(|| format!("{d}:{}", "é".repeat(d % 50)).into_bytes());
        assert_eq!(reader.document(&d.to_string()), expected, "{d}");
    }
    assert_eq!(reader.document("bare"), None);
    assert_eq!(reader.document("missing"), None);
    let hits = reader.query(&Query::new("item").limit(3_001));
    let payloads = reader.documents(&hits);
    for (hit, payload) in hits.iter().zip(&payloads) {
        let expected = hit
            .id
            .parse::<usize>()
            .ok()
            .filter(|d| d % 3 != 0)
            .map(|d| format!("{d}:{}", "é".repeat(d % 50)).into_bytes());
        assert_eq!(payload, &expected, "{}", hit.id);
    }
    std::fs::remove_dir_all(dir).unwrap();
}
