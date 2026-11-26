use std::ops::{Bound, RangeBounds};

use crate::document::Number;
use crate::segment::{NumericField, Segment};

/// Which documents a query may return, evaluated to a bitset per segment before scoring.
#[derive(Clone, Debug, PartialEq)]
pub enum Filter {
    /// The keyword field holds any of the values.
    Keyword {
        field: String,
        values: Vec<String>,
    },
    /// The numeric field holds a value in the range.
    Range {
        field: String,
        lower: Bound<Number>,
        upper: Bound<Number>,
    },
    And(Vec<Filter>),
    Or(Vec<Filter>),
    Not(Box<Filter>),
}

impl Filter {
    pub fn keyword(field: impl Into<String>, value: impl Into<String>) -> Self {
        Filter::Keyword {
            field: field.into(),
            values: vec![value.into()],
        }
    }

    pub fn keyword_in<V: Into<String>>(
        field: impl Into<String>,
        values: impl IntoIterator<Item = V>,
    ) -> Self {
        Filter::Keyword {
            field: field.into(),
            values: values.into_iter().map(Into::into).collect(),
        }
    }

    /// Numbers in `range`, such as `10..=20`, `..5.0` or `100..`.
    pub fn range<N: Into<Number> + Clone>(
        field: impl Into<String>,
        range: impl RangeBounds<N>,
    ) -> Self {
        let bound = |b: Bound<&N>| b.cloned().map(Into::into);
        Filter::Range {
            field: field.into(),
            lower: bound(range.start_bound()),
            upper: bound(range.end_bound()),
        }
    }

    pub fn equals(field: impl Into<String>, value: impl Into<Number>) -> Self {
        let value = value.into();
        Filter::Range {
            field: field.into(),
            lower: Bound::Included(value),
            upper: Bound::Included(value),
        }
    }

    pub fn and(self, other: Filter) -> Self {
        match self {
            Filter::And(mut all) => {
                all.push(other);
                Filter::And(all)
            }
            first => Filter::And(vec![first, other]),
        }
    }

