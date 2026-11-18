# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

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
