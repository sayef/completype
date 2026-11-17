# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

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
