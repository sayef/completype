//! Token positions per term and field, one block per postings window: each document's first
//! position then its gaps minus one, bit-packed, behind a directory of `block_len << 6 | bits`.

use crate::format::{read_varint, u32_at, write_varint, Headers, MAX_TF, WINDOW};
use crate::segment::TermInfo;

/// Windows between skip entries, which let phrase intersections jump through long lists.
pub(crate) const SKIP: usize = 16;
const SKIP_LEN: usize = 20;

/// Positions a document keeps for a term: as many as its stored frequency.
#[inline]
pub(crate) fn kept(tf: u32) -> usize {
    tf.min(MAX_TF) as usize
}

/// Where a term's postings stand before window `k * SKIP`: the previous window's id and the
/// offsets of the window's header and body within the term's.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Anchor {
    pub previous: u32,
    pub headers: u32,
    pub body: u32,
}

/// Appends a term-field's positions: `varint blocks_len, varint directory_len, varint skips`,
/// the skip entries, the directory and the blocks.
pub(crate) fn write(out: &mut Vec<u8>, list: &[(u32, u32)], positions: &[u32], anchors: &[Anchor]) {
    let (mut directory, mut blocks, mut skips) = (Vec::new(), Vec::new(), Vec::new());
    let mut values = Vec::new();
    let mut at = 0;
    let chunks = list.chunk_by(|a, b| a.0 as usize / WINDOW == b.0 as usize / WINDOW);
    for (rank, chunk) in chunks.enumerate() {
        if rank > 0 && rank.is_multiple_of(SKIP) {
            let anchor = anchors[rank / SKIP - 1];
            for v in [
                anchor.previous,
                anchor.headers,
                anchor.body,
                directory.len() as u32,
                blocks.len() as u32,
            ] {
                skips.extend_from_slice(&v.to_le_bytes());
            }
        }
        values.clear();
        for &(_, tf) in chunk {
            let doc = &positions[at..at + kept(tf)];
            at += doc.len();
            let mut previous = None;
            for &p in doc {
                values.push(previous.map_or(p, |q: u32| p - q - 1));
                previous = Some(p);
            }
        }
        let bits = values
            .iter()
            .map(|v| u32::BITS - v.leading_zeros())
            .max()
            .unwrap_or(0);
        let start = blocks.len();
        pack(&mut blocks, &values, bits);
        write_varint(&mut directory, ((blocks.len() - start) as u32) << 6 | bits);
    }
    write_varint(out, blocks.len() as u32);
    write_varint(out, directory.len() as u32);
    write_varint(out, (skips.len() / SKIP_LEN) as u32);
    out.extend_from_slice(&skips);
    out.extend_from_slice(&directory);
    out.extend_from_slice(&blocks);
}

fn pack(out: &mut Vec<u8>, values: &[u32], bits: u32) {
    if bits == 0 {
        return;
    }
    let (mut buffer, mut filled) = (0_u64, 0);
    for &v in values {
        buffer |= u64::from(v) << filled;
        filled += bits;
        while filled >= 8 {
            out.push(buffer as u8);
            buffer >>= 8;
            filled -= 8;
        }
    }
    if filled > 0 {
        out.push(buffer as u8);
    }
}

/// Where a term-field's skip entries, directory and blocks start in the positions section.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Located {
    pub skips: usize,
    pub skip_count: u32,
    pub directory: usize,
    pub blocks: usize,
}

/// The `rank`-th field's positions of the term whose positions start at `at`.
pub(crate) fn locate(section: &[u8], mut at: usize, rank: usize) -> Located {
    for _ in 0..rank {
        let blocks = read_varint(section, &mut at) as usize;
        let directory = read_varint(section, &mut at) as usize;
        let skips = read_varint(section, &mut at) as usize;
        at += skips * SKIP_LEN + directory + blocks;
    }
    read_varint(section, &mut at);
    let directory = read_varint(section, &mut at) as usize;
    let skip_count = read_varint(section, &mut at);
    let start = at + skip_count as usize * SKIP_LEN;
    Located {
        skips: at,
        skip_count,
        directory: start,
        blocks: start + directory,
    }
}

impl Located {
    fn skip(&self, section: &[u8], e: usize) -> [u32; 5] {
        let at = self.skips + e * SKIP_LEN;
        std::array::from_fn(|i| u32_at(section, at + 4 * i))
    }

    /// The block of the term's `rank`-th window, as `(offset in section, bits)`.
    #[inline]
    pub fn block(&self, section: &[u8], rank: usize) -> (usize, u8) {
        self.block_after(section, rank, &mut Walk::default())
    }