    pub fn or(self, other: Filter) -> Self {
        match self {
            Filter::Or(mut any) => {
                any.push(other);
                Filter::Or(any)
            }
            first => Filter::Or(vec![first, other]),
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn not(self) -> Self {
        Filter::Not(Box::new(self))
    }

    /// Allowed documents of `segment`, one bit each; bits past the last document stay clear.
    pub(crate) fn allowed(&self, segment: &Segment) -> Vec<u64> {
        let docs = segment.len();
        let mut bits = vec![0_u64; docs.div_ceil(64)];
        let mut set = |d: u32| bits[d as usize / 64] |= 1 << (d % 64);
        match self {
            Filter::Keyword { field, values } => {
                if let Some(f) = segment.keyword_fields().iter().position(|n| n == field) {
                    for value in values {
                        segment.keyword_docs(f as u8, value).for_each(&mut set);
                    }
                }
            }
            Filter::Range {
                field,
                lower,
                upper,
            } => {
                if let Some(field) = segment.numeric_field(field) {
                    let (start, end) = value_range(segment, field, *lower, *upper);
                    (start..end).for_each(|i| set(segment.numeric(field, i).1));
                }
            }
            Filter::And(all) => {
                bits = full(docs);
                for filter in all {
                    if bits.iter().all(|&w| w == 0) {
                        break;
                    }
                    let other = filter.allowed(segment);
                    bits.iter_mut().zip(other).for_each(|(w, o)| *w &= o);
                }
            }
            Filter::Or(any) => {
                for filter in any {
                    let other = filter.allowed(segment);
                    bits.iter_mut().zip(other).for_each(|(w, o)| *w |= o);
                }
            }
            Filter::Not(filter) => {
                bits = filter.allowed(segment);
                bits.iter_mut()
                    .zip(full(docs))
                    .for_each(|(w, all)| *w = !*w & all);
            }
        }
        bits
    }
}

fn full(docs: usize) -> Vec<u64> {
    let mut bits = vec![u64::MAX; docs.div_ceil(64)];
    if !docs.is_multiple_of(64) {
        *bits.last_mut().unwrap() = (1 << (docs % 64)) - 1;
    }
    bits
}

/// Positions of the field's sorted values that fall within the bounds.
fn value_range(
    segment: &Segment,
    field: &NumericField,
    lower: Bound<Number>,
    upper: Bound<Number>,
) -> (usize, usize) {
    let first_where = |pred: &dyn Fn(Number) -> bool| {
        let (mut lo, mut hi) = (0, field.count);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if pred(segment.numeric(field, mid).0) {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        lo
    };
    if field.float {
        let as_float = |b: Bound<Number>| b.map(Number::as_f64);
        let (lower, upper) = (as_float(lower), as_float(upper));
        let value = |n: Number| n.as_f64();
        if [lower, upper]
            .iter()
            .any(|b| matches!(b, Bound::Included(x) | Bound::Excluded(x) if x.is_nan()))
        {
            return (0, 0);
        }
        let start = match lower {
            Bound::Unbounded => 0,
            Bound::Included(b) => first_where(&|n| value(n) >= b),
            Bound::Excluded(b) => first_where(&|n| value(n) > b),
        };
        let end = match upper {
            Bound::Unbounded => field.count,
            Bound::Included(b) => first_where(&|n| value(n) > b),
            Bound::Excluded(b) => first_where(&|n| value(n) >= b),
        };
        return (start, end.max(start));
    }
    let Some((low, high)) = integer_bounds(lower, upper) else {
        return (0, 0);
    };
    let value = |n: Number| match n {
        Number::Int(v) => v,
        Number::Float(_) => unreachable!("integer columns hold integers"),
    };
    let start = first_where(&|n| value(n) >= low);
    let end = first_where(&|n| value(n) > high);
    (start, end.max(start))
}

/// Inclusive integer bounds equal to `lower..upper`, or `None` when nothing fits.
fn integer_bounds(lower: Bound<Number>, upper: Bound<Number>) -> Option<(i64, i64)> {
    const LIMIT: f64 = 9_223_372_036_854_775_808.0;
    let low = match lower {
        Bound::Unbounded => i64::MIN,
        Bound::Included(Number::Int(v)) => v,
        Bound::Excluded(Number::Int(v)) => v.checked_add(1)?,
        Bound::Included(Number::Float(v)) | Bound::Excluded(Number::Float(v)) => {
            let v = if matches!(lower, Bound::Included(_)) {
                v.ceil()
            } else {
                v.floor() + 1.0
            };
            if v.is_nan() || v >= LIMIT {
                return None;
            }
            v.max(-LIMIT) as i64
        }
    };
    let high = match upper {
        Bound::Unbounded => i64::MAX,
        Bound::Included(Number::Int(v)) => v,
        Bound::Excluded(Number::Int(v)) => v.checked_sub(1)?,
        Bound::Included(Number::Float(v)) | Bound::Excluded(Number::Float(v)) => {
            let v = if matches!(upper, Bound::Included(_)) {
                v.floor()
            } else {
                v.ceil() - 1.0
            };
            if v.is_nan() || v < -LIMIT {
                return None;
            }
            v.min(LIMIT) as i64
        }
    };
    (low <= high).then_some((low, high))
}

/// The text outside double quotes, and each quoted phrase with the slop of a following `~N`.
pub(crate) fn split_phrases(text: &str) -> (String, Vec<(String, u32)>) {
    let (mut plain, mut phrases) = (String::new(), Vec::new());
    let mut rest = text;
    while let Some(open) = rest.find('"') {
        let Some(close) = rest[open + 1..].find('"') else {
            break;
        };
        plain.push_str(&rest[..open]);
        plain.push(' ');
        let phrase = &rest[open + 1..open + 1 + close];
        rest = &rest[open + close + 2..];
        let mut slop = 0;
        if let Some(after) = rest.strip_prefix('~') {
            let digits = after.len() - after.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            slop = after[..digits].parse().unwrap_or(u32::MAX);
            rest = &after[digits..];
        }
        phrases.push((phrase.to_owned(), slop));
    }
    plain.push_str(rest);
    (plain, phrases)
}

/// A search over named text fields with boosts, an optional filter and a result limit.
/// Quoted text is a phrase, `"new york"~2` one with slop, which needs an index with positions.
#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    pub(crate) text: String,
    pub(crate) phrases: Vec<(String, u32)>,
    pub(crate) fields: Vec<(String, f32)>,
    pub(crate) filter: Option<Filter>,
    pub(crate) limit: usize,
    pub(crate) typos: bool,
}

impl Query {
    /// Searches every text field with boost 1 unless [`Query::field`] names some.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            phrases: Vec::new(),
            fields: Vec::new(),
            filter: None,
            limit: 10,
            typos: true,
        }
    }

    /// Also scores documents holding `text`'s words next to each other, in order.
    pub fn phrase(self, text: impl Into<String>) -> Self {
        self.phrase_slop(text, 0)
    }

    /// A phrase whose words may be up to `slop` moves apart, Lucene's slop; each match counts
    /// `1 / (1 + distance)`.
    pub fn phrase_slop(mut self, text: impl Into<String>, slop: u32) -> Self {
        self.phrases.push((text.into(), slop));
        self
    }

    /// Searches `field`, its BM25 scores multiplied by `boost`.
    pub fn field(mut self, field: impl Into<String>, boost: f32) -> Self {
        self.fields.push((field.into(), boost));
        self
    }

    pub fn filter(mut self, filter: Filter) -> Self {
        self.filter = Some(filter);
        self
    }

    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    /// Skips spelling corrections, as [`crate::Reader::search_exact`] does.
    pub fn exact(mut self) -> Self {
        self.typos = false;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::split_phrases;

    #[test]
    fn quoted_phrases_and_slop_are_split_from_the_text() {
        let (text, phrases) = split_phrases(r#""new york"~2 pizza "big apple" "open"#);
        assert_eq!(
            text.split_whitespace().collect::<Vec<_>>(),
            ["pizza", "\"open"]
        );
        assert_eq!(
            phrases,
            [("new york".to_owned(), 2), ("big apple".to_owned(), 0)]
        );
        assert_eq!(split_phrases("plain").1, []);
    }
}
