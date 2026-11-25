"""Evaluate prepared BEIR queries against standalone and server search engines."""

import argparse
import json
import math
import os
import random
import re
import secrets
import signal
import socket
import statistics
import subprocess
import tempfile
import threading
import time
from collections import Counter
from contextlib import contextmanager
from itertools import islice
from pathlib import Path

import psutil
import requests
import tantivy

from workload import Vocabulary, batches, expand_query, ndcg10, query_variants, recall100, tokens

ENGINES = ("completype-fts", "fts", "fts-any-first", "fts-exact", "fts-reordered", "fts-stop", "fts-stem",
           "fts-stem-stop", "tantivy", "lucene", "meilisearch", "typesense")
K = 100
BATCH_SIZE = 5000
LARGE_CORPUS_ENGINE_LIMIT_SECONDS = 25 * 60
MEILI_ID = re.compile(r"[A-Za-z0-9_-]{1,511}")
MEILI_MAPPED_PREFIX = "_fts_"


@contextmanager
def launch(command, work, *, env=None, pipes=False):
    with (work / "engine.log").open("w+", encoding="utf-8", errors="replace") as log:
        process = subprocess.Popen(
            command, env=env, text=True, encoding="utf-8", stderr=log,
            stdin=subprocess.PIPE if pipes else subprocess.DEVNULL, stdout=subprocess.PIPE if pipes else log,
        )
        try:
            yield process
        except Exception:
            log.seek(0, os.SEEK_END)
            log.seek(max(log.tell() - 4000, 0))
            print("\n".join(line for line in log.read().splitlines() if "key" not in line.lower()), flush=True)
            raise
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
            if pipes:
                process.stdin.close()
                process.stdout.close()


@contextmanager
def start_pipe(engine, binary_dir, work, corpus):
    weighted = engine == "completype-fts"
    if weighted:
        command = [binary_dir / "completype-fts", "--interactive"]
    else:
        jars = ":".join(str(binary_dir / jar) for jar in ("lucene-core.jar", "lucene-analysis-common.jar"))
        entry_point = "BenchLucene" if (binary_dir / "BenchLucene.class").is_file() else str(
            Path(__file__).resolve().with_name("bench_lucene.java")
        )
        command = [
            "java", "-XX:ActiveProcessorCount=1", "-Xmx2g", "-cp", f"{binary_dir}:{jars}",
            entry_point, str(work / "index"),
        ]
    with launch(command, work, pipes=True) as process:
        for row in corpus:
            process.stdin.write(row)
        process.stdin.write("\n")
        process.stdin.flush()
        if process.stdout.readline() != "READY\n":
            raise RuntimeError(f"{engine} did not open its index")

        def search(terms):
            query = " ".join(f"{term}:{weight}" for term, weight in terms.items()) if weighted else " ".join(terms)
            process.stdin.write(query + "\n")
            process.stdin.flush()
            response = process.stdout.readline()
            if not response:
                raise RuntimeError(f"{engine} closed its query stream")
            return response.rstrip("\n").split("\t") if response != "\n" else []

        yield search


@contextmanager
def start_library(engine, binary_dir, work, corpus):
    mode = "exact" if engine == "fts-exact" else "typos"
    order = "reorder" if engine == "fts-reordered" else "plain"
    first = "any-first" if engine == "fts-any-first" else "keep-first"
    language = "-".join(part for part in ("stem", "stop") if part in engine.split("-")) or "plain"
    command = [binary_dir / "fts-serve", str(work / "index"), mode, order, first, language]
    with launch(command, work, pipes=True) as process:
        for row in corpus:
            process.stdin.write(row)
        process.stdin.write("\n")
        process.stdin.flush()
        if process.stdout.readline() != "READY\n":
            raise RuntimeError(f"{engine} did not open its index")

        def search(text):
            process.stdin.write(text.replace("\n", " ") + "\n")
            process.stdin.flush()
            response = process.stdout.readline()
            if not response:
                raise RuntimeError(f"{engine} closed its query stream")
            return response.rstrip("\n").split("\t") if response != "\n" else []

        yield search


