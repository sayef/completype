//! Segment layout: postings in 128-document windows, each with a small header (id delta, count,
//! frequency width, score bound) and a body (presence mask or offsets, then frequencies).

pub(crate) const WINDOW: usize = u128::BITS as usize;
pub(crate) const MASK_FROM: usize = 16;
pub(crate) const MAGIC: &[u8; 8] = b"CTFTS\0\0\x05";
/// Version 4 segments lack the position sections and are still read.
pub(crate) const MAGIC_V4: &[u8; 8] = b"CTFTS\0\0\x04";
/// Zero bytes after the bodies, so bit-packed reads never run past the end.
pub(crate) const PADDING: usize = 8;
/// Dictionary values with this bit hold a term's only posting instead of a record offset.
pub(crate) const INLINE: u64 = 1 << 63;
/// Where an inline posting keeps its field.
pub(crate) const INLINE_FIELD_SHIFT: u32 = 48;
pub(crate) const fn header_len(sections: usize) -> usize {
    8 + 4 + 4 + 8 + 8 * sections
}
pub(crate) const HEADER_LEN: usize = header_len(SECTIONS);
pub(crate) const K1: f32 = 1.2;
pub(crate) const B: f32 = 0.75;

pub(crate) const SECTIONS: usize = 16;
pub(crate) const SECTIONS_V4: usize = 14;
pub(crate) const NORMS: usize = 0;
pub(crate) const ID_OFFSETS: usize = 1;
pub(crate) const ID_BYTES: usize = 2;
pub(crate) const TERMS: usize = 3;
pub(crate) const HEADERS: usize = 4;
pub(crate) const BODIES: usize = 5;
pub(crate) const DICTIONARY: usize = 6;
pub(crate) const ID_LOOKUP: usize = 7;
pub(crate) const FIELDS: usize = 8;
pub(crate) const KEYWORDS: usize = 9;
pub(crate) const KEYWORD_DOCS: usize = 10;
pub(crate) const NUMBERS: usize = 11;
pub(crate) const STORED_INDEX: usize = 12;
pub(crate) const STORED: usize = 13;
/// Term to the offset of its positions; empty when the segment has none.
pub(crate) const POSITION_TERMS: usize = 14;
pub(crate) const POSITIONS: usize = 15;
/// Positions between values of a repeated text field, so phrases do not join them.
pub(crate) const POSITION_GAP: u32 = 100;

/// The most a stored term frequency can be.
pub(crate) const MAX_TF: u32 = u16::MAX as u32;

/// A term's postings in one window; a single posting lives in the header and has no body.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Window {
    pub id: u32,
    pub count: u8,
    pub tf_bits: u8,
    /// Stored saturation bound, or 0 for a single posting, whose exact score is cheap.
    pub bound: u8,
    pub body: u32,
    pub single: (u8, u32),
}

impl Window {
    pub fn count(&self) -> usize {
        usize::from(self.count)
    }

    pub fn body_len(&self) -> usize {
        if self.count == 1 {
            return 0;
        }
        Self::docs_len(self.count()) + (self.count() * usize::from(self.tf_bits)).div_ceil(8)
    }

    fn docs_len(count: usize) -> usize {
        if count >= MASK_FROM {
            16
        } else {
            count
        }
    }

    /// Presence mask, and the frequency of the `rank`-th present document through `tf`.
    #[inline]
    pub fn decode<'a>(&self, bodies: &'a [u8]) -> (u128, Frequencies<'a>) {
        if self.count == 1 {
            let (offset, tf) = self.single;
            return (
                1 << offset,
                Frequencies {
                    packed: &[],
                    bits: 0,
                    constant: tf,
                },
            );
        }
        let body = &bodies[self.body as usize..];
        let docs_len = Self::docs_len(self.count());
        let mask = if docs_len == 16 {
            u128::from_le_bytes(body[..16].try_into().unwrap())
        } else {
            body[..docs_len].iter().fold(0, |m, &o| m | 1 << o)
        };
        (
            mask,
            Frequencies {
                packed: &body[docs_len..],
                bits: self.tf_bits,
                constant: 1,
            },
        )
    }
}

/// Term frequencies of one window, bit-packed as `tf - 1`.
#[derive(Clone, Copy)]
pub(crate) struct Frequencies<'a> {
    packed: &'a [u8],
    bits: u8,
    constant: u32,
}

impl Frequencies<'_> {
    #[inline]
    pub fn get(&self, rank: usize) -> u32 {
        if self.bits == 0 {
            return self.constant;
        }
        let bit = rank * usize::from(self.bits);
        let word = u32::from_le_bytes(self.packed[bit / 8..bit / 8 + 4].try_into().unwrap());
        ((word >> (bit % 8)) & ((1 << self.bits) - 1)) + 1
    }
}

