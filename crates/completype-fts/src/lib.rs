//! Full-text search with exact BM25 top-k over compressed, memory-mapped segments.

mod analyzer;
mod build;
mod format;
mod fuzzy;
mod index;
mod norm;
mod reorder;
mod search;
mod segment;

pub use analyzer::Analyzer;
pub use build::SegmentBuilder;
pub use index::{Index, Reader, SearchHit, Writer};
pub use search::{Hit, Searcher, Stats};
pub use segment::Segment;
