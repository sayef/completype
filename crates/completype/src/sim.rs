//! Deterministic simulation: concurrent writers and a compactor over a store that injects faults,
//! with every version checked against a model. A failing seed replays with `COMPLETYPE_SIM_SEED`.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::{ObjectStore, PutMode, PutOptions, PutPayload};

use crate::{CompactionPolicy, Database, Document, Error, Store};

/// splitmix64: small, seedable and the same on every platform.
#[derive(Clone, Debug)]
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub(crate) fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub(crate) fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next() % n
        }
    }

    pub(crate) fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

thread_local! {
    static JITTER: RefCell<Option<Rng>> = const { RefCell::new(None) };
}

/// A draw below `n` from the simulation's seeded source, when one runs on this thread.
pub(crate) fn draw(n: u64) -> Option<u64> {
    JITTER.with(|j| j.borrow_mut().as_mut().map(|rng| rng.below(n)))
}

/// How [`FaultStore`] answers a write.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Fault {
    /// Writes, then reports an error, as a timed-out request that landed.
    LandThenFail,
    /// Writes, then reports that the object exists, as a client retry of a landed request.
    LandThenExists,
    /// Fails without writing.
    FailBefore,
}

/// An in-memory store that injects scripted faults into create-only writes, or seeded random ones
/// into every kind of request, and records the requests it answers.
#[derive(Debug, Default)]
pub(crate) struct FaultStore {
    inner: InMemory,
    script: Mutex<Vec<Fault>>,
    random: Mutex<Option<Rng>>,
    pub(crate) failing_reads: AtomicBool,
    /// Reads still to fail, each once.
    pub(crate) read_faults: AtomicUsize,
    /// Treats create-only writes as plain ones, as some S3-compatible stores do.
    pub(crate) ignore_create: AtomicBool,
    /// Requests answered before the simulated process crashes and every later one fails.
    pub(crate) crash_after: Mutex<Option<usize>>,
    requests: AtomicUsize,
    /// Added to every request.
    pub(crate) latency_ms: std::sync::atomic::AtomicU64,
    trace: Mutex<Vec<String>>,
}

impl FaultStore {
    /// Faults applied to the next create-only writes, in order.
    pub(crate) fn script(&self, faults: &[Fault]) {
        *self.script.lock().unwrap() = faults.iter().rev().copied().collect();
    }

    pub(crate) fn random(seed: u64) -> Arc<Self> {
        let store = Self::default();
        *store.random.lock().unwrap() = Some(Rng::new(seed));
        Arc::new(store)
    }

    pub(crate) fn database(self: &Arc<Self>) -> Database {
        Database::new(Store::from_object_store(self.clone()))
    }

    /// The requests answered so far, as `"<kind> <key>"`, and forgets them.
    pub(crate) fn take_trace(&self) -> Vec<String> {
        std::mem::take(&mut *self.trace.lock().unwrap())
    }

    fn calm(&self) {
        *self.random.lock().unwrap() = None;
    }

    fn roll(&self, percent: u64) -> bool {
        self.random
            .lock()
            .unwrap()
            .as_mut()
            .is_some_and(|rng| rng.chance(percent))
    }

    /// A simulated delay, so that concurrent actors interleave differently per seed.
    async fn stall(&self) {
        let ms = self
            .random
            .lock()
            .unwrap()
            .as_mut()
            .map_or(0, |rng| rng.below(20))
            + self.latency_ms.load(Ordering::SeqCst);
        if ms > 0 {
            tokio::time::sleep(Duration::from_millis(ms)).await;
        }
    }

    /// Counts a request; true once the simulated process has crashed.
    fn crashed(&self) -> bool {
        let n = self.requests.fetch_add(1, Ordering::SeqCst);
        self.crash_after
            .lock()
            .unwrap()
            .is_some_and(|limit| n >= limit)
    }

    /// Crashes the process after `requests` more requests, or, with `None`, starts a new one.
    pub(crate) fn crash_in(&self, requests: Option<usize>) {
        let now = self.requests.load(Ordering::SeqCst);
        *self.crash_after.lock().unwrap() = requests.map(|r| now + r);
    }

    pub(crate) fn has_crashed(&self) -> bool {
        let now = self.requests.load(Ordering::SeqCst);
        self.crash_after
            .lock()
            .unwrap()
            .is_some_and(|limit| now > limit)
    }

