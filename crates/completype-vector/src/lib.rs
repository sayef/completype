//! Quantised embeddings: TurboQuant codes (turbovec) searched by inner product under a mask.
//! TurboQuant is data-oblivious, so codes encoded separately stay compatible and can be concatenated.

use std::fmt;

use turbovec::TurboQuantIndex;

/// The largest supported dimension.
pub const MAX_DIM: usize = turbovec::MAX_DIM;

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// Bits per dimension outside 2, 3 or 4.
    InvalidBits(u8),
    /// A dimension that is zero, not a multiple of 8 or above [`MAX_DIM`].
    InvalidDim(usize),
    /// Input row `row` has `len` dimensions instead of `dim`.
    RowDim { row: usize, len: usize, dim: usize },
    /// Input row `row` holds a NaN or infinity.
    RowNotFinite { row: usize },
    /// Input row `row` is a code of the wrong length.
    CodeSize { row: usize },
    /// The quantiser rejected its input.
    Quantiser(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBits(bits) => write!(f, "vector bits must be 2, 3 or 4, not {bits}"),
            Self::InvalidDim(dim) => write!(
                f,
                "vector dimension must be a positive multiple of 8 up to {MAX_DIM}, not {dim}"
            ),
            Self::RowDim { row, len, dim } => {
                write!(f, "vector {row} has {len} dimensions, not {dim}")
            }
            Self::RowNotFinite { row } => write!(f, "vector {row} is not finite"),
            Self::CodeSize { .. } => write!(f, "vector code of the wrong size"),
            Self::Quantiser(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

/// An embedding as floats to quantise, or as a code and scale encoded earlier.
#[derive(Debug, Clone, Copy)]
pub enum Row<'a> {
    Float(&'a [f32]),
    Code(&'a [u8], f32),
}

pub fn validate_bits(bits: u8) -> Result<(), Error> {
    if (2..=4).contains(&bits) {
        Ok(())
    } else {
        Err(Error::InvalidBits(bits))
    }
}

pub fn validate_dim(dim: usize) -> Result<(), Error> {
    if dim > 0 && dim.is_multiple_of(8) && dim <= MAX_DIM {
        Ok(())
    } else {
        Err(Error::InvalidDim(dim))
    }
}

/// Bytes in the code of one vector.
pub fn code_bytes(dim: usize, bits: u8) -> usize {
    dim / 8 * bits as usize
}

/// The packed codes and scales of `rows` in order: floats are quantised, codes copied.
pub fn encode(dim: usize, bits: u8, rows: &[Row]) -> Result<(Vec<u8>, Vec<f32>), Error> {
    validate_bits(bits)?;
    validate_dim(dim)?;
    let stride = code_bytes(dim, bits);
    let mut floats = Vec::new();
    for (row, value) in rows.iter().enumerate() {
        match *value {
            Row::Float(v) if v.len() != dim => {
                return Err(Error::RowDim {
                    row,
                    len: v.len(),
                    dim,
                })
            }
            Row::Float(v) if v.iter().any(|x| !x.is_finite()) => {
                return Err(Error::RowNotFinite { row })
            }
            Row::Float(v) => floats.extend_from_slice(v),
            Row::Code(code, _) if code.len() != stride => return Err(Error::CodeSize { row }),
            Row::Code(..) => {}
        }
    }
    let encoded = if floats.is_empty() {
        None
    } else {
        let mut index = TurboQuantIndex::new(dim, bits as usize)
            .map_err(|e| Error::Quantiser(e.to_string()))?;
        index.add(&floats);
        Some(index)
    };
    let mut codes = Vec::with_capacity(rows.len() * stride);
    let mut scales = Vec::with_capacity(rows.len());
    let mut next_float = 0;
    for row in rows {
        match *row {
            Row::Float(_) => {
                let index = encoded.as_ref().unwrap();
                codes.extend_from_slice(
                    &index.packed_codes()[next_float * stride..(next_float + 1) * stride],
                );
                scales.push(index.scales()[next_float]);
                next_float += 1;
            }
            Row::Code(code, scale) => {
                codes.extend_from_slice(code);
                scales.push(scale);
            }
        }
    }
    Ok((codes, scales))
}

/// A searchable index over encoded vectors; searches use the current rayon pool.
pub struct VectorIndex {
    index: TurboQuantIndex,
}

impl VectorIndex {
    /// Builds from the output of [`encode`], one scale per vector.
    pub fn from_codes(
        dim: usize,
        bits: u8,
        codes: Vec<u8>,
        scales: Vec<f32>,
    ) -> Result<Self, Error> {
        let index = TurboQuantIndex::from_parts(
            Some(dim),
            bits as usize,
            scales.len(),
            codes,
            scales,
            Vec::new(),
            Vec::new(),
        )
        .map_err(|e| Error::Quantiser(e.to_string()))?;
        index.prepare();
        Ok(Self { index })
    }

    /// Up to `k` `(slot, score)` by approximate inner product among slots allowed by `mask`, best first.
    pub fn search(
        &self,
        query: &[f32],
        k: usize,
        mask: Option<&[bool]>,
    ) -> Result<Vec<(usize, f32)>, Error> {
        let results = self
            .index
            .try_search_with_mask(query, k, mask)
            .map_err(|e| Error::Quantiser(e.to_string()))?;
        Ok(results
            .scores_for_query(0)
            .iter()
            .zip(results.indices_for_query(0))
            .filter(|(_, &slot)| slot >= 0)
            .map(|(&score, &slot)| (slot as usize, score))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(i: usize) -> Vec<f32> {
        (0..16).map(|k| if k == i { 1.0 } else { 0.0 }).collect()
    }

    #[test]
    fn finds_the_nearest_allowed_vector() {
        let vectors: Vec<Vec<f32>> = (0..4).map(unit).collect();
        let rows: Vec<Row> = vectors.iter().map(|v| Row::Float(v.as_slice())).collect();
        let (codes, scales) = encode(16, 4, &rows).unwrap();
        assert_eq!((codes.len(), scales.len()), (4 * code_bytes(16, 4), 4));
        let index = VectorIndex::from_codes(16, 4, codes.clone(), scales.clone()).unwrap();
        assert_eq!(index.search(&unit(2), 1, None).unwrap()[0].0, 2);
        let mask = [true, true, false, true];
        let hits = index.search(&unit(2), 4, Some(&mask[..])).unwrap();
        assert!(!hits.is_empty() && hits.iter().all(|&(slot, _)| slot != 2));

        let stride = code_bytes(16, 4);
        let carried = [Row::Code(&codes[stride..2 * stride], scales[1])];
        let (copy, copy_scales) = encode(16, 4, &carried).unwrap();
        assert_eq!(
            (&copy[..], copy_scales[0]),
            (&codes[stride..2 * stride], scales[1])
        );
    }

    #[test]
    fn rejects_invalid_shapes() {
        assert_eq!(validate_bits(5), Err(Error::InvalidBits(5)));
        assert_eq!(validate_dim(12), Err(Error::InvalidDim(12)));
        let (first, short) = (unit(0), [0.0f32; 8]);
        let rows = [Row::Float(&first), Row::Float(&short)];
        let err = encode(16, 4, &rows).unwrap_err();
        assert_eq!(
            err,
            Error::RowDim {
                row: 1,
                len: 8,
                dim: 16
            }
        );
        let nan = [f32::NAN; 16];
        let err = encode(16, 4, &[Row::Float(&nan)]).unwrap_err();
        assert_eq!(err, Error::RowNotFinite { row: 0 });
        let err = encode(16, 4, &[Row::Code(&[0u8; 3], 1.0)]).unwrap_err();
        assert_eq!(err, Error::CodeSize { row: 0 });
    }
}
