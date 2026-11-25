use std::cmp::Ordering;
use std::collections::BinaryHeap;

use crate::format::*;
use crate::phrase::{self, MAX_WORDS};
use crate::positions::{self, Blocks, Located};
use crate::segment::{Segment, TermInfo};

/// Most terms a pruned query takes; longer queries are scored exhaustively.
const MAX_TERMS: usize = 64;
const BUCKETS: usize = 1024;
/// Windows scored best first before the rest are swept in document order.
const EAGER: usize = 128;
/// Marks a scored window's bound, below any threshold.
const DONE: f32 = -1.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub doc: u32,
    pub score: f32,
}

/// A hit across segments: `position` orders segments, then documents within them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Ranked {
    pub score: f32,
    pub position: u64,
}

impl Eq for Ranked {}

/// Worst first, so a max-heap's top is the one to evict.
impl Ord for Ranked {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .score
            .total_cmp(&self.score)
            .then(self.position.cmp(&other.position))
    }
}

impl PartialOrd for Ranked {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub(crate) struct TopK {
    heap: BinaryHeap<Ranked>,
    limit: usize,
}

impl TopK {
    pub fn new(limit: usize) -> Self {
        Self {
            heap: BinaryHeap::with_capacity(limit.min(4096) + 1),
            limit,
        }
    }

    fn threshold(&self) -> f32 {
        if self.heap.len() == self.limit {
            self.heap.peek().unwrap().score
        } else {
            0.0
        }
    }

    fn offer(&mut self, hit: Ranked) {
        if self.heap.len() < self.limit {
            self.heap.push(hit);
        } else if hit < *self.heap.peek().unwrap() {
            *self.heap.peek_mut().unwrap() = hit;
        }
    }

    pub fn into_sorted(self) -> Vec<Ranked> {
        self.heap.into_sorted_vec()
    }

    fn into_hits(self) -> Vec<Hit> {
        self.into_sorted()
            .into_iter()
            .map(|r| Hit {
                doc: r.position as u32,
                score: r.score,
            })
            .collect()
    }
}

pub(crate) struct QueryTerm {
    /// A phrase's first word.
    info: TermInfo,
    numerator: f32,
    field: u8,
    phrase: Option<Box<Phrase>>,
}

/// A phrase's distinct words with their positions, which word stands at each place, and its slop.
pub(crate) struct Phrase {
    members: Vec<(TermInfo, Located)>,
    order: Vec<u8>,
    slop: u32,
}

/// One phrase word's postings in a window, with each document's first position in the block.
#[derive(Clone, Copy)]
struct MemberWindow<'a> {
    mask: u128,
    tfs: Frequencies<'a>,
    starts: [u32; WINDOW + 1],
    block: (usize, u8),
}

/// Space a window's phrase scoring reuses.
#[derive(Default)]
struct PhraseScratch<'a> {
    members: Vec<MemberWindow<'a>>,
    lists: Vec<Vec<u32>>,
    freqs: Vec<[f32; WINDOW]>,
}

/// Statistics of one text field, possibly spanning several segments.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FieldStats {
    /// Documents with the field.
    pub docs: u64,
    pub tokens: u64,
}

/// Collection statistics BM25 scores against, for each text field of one segment.
#[derive(Clone, Debug)]
pub(crate) struct Collection {
    pub fields: Vec<FieldStats>,
}

impl Collection {
    pub fn of(segment: &Segment) -> Self {
        let fields = segment
            .text_fields()
            .iter()
            .map(|f| FieldStats {
                docs: u64::from(f.docs),
                tokens: f.tokens,
            })
            .collect();
        Self { fields }
    }
}

impl FieldStats {
    fn average(&self) -> f32 {
        if self.docs == 0 {
            1.0
        } else {
            (self.tokens as f64 / self.docs as f64).max(f64::MIN_POSITIVE) as f32
        }
    }

    pub fn idf(&self, df: u64) -> f32 {
        let (docs, df) = (self.docs as f64, df as f64);
        ((docs - df + 0.5) / (df + 0.5)).ln_1p() as f32
    }
}

#[inline(always)]
fn saturation(tf: u32, length: f32) -> f32 {
    let tf = tf as f32;
    tf / (tf + length)
}