    fn record(&self, op: &str, location: &Path) {
        // Segment names are random; only their kind matters for replay.
        let key = location.as_ref();
        let key = match key.split_once('/') {
            Some((kind @ ("segments" | "_probe" | "_clock" | "_inbox"), _)) => format!("{kind}/*"),
            _ => key.to_owned(),
        };
        self.trace.lock().unwrap().push(format!("{op} {key}"));
    }

    fn error() -> object_store::Error {
        object_store::Error::Generic {
            store: "FaultStore",
            source: "request timed out".into(),
        }
    }

    fn next_fault(&self, create: bool) -> Option<Fault> {
        if create {
            if let Some(fault) = self.script.lock().unwrap().pop() {
                return Some(fault);
            }
        }
        let mut random = self.random.lock().unwrap();
        let rng = random.as_mut()?;
        match rng.below(100) {
            0..=3 => Some(Fault::FailBefore),
            4..=7 => Some(Fault::LandThenFail),
            8..=11 if create => Some(Fault::LandThenExists),
            _ => None,
        }
    }
}

impl std::fmt::Display for FaultStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FaultStore")
    }
}

#[async_trait::async_trait]
impl ObjectStore for FaultStore {
    async fn put_opts(
        &self,
        location: &Path,
        payload: PutPayload,
        mut opts: PutOptions,
    ) -> object_store::Result<object_store::PutResult> {
        self.stall().await;
        if self.crashed() {
            return Err(Self::error());
        }
        if self.ignore_create.load(Ordering::SeqCst) {
            opts.mode = PutMode::Overwrite;
        }
        self.record("put", location);
        match self.next_fault(matches!(opts.mode, PutMode::Create)) {
            None => self.inner.put_opts(location, payload, opts).await,
            Some(Fault::FailBefore) => Err(Self::error()),
            Some(Fault::LandThenFail) => {
                self.inner.put_opts(location, payload, opts).await?;
                Err(Self::error())
            }
            Some(Fault::LandThenExists) => {
                self.inner
                    .put_opts(location, payload.clone(), opts.clone())
                    .await?;
                self.inner.put_opts(location, payload, opts).await
            }
        }
    }

    async fn put_multipart_opts(
        &self,
        location: &Path,
        opts: object_store::PutMultipartOptions,
    ) -> object_store::Result<Box<dyn object_store::MultipartUpload>> {
        self.inner.put_multipart_opts(location, opts).await
    }

    async fn get_opts(
        &self,
        location: &Path,
        options: object_store::GetOptions,
    ) -> object_store::Result<object_store::GetResult> {
        self.stall().await;
        if self.crashed() {
            return Err(Self::error());
        }
        self.record(if options.head { "head" } else { "get" }, location);
        let scripted = self.read_faults.load(Ordering::SeqCst) > 0
            && self.read_faults.fetch_sub(1, Ordering::SeqCst) > 0;
        if scripted || self.failing_reads.load(Ordering::SeqCst) || self.roll(4) {
            return Err(Self::error());
        }
        self.inner.get_opts(location, options).await
    }

    fn delete_stream(
        &self,
        locations: futures::stream::BoxStream<'static, object_store::Result<Path>>,
    ) -> futures::stream::BoxStream<'static, object_store::Result<Path>> {
        self.trace.lock().unwrap().push("delete".into());
        if self.crashed() {
            return locations.map(|_| Err(Self::error())).boxed();
        }
        self.inner.delete_stream(locations)
    }

    fn list(
        &self,
        prefix: Option<&Path>,
    ) -> futures::stream::BoxStream<'static, object_store::Result<object_store::ObjectMeta>> {
        self.record("list", prefix.unwrap_or(&Path::default()));
        if self.crashed() {
            return futures::stream::once(async { Err(Self::error()) }).boxed();
        }
        let listing = self.inner.list(prefix);
        // A listing cut short, as an eventually consistent or paginated store may return.
        if self.roll(4) {
            let keep = self
                .random
                .lock()
                .unwrap()
                .as_mut()
                .map_or(0, |r| r.below(3));
            return listing.take(keep as usize).boxed();
        }
        listing
    }

    async fn list_with_delimiter(
        &self,
        prefix: Option<&Path>,
    ) -> object_store::Result<object_store::ListResult> {
        self.inner.list_with_delimiter(prefix).await
    }

    async fn copy_opts(
        &self,
        from: &Path,
        to: &Path,
        options: object_store::CopyOptions,
    ) -> object_store::Result<()> {
        self.inner.copy_opts(from, to, options).await
    }
}

