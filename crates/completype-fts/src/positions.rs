//! Token positions per term and field, one block per postings window: each document's first
//! position then its gaps minus one, bit-packed, behind a directory of `block_len << 6 | bits`.

use crate::format::{read_varint, write_varint, MAX_TF, WINDOW};

/// Positions a document keeps for a term: as many as its stored frequency.
#[inline]
pub(crate) fn kept(tf: u32) -> usize {
    tf.min(MAX_TF) as usize
}

/// Appends a term-field's positions; `positions` holds `kept(tf)` ascending values per posting.
pub(crate) fn write(out: &mut Vec<u8>, list: &[(u32, u32)], positions: &[u32]) {
    let (mut directory, mut blocks) = (Vec::new(), Vec::new());
    let mut values = Vec::new();
    let mut at = 0;
    for chunk in list.chunk_by(|a, b| a.0 as usize / WINDOW == b.0 as usize / WINDOW) {
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

/// Where a term-field's directory and blocks start in the positions section.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Located {
    pub directory: usize,
    pub blocks: usize,
}

/// The `rank`-th field's positions of the term whose positions start at `at`.
pub(crate) fn locate(section: &[u8], mut at: usize, rank: usize) -> Located {
    for _ in 0..rank {
        let blocks = read_varint(section, &mut at) as usize;
        let directory = read_varint(section, &mut at) as usize;
        at += directory + blocks;
    }
    read_varint(section, &mut at);
    let directory = read_varint(section, &mut at) as usize;
    Located {
        directory: at,
        blocks: at + directory,
    }
}

/// Each window's block as `(offset in section, bits)`, in window order.
pub(crate) struct Blocks<'a> {
    section: &'a [u8],
    at: usize,
    end: usize,
    block: usize,
}

impl<'a> Blocks<'a> {
    pub fn new(section: &'a [u8], located: Located) -> Self {
        Self {
            section,
            at: located.directory,
            end: located.blocks,
            block: located.blocks,
        }
    }
}

impl Iterator for Blocks<'_> {
    type Item = (usize, u8);

    #[inline]
    fn next(&mut self) -> Option<(usize, u8)> {
        if self.at >= self.end {
            return None;
        }
        let entry = read_varint(self.section, &mut self.at);
        let block = self.block;
        self.block += (entry >> 6) as usize;
        Some((block, (entry & 63) as u8))
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
