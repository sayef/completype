use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use fst::Streamer;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::search::{Collection, Ranked, TopK};
use crate::{Analyzer, Searcher, Segment, SegmentBuilder};

const MANIFEST: &str = "manifest";
const MANIFEST_HEADER: &str = "completype-fts 1";
const LOCK: &str = "writer.lock";
/// Merges the smallest segments once a commit leaves more than this many.
const MAX_SEGMENTS: usize = 10;
/// Terms in at least this many documents are taken as spelled correctly.
const RARE_DF: u64 = 20;
/// A correction replaces a known term only when this many times more common.
const MORE_COMMON: u64 = 50;

/// A directory of segments, written by one [`Writer`] and searched through [`Reader`] snapshots.
#[derive(Clone)]
pub struct Index {
    dir: PathBuf,
    analyzer: Analyzer,
    reorder: Reordering,
    first_letter_typos: bool,
}

/// When segments are written with similar documents placed together.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Reordering {
    Never,
    /// Commits stay quick; merged segments, which hold most documents over time, are reordered.
    #[default]
    OnMerge,
    Always,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchHit {
    pub id: String,
    pub score: f32,
}

#[derive(Clone, Debug, Default)]
struct Entry {
    number: u64,
    deletes: Option<u64>,
}

fn segment_file(number: u64) -> String {
    format!("{number:010}.seg")
}

fn deletes_file(number: u64, generation: u64) -> String {
    format!("{number:010}.{generation}.del")
}

fn read_manifest(dir: &Path) -> io::Result<Vec<Entry>> {
    let text = fs::read_to_string(dir.join(MANIFEST))?;
    let mut lines = text.lines();
    if lines.next() != Some(MANIFEST_HEADER) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a completype-fts index",
        ));
    }
    lines
        .filter(|l| !l.is_empty())
        .map(|line| {
            let mut parts = line.split(' ');
            let number = parts.next().and_then(|n| n.parse().ok());
            let deletes = parts.next().map(|d| {
                if d == "-" {
                    Ok(None)
                } else {
                    d.parse().map(Some)
                }
            });
            match (number, deletes) {
                (Some(number), Some(Ok(deletes))) => Ok(Entry { number, deletes }),
                _ => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("bad manifest line: {line}"),
                )),
            }
        })
        .collect()
}

/// Replaces a file atomically: write beside it, sync, rename.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    let mut file = fs::File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(tmp, path)
}

fn write_manifest(dir: &Path, entries: &[Entry]) -> io::Result<()> {
    let mut text = format!("{MANIFEST_HEADER}\n");
    for e in entries {
        let deletes = e.deletes.map_or("-".to_owned(), |g| g.to_string());
        text.push_str(&format!("{} {deletes}\n", e.number));
    }
    write_atomic(&dir.join(MANIFEST), text.as_bytes())
}

fn read_deletes(dir: &Path, entry: &Entry) -> io::Result<Option<Vec<u64>>> {
    let Some(generation) = entry.deletes else {
        return Ok(None);
    };
    let bytes = fs::read(dir.join(deletes_file(entry.number, generation)))?;
    Ok(Some(
        bytes
            .chunks(8)
            .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
            .collect(),
    ))
}

impl Index {
    /// Opens the index in `dir`, creating an empty one if there is none.
    pub fn open(dir: impl AsRef<Path>) -> io::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        if !dir.join(MANIFEST).exists() {
            write_manifest(&dir, &[])?;
        }
        read_manifest(&dir)?;
        Ok(Self {
            dir,
            analyzer: Analyzer::default(),
            reorder: Reordering::OnMerge,
            first_letter_typos: false,
        })
    }

    pub fn with_analyzer(mut self, analyzer: Analyzer) -> Self {
        self.analyzer = analyzer;
        self
    }

    /// Which segments place similar documents together: smaller and faster, slower to write.
    pub fn with_reordering(mut self, reordering: Reordering) -> Self {
        self.reorder = reordering;
        self
    }

    /// Lets corrections change a term's first character; by default they keep it, as Lucene's
    /// spell checker does, which makes them several times faster.
    pub fn with_first_letter_typos(mut self, on: bool) -> Self {
        self.first_letter_typos = on;
        self
    }

    pub fn analyzer(&self) -> &Analyzer {
        &self.analyzer
    }

    /// The only writer; fails while another holds the lock.
    pub fn writer(&self) -> io::Result<Writer> {
        let lock = self.dir.join(LOCK);
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
            .map_err(|e| {
                if e.kind() == io::ErrorKind::AlreadyExists {
                    io::Error::new(e.kind(), format!("another writer holds {}", lock.display()))
                } else {
                    e
                }
            })?;
        Ok(Writer {
            index: self.clone(),
            lock,
            builder: self.new_builder(false),
            buffered: FxHashMap::default(),
            deletes: FxHashSet::default(),
        })
    }

    fn new_builder(&self, merging: bool) -> SegmentBuilder {
        let mut builder = SegmentBuilder::default();
        builder.reorder(match self.reorder {
            Reordering::Never => false,
            Reordering::OnMerge => merging,
            Reordering::Always => true,
        });
        builder
    }

    /// A snapshot of the committed segments.
    pub fn reader(&self) -> io::Result<Reader> {
        let entries = read_manifest(&self.dir)?;
        let mut segments = Vec::with_capacity(entries.len());
        for entry in &entries {
            let segment = Segment::open(self.dir.join(segment_file(entry.number)))?;
            segments.push((segment, read_deletes(&self.dir, entry)?));
        }
        let collection = Collection {
            docs: segments.iter().map(|s| s.0.len() as u64).sum(),
            tokens: segments.iter().map(|s| s.0.total_tokens()).sum(),
        };
        Ok(Reader {
            analyzer: self.analyzer.clone(),
            first_letter_typos: self.first_letter_typos,
            segments,
            collection,
        })
    }
}

