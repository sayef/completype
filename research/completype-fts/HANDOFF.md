# completype-fts implementation and benchmark handoff

24 November 2026 · branch `poc/lexical-fts` · base `cbc9367` (`v0.2.3`)

The original brief asked for a standalone Rust proof of concept that could make completype's vocabulary useful for document retrieval and match Tantivy on exact BM25 speed. The retained scorer meets the latency target on Natural Questions and six other corpora. The hybrid experiments establish relevance close to tuned Lucene on two held-out tests. This is an in-memory POC under `research/` with an independently buildable crate. The owner still needs to decide the production package boundary.

## Where to start

| Path | What it contains |
|---|---|
| [README](README.md) | Build commands, library API and CLI protocol |
| [rust/src/lib.rs](rust/src/lib.rs) | Builder, postings, collection statistics and the exhaustive correctness oracle |
| [rust/src/pruned.rs](rust/src/pruned.rs) | The retained bitmap-assisted Block-Max MaxScore implementation |
| [latency/src/main.rs](latency/src/main.rs) | Paired, in-process Rust and Tantivy timing with an exact-score check |
| [bench_engines.py](bench_engines.py), [bench_lucene.java](bench_lucene.java), [workload.py](workload.py) | End-to-end adapters, query preparation and relevance metrics |
| [Final Linux benchmarks](benchmark-linux.md) | Complete lexical tables and reproduction commands |
| [Raw full-text results](benchmark-linux-results.jsonl) | End-to-end measurements with sample sizes and provenance |

The POC is part of the uncommitted worktree. Its manifest sets `publish = false` and declares its own workspace. The Linux benchmark sources and final data are retained. Hybrid experiment scripts, embeddings, model downloads and raw run logs were temporary and were cleaned up. The protocol and final hybrid numbers below are the surviving record of those runs.

## What was built

- The Rust builder accepts pre-tokenised documents, assigns insertion-order IDs and builds an immutable in-memory index. Postings hold 32-bit document IDs and term frequencies. BM25 uses `k1=1.2` and `b=0.75`. The index computes statistics for one complete corpus.
- The query path reuses a single global term ordering, applies conservative score bounds over 128-document windows and forms a candidate bitmap from essential terms. For eligible optional lists it intersects that bitmap with stored term presence, then reads the postings that can affect the candidates. The heap cutoff controls how much work can be skipped.
- Exact contributions are summed in query order using `f64` and ties follow insertion order. The exhaustive scorer handles weights outside the pruning range and gives the benchmark an oracle for exact IDs, order and scores. This equality is against the POC's exhaustive formula. Tantivy has its own BM25 implementation.
- The product benchmark uses the released completype 0.2.3 vocabulary to expand whole-word corrections before sending weighted terms to the Rust CLI. The in-process benchmark supplies identical prepared terms to both scorers. The latency win in that test belongs to the scorer.
- Hypotheses about ordinary Block-Max WAND, posting-aligned WAND and lazy MaxScore were tried during development. The WAND controls saved score evaluations but spent more time seeking and checking bounds. Bitmap filtering addressed the costly optional posting reads. The final crate keeps the winning search path and the exhaustive oracle. Earlier variants and comparative tables were removed from the POC.

## Expectations and outcomes

| Expectation from the original brief and follow-up | Outcome |
|---|---|
| Match or beat Tantivy in-process and preserve exact top-k | Met on the seven measured corpora, including full-corpus Natural Questions. MS MARCO was outside the measured set. |
| Keep clean lexical relevance close | Broadly met on the measured clean sets. Natural Questions was 0.292 nDCG@10 for all three BM25 engines. TREC-COVID was 0.560 for the corrected POC and 0.570 for Tantivy and Lucene, outside the original 0.005 tolerance. |
| Improve robustness to typos | Met on several workloads. On Natural Questions, synthetic-typo nDCG@10 was 0.282 versus 0.222 for both Tantivy and Lucene. Real-typo nDCG@10 was 0.279 versus 0.268. The matched clean subset scored 0.285 for all three. Correction also improved synthetic typo scores on TREC-COVID, where its real-typo score was lower than both baselines. |
| Expose a reusable correction API and support accent folding | The benchmark exercises whole-word expansion through released Python APIs. A dedicated public `correct` API and accent-folding changes for this project remain open. |
| Beat competing engines end to end | Workload-dependent. On Natural Questions the POC beat the Tantivy adapter on median and p99. Lucene had the lower median and p99. The corrected CLI took more time than the Tantivy adapter on small SciFact and NFCorpus. |
| Match or beat hybrid peers on relevance | The shared-embedding pilot beat the tested Meilisearch and Typesense configurations on SciFact and NFCorpus. The tuned held-out comparison with Lucene was effectively even. A held-out hybrid comparison with tuned Meilisearch or Typesense, and a hybrid relevance comparison with Tantivy, remain open. |
| Smaller persistent index, updates and cold starts | Open. The POC is an immutable in-memory index. Persistent segments, incremental updates, deletes, compaction, storage integration and object-store cold-start measurements remain open. |
| Finish with a clean prototype and benchmark handoff | Standalone crate, reproduction commands, final data and this handoff are available. Workspace integration, a publishable API and a release are owner decisions. |

