//! Document reordering by recursive graph bisection, so documents sharing terms share windows.

use std::cell::RefCell;

const ITERATIONS: usize = 12;
const LEAF: usize = crate::format::WINDOW;

/// Documents as term lists, for terms in more than one document.
pub(crate) struct Forward {
    offsets: Vec<usize>,
    terms: Vec<u32>,
    term_count: usize,
}

impl Forward {
    pub fn new<'a>(docs: usize, lists: impl Iterator<Item = &'a [(u32, u32)]> + Clone) -> Self {
        let mut offsets = vec![0; docs + 1];
        let mut term_count = 0;
        for list in lists.clone().filter(|l| l.len() > 1) {
            term_count += 1;
            for &(doc, _) in list {
                offsets[doc as usize + 1] += 1;
            }
        }
        for d in 0..docs {
            offsets[d + 1] += offsets[d];
        }
        let mut next = offsets.clone();
        let mut terms = vec![0; offsets[docs]];
        for (t, list) in lists.filter(|l| l.len() > 1).enumerate() {
            for &(doc, _) in list {
                terms[next[doc as usize]] = t as u32;
                next[doc as usize] += 1;
            }
        }
        Self {
            offsets,
            terms,
            term_count,
        }
    }

    fn terms(&self, doc: u32) -> &[u32] {
        &self.terms[self.offsets[doc as usize]..self.offsets[doc as usize + 1]]
    }
}

thread_local! {
    static DEGREES: RefCell<(Vec<u32>, Vec<u32>)> = const { RefCell::new((Vec::new(), Vec::new())) };
}

/// New order of the documents: `order[new] = old`.
pub(crate) fn order(forward: &Forward, docs: usize) -> Vec<u32> {
    let mut order: Vec<u32> = (0..docs as u32).collect();
    let log2: Vec<f32> = (0..docs + 2).map(|n| (n as f32).log2()).collect();
    bisect(forward, &mut order, &log2);
    order
}

fn bisect(forward: &Forward, docs: &mut [u32], log2: &[f32]) {
    if docs.len() <= LEAF {
        return;
    }
    DEGREES.with_borrow_mut(|(left, right)| {
        if left.len() < forward.term_count {
            left.resize(forward.term_count, 0);
            right.resize(forward.term_count, 0);
        }
        partition(forward, docs, left, right, log2);
    });
    let (a, b) = docs.split_at_mut(docs.len() / 2);
    if a.len() > 4096 {
        rayon::join(|| bisect(forward, a, log2), || bisect(forward, b, log2));
    } else {
        bisect(forward, a, log2);
        bisect(forward, b, log2);
    }
}

fn partition(
    forward: &Forward,
    docs: &mut [u32],
    left: &mut [u32],
    right: &mut [u32],
    log2: &[f32],
) {
    let mid = docs.len() / 2;
    let (n1, n2) = (log2[mid], log2[docs.len() - mid]);
    // Cost of a term split d1 / d2 between the halves, as in Dhulipala et al.
    let cost = |d1: u32, d2: u32| {
        d1 as f32 * (n1 - log2[d1 as usize + 1]) + d2 as f32 * (n2 - log2[d2 as usize + 1])
    };
    let mut gains_a: Vec<(f32, u32)> = Vec::with_capacity(mid);
    let mut gains_b: Vec<(f32, u32)> = Vec::with_capacity(docs.len() - mid);
    for _ in 0..ITERATIONS {
        for &d in docs.iter() {
            for &t in forward.terms(d) {
                left[t as usize] = 0;
                right[t as usize] = 0;
            }
        }
        for &d in &docs[..mid] {
            forward.terms(d).iter().for_each(|&t| left[t as usize] += 1);
        }
        for &d in &docs[mid..] {
            forward
                .terms(d)
                .iter()
                .for_each(|&t| right[t as usize] += 1);
        }
        gains_a.clear();
        gains_a.extend(docs[..mid].iter().map(|&d| {
            let gain = forward.terms(d).iter().map(|&t| {
                let (l, r) = (left[t as usize], right[t as usize]);
                cost(l, r) - cost(l - 1, r + 1)
            });
            (gain.sum(), d)
        }));
        gains_b.clear();
        gains_b.extend(docs[mid..].iter().map(|&d| {
            let gain = forward.terms(d).iter().map(|&t| {
                let (l, r) = (left[t as usize], right[t as usize]);
                cost(l, r) - cost(l + 1, r - 1)
            });
            (gain.sum(), d)
        }));
        gains_a.sort_unstable_by(|x, y| y.0.total_cmp(&x.0).then(x.1.cmp(&y.1)));
        gains_b.sort_unstable_by(|x, y| y.0.total_cmp(&x.0).then(x.1.cmp(&y.1)));
        let swaps = gains_a
            .iter()
            .zip(&gains_b)
            .take_while(|(a, b)| a.0 + b.0 > 0.0)
            .count();
        if swaps == 0 {
            break;
        }
        for (i, (a, b)) in gains_a.iter().zip(&gains_b).enumerate() {
            if i < swaps {
                docs[i] = b.1;
                docs[mid + i] = a.1;
            } else {
                docs[i] = a.1;
                docs[mid + i] = b.1;
            }
        }
        if mid != docs.len() - mid {
            docs[docs.len() - 1] = gains_b[gains_b.len() - 1].1;
        }
    }
}
