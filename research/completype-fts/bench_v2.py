"""Feed identical BEIR documents and test queries to the three-way in-process benchmark."""

import argparse
import random
import subprocess
from pathlib import Path

from workload import batches, load_queries, tokens


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("data_dir", type=Path)
    parser.add_argument("work_dir", type=Path)
    parser.add_argument("datasets", nargs="+")
    parser.add_argument("--rounds", type=int, default=4)
    parser.add_argument("--query-limit", type=int, default=256)
    parser.add_argument("--engines", default="new,poc,tantivy")
    args = parser.parse_args()
    for dataset in args.datasets:
        print(f"\n## {dataset}", flush=True)
        queries, _ = load_queries(args.data_dir / dataset)
        texts = list(queries.values())
        random.Random(7).shuffle(texts)
        texts = texts[: args.query_limit]
        command = [str(args.binary.resolve()), str(args.rounds), str(args.work_dir), args.engines]
        with subprocess.Popen(command, stdin=subprocess.PIPE, text=True, encoding="utf-8") as bench:
            with bench.stdin as out:
                for batch in batches(args.data_dir / dataset):
                    for doc_id, text in batch:
                        out.write(f"{doc_id}\t{' '.join(tokens(text))}\n")
                out.write("\n")
                for query in texts:
                    out.write(" ".join(dict.fromkeys(tokens(query))) + "\n")
            if bench.wait():
                raise SystemExit(f"{dataset} failed")


if __name__ == "__main__":
    main()