pub(crate) fn pack_frequencies(out: &mut Vec<u8>, tfs: impl Iterator<Item = u32>, bits: u8) {
    if bits == 0 {
        return;
    }
    let (mut buffer, mut filled) = (0_u64, 0);
    for tf in tfs {
        buffer |= u64::from(tf - 1) << filled;
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

/// Walks a term's window headers in order, tracking each body's offset.
pub(crate) struct Headers<'a> {
    inline: Option<Window>,
    bytes: &'a [u8],
    at: usize,
    left: u32,
    id: u32,
    body: u32,
    first: bool,
}

impl<'a> Headers<'a> {
    pub fn new(bytes: &'a [u8], windows: u32) -> Self {
        Self {
            inline: None,
            bytes,
            at: 0,
            left: windows,
            id: 0,
            body: 0,
            first: true,
        }
    }

    pub fn single(doc: u32, tf: u32) -> Self {
        let window = Window {
            id: doc / WINDOW as u32,
            count: 1,
            tf_bits: 0,
            bound: 0,
            body: 0,
            single: ((doc % WINDOW as u32) as u8, tf),
        };
        Self {
            inline: Some(window),
            ..Self::new(&[], 0)
        }
    }
}

impl Iterator for Headers<'_> {
    type Item = Window;

    #[inline]
    fn next(&mut self) -> Option<Window> {
        if let Some(window) = self.inline.take() {
            return Some(window);
        }
        if self.left == 0 {
            return None;
        }
        self.left -= 1;
        let head = read_varint(self.bytes, &mut self.at);
        let delta = head >> 1;
        self.id = if self.first { delta } else { self.id + delta };
        self.first = false;
        let mut window = Window {
            id: self.id,
            count: 1,
            tf_bits: 0,
            bound: 0,
            body: self.body,
            single: (0, 1),
        };
        if head & 1 == 0 {
            let offset = self.bytes[self.at];
            self.at += 1;
            let tf = if offset & 0x80 != 0 {
                read_varint(self.bytes, &mut self.at)
            } else {
                1
            };
            window.single = (offset & 0x7f, tf);
        } else {
            let [count, tf_bits, bound] = self.bytes[self.at..self.at + 3].try_into().unwrap();
            self.at += 3;
            (window.count, window.tf_bits, window.bound) = (count + 1, tf_bits, bound);
            self.body += window.body_len() as u32;
        }
        Some(window)
    }
}

/// Writes one window's header and body; postings are `(offset in window, tf)`.
pub(crate) fn write_window(
    headers: &mut Vec<u8>,
    bodies: &mut Vec<u8>,
    delta: u32,
    postings: &[(u8, u32)],
    bound: u8,
) {
    if let [(offset, tf)] = postings {
        write_varint(headers, delta << 1);
        if *tf == 1 {
            headers.push(*offset);
        } else {
            headers.push(offset | 0x80);
            write_varint(headers, *tf);
        }
        return;
    }
    write_varint(headers, delta << 1 | 1);
    let max_tf = postings.iter().map(|p| p.1).max().unwrap();
    let tf_bits = (u32::BITS - (max_tf - 1).leading_zeros()) as u8;
    headers.extend_from_slice(&[(postings.len() - 1) as u8, tf_bits, bound]);
    if postings.len() >= MASK_FROM {
        let mask = postings.iter().fold(0_u128, |m, p| m | 1 << p.0);
        bodies.extend_from_slice(&mask.to_le_bytes());
    } else {
        bodies.extend(postings.iter().map(|p| p.0));
    }
    pack_frequencies(bodies, postings.iter().map(|p| p.1), tf_bits);
}

pub(crate) fn write_varint(out: &mut Vec<u8>, mut value: u32) {
    while value >= 0x80 {
        out.push(value as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

#[inline]
pub(crate) fn read_varint(bytes: &[u8], at: &mut usize) -> u32 {
    read_varint64(bytes, at) as u32
}

pub(crate) fn write_varint64(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(value as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

#[inline]
pub(crate) fn read_varint64(bytes: &[u8], at: &mut usize) -> u64 {
    let mut value = 0;
    let mut shift = 0;
    loop {
        let byte = bytes[*at];
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            return value;
        }
        shift += 7;
    }
}

pub(crate) fn write_str(out: &mut Vec<u8>, s: &str) {
    write_varint(out, s.len() as u32);
    out.extend_from_slice(s.as_bytes());
}

pub(crate) fn read_str(bytes: &[u8], at: &mut usize) -> Option<String> {
    let len = read_varint(bytes, at) as usize;
    let s = std::str::from_utf8(bytes.get(*at..*at + len)?)
        .ok()?
        .to_owned();
    *at += len;
    Some(s)
}

pub(crate) fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

pub(crate) fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

/// Saturation bound stored in a byte, never below the value it bounds.
pub(crate) fn quantize_up(saturation: f64) -> u8 {
    let mut q = (saturation * 255.0).ceil().clamp(1.0, 255.0) as u8;
    while q < 255 && (f32::from(q) / 255.0) < saturation as f32 {
        q += 1;
    }
    q
}

/// BM25 length normalisation `k1 * (1 - b + b * len / avgdl)` for each norm byte.
pub(crate) fn length_table(average: f32) -> [f32; 256] {
    std::array::from_fn(|n| {
        let len = crate::norm::decode(n as u8) as f32;
        K1 * (1.0 - B + B * len / average)
    })
}
