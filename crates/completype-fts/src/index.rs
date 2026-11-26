use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use fst::Streamer;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::build::List;
use crate::search::{Collection, FieldStats, Ranked, TopK};

/// Postings lists by text field and term, in document order.
type Postings = BTreeMap<(usize, Vec<u8>), List>;
/// A phrase clause: field, analysed words, slop and weight.
type PhraseClause<'a> = (&'a str, &'a [String], u32, f32);
use crate::{Analyzer, Document, Filter, Query, Searcher, Segment, SegmentBuilder, StopWords};

const MANIFEST: &str = "manifest";
const MANIFEST_HEADER: &str = "completype-fts 3";
/// Written before positions were recorded.
const MANIFEST_V2: &str = "completype-fts 2";
/// Written before the analyzer was recorded.
const MANIFEST_V1: &str = "completype-fts 1";
const POSITIONS_LINE: &str = "positions";
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
    stop_words: Option<StopWords>,
    positions: bool,
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

struct Manifest {
    /// The analyzer that made the segments; `None` before the first commit and in version 1.
    analyzer: Option<Analyzer>,
    positions: bool,
    entries: Vec<Entry>,
}

impl Manifest {
    /// Whether segments were committed, fixing the analyzer and positions.
    fn committed(&self) -> bool {
        self.analyzer.is_some() || !self.entries.is_empty()
    }
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn read_manifest(dir: &Path) -> io::Result<Manifest> {
    let text = fs::read_to_string(dir.join(MANIFEST))?;
    let mut lines = text.lines().peekable();
    let header = lines.next();
    let analyzer = match header {
        Some(MANIFEST_HEADER | MANIFEST_V2) => {
            match lines.next_if(|l| l.starts_with("analyzer ")) {
                Some(line) => Some(
                    Analyzer::from_spec(&line["analyzer ".len()..])
                        .ok_or_else(|| invalid(format!("unknown analyzer: {line}")))?,
                ),
                None => None,
            }
        }
        Some(MANIFEST_V1) => None,
        _ => return Err(invalid("not a completype-fts index".to_owned())),
    };
    let positions =
        header == Some(MANIFEST_HEADER) && lines.next_if(|&l| l == POSITIONS_LINE).is_some();
    let entries = lines
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
                _ => Err(invalid(format!("bad manifest line: {line}"))),
            }
        })
        .collect::<io::Result<_>>()?;
    Ok(Manifest {
        analyzer,
        positions,
        entries,
    })
}

/// Replaces a file atomically: write beside it, sync, rename.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    let mut file = fs::File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(tmp, path)
}

