"""Tabulate end-to-end benchmark JSON lines as Markdown."""

import json
import sys

print("| Dataset | Engine | Clean nDCG@10 | Synthetic typo | Real clean | Real typo | p50 ms | p99 ms | Ready RSS MiB | Index MiB | Ingest s |")
print("|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|")
for line in open(sys.argv[1], encoding="utf-8"):
    if not line.startswith("{"):
        continue
    r = json.loads(line)
    if "error" in r:
        print(f"| {r['dataset']} | {r['engine']} | error: {r['error'][:60]} |||||||||")
        continue
    v = r["variants"]
    get = lambda name, key: f"{v[name][key]:.3f}" if name in v else "—"
    ms = lambda key: f"{v['clean'][key]:.2f}"
    print(f"| {r['dataset']} | {r['engine']} | {get('clean', 'ndcg10')} | {get('synthetic', 'ndcg10')} | "
          f"{get('real_clean', 'ndcg10')} | {get('real_typo', 'ndcg10')} | {ms('p50_ms')} | {ms('p99_ms')} | "
          f"{r['ready_rss_mib']:.0f} | {r['index_disk_mib'] or 0:.1f} | {r['ingest_to_ready_seconds']:.1f} |")
