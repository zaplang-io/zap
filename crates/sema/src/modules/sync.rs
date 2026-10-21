//! `sync` module — sema facet (ticket #103, D-0032).
//!
//! Coroutine synchronization primitives: `Mutex`, `WaitGroup`, and `Once`.
//! Following the established seam pattern (`deque`/`crypto`), each primitive is
//! an opaque handle typed as `bytes`: `sync.mutex()`/`sync.waitgroup()`/
//! `sync.once()` create one and the remaining functions operate on it. The
//! handle is passed by value, so a worker coroutine receives a `WaitGroup` as a
//! `bytes` parameter (`def worker(bytes wg, ...)`), matching the D-0032
//! fire-and-forget join pattern.
//!
//! Blocking operations (`lock` on a contended mutex, `wait`, `once_begin` while
//! another caller is initializing) park the coroutine on the scheduler rather
//! than the OS thread — the runtime builds them on the shared `gopark`/`goready`
//! primitive (see the `sync` section of `zaprt.c`).
//!
//! `Atomic[T]` (I#176, D-0066) is a sibling `sync` primitive but is NOT part of
//! this flat `sync.*` surface: it is the prelude generic `struct Atomic[T]`
//! (`crates/syntax/src/parser.rs`), whose methods lower to inline LLVM atomics —
//! see `lower_atomic_method` in `crates/ir/src/lower.rs`. Still deferred (noted in
//! the PR): `RWMutex`, and the `with mu.lock():` / `with wg.tracked():`
//! context-manager guards (blocked on general context managers, D-0027).

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, ModuleCheck};
use crate::{Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "sync",
    check,
};

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        // --- Mutex ---------------------------------------------------------
        // sync.mutex() -> handle (unlocked).
        "mutex" => {
            expect_argc(checker, "sync", "mutex", args, 0, span);
            Some(Ty::Bytes)
        }
        // sync.lock(m) / sync.unlock(m): acquire / release, no result. `lock`
        // parks the coroutine while the mutex is held by another.
        "lock" | "unlock" => {
            expect_argc(checker, "sync", method, args, 1, span);
            Some(Ty::Unit)
        }
        // sync.trylock(m) -> bool: non-blocking acquire (true iff acquired).
        "trylock" => {
            expect_argc(checker, "sync", "trylock", args, 1, span);
            Some(Ty::Bool)
        }

        // --- WaitGroup -----------------------------------------------------
        // sync.waitgroup() -> handle (counter at 0).
        "waitgroup" => {
            expect_argc(checker, "sync", "waitgroup", args, 0, span);
            Some(Ty::Bytes)
        }
        // sync.add(wg, n): adjust the counter by n, no result.
        "add" => {
            expect_argc(checker, "sync", "add", args, 2, span);
            Some(Ty::Unit)
        }
        // sync.done(wg): decrement the counter (== add(wg, -1)), no result.
        "done" => {
            expect_argc(checker, "sync", "done", args, 1, span);
            Some(Ty::Unit)
        }
        // sync.wait(wg): park until the counter returns to 0, no result.
        "wait" => {
            expect_argc(checker, "sync", "wait", args, 1, span);
            Some(Ty::Unit)
        }

        // --- Once ----------------------------------------------------------
        // sync.once() -> handle (fresh).
        "once" => {
            expect_argc(checker, "sync", "once", args, 0, span);
            Some(Ty::Bytes)
        }
        // sync.once_begin(o) -> bool: true for exactly one caller (which then
        // runs the initializer and calls once_end); false for the rest, after
        // the initializer has completed.
        "once_begin" => {
            expect_argc(checker, "sync", "once_begin", args, 1, span);
            Some(Ty::Bool)
        }
        // sync.once_end(o): publish completion, waking any parked losers.
        "once_end" => {
            expect_argc(checker, "sync", "once_end", args, 1, span);
            Some(Ty::Unit)
        }

        // --- Mutex timed acquire (I#175) -----------------------------------
        // sync.lock_timeout(m, secs) -> bool: acquire within `secs`; true iff
        // acquired. Backs threading.Lock.acquire_timeout.
        "lock_timeout" => {
            expect_argc(checker, "sync", "lock_timeout", args, 2, span);
            Some(Ty::Bool)
        }

        // --- RLock: reentrant lock (I#175) ---------------------------------
        "rlock" => {
            expect_argc(checker, "sync", "rlock", args, 0, span);
            Some(Ty::Bytes)
        }
        "rlock_lock" | "rlock_unlock" => {
            expect_argc(checker, "sync", method, args, 1, span);
            Some(Ty::Unit)
        }
        "rlock_trylock" => {
            expect_argc(checker, "sync", "rlock_trylock", args, 1, span);
            Some(Ty::Bool)
        }

        // --- Event: a latch (I#175) ----------------------------------------
        "event" => {
            expect_argc(checker, "sync", "event", args, 0, span);
            Some(Ty::Bytes)
        }
        "event_set" | "event_clear" | "event_wait" => {
            expect_argc(checker, "sync", method, args, 1, span);
            Some(Ty::Unit)
        }
        "event_is_set" => {
            expect_argc(checker, "sync", "event_is_set", args, 1, span);
            Some(Ty::Bool)
        }
        "event_wait_timeout" => {
            expect_argc(checker, "sync", "event_wait_timeout", args, 2, span);
            Some(Ty::Bool)
        }

        // --- Semaphore: counting semaphore (I#175) -------------------------
        "semaphore" => {
            expect_argc(checker, "sync", "semaphore", args, 1, span);
            Some(Ty::Bytes)
        }
        "sem_acquire" | "sem_release" => {
            expect_argc(checker, "sync", method, args, 1, span);
            Some(Ty::Unit)
        }
        "sem_acquire_timeout" => {
            expect_argc(checker, "sync", "sem_acquire_timeout", args, 2, span);
            Some(Ty::Bool)
        }

        // --- Condition: condition variable over an internal lock (I#175) ---
        "condition" => {
            expect_argc(checker, "sync", "condition", args, 0, span);
            Some(Ty::Bytes)
        }
        "cond_acquire" | "cond_release" | "cond_wait" | "cond_notify_all" => {
            expect_argc(checker, "sync", method, args, 1, span);
            Some(Ty::Unit)
        }
        "cond_notify" => {
            expect_argc(checker, "sync", "cond_notify", args, 2, span);
            Some(Ty::Unit)
        }
        _ => None,
    }
}