fn write_manifest(
    dir: &Path,
    analyzer: Option<&Analyzer>,
    positions: bool,
    entries: &[Entry],
) -> io::Result<()> {
    let mut text = format!("{MANIFEST_HEADER}\n");
    if let Some(analyzer) = analyzer {
        text.push_str(&format!("analyzer {}\n", analyzer.spec()));
    }
    if positions {
        text.push_str(POSITIONS_LINE);
        text.push('\n');
    }
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
            write_manifest(&dir, None, false, &[])?;
        }
        let manifest = read_manifest(&dir)?;
        Ok(Self {
            dir,
            analyzer: manifest.analyzer.unwrap_or_default(),
            reorder: Reordering::OnMerge,
            first_letter_typos: false,
            stop_words: None,
            positions: manifest.positions,
        })
    }

    /// Keeps token positions for phrases, fixed at the first commit like the analyzer. Off by
    /// default: segments grow by about half on short texts and double on long ones.
    pub fn with_positions(mut self, on: bool) -> Self {
        self.positions = on;
        self
    }

    pub fn positions(&self) -> bool {
        self.positions
    }

    /// Analyzes documents and queries; an index keeps the one its first commit used, so
    /// [`Index::writer`] and [`Index::reader`] refuse a different one.
    pub fn with_analyzer(mut self, analyzer: Analyzer) -> Self {
        self.analyzer = analyzer;
        self
    }

    /// Leaves these words out of queries, unless a query has nothing else.
    pub fn with_stop_words(mut self, stop_words: StopWords) -> Self {
        self.stop_words = Some(stop_words);
        self
    }

    /// Fails when the committed segments were analyzed differently or differ in positions.
    fn check_analyzer(&self, manifest: &Manifest) -> io::Result<()> {
        if manifest.committed() && manifest.positions != self.positions {
            let kept = if manifest.positions {
                "with"
            } else {
                "without"
            };
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} was indexed {kept} positions", self.dir.display()),
            ));
        }
        match &manifest.analyzer {
            Some(indexed) if *indexed != self.analyzer => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "{} was indexed with analyzer {indexed}, not {}",
                    self.dir.display(),
                    self.analyzer
                ),
            )),
            _ => Ok(()),
        }
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
        self.check_analyzer(&read_manifest(&self.dir)?)?;
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
        builder.positions(self.positions);
        builder
    }

    /// A snapshot of the committed segments.
    pub fn reader(&self) -> io::Result<Reader> {
        let manifest = read_manifest(&self.dir)?;
        self.check_analyzer(&manifest)?;
        let entries = manifest.entries;
        let mut segments = Vec::with_capacity(entries.len());
        for entry in &entries {
            let segment = Segment::open(self.dir.join(segment_file(entry.number)))?;
            segments.push((segment, read_deletes(&self.dir, entry)?));
        }
        let mut fields: Vec<String> = Vec::new();
        let mut stats: FxHashMap<&str, FieldStats> = FxHashMap::default();
        for (segment, _) in &segments {
            for field in segment.text_fields() {
                if !fields.contains(&field.name) {
                    fields.push(field.name.clone());
                }
                let total = stats.entry(&field.name).or_default();
                total.docs += u64::from(field.docs);
                total.tokens += field.tokens;
            }
        }
        let collections = segments
            .iter()
            .map(|(segment, _)| Collection {
                fields: segment
                    .text_fields()
                    .iter()
                    .map(|f| stats[f.name.as_str()])
                    .collect(),
            })
            .collect();
        Ok(Reader {
            stops: self
                .stop_words
                .as_ref()
                .map(|s| Stops::new(&self.analyzer, s))
                .unwrap_or_default(),
            analyzer: self.analyzer.clone(),
            first_letter_typos: self.first_letter_typos,
            segments,
            collections,
            fields,
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

    /// Adds a document with named fields, replacing any earlier one with the same id.
    pub fn add_document(&mut self, id: &str, document: &Document) {
        self.delete(id);
        let doc = self.builder.len() as u32;
        self.builder
            .add_document(id, document, &self.index.analyzer);
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
        let mut entries = read_manifest(&dir)?.entries;
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
        write_manifest(
            &dir,
            Some(&self.index.analyzer),
            self.index.positions,
            &entries,
        )?;
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
        let count = read_manifest(&self.index.dir)?.entries.len();
        if count > 1 || self.index.reader()?.segments.iter().any(|s| s.1.is_some()) {
            self.merge_smallest(count)?;
        }
        Ok(())
    }

    fn merge_smallest(&mut self, count: usize) -> io::Result<()> {
        let dir = self.index.dir.clone();
        let entries = read_manifest(&dir)?.entries;
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
        write_manifest(
            &dir,
            Some(&self.index.analyzer),
            self.index.positions,
            &kept,
        )?;
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
    let keep = builder.keeps_positions() && sources.iter().all(|s| s.0.has_positions());
    builder.positions(keep);
    let mut remap: Vec<Vec<u32>> = Vec::with_capacity(sources.len());
    for (segment, deletes) in sources {
        let mut map = vec![u32::MAX; segment.len()];
        for doc in 0..segment.len() as u32 {
            if !is_deleted(deletes, doc) {
                map[doc as usize] = builder.push_document(segment.id(doc).to_owned());
            }
        }
        remap.push(map);
    }
    let live = builder.len();
    let mut lengths: Vec<Vec<u32>> = Vec::new();
    let mut lists = Postings::new();
    for (s, (segment, _)) in sources.iter().enumerate() {
        let fields: Vec<usize> = segment
            .text_fields()
            .iter()
            .map(|f| builder.text_field(&f.name))
            .collect();
        let count = fields.iter().map(|&f| f + 1).max().unwrap_or(0);
        if lengths.len() < count {
            lengths.resize_with(count, || vec![0; live]);
        }
        let mut stream = segment.terms();
        while let Some((term, value)) = stream.next() {
            let text = std::str::from_utf8(term).expect("terms are UTF-8");
            for (field, info) in segment.fields_of(value) {
                let f = fields[usize::from(field)];
                let positions = keep
                    .then(|| segment.term_positions(field, text))
                    .flatten()
                    .map(|located| segment.all_positions(&info, located));
                let list = lists.entry((f, term.to_vec())).or_default();
                let mut at = 0;
                for (doc, tf) in segment.postings(&info) {
                    let new = remap[s][doc as usize];
                    let count = crate::positions::kept(tf);
                    if new != u32::MAX {
                        list.docs.push((new, tf));
                        lengths[f][new as usize] += tf;
                        if let Some(positions) = &positions {
                            list.positions.extend_from_slice(&positions[at..at + count]);
                        }
                    }
                    at += count;
                }
            }
        }
    }
    for (f, lengths) in lengths.into_iter().enumerate() {
        builder.set_lengths(f, lengths);
    }
    for ((field, term), list) in lists {
        if !list.docs.is_empty() {
            let term = std::str::from_utf8(&term).expect("terms are UTF-8");
            builder.set_postings(field, term, list);
        }
    }
    for (s, (segment, _)) in sources.iter().enumerate() {
        let names = segment.keyword_fields();
        let mut stream = segment.keyword_entries();
        while let Some((key, offset)) = stream.next() {
            let name = &names[usize::from(key[0])];
            let value = std::str::from_utf8(&key[1..]).expect("keywords are UTF-8");
            for doc in segment.keyword_list(Some(offset)) {
                let new = remap[s][doc as usize];
                if new != u32::MAX {
                    builder.push_keyword(name, value, new);
                }
            }
        }
        for field in segment.numeric_fields() {
            for i in 0..field.count {
                let (value, doc) = segment.numeric(field, i);
                let new = remap[s][doc as usize];
                if new != u32::MAX {
                    builder.push_number(&field.name, value, new);
                }
            }
        }
        let mut block: Option<(usize, Vec<u8>)> = None;
        for doc in 0..segment.len() as u32 {
            let new = remap[s][doc as usize];
            let Some((b, first)) = segment.stored_block(doc).filter(|_| new != u32::MAX) else {
                continue;
            };
            if block.as_ref().is_none_or(|c| c.0 != b) {
                block = Some((b, segment.stored_bytes(b)));
            }
            let bytes = &block.as_ref().expect("block just read").1;
            if let Some(payload) = crate::stored::extract(bytes, (doc - first) as usize) {
                builder.set_stored(new, payload.into());
            }
        }
    }
    builder.finish()
}

/// Stop words as analyzed: each word, and its term after stemming.
#[derive(Default)]
struct Stops {
    words: FxHashSet<String>,
    terms: FxHashSet<String>,
}

impl Stops {
    /// Keeps the stop words that analyze to a single term.
    fn new(analyzer: &Analyzer, list: &StopWords) -> Self {
        let mut stops = Self::default();
        for entry in list.words() {
            let mut parts = Vec::new();
            analyzer.analyze_words(entry, |word, term| {
                parts.push((word.to_owned(), term.to_owned()))
            });
            if let [(word, term)] = parts.as_slice() {
                stops.words.insert(word.clone());
                stops.terms.insert(term.clone());
            }
        }
        stops
    }
}

/// A consistent view of the index at one commit.
pub struct Reader {
    analyzer: Analyzer,
    stops: Stops,
    first_letter_typos: bool,
    segments: Vec<(Segment, Option<Vec<u64>>)>,
    collections: Vec<Collection>,
    /// Text fields of all segments, in order of first appearance.
    fields: Vec<String>,
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

    /// The best `limit` documents for `text` in every text field, analysed like the documents,
    /// with rare terms also matching their likely correct spelling.
    pub fn search(&self, text: &str, limit: usize) -> Vec<SearchHit> {
        let weights = self.corrected_terms(text);
        self.search_terms(weights.iter().map(|(t, w)| (t.as_str(), *w)), limit)
    }

    fn corrected_terms(&self, text: &str) -> Vec<(String, f32)> {
        self.query_terms(text, true, true)
    }

    /// Weighted terms of a query, without stop words and corrections to them unless that is all
    /// the query has, phrases included.
    fn query_terms(&self, text: &str, typos: bool, alone: bool) -> Vec<(String, f32)> {
        let (mut kept, mut all) = (Vec::new(), Vec::new());
        self.analyzer.analyze_words(text, |word, term| {
            let spellings = if typos {
                self.spellings(term)
            } else {
                vec![(term.to_owned(), 1.0)]
            };
            if !self.stops.words.contains(word) {
                kept.extend(
                    spellings
                        .iter()
                        .filter(|(s, _)| s == term || !self.stops.terms.contains(s))
                        .cloned(),
                );
            }
            all.extend(spellings);
        });
        let chosen = if kept.is_empty() && alone { all } else { kept };
        let mut weights: Vec<(String, f32)> = Vec::new();
        for (word, weight) in chosen {
            match weights.iter_mut().find(|w| w.0 == word) {
                Some(w) if typos => w.1 = w.1.max(weight),
                Some(w) => w.1 += weight,
                None => weights.push((word, weight)),
            }
        }
        weights
    }

    /// [`Reader::search`] without spelling corrections.
    pub fn search_exact(&self, text: &str, limit: usize) -> Vec<SearchHit> {
        let weights = self.exact_terms(text);
        self.search_terms(weights.iter().map(|(t, w)| (t.as_str(), *w)), limit)
    }

    fn exact_terms(&self, text: &str) -> Vec<(String, f32)> {
        self.query_terms(text, false, true)
    }

    /// The best documents for a query over named fields with boosts, within its filter.
    ///
    /// A query without terms returns the first documents the filter allows, with score 0.
    /// Without positions, phrases are searched as plain words; [`Reader::try_query`] refuses them.
    pub fn query(&self, query: &Query) -> Vec<SearchHit> {
        self.run_query(query, false)
    }

    /// [`Reader::query`], failing when it has a phrase and a segment keeps no positions.
    pub fn try_query(&self, query: &Query) -> io::Result<Vec<SearchHit>> {
        if !self.has_positions() {
            let (_, mut phrases) = crate::query::split_phrases(&query.text);
            phrases.extend(query.phrases.iter().cloned());
            if phrases.iter().any(|p| self.analyzer.terms(&p.0).len() > 1) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "the index was written without positions; cannot run a phrase query",
                ));
            }
        }
        Ok(self.query(query))
    }

    /// Whether every segment keeps positions, so phrases can match anywhere.
    pub fn has_positions(&self) -> bool {
        self.segments.iter().all(|s| s.0.has_positions())
    }

    /// The query's text outside quotes, and its phrases analysed, quoted ones first.
    /// Without positions every phrase joins the plain text instead.
    fn phrases(&self, query: &Query) -> (String, Vec<(Vec<String>, u32)>) {
        let (mut text, mut phrases) = crate::query::split_phrases(&query.text);
        phrases.extend(query.phrases.iter().cloned());
        if !self.has_positions() {
            for (phrase, _) in phrases.drain(..) {
                text.push(' ');
                text.push_str(&phrase);
            }
        }
        let analysed = phrases
            .into_iter()
            .map(|(text, slop)| (self.analyzer.terms(&text), slop))
            .filter(|p| !p.0.is_empty())
            .collect();
        (text, analysed)
    }

    /// [`Reader::query`] scoring every posting: the reference pruning must equal.
    #[doc(hidden)]
    pub fn query_exhaustive(&self, query: &Query) -> Vec<SearchHit> {
        self.run_query(query, true)
    }

    fn run_query(&self, query: &Query, exhaustive: bool) -> Vec<SearchHit> {
        let (text, phrases) = self.phrases(query);
        let weights = self.query_terms(&text, query.typos, phrases.is_empty());
        if weights.is_empty() && phrases.is_empty() {
            return self.placeholder(query.filter.as_ref(), query.limit);
        }
        let fields: Vec<(&str, f32)> = if query.fields.is_empty() {
            self.all_fields()
        } else {
            query.fields.iter().map(|(f, b)| (f.as_str(), *b)).collect()
        };
        let weights: Vec<(&str, f32)> = weights.iter().map(|(t, w)| (t.as_str(), *w)).collect();
        let terms = expand(&weights, &fields);
        let phrases: Vec<PhraseClause> = fields
            .iter()
            .flat_map(|&(f, boost)| {
                phrases
                    .iter()
                    .map(move |(words, slop)| (f, words.as_slice(), *slop, boost))
            })
            .collect();
        self.run(
            &terms,
            &phrases,
            query.filter.as_ref(),
            query.limit,
            exhaustive,
        )
    }

    fn all_fields(&self) -> Vec<(&str, f32)> {
        self.fields.iter().map(|f| (f.as_str(), 1.0)).collect()
    }

    /// The first `limit` live documents `filter` allows, oldest segment first.
    fn placeholder(&self, filter: Option<&Filter>, limit: usize) -> Vec<SearchHit> {
        let mut hits = Vec::new();
        for (segment, deletes) in &self.segments {
            let allowed = filter.map(|f| f.allowed(segment));
            for doc in 0..segment.len() as u32 {
                if hits.len() >= limit {
                    return hits;
                }
                let out = allowed
                    .as_ref()
                    .is_some_and(|a| a[doc as usize / 64] >> (doc % 64) & 1 == 0);
                if !out && !is_deleted(deletes, doc) {
                    hits.push(SearchHit {
                        id: segment.id(doc).to_owned(),
                        score: 0.0,
                    });
                }
            }
        }
        hits
    }

    /// The stored payload of a live document.
    pub fn document(&self, id: &str) -> Option<Vec<u8>> {
        let (i, doc) = self.locate(id)?;
        self.segments[i].0.stored(doc)
    }

    /// Stored payloads of `hits`, in order, decompressing each block once.
    pub fn documents(&self, hits: &[SearchHit]) -> Vec<Option<Vec<u8>>> {
        let mut blocks: FxHashMap<(usize, usize), Vec<u8>> = FxHashMap::default();
        hits.iter()
            .map(|hit| {
                let (i, doc) = self.locate(&hit.id)?;
                let segment = &self.segments[i].0;
                let (block, first) = segment.stored_block(doc)?;
                let bytes = blocks
                    .entry((i, block))
                    .or_insert_with(|| segment.stored_bytes(block));
                crate::stored::extract(bytes, (doc - first) as usize)
            })
            .collect()
    }

    /// The segment and document holding the live document `id`.
    fn locate(&self, id: &str) -> Option<(usize, u32)> {
        self.segments
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, (segment, deletes))| {
                let doc = segment.doc(id)?;
                (!is_deleted(deletes, doc)).then_some((i, doc))
            })
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

    /// The best `limit` documents for weighted terms in every text field, scored with BM25 over all segments.
    pub fn search_terms<'a>(
        &self,
        terms: impl IntoIterator<Item = (&'a str, f32)>,
        limit: usize,
    ) -> Vec<SearchHit> {
        let terms: Vec<(&str, f32)> = terms.into_iter().collect();
        self.run(&expand(&terms, &self.all_fields()), &[], None, limit, false)
    }

    /// [`Reader::search_terms`] scoring every posting: the reference pruning must equal.
    #[doc(hidden)]
    pub fn search_terms_exhaustive<'a>(
        &self,
        terms: impl IntoIterator<Item = (&'a str, f32)>,
        limit: usize,
    ) -> Vec<SearchHit> {
        let terms: Vec<(&str, f32)> = terms.into_iter().collect();
        self.run(&expand(&terms, &self.all_fields()), &[], None, limit, true)
    }

    /// Documents with `term` in field `f`, over all segments.
    fn field_df(&self, f: &str, term: &str) -> u64 {
        self.segments
            .iter()
            .filter_map(|(s, _)| Some(s.field_document_frequency(s.text_field(f)?, term)))
            .map(u64::from)
            .sum()
    }

    /// Top `limit` for `(field, term, weight)` and phrase clauses within `filter`.
    fn run(
        &self,
        terms: &[(&str, &str, f32)],
        phrases: &[PhraseClause],
        filter: Option<&Filter>,
        limit: usize,
        exhaustive: bool,
    ) -> Vec<SearchHit> {
        let dfs: Vec<u64> = terms.iter().map(|&(f, t, _)| self.field_df(f, t)).collect();
        let phrase_dfs: Vec<Vec<u64>> = phrases
            .iter()
            .map(|&(f, words, _, _)| words.iter().map(|w| self.field_df(f, w)).collect())
            .collect();
        let mut top = TopK::new(limit);
        let mut order: Vec<usize> = (0..self.segments.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(self.segments[i].0.len()));
        for i in order {
            let (segment, deletes) = &self.segments[i];
            let excluded = match filter {
                Some(filter) => match self.excluded(i, filter) {
                    Some(bits) => Some(bits),
                    None => continue,
                },
                None => None,
            };
            let deleted = excluded.as_deref().or(deletes.as_deref());
            let mut searcher = Searcher::within(segment, self.collections[i].clone(), deleted);
            let mut query =
                searcher.prepare_with(terms.iter().zip(&dfs).filter_map(|(&(f, t, w), &df)| {
                    Some((segment.text_field(f)?, t, w, Some(df)))
                }));
            for (&(f, words, slop, w), dfs) in phrases.iter().zip(&phrase_dfs) {
                let Some(field) = segment.text_field(f) else {
                    continue;
                };
                let words: Vec<&str> = words.iter().map(String::as_str).collect();
                query.extend(searcher.prepare_phrase(field, &words, slop, w, Some(dfs)));
            }
            if exhaustive {
                searcher.collect_exhaustive(&query, &mut top, (i as u64) << 32);
            } else {
                searcher.collect(&query, &mut top, (i as u64) << 32);
            }
        }
        top.into_sorted().into_iter().map(|r| self.hit(r)).collect()
    }

    /// Documents of segment `i` that `filter` or a deletion rules out; `None` when that is all.
    fn excluded(&self, i: usize, filter: &Filter) -> Option<Vec<u64>> {
        let (segment, deletes) = &self.segments[i];
        let mut bits = filter.allowed(segment);
        if let Some(deletes) = deletes {
            bits.iter_mut().zip(deletes).for_each(|(w, d)| *w &= !d);
        }
        if bits.iter().all(|&w| w == 0) {
            return None;
        }
        bits.iter_mut().for_each(|w| *w = !*w);
        Some(bits)
    }

    fn hit(&self, ranked: Ranked) -> SearchHit {
        let segment = &self.segments[(ranked.position >> 32) as usize].0;
        SearchHit {
            id: segment.id(ranked.position as u32).to_owned(),
            score: ranked.score,
        }
    }
}

/// Every term in every field, weighted by both.
fn expand<'a>(terms: &[(&'a str, f32)], fields: &[(&'a str, f32)]) -> Vec<(&'a str, &'a str, f32)> {
    fields
        .iter()
        .flat_map(|&(f, boost)| terms.iter().map(move |&(t, w)| (f, t, w * boost)))
        .collect()
}
