//! `logging` module — sema facet (ticket #101, D-0056).
//!
//! Leveled logging over the `io`/`sys` stderr path (#97). The level thresholds
//! (`logging.DEBUG`/`INFO`/`WARNING`/`ERROR`/`CRITICAL`) are *attributes*
//! (`logging.DEBUG`), not calls, typed by [`const_ty`] through the const sub-
//! registry in `super` (like `math.pi`). Everything else is a method call:
//! the per-level emitters (`debug`/`info`/…), the generic `log(level, msg)`, and
//! the configuration setters (`set_level`/`set_stream`/`set_format`).

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, ModuleCheck, ModuleConst};
use crate::{Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "logging",
    check,
};

pub(super) const MODULE_CONST: ModuleConst = ModuleConst {
    name: "logging",
    const_ty,
};

/// Type of a `logging` level threshold accessed as `logging.<name>` (all
/// `int64`, Python's numeric values: DEBUG=10 … CRITICAL=50).
fn const_ty(name: &str) -> Option<Ty> {
    match name {
        "DEBUG" | "INFO" | "WARNING" | "ERROR" | "CRITICAL" => Some(Ty::Int64),
        _ => None,
    }
}

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    Some(match method {
        // Per-level emitters: logging.info("...") -> Unit.
        "debug" | "info" | "warning" | "error" | "critical" => {
            expect_argc(checker, "logging", method, args, 1, span);
            Ty::Unit
        }
        // Generic emitter: logging.log(level, msg) -> Unit.
        "log" => {
            expect_argc(checker, "logging", method, args, 2, span);
            Ty::Unit
        }
        // Configuration.
        "set_level" => {
            expect_argc(checker, "logging", method, args, 1, span);
            Ty::Unit
        }
        "get_level" => {
            expect_argc(checker, "logging", method, args, 0, span);
            Ty::Int64
        }
        // Direct output to a raw fd (default stderr = sys.stderr_fd()).
        "set_stream" => {
            expect_argc(checker, "logging", method, args, 1, span);
            Ty::Unit
        }
        // Line format template: %(asctime)s / %(levelname)s / %(message)s.
        "set_format" => {
            expect_argc(checker, "logging", method, args, 1, span);
            Ty::Unit
        }
        _ => return None,
    })
}