/// Buffers additions and deletions until [`Writer::commit`] makes them visible to new readers.
pub struct Writer {
    index: Index,
    lock: PathBuf,
    builder: SegmentBuilder,
    buffered: FxHashMap<String, u32>,
    deletes: FxHashSet<String>,
}

impl Drop for Writer {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lock);
    }
}

impl Writer {
    /// Adds a document, replacing any earlier one with the same id.
    pub fn add(&mut self, id: &str, text: &str) {
        let terms = self.index.analyzer.terms(text);
        self.add_terms(id, terms.iter().map(String::as_str));
    }

    /// Adds an already analysed document, replacing any earlier one with the same id.
    pub fn add_terms<'a>(&mut self, id: &str, terms: impl IntoIterator<Item = &'a str>) {
        self.delete(id);
        let doc = self.builder.len() as u32;
        self.builder.add(id, terms);
        self.buffered.insert(id.to_owned(), doc);
    }

    pub fn delete(&mut self, id: &str) {
        if let Some(doc) = self.buffered.remove(id) {
            self.builder.drop_document(doc);
        }
        self.deletes.insert(id.to_owned());
    }

    /// Writes buffered documents as a segment and records deletions, atomically.
    pub fn commit(&mut self) -> io::Result<()> {
        let dir = self.index.dir.clone();
        let mut entries = read_manifest(&dir)?;
        let mut obsolete = Vec::new();
        for entry in entries.iter_mut() {
            let segment = Segment::open(dir.join(segment_file(entry.number)))?;
            let docs: Vec<u32> = self
                .deletes
                .iter()
                .filter_map(|id| segment.doc(id))
                .collect();
            let mut bits =
                read_deletes(&dir, entry)?.unwrap_or_else(|| vec![0; segment.len().div_ceil(64)]);
            let fresh: Vec<u32> = docs
                .into_iter()
                .filter(|&d| bits[d as usize / 64] >> (d % 64) & 1 == 0)
                .collect();
            if fresh.is_empty() {
                continue;
            }
            fresh
                .iter()
                .for_each(|&d| bits[d as usize / 64] |= 1 << (d % 64));
            let generation = entry.deletes.map_or(1, |g| g + 1);
            write_atomic(
                &dir.join(deletes_file(entry.number, generation)),
                &bits_bytes(&bits),
            )?;
            if let Some(old) = entry.deletes {
                obsolete.push(deletes_file(entry.number, old));
            }
            entry.deletes = Some(generation);
        }
        if self.builder.live() > 0 {
            let builder = std::mem::replace(&mut self.builder, self.index.new_builder(false));
            let number = entries.iter().map(|e| e.number + 1).max().unwrap_or(1);
            write_atomic(&dir.join(segment_file(number)), &builder.finish())?;
            let entry = Entry {
                number,
                deletes: None,
            };
            entries.push(entry);
        }
        write_manifest(&dir, &entries)?;
        for file in obsolete {
            let _ = fs::remove_file(dir.join(file));
        }
        self.buffered.clear();
        self.deletes.clear();
        if entries.len() > MAX_SEGMENTS {
            self.merge_smallest(entries.len() - MAX_SEGMENTS + 1)?;
        }
        Ok(())
    }

    /// Merges every committed segment into one, dropping deleted documents.
    pub fn merge(&mut self) -> io::Result<()> {
        let count = read_manifest(&self.index.dir)?.len();
        if count > 1 || self.index.reader()?.segments.iter().any(|s| s.1.is_some()) {
            self.merge_smallest(count)?;
        }
        Ok(())
    }

    fn merge_smallest(&mut self, count: usize) -> io::Result<()> {
        let dir = self.index.dir.clone();
        let entries = read_manifest(&dir)?;
        let mut sized: Vec<(u64, usize)> = entries
            .iter()
            .map(|e| {
                fs::metadata(dir.join(segment_file(e.number))).map(|m| (m.len(), e.number as usize))
            })
            .collect::<io::Result<_>>()?;
        sized.sort_unstable();
        let chosen: FxHashSet<u64> = sized.iter().take(count).map(|s| s.1 as u64).collect();
        let merged: Vec<&Entry> = entries
            .iter()
            .filter(|e| chosen.contains(&e.number))
            .collect();
        let mut sources = Vec::new();
        for entry in &merged {
            sources.push((
                Segment::open(dir.join(segment_file(entry.number)))?,
                read_deletes(&dir, entry)?,
            ));
        }
        let bytes = merge_segments(&sources, self.index.new_builder(true));
        let number = entries.iter().map(|e| e.number + 1).max().unwrap_or(1);
        write_atomic(&dir.join(segment_file(number)), &bytes)?;
        let mut kept: Vec<Entry> = entries
            .iter()
            .filter(|e| !chosen.contains(&e.number))
            .cloned()
            .collect();
        kept.push(Entry {
            number,
            deletes: None,
        });
        write_manifest(&dir, &kept)?;
        drop(sources);
        for entry in merged {
            let _ = fs::remove_file(dir.join(segment_file(entry.number)));
            if let Some(g) = entry.deletes {
                let _ = fs::remove_file(dir.join(deletes_file(entry.number, g)));
            }
        }
        Ok(())
    }
}

