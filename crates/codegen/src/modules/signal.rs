//! `signal` module — runtime-symbol declarations (ticket #77, D-0058).
//!
//! Signals, runtime diagnostics & fault reporting. `zap_signal_init()` runs at
//! `main`'s prologue (installs the default SIGQUIT/SIGUSR1 handlers unless built
//! `-DZAP_NO_DEFAULT_SIGNALS`); `zap_signal_notify(signo)` returns a cap-1
//! coalescing channel[Signal]; `zap_signal_on(signo, fn)` registers an
//! `@on_signal` handler (a detached receiver goroutine); `zap_scope_cancel_on
//! (scope, mask, grace_ms)` / `zap_scope_disarm(scope)` wire `scope(cancel_on=..,
//! grace=..)`. `zap_signal_selftest`/`raise`/`drain` are C integration-test
//! hooks. See `zaprt.c`.

use inkwell::AddressSpace;

use super::Cg;

pub(crate) fn declare_rt(cg: &mut Cg) {
    let i = cg.ctx.i64_type();
    let p = cg.ctx.ptr_type(AddressSpace::default());
    cg.declare_rt("zap_signal_init", &[], None);
    cg.declare_rt("zap_signal_notify", &[i.into()], Some(p.into()));
    cg.declare_rt("zap_signal_on", &[i.into(), p.into()], None);
    cg.declare_rt("zap_scope_cancel_on", &[p.into(), i.into(), i.into()], None);
    cg.declare_rt("zap_scope_disarm", &[p.into()], None);
    cg.declare_rt("zap_signal_selftest", &[], Some(i.into()));
    cg.declare_rt("zap_signal_raise", &[i.into()], None);
    cg.declare_rt("zap_signal_drain", &[], None);
}
