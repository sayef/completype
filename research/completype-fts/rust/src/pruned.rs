use std::cmp::Ordering;
use std::collections::BinaryHeap;

use super::{Hit, Index, Posting};

const BLOCK_SIZE: usize = u128::BITS as usize;

pub(super) struct Term {
    pub(super) postings: Vec<Posting>,
    blocks: Vec<Block>,
    max_saturation: f64,
    masks: Vec<u128>,
}

impl Term {
    pub(super) fn new(postings: Vec<Posting>, norm: &[f64]) -> Self {
        let mut max_saturation = 0.0_f64;
        let mut end = 0;
        let mut masks = Vec::new();
        let build_masks = postings.len() >= norm.len().div_ceil(BLOCK_SIZE);
        let blocks = postings
            .chunk_by(|a, b| a.doc() / BLOCK_SIZE == b.doc() / BLOCK_SIZE)
            .map(|block| {
                end += block.len();
                let mut peak = 0.0_f64;
                let mut mask = 0;
                for posting in block {
                    let tf = posting.tf as f64;
                    peak = peak.max(tf / (tf + norm[posting.doc()]));
                    if build_masks {
                        mask |= 1 << (posting.doc() % BLOCK_SIZE);
                    }
                }
                if build_masks {
                    masks.push(mask);
                }
                max_saturation = max_saturation.max(peak);
                Block {
                    id: (block[0].doc() / BLOCK_SIZE) as u32,
                    end: u32::try_from(end).expect("posting count fits u32"),
                    max_saturation: peak,
                }
            })
            .collect();
        Self {
            postings,
            blocks,
            max_saturation,
            masks,
        }
    }
}

struct Block {
    id: u32,
    end: u32,
    max_saturation: f64,
}

#[derive(Clone, Copy)]
struct Cursor<'a> {
    postings: &'a [Posting],
    blocks: &'a [Block],
    numerator: f64,
    global_bound: f64,
    next: usize,
}

