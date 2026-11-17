import json
import os
import signal
import time
import warnings

import pytest

import completype

pytestmark = pytest.mark.skipif(not hasattr(os, "fork"), reason="needs os.fork")


def in_child(work, timeout=30.0):
    """Runs `work` in a forked child and returns what it returned; a hang fails after `timeout`."""
    read, write = os.pipe()
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", DeprecationWarning)  # fork with threads running
        pid = os.fork()
    if pid == 0:
        os.close(read)
        try:
            out = {"ok": work()}
        except BaseException as e:
            out = {"error": f"{type(e).__name__}: {e}"}
        os.write(write, json.dumps(out).encode())
        os._exit(0)
    os.close(write)
    deadline = time.monotonic() + timeout
    while os.waitpid(pid, os.WNOHANG) == (0, 0):
        if time.monotonic() > deadline:
            os.kill(pid, signal.SIGKILL)
            os.waitpid(pid, 0)
            pytest.fail("the forked child hung")
        time.sleep(0.02)
    with os.fdopen(read) as pipe:
        out = json.loads(pipe.read() or '{"error": "the child wrote nothing"}')
    assert "error" not in out, out["error"]
    return out["ok"]


def texts(hits):
    return sorted(h.text for h in hits)


def test_a_forked_child_commits_and_syncs_a_database_opened_before(tmp_path):
    db = completype.connect(str(tmp_path / "db"))
    t = db.begin()
    t.append("s", [{"id": 1, "text": "PlayStation 5", "popularity": 1.0}])
    t.commit()
    engine = db.engine(sync_every=None)
    assert texts(engine.complete("play", ["s"])) == ["PlayStation 5"]

    def child():
        t = db.begin()
        t.append("s", [{"id": 2, "text": "Play-Doh", "popularity": 2.0}])
        t.commit()
        engine.sync()
        return texts(engine.complete("play", ["s"]))

    assert in_child(child) == ["Play-Doh", "PlayStation 5"]


def test_a_forked_child_follows_commits_by_itself(tmp_path):
    db = completype.connect(str(tmp_path / "db"))
    t = db.begin()
    t.append("s", [{"id": 1, "text": "PlayStation 5", "popularity": 1.0}])
    t.commit()
    engine = db.engine(sync_every=0.02)
    engine.complete("play", ["s"])

    def child():
        t = db.begin()
        t.append("s", [{"id": 2, "text": "Play-Doh", "popularity": 2.0}])
        t.commit()
        for _ in range(500):
            if len(engine.complete("play", ["s"])) == 2:
                return True
            time.sleep(0.01)
        return False

    assert in_child(child)


@pytest.mark.parametrize("threads", [0, 1, 2])
def test_a_forked_child_builds_and_searches_vectors(threads):
    np = pytest.importorskip("numpy")
    docs = [{"id": i, "text": f"item {i} word{i % 97}", "popularity": 0.5} for i in range(2000)]
    vectors = np.random.default_rng(0).standard_normal((2000, 16)).astype(np.float32)

    def build():
        segment = completype.Segment.build(docs, vectors=vectors, vector_bits=4)
        return completype.Index([segment], max_score=1000.0, vector_threads=threads)

    index = build()
    expected = [h.id for h in index.vector_search(vectors[1], 3)]
    assert in_child(lambda: [h.id for h in build().vector_search(vectors[1], 3)]) == expected
    assert in_child(lambda: [h.id for h in index.vector_search(vectors[1], 3)]) == expected


def test_a_forked_child_uses_a_collection_opened_before(tmp_path):
    products = completype.Client(str(tmp_path / "db"), sync_every=None).get_or_create_collection("p")
    products.add([{"id": 1, "text": "PlayStation 5", "popularity": 1.0}])

    def child():
        products.add([{"id": 2, "text": "Play-Doh", "popularity": 2.0}])
        return texts(products.complete("play"))

    assert in_child(child) == ["Play-Doh", "PlayStation 5"]