const INDEX: &str = "songs";
const TAG: &str = "sim.tag";

/// What a writer's commit changed, and how it ended.
#[derive(Debug)]
struct Commit {
    adds: Vec<u64>,
    deletes: Vec<u64>,
    /// The version it reported, or `None` if its outcome is unknown; absent if it failed.
    outcome: Option<Option<u64>>,
}

type Log = Rc<RefCell<BTreeMap<String, Commit>>>;

async fn writer(database: Database, writer: u64, commits: u64, seed: u64, log: Log) {
    let mut rng = Rng::new(seed);
    for k in 0..commits {
        tokio::time::sleep(Duration::from_millis(rng.below(30))).await;
        let tag = format!("{writer}:{k}");
        let adds: Vec<u64> = (0..=rng.below(3))
            .map(|j| writer * 10_000 + k * 10 + j)
            .collect();
        let acked: Vec<u64> = log
            .borrow()
            .values()
            .filter(|c| matches!(c.outcome, Some(Some(_))))
            .flat_map(|c| c.adds.clone())
            .collect();
        let deletes: Vec<u64> = if !acked.is_empty() && rng.chance(40) {
            vec![acked[rng.below(acked.len() as u64) as usize]]
        } else {
            Vec::new()
        };
        let docs = adds
            .iter()
            .map(|&id| Document::new(id, format!("song {id}"), 0.5));
        let outcome = async {
            let mut txn = database.begin().await?;
            txn.append_documents(INDEX, docs, deletes.clone())?
                .set_metadata(TAG, Some(&tag));
            txn.commit().await
        }
        .await;
        let outcome = match outcome {
            Ok(manifest) => Some(Some(manifest.version)),
            Err(Error::OutcomeUnknown(_)) => Some(None),
            Err(_) => None,
        };
        log.borrow_mut().insert(
            tag,
            Commit {
                adds,
                deletes,
                outcome,
            },
        );
    }
}

async fn compactor(database: Database, rounds: u64, seed: u64) {
    let mut rng = Rng::new(seed);
    let policy = CompactionPolicy::default()
        .fanout(2)
        .max_segments(4)
        .max_hidden_fraction(0.2);
    for _ in 0..rounds {
        tokio::time::sleep(Duration::from_millis(rng.below(80))).await;
        // Compaction may lose races or hit faults; the model only checks that it changes nothing.
        let _ = database
            .namespace("default")
            .unwrap()
            .compact_all(INDEX, &policy)
            .await;
    }
}

/// Runs one seed; returns the store's request trace, or what the model found wrong.
pub(crate) fn run(seed: u64) -> Result<Vec<String>, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap();
    JITTER.with(|j| *j.borrow_mut() = Some(Rng::new(seed ^ 0x5EED)));
    let store = FaultStore::random(seed);
    let database = store.database();
    let log: Log = Rc::default();
    let mut rng = Rng::new(seed.wrapping_mul(31));
    let writers = 2 + rng.below(3);
    let result = runtime.block_on(async {
        let mut actors: Vec<std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>> = Vec::new();
        for w in 0..writers {
            let commits = 3 + rng.below(4);
            actors.push(Box::pin(writer(
                database.clone(),
                w,
                commits,
                rng.next(),
                log.clone(),
            )));
        }
        actors.push(Box::pin(compactor(database.clone(), 3, rng.next())));
        futures::future::join_all(actors).await;
        store.calm();
        let log = std::mem::take(&mut *log.borrow_mut());
        check(&database, &log).await
    });
    JITTER.with(|j| *j.borrow_mut() = None);
    result.map(|()| store.trace.lock().unwrap().clone())
}

