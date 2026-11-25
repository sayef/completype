use std::cmp::Ordering;
use std::collections::BinaryHeap;

use crate::format::*;
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
            heap: BinaryHeap::with_capacity(limit + 1),
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
    info: TermInfo,
    numerator: f32,
}

/// Collection statistics BM25 scores against, possibly spanning several segments.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Collection {
    pub docs: u64,
    pub tokens: u64,
}

impl Collection {
    pub fn of(segment: &Segment) -> Self {
        Self {
            docs: segment.len() as u64,
            tokens: segment.total_tokens(),
        }
    }

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

/// A window's score bound for a term: stored, or exact for a single posting.
#[inline(always)]
fn bound(
    numerator: f32,
    window: &Window,
    norms: &[u8],
    lengths: &[f32; 256],
    stored: &[f32; 256],
) -> f32 {
    if window.count == 1 {
        let doc = window.id as usize * WINDOW + usize::from(window.single.0);
        numerator * saturation(window.single.1, lengths[usize::from(norms[doc])])
    } else {
        numerator * stored[usize::from(window.bound)]
    }
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
    lengths: [f32; 256],
    saturations: Box<[[f32; 256]; TABLE_TF + 1]>,
    stored: [f32; 256],
    bounds: Vec<f32>,
    present: Vec<u64>,
    touched: Vec<u32>,
    lists: Vec<Vec<Window>>,
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
        let average = collection.average();
        let windows = segment.windows();
        Self {
            segment,
            collection,
            deleted,
            lengths: length_table(average),
            saturations: saturation_table(&length_table(average)),
            stored: stored_bounds(Collection::of(segment).average(), average),
            bounds: vec![0.0; windows],
            present: vec![0; windows],
            touched: Vec::new(),
            lists: Vec::new(),
            slots: Vec::new(),
            order: Vec::new(),
            counts: vec![0; BUCKETS + 1],
            stats: Stats::default(),
        }
    }

    fn prepare<'a>(&self, terms: impl IntoIterator<Item = (&'a str, f32)>) -> Vec<QueryTerm> {
        let terms = terms.into_iter().map(|(term, weight)| (term, weight, None));
        self.prepare_with(terms)
    }

    /// Query terms in this segment; `df` overrides the segment's document frequency.
    pub(crate) fn prepare_with<'a>(
        &self,
        terms: impl IntoIterator<Item = (&'a str, f32, Option<u64>)>,
    ) -> Vec<QueryTerm> {
        terms
            .into_iter()
            .filter(|&(_, weight, _)| weight > 0.0 && weight <= 1e30)
            .filter_map(|(term, weight, df)| {
                let info = self.segment.term(term)?;
                let idf = self.collection.idf(df.unwrap_or(u64::from(info.df)));
                Some(QueryTerm {
                    info,
                    numerator: weight * idf * (K1 + 1.0),
                })
            })
            .collect()
    }

    /// Exact BM25 top `limit` for weighted terms, best first, ties by document order.
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

    #[inline(always)]
    fn saturation(&self, tf: u32, norm: u8) -> f32 {
        match self.saturations.get(tf as usize) {
            Some(row) => row[usize::from(norm)],
            None => saturation(tf, self.lengths[usize::from(norm)]),
        }
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Sums each window's score bound over the query terms and keeps their headers.
    fn scan(&mut self, query: &[QueryTerm]) {
        if self.lists.len() < query.len() {
            self.lists.resize_with(query.len(), Vec::new);
        }
        let windows = self.segment.windows();
        if self.slots.len() < query.len() * windows {
            self.slots.resize(query.len() * windows, 0);
        }
        for (t, term) in query.iter().enumerate() {
            let list = &mut self.lists[t];
            list.clear();
            list.reserve(term.info.windows as usize);
            self.stats.headers_read += term.info.windows as usize;
            let norms = self.segment.norms();
            for window in self.segment.headers(&term.info) {
                let w = window.id as usize;
                if self.present[w] == 0 {
                    self.touched.push(window.id);
                }
                self.present[w] |= 1 << t;
                self.slots[t * windows + w] = list.len() as u32;
                self.bounds[w] +=
                    bound(term.numerator, &window, norms, &self.lengths, &self.stored);
                list.push(window);
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
                self.stats.postings_scored += self.window(query, w, guard, top, base, &mut local);
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
            self.stats.postings_scored += self.window(query, w, guard, top, base, &mut local);
        }
    }

    fn window(
        &self,
        query: &[QueryTerm],
        w: u32,
        guard: f32,
        top: &mut TopK,
        segment_base: u64,
        local: &mut Vec<(usize, f32, u128, Window)>,
    ) -> usize {
        let mut scored = 0;
        let base = w as usize * WINDOW;
        let live = self
            .deleted
            .map_or(u128::MAX, |d| !window_bits(d, w as usize));
        let norms = &self.segment.norms()[base..(base + WINDOW).min(self.segment.len())];
        local.clear();
        let mut bits = self.present[w as usize];
        while bits != 0 {
            let t = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let window =
                self.lists[t][self.slots[t * self.segment.windows() + w as usize] as usize];
            let bound = bound(
                query[t].numerator,
                &window,
                self.segment.norms(),
                &self.lengths,
                &self.stored,
            );
            local.push((t, bound, 0, window));
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
        for entry in local.iter_mut() {
            let bodies = self.segment.bodies(&query[entry.0].info);
            entry.2 = entry.3.decode(bodies).0;
            if essential & (1 << entry.0) != 0 {
                candidates |= entry.2;
            }
        }
        candidates &= live;
        let mut scores = [0.0_f32; WINDOW];
        for &(t, _, mask, window) in local.iter() {
            let term = &query[t];
            let (_, tfs) = window.decode(self.segment.bodies(&term.info));
            if essential & (1 << t) != 0 && live == u128::MAX {
                let (mut m, mut rank) = (mask, 0);
                while m != 0 {
                    let off = m.trailing_zeros() as usize;
                    m &= m - 1;
                    scores[off] += term.numerator * self.saturation(tfs.get(rank), norms[off]);
                    rank += 1;
                    scored += 1;
                }
            } else {
                let mut m = mask & candidates;
                while m != 0 {
                    let off = m.trailing_zeros() as usize;
                    m &= m - 1;
                    let rank = (mask & ((1_u128 << off) - 1)).count_ones() as usize;
                    scores[off] += term.numerator * self.saturation(tfs.get(rank), norms[off]);
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
        let norms = self.segment.norms();
        let mut scores = vec![0.0_f32; self.segment.len()];
        for term in query {
            let bodies = self.segment.bodies(&term.info);
            for window in self.segment.headers(&term.info) {
                let (mut mask, tfs) = window.decode(bodies);
                let start = window.id as usize * WINDOW;
                let mut rank = 0;
                while mask != 0 {
                    let doc = start + mask.trailing_zeros() as usize;
                    mask &= mask - 1;
                    scores[doc] += term.numerator * self.saturation(tfs.get(rank), norms[doc]);
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

/// The 128 bits of `bits` for window `w`.
fn window_bits(bits: &[u64], w: usize) -> u128 {
    let low = bits.get(2 * w).copied().unwrap_or(0);
    let high = bits.get(2 * w + 1).copied().unwrap_or(0);
    u128::from(low) | u128::from(high) << 64
}
