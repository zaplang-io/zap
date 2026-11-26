//! `sync` module — lowering facet (ticket #103, D-0032).
//!
//! Emits the backing `zap_mutex_*` / `zap_waitgroup_*` / `zap_once_*` runtime
//! calls. The handle is a `Ptr`; the count argument is an `int64` word.
//! [`lower_args`] passes the handle through and coerces integer arguments to
//! `i64`. Most are infallible from the lowering's view and lower to plain
//! (non-throwing) calls. The misuse paths that raise a **catchable** exception
//! in the runtime — a negative `WaitGroup` counter (`add`/`done`) and releasing
//! an un-acquired `RLock` (`rlock_unlock`) — lower through an unwind edge
//! (`throwing: true`) so `try/except` in the calling frame catches them (#1566).

use zapc_syntax::ast;

use super::support::{call_bool, call_ret, call_void, lower_args};
use super::ModuleLower;
use crate::ir::*;
use crate::lower::{FnLower, LowerError};

pub(super) const MODULE: ModuleLower = ModuleLower {
    name: "sync",
    lower,
};

fn lower(
    lowerer: &mut FnLower,
    method: &str,
    args: &[ast::Arg],
) -> Result<Option<(Value, Ty)>, LowerError> {
    let a = lower_args(lowerer, args)?;
    Ok(Some(match method {
        // Each synchronisation primitive's constructor returns an opaque GC
        // pointer handle that sema types as `bytes`; the IR agrees (`Ty::Bytes`,
        // not a bare `str` `Ty::Ptr`) so a handle stored in a `bytes`-typed
        // collection is not mis-rejected now that `bytes` and `str` are distinct
        // `Elem` kinds (#1074, D-0241). Same pointer word at codegen.
        // Mutex
        "mutex" => call_ret(lowerer, "zap_mutex_new", a, Ty::Bytes, false),
        "lock" => call_void(lowerer, "zap_mutex_lock", a, false),
        "unlock" => call_void(lowerer, "zap_mutex_unlock", a, false),
        "trylock" => call_bool(lowerer, "zap_mutex_trylock", a),
        // WaitGroup
        "waitgroup" => call_ret(lowerer, "zap_waitgroup_new", a, Ty::Bytes, false),
        // A negative resulting counter raises a catchable exception in the
        // runtime (#1566), so `add`/`done` must lower through an unwind edge.
        "add" => call_void(lowerer, "zap_waitgroup_add", a, true),
        "done" => call_void(lowerer, "zap_waitgroup_done", a, true),
        "wait" => call_void(lowerer, "zap_waitgroup_wait", a, false),
        // Once
        "once" => call_ret(lowerer, "zap_once_new", a, Ty::Bytes, false),
        "once_begin" => call_bool(lowerer, "zap_once_begin", a),
        "once_end" => call_void(lowerer, "zap_once_end", a, false),
        // Mutex timed acquire (I#175).
        "lock_timeout" => call_bool(lowerer, "zap_mutex_lock_timeout", a),
        // RLock (I#175).
        "rlock" => call_ret(lowerer, "zap_rlock_new", a, Ty::Bytes, false),
        "rlock_lock" => call_void(lowerer, "zap_rlock_lock", a, false),
        // Releasing an un-acquired RLock raises a catchable exception (#1566).
        "rlock_unlock" => call_void(lowerer, "zap_rlock_unlock", a, true),
        "rlock_trylock" => call_bool(lowerer, "zap_rlock_trylock", a),
        // Event (I#175).
        "event" => call_ret(lowerer, "zap_event_new", a, Ty::Bytes, false),
        "event_set" => call_void(lowerer, "zap_event_set", a, false),
        "event_clear" => call_void(lowerer, "zap_event_clear", a, false),
        "event_is_set" => call_bool(lowerer, "zap_event_is_set", a),
        "event_wait" => call_void(lowerer, "zap_event_wait", a, false),
        "event_wait_timeout" => call_bool(lowerer, "zap_event_wait_timeout", a),
        // Semaphore (I#175).
        "semaphore" => call_ret(lowerer, "zap_sem_new", a, Ty::Bytes, false),
        "sem_acquire" => call_void(lowerer, "zap_sem_acquire", a, false),
        "sem_acquire_timeout" => call_bool(lowerer, "zap_sem_acquire_timeout", a),
        "sem_release" => call_void(lowerer, "zap_sem_release", a, false),
        // Condition (I#175).
        "condition" => call_ret(lowerer, "zap_cond_new", a, Ty::Bytes, false),
        "cond_acquire" => call_void(lowerer, "zap_cond_acquire", a, false),
        "cond_release" => call_void(lowerer, "zap_cond_release", a, false),
        "cond_wait" => call_void(lowerer, "zap_cond_wait", a, false),
        "cond_notify" => call_void(lowerer, "zap_cond_notify", a, false),
        "cond_notify_all" => call_void(lowerer, "zap_cond_notify_all", a, false),
        _ => return Ok(None),
    }))
}