@contextmanager
def start_tantivy(work, corpus):
    schema = (
        tantivy.SchemaBuilder()
        .add_text_field("id", stored=True, tokenizer_name="raw")
        .add_text_field("body", tokenizer_name="prepared", index_option="freq")
        .build()
    )
    directory = work / "index"
    directory.mkdir()
    index = tantivy.Index(schema, path=str(directory))
    index.register_tokenizer("prepared", tantivy.TextAnalyzerBuilder(tantivy.Tokenizer.whitespace()).build())
    writer = index.writer(heap_size=256_000_000, num_threads=1)
    for row in corpus:
        doc_id, body = row.rstrip("\n").split("\t", 1)
        writer.add_document(tantivy.Document(id=doc_id, body=body))
    writer.commit()
    writer.wait_merging_threads()
    index.reload()
    searcher = index.searcher()

    def search(terms):
        queries = [
            (tantivy.Occur.Should, tantivy.Query.term_query(schema, "body", term, index_option="freq"))
            for term in terms
        ]
        query = tantivy.Query.boolean_query(queries) if queries else tantivy.Query.empty_query()
        return [searcher.doc(addr)["id"][0] for _, addr in searcher.search(query, K, count=False).hits]

    yield search


def corpus_batch(corpus, batch_size):
    corpus = iter(corpus)
    while rows := list(islice(corpus, batch_size)):
        parts = (row.rstrip("\n").split("\t", 1) for row in rows)
        yield [{"id": doc_id, "body": body} for doc_id, body in parts]


def loopback_port(exclude=None):
    while True:
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
            if port != exclude:
                return port


@contextmanager
def start_server(engine, binary_dir, work, corpus, expected_docs):
    port = loopback_port()
    key = secrets.token_hex(24)
    base = f"http://127.0.0.1:{port}"
    index = work / "index"
    index.mkdir()
    meili = engine == "meilisearch"
    if meili and expected_docs >= 50_000:
        batch_size = 100_000
    elif expected_docs >= 1_000_000:
        batch_size = 25_000
    else:
        batch_size = BATCH_SIZE
    env = dict(os.environ)
    if meili:
        env.update(MEILI_MASTER_KEY=key, MEILI_MAX_INDEXING_THREADS="1",
                   MEILI_MAX_INDEXING_MEMORY="1GiB", MEILI_HTTP_PAYLOAD_SIZE_LIMIT="256MiB",
                   MEILI_NO_ANALYTICS="true")
        command = [binary_dir / engine, "--http-addr", f"127.0.0.1:{port}", "--db-path", index]
        headers = {"Authorization": f"Bearer {key}"}
    else:
        env["TYPESENSE_API_KEY"] = key
        command = [binary_dir / "typesense-server", f"--data-dir={index}", "--api-address=127.0.0.1",
                   f"--api-port={port}", "--peering-address=127.0.0.1", f"--peering-port={loopback_port(port)}",
                   "--thread-pool-size=2", "--max-indexing-concurrency=1", "--enable-cors=false"]
        headers = {"X-TYPESENSE-API-KEY": key}
    with launch(command, work, env=env) as process, requests.Session() as session:
        session.headers.update(headers)

        def request(method, route, **kwargs):
            response = session.request(method, base + route, timeout=90, **kwargs)
            if not response.ok:
                raise RuntimeError(f"{engine} {route} returned {response.status_code}: {response.text[:500]}")
            return response

        deadline = time.monotonic() + 90
        while time.monotonic() < deadline and process.poll() is None:
            try:
                if session.get(base + "/health", timeout=1).ok:
                    break
            except requests.RequestException:
                pass
            time.sleep(0.1)
        else:
            raise RuntimeError(f"{engine} did not start")

        def await_task(response):
            task = response.json()["taskUid"]
            deadline = time.monotonic() + 900
            while time.monotonic() < deadline:
                result = request("GET", f"/tasks/{task}").json()
                if result["status"] == "succeeded":
                    return
                if result["status"] in ("failed", "canceled"):
                    raise RuntimeError(result)
                time.sleep(0.05)
            raise RuntimeError(f"Meilisearch task {task} exceeded the indexing timeout")

        if meili:
            await_task(request("POST", "/indexes", json={"uid": "beir", "primaryKey": "id"}))
            await_task(request("PATCH", "/indexes/beir/settings", json={"searchableAttributes": ["body"]}))
            imported = 0
            for items in corpus_batch(corpus, batch_size):
                for ordinal, item in enumerate(items, imported):
                    doc_id = item["id"]
                    if not MEILI_ID.fullmatch(doc_id) or doc_id.startswith(MEILI_MAPPED_PREFIX):
                        item["source_id"] = doc_id
                        item["id"] = f"{MEILI_MAPPED_PREFIX}{ordinal}"
                await_task(request("POST", "/indexes/beir/documents", json=items))
                imported += len(items)
                if expected_docs >= 1_000_000:
                    print(f"{engine} committed {imported:,} of {expected_docs:,} documents", flush=True)
            actual = request("GET", "/indexes/beir/stats").json()["numberOfDocuments"]

            def search(terms):
                result = request("POST", "/indexes/beir/search", json={
                    "q": " ".join(terms), "limit": K, "attributesToRetrieve": ["id", "source_id"],
                }).json()
                return [hit.get("source_id", hit["id"]) for hit in result["hits"]]

        else:
            request("POST", "/collections", json={"name": "beir", "fields": [{"name": "body", "type": "string"}]})
            imported = 0
            for items in corpus_batch(corpus, batch_size):
                body = "\n".join(json.dumps(item, ensure_ascii=False) for item in items).encode()
                response = request("POST", "/collections/beir/documents/import", params={"action": "create"}, data=body)
                results = [json.loads(line) for line in response.text.splitlines()]
                if len(results) != len(items) or not all(result["success"] for result in results):
                    raise RuntimeError(f"Typesense rejected a batch: {str(results[:3])}")
                imported += len(items)
                if expected_docs >= 1_000_000 and (imported % 100_000 == 0 or imported == expected_docs):
                    print(f"{engine} imported {imported:,} of {expected_docs:,} documents", flush=True)
            actual = request("GET", "/collections/beir").json()["num_documents"]

            def search(terms):
                result = request("GET", "/collections/beir/documents/search", params={
                    "q": " ".join(terms), "query_by": "body", "per_page": K, "include_fields": "id",
                    "highlight_fields": "none",
                }).json()
                return [hit["document"]["id"] for hit in result["hits"]]

        if actual != expected_docs:
            raise RuntimeError(f"{engine} indexed {actual:,} of {expected_docs:,} documents")
        yield search


