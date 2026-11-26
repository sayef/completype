import threading

import pytest

import completype
from completype.fts import LANGUAGES, Filter, Hit, Index, stop_words

PHONES = [
    ("sku-1", "Apple iPhone 15", "A phone with a 6.1 inch display", "apple", ["phone", "5g"], 799.0, 2023),
    ("sku-2", "Samsung Galaxy S24", "An android phone with a bright display", "samsung", ["phone", "5g"], 899.0, 2024),
    ("sku-3", "Apple MacBook Air", "A light laptop for travel", "apple", ["laptop"], 1199.0, 2024),
    ("sku-4", "Phone case", "A leather case for the iPhone 15", "generic", ["accessory"], 29.0, 2023),
]


@pytest.fixture
def shop(tmp_path):
    index = Index.open(tmp_path / "shop")
    with index.writer() as w:
        for id, title, body, brand, tags, price, year in PHONES:
            w.add(
                id,
                {"title": title, "body": body},
                keywords={"brand": brand, "tags": tags},
                numbers={"price": price, "year": year},
                stored={"title": title, "price": price},
            )
    return index


def ids(hits):
    return [h.id for h in hits]


def test_commit_makes_documents_visible_and_they_survive_reopening(tmp_path):
    index = Index.open(tmp_path / "idx")
    w = index.writer()
    w.add("1", "restaurant reviews in Zürich")
    w.add("2", "a recipe for sourdough bread")
    assert len(index.reader()) == 0
    w.commit()
    w.close()
    assert len(index.reader()) == 2
    reopened = Index.open(tmp_path / "idx")
    [hit] = reopened.search("zurich")
    assert isinstance(hit, Hit) and hit.id == "1" and hit.score > 0 and hit.stored is None
    match hit:
        case Hit(id, _, stored):
            assert (id, stored) == ("1", None)


def test_a_failed_block_discards_its_changes_and_releases_the_lock(tmp_path):
    index = Index.open(tmp_path / "idx")
    with pytest.raises(RuntimeError), index.writer() as w:
        w.add("1", "lost")
        raise RuntimeError
    assert len(index.reader()) == 0
    with index.writer() as w:
        w.add("1", "kept")
    assert ids(index.search("kept")) == ["1"]
    with pytest.raises(ValueError, match="closed"):
        w.add("2", "late")


def test_one_writer_at_a_time(tmp_path):
    index = Index.open(tmp_path / "idx")
    w = index.writer()
    with pytest.raises(completype.StorageError, match="another writer"):
        index.writer()
    w.close()
    index.writer().close()


def test_fields_boosts_and_stored_payloads(shop):
    assert ids(shop.search("iphone", fields={"title": 3.0, "body": 1.0})) == ["sku-1", "sku-4"]
    assert ids(shop.search("iphone", fields={"title": 1.0, "body": 3.0})) == ["sku-4", "sku-1"]
    assert ids(shop.search("iphone", fields=["title"])) == ["sku-1"]
    top = shop.search("macbook")[0]
    assert top.json() == {"title": "Apple MacBook Air", "price": 1199.0}
    assert shop.search("macbook", stored=False)[0].stored is None
    reader = shop.reader()
    assert reader.get("sku-2") == b'{"title":"Samsung Galaxy S24","price":899.0}'
    assert reader.get("missing") is None


def test_stored_bytes_and_strings_are_kept_as_given(tmp_path):
    index = Index.open(tmp_path / "idx")
    with index.writer() as w:
        w.add("b", "bytes", stored=b"\x00\xff")
        w.add("s", "string", stored="héllo")
    reader = index.reader()
    assert reader.get("b") == b"\x00\xff"
    assert reader.get("s") == "héllo".encode()


def test_filters(shop):
    def search(f, query="phone"):
        return sorted(ids(shop.search(query, filter=f)))

    assert search(Filter.keyword("brand", "apple")) == ["sku-1"]
    assert search(Filter.keyword_in("brand", ["apple", "samsung"])) == ["sku-1", "sku-2"]
    assert search(Filter.keyword("tags", "5g")) == ["sku-1", "sku-2"]
    assert search(Filter.range("price", lt=800)) == ["sku-1", "sku-4"]
    assert search(Filter.range("price", gt=799, le=899.0)) == ["sku-2"]
    assert search(Filter.equals("year", 2024)) == ["sku-2"]
    assert search(Filter.keyword("brand", "apple") | Filter.equals("year", 2024)) == ["sku-1", "sku-2"]
    assert search(Filter.keyword("tags", "phone") & ~Filter.keyword("brand", "apple")) == ["sku-2"]
    assert search(Filter.keyword("brand", "apple"), query="") == ["sku-1", "sku-3"]
    assert search(Filter.keyword("brand", "nokia")) == []
    assert repr(Filter.range("price", ge=1, lt=2.5) & ~Filter.keyword("b", "x")) == (
        "(Filter.range('price', ge=1, lt=2.5) & ~Filter.keyword('b', 'x'))"
    )
    assert Filter.keyword("a", "b") == Filter.keyword("a", "b")
    with pytest.raises(completype.InvalidInputError, match="not both"):
        Filter.range("price", gt=1, ge=2)


