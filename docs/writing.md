# Writing the docs

How the README, this site, the changelog and code comments are written. The aim is a reader who finds the
answer, trusts it, and can run it. For colours, type and components, see [Design and brand](design.md).

## Voice

- **Describe, do not sell.** Say what completype does and show it. "Answers in 0.15 ms at the median on
  Hacker News titles" beats "blazing fast".
- **Plain and direct.** Short sentences in the active voice, second person for instructions ("Pass
  `cache_dir` to keep segments on disk"), present tense for behaviour.
- **British spelling**, as the rest of the project: normalised, quantised, catalogue, licence (the noun).
- **Honest about limits.** Say where other engines lead, what completype does not do, and what is young.
  The README's "What it is not" and the benchmarks' caveats are part of the product.
- No hype words: powerful, robust, seamless, blazing, cutting-edge, effortless, next-generation, unlock,
  supercharge. No exclamation marks and no emoji.

## Names

| Write | Not |
|---|---|
| completype, lower case, even at the start of a sentence | Completype, CompleType, completype's engine |
| an index, a segment, a database, an engine, a layer, a namespace | a collection (unless you mean the collections layer), a table |
| a document, its text, its popularity | a record, a row, a title (except for corpora of titles) |
| match kinds by their names: `exact`, `prefix`, `infix`, `abbreviation`, `synonym`, `fuzzy`, `semantic` | "fuzzy search mode" |
| serverless: no completype server; storage is the only shared component | "server-less", "lambda-only" |
| S3, Google Cloud Storage (GCS), Azure Blob Storage | AWS S3 bucket store |

Code identifiers appear in code formatting, exactly as they are spelled in the language of the example:
`engine.complete(...)` in Python, `Engine::complete` in Rust.

## Numbers

- Every number comes from a measurement in the repository: [Benchmarks](benchmarks.md), the bench results
  in `bench/results/`, or a runnable example. Say what was measured, on what, and how to reproduce it.
- Never round a result into a better one. Ranges stay ranges ("1.5 to 3.0 ms at the median").
- Units with a space: `0.15 ms`, `3.1 GB`, `256 MB`. Thousands with commas: `124,440`. Percentages with the
  sign: `28.5%`.
- When the README, the landing page and a guide quote the same number, change all of them together.

## Pages

- **One page, one question.** A guide answers "how do I…"; [Concepts](concepts.md) and
  [Architecture](architecture.md) answer "how does it work"; the reference lists everything.
- **Start with a sentence that says what the page covers**, then the shortest example that works.
- **Python first, Rust beside it.** Use content tabs (`=== "Python"`, `=== "Rust"`) when both languages
  show the same thing.
- **Show output.** Put real output in a `text` block under the code that produced it; comments such as
  `# ['Dancing Queen – ABBA', ...]` work for one-liners.
- **Use the music catalogue** of the examples (Queen, ABBA, Bruce Springsteen and the rest) so results
  stay comparable from page to page.
- **Headings in sentence case**, and short: they become the table of contents.
- **Tables for options and comparisons**, lists for steps and properties, prose for reasoning.
- **Admonitions sparingly.** `warning` for what loses data or memory, `note` for an exception. Never two
  in a row.

## Examples

- Examples are complete enough to run once the names they assume exist (`docs`, `db`, `engine`). A block
  that is a signature or a fragment gets `<!-- skip-test -->` on the line before it, as in the guides.
- Use `memory://` or `./data` for databases in examples, and `s3://my-bucket/...` for buckets. Never a real
  bucket, account or credential.
- Keep lines under about 110 characters, so code does not scroll on a laptop.

## Links

- Link the first mention of a guide, concept or option on a page, not every mention.
- Inside the site, link to the Markdown file (`guides/layers.md#namespaces`), so the strict build checks it.
- **The README is also the crates.io and PyPI page.** Every link and image in it is an absolute URL: the
  site (`https://completype.pages.dev/...`), GitHub (`https://github.com/sayef/completype/blob/main/...`) or
  raw files for images.
- Files from the repository root (`CHANGELOG.md`, `CONTRIBUTING.md`, `DESIGN.md`) are included into the
  site with snippets; `docs/hooks.py` points their links at the right pages.

## Changelog

Write user-facing changes under `## [Unreleased]` in the [changelog](changelog.md), in the [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) groups (Added, Changed, Deprecated, Removed,
Fixed, Security). One line per change, in the past tense, naming the API it touches. Release automation
turns `[Unreleased]` into the version's entry.

## Comments and docstrings

- A comment says what the code does, in one line, two at most. The reasoning goes in the pull request.
- Docstrings in the Python stubs are the API reference: one summary line, then `Args`, `Returns` and
  `Raises` in Google style, as `mkdocstrings` renders them.
- If a comment needs a paragraph to make sense, change the code instead.

## Before you push

```sh
pip install -r docs/requirements.txt
mkdocs build --strict      # no warnings: broken links and anchors fail the build
mkdocs serve               # preview at http://127.0.0.1:8000
```
