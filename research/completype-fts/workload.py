"""BEIR input, typo workload, relevance metrics and query correction."""

import math
import random
import re
from collections import Counter, defaultdict

import pyarrow.parquet as pq
import completype

token = re.compile(r"[^\W_]+")
RARE_DF, MORE_COMMON = 20, 50


def tokens(text, *, fold_accents=False):
    if fold_accents:
        text = completype.fold_accents(text)
    return [t for t in token.findall(text.lower()) if len(t) <= 40]


def expand_query(text, policy, analyse=tokens):
    weighted = defaultdict(float)
    for term in analyse(text):
        for word, weight in policy(term).items():
            weighted[word] = max(weighted[word], weight)
    return weighted


def batches(dataset_dir):
    f = pq.ParquetFile(dataset_dir / "corpus.parquet")
    for batch in f.iter_batches(batch_size=20000, columns=["_id", "title", "text"]):
        b = batch.to_pydict()
        yield [(i, (t or "") + " " + (x or "")) for i, t, x in zip(b["_id"], b["title"], b["text"])]


def load_queries(dataset_dir):
    queries = {r["_id"]: r["text"] for r in pq.read_table(dataset_dir / "queries.parquet").to_pylist()}
    qrels = defaultdict(dict)
    with (dataset_dir / "test.tsv").open(encoding="utf-8") as f:
        next(f)
        for line in f:
            q, d, s = line.rstrip("\n").split("\t")
            if int(float(s)) > 0:
                qrels[q][d] = int(float(s))
    return {q: queries[q] for q in qrels if q in queries}, qrels


def real_misspellings(path):
    wrong_for = defaultdict(list)
    with path.open(encoding="utf-8") as f:
        for line in f:
            if "->" in line:
                wrong, right = line.strip().split("->", 1)
                for r in right.split(","):
                    r = r.strip().lower()
                    if r.isalpha() and len(r) >= 4 and wrong.strip().isalpha():
                        wrong_for[r].append(wrong.strip().lower())
    return wrong_for


def synthetic_typo(query, rng):
    words = query.split()
    long = [i for i, w in enumerate(words) if len(w) >= 5 and w.isalpha()]
    if not long:
        return query
    i = rng.choice(long)
    w = words[i]
    for _ in range(10):
        p = rng.randrange(1, len(w) - 1)
        letter = rng.choice("abcdefghijklmnopqrstuvwxyz")
        new = rng.choice([w[:p] + w[p + 1:], w[:p] + letter + w[p + 1:], w[:p] + w[p + 1] + w[p] + w[p + 2:], w[:p] + letter + w[p:]])
        if new.lower() != w.lower():
            words[i] = new
            return " ".join(words)
    return query


def real_typo(query, wrong_for, rng):
    candidates = [w for w in set(tokens(query)) if w in wrong_for]
    if not candidates:
        return None
    word = rng.choice(sorted(candidates))
    return re.sub(rf"(?i)\b{re.escape(word)}\b", rng.choice(wrong_for[word]), query, count=1)


def query_variants(data_dir, dataset):
    queries, qrels = load_queries(data_dir / dataset)
    rng = random.Random(7)
    wrong_for = real_misspellings(data_dir / "misspellings" / "wikipedia.txt")
    synthetic = {q: synthetic_typo(text, rng) for q, text in queries.items()}
    real = {q: typo for q, text in queries.items() if (typo := real_typo(text, wrong_for, rng))}
    return (queries, synthetic, {q: queries[q] for q in real}, real), qrels


def osa(a, b):
    """Edit distance counting a swap of neighbouring letters as one edit."""
    d = [[0] * (len(b) + 1) for _ in range(len(a) + 1)]
    for i in range(len(a) + 1):
        d[i][0] = i
    for j in range(len(b) + 1):
        d[0][j] = j
    for i in range(1, len(a) + 1):
        for j in range(1, len(b) + 1):
            cost = a[i - 1] != b[j - 1]
            d[i][j] = min(d[i - 1][j] + 1, d[i][j - 1] + 1, d[i - 1][j - 1] + cost)
            if i > 1 and j > 1 and a[i - 1] == b[j - 2] and a[i - 2] == b[j - 1]:
                d[i][j] = min(d[i][j], d[i - 2][j - 2] + 1)
    return d[-1][-1]


def ndcg10(ranked, rel):
    dcg = sum((2 ** rel.get(d, 0) - 1) / math.log2(i + 2) for i, d in enumerate(ranked[:10]))
    idcg = sum((2 ** r - 1) / math.log2(i + 2) for i, r in enumerate(sorted(rel.values(), reverse=True)[:10]))
    return dcg / idcg if idcg else 0.0


def recall100(ranked, rel):
    return len(set(ranked[:100]) & rel.keys()) / len(rel)


class Vocabulary:
    def __init__(self, df: Counter):
        self.df = df
        self.db = completype.connect(f"memory://vocab-{id(self)}")
        top = math.log1p(max(df.values()))
        words = [dict(id=t, text=t, popularity=float(math.log1p(d) / top)) for t, d in df.items()]
        for start in range(0, len(words), 500_000):
            with self.db.begin() as txn:
                txn.namespace("v").append("w", words[start:start + 500_000])
        self.ns = self.db.engine(sync_every=None).namespace("v")

    def expand_whole_words(self, term):
        """Whole-word corrections only: the closest, then the most common, never a completion."""
        df = self.df.get(term, 0)
        if df >= RARE_DF or len(term) < 3:
            return {term: 1.0}
        hits = self.ns.complete(term, ["w"], 16)
        scored = []
        for h in hits:
            if h.id == term:
                continue
            dist = osa(term, h.id)
            if dist <= (1 if len(term) <= 4 else 2):
                scored.append((dist, -self.df.get(h.id, 0), h.id))
        return self.correction_expansion(term, df, min(scored) if scored else None)

    @staticmethod
    def correction_expansion(term, df, match):
        if match is None:
            return {term: 1.0}
        dist, neg_df, best = match
        if df and -neg_df < MORE_COMMON * df:
            return {term: 1.0}
        out = {best: 0.9 if dist == 1 else 0.7}
        if df:
            out[term] = 1.0
        return out