impl Cursor<'_> {
    fn current(&self) -> Option<&Block> {
        self.blocks.get(self.next)
    }

    fn postings(&self) -> &[Posting] {
        let start = self.next.checked_sub(1).map_or(0, |i| self.blocks[i].end);
        &self.postings[start as usize..self.blocks[self.next].end as usize]
    }

    fn seek_block(&mut self, id: u32) {
        if self.current().is_some_and(|block| block.id < id) {
            self.next += 1;
            if self.current().is_some_and(|block| block.id < id) {
                self.next += self.blocks[self.next..].partition_point(|block| block.id < id);
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Ranked {
    score: f64,
    doc: usize,
}

impl Ord for Ranked {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .score
            .total_cmp(&self.score)
            .then(self.doc.cmp(&other.doc))
    }
}

impl PartialOrd for Ranked {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Ranked {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Ranked {}

impl Index {
    /// Search positive weighted terms with exact bitmap-assisted Block-Max MaxScore.
    pub fn search<'a>(
        &self,
        weighted_terms: impl IntoIterator<Item = (&'a str, f64)>,
        limit: usize,
    ) -> Vec<Hit<'_>> {
        if limit == 0 {
            return Vec::new();
        }
        let terms: Vec<_> = weighted_terms.into_iter().collect();
        if terms
            .iter()
            .any(|&(_, weight)| !(1e-100..=1e100).contains(&weight))
        {
            return self.search_exhaustive(terms, limit);
        }
        let mut masks = Vec::new();
        let cursors: Vec<_> = terms
            .into_iter()
            .filter_map(|(term, weight)| {
                let term = self.terms.get(term)?;
                masks.push((!term.masks.is_empty()).then_some(term.masks.as_slice()));
                let numerator = self.numerator(term.postings.len(), weight);
                Some(Cursor {
                    postings: &term.postings,
                    blocks: &term.blocks,
                    numerator,
                    global_bound: numerator * term.max_saturation,
                    next: 0,
                })
            })
            .collect();
        // Bounds can be summed in a different order from the exact scores.
        let rounding_guard = 1.0 + 8.0 * (cursors.len() as f64 + 1.0) * f64::EPSILON;
        let mut heap = BinaryHeap::with_capacity(limit.min(self.ids.len()));
        self.max_score(cursors, limit, rounding_guard, &mut heap, &masks);
        heap.into_sorted_vec()
            .into_iter()
            .map(|hit| Hit {
                id: &self.ids[hit.doc],
                score: hit.score,
            })
            .collect()
    }

    fn max_score(
        &self,
        mut cursors: Vec<Cursor<'_>>,
        limit: usize,
        rounding_guard: f64,
        heap: &mut BinaryHeap<Ranked>,
        masks: &[Option<&[u128]>],
    ) {
        let mut active = Vec::with_capacity(cursors.len());
        let mut local_bounds = vec![0.0; cursors.len()];
        let mut essential = vec![false; cursors.len()];
        let mut global_order: Vec<_> = (0..cursors.len()).collect();
        global_order
            .sort_unstable_by(|&a, &b| cursors[a].global_bound.total_cmp(&cursors[b].global_bound));
        let mut first_essential = 0;
        let mut global_nonessential_bound = 0.0;
        let mut threshold = 0.0;
        let mut prune_below = 0.0;

        loop {
            while let Some(&i) = global_order.get(first_essential) {
                let next = global_nonessential_bound + cursors[i].global_bound;
                if next >= prune_below {
                    break;
                }
                global_nonessential_bound = next;
                first_essential += 1;
            }
            let Some(block_id) = global_order[first_essential..]
                .iter()
                .filter_map(|&i| cursors[i].current().map(|block| block.id))
                .min()
            else {
                break;
            };
            active.clear();
            let mut block_bound = 0.0;
            for (i, cursor) in cursors.iter_mut().enumerate() {
                cursor.seek_block(block_id);
                let bound = cursor
                    .current()
                    .filter(|block| block.id == block_id)
                    .map(|block| cursor.numerator * block.max_saturation);
                if let Some(bound) = bound {
                    block_bound += bound;
                    active.push(i);
                }
                local_bounds[i] = bound.unwrap_or(0.0);
            }
            if block_bound >= prune_below {
                let nonessential_bound = partition_window(
                    global_order.iter().map(|&i| (i, local_bounds[i])),
                    &mut essential,
                    prune_below,
                );
                let needs_mask = nonessential_bound > 0.0;
                let mut candidates = 0_u128;
                if needs_mask {
                    for &i in &active {
                        if essential[i] {
                            if let Some(mask) = masks[i] {
                                candidates |= mask[cursors[i].next];
                                continue;
                            }
                            for posting in cursors[i].postings() {
                                let offset = posting.doc() % BLOCK_SIZE;
                                candidates |= 1 << offset;
                            }
                        }
                    }
                }
                let mut scores = [0.0; BLOCK_SIZE];
                for &i in &active {
                    let cursor = &cursors[i];
                    let postings = cursor.postings();
                    if let Some(masks) = (needs_mask && !essential[i]).then(|| masks[i]).flatten() {
                        let presence = masks[cursor.next];
                        let mut matching = presence & candidates;
                        if (matching.count_ones() as usize) * 4 <= postings.len() {
                            while matching != 0 {
                                let offset = matching.trailing_zeros() as usize;
                                let bit = 1_u128 << offset;
                                matching &= matching - 1;
                                let position = (presence & (bit - 1)).count_ones() as usize;
                                scores[offset] += self.term_score(*cursor, &postings[position]);
                            }
                            continue;
                        }
                    }
                    for posting in postings {
                        let offset = posting.doc() % BLOCK_SIZE;
                        let bit = 1 << offset;
                        if !needs_mask {
                            candidates |= bit;
                        }
                        if essential[i] || candidates & bit != 0 {
                            scores[offset] += self.term_score(*cursor, posting);
                        }
                    }
                }
                let mut competitive = threshold;
                while candidates != 0 {
                    let offset = candidates.trailing_zeros() as usize;
                    candidates &= candidates - 1;
                    let score = scores[offset];
                    if score > competitive {
                        collect_competitive(
                            heap,
                            limit,
                            block_id as usize * BLOCK_SIZE + offset,
                            score,
                        );
                        competitive = cutoff(heap, limit);
                    }
                }
                if competitive > threshold {
                    threshold = competitive;
                    // Round the inverse guard down so the raw bounds remain conservative.
                    prune_below = (threshold.next_down() / rounding_guard).next_down();
                }
            }
            for &i in &active {
                cursors[i].next += 1;
            }
        }
    }

    fn term_score(&self, cursor: Cursor<'_>, posting: &Posting) -> f64 {
        let tf = posting.tf as f64;
        cursor.numerator * tf / (tf + self.norm[posting.doc()])
    }
}

fn partition_window(
    terms: impl Iterator<Item = (usize, f64)>,
    essential: &mut [bool],
    prune_below: f64,
) -> f64 {
    let mut nonessential_bound = 0.0;
    for (i, bound) in terms {
        let next = nonessential_bound + bound;
        essential[i] = next >= prune_below;
        if !essential[i] {
            nonessential_bound = next;
        }
    }
    nonessential_bound
}

fn cutoff(heap: &BinaryHeap<Ranked>, limit: usize) -> f64 {
    if heap.len() == limit {
        heap.peek().unwrap().score
    } else {
        0.0
    }
}

fn collect_competitive(heap: &mut BinaryHeap<Ranked>, limit: usize, doc: usize, score: f64) {
    let hit = Ranked { score, doc };
    if heap.len() < limit {
        heap.push(hit);
    } else {
        *heap.peek_mut().unwrap() = hit;
    }
}
