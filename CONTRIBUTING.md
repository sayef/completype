# Contributing to completype

Thanks for your interest in completype. Bug reports, benchmarks, documentation fixes and code are all welcome.

## Before you start

- For anything beyond a small fix, open an issue first so we can agree on the approach.
- Security issues go through [SECURITY.md](SECURITY.md), not public issues.
- By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).

## Development setup

You need Rust 1.94.1 or newer (the minimum supported version, checked in CI) and, for the Python
bindings, Python 3.11+ with [maturin](https://www.maturin.rs/).

```sh
git clone https://github.com/sayef/completype && cd completype
cargo test --release -p completype --features store

python -m venv .venv && . .venv/bin/activate
pip install maturin pytest numpy
maturin develop --release
pytest crates/completype-py/tests
```

Cloud tests run only when you point them at a bucket you own:

```sh
COMPLETYPE_TEST_S3_URL=s3://your-bucket/completype-tests cargo test --release -p completype --features aws-credentials
```

The documentation site is MkDocs with the Material theme:

```sh
pip install -r docs/requirements.txt
mkdocs serve               # preview at http://127.0.0.1:8000
mkdocs build --strict      # what CI runs: no warnings allowed
```

## Project layout

- `crates/completype`: the engine and, behind the `store` feature, databases, replicas, the ingestor and
  collections. Segment format in `src/segment*`, ranking in `src/search.rs`, storage in `src/store.rs` and
  `src/database.rs`.
- `crates/completype-py`: the Python bindings (PyO3), their typed stubs in `python/completype/*.pyi`, which
  are also the source of the Python API reference, and their tests.
- `crates/completype-cli`: the `completype` command.
- `bench/`: the comparison with tantivy, Typesense and Meilisearch, and the results behind
  [Benchmarks](https://completype.pages.dev/benchmarks/).
- `docs/`, `overrides/` and `mkdocs.yml`: the documentation site. `assets/`: the brand assets and the
  scripts that generate them.

## Checklist for a pull request

- `cargo fmt --all` and `cargo clippy --all-targets --features store -- -D warnings` are clean.
- `cargo deny check` passes (licences, advisories and sources; install with `cargo install cargo-deny --locked`).
- New behaviour has tests. Ranking changes include before-and-after results on the benchmark
  (`cargo run --release -p completype --example bench`).
- Performance changes include criterion results (`cargo bench -p completype --bench search`); CI also posts a
  base-against-head comparison to the job summary of each pull request.
- Determinism holds: the same input gives bit-identical results, whatever the segmentation or thread
  count. Break ties explicitly (by id) in any new ordering.
- Changes to the segment format bump its version, and loading rejects what it cannot validate.
- Public API changes update the Python stubs (`crates/completype-py/python/completype/__init__.pyi`), the
  guides, the README and the [changelog](CHANGELOG.md) (under `## [Unreleased]`).
- Documentation changes build with `mkdocs build --strict`, and follow
  [Writing the docs](https://completype.pages.dev/writing/). Changes to the site's look, the landing page or
  the brand assets follow [DESIGN.md](DESIGN.md).

## Style

- Follow the surrounding code. Keep comments short, one line or two: say what the code does, and put the
  reasoning in the pull request.
- Prefer returning errors to panicking on bad input, especially on anything read from storage.
- Measure before optimising, and include the numbers in the pull request.
- Precision in claims: a number in the docs or the README comes from a measurement anyone can reproduce.

## Commit messages

One short line in the imperative mood ("Add hybrid fusion options"). The pull request description says why.

## Releasing

Every push to `main` opens or updates a release pull request with the version bump and the changelog entry,
made from the notes under `## [Unreleased]`. Merging it publishes `completype` and `completype-cli` to
crates.io at one version, tags `v<version>` and creates the GitHub release; the Release workflow then builds
the wheels and publishes them to PyPI.