    /// [`Located::block`], walking on from where `walk` last stopped when that is nearer.
    #[inline]
    pub fn block_after(&self, section: &[u8], rank: usize, walk: &mut Walk) -> (usize, u8) {
        let anchor = rank / SKIP * SKIP;
        if !walk.started || walk.rank > rank || walk.rank < anchor {
            (walk.at, walk.block) = match rank / SKIP {
                0 => (self.directory, self.blocks),
                e => {
                    let s = self.skip(section, e - 1);
                    (self.directory + s[3] as usize, self.blocks + s[4] as usize)
                }
            };
            (walk.rank, walk.started) = (anchor, true);
        }
        while walk.rank < rank {
            walk.block += (read_varint(section, &mut walk.at) >> 6) as usize;
            walk.rank += 1;
        }
        let mut at = walk.at;
        (walk.block, (read_varint(section, &mut at) & 63) as u8)
    }
}

/// Where a walk through a directory stands: the next window's rank, entry and block.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Walk {
    started: bool,
    rank: usize,
    at: usize,
    block: usize,
}

/// A term's windows in order, able to jump ahead through skip entries.
pub(crate) struct Cursor<'a> {
    headers: Headers<'a>,
    bytes: &'a [u8],
    section: &'a [u8],
    located: Located,
    windows: u32,
    /// The current window and its rank among the term's windows.
    pub window: crate::format::Window,
    pub rank: u32,
}

impl<'a> Cursor<'a> {
    pub fn new(
        headers: Headers<'a>,
        bytes: &'a [u8],
        section: &'a [u8],
        located: Located,
        info: &TermInfo,
    ) -> Option<Self> {
        let mut headers = headers;
        let window = headers.next()?;
        Some(Self {
            headers,
            bytes,
            section,
            located,
            windows: info.windows,
            window,
            rank: 0,
        })
    }

    pub fn advance(&mut self) -> bool {
        match self.headers.next() {
            Some(w) => {
                self.window = w;
                self.rank += 1;
                true
            }
            None => false,
        }
    }

    /// Moves to the first window with id at least `target`; false when there is none.
    pub fn seek(&mut self, target: u32) -> bool {
        if self.window.id >= target {
            return true;
        }
        let mut best = None;
        let mut e = self.rank as usize / SKIP;
        while e < self.located.skip_count as usize {
            if u32_at(self.section, self.located.skips + e * SKIP_LEN) >= target {
                break;
            }
            best = Some(e);
            e += 1;
        }
        if let Some(e) = best {
            let s = self.located.skip(self.section, e);
            let rank = (e + 1) * SKIP;
            let left = self.windows - rank as u32;
            self.headers = Headers::resume(&self.bytes[s[1] as usize..], left, s[0], s[2]);
            self.rank = rank as u32 - 1;
            if !self.advance() {
                return false;
            }
        }
        while self.window.id < target {
            if !self.advance() {
                return false;
            }
        }
        true
    }
}

/// Decodes `count` positions of one document starting at value `first` of a block into `out`.
#[inline]
pub(crate) fn read(
    section: &[u8],
    block: (usize, u8),
    first: usize,
    count: usize,
    out: &mut Vec<u32>,
) {
    out.clear();
    let (start, bits) = (block.0, usize::from(block.1));
    let mask = if bits == 32 {
        u64::from(u32::MAX)
    } else {
        (1 << bits) - 1
    };
    let mut position = 0_u32;
    for i in 0..count {
        let value = if bits == 0 {
            0
        } else {
            let bit = (first + i) * bits;
            let at = start + bit / 8;
            let word = u64::from_le_bytes(section[at..at + 8].try_into().unwrap());
            ((word >> (bit % 8)) & mask) as u32
        };
        position = if i == 0 { value } else { position + value + 1 };
        out.push(position);
    }
}

/// The value at `index` of a block: a position when it starts its document's list.
#[inline]
pub(crate) fn read_one(section: &[u8], block: (usize, u8), index: usize) -> u32 {
    let bits = usize::from(block.1);
    if bits == 0 {
        return 0;
    }
    let bit = index * bits;
    let at = block.0 + bit / 8;
    let word = u64::from_le_bytes(section[at..at + 8].try_into().unwrap());
    ((word >> (bit % 8)) & ((1_u64 << bits) - 1)) as u32
}

/// Each window's block in order, for decoding whole lists.
pub(crate) fn blocks(section: &[u8], located: Located) -> impl Iterator<Item = (usize, u8)> + '_ {
    let (mut at, mut block) = (located.directory, located.blocks);
    std::iter::from_fn(move || {
        if at >= located.blocks {
            return None;
        }
        let entry = read_varint(section, &mut at);
        let current = block;
        block += (entry >> 6) as usize;
        Some((current, (entry & 63) as u8))
    })
}
