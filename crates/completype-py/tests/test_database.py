import os
import time

import pytest

import completype

from completype import ConflictError
from completype import ChangeSet, Database, Engine, Replica, Store, Ingestor

DOCS = [
    {"id": i, "text": f"item {name} {i}", "popularity": (i % 10) / 10}
    for i, name in enumerate(["rust", "python", "data", "cloud"] * 25)
]

URLS = ["memory:///py"]
if "COMPLETYPE_TEST_S3_URL" in os.environ:
    URLS.append(f"{os.environ['COMPLETYPE_TEST_S3_URL'].rstrip('/')}/py-database")


@pytest.fixture(params=["local", *URLS])
def database(request, tmp_path):
    url = str(tmp_path / "ds") if request.param == "local" else f"{request.param}-{time.time_ns()}"
    yield Database(url)
    store = Store(url)
    for key in store.list():
        store.delete(key)


def test_commit_rebase_conflict(database):
    t = database.begin()
    t.append("catalog", DOCS)
    t.set_max_score("catalog", 1000.0)
    t.set_metadata("cursor", "42")
    assert t.commit()["version"] == 1

    first, second, strict = database.begin(), database.begin(), database.begin()
    first.append("catalog", [{"id": 1000, "text": "rust async", "popularity": 0.9}], deletes=[0])
    second.append("catalog", [{"id": 1001, "text": "rust macros", "popularity": 0.8}])
    strict.append("catalog", [{"id": 1002, "text": "rust unsafe", "popularity": 0.1}])
    strict.strict()
    first.commit()
    manifest = second.commit()
    assert manifest["version"] == 3 and manifest["metadata"] == {"cursor": "42"}
    with pytest.raises(ConflictError):
        strict.commit()

    index = database.open_index("catalog")
    assert len(index) == 101
    assert {h.id for h in index.complete("rust a")} >= {1000}


def test_compact_cleanup_and_follow(database):
    t = database.begin()
    t.append("catalog", DOCS)
    t.commit()
    for i in range(4):
        t = database.begin()
        t.append("catalog", [{"id": 2000 + i, "text": f"cloud native {i}", "popularity": 0.5}], deletes=[i])
        t.commit()

    engine = Engine()
    replica = Replica(database, engine)
    assert replica.sync() == 5
    before = [(h.id, h.score) for h in engine.complete("cloud", ["catalog"], 20)]

    assert database.compact("catalog") == 6
    assert replica.sync() == 6 and replica.sync() is None
    assert [(h.id, h.score) for h in engine.complete("cloud", ["catalog"], 20)] == before

    stats = database.cleanup(keep_versions=1, older_than_seconds=0)
    assert stats["versions_removed"] == 5 and stats["segments_removed"] == 4
    assert database.versions() == [6]


def test_lease(database):
    lease = database.acquire_lease("compactor", "worker-a", 60)
    assert lease is not None and database.acquire_lease("compactor", "worker-b", 60) is None
    assert lease.renew(60) and lease.generation == 2
    lease.release()
    other = database.acquire_lease("compactor", "worker-b", 60)
    assert other is not None and other.generation == 4
    other.release()


def test_inbox_and_single_writer(database):
    for n in range(3):
        batch = ChangeSet()
        batch.upsert("catalog", [{"id": 5000 + n, "text": f"streamed item {n}", "popularity": 0.4}])
        if n == 2:
            batch.delete("catalog", [5000])
        database.submit(batch)
    assert database.pending_change_sets() == 3
    with pytest.raises(ValueError):
        database.submit(batch)

    leader, standby = Ingestor(database, "a"), Ingestor(database, "b")
    step = leader.run_once()
    assert step["step"] == "committed" and step["change_sets"] == 3 and leader.is_active
    assert standby.run_once()["step"] == "standby"
    assert leader.run_once()["step"] == "idle"
    index = database.open_index("catalog")
    assert index.get(5000) is None and index.get(5002).text == "streamed item 2"
    leader.release()
    assert standby.run_once()["step"] == "idle" and standby.is_active
    standby.release()