/// Term frequencies up to this read their saturation from a table instead of dividing.
const TABLE_TF: usize = 16;

/// `tf / (tf + length)` for each small `tf` and norm byte, as [`saturation`] computes it.
fn saturation_table(lengths: &[f32; 256]) -> Box<[[f32; 256]; TABLE_TF + 1]> {
    Box::new(std::array::from_fn(|tf| {
        std::array::from_fn(|n| saturation(tf as u32, lengths[n]))
    }))
}

/// Length normalisation, saturations and stored-bound decoding for one text field.
struct Scoring {
    lengths: [f32; 256],
    saturations: Box<[[f32; 256]; TABLE_TF + 1]>,
    stored: [f32; 256],
}

impl Scoring {
    fn new(segment: FieldStats, collection: FieldStats) -> Box<Self> {
        let lengths = length_table(collection.average());
        Box::new(Self {
            saturations: saturation_table(&lengths),
            lengths,
            stored: stored_bounds(segment.average(), collection.average()),
        })
    }

    #[inline(always)]
    fn saturation(&self, tf: u32, norm: u8) -> f32 {
        match self.saturations.get(tf as usize) {
            Some(row) => row[usize::from(norm)],
            None => saturation(tf, self.lengths[usize::from(norm)]),
        }
    }

    /// Saturation of a phrase frequency, which may be fractional.
    #[inline(always)]
    fn phrase_saturation(&self, freq: f32, norm: u8) -> f32 {
        freq / (freq + self.lengths[usize::from(norm)])
    }
}

/// A window's saturation bound for a term: stored, or exact for a single posting.
#[inline(always)]
fn saturation_bound(window: &Window, norms: &[u8], scoring: &Scoring) -> f32 {
    if window.count == 1 {
        let doc = window.id as usize * WINDOW + usize::from(window.single.0);
        saturation(window.single.1, scoring.lengths[usize::from(norms[doc])])
    } else {
        scoring.stored[usize::from(window.bound)]
    }
}

/// A window's score bound for a term, or for a phrase from its words' windows: a phrase occurs
/// at most as often as its rarest word, so the smallest word bound holds.
#[inline(always)]
fn bound(numerator: f32, windows: &[Window], norms: &[u8], scoring: &Scoring) -> f32 {
    let saturation = windows
        .iter()
        .map(|w| saturation_bound(w, norms, scoring))
        .fold(f32::INFINITY, f32::min);
    numerator * saturation
}

/// Saturation bounds for each stored byte, valid under the collection's average length.
///
/// Bytes bound `tf / (tf + K)` under the segment's average; a different average scales `K` by at least `c`.
fn stored_bounds(segment_average: f32, average: f32) -> [f32; 256] {
    let c = (segment_average / average).min(1.0);
    std::array::from_fn(|q| {
        let s = q as f32 / 255.0;
        if c >= 1.0 || s >= 1.0 {
            s
        } else if s <= 0.0 {
            0.0
        } else {
            (1.0 / (1.0 + c * (1.0 / s - 1.0))).next_up()
        }
    })
}

/// Work done by the last query, for tuning.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub windows_touched: usize,
    pub windows_scored: usize,
    pub headers_read: usize,
    pub postings_scored: usize,
    pub scan_micros: u64,
    pub score_micros: u64,
}

/// Runs queries against one segment, reusing its scratch space between them.
pub struct Searcher<'s> {
    segment: &'s Segment,
    collection: Collection,
    deleted: Option<&'s [u64]>,
    /// Per text field of the segment, built for the fields a query uses.
    scoring: Vec<Option<Box<Scoring>>>,
    bounds: Vec<f32>,
    present: Vec<u64>,
    touched: Vec<u32>,
    /// Each term's windows; a phrase keeps one per word for every window all its words share.
    lists: Vec<Vec<Window>>,
    /// Position blocks matching `lists`, for phrases.
    blocks: Vec<Vec<(usize, u8)>>,
    /// `slots[t * windows + w]`: where window `w` sits in `lists[t]`, valid while `present` says so.
    slots: Vec<u32>,
    order: Vec<u32>,
    counts: Vec<u32>,
    stats: Stats,
}

impl<'s> Searcher<'s> {
    pub fn new(segment: &'s Segment) -> Self {
        Self::within(segment, Collection::of(segment), None)
    }

