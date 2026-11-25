# completype-fts final benchmark

The final Rust POC uses exact BM25 with bitmap-assisted Block-Max MaxScore. The benchmarks ran on a Xeon Silver 4112 Linux host, on one logical CPU at nice 19 with idle I/O priority. Tantivy was 0.27.0, Lucene was 10.6.0, Meilisearch was 1.57.0, and Typesense was 30.2.

## In-process query latency

completype-fts led at all three latency percentiles on all seven corpora at both cutoffs. Both engines used the same prepared terms and BM25 disjunction workloads. Each corpus ran four warm passes over all test queries up to a seed-7 sample of 256. The harness verified each completype-fts result against its exhaustive BM25 reference for IDs, order, and exact scores before timing. Tantivy used its own BM25 implementation. Latencies are in milliseconds.

| Dataset | Top k | completype-fts p50 | p95 | p99 | Tantivy p50 | p95 | p99 |
|---|---:|---:|---:|---:|---:|---:|---:|
| SciFact | 10 | 0.044 | 0.184 | 0.294 | 0.157 | 0.514 | 0.728 |
| SciFact | 100 | 0.111 | 0.272 | 0.345 | 0.323 | 0.798 | 1.213 |
| NFCorpus | 10 | 0.007 | 0.026 | 0.036 | 0.016 | 0.087 | 0.114 |
| NFCorpus | 100 | 0.021 | 0.071 | 0.083 | 0.042 | 0.193 | 0.230 |
| TREC-COVID | 10 | 1.153 | 2.196 | 6.034 | 2.493 | 6.012 | 14.202 |
| TREC-COVID | 100 | 1.539 | 3.301 | 8.199 | 4.497 | 9.553 | 23.802 |
| FiQA | 10 | 0.351 | 1.685 | 2.638 | 0.975 | 3.708 | 5.395 |
| FiQA | 100 | 0.516 | 2.349 | 3.256 | 1.709 | 5.483 | 7.713 |
| Natural Questions | 10 | 4.822 | 15.566 | 25.033 | 5.724 | 17.662 | 32.516 |
| Natural Questions | 100 | 8.721 | 20.332 | 27.021 | 11.037 | 28.860 | 49.530 |
| Quora | 10 | 0.925 | 3.713 | 7.130 | 1.142 | 8.718 | 15.414 |
| Quora | 100 | 1.714 | 4.476 | 8.677 | 3.010 | 15.208 | 23.729 |
| DBpedia | 10 | 2.431 | 16.037 | 25.765 | 2.623 | 18.057 | 35.312 |
| DBpedia | 100 | 4.480 | 20.823 | 32.639 | 4.721 | 28.820 | 55.697 |

## End-to-end full-text search

Every engine received the same pre-tokenised document text and matching BEIR query selections. The timed path covered query preparation through return of up to 100 document IDs. completype-fts used whole-word correction, Tantivy and Lucene used BM25, and Meilisearch and Typesense used their default ranking over loopback HTTP. The Rust binary ran as a subprocess. The Meilisearch adapter maps unsupported document keys and returns their original IDs for scoring. This measures the workload through each adapter.

All completype-fts rows and all Quora and DBpedia rows are from the final run. Comparator rows for the other corpora use the existing captures from the same host for the same versions, engine settings, and selected queries. The two timing tables use different interfaces. The [raw full-text results](benchmark-linux-results.jsonl) include sample sizes and p50/p95/p99, nDCG@10, and recall@100 for each query set.

### Ranking and warm latency

The real-typo clean column uses the same query IDs as real typos. Latency columns describe clean queries.

