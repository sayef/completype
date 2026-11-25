/// The text field that [`crate::Writer::add`] and [`crate::SegmentBuilder::add`] fill.
pub const DEFAULT_FIELD: &str = "text";

/// A numeric field value or filter bound.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Number {
    Int(i64),
    Float(f64),
}

impl From<i64> for Number {
    fn from(v: i64) -> Self {
        Number::Int(v)
    }
}

impl From<i32> for Number {
    fn from(v: i32) -> Self {
        Number::Int(v.into())
    }
}

impl From<u32> for Number {
    fn from(v: u32) -> Self {
        Number::Int(v.into())
    }
}

impl From<f64> for Number {
    fn from(v: f64) -> Self {
        Number::Float(v)
    }
}

impl From<f32> for Number {
    fn from(v: f32) -> Self {
        Number::Float(v.into())
    }
}

impl Number {
    pub(crate) fn as_f64(self) -> f64 {
        match self {
            Number::Int(v) => v as f64,
            Number::Float(v) => v,
        }
    }
}

/// A document: named text fields to search, keyword and numeric fields to filter, and a payload.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Document {
    pub(crate) text: Vec<(String, String)>,
    pub(crate) keywords: Vec<(String, String)>,
    pub(crate) numbers: Vec<(String, Number)>,
    pub(crate) stored: Option<Vec<u8>>,
}

impl Document {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds searchable text; repeated calls for one field add to it.
    pub fn text(mut self, field: impl Into<String>, text: impl Into<String>) -> Self {
        self.text.push((field.into(), text.into()));
        self
    }

    /// Adds an exact value to filter on; a field may hold several.
    pub fn keyword(mut self, field: impl Into<String>, value: impl Into<String>) -> Self {
        self.keywords.push((field.into(), value.into()));
        self
    }

    /// Adds a number to filter on; NaN is ignored.
    pub fn number(mut self, field: impl Into<String>, value: impl Into<Number>) -> Self {
        self.numbers.push((field.into(), value.into()));
        self
    }

    /// Sets the payload returned by [`crate::Reader::document`], such as JSON.
    pub fn stored(mut self, payload: impl Into<Vec<u8>>) -> Self {
        self.stored = Some(payload.into());
        self
    }
}
