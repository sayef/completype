---
title: completype
template: home.html
hide:
  - navigation
  - toc
---

# completype

Serverless autocompletion for Rust and Python. completype is an embedded engine whose database is a bucket:
every process reads the index straight from storage and ranks exact, prefix, infix, abbreviation,
spelling-tolerant, word-decomposing and semantic matches together, in a fraction of a millisecond.