| Dataset | Engine | Clean nDCG@10 | Clean recall@100 | Synthetic typo nDCG@10 | Real-typo clean nDCG@10 | Real typo nDCG@10 | p50 ms | p95 ms | p99 ms |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| SciFact | completype-fts | 0.666 | 0.894 | 0.671 | 0.674 | 0.675 | 1.30 | 4.23 | 6.68 |
| SciFact | Tantivy | 0.661 | 0.889 | 0.648 | 0.667 | 0.673 | 0.39 | 0.55 | 3.42 |
| SciFact | Lucene | 0.661 | 0.889 | 0.648 | 0.667 | 0.673 | 3.41 | 8.30 | 13.88 |
| SciFact | Meilisearch | 0.376 | 0.532 | 0.372 | 0.368 | 0.364 | 25.61 | 66.98 | 74.97 |
| SciFact | Typesense | 0.320 | 0.328 | 0.326 | 0.317 | 0.329 | 38.73 | 112.71 | 165.64 |
| NFCorpus | completype-fts | 0.314 | 0.238 | 0.304 | 0.274 | 0.274 | 0.51 | 1.47 | 2.09 |
| NFCorpus | Tantivy | 0.309 | 0.237 | 0.163 | 0.267 | 0.240 | 0.19 | 0.28 | 0.31 |
| NFCorpus | Lucene | 0.309 | 0.237 | 0.163 | 0.267 | 0.240 | 1.11 | 5.18 | 9.11 |
| NFCorpus | Meilisearch | 0.255 | 0.170 | 0.242 | 0.153 | 0.150 | 8.75 | 57.93 | 63.26 |
| NFCorpus | Typesense | 0.205 | 0.116 | 0.205 | 0.122 | 0.115 | 2.10 | 15.41 | 34.45 |
| TREC-COVID | completype-fts | 0.560 | 0.097 | 0.563 | 0.568 | 0.571 | 1.56 | 3.35 | 8.16 |
| TREC-COVID | Tantivy | 0.570 | 0.097 | 0.514 | 0.578 | 0.590 | 4.08 | 5.99 | 10.04 |
| TREC-COVID | Lucene | 0.570 | 0.097 | 0.514 | 0.578 | 0.590 | 7.26 | 18.44 | 26.48 |
| TREC-COVID | Meilisearch | 0.175 | 0.017 | 0.171 | 0.167 | 0.144 | 77.96 | 116.09 | 128.52 |
| TREC-COVID | Typesense | 0.109 | 0.004 | 0.107 | 0.092 | 0.109 | 214.10 | 1011.33 | 1286.97 |
| FiQA | completype-fts | 0.236 | 0.509 | 0.231 | 0.242 | 0.244 | 0.73 | 3.16 | 4.69 |
| FiQA | Tantivy | 0.237 | 0.510 | 0.202 | 0.245 | 0.232 | 1.38 | 2.42 | 4.84 |
| FiQA | Lucene | 0.237 | 0.510 | 0.202 | 0.245 | 0.232 | 3.80 | 7.50 | 10.03 |
| FiQA | Meilisearch | 0.063 | 0.143 | 0.059 | 0.061 | 0.048 | 53.33 | 102.07 | 144.89 |
| FiQA | Typesense | 0.041 | 0.042 | 0.042 | 0.038 | 0.040 | 70.23 | 510.02 | 732.61 |
| Natural Questions | completype-fts | 0.292 | 0.713 | 0.282 | 0.285 | 0.279 | 9.91 | 25.48 | 37.98 |
| Natural Questions | Tantivy | 0.292 | 0.707 | 0.222 | 0.285 | 0.268 | 45.21 | 71.70 | 78.32 |
| Natural Questions | Lucene | 0.292 | 0.707 | 0.222 | 0.285 | 0.268 | 8.48 | 22.42 | 30.64 |
| Quora | completype-fts | 0.756 | 0.941 | 0.744 | 0.754 | 0.745 | 2.11 | 6.27 | 10.59 |
| Quora | Tantivy | 0.757 | 0.937 | 0.635 | 0.755 | 0.701 | 5.17 | 9.36 | 12.47 |
| Quora | Lucene | 0.757 | 0.937 | 0.635 | 0.755 | 0.701 | 5.63 | 16.78 | 22.39 |
| Quora | Meilisearch | 0.274 | 0.369 | 0.257 | 0.282 | 0.195 | 61.86 | 72.93 | 78.92 |
| Quora | Typesense | 0.255 | 0.270 | 0.253 | 0.256 | 0.262 | 48.22 | 266.10 | 791.54 |
| DBpedia | completype-fts | 0.248 | 0.390 | 0.211 | 0.178 | 0.166 | 5.24 | 27.58 | 45.16 |
| DBpedia | Tantivy | 0.250 | 0.391 | 0.119 | 0.182 | 0.141 | 20.52 | 95.82 | 111.04 |
| DBpedia | Lucene | 0.251 | 0.391 | 0.119 | 0.182 | 0.141 | 6.40 | 22.88 | 35.38 |

