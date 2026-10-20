//! `random` module — sema facet (ticket #98, D-0056).
//!
//! A seedable xoshiro256** PRNG (seeded from OS entropy by default). Python-
//! familiar surface: `seed(n)`, `random()`, `uniform(a, b)`, `randint(a, b)`,
//! `choice(seq)`, `shuffle(seq)`. A "sequence" is a `list[T]`: `choice` yields
//! an element (`T`), `shuffle` permutes it in place.

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, ModuleCheck};
use crate::{expr_span, Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "random",
    check,
};

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    match method {
        // random.seed(n): reseed the generator from an integer.
        "seed" => {
            expect_argc(checker, "random", "seed", args, 1, span);
            Some(Ty::Unit)
        }
        // random.random() -> float64 in [0, 1).
        "random" => {
            expect_argc(checker, "random", "random", args, 0, span);
            Some(Ty::Float64)
        }
        // random.uniform(a, b) -> float64 in [a, b].
        "uniform" => {
            expect_argc(checker, "random", "uniform", args, 2, span);
            Some(Ty::Float64)
        }
        // random.randint(a, b) -> int64 in [a, b] (both endpoints inclusive,
        // matching Python).
        "randint" => {
            expect_argc(checker, "random", "randint", args, 2, span);
            Some(Ty::Int64)
        }
        // random.choice(seq) -> element of the list.
        "choice" => {
            let ty = require_list(checker, "choice", args, span);
            Some(ty)
        }
        // random.shuffle(seq): permute the list in place.
        "shuffle" => {
            require_list(checker, "shuffle", args, span);
            Some(Ty::Unit)
        }
        _ => None,
    }
}

/// Require a single `list[T]` argument, returning its element type `T` (or
/// `Unknown` on error). Shared by `choice` (which yields `T`) and `shuffle`.
fn require_list(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Ty {
    if args.len() != 1 {
        checker.error(
            format!(
                "random.{method}(seq) takes 1 argument, found {}",
                args.len()
            ),
            span,
        );
        for a in args {
            checker.infer(&a.value);
        }
        return Ty::Unknown;
    }
    match checker.infer(&args[0].value) {
        Ty::List(elem) => *elem,
        Ty::Unknown => Ty::Unknown,
        other => {
            checker.error(
                format!("random.{method} expects a list, found {}", other.display()),
                expr_span(&args[0].value),
            );
            Ty::Unknown
        }
    }
}
