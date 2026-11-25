use std::io::{self, BufRead, Write};

use completype_fts::Builder;

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn main() -> io::Result<()> {
    let interactive = std::env::args().any(|arg| arg == "--interactive");
    let stdin = io::stdin().lock();
    let mut lines = stdin.lines();
    let mut builder = Builder::default();
    loop {
        let line = lines
            .next()
            .ok_or_else(|| invalid("missing end of corpus"))??;
        if line.is_empty() {
            break;
        }
        let (id, text) = line
            .split_once('\t')
            .ok_or_else(|| invalid("expected document id and terms"))?;
        builder.add(id.to_owned(), text.split_ascii_whitespace());
    }

    let index = builder.finish();
    let mut out = io::BufWriter::new(io::stdout().lock());
    if interactive {
        out.write_all(b"READY\n")?;
        out.flush()?;
    }
    for line in lines {
        let line = line?;
        let query = line
            .split_ascii_whitespace()
            .map(|item| {
                let (term, weight) = item
                    .rsplit_once(':')
                    .ok_or_else(|| invalid("expected term:weight"))?;
                let weight: f64 = weight.parse().map_err(|_| invalid("invalid weight"))?;
                if !weight.is_finite() || weight <= 0.0 {
                    return Err(invalid("weight must be finite and positive"));
                }
                Ok((term, weight))
            })
            .collect::<io::Result<Vec<_>>>()?;
        let hits = index.search(query, 100);
        for (rank, hit) in hits.into_iter().enumerate() {
            if rank > 0 {
                out.write_all(b"\t")?;
            }
            out.write_all(hit.id.as_bytes())?;
        }
        out.write_all(b"\n")?;
        if interactive {
            out.flush()?;
        }
    }
    out.flush()
}
