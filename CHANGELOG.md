# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- First release: an embedded autocompletion engine for Rust and Python whose database is an object-store
  bucket or a local directory.
- Exact, prefix, infix, abbreviation, synonym, spelling-tolerant, word-decomposing, semantic and hybrid
  completion, ranked by popularity, match kind and text, with deterministic order.
- Immutable, checksummed segments read in place, streaming builds in bounded memory, and tiered compaction.
- Versioned databases on disk, S3, GCS, Azure or memory, with create-only commits, namespaces, override
  layers, engines that follow new versions, and a lease-elected ingestor for many writers.
- Python bindings with an asyncio API and typed stubs, and the `completype` command line.
