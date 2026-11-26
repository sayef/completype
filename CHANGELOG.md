# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Experimental `completype.fts` Python module over `completype-fts`: `Index`, `Writer`, `Reader`, `Filter` and
  `Hit`, with text, keyword and numeric fields, stored payloads, stemming, stop words and typo tolerance.
  `Index.open(path, positions=True)` keeps token positions, so quoted phrases (`"new york"`, `"new york"~2`) match
  as phrases; without positions their words are searched as plain terms.

## [0.2.4](https://github.com/sayef/completype/compare/v0.2.3...v0.2.4) - 2026-11-25

### Changed

- Vector quantisation and search moved into the new `completype-vector` crate, which `completype` depends on; the API, behaviour and file format are unchanged ([#74](https://github.com/sayef/completype/pull/74))

## [0.2.3](https://github.com/sayef/completype/compare/v0.2.2...v0.2.3) - 2026-11-20

### Fixed

- A query that equals a document's whole abbreviation scores at least as a prefix match. It used to
  drop to an infix match when the title also contained the query, so `Microsoft Excel` with the
  abbreviation `EXCEL` ranked below less popular titles starting with "Excel".

### Changed

- Release wheels are built from `Cargo.lock` (`--locked`), and Dependabot waits 7 days after a
  dependency's release before proposing it.

## [0.2.2](https://github.com/sayef/completype/compare/v0.2.1...v0.2.2) - 2026-11-19

### Fixed

- `Engine.begin()` (`Replica::begin`) transactions are strict, so a commit fails instead of rebasing
  over a write it did not see, and `begin` on an engine that never synced a non-empty database fails
  instead of comparing with nothing.
- `sync` without `ids` and `prune` keep a layer's hide markers; passing a hidden id in `ids` still
  unhides it. `sync` rewrites a document whose stored vector the source no longer has.
- `fields` from pyarrow, polars and pandas tables are accepted: maps as `(name, value)` pairs, `None`
  values left out, and `None` or NaN as no fields.
- `Engine.transact` retries only conflicts a newer version can resolve (`Error::is_stale`), and
  `AsyncEngine.transact` refuses an `async` function instead of silently staging nothing.
- Compaction writes the stored-fields segment format only when a remaining document has fields, so
  older releases keep reading the rest.

## [0.2.1](https://github.com/sayef/completype/compare/v0.2.0...v0.2.1) - 2026-11-18

### Added

- Stored fields: `Document.fields` (`with_field` in Rust, `fields` in Python) are kept with a document
  and returned with it, never searched. Segments without them are written as before, so older releases
  still read them.
- `Engine.begin()` (`Replica::begin` in Rust) starts a transaction against the version an engine serves.
  Its `sync(index, documents, ids)` writes only what differs from what the engine serves, and
  `prune(index, keep)` deletes what is no longer wanted. `Index::changes` returns what `sync` would write.
- `Engine.transact(function)` syncs, runs `function` with a strict transaction and commits, again
  while another writer changed an index it touches in between.
- `Transaction.hide(index, ids)` hides ids from earlier layers, and `Transaction.is_empty()` tells whether
  anything is staged. `Index.ids()` lists the live ids in Python.

## [0.2.0](https://github.com/sayef/completype/compare/v0.1.3...v0.2.0) - 2026-11-18

### Changed

- **Breaking:** in a layered search, a layer hides an id from earlier layers only while it holds a live
  document with that id; hide one with a document that has no text. A deletion now only removes the
  layer's own documents. Before, a deletion also hid the id in every earlier layer, but compaction could
  drop the deletion, so results changed depending on how a layer's segments had been merged, and
  deleting a layer's override hid the original until then. A layer that hid ids with `deletes` should
  write empty documents instead.

## [0.1.3](https://github.com/sayef/completype/compare/v0.1.2...v0.1.3) - 2026-11-18

### Fixed

- Python: the storage clients completype uses reach `logging` only with warnings and errors. Before, their
  debug and info messages did too, printing credential loading details, such as the AWS access key id and
  account, and request details into application logs.
- The musllinux wheel imports on Alpine instead of failing with "initial-exec TLS resolves to dynamic
  definition".

## [0.1.2](https://github.com/sayef/completype/compare/v0.1.1...v0.1.2) - 2026-11-17

### Added

- `layer_weights` on layered searches and `Collection.complete`: a multiplier per layer for its scores, so
  a layer's own matches can rank higher. Without it, every layer stays on the first layer's scale.

### Fixed

- Databases, engines, clients and indexes opened before `fork()` keep working in the forked children
  instead of hanging.
- A long word that only matches once split in two keeps the matches it has as typed: in a German catalogue,
  "versicherungsrec" no longer returns nothing.
- Matches that ignore a word too short to index, such as "HR" in "HR policy", fill the list below the full
  matches instead of only when nothing matches.
- A long word corrected by splitting it is labelled `fuzzy`, not `prefix` or `infix`.
- The crates.io and PyPI pages link to the documentation site.
- A commit, lease or change set whose write landed but was reported as failed, or as already existing
  because the store client retried it, counts once. Before, a commit could be applied twice or fail
  with a conflict against itself. When a write's outcome cannot be read back, the commit raises the new
  `OutcomeUnknown` error (`OutcomeUnknownError` in Python, a `StorageError`).
- Lease expiry is measured by the store's clock, from the lease object's modification time, so a machine
  with a fast clock can no longer take over a live lease. Leases written by earlier versions keep the
  old rule.
- Reads, listings, deletes and plain writes are retried after transient store errors, with jittered
  backoff. Create-only writes are never repeated blindly.
- Before its first commit, lease or change set, a database checks once that the store refuses a second
  create-only write to the same key. Some S3-compatible stores ignore `If-None-Match`, which would let
  concurrent writers overwrite each other; completype now refuses to write to them.
- Cleanup also removes clock and create-only probe objects left by a process that stopped between
  writing and deleting them.

### Changed

- Concurrent `Collection` writes to one namespace share commits: writes that arrive while a commit runs
  go into the next one together. Each write still returns once it is durable, with the version that
  holds it.

## [0.1.1](https://github.com/sayef/completype/releases/tag/v0.1.1) - 2026-10-15

### Added

- First release: an embedded autocompletion engine for Rust and Python whose database is an object-store
  bucket or a local directory.
- Exact, prefix, infix, abbreviation, synonym, spelling-tolerant, word-decomposing, semantic and hybrid
  completion, ranked by popularity, match kind and text, with deterministic order.
- Immutable, checksummed segments read in place, streaming builds in bounded memory, and tiered compaction.
- Versioned databases on disk, S3, GCS, Azure or memory, with create-only commits, namespaces, override
  layers, engines that follow new versions, and a lease-elected ingestor for many writers.
- Python bindings with an asyncio API and typed stubs, and the `completype` command line.
