use std::io;
use std::path::Path;

use rustc_hash::FxHashMap;

use crate::format::*;
use crate::{norm, reorder};

/// Collects documents as token streams and writes them as one immutable segment.
#[derive(Default)]
pub struct SegmentBuilder {
    ids: Vec<String>,
    lengths: Vec<u32>,
    postings: FxHashMap<Box<str>, Vec<(u32, u32)>>,
    counts: FxHashMap<String, u32>,
    reorder: bool,
    dropped: Vec<u32>,
}

impl SegmentBuilder {
    pub fn add<'a>(&mut self, id: impl Into<String>, tokens: impl IntoIterator<Item = &'a str>) {
        let doc =
            u32::try_from(self.ids.len()).expect("a segment holds at most u32::MAX documents");
        self.ids.push(id.into());
        let mut length = 0_u32;
        for token in tokens {
            length = length.saturating_add(1);
            match self.counts.get_mut(token) {
                Some(n) => *n += 1,
                None => {
                    self.counts.insert(token.to_owned(), 1);
                }
            }
        }
        self.lengths.push(length);
        for (term, tf) in self.counts.drain() {
            match self.postings.get_mut(term.as_str()) {
                Some(list) => list.push((doc, tf)),
                None => {
                    self.postings.insert(term.into_boxed_str(), vec![(doc, tf)]);
                }
            }
        }
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

    /// Appends a document known only by its id and length, for merging.
    pub(crate) fn push_document(&mut self, id: String, length: u32) -> u32 {
        self.ids.push(id);
        self.lengths.push(length);
        (self.ids.len() - 1) as u32
    }

    /// Sets a term's whole postings list, in document order, for merging.
    pub(crate) fn set_postings(&mut self, term: &str, list: Vec<(u32, u32)>) {
        self.postings.insert(term.into(), list);
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
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
        let mut doc = 0;
        self.ids.retain(|_| (keep[doc], doc += 1).0);
        let mut doc = 0;
        self.lengths.retain(|_| (keep[doc], doc += 1).0);
        self.postings.retain(|_, list| {
            list.retain_mut(|p| {
                p.0 = new_of[p.0 as usize];
                p.0 != u32::MAX
            });
            !list.is_empty()
        });
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
            let forward = reorder::Forward::new(docs, self.postings.values().map(Vec::as_slice));
            let order = reorder::order(&forward, docs);
            drop(forward);
            let mut new_of = vec![0_u32; docs];
            for (new, &old) in order.iter().enumerate() {
                new_of[old as usize] = new as u32;
            }
            let mut ids = std::mem::take(&mut self.ids);
            self.ids = order
                .iter()
                .map(|&old| std::mem::take(&mut ids[old as usize]))
                .collect();
            self.lengths = order
                .iter()
                .map(|&old| self.lengths[old as usize])
                .collect();
            for list in self.postings.values_mut() {
                list.iter_mut().for_each(|p| p.0 = new_of[p.0 as usize]);
                list.sort_unstable_by_key(|p| p.0);
            }
        }
        let total: u64 = self.lengths.iter().map(|&l| u64::from(l)).sum();
        let norms: Vec<u8> = self.lengths.iter().map(|&l| norm::encode(l)).collect();
        let average = if docs == 0 {
            1.0
        } else {
            (total as f64 / docs as f64).max(f64::MIN_POSITIVE)
        };
        let lengths: [f64; 256] = std::array::from_fn(|n| {
            f64::from(K1)
                * (1.0 - f64::from(B) + f64::from(B) * f64::from(norm::decode(n as u8)) / average)
        });

        let mut id_offsets = Vec::with_capacity((docs + 1) * 8);
        let mut id_bytes = Vec::new();
        for id in &self.ids {
            id_offsets.extend_from_slice(&(id_bytes.len() as u64).to_le_bytes());
            id_bytes.extend_from_slice(id.as_bytes());
        }
        id_offsets.extend_from_slice(&(id_bytes.len() as u64).to_le_bytes());

        let mut terms: Vec<_> = self.postings.into_iter().collect();
        terms.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        let mut table = Vec::new();
        let mut headers = Vec::new();
        let mut bodies = Vec::new();
        let mut dictionary = fst::MapBuilder::memory();
        let mut window = Vec::with_capacity(WINDOW);
        for (term, list) in &terms {
            if let [(doc, tf)] = list[..] {
                let value = INLINE | u64::from(tf.min(MAX_TF)) << 32 | u64::from(doc);
                dictionary
                    .insert(term.as_bytes(), value)
                    .expect("sorted unique terms");
                continue;
            }
            dictionary
                .insert(term.as_bytes(), table.len() as u64)
                .expect("sorted unique terms");
            write_varint64(&mut table, headers.len() as u64);
            write_varint64(&mut table, bodies.len() as u64);
            write_varint(&mut table, list.len() as u32);
            let mut windows = 0_u32;
            let mut previous = None;
            for chunk in list.chunk_by(|a, b| a.0 as usize / WINDOW == b.0 as usize / WINDOW) {
                let id = chunk[0].0 / WINDOW as u32;
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
                        tf / (tf + lengths[usize::from(norms[doc as usize])])
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
        }
        bodies.extend_from_slice(&[0; PADDING]);
        let dictionary = dictionary.into_inner().expect("in-memory dictionary");

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
        let sections = [
            norms, id_offsets, id_bytes, table, headers, bodies, dictionary, lookup,
        ];
        let mut out = Vec::with_capacity(HEADER_LEN + sections.iter().map(Vec::len).sum::<usize>());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(docs as u32).to_le_bytes());
        out.extend_from_slice(&(terms.len() as u32).to_le_bytes());
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
