use std::io;
use std::path::Path;

use rustc_hash::FxHashMap;

use crate::document::{Document, Number, DEFAULT_FIELD};
use crate::format::*;
use crate::{norm, positions, reorder, stored, Analyzer};

/// A term's postings in one field, and their positions when the segment keeps them.
#[derive(Clone, Debug, Default)]
pub(crate) struct List {
    pub docs: Vec<(u32, u32)>,
    /// `positions::kept(tf)` ascending positions per posting, in document order.
    pub positions: Vec<u32>,
}

type Postings = FxHashMap<Box<str>, List>;
/// A term, its field and its postings there.
type FieldTerm<'a> = (&'a str, u8, &'a List);

#[derive(Default)]
struct TextField {
    name: String,
    /// Tokens per document; documents past the end have none.
    lengths: Vec<u32>,
    postings: Postings,
}

#[derive(Default)]
struct KeywordField {
    name: String,
    docs: FxHashMap<Box<str>, Vec<u32>>,
}

#[derive(Default)]
struct NumericField {
    name: String,
    values: Vec<(Number, u32)>,
}

/// Collects documents and writes them as one immutable segment.
#[derive(Default)]
pub struct SegmentBuilder {
    ids: Vec<String>,
    text: Vec<TextField>,
    keywords: Vec<KeywordField>,
    numbers: Vec<NumericField>,
    stored: Vec<Option<Box<[u8]>>>,
    /// The current document's terms with their frequency, or with positions their slot in `tfs`.
    counts: FxHashMap<String, u32>,
    tfs: Vec<u32>,
    /// `(slot, position)` of each token of the current document, when keeping positions.
    pending: Vec<(u32, u32)>,
    sorted: Vec<u32>,
    reorder: bool,
    positions: bool,
    dropped: Vec<u32>,
}

fn field_index<T>(
    fields: &mut Vec<T>,
    name: &str,
    of: impl Fn(&T) -> &str,
    new: impl Fn() -> T,
) -> usize {
    if let Some(i) = fields.iter().position(|f| of(f) == name) {
        return i;
    }
    assert!(
        fields.len() < 255,
        "a segment holds at most 255 fields of a kind"
    );
    fields.push(new());
    fields.len() - 1
}

/// Moves `items[old]` to `new_of[old]`, leaving out those mapped to `u32::MAX`.
fn permute<T: Default>(items: Vec<T>, new_of: &[u32], count: usize) -> Vec<T> {
    let mut out: Vec<T> = (0..count).map(|_| T::default()).collect();
    for (old, item) in items.into_iter().enumerate() {
        let new = new_of[old];
        if new != u32::MAX {
            out[new as usize] = item;
        }
    }
    out
}

