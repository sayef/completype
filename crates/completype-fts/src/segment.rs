use std::fs::File;
use std::io;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use memmap2::Mmap;

use crate::format::*;

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
        if bytes.len() < HEADER_LEN || &bytes[..8] != MAGIC {
            return Err(invalid("not a completype-fts segment"));
        }
        let docs = u32_at(bytes, 8);
        let total = u64_at(bytes, 16);
        let starts: Vec<usize> = (0..SECTIONS)
            .map(|i| u64_at(bytes, 24 + 8 * i) as usize)
            .collect();
        let mut sections: [Range<usize>; SECTIONS] = Default::default();
        for i in 0..SECTIONS {
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
        Ok(Self {
            storage,
            sections,
            docs,
            total,
            dictionary,
            lookup,
        })
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

    pub(crate) fn info(&self, value: u64) -> TermInfo {
        if value & INLINE != 0 {
            let posting = (value as u32, (value >> 32) as u32 & MAX_TF);
            return TermInfo {
                inline: Some(posting),
                headers: 0,
                bodies: 0,
                df: 1,
                windows: 1,
            };
        }
        let (table, mut at) = (self.section(TERMS), value as usize);
        let headers = read_varint64(table, &mut at) as usize;
        let bodies = read_varint64(table, &mut at) as usize;
        let df = read_varint(table, &mut at);
        let windows = read_varint(table, &mut at);
        TermInfo {
            inline: None,
            headers,
            bodies,
            df,
            windows,
        }
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

    pub(crate) fn norms(&self) -> &[u8] {
        self.section(NORMS)
    }

    pub(crate) fn term(&self, term: &str) -> Option<TermInfo> {
        self.dictionary.get(term).map(|value| self.info(value))
    }

    pub fn document_frequency(&self, term: &str) -> u32 {
        self.term(term).map_or(0, |t| t.df)
    }

    pub(crate) fn headers(&self, info: &TermInfo) -> Headers<'_> {
        match info.inline {
            Some((doc, tf)) => Headers::single(doc, tf),
            None => Headers::new(&self.section(HEADERS)[info.headers..], info.windows),
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
        ];
        std::array::from_fn(|i| (names[i], self.sections[i].len()))
    }
}
