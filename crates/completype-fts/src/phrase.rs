//! Phrase frequency within one document, from each phrase word's positions.

/// Each match adds `1 / (1 + width)`; `lists[order[i]]` holds the `i`-th word's positions.
/// A match consumes a position of every word, so this never exceeds the rarest word's frequency.
pub(crate) fn frequency(lists: &[Vec<u32>], order: &[u8], slop: u32) -> f32 {
    let n = order.len();
    if slop == 0 && lists.len() == n {
        return exact(lists, order) as f32;
    }
    sliding(lists, order, slop)
}

/// Slides a window over every word's positions, moving the lowest on until a match fits.
fn sliding(lists: &[Vec<u32>], order: &[u8], slop: u32) -> f32 {
    let n = order.len();
    let mut at = [0_usize; MAX_WORDS];
    let at = &mut at[..n];
    let list = |i: usize| &lists[usize::from(order[i])];
    let mut freq = 0.0;
    loop {
        if (0..n).any(|i| at[i] >= list(i).len()) {
            return freq;
        }
        let position = |i: usize, at: &[usize]| list(i)[at[i]];
        // Two places of one repeated word cannot share a position: move the later place on.
        let clash = (1..n)
            .find(|&j| (0..j).any(|i| order[i] == order[j] && position(i, at) == position(j, at)));
        if let Some(j) = clash {
            at[j] += 1;
            continue;
        }
        let (mut low, mut high, mut lowest) = (i64::MAX, i64::MIN, 0);
        for i in 0..n {
            let v = i64::from(position(i, at)) - i as i64;
            if v < low {
                (low, lowest) = (v, i);
            }
            high = high.max(v);
        }
        let width = (high - low) as u64;
        if width <= u64::from(slop) {
            freq += 1.0 / (1.0 + width as f32);
            at.iter_mut().for_each(|a| *a += 1);
        } else {
            at[lowest] += 1;
        }
    }
}

/// Start positions of the first word where every other word follows in place, for phrases
/// without repeated words; equals [`frequency`] with no slop.
fn exact(lists: &[Vec<u32>], order: &[u8]) -> u32 {
    let mut at = [0_usize; MAX_WORDS];
    let mut count = 0;
    'starts: for &p in &lists[usize::from(order[0])] {
        for (i, &word) in order.iter().enumerate().skip(1) {
            let list = &lists[usize::from(word)];
            let want = u64::from(p) + i as u64;
            while at[i] < list.len() && u64::from(list[at[i]]) < want {
                at[i] += 1;
            }
            match list.get(at[i]) {
                None => return count,
                Some(&q) if u64::from(q) != want => continue 'starts,
                Some(_) => {}
            }
        }
        count += 1;
    }
    count
}

/// Longest phrase searched; later words are dropped.
pub(crate) const MAX_WORDS: usize = 32;

#[cfg(test)]
mod tests {
    use super::*;

    fn freq(doc: &str, phrase: &str, slop: u32) -> f32 {
        let doc: Vec<&str> = doc.split(' ').collect();
        let words: Vec<&str> = phrase.split(' ').collect();
        let mut distinct: Vec<&str> = Vec::new();
        let order: Vec<u8> = words
            .iter()
            .map(|w| match distinct.iter().position(|d| d == w) {
                Some(i) => i as u8,
                None => {
                    distinct.push(w);
                    (distinct.len() - 1) as u8
                }
            })
            .collect();
        let lists: Vec<Vec<u32>> = distinct
            .iter()
            .map(|d| {
                (0..doc.len() as u32)
                    .filter(|&p| doc[p as usize] == *d)
                    .collect()
            })
            .collect();
        frequency(&lists, &order, slop)
    }

    #[test]
    fn exact_sloppy_and_repeated_phrases() {
        assert_eq!(freq("new york new york", "new york", 0), 2.0);
        assert_eq!(freq("york new", "new york", 0), 0.0);
        assert_eq!(freq("york new", "new york", 2), 1.0 / 3.0);
        assert_eq!(freq("new big york", "new york", 0), 0.0);
        assert_eq!(freq("new big york", "new york", 1), 0.5);
        assert_eq!(freq("a b a b a", "a b a", 0), 2.0);
        assert_eq!(freq("a a a", "a a", 0), 2.0);
        assert_eq!(freq("a", "a a", 0), 0.0);
        assert_eq!(freq("a b a", "a a", 1), 0.5);
        assert_eq!(freq("x y z", "x z", 0), 0.0);
    }

    #[test]
    fn the_exact_path_counts_what_sliding_counts() {
        let mut state = 1_u64;
        let mut next = |n: u32| {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            (state >> 33) as u32 % n
        };
        for _ in 0..2000 {
            let n = 2 + next(3) as usize;
            let lists: Vec<Vec<u32>> = (0..n)
                .map(|_| {
                    let mut l: Vec<u32> = (0..next(6)).map(|_| next(20)).collect();
                    l.sort_unstable();
                    l.dedup();
                    l
                })
                .collect();
            let order: Vec<u8> = (0..n as u8).collect();
            assert_eq!(exact(&lists, &order) as f32, sliding(&lists, &order, 0));
        }
    }
}