def mib(value):
    return round(value / 2**20, 1)


def rss(process):
    result = 0
    for child in [process, *process.children(recursive=True)]:
        try:
            result += child.memory_info().rss
        except psutil.NoSuchProcess:
            continue
    return result


def percentile(samples, p):
    return sorted(samples)[math.ceil(len(samples) * p / 100) - 1]


def measure(search, variants, qrels, prepare):
    results = {}
    for name, texts in variants:
        timings, ndcg, recall = [], [], []
        for query_id, text in texts.items():
            start = time.perf_counter()
            hits = search(prepare(text))
            timings.append((time.perf_counter() - start) * 1000)
            ndcg.append(ndcg10(hits, qrels[query_id]))
            recall.append(recall100(hits, qrels[query_id]))
        if timings:
            results[name] = {"queries": len(timings), "ndcg10": statistics.mean(ndcg),
                             "recall100": statistics.mean(recall),
                             **{f"p{p}_ms": percentile(timings, p) for p in (50, 95, 99)}}
    return results


def run(engine, args, dataset, corpus_path, count, variants, qrels, df):
    process = psutil.Process()
    peak = [rss(process)]
    stop = threading.Event()
    limit_reason = []

    def stop_for_memory(_signal, _frame):
        raise RuntimeError(limit_reason[0])

    def watch():
        while not stop.wait(0.25):
            peak[0] = max(peak[0], rss(process))
            available = psutil.virtual_memory().available
            if peak[0] > args.rss_limit_mib * 2**20:
                limit_reason.append(f"benchmark RSS {mib(peak[0])} MiB exceeded the {args.rss_limit_mib} MiB limit")
            elif available < 24 * 2**30:
                limit_reason.append(f"host available memory fell to {mib(available)} MiB")
            if limit_reason:
                for child in process.children(recursive=True):
                    try:
                        child.terminate()
                    except psutil.NoSuchProcess:
                        continue
                os.kill(process.pid, signal.SIGUSR1)
                return

    worker = threading.Thread(target=watch, daemon=True)
    with tempfile.TemporaryDirectory(prefix=f"{engine}-", dir=args.data_dir.parent) as tmp, corpus_path.open(encoding="utf-8") as corpus:
        work = Path(tmp)
        if engine == "tantivy":
            launch_engine = start_tantivy(work, corpus)
        elif engine.startswith("fts"):
            launch_engine = start_library(engine, args.bin_dir, work, corpus)
        elif engine in ("meilisearch", "typesense"):
            launch_engine = start_server(engine, args.bin_dir, work, corpus, count)
        else:
            launch_engine = start_pipe(engine, args.bin_dir, work, corpus)
        previous = signal.signal(signal.SIGUSR1, stop_for_memory)
        worker.start()
        try:
            start = time.perf_counter()
            if engine == "completype-fts":
                vocab = Vocabulary(df)

                def prepare(text):
                    return expand_query(text, vocab.expand_whole_words)

            elif engine.startswith("fts"):
                def prepare(text):
                    return " ".join(tokens(text))

            else:
                def prepare(text):
                    return dict.fromkeys(tokens(text), 1.0)

            with launch_engine as search:
                index_seconds = time.perf_counter() - start
                print(f"{engine} searchable on {dataset} after {index_seconds:.1f} seconds", flush=True)
                ready_rss = rss(process)
                files = [path.stat() for path in (work / "index").rglob("*") if path.is_file()]
                disk_bytes = sum(file.st_size for file in files)
                allocated_bytes = sum(file.st_blocks * 512 for file in files)
                text = next(iter(variants[0][1].values()))
                start = time.perf_counter()
                search(prepare(text))
                first_ms = (time.perf_counter() - start) * 1000
                for text in variants[0][1].values():
                    search(prepare(text))
                variants_results = measure(search, variants, qrels, prepare)
        finally:
            stop.set()
            worker.join()
            signal.signal(signal.SIGUSR1, previous)
    return dict(engine=engine, dataset=dataset, docs=count, ingest_to_ready_seconds=index_seconds,
                sampled_peak_rss_mib=mib(peak[0]), ready_rss_mib=mib(ready_rss),
                index_disk_mib=mib(disk_bytes) if disk_bytes else None,
                allocated_index_mib=mib(allocated_bytes) if files else None, first_query_ms=first_ms,
                variants=variants_results)