    /// Searches `segment` as part of `collection`, skipping documents set in `deleted`.
    pub(crate) fn within(
        segment: &'s Segment,
        collection: Collection,
        deleted: Option<&'s [u64]>,
    ) -> Self {
        let windows = segment.windows();
        Self {
            segment,
            scoring: (0..collection.fields.len()).map(|_| None).collect(),
            collection,
            deleted,
            bounds: vec![0.0; windows],
            present: vec![0; windows],
            touched: Vec::new(),
            lists: Vec::new(),
            blocks: Vec::new(),
            slots: Vec::new(),
            order: Vec::new(),
            counts: vec![0; BUCKETS + 1],
            stats: Stats::default(),
        }
    }

    /// Each weighted term in every text field.
    fn prepare<'a>(&mut self, terms: impl IntoIterator<Item = (&'a str, f32)>) -> Vec<QueryTerm> {
        let fields = self.segment.text_fields().len() as u8;
        let terms: Vec<_> = terms.into_iter().collect();
        let terms = (0..fields).flat_map(|f| terms.iter().map(move |&(t, w)| (f, t, w, None)));
        self.prepare_with(terms.collect::<Vec<_>>())
    }

    fn prepare_fields<'a>(
        &mut self,
        terms: impl IntoIterator<Item = (&'a str, &'a str, f32)>,
    ) -> Vec<QueryTerm> {
        let segment = self.segment;
        let terms = terms
            .into_iter()
            .filter_map(|(field, t, w)| Some((segment.text_field(field)?, t, w, None)));
        self.prepare_with(terms)
    }

    /// Query terms as `(field, term, weight, df)`; `df` overrides the segment's document frequency.
    pub(crate) fn prepare_with<'a>(
        &mut self,
        terms: impl IntoIterator<Item = (u8, &'a str, f32, Option<u64>)>,
    ) -> Vec<QueryTerm> {
        let own = Collection::of(self.segment);
        terms
            .into_iter()
            .filter(|&(_, _, weight, _)| weight > 0.0 && weight <= 1e30)
            .filter_map(|(field, term, weight, df)| {
                let info = self.segment.term(field, term)?;
                let f = usize::from(field);
                let stats = self.collection.fields[f];
                self.scoring[f].get_or_insert_with(|| Scoring::new(own.fields[f], stats));
                let idf = stats.idf(df.unwrap_or(u64::from(info.df)));
                Some(QueryTerm {
                    info,
                    numerator: weight * idf * (K1 + 1.0),
                    field,
                    phrase: None,
                })
            })
            .collect()
    }

    /// A phrase in one field, scored with its words' summed IDFs (`dfs` override theirs); `None`
    /// when a word is missing from the field or the segment keeps no positions.
    pub(crate) fn prepare_phrase(
        &mut self,
        field: u8,
        words: &[&str],
        slop: u32,
        weight: f32,
        dfs: Option<&[u64]>,
    ) -> Option<QueryTerm> {
        let words = &words[..words.len().min(MAX_WORDS)];
        if let [word] = words {
            let df = dfs.map(|d| d[0]);
            return self.prepare_with([(field, *word, weight, df)]).pop();
        }
        if words.is_empty() || !(weight > 0.0 && weight <= 1e30) {
            return None;
        }
        let segment = self.segment;
        let mut distinct: Vec<&str> = Vec::new();
        let mut members = Vec::new();
        let mut order = Vec::with_capacity(words.len());
        let f = usize::from(field);
        let stats = *self.collection.fields.get(f)?;
        let mut idf = 0.0;
        for (i, &word) in words.iter().enumerate() {
            let info = segment.term(field, word)?;
            idf += stats.idf(dfs.map_or(u64::from(info.df), |d| d[i]));
            let member = match distinct.iter().position(|&d| d == word) {
                Some(m) => m,
                None => {
                    members.push((info, segment.term_positions(field, word)?));
                    distinct.push(word);
                    distinct.len() - 1
                }
            };
            order.push(member as u8);
        }
        let own = Collection::of(segment);
        self.scoring[f].get_or_insert_with(|| Scoring::new(own.fields[f], stats));
        Some(QueryTerm {
            info: members[0].0,
            numerator: weight * idf * (K1 + 1.0),
            field,
            phrase: Some(Box::new(Phrase {
                members,
                order,
                slop,
            })),
        })
    }

    /// Weighted terms and phrases `(words, slop, weight)` in every text field.
    fn prepare_phrases<'a>(
        &mut self,
        terms: impl IntoIterator<Item = (&'a str, f32)>,
        phrases: &[(&[&str], u32, f32)],
    ) -> Vec<QueryTerm> {
        let mut query = self.prepare(terms);
        for field in 0..self.segment.text_fields().len() as u8 {
            for &(words, slop, weight) in phrases {
                query.extend(self.prepare_phrase(field, words, slop, weight, None));
            }
        }
        query
    }

    #[inline(always)]
    fn scoring(&self, field: u8) -> &Scoring {
        self.scoring[usize::from(field)]
            .as_deref()
            .expect("prepared fields have scoring")
    }

    /// Exact BM25 top `limit` for weighted terms in every text field, best first, ties by document order.
    pub fn search<'a>(
        &mut self,
        terms: impl IntoIterator<Item = (&'a str, f32)>,
        limit: usize,
    ) -> Vec<Hit> {
        let query = self.prepare(terms);
        let mut top = TopK::new(limit);
        self.collect(&query, &mut top, 0);
        top.into_hits()
    }

    /// Exact BM25 top `limit` for `(field, term, weight)`, summed over fields.
    pub fn search_fields<'a>(
        &mut self,
        terms: impl IntoIterator<Item = (&'a str, &'a str, f32)>,
        limit: usize,
    ) -> Vec<Hit> {
        let query = self.prepare_fields(terms);
        let mut top = TopK::new(limit);
        self.collect(&query, &mut top, 0);
        top.into_hits()
    }

    /// Exact BM25 top `limit` for weighted terms and phrases `(words, slop, weight)` in every text
    /// field; phrases match nothing in a segment without positions.
    pub fn search_phrases<'a>(
        &mut self,
        terms: impl IntoIterator<Item = (&'a str, f32)>,
        phrases: &[(&[&str], u32, f32)],
        limit: usize,
    ) -> Vec<Hit> {
        let query = self.prepare_phrases(terms, phrases);
        let mut top = TopK::new(limit);
        self.collect(&query, &mut top, 0);
        top.into_hits()
    }

    /// [`Searcher::search_phrases`] scoring every match.
    pub fn search_phrases_exhaustive<'a>(
        &mut self,
        terms: impl IntoIterator<Item = (&'a str, f32)>,
        phrases: &[(&[&str], u32, f32)],
        limit: usize,
    ) -> Vec<Hit> {
        let query = self.prepare_phrases(terms, phrases);
        self.exhaustive(&query, limit)
    }

    /// Adds this segment's matches to `top`, at positions `base + doc`.
    pub(crate) fn collect(&mut self, query: &[QueryTerm], top: &mut TopK, base: u64) {
        self.stats = Stats::default();
        if query.is_empty() || top.limit == 0 {
            return;
        }
        if query.len() > MAX_TERMS {
            return self.exhaustive_into(query, top, base);
        }
        let guard = 1.0 + 8.0 * (query.len() as f32 + 2.0) * f32::EPSILON;
        let start = std::time::Instant::now();
        self.scan(query);
        self.stats.scan_micros = start.elapsed().as_micros() as u64;
        self.stats.windows_touched = self.touched.len();
        let start = std::time::Instant::now();
        self.best_first(query, top, base, guard);
        self.stats.score_micros = start.elapsed().as_micros() as u64;
        for &w in &self.touched {
            self.bounds[w as usize] = 0.0;
            self.present[w as usize] = 0;
        }
        self.touched.clear();
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Sums each window's score bound over the query terms and keeps their headers.
    fn scan(&mut self, query: &[QueryTerm]) {
        if self.lists.len() < query.len() {
            self.lists.resize_with(query.len(), Vec::new);
            self.blocks.resize_with(query.len(), Vec::new);
        }
        let windows = self.segment.windows();
        if self.slots.len() < query.len() * windows {
            self.slots.resize(query.len() * windows, 0);
        }
        for (t, term) in query.iter().enumerate() {
            if let Some(phrase) = &term.phrase {
                self.scan_phrase(t, term, phrase);
                continue;
            }
            let list = &mut self.lists[t];
            list.clear();
            list.reserve(term.info.windows as usize);
            self.stats.headers_read += term.info.windows as usize;
            let norms = self.segment.norms(term.field);
            let scoring = self.scoring[usize::from(term.field)]
                .as_deref()
                .expect("prepared fields have scoring");
            for window in self.segment.headers(&term.info) {
                let w = window.id as usize;
                if self.deleted.is_some_and(|d| window_bits(d, w) == u128::MAX) {
                    continue;
                }
                if self.present[w] == 0 {
                    self.touched.push(window.id);
                }
                self.present[w] |= 1 << t;
                self.slots[t * windows + w] = list.len() as u32;
                self.bounds[w] += bound(term.numerator, &[window], norms, scoring);
                list.push(window);
            }
        }
    }

    /// Keeps the windows every word of a phrase shares, with their position blocks.
    fn scan_phrase(&mut self, t: usize, term: &QueryTerm, phrase: &Phrase) {
        let segment = self.segment;
        let section = segment.positions_section();
        let (list, blocks) = (&mut self.lists[t], &mut self.blocks[t]);
        list.clear();
        blocks.clear();
        let m = phrase.members.len();
        let windows = segment.windows();
        let norms = segment.norms(term.field);
        let scoring = self.scoring[usize::from(term.field)]
            .as_deref()
            .expect("prepared fields have scoring");
        let mut cursors: Vec<_> = phrase
            .members
            .iter()
            .map(|(info, located)| {
                self.stats.headers_read += info.windows as usize;
                segment.headers(info).zip(Blocks::new(section, *located))
            })
            .collect();
        let mut current = Vec::with_capacity(m);
        for cursor in &mut cursors {
            match cursor.next() {
                Some(c) => current.push(c),
                None => return,
            }
        }
        let mut shared = Vec::with_capacity(m);
        loop {
            let target = current.iter().map(|c| c.0.id).max().unwrap();
            let mut aligned = true;
            for (cursor, c) in cursors.iter_mut().zip(current.iter_mut()) {
                while c.0.id < target {
                    match cursor.next() {
                        Some(next) => *c = next,
                        None => return,
                    }
                }
                aligned &= c.0.id == target;
            }
            if !aligned {
                continue;
            }
            let w = target as usize;
            if !self.deleted.is_some_and(|d| window_bits(d, w) == u128::MAX) {
                if self.present[w] == 0 {
                    self.touched.push(target);
                }
                self.present[w] |= 1 << t;
                self.slots[t * windows + w] = (list.len() / m) as u32;
                shared.clear();
                shared.extend(current.iter().map(|c| c.0));
                self.bounds[w] += bound(term.numerator, &shared, norms, scoring);
                list.extend_from_slice(&shared);
                blocks.extend(current.iter().map(|c| c.1));
            }
            for (cursor, c) in cursors.iter_mut().zip(current.iter_mut()) {
                match cursor.next() {
                    Some(next) => *c = next,
                    None => return,
                }
            }
        }
    }

    /// Scores windows in descending bound order until no remaining window can enter the top k.
    fn best_first(&mut self, query: &[QueryTerm], top: &mut TopK, base: u64, guard: f32) {
        let max = self
            .touched
            .iter()
            .map(|&w| self.bounds[w as usize])
            .fold(0.0, f32::max);
        let scale = (BUCKETS - 1) as f32 / max;
        let bucket = |bound: f32| ((bound * scale) as usize).min(BUCKETS - 1);
        self.counts.iter_mut().for_each(|c| *c = 0);
        for &w in &self.touched {
            self.counts[bucket(self.bounds[w as usize]) + 1] += 1;
        }
        for b in 0..BUCKETS {
            self.counts[b + 1] += self.counts[b];
        }
        self.order.resize(self.touched.len(), 0);
        let mut next = self.counts.clone();
        for &w in &self.touched {
            let b = bucket(self.bounds[w as usize]);
            self.order[next[b] as usize] = w;
            next[b] += 1;
        }
        let mut local = Vec::with_capacity(query.len());
        let mut scratch = PhraseScratch::default();
        'buckets: for b in (0..BUCKETS).rev() {
            let ceiling = (b + 1) as f32 / scale * (1.0 + 1e-5);
            if ceiling * guard < top.threshold() {
                return;
            }
            for i in self.counts[b]..self.counts[b + 1] {
                if self.stats.windows_scored == EAGER {
                    break 'buckets;
                }
                let w = self.order[i as usize];
                if self.bounds[w as usize] * guard < top.threshold() {
                    continue;
                }
                self.stats.windows_scored += 1;
                self.stats.postings_scored +=
                    self.window(query, w, guard, top, base, &mut local, &mut scratch);
                self.bounds[w as usize] = DONE;
            }
        }
        // The best windows set the threshold; the rest go in document order, which reads memory in sequence.
        self.order.clear();
        self.order.extend(
            self.touched
                .iter()
                .copied()
                .filter(|&w| self.bounds[w as usize] * guard >= top.threshold()),
        );
        self.order.sort_unstable();
        for i in 0..self.order.len() {
            let w = self.order[i];
            if self.bounds[w as usize] * guard < top.threshold() {
                continue;
            }
            self.stats.windows_scored += 1;
            self.stats.postings_scored +=
                self.window(query, w, guard, top, base, &mut local, &mut scratch);
        }
    }

    /// The windows of term `t` in window `w`: one, or one per word of a phrase.
    #[inline(always)]
    fn windows_of(&self, query: &[QueryTerm], t: usize, w: u32) -> (&[Window], usize) {
        let m = query[t].phrase.as_ref().map_or(1, |p| p.members.len());
        let slot = self.slots[t * self.segment.windows() + w as usize] as usize;
        (&self.lists[t][slot * m..(slot + 1) * m], slot * m)
    }

    /// Frequency of phrase `t` in the document at `off`, from its words' windows in `members`.
    fn phrase_frequency(
        &self,
        phrase: &Phrase,
        members: &[MemberWindow],
        lists: &mut [Vec<u32>],
        off: usize,
    ) -> f32 {
        let section = self.segment.positions_section();
        for (member, list) in members.iter().zip(lists.iter_mut()) {
            let rank = (member.mask & ((1_u128 << off) - 1)).count_ones() as usize;
            let count = positions::kept(member.tfs.get(rank));
            let first = member.starts[rank] as usize;
            positions::read(section, member.block, first, count, list);
        }
        phrase::frequency(lists, &phrase.order, phrase.slop)
    }

    #[allow(clippy::too_many_arguments)]
    fn window(
        &self,
        query: &[QueryTerm],
        w: u32,
        guard: f32,
        top: &mut TopK,
        segment_base: u64,
        local: &mut Vec<(usize, f32, u128, Window)>,
        scratch: &mut PhraseScratch<'s>,
    ) -> usize {
        let mut scored = 0;
        let base = w as usize * WINDOW;
        let live = self
            .deleted
            .map_or(u128::MAX, |d| !window_bits(d, w as usize));
        let end = (base + WINDOW).min(self.segment.len());
        local.clear();
        let mut bits = self.present[w as usize];
        while bits != 0 {
            let t = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let (windows, _) = self.windows_of(query, t, w);
            let field = query[t].field;
            let bound = bound(
                query[t].numerator,
                windows,
                self.segment.norms(field),
                self.scoring(field),
            );
            local.push((t, bound, 0, windows[0]));
        }
        // Terms whose bounds together cannot reach the threshold only score documents others found.
        let threshold = top.threshold();
        let mut essential = u64::MAX;
        if threshold > 0.0 {
            local.sort_unstable_by(|a, b| a.1.total_cmp(&b.1));
            let mut optional = 0.0;
            for entry in local.iter() {
                if (optional + entry.1) * guard >= threshold {
                    break;
                }
                optional += entry.1;
                essential &= !(1 << entry.0);
            }
            local.sort_unstable_by_key(|entry| entry.0);
        }
        let mut candidates = 0_u128;
        let has_phrases = local.iter().any(|e| query[e.0].phrase.is_some());
        if has_phrases {
            scratch.members.clear();
            if scratch.freqs.len() < local.len() {
                scratch.freqs.resize(local.len(), [0.0; WINDOW]);
            }
        }
        // A phrase's entry keeps where its words start in `scratch.members` in place of a window.
        let mut starts = [0_usize; 64];
        for (k, entry) in local.iter_mut().enumerate() {
            let term = &query[entry.0];
            let Some(phrase) = &term.phrase else {
                let bodies = self.segment.bodies(&term.info);
                entry.2 = entry.3.decode(bodies).0;
                if essential & (1 << entry.0) != 0 {
                    candidates |= entry.2;
                }
                continue;
            };
            let (windows, at) = self.windows_of(query, entry.0, w);
            starts[k] = scratch.members.len();
            let mut maybe = live;
            for (j, window) in windows.iter().enumerate() {
                let segment: &'s Segment = self.segment;
                let (mask, tfs) = window.decode(segment.bodies(&phrase.members[j].0));
                let mut member = MemberWindow {
                    mask,
                    tfs,
                    starts: [0; WINDOW + 1],
                    block: self.blocks[entry.0][at + j],
                };
                for rank in 0..mask.count_ones() as usize {
                    member.starts[rank + 1] =
                        member.starts[rank] + positions::kept(tfs.get(rank)) as u32;
                }
                scratch.members.push(member);
                maybe &= mask;
            }
            let m = windows.len();
            if scratch.lists.len() < m {
                scratch.lists.resize_with(m, Vec::new);
            }
            if essential & (1 << entry.0) == 0 {
                entry.2 = maybe;
                continue;
            }
            let members = &scratch.members[starts[k]..starts[k] + m];
            let mut found = 0_u128;
            while maybe != 0 {
                let off = maybe.trailing_zeros() as usize;
                maybe &= maybe - 1;
                let freq = self.phrase_frequency(phrase, members, &mut scratch.lists, off);
                if freq > 0.0 {
                    found |= 1 << off;
                    scratch.freqs[k][off] = freq;
                }
            }
            entry.2 = found;
            candidates |= found;
        }
        candidates &= live;
        let mut scores = [0.0_f32; WINDOW];
        for (k, &(t, _, mask, window)) in local.iter().enumerate() {
            let term = &query[t];
            let norms = &self.segment.norms(term.field)[base..end];
            let scoring = self.scoring(term.field);
            if let Some(phrase) = &term.phrase {
                if essential & (1 << t) != 0 {
                    let mut m = mask & candidates;
                    while m != 0 {
                        let off = m.trailing_zeros() as usize;
                        m &= m - 1;
                        let freq = scratch.freqs[k][off];
                        scores[off] += term.numerator * scoring.phrase_saturation(freq, norms[off]);
                        scored += 1;
                    }
                    continue;
                }
                let members = &scratch.members[starts[k]..starts[k] + phrase.members.len()];
                let mut m = mask & candidates;
                while m != 0 {
                    let off = m.trailing_zeros() as usize;
                    m &= m - 1;
                    let freq = self.phrase_frequency(phrase, members, &mut scratch.lists, off);
                    if freq > 0.0 {
                        scores[off] += term.numerator * scoring.phrase_saturation(freq, norms[off]);
                    }
                    scored += 1;
                }
                continue;
            }
            let (_, tfs) = window.decode(self.segment.bodies(&term.info));
            if essential & (1 << t) != 0 && live == u128::MAX {
                let (mut m, mut rank) = (mask, 0);
                while m != 0 {
                    let off = m.trailing_zeros() as usize;
                    m &= m - 1;
                    scores[off] += term.numerator * scoring.saturation(tfs.get(rank), norms[off]);
                    rank += 1;
                    scored += 1;
                }
            } else {
                let mut m = mask & candidates;
                while m != 0 {
                    let off = m.trailing_zeros() as usize;
                    m &= m - 1;
                    let rank = (mask & ((1_u128 << off) - 1)).count_ones() as usize;
                    scores[off] += term.numerator * scoring.saturation(tfs.get(rank), norms[off]);
                    scored += 1;
                }
            }
        }
        while candidates != 0 {
            let off = candidates.trailing_zeros() as usize;
            candidates &= candidates - 1;
            top.offer(Ranked {
                score: scores[off],
                position: segment_base + (base + off) as u64,
            });
        }
        scored
    }

    /// Scores every posting; the reference the pruned search must equal.
    pub fn search_exhaustive<'a>(
        &mut self,
        terms: impl IntoIterator<Item = (&'a str, f32)>,
        limit: usize,
    ) -> Vec<Hit> {
        let query = self.prepare(terms);
        self.exhaustive(&query, limit)
    }

    /// [`Searcher::search_fields`] scoring every posting.
    pub fn search_fields_exhaustive<'a>(
        &mut self,
        terms: impl IntoIterator<Item = (&'a str, &'a str, f32)>,
        limit: usize,
    ) -> Vec<Hit> {
        let query = self.prepare_fields(terms);
        self.exhaustive(&query, limit)
    }

    fn exhaustive(&self, query: &[QueryTerm], limit: usize) -> Vec<Hit> {
        let mut top = TopK::new(limit);
        self.exhaustive_into(query, &mut top, 0);
        top.into_hits()
    }

    pub(crate) fn collect_exhaustive(&self, query: &[QueryTerm], top: &mut TopK, base: u64) {
        self.exhaustive_into(query, top, base)
    }

    fn exhaustive_into(&self, query: &[QueryTerm], top: &mut TopK, base: u64) {
        if top.limit == 0 {
            return;
        }
        let mut scores = vec![0.0_f32; self.segment.len()];
        for term in query {
            let norms = self.segment.norms(term.field);
            let scoring = self.scoring(term.field);
            if let Some(phrase) = &term.phrase {
                self.exhaustive_phrase(term, phrase, &mut scores);
                continue;
            }
            let bodies = self.segment.bodies(&term.info);
            for window in self.segment.headers(&term.info) {
                let (mut mask, tfs) = window.decode(bodies);
                let start = window.id as usize * WINDOW;
                let mut rank = 0;
                while mask != 0 {
                    let doc = start + mask.trailing_zeros() as usize;
                    mask &= mask - 1;
                    scores[doc] += term.numerator * scoring.saturation(tfs.get(rank), norms[doc]);
                    rank += 1;
                }
            }
        }
        for (doc, &score) in scores.iter().enumerate() {
            let deleted = self
                .deleted
                .is_some_and(|d| d[doc / 64] >> (doc % 64) & 1 != 0);
            if score > 0.0 && !deleted {
                top.offer(Ranked {
                    score,
                    position: base + doc as u64,
                });
            }
        }
    }
}

