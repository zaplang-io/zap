//! `runtime` module — sema facet (ticket #77, D-0058).

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, ModuleCheck};
use crate::{Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "runtime",
    check,
};

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        // runtime.goroutine_dump(): dump goroutine stacks + scheduler summary.
        "goroutine_dump" => {
            expect_argc(checker, "runtime", "goroutine_dump", args, 0, span);
            Some(Ty::Unit)
        }
        // runtime.num_goroutines() -> int64.
        "num_goroutines" => {
            expect_argc(checker, "runtime", "num_goroutines", args, 0, span);
            Some(Ty::Int64)
        }
        // runtime.maxprocs() -> int64: the effective worker-M count (ticket #172).
        "maxprocs" => {
            expect_argc(checker, "runtime", "maxprocs", args, 0, span);
            Some(Ty::Int64)
        }
        // runtime.lock_os_thread(): pin the calling coroutine to its own OS
        // thread until unlock_os_thread (ticket #172, Go's LockOSThread analogue).
        "lock_os_thread" => {
            expect_argc(checker, "runtime", "lock_os_thread", args, 0, span);
            Some(Ty::Unit)
        }
        // runtime.unlock_os_thread(): release the pin (balances nested locks).
        "unlock_os_thread" => {
            expect_argc(checker, "runtime", "unlock_os_thread", args, 0, span);
            Some(Ty::Unit)
        }
        // runtime.profile_start(): begin CPU profiling (ticket #78, D-0058). The
        // output path / sample rate come from ZAP_PROFILE_OUT / ZAP_PROFILE_HZ;
        // folded stacks are written at profile_stop. Same effect as SIGUSR2.
        "profile_start" => {
            expect_argc(checker, "runtime", "profile_start", args, 0, span);
            Some(Ty::Unit)
        }
        // runtime.profile_stop(): stop CPU profiling and write the folded output.
        "profile_stop" => {
            expect_argc(checker, "runtime", "profile_stop", args, 0, span);
            Some(Ty::Unit)
        }
        _ => None,
    }
}
