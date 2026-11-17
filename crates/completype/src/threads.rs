//! Rayon work that survives `fork()`: a forked child's copy of the global pool has no threads.

use std::sync::{Mutex, OnceLock};

/// Runs `f` on rayon's global pool, or in a forked child on a pool of the child's own.
pub(crate) fn global<R: Send>(f: impl FnOnce() -> R + Send) -> R {
    static FIRST: OnceLock<u32> = OnceLock::new();
    let pid = std::process::id();
    if *FIRST.get_or_init(|| pid) == pid {
        return f();
    }
    static CHILD: Mutex<Option<(u32, &'static rayon::ThreadPool)>> = Mutex::new(None);
    let pool = {
        let mut child = CHILD.lock().unwrap_or_else(|e| e.into_inner());
        match *child {
            Some((owner, pool)) if owner == pid => pool,
            // A grandchild leaks its parent's pool too, as dropping it would wait for its threads.
            _ => {
                let pool = rayon::ThreadPoolBuilder::new()
                    .build()
                    .expect("failed to start a thread pool");
                let pool: &'static rayon::ThreadPool = Box::leak(Box::new(pool));
                *child = Some((pid, pool));
                pool
            }
        }
    };
    pool.install(f)
}
