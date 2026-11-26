# completype-fts

Embedded full-text search with exact BM25 top-k, typo tolerance and memory-mapped segments.

```rust
use completype_fts::Index;

let index = Index::open("my-index")?;
let mut writer = index.writer()?;
writer.add("1", "Restaurant reviews in Zürich");
writer.add("2", "A recipe for sourdough bread");
writer.commit()?;

let reader = index.reader()?;
let hits = reader.search("resturant zurich", 10); // corrects the typo
assert_eq!(hits[0].id, "1");
```

Documents with fields, filters and stored payloads:

```rust
use completype_fts::{Document, Filter, Query};

writer.add_document(
    "sku-1",
    &Document::new()
        .text("title", "Apple iPhone 15")
        .text("description", "A phone with a 6.1 inch display")
        .keyword("brand", "apple")
        .keyword("tags", "phone")
        .keyword("tags", "5g")
        .number("price", 799.0)
        .number("stock", 12)
        .stored(r#"{"name":"iPhone 15"}"#),
);
writer.commit()?;

let reader = index.reader()?;
let query = Query::new("iphone")
    .field("title", 3.0)
    .field("description", 1.0)
    .filter(Filter::keyword_in("brand", ["apple", "samsung"]).and(Filter::range("price", ..1000.0)))
    .limit(20);
let hits = reader.query(&query);
let payloads = reader.documents(&hits);
```

Stemming and stop words, both off by default:

```rust
use completype_fts::{Analyzer, Index, Language, StopWords};

let index = Index::open("my-index")?
    .with_analyzer(Analyzer::default().stem(Language::English))
    .with_stop_words(StopWords::snowball(Language::English).unwrap().with(["etc"]));
```

## What it does

- **Exact BM25.** Results equal scoring every matching document (Lucene's one-byte document lengths, `k1 = 1.2`, `b = 0.75`), ties in document order. Pruning never changes a result.
- **Best-first windows.** Postings are stored in 128-document windows with a score bound each. A query scores windows in descending bound order and stops once no remaining window can enter the top k.
- **Fields and boosts.** A document has named text fields, each with its own postings, lengths and statistics. A query term scores `boost × BM25` in every field it searches, summed (Lucene's `most_fields`), so each field is one more weighted term to the same exact pruning. `Writer::add` and `Reader::search` use the single field `text` and search every field.
- **Filters.** Keyword fields (exact values, several per document) and numeric fields (`i64` or `f64`) filter with `keyword`, `keyword_in`, `range`, `equals`, `and`, `or` and `not`. A filter becomes a bitset per segment before scoring and joins the deletions, so results stay exact; windows with no allowed document are skipped. A query without terms lists the documents its filter allows.
- **Stored documents.** An optional payload per document (bytes, such as JSON), zstd-compressed in blocks; `Reader::document` and `Reader::documents` fetch them.
- **Document reordering.** Merged segments place documents that share terms in the same windows (recursive graph bisection), which tightens the bounds and shrinks the index; commits stay quick. `with_reordering` chooses `Never`, `OnMerge` (default) or `Always`.
- **Compact, memory-mapped segments.** Single postings live in the term dictionary, windows hold a presence mask or offsets and bit-packed term frequencies.
- **Index lifecycle.** One writer adds, replaces and deletes documents by id and commits atomically; readers are snapshots; small segments merge automatically, `Writer::merge` merges everything. Fields, filter values and payloads follow their documents through merges and reordering.
- **Typo tolerance.** `Reader::search` and `Reader::query` also match the closest much more common indexed spelling of a rare query term, looked up in all fields together (adjacent transpositions count as one edit, the first letter is kept unless `with_first_letter_typos(true)`). `Reader::search_exact` and `Query::exact` skip it.
- **Analysis.** Unicode words, lowercase, accents folded, CJK characters as single terms. One analyzer per index, used for every field.
- **Stemming.** `Analyzer::stem` reduces words to their Snowball stem (35 languages, via `frostem`) at index and query time, before folding accents. The index records its analyzer at the first commit, `Index::open` adopts it, and a reader or writer with a different one is refused.
- **Stop words.** `Index::with_stop_words` leaves words out of queries, never out of the index; a query of nothing but stop words keeps them all. `StopWords::snowball` gives the Snowball project's list for 17 languages (BSD-3-Clause, see `stopwords/LICENSE`). A typo corrected to a stop word is dropped too.

## Not yet

Phrase and proximity queries, facet counts, sorting by a field, per-field analyzers.

## Python

Experimental bindings, with no semver promise, ship in the `completype` wheel as `completype.fts`; see the
[full-text search guide](https://completype.pages.dev/guides/full-text-search/).
