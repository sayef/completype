# Python API

The reference for the `completype` package, generated from its typed stubs:
[`__init__.pyi`](https://github.com/mindyard/completype/blob/main/crates/completype-py/python/completype/__init__.pyi),
[`completype.pyi`](https://github.com/mindyard/completype/blob/main/crates/completype-py/python/completype/completype.pyi)
and [`_async.pyi`](https://github.com/mindyard/completype/blob/main/crates/completype-py/python/completype/_async.pyi).
Every name on this page is importable from `completype`.

## Databases

::: completype.connect
    options:
      heading_level: 3

`connect(url, ...)` is `Database(url, ...)`: it takes `url`, `options`, `cache_dir` and the build options
`min_word_chars`, `max_edit_distance`, `fuzzy_prefix_chars`, `vector_bits`, `compact_keys` and
`build_threads`. See [Configuration](../guides/configuration.md#database-options).

::: completype.Database
    options:
      heading_level: 3

::: completype.DatabaseNamespace
    options:
      heading_level: 3

::: completype.Transaction
    options:
      heading_level: 3

::: completype.NamespaceTransaction
    options:
      heading_level: 3

::: completype.Lease
    options:
      heading_level: 3

::: completype.Store
    options:
      heading_level: 3

## Indexes and engines

::: completype.Index
    options:
      heading_level: 3

::: completype.Segment
    options:
      heading_level: 3

::: completype.SegmentWriter
    options:
      heading_level: 3

::: completype.Engine
    options:
      heading_level: 3

::: completype.Namespace
    options:
      heading_level: 3

::: completype.Replica
    options:
      heading_level: 3

## Many writers

::: completype.ChangeSet
    options:
      heading_level: 3

::: completype.NamespaceChanges
    options:
      heading_level: 3

::: completype.Ingestor
    options:
      heading_level: 3

## Documents

::: completype.Document
    options:
      heading_level: 3

::: completype.DocumentDict
    options:
      heading_level: 3

::: completype.Id
    options:
      heading_level: 3

::: completype.Documents
    options:
      heading_level: 3

::: completype.Vector
    options:
      heading_level: 3

::: completype.Vectors
    options:
      heading_level: 3

::: completype.key_id
    options:
      heading_level: 3

## Suggestions

::: completype.Suggestion
    options:
      heading_level: 3

::: completype.AliasSuggestion
    options:
      heading_level: 3

::: completype.HybridSuggestion
    options:
      heading_level: 3

::: completype.MatchKind
    options:
      heading_level: 3

::: completype.FusionKind
    options:
      heading_level: 3

## asyncio

::: completype.AsyncDatabase
    options:
      heading_level: 3

::: completype.AsyncDatabaseNamespace
    options:
      heading_level: 3

::: completype.AsyncEngine
    options:
      heading_level: 3

## Errors

Every error derives from `CompletypeError`. `NotFoundError` is also a `LookupError`, `InvalidInputError` a
`ValueError`, and `StorageError` an `OSError`. `NamespaceNotFoundError` and `LayerNotFoundError` are
`NotFoundError`s that carry the missing `name` and the names that exist in `available`.

::: completype.CompletypeError
    options:
      heading_level: 3

::: completype.ConflictError
    options:
      heading_level: 3

::: completype.CorruptionError
    options:
      heading_level: 3

::: completype.NotFoundError
    options:
      heading_level: 3

::: completype.NamespaceNotFoundError
    options:
      heading_level: 3

::: completype.LayerNotFoundError
    options:
      heading_level: 3

::: completype.InvalidInputError
    options:
      heading_level: 3

::: completype.StorageError
    options:
      heading_level: 3

## Collections

The [collections](../guides/collections.md) layer.

::: completype.Client
    options:
      heading_level: 3

::: completype.ClientNamespace
    options:
      heading_level: 3

::: completype.Collection
    options:
      heading_level: 3

::: completype.CollectionStats
    options:
      heading_level: 3

::: completype.OptimizeKind
    options:
      heading_level: 3

::: completype.AsyncClient
    options:
      heading_level: 3

::: completype.AsyncCollection
    options:
      heading_level: 3

::: completype.AsyncClientNamespace
    options:
      heading_level: 3
