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

## What it does

- **Exact BM25.** Results equal scoring every matching document (Lucene's one-byte document lengths, `k1 = 1.2`, `b = 0.75`), ties in document order. Pruning never changes a result.
- **Best-first windows.** Postings are stored in 128-document windows with a score bound each. A query scores windows in descending bound order and stops once no remaining window can enter the top k.
- **Document reordering.** Merged segments place documents that share terms in the same windows (recursive graph bisection), which tightens the bounds and shrinks the index; commits stay quick. `with_reordering` chooses `Never`, `OnMerge` (default) or `Always`.
- **Compact, memory-mapped segments.** Single postings live in the term dictionary, windows hold a presence mask or offsets and bit-packed term frequencies.
- **Index lifecycle.** One writer adds, replaces and deletes documents by id and commits atomically; readers are snapshots; small segments merge automatically, `Writer::merge` merges everything.
- **Typo tolerance.** `Reader::search` also matches the closest much more common indexed spelling of a rare query term (adjacent transpositions count as one edit, the first letter is kept unless `with_first_letter_typos(true)`). `Reader::search_exact` skips it.
- **Analysis.** Unicode words, lowercase, accents folded, CJK characters as single terms.

## Not yet

Phrase and proximity queries, fields and boosts, filters and facets, stored documents, stemming, Python bindings.
