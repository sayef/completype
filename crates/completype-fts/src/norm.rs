//! One-byte document lengths, encoded like Lucene's `SmallFloat.intToByte4`.

const FREE: u32 = 24;

pub(crate) fn encode(len: u32) -> u8 {
    let len = len.min(i32::MAX as u32);
    if len < FREE {
        return len as u8;
    }
    let i = len - FREE;
    let bits = u32::BITS - i.leading_zeros();
    let int4 = if bits < 4 {
        i
    } else {
        let shift = bits - 4;
        ((i >> shift) & 7) | ((shift + 1) << 3)
    };
    (FREE + int4) as u8
}

pub(crate) fn decode(byte: u8) -> u32 {
    let i = u32::from(byte);
    if i < FREE {
        return i;
    }
    let j = i - FREE;
    let bits = j & 7;
    FREE + match (j >> 3).checked_sub(1) {
        None => bits,
        Some(shift) => (bits | 8) << shift,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_lengths_are_exact_and_long_ones_round_down_monotonically() {
        for len in 0..40 {
            assert_eq!(decode(encode(len)), len);
        }
        let mut previous = 0;
        for len in (0..1_000_000).chain([i32::MAX as u32, u32::MAX]) {
            let decoded = decode(encode(len));
            assert!(decoded <= len.min(i32::MAX as u32) && decoded >= previous);
            previous = decoded;
        }
        assert_eq!(encode(u32::MAX), 255);
    }
}