### Build time, memory, and disk

RSS covers the benchmark client and its engine processes. Peak RSS is sampled periodically and can miss allocations between samples. First query is measured after opening the index with normal OS caches. completype-fts holds its index in memory. An asterisk marks a capture of logical file size. Other disk values use allocated bytes.

| Dataset | Engine | Ingest to ready s | First query ms | Ready RSS MiB | Peak RSS MiB | Index MiB |
|---|---|---:|---:|---:|---:|---:|
| SciFact | completype-fts | 0.50 | 0.28 | 243.20 | 243.60 | — |
| SciFact | Tantivy | 0.89 | 0.58 | 198.20 | 209.20 | 1.5* |
| SciFact | Lucene | 3.86 | 119.87 | 331.40 | 807.60 | 2.5 |
| SciFact | Meilisearch | 12.49 | 51.22 | 489.50 | 581.10 | 227.9* |
| SciFact | Typesense | 6.45 | 63.33 | 322.50 | 328.10 | 39.2 |
| NFCorpus | completype-fts | 0.36 | 0.48 | 225.90 | 226.00 | — |
| NFCorpus | Tantivy | 0.85 | 0.10 | 175.80 | 186.40 | 1.1 |
| NFCorpus | Lucene | 4.39 | 15.68 | 376.90 | 589.50 | 1.9 |
| NFCorpus | Meilisearch | 8.50 | 5.84 | 322.90 | 427.20 | 90.8 |
| NFCorpus | Typesense | 5.60 | 5.28 | 328.70 | 329.00 | 35.3 |
| TREC-COVID | completype-fts | 7.68 | 1.74 | 895.10 | 895.10 | — |
| TREC-COVID | Tantivy | 6.77 | 5.06 | 554.90 | 573.90 | 32.1 |
| TREC-COVID | Lucene | 19.47 | 148.28 | 1276.50 | 1298.20 | 56.8 |
| TREC-COVID | Meilisearch | 260.88 | 64.34 | 2814.20 | 3106.60 | 2295.5 |
| TREC-COVID | Typesense | 76.71 | 1061.28 | 842.00 | 870.40 | 539.2 |
| FiQA | completype-fts | 2.10 | 0.45 | 533.50 | 525.20 | — |
| FiQA | Tantivy | 2.40 | 3.33 | 258.50 | 278.60 | 8.9 |
| FiQA | Lucene | 8.38 | 115.94 | 561.80 | 906.80 | 15.9 |
| FiQA | Meilisearch | 59.35 | 10.81 | 848.20 | 1184.20 | 414.5 |
| FiQA | Typesense | 18.46 | 22.70 | 431.10 | 442.20 | 167.3 |
| Natural Questions | completype-fts | 67.43 | 11.84 | 4383.30 | 4421.90 | — |
| Natural Questions | Tantivy | 59.08 | 50.52 | 1123.10 | 1348.30 | 307.4 |
| Natural Questions | Lucene | 93.89 | 249.64 | 2330.10 | 2590.20 | 468.2 |
| Quora | completype-fts | 2.40 | 2.24 | 412.00 | 407.00 | — |
| Quora | Tantivy | 3.79 | 5.51 | 254.00 | 314.90 | 19.0 |
| Quora | Lucene | 8.43 | 171.05 | 672.90 | 884.10 | 15.6 |
| Quora | Meilisearch | 58.70 | 49.65 | 1557.60 | 1656.90 | 988.7 |
| Quora | Typesense | 28.36 | 48.82 | 413.20 | 413.40 | 121.0 |
| DBpedia | completype-fts | 106.97 | 1.10 | 6291.10 | 6472.40 | — |
| DBpedia | Tantivy | 95.70 | 11.77 | 2307.70 | 2669.90 | 544.8 |
| DBpedia | Lucene | 117.03 | 163.02 | 3686.00 | 4105.20 | 601.1 |

