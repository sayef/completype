"""Feed identical BEIR documents and clean queries to the in-process Rust benchmark."""

import argparse
import random
import subprocess
from pathlib import Path

from workload import batches, load_queries, tokens


def run(binary, dataset_dir, rounds, query_limit):
    print(f"\n## {dataset_dir.name}", flush=True)
    queries, _ = load_queries(dataset_dir)
    texts = list(queries.values())
    if query_limit is not None and query_limit < len(texts):
        random.Random(7).shuffle(texts)
        texts = texts[:query_limit]
    command = [str(binary.resolve()), str(rounds)]
    with subprocess.Popen(command, stdin=subprocess.PIPE, text=True, encoding="utf-8") as benchmark:
        with benchmark.stdin as requests:
            for batch in batches(dataset_dir):
                for doc_id, text in batch:
                    requests.write(f"{doc_id}\t{' '.join(tokens(text))}\n")
            requests.write("\n")
            for query in texts:
                requests.write(" ".join(dict.fromkeys(tokens(query))) + "\n")
        status = benchmark.wait()
        if status:
            raise subprocess.CalledProcessError(status, benchmark.args)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("data_dir", type=Path)
    parser.add_argument("datasets", nargs="+")
    parser.add_argument("--rounds", type=int, default=21)
    parser.add_argument("--query-limit", type=int, help="sample this many test queries with seed 7")
    args = parser.parse_args()
    if args.rounds < 1 or (args.query_limit is not None and args.query_limit < 1):
        parser.error("rounds and query limit must be positive")
    for dataset in args.datasets:
        run(args.binary, args.data_dir / dataset, args.rounds, args.query_limit)


if __name__ == "__main__":
    main()
