//! `argparse` module — sema facet (ticket #101, D-0056).
//!
//! CLI argument parsing over a **global singleton parser** (a notable v1 choice,
//! flagged in the PR): with no prelude object/namespace types on the module seam
//! yet, the parser is process-global state manipulated through flat module
//! functions — declare arguments (`add_flag`/`add_option`/`add_int`/
//! `add_positional`), `parse(argv)` a `list[str]` (typically `sys.argv()`, #97),
//! then read typed results back (`get`/`get_int`/`get_flag`). `-h`/`--help`
//! auto-prints the generated usage; `format_help()` exposes it directly.

use zapc_syntax::ast::Arg;
use zapc_syntax::span::Span;

use super::{expect_argc, ModuleCheck};
use crate::{Checker, Ty};

pub(super) const MODULE: ModuleCheck = ModuleCheck {
    name: "argparse",
    check,
};

fn check(checker: &mut Checker, method: &str, args: &[Arg], span: Span) -> Option<Ty> {
    Some(match method {
        // Parser metadata (shown in --help).
        "set_prog" | "set_description" => {
            expect_argc(checker, "argparse", method, args, 1, span);
            Ty::Unit
        }
        // Declare a boolean `--name` flag / a required positional.
        "add_flag" | "add_positional" => {
            expect_argc(checker, "argparse", method, args, 1, span);
            Ty::Unit
        }
        // Declare `--name VALUE`: a string option (name, default) / an int option.
        "add_option" | "add_int" => {
            expect_argc(checker, "argparse", method, args, 2, span);
            Ty::Unit
        }
        // Parse a `list[str]` (typically sys.argv()); raises on a parse error.
        "parse" => {
            expect_argc(checker, "argparse", method, args, 1, span);
            Ty::Unit
        }
        // Typed result accessors.
        "get" => {
            expect_argc(checker, "argparse", method, args, 1, span);
            Ty::Str
        }
        "get_int" => {
            expect_argc(checker, "argparse", method, args, 1, span);
            Ty::Int64
        }
        "get_flag" => {
            expect_argc(checker, "argparse", method, args, 1, span);
            Ty::Bool
        }
        // Generated usage/help text, and whether -h/--help was seen.
        "format_help" => {
            expect_argc(checker, "argparse", method, args, 0, span);
            Ty::Str
        }
        "help_requested" => {
            expect_argc(checker, "argparse", method, args, 0, span);
            Ty::Bool
        }
        // Clear all registered arguments and parsed state (reuse / tests).
        "reset" => {
            expect_argc(checker, "argparse", method, args, 0, span);
            Ty::Unit
        }
        _ => return None,
    })
}
