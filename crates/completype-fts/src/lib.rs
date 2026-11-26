//! Full-text search with exact BM25 top-k over compressed, memory-mapped segments.

mod analyzer;
mod build;
mod document;
mod format;
mod fuzzy;
mod index;
mod language;
mod norm;
mod phrase;
mod positions;
mod query;
mod reorder;
mod search;
mod segment;
mod stored;

pub use analyzer::Analyzer;
pub use build::SegmentBuilder;
pub use document::{Document, Number, DEFAULT_FIELD};
pub use index::{Index, Reader, Reordering, SearchHit, Writer};
pub use language::{Language, StopWords};
pub use query::{Filter, Query};
pub use search::{Hit, Searcher, Stats};
pub use segment::Segment;
