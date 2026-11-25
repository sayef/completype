//! The vector section of a segment: codes and scales by local id, searched under a live mask.
//! Codes from different segments are compatible, so compaction concatenates them.

use std::cell::OnceCell;
use std::sync::{Arc, Mutex, OnceLock};

pub(crate) use completype_vector::Row;
use completype_vector::{code_bytes, VectorIndex};
use rustc_hash::FxHashMap;

use crate::codec::{Column, Reader, Writer};
use crate::Error;

/// Vector codes carried from other segments into a merged one: `id -> (code, scale)`.
pub(crate) type CarriedCodes = FxHashMap<u64, (Vec<u8>, f32)>;

pub(crate) struct Vectors {
    codes: Column<u8>,
    scales: Column<f32>,
    /// Built from the codes on the first search, as it copies them.
    index: OnceLock<Result<VectorIndex, String>>,
    dim: usize,
    bits: u8,
    /// Local id of each slot, ascending.
    slot_locals: Column<u32>,
}

pub(crate) fn validate_bits(bits: u8) -> Result<(), Error> {
    completype_vector::validate_bits(bits).map_err(|e| Error::input(e.to_string()))
}

/// The error of encoding the rows of `locals`, naming the local of a bad row.
fn encode_error(e: completype_vector::Error, locals: &[u32]) -> Error {
    use completype_vector::Error as E;
    Error::input(match e {
        E::RowDim { row, len, dim } => {
            format!(
                "vector of local {} has {len} dimensions, not {dim}",
                locals[row]
            )
        }
        E::RowNotFinite { row } => format!("vector of local {} is not finite", locals[row]),
        e => e.to_string(),
    })
}

impl Vectors {
    /// Writes the vector section; `rows` are `(local, row)` in ascending local order.
    pub(crate) fn write(
        w: &mut Writer,
        dim: usize,
        bits: u8,
        rows: &[(u32, Row)],
    ) -> Result<(), Error> {
        if rows.is_empty() {
            w.raw(&[0]);
            return Ok(());
        }
        let (locals, values): (Vec<u32>, Vec<Row>) = rows.iter().copied().unzip();
        let (codes, scales) =
            completype_vector::encode(dim, bits, &values).map_err(|e| encode_error(e, &locals))?;
        w.raw(&[1, bits]);
        w.u64(dim as u64);
        w.column(&locals);
        w.column(&codes);
        w.column(&scales);
        Ok(())
    }

    pub(crate) fn read(r: &mut Reader, docs: usize, full: bool) -> Result<Option<Self>, Error> {
        if r.u8()? == 0 {
            r.align()?;
            return Ok(None);
        }
        let bits = r.u8()?;
        let dim = usize::try_from(r.u64()?)
            .map_err(|_| Error::Corrupt("vector dimension overflow".into()))?;
        let slot_locals: Column<u32> = r.column()?;
        let codes: Column<u8> = r.column()?;
        let scales: Column<f32> = r.column()?;
        let bad = |what: &str| Error::Corrupt(format!("invalid vectors: {what}"));
        completype_vector::validate_bits(bits).map_err(|_| bad("bits"))?;
        completype_vector::validate_dim(dim).map_err(|_| bad("dimension"))?;
        let n = slot_locals.len();
        let locals = slot_locals.as_slice();
        if scales.len() != n
            || codes.len() != n * code_bytes(dim, bits)
            || locals.last().is_some_and(|&l| l as usize >= docs)
            || full
                && (locals.windows(2).any(|w| w[0] >= w[1])
                    || scales.as_slice().iter().any(|s| !s.is_finite()))
        {
            return Err(bad("sizes"));
        }
        Ok(Some(Self {
            codes,
            scales,
            index: OnceLock::new(),
            dim,
            bits,
            slot_locals,
        }))
    }

    pub(crate) fn dim(&self) -> usize {
        self.dim
    }

    pub(crate) fn bits(&self) -> u8 {
        self.bits
    }

    pub(crate) fn locals(&self) -> &[u32] {
        self.slot_locals.as_slice()
    }

    pub(crate) fn size(&self) -> usize {
        self.slot_locals.len() * (4 + 4 + code_bytes(self.dim, self.bits))
    }

    /// The stored code and scale of `local`, if it has a vector.
    pub(crate) fn row(&self, local: u32) -> Option<(&[u8], f32)> {
        let slot = self.locals().binary_search(&local).ok()?;
        let stride = code_bytes(self.dim, self.bits);
        Some((
            self.codes
                .as_slice()
                .get(slot * stride..(slot + 1) * stride)?,
            *self.scales.as_slice().get(slot)?,
        ))
    }

    /// Up to `k` `(local, score)` among slots allowed by `mask`, best first.
    pub(crate) fn search(
        &self,
        query: &[f32],
        k: usize,
        mask: &[bool],
        threads: usize,
    ) -> Result<Vec<(u32, f32)>, Error> {
        let results = in_pool(threads, || {
            self.index().and_then(|index| {
                index
                    .search(query, k, Some(mask))
                    .map_err(|e| Error::input(e.to_string()))
            })
        })?;
        let locals = self.locals();
        Ok(results
            .into_iter()
            .map(|(slot, score)| (locals[slot], score))
            .collect())
    }

    /// The search index, built from the codes on first use.
    pub(crate) fn index(&self) -> Result<&VectorIndex, Error> {
        self.index
            .get_or_init(|| {
                VectorIndex::from_codes(
                    self.dim,
                    self.bits,
                    self.codes.as_slice().to_vec(),
                    self.scales.as_slice().to_vec(),
                )
                .map_err(|e| e.to_string())
            })
            .as_ref()
            .map_err(|e| Error::Corrupt(format!("invalid vectors: {e}")))
    }
}

thread_local! {
    /// A one-thread pool on the calling thread itself, so a query runs inline without a hand-off.
    static INLINE: OnceCell<rayon::ThreadPool> = const { OnceCell::new() };
}

/// Runs `f` inline (`threads == 1`), on rayon's global pool (`0`), or on a shared pool of `threads`.
pub(crate) fn in_pool<R: Send>(threads: usize, f: impl FnOnce() -> R + Send) -> R {
    match threads {
        0 => crate::threads::global(f),
        1 => INLINE.with(|cell| {
            let pool = cell.get_or_init(|| {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(1)
                    .use_current_thread()
                    .build()
                    .expect("failed to create the inline vector search pool")
            });
            pool.install(f)
        }),
        n => {
            // Pools per process: a forked child's copies have no threads, so they are leaked.
            type Pools = (u32, FxHashMap<usize, Arc<rayon::ThreadPool>>);
            static POOLS: OnceLock<Mutex<Pools>> = OnceLock::new();
            let pid = std::process::id();
            let mut pools = POOLS
                .get_or_init(|| Mutex::new((pid, FxHashMap::default())))
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if pools.0 != pid {
                std::mem::forget(std::mem::take(&mut pools.1));
                pools.0 = pid;
            }
            let pool = pools
                .1
                .entry(n)
                .or_insert_with(|| {
                    Arc::new(
                        rayon::ThreadPoolBuilder::new()
                            .num_threads(n)
                            .build()
                            .expect("failed to create a vector search pool"),
                    )
                })
                .clone();
            drop(pools);
            pool.install(f)
        }
    }
}
