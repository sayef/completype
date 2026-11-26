use std::fs::File;
use std::io;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use memmap2::Mmap;

use crate::document::Number;
use crate::format::*;
use crate::positions::{self, Located};
use crate::stored;

enum Storage {
    Owned(Vec<u8>),
    Mapped(Mmap),
}

impl AsRef<[u8]> for Storage {
    fn as_ref(&self) -> &[u8] {
        match self {
            Storage::Owned(bytes) => bytes,
            Storage::Mapped(map) => map,
        }
    }
}

#[derive(Clone)]
pub(crate) struct Slice {
    storage: Arc<Storage>,
    range: Range<usize>,
}

impl AsRef<[u8]> for Slice {
    fn as_ref(&self) -> &[u8] {
        &self.storage.as_ref().as_ref()[self.range.clone()]
    }
}

/// An immutable, searchable set of documents, read from memory or a memory-mapped file.
pub struct Segment {
    storage: Arc<Storage>,
    sections: [Range<usize>; SECTIONS],
    docs: u32,
    total: u64,
    dictionary: fst::Map<Slice>,
    lookup: fst::Map<Slice>,
    text: Vec<TextField>,
    keyword_fields: Vec<String>,
    keywords: fst::Map<Slice>,
    numeric: Vec<NumericField>,
    position_terms: Option<fst::Map<Slice>>,
}

/// A text field's name and statistics in one segment.
#[derive(Clone, Debug)]
pub(crate) struct TextField {
    pub name: String,
    /// Documents with at least one token in the field.
    pub docs: u32,
    pub tokens: u64,
}

/// A numeric field's sorted values and their documents.
#[derive(Clone, Debug)]
pub(crate) struct NumericField {
    pub name: String,
    pub float: bool,
    offset: usize,
    pub count: usize,
}

/// Document frequency of a term summed over its fields.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TermSummary {
    pub df: u32,
}

/// Where a term's postings are and how they are bounded.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TermInfo {
    pub inline: Option<(u32, u32)>,
    pub headers: usize,
    pub bodies: usize,
    pub df: u32,
    pub windows: u32,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}

