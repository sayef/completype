"""Experimental full-text search: BM25, filters, stemming and typo tolerance. The API may change in any release."""

from collections.abc import Mapping, Sequence
from os import PathLike
from pathlib import Path
from types import TracebackType
from typing import Any, Literal, Self

LANGUAGES: tuple[str, ...]
"""Languages `Index.open` can stem, by Snowball name."""

Text = str | Mapping[str, str | Sequence[str] | None]
"""A string for the field `text`, or searchable text by field name."""
Keywords = Mapping[str, str | Sequence[str] | None]
"""Exact values to filter on, by field name; a field may hold several."""
Numbers = Mapping[str, int | float | Sequence[int | float] | None]
"""Numbers to filter on, by field name; a field may hold several."""
Stored = bytes | bytearray | str | Any
"""A payload kept with a document: bytes, a string, or a value saved as JSON."""
Fields = Mapping[str, float] | Sequence[str]
"""Text fields to search, with a boost each if a mapping."""

def stop_words(language: str) -> list[str]:
    """The Snowball project's stop words for `language`."""

class Index:
    """A full-text index in a directory, written by one `Writer` and searched through `Reader` snapshots."""
    @staticmethod
    def open(
        path: str | PathLike[str],
        *,
        language: str | None = None,
        keep_accents: bool | None = None,
        stop_words: str | Sequence[str] | None = None,
        reordering: Literal["never", "on_merge", "always"] = "on_merge",
        first_letter_typos: bool = False,
    ) -> Index:
        """Opens the index in `path`, creating it if missing. `language` and `keep_accents` default to what the
        index was built with; `stop_words`, a language or a list of words, are left out of queries."""
    @property
    def path(self) -> Path: ...
    @property
    def language(self) -> str | None:
        """The stemming language, or `None` when words are not stemmed."""
    def writer(self) -> Writer:
        """The only writer; raises `StorageError` while another holds the lock."""
    def reader(self) -> Reader:
        """A snapshot of the committed documents."""
    def search(
        self,
        query: str,
        limit: int = 10,
        *,
        fields: Fields | None = None,
        filter: Filter | None = None,
        typos: bool = True,
        stored: bool = True,
    ) -> list[Hit]:
        """`Reader.search` on a snapshot of the latest commit."""

class Writer:
    """Adds and deletes documents until a commit makes them visible. As a context manager it commits on success,
    discards the changes on an exception, and releases the lock."""
    def add(
        self,
        id: str,
        text: Text | None = None,
        *,
        keywords: Keywords | None = None,
        numbers: Numbers | None = None,
        stored: Stored | None = None,
    ) -> None:
        """Adds a document, replacing any with the same id."""
    def delete(self, id: str) -> None:
        """Deletes the document `id`, committed or not; an unknown id is ignored."""
    def commit(self) -> None:
        """Makes the changes so far visible to new readers, atomically."""
    def merge(self) -> None:
        """Merges every committed segment into one, dropping deleted documents."""
    def close(self) -> None:
        """Discards uncommitted changes and releases the lock."""
    def __enter__(self) -> Self: ...
    def __exit__(
        self, exc_type: type[BaseException] | None, exc: BaseException | None, tb: TracebackType | None
    ) -> bool: ...

class Reader:
    """A consistent view of the index at one commit."""
    def search(
        self,
        query: str,
        limit: int = 10,
        *,
        fields: Fields | None = None,
        filter: Filter | None = None,
        typos: bool = True,
        stored: bool = True,
    ) -> list[Hit]:
        """The best `limit` documents for `query` by BM25 within `filter`, in every text field unless `fields`
        names some. A query without terms lists the documents `filter` allows."""
    def get(self, id: str) -> bytes | None:
        """The stored payload of document `id`, or `None` if it has none or does not exist."""
    @property
    def segment_count(self) -> int: ...
    def __len__(self) -> int: ...

class Hit:
    __match_args__ = ("id", "score", "stored")
    id: str
    score: float
    stored: bytes | None
    """The stored payload, unless the search passed `stored=False`."""
    def json(self) -> Any:
        """The stored payload parsed as JSON, or `None` without one."""

class Filter:
    """Which documents a search may return. Combine with `&`, `|` and `~`."""
    @staticmethod
    def keyword(field: str, value: str) -> Filter:
        """Documents whose keyword field `field` holds `value`."""
    @staticmethod
    def keyword_in(field: str, values: Sequence[str]) -> Filter:
        """Documents whose keyword field `field` holds any of `values`."""
    @staticmethod
    def range(
        field: str,
        *,
        gt: int | float | None = None,
        ge: int | float | None = None,
        lt: int | float | None = None,
        le: int | float | None = None,
    ) -> Filter:
        """Documents whose numeric field `field` holds a value within the bounds; no bound means any value."""
    @staticmethod
    def equals(field: str, value: int | float) -> Filter:
        """Documents whose numeric field `field` holds `value`."""
    def __and__(self, other: Filter) -> Filter: ...
    def __or__(self, other: Filter) -> Filter: ...
    def __invert__(self) -> Filter: ...