### Unfinished service runs

There are no query metrics for these runs. The benchmark stops at 8 GiB combined RSS or 24 GiB available host RAM. Quora and DBpedia service runs also have a 600-second per-engine budget.

| Dataset | Engine | Outcome |
|---|---|---|
| Natural Questions | Meilisearch | The 8 GiB benchmark memory guard stopped ingestion after 500,000 documents were committed. |
| Natural Questions | Typesense | Indexed all 2,681,468 documents in 781.2 seconds, then a search hit the 90-second HTTP timeout. |
| DBpedia | Meilisearch | The benchmark workspace ran out of disk space during import. Last reported ingestion count was 400,000 of 4,635,922 documents. |
| DBpedia | Typesense | An HTTP request timed out during import. Last reported ingestion count was 2,800,000 of 4,635,922 documents. |

## Autocomplete reference

The released completype 0.2.3 library also has existing results for autocomplete on 124,440 Hacker News titles. These are separate measurements of the autocomplete libraries and services. The ranking columns are mean reciprocal rank, with eight closed-loop clients for throughput. [Raw autocomplete results](benchmark-linux-autocomplete.json)

| Engine | Popular clean | Uniform clean | Real misspelling | Prefix p50 ms | Prefix p99 ms | 8-client QPS | Build s | Disk MiB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| completype 0.2.3 | 0.861 | 0.805 | 0.853 | 0.45 | 2.23 | 1704 | 0.74 | 10.1 |
| tantivy-py 0.27.0 | 0.803 | 0.738 | 0.610 | 0.94 | 5.52 | 758 | 1.67 | 9.4 |
| meilisearch 1.57.0 | 0.846 | 0.798 | 0.745 | 3.05 | 5.00 | 254 | 16.30 | 80.7 |
| typesense 30.2 | 0.784 | 0.755 | 0.788 | 1.82 | 20.89 | 185 | 5.88 | 79.0 |

## Reproduce

Prepare each BEIR dataset under `data/<name>/` with `corpus.parquet`, `queries.parquet` and `test.tsv` from the matching qrels repository. Put the Wikipedia misspelling list under `data/misspellings/wikipedia.txt`. Install `completype==0.2.3`, `tantivy==0.27.0`, `pyarrow==26.0.1`, `requests` and `psutil`. Put the Meilisearch and Typesense binaries and Lucene 10.6.0 jars in `bin/`. A JDK 21 can compile `bench_lucene.java` to `bin/BenchLucene.class`.

```sh
cd research/completype-fts
RUSTFLAGS="-C target-feature=+popcnt" cargo build --release --manifest-path rust/Cargo.toml
RUSTFLAGS="-C target-feature=+popcnt" cargo build --release --manifest-path latency/Cargo.toml
cp rust/target/release/completype-fts latency/target/release/completype-fts-latency bin/
javac --release 21 -cp bin/lucene-core.jar:bin/lucene-analysis-common.jar -d bin bench_lucene.java

python -u bench_latency.py bin/completype-fts-latency data \
  scifact nfcorpus trec-covid fiqa nq quora dbpedia-entity --rounds 4 --query-limit 256
python -u bench_engines.py bin data scifact nfcorpus trec-covid fiqa
python -u bench_engines.py bin data nq --query-limit 256
python -u bench_engines.py bin data quora dbpedia-entity --query-limit 256 --engine-limit-seconds 600
```