def benchmark(dataset, args):
    variants, qrels = query_variants(args.data_dir, dataset)
    clean, synthetic, _, real = variants
    query_ids = list(clean)
    random.Random(7).shuffle(query_ids)
    selected = query_ids[:args.query_limit]
    variants = [
        ("clean", {q: clean[q] for q in selected}),
        ("synthetic", {q: synthetic[q] for q in selected}),
        ("real_clean", {q: clean[q] for q in selected if q in real}),
        ("real_typo", {q: real[q] for q in selected if q in real}),
    ]
    df = Counter()
    with tempfile.TemporaryDirectory(prefix=f"{dataset}-", dir=args.data_dir.parent) as tmp:
        corpus = Path(tmp) / "prepared.tsv"
        count = 0
        with corpus.open("w", encoding="utf-8") as out:
            for batch in batches(args.data_dir / dataset):
                for doc_id, text in batch:
                    terms = tokens(text)
                    if "completype-fts" in args.engines:
                        df.update(set(terms))
                    out.write(f"{doc_id}\t{' '.join(terms)}\n")
                    count += 1
        for engine in args.engines:
            print(f"Running {engine} on {dataset} ({count:,} documents)", flush=True)
            limit = args.engine_limit_seconds or (
                LARGE_CORPUS_ENGINE_LIMIT_SECONDS if count >= 1_000_000 else 0
            )

            def timed_out(_signal, _frame):
                raise RuntimeError(f"engine exceeded its {limit}-second run budget")

            previous = signal.signal(signal.SIGALRM, timed_out)
            signal.alarm(limit)
            try:
                result = run(engine, args, dataset, corpus, count, variants, qrels, df)
            except Exception as error:
                print(json.dumps({"engine": engine, "dataset": dataset, "error": str(error)}), flush=True)
            else:
                print(json.dumps(result), flush=True)
            finally:
                signal.alarm(0)
                signal.signal(signal.SIGALRM, previous)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("bin_dir", type=lambda value: Path(value).resolve())
    parser.add_argument("data_dir", type=lambda value: Path(value).resolve())
    parser.add_argument("datasets", nargs="+")
    parser.add_argument("--engines", nargs="+", choices=ENGINES, default=ENGINES)
    parser.add_argument("--query-limit", type=int)
    parser.add_argument("--rss-limit-mib", type=int, default=8192)
    parser.add_argument("--engine-limit-seconds", type=int, default=0, help="override the per-engine run budget")
    args = parser.parse_args()
    if args.query_limit is not None and args.query_limit < 1:
        parser.error("query limit must be positive")
    if args.rss_limit_mib < 1:
        parser.error("RSS limit must be positive")
    if args.engine_limit_seconds < 0:
        parser.error("engine run budget must be nonnegative")
    for dataset in args.datasets:
        benchmark(dataset, args)