def test_namespaces_switch_whole_and_writers_compact(database):
    with database.namespace("en").begin() as t:
        for tenant in ("base", "acme"):
            t.append(tenant, DOCS[:20])
        t.namespace("de").append("base", DOCS[:20])
    engine = Engine()
    replica = Replica(database, engine)
    replica.sync()
    assert engine.namespaces() == ["de", "en"]
    assert engine.namespace("en").names() == ["acme", "base"]
    held = engine.namespace("en")
    ingestor = Ingestor(database, "w")
    for n in range(6):
        batch = ChangeSet()
        batch.namespace("en").upsert("base", [{"id": 9000 + n, "text": f"extra {n}", "popularity": 0.2}])
        database.submit(batch)
        ingestor.run_once()
    replica.sync()
    en = engine.namespace("en")
    assert len(en.get("base")) == 26 and len(en.get("base").segments()) < 7
    assert len(held.get("base")) == 20 and held.version < en.version
    assert database.namespace("en").index_names() == ["acme", "base"]
    ingestor.release()


def test_a_failed_block_commits_nothing(database):
    with pytest.raises(RuntimeError):
        with database.namespace("en").begin() as t:
            t.append("base", DOCS[:5])
            raise RuntimeError("abandoned")
    assert database.namespaces() == []


def test_threaded_queries_during_updates(database):
    import random
    import threading

    t = database.begin()
    t.append(
        "catalog",
        [{"id": i, "text": f"item {w} {i}", "popularity": (i % 10) / 10} for i, w in enumerate(["rust", "python", "data", "cloud"] * 500)],
    )
    t.set_max_score("catalog", 1000.0)
    t.commit()
    engine = Engine()
    replica = Replica(database, engine)
    replica.sync()
    stop, samples, errors = threading.Event(), [], []

    def read(seed):
        rng = random.Random(seed)
        try:
            while not stop.is_set():
                query = rng.choice(["ru", "pyth", "data it", "cloud 1", "ite", "rsut"])
                index = engine.get("catalog")
                result = [(h.id, h.score, h.kind) for h in index.complete(query, 10)]
                if rng.random() < 0.05:
                    samples.append((index, query, result))
        except Exception as e:  # noqa: BLE001
            errors.append(e)

    readers = [threading.Thread(target=read, args=(n,)) for n in range(6)]
    for r in readers:
        r.start()
    ingestor = Ingestor(database, "w")
    for n in range(30):
        batch = ChangeSet()
        batch.upsert("catalog", [{"id": 10_000 + n, "text": f"rust streamed {n}", "popularity": 0.3}, {"id": n, "text": f"python renamed {n}", "popularity": 0.9}])
        database.submit(batch)
        ingestor.run_once()
        replica.sync()
    stop.set()
    for r in readers:
        r.join()
    ingestor.release()

    assert not errors
    assert len({id(index) for index, _, _ in samples}) > 1
    for index, query, result in samples:
        assert [(h.id, h.score, h.kind) for h in index.complete(query, 10)] == result


def test_tags_are_accepted_everywhere_and_ignored_by_local_stores(tmp_path):
    tags = {"LifecycleRule": "KeepForever"}
    db = completype.connect(str(tmp_path / "db"), tags=tags)
    with db.begin() as txn:
        txn.append("songs", DOCS[:2])
    assert db.index_names() == ["songs"]
    completype.Store(str(tmp_path / "store"), tags=tags).put("a", b"x")
    client = completype.Client(str(tmp_path / "client"), sync_every=None, tags=tags)
    client.create_collection("songs").add(DOCS[:2])


def test_stored_fields_come_back_with_documents_and_survive_compaction(tmp_path):
    database = completype.connect(str(tmp_path))
    for key, text in [("ml", "Machine Learning"), ("mv", "Machine Vision")]:
        txn = database.begin()
        txn.append("skills", [{"id": key, "text": text, "fields": {"title_en": text}}])
        txn.commit()
    database.namespace("default").compact("skills", max_segments=1, until_done=True)
    engine = database.engine(sync_every=None)
    engine.sync()
    document = engine.namespace("default").get("skills").get("mv")
    assert document.fields == {"title_en": "Machine Vision"}
    assert completype.Document("x", "X", fields={"a": "b"}).to_dict()["fields"] == {"a": "b"}


