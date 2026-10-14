# Python API

The reference for the `typemind` package, generated from its typed stubs:
[`__init__.pyi`](https://github.com/mindyard/typemind/blob/main/crates/typemind-py/python/typemind/__init__.pyi),
[`typemind.pyi`](https://github.com/mindyard/typemind/blob/main/crates/typemind-py/python/typemind/typemind.pyi)
and [`_async.pyi`](https://github.com/mindyard/typemind/blob/main/crates/typemind-py/python/typemind/_async.pyi).
Every name on this page is importable from `typemind`.

## Databases

::: typemind.connect
    options:
      heading_level: 3

`connect(url, ...)` is `Database(url, ...)`: it takes `url`, `options`, `cache_dir` and the build options
`min_word_chars`, `max_edit_distance`, `fuzzy_prefix_chars`, `vector_bits`, `compact_keys` and
`build_threads`. See [Configuration](../guides/configuration.md#database-options).

::: typemind.Database
    options:
      heading_level: 3

::: typemind.DatabaseNamespace
    options:
      heading_level: 3

::: typemind.Transaction
    options:
      heading_level: 3

::: typemind.NamespaceTransaction
    options:
      heading_level: 3

::: typemind.Lease
    options:
      heading_level: 3

::: typemind.Store
    options:
      heading_level: 3

## Indexes and engines

::: typemind.Index
    options:
      heading_level: 3

::: typemind.Segment
    options:
      heading_level: 3

::: typemind.SegmentWriter
    options:
      heading_level: 3

::: typemind.Engine
    options:
      heading_level: 3

::: typemind.Namespace
    options:
      heading_level: 3

::: typemind.Replica
    options:
      heading_level: 3

## Many writers

::: typemind.ChangeSet
    options:
      heading_level: 3

::: typemind.NamespaceChanges
    options:
      heading_level: 3

::: typemind.Ingestor
    options:
      heading_level: 3

## Documents

::: typemind.Document
    options:
      heading_level: 3

::: typemind.DocumentDict
    options:
      heading_level: 3

::: typemind.Id
    options:
      heading_level: 3

::: typemind.Documents
    options:
      heading_level: 3

::: typemind.Vector
    options:
      heading_level: 3

::: typemind.Vectors
    options:
      heading_level: 3

::: typemind.key_id
    options:
      heading_level: 3

## Suggestions

::: typemind.Suggestion
    options:
      heading_level: 3

::: typemind.AliasSuggestion
    options:
      heading_level: 3

::: typemind.HybridSuggestion
    options:
      heading_level: 3

::: typemind.MatchKind
    options:
      heading_level: 3

::: typemind.FusionKind
    options:
      heading_level: 3

## asyncio

::: typemind.AsyncDatabase
    options:
      heading_level: 3

::: typemind.AsyncDatabaseNamespace
    options:
      heading_level: 3

::: typemind.AsyncEngine
    options:
      heading_level: 3

## Errors

Every error derives from `TypemindError`. `NotFoundError` is also a `LookupError`, `InvalidInputError` a
`ValueError`, and `StorageError` an `OSError`. `NamespaceNotFoundError` and `LayerNotFoundError` are
`NotFoundError`s that carry the missing `name` and the names that exist in `available`.

::: typemind.TypemindError
    options:
      heading_level: 3

::: typemind.ConflictError
    options:
      heading_level: 3

::: typemind.CorruptionError
    options:
      heading_level: 3

::: typemind.NotFoundError
    options:
      heading_level: 3

::: typemind.NamespaceNotFoundError
    options:
      heading_level: 3

::: typemind.LayerNotFoundError
    options:
      heading_level: 3

::: typemind.InvalidInputError
    options:
      heading_level: 3

::: typemind.StorageError
    options:
      heading_level: 3

## Collections

The [collections](../guides/collections.md) layer.

::: typemind.Client
    options:
      heading_level: 3

::: typemind.ClientNamespace
    options:
      heading_level: 3

::: typemind.Collection
    options:
      heading_level: 3

::: typemind.CollectionStats
    options:
      heading_level: 3

::: typemind.OptimizeKind
    options:
      heading_level: 3

::: typemind.AsyncClient
    options:
      heading_level: 3

::: typemind.AsyncCollection
    options:
      heading_level: 3

::: typemind.AsyncClientNamespace
    options:
      heading_level: 3
