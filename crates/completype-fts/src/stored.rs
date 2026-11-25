//! Stored payloads in zstd blocks; the index holds each block's first document and offset.

use crate::format::*;

const BLOCK_BYTES: usize = 16 * 1024;
const BLOCK_DOCS: usize = 1024;
const LEVEL: i32 = 3;
const ENTRY: usize = 12;

/// Index and data sections for `docs` documents' payloads; empty when none has one.
pub(crate) fn write(payloads: &[Option<Box<[u8]>>], docs: usize) -> (Vec<u8>, Vec<u8>) {
    let (mut index, mut data) = (Vec::new(), Vec::new());
    if payloads.iter().all(Option::is_none) {
        return (index, data);
    }
    let mut first = 0;
    while first < docs {
        let (mut end, mut bytes) = (first, 0);
        while end < docs && end - first < BLOCK_DOCS && bytes < BLOCK_BYTES {
            bytes += payloads
                .get(end)
                .and_then(Option::as_ref)
                .map_or(0, |p| p.len());
            end += 1;
        }
        let mut plain = Vec::with_capacity(bytes + 2 * (end - first));
        write_varint(&mut plain, (end - first) as u32);
        for d in first..end {
            let payload = payloads.get(d).and_then(Option::as_ref);
            write_varint(&mut plain, payload.map_or(0, |p| p.len() as u32 + 1));
        }
        for d in first..end {
            if let Some(p) = payloads.get(d).and_then(Option::as_ref) {
                plain.extend_from_slice(p);
            }
        }
        index.extend_from_slice(&(first as u32).to_le_bytes());
        index.extend_from_slice(&(data.len() as u64).to_le_bytes());
        data.extend_from_slice(&zstd::bulk::compress(&plain, LEVEL).expect("zstd compresses"));
        first = end;
    }
    index.extend_from_slice(&(docs as u32).to_le_bytes());
    index.extend_from_slice(&(data.len() as u64).to_le_bytes());
    (index, data)
}

/// The block holding `doc` and its first document.
pub(crate) fn block_of(index: &[u8], doc: u32) -> Option<(usize, u32)> {
    let blocks = (index.len() / ENTRY).checked_sub(1)?;
    let first = |b: usize| u32_at(index, b * ENTRY);
    if doc >= first(blocks) {
        return None;
    }
    let (mut lo, mut hi) = (0, blocks);
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if first(mid) <= doc {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Some((lo, first(lo)))
}

pub(crate) fn decompress(index: &[u8], data: &[u8], block: usize) -> Vec<u8> {
    let (start, end) = (
        u64_at(index, block * ENTRY + 4) as usize,
        u64_at(index, (block + 1) * ENTRY + 4) as usize,
    );
    zstd::stream::decode_all(&data[start..end]).expect("stored block decompresses")
}

/// The payload of the document `offset` places into a decompressed block.
pub(crate) fn extract(block: &[u8], offset: usize) -> Option<Vec<u8>> {
    let mut at = 0;
    let count = read_varint(block, &mut at) as usize;
    let mut skip = 0;
    let mut found = None;
    for d in 0..count {
        let len = read_varint(block, &mut at) as usize;
        if d < offset {
            skip += len.saturating_sub(1);
        } else if d == offset {
            found = len.checked_sub(1);
        }
    }
    let len = found?;
    Some(block[at + skip..at + skip + len].to_vec())
}
