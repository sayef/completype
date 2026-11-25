# completype-fts library benchmark

25 November 2026 · `crates/completype-fts` on branch `feat/completype-fts` · Linux, Xeon Silver 4112, nice 10

This measures the library crate, not the proof of concept under `rust/`. Segments are compressed and memory-mapped; queries return exact BM25 top-k (checked against exhaustive scoring on every benchmark query). "Reordered" segments were written with recursive graph bisection, which the index does by default when it merges segments.

## In-process query latency

Both engines get the same pre-tokenised terms and run BM25 disjunctions single-threaded, warm, four shuffled passes over up to 256 test queries. Tantivy 0.27.0 is merged to one segment, its fastest layout. Milliseconds.

| Dataset | Docs | k | completype-fts p50 / p99 | reordered p50 / p99 | Tantivy p50 / p99 |
|---|---:|---:|---:|---:|---:|
| SciFact | 5,183 | 10 | 0.061 / 0.191 | **0.053 / 0.187** | 0.163 / 0.768 |
| | | 100 | 0.109 / 0.264 | **0.098 / 0.248** | 0.329 / 1.254 |
| NFCorpus | 3,633 | 10 | **0.011** / 0.046 | 0.011 / **0.047** | 0.018 / 0.123 |
| | | 100 | **0.029** / 0.088 | 0.029 / **0.087** | 0.043 / 0.233 |
| FiQA | 57,638 | 10 | 0.470 / 1.528 | **0.374 / 1.415** | 1.054 / 5.519 |
| | | 100 | 0.675 / 1.901 | **0.573 / 1.797** | 1.824 / 8.062 |
| TREC-COVID | 171,332 | 10 | 1.366 / 4.974 | **0.911 / 3.683** | 2.599 / 14.438 |
| | | 100 | 1.920 / 6.049 | **1.353 / 4.545** | 4.761 / 24.108 |
| Quora | 522,931 | 10 | 0.757 / 7.636 | **0.417 / 4.833** | 1.186 / 16.171 |
| | | 100 | 1.806 / 8.475 | **0.883 / 6.119** | 3.085 / 24.416 |
| Natural Questions | 2,681,468 | 10 | 4.18 / 30.31 | **3.21 / 21.41** | 5.34 / 31.82 |
| | | 100 | 7.72 / 37.13 | **6.16 / 29.03** | 10.25 / 49.69 |
| DBpedia | 4,635,922 | 10 | 2.91 / 26.68 | **1.88 / 13.56** | 2.19 / 32.06 |
| | | 100 | 4.55 / 35.35 | **2.47 / 19.86** | 3.96 / 51.03 |

Reordered segments lead Tantivy at p50, p95 and p99 on every corpus and cutoff. Unordered segments lead everywhere except the DBpedia median.

## Index size and build time

Index size excludes the external id store, which Tantivy's index here does not have.

| Dataset | completype-fts | reordered | Tantivy | Build: plain / reordered / Tantivy |
|---|---:|---:|---:|---:|
| TREC-COVID | — | 30.6 MiB | 27.1 MiB | 4.3 / 16.1 / 5.2 s |
| Quora | — | 12.4 MiB | 12.3 MiB | 1.5 / 12.5 / 2.4 s |
| Natural Questions | 301.0 MiB | 279.4 MiB | 263.3 MiB | 42 / 278 / 57 s |
| DBpedia | — | 369.4 MiB | 324.0 MiB | 65 / 429 / 72 s |

Reordering ran on four threads, plain builds and Tantivy on one. The proof of concept needed 1.8 GiB of extra memory for Natural Questions; segments are now read through the page cache.

## End to end: relevance, typos and latency

Every engine received the same prepared text and the same up to 1,000 test queries per set. completype-fts analyses text itself and runs as a subprocess over a pipe; Tantivy runs through its Python binding, Lucene 10.6.0 over a Java pipe, Meilisearch 1.57.0 and Typesense 30.2 over loopback HTTP with default ranking. One logical CPU. nDCG@10; latency is for clean queries and includes each adapter's round trip.

