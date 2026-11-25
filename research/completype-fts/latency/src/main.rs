use std::hint::black_box;
use std::io::{self, BufRead};
use std::time::{Duration, Instant};

use completype_fts::Builder;
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, EmptyQuery, Occur, Query, TermQuery};
use tantivy::schema::{IndexRecordOption, Schema, TextFieldIndexing, TextOptions};
use tantivy::tokenizer::{TextAnalyzer, WhitespaceTokenizer};
use tantivy::{doc, Index, Term};

const INDEX_BUDGET: usize = 500_000_000;
const DEFAULT_ROUNDS: usize = 21;
const TOKENIZER: &str = "pretokenized";

fn measure_pass(order: &[usize], durations: &mut Vec<Duration>, search: impl Fn(usize) -> usize) {
    for &i in order {
        let start = Instant::now();
        search(i);
        durations.push(start.elapsed());
    }
}

fn quantiles(samples: &mut [Duration]) -> [f64; 3] {
    samples.sort_unstable();
    [50, 95, 99].map(|p| {
        let rank = (samples.len() * p).div_ceil(100);
        samples[rank - 1].as_secs_f64() * 1_000_000.0
    })
}

fn shuffle(order: &mut [usize], state: &mut u64) {
    for i in (1..order.len()).rev() {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        order.swap(i, (*state % (i as u64 + 1)) as usize);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let rounds = args.next().map_or(Ok(DEFAULT_ROUNDS), |arg| arg.parse())?;
    if rounds == 0 {
        return Err("rounds must be positive".into());
    }
    let stdin = io::stdin().lock();
    let mut lines = stdin.lines();
    let mut builder = Builder::default();
    let mut schema = Schema::builder();
    let indexing = TextFieldIndexing::default()
        .set_tokenizer(TOKENIZER)
        .set_index_option(IndexRecordOption::WithFreqs);
    let body = schema.add_text_field(
        "body",
        TextOptions::default().set_indexing_options(indexing),
    );
    let tantivy = Index::create_in_ram(schema.build());
    tantivy.tokenizers().register(
        TOKENIZER,
        TextAnalyzer::builder(WhitespaceTokenizer::default()).build(),
    );

    let mut writer = tantivy.writer_with_num_threads(1, INDEX_BUDGET)?;
    let mut documents = 0;
    loop {
        let line = lines.next().ok_or("missing end of corpus")??;
        if line.is_empty() {
            break;
        }
        let (id, text) = line
            .split_once('\t')
            .ok_or("expected document id and terms")?;
        builder.add(id.to_owned(), text.split_ascii_whitespace());
        writer.add_document(doc!(body => text))?;
        documents += 1;
    }
    let index = builder.finish();
    writer.commit()?;
    writer.wait_merging_threads()?;
    let reader = tantivy.reader()?;
    let searcher = reader.searcher();
    assert_eq!(searcher.num_docs(), documents);

    let queries: Vec<_> = lines.collect::<io::Result<_>>()?;
    if queries.is_empty() {
        return Err("expected at least one query".into());
    }
    let weighted: Vec<Vec<_>> = queries
        .iter()
        .map(|line| {
            line.split_ascii_whitespace()
                .map(|word| (word, 1.0))
                .collect()
        })
        .collect();
    let tantivy_queries: Vec<Box<dyn Query>> = weighted
        .iter()
        .map(|query| {
            if query.is_empty() {
                return Box::new(EmptyQuery) as Box<dyn Query>;
            }
            let clauses = query
                .iter()
                .map(|&(word, _)| {
                    let term = Term::from_field_text(body, word);
                    let clause = TermQuery::new(term, IndexRecordOption::WithFreqs);
                    (Occur::Should, Box::new(clause) as Box<dyn Query>)
                })
                .collect();
            Box::new(BooleanQuery::new(clauses))
        })
        .collect();

    println!(
        "{documents} documents, {} queries, {} Tantivy segments, {rounds} measured passes per k",
        queries.len(),
        searcher.segment_readers().len(),
    );
    println!("| k | engine | p50 µs | p95 µs | p99 µs |");
    println!("|---:|---|---:|---:|---:|");
    for k in [10, 100] {
        let collector = TopDocs::with_limit(k).order_by_score();
        let run_fts = |i: usize| black_box(index.search(weighted[i].iter().copied(), k)).len();
        let run_tantivy = |i: usize| {
            black_box(
                searcher
                    .search(tantivy_queries[i].as_ref(), &collector)
                    .expect("search"),
            )
            .len()
        };
        for (i, terms) in weighted.iter().enumerate() {
            let reference = index.search_exhaustive(terms.iter().copied(), k);
            assert_eq!(
                index.search(terms.iter().copied(), k),
                reference,
                "completype-fts ranking for query {i}"
            );
            assert_eq!(
                reference.len(),
                run_tantivy(i),
                "Tantivy hit count for query {i}"
            );
        }
        let mut order: Vec<_> = (0..queries.len()).collect();
        let mut state = 7;
        let engines: [(&str, &dyn Fn(usize) -> usize); 2] = [
            ("completype-fts", &run_fts),
            ("Tantivy 0.27.0", &run_tantivy),
        ];
        let mut samples = [Vec::new(), Vec::new()];
        for round in 0..rounds {
            shuffle(&mut order, &mut state);
            for offset in 0..engines.len() {
                let i = (round + offset) % engines.len();
                measure_pass(&order, &mut samples[i], engines[i].1);
            }
        }
        for ((engine, _), mut samples) in engines.into_iter().zip(samples) {
            let [p50, p95, p99] = quantiles(&mut samples);
            println!("| {k} | {engine} | {p50:.1} | {p95:.1} | {p99:.1} |");
        }
    }
    Ok(())
}
