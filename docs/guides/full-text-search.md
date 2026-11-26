# Full-text search

`completype.fts` searches whole documents rather than completing a query: exact BM25 over named text
fields, keyword and numeric filters, stored payloads, stemming, stop words and typo tolerance. It wraps the
`completype-fts` crate and keeps an index in a local directory.

!!! note "Experimental"
    `completype.fts` may change in any release, without a semver promise.

## A first index

```python
from completype import fts

index = fts.Index.open("./songs-fts", language="english", stop_words="english")
with index.writer() as w:
    w.add(
        "bohemian",
        {"title": "Bohemian Rhapsody", "artist": "Queen", "about": "A mock opera and a confession"},
        keywords={"genre": ["rock", "opera"]},
        numbers={"year": 1975},
        stored={"title": "Bohemian Rhapsody", "artist": "Queen"},
    )
    w.add(
        "dancing",
        {"title": "Dancing Queen", "artist": "ABBA", "about": "Dancing on a Friday night"},
        keywords={"genre": ["pop", "disco"]},
        numbers={"year": 1976},
        stored={"title": "Dancing Queen", "artist": "ABBA"},
    )
    w.add(
        "dark",
        {"title": "Dancing in the Dark", "artist": "Bruce Springsteen", "about": "Restless nights"},
        keywords={"genre": ["rock"]},
        numbers={"year": 1984},
        stored={"title": "Dancing in the Dark", "artist": "Bruce Springsteen"},
    )

for hit in index.search("queen dances", fields={"title": 3.0, "artist": 2.0, "about": 1.0}):
    print(hit.id, round(hit.score, 3), hit.json())
```

```text
dancing 5.771 {'title': 'Dancing Queen', 'artist': 'ABBA'}
bohemian 2.185 {'title': 'Bohemian Rhapsody', 'artist': 'Queen'}
dark 1.171 {'title': 'Dancing in the Dark', 'artist': 'Bruce Springsteen'}
```

`dances` matches `dancing` through the English stemmer. Leaving out `fields` searches every text field with
boost 1.

## Documents

`Writer.add(id, text, *, keywords, numbers, stored)` declares each field's kind by the argument it is in:

| Argument | Holds | Used for |
|---|---|---|
| `text` | a string for the field `text`, or `{field: text}` | searching, scored by BM25 |
| `keywords` | `{field: value or [values]}`, strings only | `Filter.keyword`, `Filter.keyword_in` |
| `numbers` | `{field: number or [numbers]}`, ints or floats | `Filter.range`, `Filter.equals` |
| `stored` | bytes, a string, or any value saved as JSON | returned as `Hit.stored`, `Hit.json()` |

Types are never guessed: a string in `text` is searched, a string in `keywords` is matched exactly, and a
value of the wrong kind raises `InvalidInputError`. Adding an id again replaces the document.

A writer holds the index's lock. Changes become visible to new searches when it commits: on leaving the
`with` block, or at `w.commit()`. An exception in the block discards them. `w.delete(id)` removes a
document, and `w.merge()` merges every segment into one.

## Filters

```python
print([h.id for h in index.search("dancing", filter=fts.Filter.keyword("genre", "rock"))])
print([h.id for h in index.search("dancing", filter=fts.Filter.range("year", lt=1980))])
rock_not_1975 = fts.Filter.keyword("genre", "rock") & ~fts.Filter.equals("year", 1975)
print([h.id for h in index.search("", filter=rock_not_1975)])
```

```text
['dark']
['dancing']
['dark']
```

`Filter.range` takes `gt`, `ge`, `lt` and `le`. Combine filters with `&`, `|` and `~`. A query without
terms lists the documents the filter allows, with score 0.

## Analysis

- **Typos.** A rare query term also matches the closest much more common indexed spelling, so
  `index.search("springstein")` finds `dark`. `typos=False` turns it off.
- **Stemming.** `language` is one of `fts.LANGUAGES`. The index records its analyzer at the first
  commit and reopens with it; opening it with a different `language` or `keep_accents` raises
  `InvalidInputError`.
- **Stop words.** `stop_words="english"` takes the Snowball list, `fts.stop_words("english")` returns it,
  and a list of words works too. They are left out of queries, never out of the index, and a query of
  nothing but stop words keeps them.

## Snapshots

`index.search` reads the latest commit each time. For many searches, take a snapshot with
`reader = index.reader()` and call `reader.search(...)`; `reader.get(id)` returns a stored payload and
`len(reader)` counts the documents. Searches and commits release the GIL, so threads search in parallel.