| Dataset | Engine | Clean | Synthetic typos | Real typos (clean) | p50 / p99 ms | Index MiB |
|---|---|---:|---:|---:|---:|---:|
| SciFact | **completype-fts** | **0.665** | **0.666** | **0.677** (0.673) | 0.34 / 3.37 | 1.8 |
| | Tantivy | 0.661 | 0.648 | 0.673 (0.667) | 0.36 / 0.52 | 1.5 |
| | Lucene | 0.661 | 0.648 | 0.673 (0.667) | 2.39 / 8.78 | 2.5 |
| | Meilisearch | 0.376 | 0.372 | 0.364 (0.368) | 13.67 / 71.59 | 228.0 |
| | Typesense | 0.320 | 0.326 | 0.329 (0.317) | 42.06 / 151.02 | 22.3 |
| NFCorpus | **completype-fts** | **0.316** | **0.307** | **0.276** (0.276) | 0.15 / 0.53 | 1.3 |
| | Tantivy | 0.309 | 0.163 | 0.240 (0.267) | 0.19 / 0.34 | 1.1 |
| | Lucene | 0.309 | 0.163 | 0.240 (0.267) | 1.09 / 7.44 | 1.9 |
| | Meilisearch | 0.255 | 0.242 | 0.150 (0.153) | 7.55 / 62.93 | 91.0 |
| | Typesense | 0.205 | 0.205 | 0.115 (0.122) | 2.21 / 29.63 | 22.0 |
| FiQA | **completype-fts** | **0.238** | **0.234** | **0.245** (0.245) | 0.77 / 4.50 | 10.3 |
| | Tantivy | 0.237 | 0.202 | 0.232 (0.245) | 1.40 / 4.81 | 8.9 |
| | Lucene | 0.237 | 0.202 | 0.232 (0.245) | 3.11 / 12.13 | 15.8 |
| | Meilisearch | 0.063 | 0.059 | 0.048 (0.061) | 68.91 / 166.97 | 414.6 |
| | Typesense | 0.041 | 0.042 | 0.040 (0.038) | 72.05 / 771.59 | 141.7 |
| TREC-COVID | **completype-fts** | **0.570** | **0.568** | 0.581 (0.578) | 1.90 / 5.78 | 37.9 |
| | Tantivy | 0.570 | 0.514 | **0.590** (0.578) | 4.04 / 10.05 | 32.1 |
| | Lucene | 0.570 | 0.514 | 0.590 (0.578) | 6.74 / 24.73 | 56.8 |
| | Meilisearch | 0.175 | 0.171 | 0.144 (0.167) | 49.48 / 133.82 | 2295.3 |
| | Typesense | 0.109 | 0.107 | 0.109 (0.092) | 197.56 / 1313.48 | 450.8 |
| Quora | **completype-fts** | **0.751** | **0.740** | **0.740** (0.752) | 2.00 / 8.96 | 21.7 |
| | completype-fts, reordered | 0.751 | 0.740 | 0.741 (0.752) | 1.10 / 6.08 | 19.3 |
| | Tantivy | 0.751 | 0.644 | 0.705 (0.752) | 5.19 / 13.81 | 19.0 |
| | Lucene | 0.751 | 0.644 | 0.705 (0.752) | 2.73 / 11.27 | 15.6 |
| | Meilisearch | 0.267 | 0.252 | 0.175 (0.268) | 61.20 / 82.73 | 988.8 |
| | Typesense | 0.257 | 0.257 | 0.255 (0.255) | 53.09 / 502.03 | 108.4 |

- Clean relevance equals Tantivy and Lucene (same BM25); the proof of concept's TREC-COVID gap (0.560 against 0.570) is gone.
- With typos, completype-fts keeps most of its clean score where the BM25 engines drop: Quora synthetic typos 0.740 against 0.644, NFCorpus 0.307 against 0.163.
- The one loss: TREC-COVID real misspellings, 0.581 against 0.590, where a correction occasionally replaces a correct rare term. `search_exact` scores 0.593 there.
- Keeping the first letter when correcting (the default, as in Lucene's spell checker) changed nDCG by at most 0.003 against correcting any letter, at a fraction of the cost.
- Meilisearch and Typesense rank these long, question-style queries poorly with default settings and need 10 to 60 times the index size; they target short as-you-type queries, where the earlier autocomplete benchmark compares them.
- End-to-end Tantivy latency on the tiny corpora is lower at p99 because its Python binding avoids our pipe round trip; in process it is slower everywhere.

Raw data: [results-v2-e2e.jsonl](results-v2-e2e.jsonl) (rows named `fts-v2*` are the final library build), [results-v2-latency.txt](results-v2-latency.txt).

## Not measured yet

Natural Questions and DBpedia end to end (Meilisearch and Typesense did not finish them before), multi-threaded throughput, cold start from object storage, hybrid search with vectors through the library.
