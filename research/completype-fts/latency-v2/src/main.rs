use std::hint::black_box;
use std::io::{self, BufRead};
use std::path::Path;
use std::time::{Duration, Instant};

use completype_fts::{Searcher, Segment, SegmentBuilder};
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, Query, TermQuery};
use tantivy::schema::{IndexRecordOption, Schema, TextFieldIndexing, TextOptions};
use tantivy::tokenizer::{TextAnalyzer, WhitespaceTokenizer};
use tantivy::{doc, Index, Term};

fn quantiles(samples: &mut [Duration]) -> [f64; 3] {
    samples.sort_unstable();
    [50, 95, 99].map(|p| samples[(samples.len() * p).div_ceil(100) - 1].as_secs_f64() * 1e6)
}

fn shuffle(order: &mut [usize], state: &mut u64) {
    for i in (1..order.len()).rev() {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        order.swap(i, (*state % (i as u64 + 1)) as usize);
    }
}

fn dir_size(path: &Path) -> u64 {
    std::fs::read_dir(path).unwrap().map(|e| e.unwrap().metadata().unwrap().len()).sum()
}

fn rss_mib() -> f64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|v| v.trim().trim_end_matches(" kB").trim().parse::<f64>().ok())
        .map_or(0.0, |kb| kb / 1024.0)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let rounds: usize = args.next().map_or(Ok(4), |a| a.parse())?;
    let work = std::path::PathBuf::from(args.next().ok_or("work directory")?);
    let engines: Vec<String> = args.next().unwrap_or("new,poc,tantivy".into()).split(',').map(String::from).collect();
    std::fs::create_dir_all(&work)?;
    let mut lines = io::stdin().lock().lines();
    let mut corpus = Vec::new();
    loop {
        let line = lines.next().ok_or("missing end of corpus")??;
        if line.is_empty() {
            break;
        }
        let (id, text) = line.split_once('\t').ok_or("expected id and terms")?;
        corpus.push((id.to_owned(), text.to_owned()));
    }
    let queries: Vec<String> = lines.collect::<io::Result<_>>()?;
    let weighted: Vec<Vec<(&str, f32)>> =
        queries.iter().map(|q| q.split_ascii_whitespace().map(|w| (w, 1.0)).collect()).collect();
    println!("{} documents, {} queries, {rounds} passes per k", corpus.len(), queries.len());

    let base_rss = rss_mib();
    let start = Instant::now();
    let segment_path = work.join("new.seg");
    let mut builder = SegmentBuilder::default();
    for (id, text) in &corpus {
        builder.add(id.as_str(), text.split_ascii_whitespace());
    }
    builder.write(&segment_path)?;
    let segment = Segment::open(&segment_path)?;
    let new_build = start.elapsed();
    let new_bytes = std::fs::metadata(&segment_path)?.len();
    println!("new: build {:.1} s, segment {:.1} MiB", new_build.as_secs_f64(), new_bytes as f64 / 1048576.0);
    let ids: usize = segment.section_sizes()[1..3].iter().map(|s| s.1).sum();
    println!("  {:.1} MiB without ids", (new_bytes as usize - ids) as f64 / 1048576.0);

    let bp = engines.iter().any(|e| e == "bp").then(|| {
        let start = Instant::now();
        let path = work.join("bp.seg");
        let mut builder = SegmentBuilder::default();
        builder.reorder(true);
        for (id, text) in &corpus {
            builder.add(id.as_str(), text.split_ascii_whitespace());
        }
        builder.write(&path).unwrap();
        let segment = Segment::open(&path).unwrap();
        let ids: usize = segment.section_sizes()[1..3].iter().map(|s| s.1).sum();
        println!(
            "bp: build {:.1} s, segment {:.1} MiB, {:.1} MiB without ids",
            start.elapsed().as_secs_f64(),
            std::fs::metadata(&path).unwrap().len() as f64 / 1048576.0,
            (std::fs::metadata(&path).unwrap().len() as usize - ids) as f64 / 1048576.0
        );
        std::fs::remove_file(&path).unwrap();
        segment
    });
    let poc = engines.iter().any(|e| e == "poc").then(|| {
        let before = rss_mib();
        let start = Instant::now();
        let mut b = completype_fts_poc::Builder::default();
        for (id, text) in &corpus {
            b.add(id.clone(), text.split_ascii_whitespace());
        }
        let index = b.finish();
        println!("poc: build {:.1} s, +{:.0} MiB RSS", start.elapsed().as_secs_f64(), rss_mib() - before);
        index
    });

    let tantivy_dir = work.join("tantivy");
    let _ = std::fs::remove_dir_all(&tantivy_dir);
    std::fs::create_dir_all(&tantivy_dir)?;
    let mut schema = Schema::builder();
    let indexing = TextFieldIndexing::default().set_tokenizer("ws").set_index_option(IndexRecordOption::WithFreqs);
    let body = schema.add_text_field("body", TextOptions::default().set_indexing_options(indexing));
    let start = Instant::now();
    let tantivy = Index::create_in_dir(&tantivy_dir, schema.build())?;
    tantivy.tokenizers().register("ws", TextAnalyzer::builder(WhitespaceTokenizer::default()).build());
    let mut writer = tantivy.writer_with_num_threads(1, 500_000_000)?;
    for (_, text) in &corpus {
        writer.add_document(doc!(body => text.as_str()))?;
    }
    writer.commit()?;
    let ids = tantivy.searchable_segment_ids()?;
    if ids.len() > 1 {
        writer.merge(&ids).wait()?;
    }
    writer.garbage_collect_files().wait()?;
    writer.wait_merging_threads()?;
    let reader = tantivy.reader()?;
    let searcher = reader.searcher();
    println!(
        "tantivy: build+merge {:.1} s, {} segment(s), index {:.1} MiB",
        start.elapsed().as_secs_f64(),
        searcher.segment_readers().len(),
        dir_size(&tantivy_dir) as f64 / 1048576.0
    );
    drop(corpus);
    println!("process RSS after building everything: {:.0} MiB (start {base_rss:.0})", rss_mib());
    let tantivy_queries: Vec<Box<dyn Query>> = weighted
        .iter()
        .map(|q| {
            let clauses = q
                .iter()
                .map(|&(w, _)| {
                    let t = TermQuery::new(Term::from_field_text(body, w), IndexRecordOption::WithFreqs);
                    (Occur::Should, Box::new(t) as Box<dyn Query>)
                })
                .collect();
            Box::new(BooleanQuery::new(clauses)) as Box<dyn Query>
        })
        .collect();

    let mut new_searcher = Searcher::new(&segment);
    let mut bp_searcher = bp.as_ref().map(Searcher::new);
    if std::env::var_os("STATS").is_some() {
        let mut rows = Vec::new();
        for q in &weighted {
            let t = Instant::now();
            new_searcher.search(q.iter().copied(), 10);
            let micros = t.elapsed().as_micros();
            let st = new_searcher.stats();
            let bp_stats = bp_searcher.as_mut().map(|s| {
                let t = Instant::now();
                s.search(q.iter().copied(), 10);
                (t.elapsed().as_micros(), s.stats().windows_touched, s.stats().windows_scored)
            });
            rows.push((micros, st.headers_read, st.windows_touched, st.windows_scored, bp_stats, q.len(), st.postings_scored, st.scan_micros, st.score_micros));
        }
        rows.sort_by_key(|r| std::cmp::Reverse(r.0));
        println!("slowest k=10: µs, headers, touched, scored, (bp µs, touched, scored), terms, postings, scan µs, score µs");
        for r in rows.iter().take(12) {
            println!("{r:?}");
        }
        let n = rows.len();
        let mid = &rows[n / 2];
        println!("median-ish: {mid:?}");
    }
    println!("| k | engine | p50 µs | p95 µs | p99 µs |\n|---:|---|---:|---:|---:|");
    for k in [10, 100] {
        let mut checked = 0;
        for q in &weighted {
            let pruned = new_searcher.search(q.iter().copied(), k);
            assert_eq!(pruned, new_searcher.search_exhaustive(q.iter().copied(), k), "query {q:?}");
            checked += pruned.len();
            if let Some(s) = bp_searcher.as_mut() {
                assert_eq!(s.search(q.iter().copied(), k), s.search_exhaustive(q.iter().copied(), k));
            }
        }
        let collector = TopDocs::with_limit(k).order_by_score();
        let mut runs: Vec<(&str, Box<dyn FnMut(usize) -> usize + '_>)> = Vec::new();
        if engines.iter().any(|e| e == "new") {
            let s = &mut new_searcher;
            let w = &weighted;
            runs.push(("completype-fts", Box::new(move |i| black_box(s.search(w[i].iter().copied(), k)).len())));
        }
        if let Some(s) = bp_searcher.as_mut() {
            let w = &weighted;
            runs.push(("completype-fts reordered", Box::new(move |i| black_box(s.search(w[i].iter().copied(), k)).len())));
        }
        if let Some(index) = &poc {
            let w = &weighted;
            runs.push(("poc", Box::new(move |i| black_box(index.search(w[i].iter().map(|&(t, x)| (t, f64::from(x))), k)).len())));
        }
        if engines.iter().any(|e| e == "tantivy") {
            let (s, q, c) = (&searcher, &tantivy_queries, &collector);
            runs.push(("tantivy 0.27", Box::new(move |i| black_box(s.search(q[i].as_ref(), c).unwrap()).len())));
        }
        let mut samples: Vec<Vec<Duration>> = runs.iter().map(|_| Vec::new()).collect();
        let mut order: Vec<usize> = (0..weighted.len()).collect();
        let mut state = 7;
        for round in 0..rounds {
            shuffle(&mut order, &mut state);
            for offset in 0..runs.len() {
                let e = (round + offset) % runs.len();
                for &i in &order {
                    let t = Instant::now();
                    (runs[e].1)(i);
                    samples[e].push(t.elapsed());
                }
            }
        }
        for ((name, _), mut s) in runs.into_iter().zip(samples) {
            let [p50, p95, p99] = quantiles(&mut s);
            println!("| {k} | {name} | {p50:.1} | {p95:.1} | {p99:.1} |");
        }
        let _ = checked;
    }
    let _ = std::fs::remove_dir_all(&tantivy_dir);
    let _ = std::fs::remove_file(&segment_path);
    Ok(())
}