### Final lexical benchmark

On the benchmark host, warm single-threaded in-process queries used matching input terms, four passes per cutoff, and all test queries up to a seed-7 sample of 256. The POC led Tantivy 0.27.0 across p50, p95 and p99 at both top‑10 and top‑100 on SciFact, NFCorpus, TREC-COVID, FiQA, Natural Questions, Quora and DBpedia. Key rows are below. The complete final speed, relevance, build, memory and disk tables are linked at the end. [Final Linux benchmark](benchmark-linux.md)

| Workload | completype-fts p50 / p99 | Tantivy p50 / p99 | Lucene p50 / p99 |
|---|---:|---:|---:|
| Natural Questions top‑100, in process | 8.72 / 27.02 ms | 11.04 / 49.53 ms | — |
| DBpedia top‑100, in process | 4.48 / 32.64 ms | 4.72 / 55.70 ms | — |
| Natural Questions top‑100, end to end | 9.91 / 37.98 ms | 45.21 / 78.32 ms | 8.48 / 30.64 ms |

The end-to-end timings include query preparation, adapter work and result-ID retrieval. They use a released completype vocabulary before the POC, Python's Tantivy adapter, a Java pipe for Lucene and loopback HTTP for Meilisearch and Typesense. Some competitor rows reused compatible earlier captures, marked in the raw data. On Natural Questions, ready RSS was **4,383 MiB** for the POC, **1,123 MiB** for Tantivy and **2,330 MiB** for Lucene. POC storage is in-memory and lacks a comparable on-disk index size. Meilisearch and Typesense had incomplete Natural Questions and DBpedia runs. Their reported outcomes cover the memory guard, timeouts and a full benchmark filesystem. Keep those rows out of any speed ranking. [Benchmark methodology and incomplete runs](benchmark-linux.md) · [Raw results](benchmark-linux-results.jsonl)

## Hybrid evidence

These experiments combined the standalone BM25 scorer with vectors outside the crate. Existing completype hybrid search combines *autocomplete* with vectors. For document retrieval, the practical integration point is completype's `vector_search` plus document-ID rank fusion, or a direct TurboVec backend behind the FTS crate. [Completype vector and hybrid guide](../../docs/guides/semantic-hybrid.md)

### Shared-embedding comparison