fn bits_bytes(bits: &[u64]) -> Vec<u8> {
    bits.iter().flat_map(|w| w.to_le_bytes()).collect()
}

fn is_deleted(deletes: &Option<Vec<u64>>, doc: u32) -> bool {
    deletes
        .as_ref()
        .is_some_and(|d| d[doc as usize / 64] >> (doc % 64) & 1 != 0)
}

/// One segment holding the live documents of `sources`, oldest first.
fn merge_segments(sources: &[(Segment, Option<Vec<u64>>)], mut builder: SegmentBuilder) -> Vec<u8> {
    let mut remap: Vec<Vec<u32>> = Vec::with_capacity(sources.len());
    let mut lengths: Vec<u32> = Vec::new();
    for (segment, deletes) in sources {
        let mut map = vec![u32::MAX; segment.len()];
        for doc in 0..segment.len() as u32 {
            if !is_deleted(deletes, doc) {
                map[doc as usize] = lengths.len() as u32;
                lengths.push(0);
            }
        }
        remap.push(map);
    }
    let mut lists: BTreeMap<Vec<u8>, Vec<(u32, u32)>> = BTreeMap::new();
    for (s, (segment, _)) in sources.iter().enumerate() {
        let mut stream = segment.terms();
        while let Some((term, value)) = stream.next() {
            let info = segment.info(value);
            let list = lists.entry(term.to_vec()).or_default();
            for (doc, tf) in segment.postings(&info) {
                let new = remap[s][doc as usize];
                if new != u32::MAX {
                    list.push((new, tf));
                    lengths[new as usize] += tf;
                }
            }
        }
    }
    for (s, (segment, _)) in sources.iter().enumerate() {
        for doc in 0..segment.len() as u32 {
            let new = remap[s][doc as usize];
            if new != u32::MAX {
                builder.push_document(segment.id(doc).to_owned(), lengths[new as usize]);
            }
        }
    }
    for (term, list) in lists {
        if !list.is_empty() {
            builder.set_postings(std::str::from_utf8(&term).expect("terms are UTF-8"), list);
        }
    }
    builder.finish()
}

/// A consistent view of the index at one commit.
pub struct Reader {
    analyzer: Analyzer,
    first_letter_typos: bool,
    segments: Vec<(Segment, Option<Vec<u64>>)>,
    collection: Collection,
}

impl Reader {
    /// Live documents.
    pub fn len(&self) -> usize {
        self.segments
            .iter()
            .map(|(s, d)| {
                s.len()
                    - d.as_ref()
                        .map_or(0, |d| d.iter().map(|w| w.count_ones() as usize).sum())
            })
            .sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }

    /// The best `limit` documents for `text`, analysed like the documents, with rare terms
    /// also matching their likely correct spelling.
    pub fn search(&self, text: &str, limit: usize) -> Vec<SearchHit> {
        let mut weights: Vec<(String, f32)> = Vec::new();
        for term in self.analyzer.terms(text) {
            for (word, weight) in self.spellings(&term) {
                match weights.iter_mut().find(|w| w.0 == word) {
                    Some(w) => w.1 = w.1.max(weight),
                    None => weights.push((word, weight)),
                }
            }
        }
        self.search_terms(weights.iter().map(|(t, w)| (t.as_str(), *w)), limit)
    }