/// A term's postings and all their positions.
type DecodedTerm = (Vec<(u32, u32)>, Vec<u32>);

impl Searcher<'_> {
    /// Adds a phrase's score to every document it occurs in, decoding whole lists.
    fn exhaustive_phrase(&self, term: &QueryTerm, phrase: &Phrase, scores: &mut [f32]) {
        let norms = self.segment.norms(term.field);
        let scoring = self.scoring(term.field);
        let members: Vec<DecodedTerm> = phrase
            .members
            .iter()
            .map(|(info, located)| {
                let postings = self.segment.postings(info).collect();
                (postings, self.segment.all_positions(info, *located))
            })
            .collect();
        let mut cursors = vec![(0_usize, 0_usize); members.len()];
        let mut lists = vec![Vec::new(); members.len()];
        for &(doc, _) in &members[0].0 {
            let mut all = true;
            for (j, (postings, positions)) in members.iter().enumerate() {
                let (at, first) = &mut cursors[j];
                while *at < postings.len() && postings[*at].0 < doc {
                    *first += positions::kept(postings[*at].1);
                    *at += 1;
                }
                if *at == postings.len() || postings[*at].0 != doc {
                    all = false;
                    continue;
                }
                let count = positions::kept(postings[*at].1);
                lists[j].clear();
                lists[j].extend_from_slice(&positions[*first..*first + count]);
            }
            if !all {
                continue;
            }
            let freq = phrase::frequency(&lists, &phrase.order, phrase.slop);
            if freq > 0.0 {
                let d = doc as usize;
                scores[d] += term.numerator * scoring.phrase_saturation(freq, norms[d]);
            }
        }
    }
}

/// The 128 bits of `bits` for window `w`.
fn window_bits(bits: &[u64], w: usize) -> u128 {
    let low = bits.get(2 * w).copied().unwrap_or(0);
    let high = bits.get(2 * w + 1).copied().unwrap_or(0);
    u128::from(low) | u128::from(high) << 64
}