/// Every version follows its parent, no commit lands twice, acknowledged and failed commits are
/// where they say, and each version holds exactly the documents its commits leave.
async fn check(database: &Database, log: &BTreeMap<String, Commit>) -> Result<(), String> {
    let versions = database.versions().await.map_err(|e| e.to_string())?;
    if versions != (1..=versions.len() as u64).collect::<Vec<_>>() {
        return Err(format!("versions are not contiguous: {versions:?}"));
    }
    let mut commit_ids = BTreeSet::new();
    let mut landed: BTreeMap<&str, u64> = BTreeMap::new();
    let mut model: BTreeSet<u64> = BTreeSet::new();
    let mut tag: Option<String> = None;
    for &version in &versions {
        let manifest = database
            .manifest(version)
            .await
            .map_err(|e| e.to_string())?;
        if manifest.parent != (version > 1).then(|| version - 1) {
            return Err(format!(
                "version {version} has parent {:?}",
                manifest.parent
            ));
        }
        let id = manifest
            .commit_id
            .clone()
            .ok_or(format!("version {version} has no commit id"))?;
        if !commit_ids.insert(id.clone()) {
            return Err(format!(
                "commit {id} landed twice, again at version {version}"
            ));
        }
        let current = manifest.metadata.get(TAG).cloned();
        if current != tag {
            let name = current.clone().unwrap();
            let (key, commit) = log
                .get_key_value(&name)
                .ok_or(format!("version {version} carries unknown commit {name}"))?;
            match commit.outcome {
                None => {
                    return Err(format!(
                        "commit {name} failed but landed at version {version}"
                    ))
                }
                Some(Some(reported)) if reported != version => {
                    return Err(format!(
                        "commit {name} reported version {reported}, landed at {version}"
                    ))
                }
                _ => {}
            }
            if landed.insert(key.as_str(), version).is_some() {
                return Err(format!("commit {name} landed twice"));
            }
            model.extend(&commit.adds);
            for id in &commit.deletes {
                model.remove(id);
            }
            tag = current;
        }
        let documents: BTreeSet<u64> = match manifest.index("default", INDEX) {
            Some(_) => database
                .open_index(&manifest, INDEX, Default::default())
                .await
                .map_err(|e| e.to_string())?
                .documents()
                .map(|d| d.id)
                .collect(),
            None => BTreeSet::new(),
        };
        if documents != model {
            return Err(format!(
                "version {version} holds {:?} beyond the model and lacks {:?}",
                documents.difference(&model).collect::<Vec<_>>(),
                model.difference(&documents).collect::<Vec<_>>()
            ));
        }
    }
    for (name, commit) in log {
        if matches!(commit.outcome, Some(Some(_))) && !landed.contains_key(name.as_str()) {
            return Err(format!("acknowledged commit {name} is missing"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeds() -> Vec<u64> {
        if let Ok(seed) = std::env::var("COMPLETYPE_SIM_SEED") {
            return vec![seed.parse().expect("COMPLETYPE_SIM_SEED is a number")];
        }
        let count = std::env::var("COMPLETYPE_SIM_SEEDS")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(200);
        (0..count).collect()
    }

    #[test]
    fn concurrent_writers_and_compaction_under_faults_match_the_model() {
        for seed in seeds() {
            if let Err(problem) = run(seed) {
                panic!("seed {seed}: {problem}; replay with COMPLETYPE_SIM_SEED={seed}");
            }
        }
    }

    /// Submits three change sets, the second deleting a document of the first, and crashes the
    /// ingestor after each of its requests in turn. A new process then finishes the work, and
    /// every change set must be applied exactly once, with nothing left behind after cleanup.
    #[tokio::test]
    async fn an_ingestor_that_crashes_at_any_request_loses_and_doubles_nothing() {
        use crate::{ChangeSet, CleanupPolicy, Ingestor};
        for crash_at in 0.. {
            let store = Arc::new(FaultStore::default());
            let database = store.database();
            let mut ids = Vec::new();
            for (upserts, deletes) in [(vec![1, 2], vec![]), (vec![3], vec![1]), (vec![4], vec![])]
            {
                let mut changes = ChangeSet::new();
                changes.upsert(
                    INDEX,
                    upserts.into_iter().map(|id| Document::new(id, "song", 0.5)),
                );
                changes.delete(INDEX, deletes);
                ids.push(database.submit(changes).await.unwrap());
            }
            let ingest = |owner: &'static str| {
                let mut ingestor = Ingestor::new(store.database(), owner);
                ingestor.lease_ttl = Duration::from_millis(1);
                ingestor.max_change_sets = 1;
                ingestor.compaction = Some(CompactionPolicy::default().fanout(2).max_segments(2));
                ingestor
            };

            store.crash_in(Some(crash_at));
            let mut first = ingest("first");
            for _ in 0..5 {
                let _ = first.run_once().await;
            }
            let crashed = store.has_crashed();
            store.crash_in(None);

            tokio::time::sleep(Duration::from_millis(5)).await;
            let mut second = ingest("second");
            for _ in 0..10 {
                second.run_once().await.unwrap();
            }
            let at = format!("crash after {crash_at} requests");
            assert_eq!(database.pending_change_sets().await.unwrap(), 0, "{at}");
            let latest = database.latest().await.unwrap();
            let documents: BTreeSet<u64> = database
                .open_index(&latest, INDEX, Default::default())
                .await
                .unwrap()
                .documents()
                .map(|d| d.id)
                .collect();
            assert_eq!(documents, BTreeSet::from([2, 3, 4]), "{at}");

            // Each change set is first recorded as applied in exactly one version.
            let mut seen: BTreeMap<String, usize> = BTreeMap::new();
            let mut before: Vec<String> = Vec::new();
            for version in database.versions().await.unwrap() {
                let applied = crate::database::claimed(
                    &database.manifest(version).await.unwrap(),
                    crate::inbox::APPLIED,
                );
                for id in applied.iter().filter(|id| !before.contains(id)) {
                    *seen.entry(id.clone()).or_default() += 1;
                }
                before = applied;
            }
            for id in &ids {
                assert_eq!(seen.get(id), Some(&1), "{at}: change set {id}");
            }

            let policy = CleanupPolicy::default()
                .keep_versions(1)
                .older_than(Duration::ZERO);
            database.cleanup(&policy).await.unwrap();
            let referenced: BTreeSet<String> = latest
                .entries()
                .flat_map(|(_, _, e)| e.segments.iter().map(|s| s.key.clone()))
                .collect();
            let store_ = database.store();
            let segments: BTreeSet<String> =
                store_.list("segments").await.unwrap().into_iter().collect();
            assert_eq!(segments, referenced, "{at}: orphaned segments");
            for prefix in ["_probe", "_clock", "_inbox"] {
                assert!(
                    store_.list(prefix).await.unwrap().is_empty(),
                    "{at}: {prefix} left"
                );
            }
            if !crashed {
                assert!(crash_at > 10, "the crash points never reached the ingestor");
                break;
            }
        }
    }

    /// Exact requests per operation, so that an extra one fails here rather than on a bill.
    #[tokio::test]
    async fn operations_make_exactly_the_requests_they_need() {
        let store = Arc::new(FaultStore::default());
        let database = store.database();
        database.store().check_create_only().await.unwrap();
        let commit = |id: u64| {
            let database = database.clone();
            async move {
                let mut txn = database.begin().await.unwrap();
                txn.append_documents(INDEX, [Document::new(id, "Rust", 0.5)], [])
                    .unwrap();
                txn.commit().await.unwrap();
            }
        };
        commit(1).await;
        store.take_trace();
        let version = |v: u64| format!("_versions/{v:020}.json");

        commit(2).await;
        assert_eq!(
            store.take_trace(),
            [
                "list _versions".to_owned(),
                format!("get {}", version(1)),
                "put segments/*".into(),
                "list _versions".into(),
                format!("put {}", version(2)),
            ]
        );

        let engine = crate::Engine::new();
        let replica = crate::Replica::new(database.clone(), Default::default());
        replica.sync(&engine).await.unwrap();
        assert_eq!(
            store.take_trace(),
            [
                "list _versions".to_owned(),
                format!("get {}", version(2)),
                "get segments/*".into(),
                "get segments/*".into(),
            ]
        );
        replica.sync(&engine).await.unwrap();
        assert_eq!(store.take_trace(), ["list _versions"]);

        commit(3).await;
        store.take_trace();
        replica.sync(&engine).await.unwrap();
        assert_eq!(
            store.take_trace(),
            [
                "list _versions".to_owned(),
                format!("get {}", version(3)),
                "get segments/*".into(),
            ]
        );

        engine.complete(&[INDEX], "ru", 10).unwrap();
        assert!(store.take_trace().is_empty());

        let mut changes = crate::ChangeSet::new();
        changes.upsert(INDEX, [Document::new(9, "Go", 0.5)]);
        database.submit(changes).await.unwrap();
        assert_eq!(store.take_trace(), ["put _inbox/*"]);
    }

    #[test]
    fn a_seed_replays_the_same_requests() {
        for seed in [1, 2, 3] {
            assert_eq!(run(seed).unwrap(), run(seed).unwrap(), "seed {seed}");
        }
    }
}