impl SegmentBuilder {
    /// Adds a document whose default field holds `tokens`.
    pub fn add<'a>(&mut self, id: impl Into<String>, tokens: impl IntoIterator<Item = &'a str>) {
        let doc = self.push_document(id.into());
        let field = self.text_field(DEFAULT_FIELD);
        let mut length = 0_u32;
        for token in tokens {
            self.count(token, length);
            length = length.saturating_add(1);
        }
        self.flush(field, doc, length);
    }

    /// Adds a document, analysing its text fields with `analyzer`.
    pub fn add_document(
        &mut self,
        id: impl Into<String>,
        document: &Document,
        analyzer: &Analyzer,
    ) {
        let doc = self.push_document(id.into());
        for (i, (name, _)) in document.text.iter().enumerate() {
            if document.text[..i].iter().any(|(n, _)| n == name) {
                continue;
            }
            let field = self.text_field(name);
            let (mut length, mut position) = (0_u32, 0_u32);
            for (v, (_, text)) in document.text[i..]
                .iter()
                .filter(|(n, _)| n == name)
                .enumerate()
            {
                if v > 0 {
                    position = position.saturating_add(POSITION_GAP);
                }
                analyzer.analyze(text, |token| {
                    self.count(token, position);
                    length = length.saturating_add(1);
                    position = position.saturating_add(1);
                });
            }
            self.flush(field, doc, length);
        }
        for (name, value) in &document.keywords {
            self.push_keyword(name, value, doc);
        }
        for &(ref name, value) in &document.numbers {
            self.push_number(name, value, doc);
        }
        if let Some(payload) = &document.stored {
            self.set_stored(doc, payload.as_slice().into());
        }
    }

    #[inline]
    fn count(&mut self, token: &str, position: u32) {
        if !self.positions {
            match self.counts.get_mut(token) {
                Some(tf) => *tf += 1,
                None => {
                    self.counts.insert(token.to_owned(), 1);
                }
            }
            return;
        }
        let slot = match self.counts.get(token) {
            Some(&slot) => slot,
            None => {
                let slot = self.tfs.len() as u32;
                self.counts.insert(token.to_owned(), slot);
                self.tfs.push(0);
                slot
            }
        };
        self.tfs[slot as usize] += 1;
        self.pending.push((slot, position));
    }

    fn flush(&mut self, field: usize, doc: u32, length: u32) {
        let field = &mut self.text[field];
        if length > 0 {
            field.lengths.resize(doc as usize, 0);
            field.lengths.push(length);
        }
        // Counting sort of the document's positions by slot; each slot's stay ascending.
        let mut starts = Vec::new();
        if self.positions {
            starts.reserve(self.tfs.len() + 1);
            starts.push(0);
            for &tf in &self.tfs {
                starts.push(starts.last().unwrap() + tf);
            }
            self.sorted.clear();
            self.sorted.resize(self.pending.len(), 0);
            let mut next = starts.clone();
            for &(slot, position) in &self.pending {
                self.sorted[next[slot as usize] as usize] = position;
                next[slot as usize] += 1;
            }
            self.pending.clear();
        }
        for (term, value) in self.counts.drain() {
            let (tf, slot) = if self.positions {
                (self.tfs[value as usize], value)
            } else {
                (value, 0)
            };
            let list = match field.postings.get_mut(term.as_str()) {
                Some(list) => list,
                None => field.postings.entry(term.into_boxed_str()).or_default(),
            };
            list.docs.push((doc, tf));
            if self.positions {
                let start = starts[slot as usize] as usize;
                list.positions
                    .extend_from_slice(&self.sorted[start..start + positions::kept(tf)]);
            }
        }
        self.tfs.clear();
    }

    /// Keeps each token's position so phrases can be searched, at the cost of a larger segment.
    pub fn positions(&mut self, on: bool) -> &mut Self {
        self.positions = on;
        self
    }

    pub(crate) fn keeps_positions(&self) -> bool {
        self.positions
    }

    /// Reorders documents so similar ones share windows: smaller and faster, slower to build.
    pub fn reorder(&mut self, on: bool) -> &mut Self {
        self.reorder = on;
        self
    }

    /// Leaves out a document added earlier, by its position in adding order.
    pub fn drop_document(&mut self, doc: u32) {
        self.dropped.push(doc);
    }

    /// Documents added and not dropped.
    pub fn live(&self) -> usize {
        self.ids.len() - self.dropped.len()
    }

    /// Appends a document with no fields yet, for merging.
    pub(crate) fn push_document(&mut self, id: String) -> u32 {
        let doc =
            u32::try_from(self.ids.len()).expect("a segment holds at most u32::MAX documents");
        self.ids.push(id);
        doc
    }

    pub(crate) fn text_field(&mut self, name: &str) -> usize {
        field_index(
            &mut self.text,
            name,
            |f| &f.name,
            || TextField {
                name: name.to_owned(),
                ..Default::default()
            },
        )
    }

    /// Sets a field's token count for every document, for merging.
    pub(crate) fn set_lengths(&mut self, field: usize, lengths: Vec<u32>) {
        self.text[field].lengths = lengths;
    }

    /// Sets a term's whole postings list in a field, in document order, for merging.
    pub(crate) fn set_postings(&mut self, field: usize, term: &str, list: List) {
        self.text[field].postings.insert(term.into(), list);
    }

    pub(crate) fn push_keyword(&mut self, field: &str, value: &str, doc: u32) {
        let f = field_index(
            &mut self.keywords,
            field,
            |f| &f.name,
            || KeywordField {
                name: field.to_owned(),
                ..Default::default()
            },
        );
        let docs = &mut self.keywords[f].docs;
        match docs.get_mut(value) {
            Some(list) if list.last() == Some(&doc) => {}
            Some(list) => list.push(doc),
            None => {
                docs.insert(value.into(), vec![doc]);
            }
        }
    }

    pub(crate) fn push_number(&mut self, field: &str, value: Number, doc: u32) {
        let value = match value {
            Number::Float(v) if v.is_nan() => return,
            Number::Float(v) => Number::Float(v + 0.0),
            v => v,
        };
        let f = field_index(
            &mut self.numbers,
            field,
            |f| &f.name,
            || NumericField {
                name: field.to_owned(),
                ..Default::default()
            },
        );
        self.numbers[f].values.push((value, doc));
    }

    pub(crate) fn set_stored(&mut self, doc: u32, payload: Box<[u8]>) {
        if self.stored.len() <= doc as usize {
            self.stored.resize(doc as usize + 1, None);
        }
        self.stored[doc as usize] = Some(payload);
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Moves every document to `new_of[doc]`, dropping those mapped to `u32::MAX`.
    fn renumber(&mut self, new_of: &[u32], count: usize) {
        let map = |d: &mut u32| {
            *d = new_of[*d as usize];
            *d != u32::MAX
        };
        self.ids = permute(std::mem::take(&mut self.ids), new_of, count);
        for field in &mut self.text {
            field.lengths = permute(std::mem::take(&mut field.lengths), new_of, count);
            field.postings.retain(|_, list| {
                if list.positions.is_empty() {
                    list.docs.retain_mut(|p| map(&mut p.0));
                    list.docs.sort_unstable_by_key(|p| p.0);
                } else {
                    renumber_with_positions(list, new_of);
                }
                !list.docs.is_empty()
            });
        }
        for field in &mut self.keywords {
            field.docs.retain(|_, list| {
                list.retain_mut(map);
                list.sort_unstable();
                !list.is_empty()
            });
        }
        for field in &mut self.numbers {
            field.values.retain_mut(|v| map(&mut v.1));
        }
        if !self.stored.is_empty() {
            self.stored = permute(std::mem::take(&mut self.stored), new_of, count);
        }
    }

    fn remove_dropped(&mut self) {
        let mut keep = vec![true; self.ids.len()];
        self.dropped
            .drain(..)
            .for_each(|d| keep[d as usize] = false);
        let mut new_of = vec![u32::MAX; keep.len()];
        let mut next = 0;
        for (doc, &k) in keep.iter().enumerate() {
            if k {
                new_of[doc] = next;
                next += 1;
            }
        }
        self.renumber(&new_of, next as usize);
    }

    pub fn write(self, path: impl AsRef<Path>) -> io::Result<()> {
        std::fs::write(path, self.finish())
    }

    /// The segment's bytes, as [`crate::Segment::from_bytes`] reads them.
    pub fn finish(mut self) -> Vec<u8> {
        if !self.dropped.is_empty() {
            self.remove_dropped();
        }
        let docs = self.ids.len();
        if self.reorder && docs > WINDOW {
            let lists = self
                .text
                .iter()
                .flat_map(|f| f.postings.values().map(|l| l.docs.as_slice()));
            let forward = reorder::Forward::new(docs, lists);
            let order = reorder::order(&forward, docs);
            drop(forward);
            let mut new_of = vec![0_u32; docs];
            for (new, &old) in order.iter().enumerate() {
                new_of[old as usize] = new as u32;
            }
            self.renumber(&new_of, docs);
        }

        let mut fields = Vec::new();
        let mut norms = Vec::with_capacity(self.text.len() * docs);
        let mut length_tables = Vec::with_capacity(self.text.len());
        let mut total = 0_u64;
        write_varint(&mut fields, self.text.len() as u32);
        for field in &mut self.text {
            field.lengths.resize(docs, 0);
            let tokens: u64 = field.lengths.iter().map(|&l| u64::from(l)).sum();
            let with = field.lengths.iter().filter(|&&l| l > 0).count();
            total += tokens;
            write_str(&mut fields, &field.name);
            write_varint(&mut fields, with as u32);
            write_varint64(&mut fields, tokens);
            norms.extend(field.lengths.iter().map(|&l| norm::encode(l)));
            let average = if with == 0 {
                1.0
            } else {
                (tokens as f64 / with as f64).max(f64::MIN_POSITIVE)
            };
            length_tables.push(std::array::from_fn::<f64, 256, _>(|n| {
                f64::from(K1)
                    * (1.0 - f64::from(B)
                        + f64::from(B) * f64::from(norm::decode(n as u8)) / average)
            }));
        }

        let mut id_offsets = Vec::with_capacity((docs + 1) * 8);
        let mut id_bytes = Vec::new();
        for id in &self.ids {
            id_offsets.extend_from_slice(&(id_bytes.len() as u64).to_le_bytes());
            id_bytes.extend_from_slice(id.as_bytes());
        }
        id_offsets.extend_from_slice(&(id_bytes.len() as u64).to_le_bytes());

        let mut terms: Vec<FieldTerm> = self
            .text
            .iter()
            .enumerate()
            .flat_map(|(f, field)| field.postings.iter().map(move |(t, l)| (&**t, f as u8, l)))
            .collect();
        terms.sort_unstable_by(|a, b| a.0.cmp(b.0).then(a.1.cmp(&b.1)));
        let mut term_count = 0_u32;
        let mut table = Vec::new();
        let mut headers = Vec::new();
        let mut bodies = Vec::new();
        let mut dictionary = fst::MapBuilder::memory();
        let mut window = Vec::with_capacity(WINDOW);
        let mut position_terms = fst::MapBuilder::memory();
        let mut positions = Vec::new();
        // Where postings stand every `positions::SKIP` windows, for the positions' skip entries.
        let mut anchors: Vec<positions::Anchor> = Vec::new();
        for group in terms.chunk_by(|a, b| a.0 == b.0) {
            term_count += 1;
            let term = group[0].0;
            if self.positions {
                position_terms
                    .insert(term.as_bytes(), positions.len() as u64)
                    .expect("sorted unique terms");
            }
            if let [(_, field, List { docs, positions: p })] = group {
                if let [(doc, tf)] = docs.as_slice() {
                    let value = INLINE
                        | u64::from(*field) << INLINE_FIELD_SHIFT
                        | u64::from((*tf).min(MAX_TF)) << 32
                        | u64::from(*doc);
                    dictionary
                        .insert(term.as_bytes(), value)
                        .expect("sorted unique terms");
                    if self.positions {
                        positions::write(&mut positions, docs, p, &[]);
                    }
                    continue;
                }
            }
            dictionary
                .insert(term.as_bytes(), table.len() as u64)
                .expect("sorted unique terms");
            write_varint(&mut table, group.len() as u32);
            for &(
                _,
                field,
                List {
                    docs: list,
                    positions: p,
                },
            ) in group
            {
                anchors.clear();
                let (header_start, body_start) = (headers.len(), bodies.len());
                let f = usize::from(field);
                let field_norms = &norms[f * docs..(f + 1) * docs];
                let lengths = &length_tables[f];
                write_varint(&mut table, u32::from(field));
                write_varint64(&mut table, headers.len() as u64);
                write_varint64(&mut table, bodies.len() as u64);
                write_varint(&mut table, list.len() as u32);
                let mut windows = 0_u32;
                let mut previous = None;
                for chunk in list.chunk_by(|a, b| a.0 as usize / WINDOW == b.0 as usize / WINDOW) {
                    let id = chunk[0].0 / WINDOW as u32;
                    if self.positions
                        && windows > 0
                        && (windows as usize).is_multiple_of(positions::SKIP)
                    {
                        anchors.push(positions::Anchor {
                            previous: previous.unwrap_or(0),
                            headers: (headers.len() - header_start) as u32,
                            body: (bodies.len() - body_start) as u32,
                        });
                    }
                    windows += 1;
                    window.clear();
                    window.extend(
                        chunk
                            .iter()
                            .map(|&(doc, tf)| ((doc as usize % WINDOW) as u8, tf.min(MAX_TF))),
                    );
                    let saturation = chunk
                        .iter()
                        .map(|&(doc, tf)| {
                            let tf = f64::from(tf.min(MAX_TF));
                            tf / (tf + lengths[usize::from(field_norms[doc as usize])])
                        })
                        .fold(0.0, f64::max);
                    write_window(
                        &mut headers,
                        &mut bodies,
                        previous.map_or(id, |p| id - p),
                        &window,
                        quantize_up(saturation),
                    );
                    previous = Some(id);
                }
                write_varint(&mut table, windows);
                if self.positions {
                    positions::write(&mut positions, list, p, &anchors);
                }
            }
        }
        bodies.extend_from_slice(&[0; PADDING]);
        let dictionary = dictionary.into_inner().expect("in-memory dictionary");
        let position_terms = if self.positions {
            positions.extend_from_slice(&[0; PADDING]);
            position_terms
                .into_inner()
                .expect("in-memory positions map")
        } else {
            Vec::new()
        };

        let mut by_id: Vec<(&str, u32)> = self
            .ids
            .iter()
            .enumerate()
            .map(|(d, id)| (id.as_str(), d as u32))
            .collect();
        by_id.sort_by(|a, b| a.0.cmp(b.0).then(b.1.cmp(&a.1)));
        by_id.dedup_by(|later, kept| later.0 == kept.0);
        let mut lookup = fst::MapBuilder::memory();
        for (id, doc) in by_id {
            lookup
                .insert(id, u64::from(doc))
                .expect("sorted unique ids");
        }
        let lookup = lookup.into_inner().expect("in-memory id lookup");

        let (keywords, keyword_docs) = keyword_sections(&mut self.keywords, &mut fields);
        let numbers = numeric_section(&mut self.numbers, &mut fields);
        let (stored_index, stored) = stored::write(&self.stored, docs);

        let sections = [
            norms,
            id_offsets,
            id_bytes,
            table,
            headers,
            bodies,
            dictionary,
            lookup,
            fields,
            keywords,
            keyword_docs,
            numbers,
            stored_index,
            stored,
            position_terms,
            positions,
        ];
        let mut out = Vec::with_capacity(HEADER_LEN + sections.iter().map(Vec::len).sum::<usize>());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(docs as u32).to_le_bytes());
        out.extend_from_slice(&term_count.to_le_bytes());
        out.extend_from_slice(&total.to_le_bytes());
        let mut at = HEADER_LEN as u64;
        for section in &sections {
            out.extend_from_slice(&at.to_le_bytes());
            at += section.len() as u64;
        }
        for section in sections {
            out.extend_from_slice(&section);
        }
        out
    }
}