impl Segment {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        // SAFETY: segment files are written once and never modified in place.
        let map = unsafe { Mmap::map(&File::open(path)?)? };
        Self::new(Storage::Mapped(map))
    }

    pub fn from_bytes(bytes: Vec<u8>) -> io::Result<Self> {
        Self::new(Storage::Owned(bytes))
    }

    fn new(storage: Storage) -> io::Result<Self> {
        let bytes = storage.as_ref();
        let count = match bytes.get(..8) {
            Some(m) if m == MAGIC => SECTIONS,
            Some(m) if m == MAGIC_V4 => SECTIONS_V4,
            _ => return Err(invalid("not a completype-fts segment")),
        };
        if bytes.len() < header_len(count) {
            return Err(invalid("segment header is short"));
        }
        let docs = u32_at(bytes, 8);
        let total = u64_at(bytes, 16);
        let starts: Vec<usize> = (0..count)
            .map(|i| u64_at(bytes, 24 + 8 * i) as usize)
            .collect();
        let mut sections: [Range<usize>; SECTIONS] =
            std::array::from_fn(|_| bytes.len()..bytes.len());
        for i in 0..count {
            let end = starts.get(i + 1).copied().unwrap_or(bytes.len());
            if starts[i] > end || end > bytes.len() {
                return Err(invalid("segment sections are out of range"));
            }
            sections[i] = starts[i]..end;
        }
        let storage = Arc::new(storage);
        let slice = Slice {
            storage: storage.clone(),
            range: sections[DICTIONARY].clone(),
        };
        let dictionary = fst::Map::new(slice).map_err(|e| invalid(&e.to_string()))?;
        let slice = Slice {
            storage: storage.clone(),
            range: sections[ID_LOOKUP].clone(),
        };
        let lookup = fst::Map::new(slice).map_err(|e| invalid(&e.to_string()))?;
        let slice = Slice {
            storage: storage.clone(),
            range: sections[KEYWORDS].clone(),
        };
        let keywords = fst::Map::new(slice).map_err(|e| invalid(&e.to_string()))?;
        let position_terms = if sections[POSITION_TERMS].is_empty() {
            None
        } else {
            let slice = Slice {
                storage: storage.clone(),
                range: sections[POSITION_TERMS].clone(),
            };
            Some(fst::Map::new(slice).map_err(|e| invalid(&e.to_string()))?)
        };
        let meta = &storage.as_ref().as_ref()[sections[FIELDS].clone()];
        let (text, keyword_fields, numeric) =
            read_fields(meta).ok_or_else(|| invalid("segment fields are malformed"))?;
        if text.len() * docs as usize > sections[NORMS].len() {
            return Err(invalid("segment norms are short"));
        }
        Ok(Self {
            storage,
            sections,
            docs,
            total,
            dictionary,
            lookup,
            text,
            keyword_fields,
            keywords,
            numeric,
            position_terms,
        })
    }

    /// Whether the segment keeps token positions, which phrase queries need.
    pub fn has_positions(&self) -> bool {
        self.position_terms.is_some()
    }

    /// Where a term's positions in a text field are, if the segment keeps them.
    pub(crate) fn term_positions(&self, field: u8, term: &str) -> Option<Located> {
        let start = self.position_terms.as_ref()?.get(term)?;
        let value = self.dictionary.get(term)?;
        let rank = self.fields_of(value).position(|(f, _)| f == field)?;
        Some(positions::locate(
            self.section(POSITIONS),
            start as usize,
            rank,
        ))
    }

    pub(crate) fn positions_section(&self) -> &[u8] {
        self.section(POSITIONS)
    }

    /// Every position of a term-field, `positions::kept(tf)` per posting in document order.
    pub(crate) fn all_positions(&self, info: &TermInfo, located: Located) -> Vec<u32> {
        let section = self.section(POSITIONS);
        let bodies = self.bodies(info);
        let (mut out, mut doc) = (Vec::new(), Vec::new());
        for (window, block) in self.headers(info).zip(positions::blocks(section, located)) {
            let (mask, tfs) = window.decode(bodies);
            let mut first = 0;
            for rank in 0..mask.count_ones() as usize {
                let count = positions::kept(tfs.get(rank));
                positions::read(section, block, first, count, &mut doc);
                out.extend_from_slice(&doc);
                first += count;
            }
        }
        out
    }

    pub(crate) fn section(&self, i: usize) -> &[u8] {
        &self.storage.as_ref().as_ref()[self.sections[i].clone()]
    }

    pub fn len(&self) -> usize {
        self.docs as usize
    }

    pub fn is_empty(&self) -> bool {
        self.docs == 0
    }

    pub fn total_tokens(&self) -> u64 {
        self.total
    }

    pub fn windows(&self) -> usize {
        (self.docs as usize).div_ceil(WINDOW)
    }

    pub fn id(&self, doc: u32) -> &str {
        let offsets = self.section(ID_OFFSETS);
        let at = doc as usize * 8;
        let (start, end) = (
            u64_at(offsets, at) as usize,
            u64_at(offsets, at + 8) as usize,
        );
        std::str::from_utf8(&self.section(ID_BYTES)[start..end]).expect("ids are UTF-8")
    }

    /// The document with this id; the last one when a segment holds it more than once.
    pub fn doc(&self, id: &str) -> Option<u32> {
        self.lookup.get(id).map(|d| d as u32)
    }

    pub(crate) fn dictionary(&self) -> &fst::raw::Fst<Slice> {
        self.dictionary.as_fst()
    }

    /// Every term with its postings info, in byte order.
    pub(crate) fn terms(&self) -> fst::map::Stream<'_> {
        self.dictionary.stream()
    }

    pub(crate) fn info(&self, value: u64) -> TermSummary {
        let df = self
            .fields_of(value)
            .fold(0_u32, |df, (_, info)| df.saturating_add(info.df));
        TermSummary { df }
    }

    /// The fields a dictionary value's term occurs in, with its postings in each.
    pub(crate) fn fields_of(&self, value: u64) -> impl Iterator<Item = (u8, TermInfo)> + '_ {
        let inline = (value & INLINE != 0).then(|| {
            let posting = (value as u32, (value >> 32) as u32 & MAX_TF);
            let info = TermInfo {
                inline: Some(posting),
                headers: 0,
                bodies: 0,
                df: 1,
                windows: 1,
            };
            ((value >> INLINE_FIELD_SHIFT) as u8, info)
        });
        let table = self.section(TERMS);
        let mut at = value as usize;
        let count = if inline.is_some() {
            0
        } else {
            read_varint(table, &mut at)
        };
        inline.into_iter().chain((0..count).map(move |_| {
            let field = read_varint(table, &mut at) as u8;
            let headers = read_varint64(table, &mut at) as usize;
            let bodies = read_varint64(table, &mut at) as usize;
            let df = read_varint(table, &mut at);
            let windows = read_varint(table, &mut at);
            let info = TermInfo {
                inline: None,
                headers,
                bodies,
                df,
                windows,
            };
            (field, info)
        }))
    }

    /// A term's postings as `(doc, tf)`, in document order.
    pub(crate) fn postings<'a>(&'a self, info: &TermInfo) -> impl Iterator<Item = (u32, u32)> + 'a {
        let bodies = self.bodies(info);
        self.headers(info).flat_map(move |window| {
            let (mask, tfs) = window.decode(bodies);
            let base = window.id * WINDOW as u32;
            let mut m = mask;
            (0..mask.count_ones() as usize).map(move |rank| {
                let off = m.trailing_zeros();
                m &= m - 1;
                (base + off, tfs.get(rank))
            })
        })
    }

    /// One-byte lengths of a text field's documents.
    #[inline]
    pub(crate) fn norms(&self, field: u8) -> &[u8] {
        let (docs, f) = (self.docs as usize, usize::from(field));
        &self.section(NORMS)[f * docs..(f + 1) * docs]
    }

    pub(crate) fn term(&self, field: u8, term: &str) -> Option<TermInfo> {
        let value = self.dictionary.get(term)?;
        self.fields_of(value)
            .find(|&(f, _)| f == field)
            .map(|(_, info)| info)
    }

    /// Documents containing `term` in any field, counted once per field.
    pub fn document_frequency(&self, term: &str) -> u32 {
        self.dictionary.get(term).map_or(0, |v| self.info(v).df)
    }

    pub(crate) fn field_document_frequency(&self, field: u8, term: &str) -> u32 {
        self.term(field, term).map_or(0, |t| t.df)
    }

    pub(crate) fn text_fields(&self) -> &[TextField] {
        &self.text
    }

    pub(crate) fn text_field(&self, name: &str) -> Option<u8> {
        self.text
            .iter()
            .position(|f| f.name == name)
            .map(|f| f as u8)
    }

    pub(crate) fn keyword_fields(&self) -> &[String] {
        &self.keyword_fields
    }

    /// Documents whose keyword field `field` holds `value`, ascending.
    pub(crate) fn keyword_docs(&self, field: u8, value: &str) -> impl Iterator<Item = u32> + '_ {
        let mut key = Vec::with_capacity(value.len() + 1);
        key.push(field);
        key.extend_from_slice(value.as_bytes());
        let offset = self.keywords.get(key);
        self.keyword_list(offset)
    }

    pub(crate) fn keyword_list(&self, offset: Option<u64>) -> impl Iterator<Item = u32> + '_ {
        let lists = self.section(KEYWORD_DOCS);
        let mut at = offset.unwrap_or(0) as usize;
        let count = offset.map_or(0, |_| read_varint(lists, &mut at));
        let mut doc = 0;
        (0..count).map(move |_| {
            doc += read_varint(lists, &mut at);
            doc
        })
    }

    /// Every keyword entry: the field's index byte then the value, and its list offset.
    pub(crate) fn keyword_entries(&self) -> fst::map::Stream<'_> {
        self.keywords.stream()
    }

    pub(crate) fn numeric_fields(&self) -> &[NumericField] {
        &self.numeric
    }

    pub(crate) fn numeric_field(&self, name: &str) -> Option<&NumericField> {
        self.numeric.iter().find(|f| f.name == name)
    }

    /// The `i`-th smallest value of a numeric field and its document.
    #[inline]
    pub(crate) fn numeric(&self, field: &NumericField, i: usize) -> (Number, u32) {
        let bytes = self.section(NUMBERS);
        let bits = u64_at(bytes, field.offset + 8 * i);
        let doc = u32_at(bytes, field.offset + 8 * field.count + 4 * i);
        let value = if field.float {
            Number::Float(f64::from_bits(bits))
        } else {
            Number::Int(bits as i64)
        };
        (value, doc)
    }

    /// The document's stored payload, if it has one.
    pub fn stored(&self, doc: u32) -> Option<Vec<u8>> {
        let (block, first) = self.stored_block(doc)?;
        stored::extract(&self.stored_bytes(block), (doc - first) as usize)
    }

    /// The stored block holding `doc` and that block's first document.
    pub(crate) fn stored_block(&self, doc: u32) -> Option<(usize, u32)> {
        stored::block_of(self.section(STORED_INDEX), doc)
    }

    pub(crate) fn stored_bytes(&self, block: usize) -> Vec<u8> {
        stored::decompress(self.section(STORED_INDEX), self.section(STORED), block)
    }

    pub(crate) fn headers(&self, info: &TermInfo) -> Headers<'_> {
        match info.inline {
            Some((doc, tf)) => Headers::single(doc, tf),
            None => Headers::new(&self.section(HEADERS)[info.headers..], info.windows),
        }
    }

    /// A term's header bytes, empty for a single posting kept in the dictionary.
    pub(crate) fn header_bytes(&self, info: &TermInfo) -> &[u8] {
        match info.inline {
            Some(_) => &[],
            None => &self.section(HEADERS)[info.headers..],
        }
    }

    pub(crate) fn bodies(&self, info: &TermInfo) -> &[u8] {
        &self.section(BODIES)[info.bodies..]
    }

    /// Bytes per section, for size reports.
    pub fn section_sizes(&self) -> [(&'static str, usize); SECTIONS] {
        let names = [
            "norms",
            "id offsets",
            "ids",
            "terms",
            "headers",
            "bodies",
            "dictionary",
            "id lookup",
            "fields",
            "keywords",
            "keyword documents",
            "numbers",
            "stored index",
            "stored",
            "position terms",
            "positions",
        ];
        std::array::from_fn(|i| (names[i], self.sections[i].len()))
    }
}

type Fields = (Vec<TextField>, Vec<String>, Vec<NumericField>);

fn read_fields(meta: &[u8]) -> Option<Fields> {
    let mut at = 0;
    let count = |at: &mut usize| (*at < meta.len()).then(|| read_varint(meta, at));
    let n = count(&mut at)?;
    let mut text = Vec::new();
    for _ in 0..n {
        let name = read_str(meta, &mut at)?;
        let docs = read_varint(meta, &mut at);
        let tokens = read_varint64(meta, &mut at);
        text.push(TextField { name, docs, tokens });
    }
    let n = count(&mut at)?;
    let keywords = (0..n)
        .map(|_| read_str(meta, &mut at))
        .collect::<Option<_>>()?;
    let n = count(&mut at)?;
    let mut numeric = Vec::new();
    for _ in 0..n {
        let name = read_str(meta, &mut at)?;
        let float = *meta.get(at)? != 0;
        at += 1;
        let offset = read_varint64(meta, &mut at) as usize;
        let count = read_varint(meta, &mut at) as usize;
        numeric.push(NumericField {
            name,
            float,
            offset,
            count,
        });
    }
    Some((text, keywords, numeric))
}