The compared configurations received the same normalised, 384-dimensional MiniLM vectors and 623 queries across SciFact and NFCorpus. The POC used an exact vector scan. Lucene 10.6.0 used its own BM25 and kNN indexes. Both used reciprocal rank fusion with `k=60`. Meilisearch 1.57.0 used its native hybrid API at `semanticRatio=0.5`. Typesense 30.2 used its native hybrid API at `alpha=0.3`. Timings include the adapter round trip and use precomputed query vectors. [MiniLM model](https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2) · [Meilisearch hybrid](https://www.meilisearch.com/blog/meilisearch-1-6) · [Typesense vector search](https://typesense.org/docs/30.2/api/vector-search.html)

| Configuration | SciFact nDCG@10 | SciFact p50 / p99 | NFCorpus nDCG@10 | NFCorpus p50 / p99 |
|---|---:|---:|---:|---:|
| completype-fts + exact vectors | 0.6789 | 1.24 / 1.55 ms | 0.3449 | 0.77 / 1.48 ms |
| Lucene BM25 + kNN | 0.6830 | 5.65 / 16.64 ms | 0.3420 | 5.16 / 21.70 ms |
| Meilisearch hybrid | 0.6551 | 68.17 / 84.99 ms | 0.3347 | 67.99 / 74.99 ms |
| Typesense hybrid | 0.5462 | 58.02 / 178.39 ms | 0.3273 | 23.59 / 50.16 ms |

The pilot measured usable configurations on small corpora and different adapters. It gives a quality and latency comparison under those settings. Fresh query embeddings measured separately on the Mac took 3.36 ms at p50 on SciFact and 1.55 ms on NFCorpus. The tuned relevance run below used TurboVec and a different fusion parameter. Its settings have no corresponding large-corpus latency measurement.

### TurboVec and the SciFact ranking difference

- In the original SciFact hybrid pilot, the POC and Lucene had 95.43% recall@100. Swapping the BM25 lists while holding vectors fixed explained about two-thirds of Lucene's small nDCG edge. Using Lucene's decoded document lengths in the POC formula reproduced Lucene's BM25 top‑10 order on all 300 SciFact queries. The vector list accounted for most of the remaining difference. The paired relevance interval included zero. Keep exact BM25 for the POC and treat length quantisation as a separate speed, storage or relevance experiment. [Lucene 10.6.0 BM25 source](https://github.com/apache/lucene/blob/releases/lucene/10.6.0/lucene/core/src/java/org/apache/lucene/search/similarities/BM25Similarity.java)
- On the Mac, 4-bit TurboVec's median vector-stage time was 0.15 ms on SciFact. Exact partial top‑100 selection took 0.13 ms and returned the same ranking as a full sort. The hybrid recall stayed at 95.43% with either vector backend. At 100,000 synthetic 384-dimensional vectors, 4-bit TurboVec took 0.65 ms at p50 versus 4.39 ms for exact partial selection, with 76.8% overlap against the exact top‑100 neighbours. That scale test measured retrieval speed and neighbour overlap. Real-corpus relevance at that size remains open.

### Final held-out hybrid relevance

Each engine independently chose its own weighted reciprocal rank fusion settings from the same 44 pairs on the **809 SciFact training queries**. The grid used fusion constants `10, 30, 60, 100` and vector weights from 0 to 1 in steps of 0.1. Both selected `k=10` and weight `0.5`. The settings were frozen before evaluating the SciFact and ArguAna test judgments. The same MiniLM encoding and top‑100 candidate depth applied to both engines. The POC used released completype's 4-bit TurboVec API and Lucene used its own kNN query. ArguAna excluded the query document before fusion. Five judgments referenced missing corpus documents and counted as misses for both systems. [SciFact splits](https://huggingface.co/datasets/BeIR/scifact-qrels) · [ArguAna evaluation note](https://huggingface.co/datasets/BeIR/arguana)

| Held-out test | Queries | POC nDCG@10 | Lucene nDCG@10 | POC recall@100 | Lucene recall@100 |
|---|---:|---:|---:|---:|---:|
| SciFact | 300 | 0.7010 | 0.7017 | 95.43% | 95.43% |
| ArguAna | 1,406 | 0.5094 | 0.5099 | 98.36% | 98.29% |

The equal-corpus average of the paired nDCG@10 deltas, POC minus Lucene, was **−0.0006**, with a paired bootstrap 95% interval of **−0.0039 to +0.0027**. SciFact's test split also appeared in the earlier pilot, outside the tuning grid. ArguAna was new to this evaluation. These results support competitive hybrid relevance with a single embedding model. A relevance win over tuned Lucene remains unproven.

## Decisions and next work

1. **Package boundary.** Proposed direction is a separate `completype-fts` crate in the same repository, keeping the autocomplete API independent. Confirm this and the public API before moving code into the workspace or making a release.
2. **Production lexical path.** Agree on the persistent segment format and memory target. Preserve the current fast path and its score oracle. Measure on-disk bytes, ready RSS, build cost and cold startup. Address global document frequency, average length, bounds across segments, updates, deletes and deterministic ties before claiming segmented equivalence.
3. **Hybrid path.** Reuse the existing vector capability as an opt-in backend and retain an exact vector reference for evaluation. Use the frozen fusion configuration as the first reference, measure real-corpus relevance and neighbour overlap at scale, and time embedding, retrieval and fusion together on the same hardware. A future tuned comparison should give each engine the same development-query budget.
4. **Benchmark protocol.** Keep scorer-only and end-to-end numbers labelled. Keep held-out scores out of tuning. Follow the [Linux reproduction instructions](benchmark-linux.md#reproduce) for lexical and typo runs. For hybrid, mean-pool the MiniLM ONNX token states with the attention mask, truncate to 256 tokens, L2-normalise embeddings, then take 100 candidates per arm and fuse by weighted reciprocal rank. Retrieve 101 candidates for ArguAna so excluding the query document still allows 100 per arm. Measure nDCG@10 and recall@100 against BEIR qrels. Bootstrap paired query differences within each corpus.
5. **Scratch space.** Keep Cargo targets, model caches, downloaded corpora and comparator indexes in task-owned scratch directories. On the benchmark host, check the actual filesystem, use one logical CPU with low priority, cap memory and runtime, and clean up the run's own files and services. A prior run filled its workspace on `/var/tmp` while `/tmp` was backed by a larger volume.

## Handoff verification

Both Rust manifests passed `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings`. The FTS crate passed all three unit tests, including exact pruned-versus-exhaustive comparisons across blocks and weighted queries. The Python benchmark sources passed syntax checks. The verification build used a temporary Cargo target, which was removed afterward.