    /// [`Reader::search`] without spelling corrections.
    pub fn search_exact(&self, text: &str, limit: usize) -> Vec<SearchHit> {
        let mut weights: Vec<(String, f32)> = Vec::new();
        self.analyzer.analyze(text, |term| {
            match weights.iter_mut().find(|w| w.0 == term) {
                Some(w) => w.1 += 1.0,
                None => weights.push((term.to_owned(), 1.0)),
            }
        });
        self.search_terms(weights.iter().map(|(t, w)| (t.as_str(), *w)), limit)
    }

    pub fn document_frequency(&self, term: &str) -> u64 {
        self.segments
            .iter()
            .map(|s| u64::from(s.0.document_frequency(term)))
            .sum()
    }

    /// `term` and the closest much more common indexed term, weighted for a query.
    pub fn spellings(&self, term: &str) -> Vec<(String, f32)> {
        let df = self.document_frequency(term);
        let len = term.chars().count();
        if df >= RARE_DF || len < 3 {
            return vec![(term.to_owned(), 1.0)];
        }
        let mut found: FxHashMap<String, (u8, u64)> = FxHashMap::default();
        // The closest spelling wins, so two edits are searched only when one finds nothing.
        for max in [1, 2].into_iter().take(if len <= 4 { 1 } else { 2 }) {
            for (segment, _) in &self.segments {
                for m in crate::fuzzy::within_from(
                    segment.dictionary(),
                    term,
                    max,
                    !self.first_letter_typos,
                ) {
                    if m.term != term {
                        let entry = found.entry(m.term).or_insert((m.distance, 0));
                        entry.1 += u64::from(segment.info(m.value).df);
                    }
                }
            }
            if !found.is_empty() {
                break;
            }
        }
        let best = found
            .into_iter()
            .filter(|(word, _)| word != term)
            .min_by(|a, b| {
                a.1 .0
                    .cmp(&b.1 .0)
                    .then(b.1 .1.cmp(&a.1 .1))
                    .then(a.0.cmp(&b.0))
            });
        match best {
            Some((word, (distance, common))) if df == 0 || common >= MORE_COMMON * df => {
                let mut out = vec![(word, if distance == 1 { 0.9 } else { 0.7 })];
                if df > 0 {
                    out.push((term.to_owned(), 1.0));
                }
                out
            }
            _ => vec![(term.to_owned(), 1.0)],
        }
    }

    /// The best `limit` documents for weighted terms, scored with BM25 over all segments.
    pub fn search_terms<'a>(
        &self,
        terms: impl IntoIterator<Item = (&'a str, f32)>,
        limit: usize,
    ) -> Vec<SearchHit> {
        self.run(terms, limit, false)
    }

    /// [`Reader::search_terms`] scoring every posting: the reference pruning must equal.
    #[doc(hidden)]
    pub fn search_terms_exhaustive<'a>(
        &self,
        terms: impl IntoIterator<Item = (&'a str, f32)>,
        limit: usize,
    ) -> Vec<SearchHit> {
        self.run(terms, limit, true)
    }

    fn run<'a>(
        &self,
        terms: impl IntoIterator<Item = (&'a str, f32)>,
        limit: usize,
        exhaustive: bool,
    ) -> Vec<SearchHit> {
        let terms: Vec<(&str, f32)> = terms.into_iter().collect();
        let dfs: Vec<u64> = terms
            .iter()
            .map(|(t, _)| {
                self.segments
                    .iter()
                    .map(|s| u64::from(s.0.document_frequency(t)))
                    .sum()
            })
            .collect();
        let mut top = TopK::new(limit);
        let mut order: Vec<usize> = (0..self.segments.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(self.segments[i].0.len()));
        for i in order {
            let (segment, deletes) = &self.segments[i];
            let mut searcher = Searcher::within(segment, self.collection, deletes.as_deref());
            let query = searcher.prepare_with(
                terms
                    .iter()
                    .zip(&dfs)
                    .map(|(&(t, w), &df)| (t, w, Some(df))),
            );
            if exhaustive {
                searcher.collect_exhaustive(&query, &mut top, (i as u64) << 32);
            } else {
                searcher.collect(&query, &mut top, (i as u64) << 32);
            }
        }
        top.into_sorted().into_iter().map(|r| self.hit(r)).collect()
    }

    fn hit(&self, ranked: Ranked) -> SearchHit {
        let segment = &self.segments[(ranked.position >> 32) as usize].0;
        SearchHit {
            id: segment.id(ranked.position as u32).to_owned(),
            score: ranked.score,
        }
    }
}
