# completype-fts

`completype-fts` is an uncommitted standalone Rust proof of concept for exact BM25 top-k search over pre-tokenised text.

It uses 32-bit document IDs and frequencies, bitmap-assisted Block-Max MaxScore over 128-document windows, and one term ordering per query. It accumulates exact `f64` BM25 scores in query order and breaks ties by document insertion order.

[Final Linux benchmark](benchmark-linux.md) · [Implementation and benchmark handoff](HANDOFF.md)

## Build and test

```sh
cargo test --manifest-path research/completype-fts/rust/Cargo.toml
cargo build --release --manifest-path research/completype-fts/rust/Cargo.toml
```

On x86 hosts with POPCNT, build the benchmark binaries with `RUSTFLAGS="-C target-feature=+popcnt"` to reproduce the benchmark configuration.

## Library usage

```rust
use completype_fts::Builder;

let mut builder = Builder::default();
builder.add("1".into(), ["rust", "search", "library"]);
builder.add("2".into(), ["search", "index"]);
let index = builder.finish();
let hits = index.search([("rust", 1.0), ("search", 1.0)], 10);
assert_eq!(hits[0].id, "1");
```

The CLI reads document IDs and space-delimited terms separated by a tab. A blank line ends the corpus. Each following line is a query of space-delimited `term:weight` pairs. The CLI returns up to 100 tab-delimited IDs per query.

```sh
printf '1\trust search library\n2\tsearch index\n\nrust:1 search:1\n' |
  research/completype-fts/rust/target/release/completype-fts
```

The Rust scorer uses an in-memory immutable index. The product benchmark adds whole-word correction through the released completype vocabulary library before it sends weighted terms to the scorer.