/// Keys are the field's index byte then the value; values point at delta-coded document lists.
fn keyword_sections(fields: &mut [KeywordField], meta: &mut Vec<u8>) -> (Vec<u8>, Vec<u8>) {
    write_varint(meta, fields.len() as u32);
    let mut entries: Vec<(Vec<u8>, &mut Vec<u32>)> = Vec::new();
    for (f, field) in fields.iter_mut().enumerate() {
        write_str(meta, &field.name);
        for (value, docs) in field.docs.iter_mut() {
            let mut key = Vec::with_capacity(value.len() + 1);
            key.push(f as u8);
            key.extend_from_slice(value.as_bytes());
            entries.push((key, docs));
        }
    }
    entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let mut map = fst::MapBuilder::memory();
    let mut lists = Vec::new();
    for (key, docs) in entries {
        docs.sort_unstable();
        docs.dedup();
        map.insert(&key, lists.len() as u64)
            .expect("sorted unique keyword keys");
        write_varint(&mut lists, docs.len() as u32);
        let mut previous = 0;
        for &doc in docs.iter() {
            write_varint(&mut lists, doc - previous);
            previous = doc;
        }
    }
    (map.into_inner().expect("in-memory keyword map"), lists)
}

/// Per field, values sorted ascending then their documents; floats if any value is one.
fn numeric_section(fields: &mut [NumericField], meta: &mut Vec<u8>) -> Vec<u8> {
    write_varint(meta, fields.len() as u32);
    let mut out = Vec::new();
    for field in fields.iter_mut() {
        let float = field.values.iter().any(|v| matches!(v.0, Number::Float(_)));
        let mut pairs: Vec<(u64, i64, f64, u32)> = field
            .values
            .iter()
            .map(|&(v, doc)| match (float, v) {
                (false, Number::Int(i)) => (i as u64, i, 0.0, doc),
                (_, v) => {
                    let x = v.as_f64();
                    (x.to_bits(), 0, x, doc)
                }
            })
            .collect();
        if float {
            pairs.sort_unstable_by(|a, b| a.2.total_cmp(&b.2).then(a.3.cmp(&b.3)));
        } else {
            pairs.sort_unstable_by(|a, b| a.1.cmp(&b.1).then(a.3.cmp(&b.3)));
        }
        pairs.dedup_by(|a, b| a.0 == b.0 && a.3 == b.3);
        write_str(meta, &field.name);
        meta.push(u8::from(float));
        write_varint64(meta, out.len() as u64);
        write_varint(meta, pairs.len() as u32);
        for p in &pairs {
            out.extend_from_slice(&p.0.to_le_bytes());
        }
        for p in &pairs {
            out.extend_from_slice(&p.3.to_le_bytes());
        }
    }
    out
}

/// Moves postings and their positions to new document numbers, dropping those mapped to `u32::MAX`.
fn renumber_with_positions(list: &mut List, new_of: &[u32]) {
    let mut moved = Vec::with_capacity(list.docs.len());
    let mut at = 0;
    for &(doc, tf) in &list.docs {
        let new = new_of[doc as usize];
        if new != u32::MAX {
            moved.push((new, tf, at));
        }
        at += positions::kept(tf);
    }
    moved.sort_unstable_by_key(|m| m.0);
    let mut positions = Vec::with_capacity(list.positions.len());
    for &(_, tf, at) in &moved {
        positions.extend_from_slice(&list.positions[at..at + positions::kept(tf)]);
    }
    list.docs = moved.into_iter().map(|(doc, tf, _)| (doc, tf)).collect();
    list.positions = positions;
}