def test_engine_transactions_write_only_what_changed(tmp_path):
    database = completype.connect(str(tmp_path))
    rows = [{"id": i, "text": f"Skill {i}", "popularity": 0.5} for i in range(5)]
    txn = database.begin()
    txn.append("skills", rows)
    txn.commit()
    engine = database.engine(sync_every=None)
    engine.sync()

    txn = engine.begin()
    txn.sync("skills", rows)
    assert txn.is_empty()

    txn = engine.begin()
    txn.sync("skills", [{"id": 1, "text": "Renamed", "popularity": 0.5}], ids=[1, 2])
    txn.hide("skills", [3])
    txn.commit()
    engine.sync()
    skills = engine.namespace("default").get("skills")
    assert [skills.get(i).text if skills.get(i) else None for i in range(5)] == [
        "Skill 0", "Renamed", None, "", "Skill 4"
    ]

    txn = engine.begin()
    txn.prune("skills", [0, 1])
    txn.commit()
    engine.sync()
    # The hide marker for 3 stays: prune keeps hide markers.
    assert engine.namespace("default").get("skills").ids() == [0, 1, 3]
    with pytest.raises(completype.InvalidInputError):
        database.begin().sync("skills", rows)


def test_transact_retries_when_another_writer_got_there_first(tmp_path):
    database = completype.connect(str(tmp_path))
    txn = database.begin()
    txn.append("skills", [{"id": 1, "text": "Skill 1"}])
    txn.commit()
    engine = database.engine(sync_every=None)
    other = database.engine(sync_every=None)
    calls = []

    def write(txn):
        calls.append(txn.read_version)
        if len(calls) == 1:
            other.transact(lambda t: t.sync("skills", [{"id": 1, "text": "Theirs"}], ids=[1]))
        txn.sync("skills", [{"id": 1, "text": "Ours"}], ids=[1])

    assert engine.transact(write)["version"] == 3
    assert calls == [1, 2]
    assert engine.transact(lambda txn: txn.sync("skills", [{"id": 1, "text": "Ours"}], ids=[1])) is None
    engine.sync()
    assert engine.namespace("default").get("skills").get(1).text == "Ours"


def test_fields_come_from_pyarrow_polars_and_pandas_tables():
    pa = pytest.importorskip("pyarrow")
    pd = pytest.importorskip("pandas")
    rows = [{"id": 1, "text": "Machine Learning"}, {"id": 2, "text": "Machine Vision"}]
    maps = pa.table({
        "id": [1, 2],
        "text": ["Machine Learning", "Machine Vision"],
        "fields": pa.array([[("lang", "en")], None], type=pa.map_(pa.string(), pa.string())),
    })
    frame = pd.DataFrame(rows).assign(fields=[{"lang": "en"}, float("nan")])
    tables = [maps, frame]
    try:
        import polars as pl

        tables.append(pl.DataFrame({"id": [1, 2], "text": ["Machine Learning", "Machine Vision"],
                                    "fields": [{"lang": "en"}, {"lang": None}]}))
    except ImportError:
        pass
    for table in tables:
        index = completype.Index.from_documents(table)
        assert (index.get(1).fields, index.get(2).fields) == ({"lang": "en"}, {})


def test_async_transact_refuses_async_functions(tmp_path):
    import asyncio

    async def write(txn):
        txn.sync("skills", [])

    engine = completype.AsyncEngine(completype.connect(str(tmp_path)).engine(sync_every=None))
    with pytest.raises(TypeError):
        asyncio.run(engine.transact(write))


def test_sync_outside_an_engine_transaction_names_engine_begin(tmp_path):
    with pytest.raises(completype.InvalidInputError, match=r"Engine\.begin\(\)"):
        completype.connect(str(tmp_path)).begin().sync("skills", [])
