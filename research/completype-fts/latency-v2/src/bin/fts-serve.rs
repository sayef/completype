//! Builds a completype-fts index from `id\ttext` lines on stdin, then answers one query per line.

use std::io::{self, BufRead, Write};

use completype_fts::{Analyzer, Index, Language, StopWords};

fn main() -> io::Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("index directory");
    let typos = args.next().as_deref() != Some("exact");
    let reorder = args.next().as_deref() == Some("reorder");
    let any_first = args.next().as_deref() == Some("any-first");
    let language = args.next().unwrap_or_default();
    let mut index = Index::open(&dir)?.with_reordering(if reorder { completype_fts::Reordering::Always } else { completype_fts::Reordering::Never }).with_first_letter_typos(any_first);
    if language.contains("stem") {
        index = index.with_analyzer(Analyzer::default().stem(Language::English));
    }
    if language.contains("stop") {
        index = index.with_stop_words(StopWords::snowball(Language::English).expect("English list"));
    }
    let mut writer = index.writer()?;
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    for line in lines.by_ref() {
        let line = line?;
        if line.is_empty() {
            break;
        }
        let (id, text) = line.split_once('\t').expect("id and text");
        writer.add(id, text);
    }
    writer.commit()?;
    drop(writer);
    let reader = index.reader()?;
    let mut out = io::BufWriter::new(io::stdout().lock());
    writeln!(out, "READY")?;
    out.flush()?;
    for line in lines {
        let line = line?;
        let hits = if typos { reader.search(&line, 100) } else { reader.search_exact(&line, 100) };
        let ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
        writeln!(out, "{}", ids.join("\t"))?;
        out.flush()?;
    }
    Ok(())
}
