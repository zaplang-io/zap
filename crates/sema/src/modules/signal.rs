//! `signal` module — sema facet (ticket #77, D-0058).

use zapc_diag::ErrorCode;
use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::ModuleCheck;
use crate::{expr_span, Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "signal",
    check,
};

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        // signal.notify(Signal) -> channel[Signal]: a cap-1 coalescing channel
        // delivering the given signal (D-0058).
        "notify" => {
            if args.len() != 1 {
                checker.error(
                    ErrorCode::ZAP4025,
                    format!("signal.notify(sig) takes 1 argument, found {}", args.len()),
                    span,
                );
            } else {
                let at = checker.infer(&args[0].value);
                if !matches!(&at, Ty::Named(n) if n == "Signal") && !matches!(at, Ty::Unknown) {
                    checker.error(
                        ErrorCode::ZAP4024,
                        format!("signal.notify expects a Signal, found {}", at.display()),
                        expr_span(&args[0].value),
                    );
                }
            }
            Some(Ty::Chan(Box::new(Ty::Named("Signal".to_string()))))
        }
        _ => None,
    }
}
