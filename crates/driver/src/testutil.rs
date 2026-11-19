//! Shared test-only temp-path helpers (issue #917, extending #180/#916).
//!
//! Historically each test module grew its own "unique temp name" helper derived
//! from a `SystemTime` nanosecond reading. On a microsecond-quantised clock
//! (macOS) two allocations landing in the same tick produced identical names and
//! stomped each other's fixtures under parallel runs — the flake behind #180 /
//! #735. PR #916 fixed the MCP modules by routing them through one pid+counter
//! helper; #917 routes the remaining sibling sites (the module linker and the
//! diagnostics render tests) through this single crate-wide helper so the
//! clock-based mistake cannot reappear piecemeal.
//!
//! The uniqueness guarantee is a process-global `AtomicU64` sequence combined
//! with the process id, and **no clock reading**: the pid makes names distinct
//! across concurrent `cargo test` processes, the counter makes them distinct
//! across threads and repeated calls within one process, and dropping the clock
//! removes the only source that could ever collide.

#![cfg(test)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// Process-global allocation sequence shared by every call below. `Relaxed` is
/// sufficient: we only need each `fetch_add` to observe a distinct value, not
/// any ordering relative to other memory.
static SEQ: AtomicU64 = AtomicU64::new(0);

/// A process-unique path under the system temp dir, named
/// `<prefix>-<pid>-<seq><suffix>`. Does **not** touch the filesystem. `suffix`
/// is appended verbatim (e.g. `".zap"`); pass `""` for none.
pub(crate) fn unique_temp_path(prefix: &str, suffix: &str) -> PathBuf {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{prefix}-{}-{seq}{suffix}", std::process::id()))
}

/// A freshly-created, process-unique temp directory named `<prefix>-<pid>-<seq>`.
/// This is the one sanctioned way for the crate's test modules to name a fixture
/// directory.
pub(crate) fn unique_temp_dir(prefix: &str) -> PathBuf {
    let dir = unique_temp_path(prefix, "");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::{Arc, Barrier, Mutex};

    /// Regression guard for issue #917 (and the #180 flake class): temp names
    /// must be unique regardless of timing. Many threads allocate in a tight
    /// loop behind a barrier so they all start at the same instant — the exact
    /// condition under which a `SystemTime`-nanosecond name used to collide on a
    /// microsecond-quantised clock and stomp a sibling's fixture. The
    /// atomic-counter mechanism makes every name distinct, so this passes
    /// deterministically; a regression to clock-based naming fails it here
    /// instead of flaking in unrelated tests under parallel CI.
    #[test]
    fn unique_temp_paths_are_distinct_under_parallel_allocation() {
        const THREADS: usize = 8;
        const PER_THREAD: usize = 256;

        let barrier = Arc::new(Barrier::new(THREADS));
        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    // Release all threads simultaneously, maximising the chance a
                    // time-based name would land in the same nanosecond bucket.
                    barrier.wait();
                    (0..PER_THREAD)
                        // A shared prefix across every thread: only the pid+seq
                        // suffix can distinguish these, so this exercises exactly
                        // the same-tick-collision path.
                        .map(|_| unique_temp_path("zap-testutil-uniq", ".tmp"))
                        .collect::<Vec<_>>()
                })
            })
            .collect();

        let seen: Arc<Mutex<HashSet<PathBuf>>> = Arc::new(Mutex::new(HashSet::new()));
        for h in handles {
            let paths = h.join().expect("allocation thread panicked");
            let mut guard = seen.lock().unwrap();
            for p in paths {
                assert!(
                    guard.insert(p.clone()),
                    "temp-path name collided under parallel allocation: {}",
                    p.display()
                );
            }
        }
        assert_eq!(seen.lock().unwrap().len(), THREADS * PER_THREAD);
    }

    /// The directory variant creates the directory and yields distinct, existing
    /// paths.
    #[test]
    fn unique_temp_dir_creates_distinct_directories() {
        let a = unique_temp_dir("zap-testutil-dir");
        let b = unique_temp_dir("zap-testutil-dir");
        assert_ne!(a, b);
        assert!(a.is_dir());
        assert!(b.is_dir());
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }

    /// The suffix is appended verbatim and the prefix is preserved, so existing
    /// greps for the fixed name prefixes keep matching.
    #[test]
    fn name_shape_preserves_prefix_and_suffix() {
        let p = unique_temp_path("zap-diag", ".zap");
        let name = p.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("zap-diag-"), "prefix lost: {name}");
        assert!(name.ends_with(".zap"), "suffix lost: {name}");
    }
}