def test_delete_and_merge(shop):
    with shop.writer() as w:
        w.add("sku-5", {"title": "Pixel phone"}, keywords={"brand": "google"})
    assert shop.reader().segment_count == 2
    with shop.writer() as w:
        w.delete("sku-1")
        w.delete("unknown")
    assert "sku-1" not in ids(shop.search("phone"))
    assert shop.reader().get("sku-1") is None
    with shop.writer() as w:
        w.merge()
    reader = shop.reader()
    assert (len(reader), reader.segment_count) == (4, 1)
    assert sorted(ids(shop.search("phone"))) == ["sku-2", "sku-4", "sku-5"]


def test_adding_an_id_again_replaces_the_document(shop):
    with shop.writer() as w:
        w.add("sku-1", {"title": "Apple Watch"}, keywords={"brand": "apple"})
    assert len(shop.reader()) == 4
    assert "sku-1" not in ids(shop.search("iphone"))
    assert ids(shop.search("watch")) == ["sku-1"]
    assert shop.reader().get("sku-1") is None


def test_typo_correction(shop):
    assert ids(shop.search("macbok"))[0] == "sku-3"
    assert shop.search("macbok", typos=False) == []
    assert ids(shop.search("samsnug galaxy", limit=1)) == ["sku-2"]


def test_stemming(tmp_path):
    index = Index.open(tmp_path / "idx", language="english")
    assert index.language == "english"
    with index.writer() as w:
        w.add("1", "running dogs")
    assert ids(index.search("run dog", typos=False)) == ["1"]
    # The analyzer is recorded with the index: reopening adopts it, a different one is refused.
    assert Index.open(tmp_path / "idx").language == "english"
    with pytest.raises(completype.InvalidInputError, match="analyzer"):
        Index.open(tmp_path / "idx", language="german").reader()
    plain = Index.open(tmp_path / "plain")
    with plain.writer() as w:
        w.add("1", "running dogs")
    assert plain.search("run dog", typos=False) == []
    with pytest.raises(completype.InvalidInputError, match="unknown language"):
        Index.open(tmp_path / "x", language="klingon")
    assert "english" in LANGUAGES


def test_stop_words(tmp_path):
    path = tmp_path / "idx"
    with Index.open(path).writer() as w:
        w.add("1", "the cat")
        w.add("2", "the dog and the bird and the fish")
    assert sorted(ids(Index.open(path).search("the cat"))) == ["1", "2"]
    english = Index.open(path, stop_words="english")
    assert ids(english.search("the cat")) == ["1"]
    assert sorted(ids(english.search("the"))) == ["1", "2"]
    custom = Index.open(path, stop_words=["cat"])
    assert ids(custom.search("cat dog")) == ["2"]
    assert "the" in stop_words("english")
    with pytest.raises(completype.InvalidInputError, match="no Snowball stop word list"):
        stop_words("greek")


def test_field_kinds_are_checked(tmp_path):
    with Index.open(tmp_path / "idx").writer() as w:
        with pytest.raises(completype.InvalidInputError, match='keyword field "n" must hold strings'):
            w.add("1", keywords={"n": 5})
        with pytest.raises(completype.InvalidInputError, match="not bools"):
            w.add("1", numbers={"flag": True})
        with pytest.raises(completype.InvalidInputError, match="must hold ints or floats"):
            w.add("1", numbers={"year": "2020"})
        with pytest.raises(completype.InvalidInputError, match="text must be a mapping"):
            w.add("1", 42)
        w.add("1", {"title": ["two", "parts"], "body": None}, numbers={"sizes": [1, 2.5]})
    index = Index.open(tmp_path / "idx")
    assert ids(index.search("parts", filter=Filter.equals("sizes", 2.5))) == ["1"]


def test_searches_run_in_parallel_threads(shop):
    reader = shop.reader()
    results = []

    def search():
        results.append(ids(reader.search("phone", filter=Filter.keyword("brand", "apple"))))

    threads = [threading.Thread(target=search) for _ in range(8)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert results == [["sku-1"]] * 8
