//! Zap semantic analysis — the type checker (Phase 1).
//!
//! Scope for this pass: scalars and functions with local inference, enough to
//! type-check the milestone programs (`fib`, `loop_sum`). It resolves function
//! signatures, infers local variable types (`auto`), checks declared types and
//! assignments (with implicit widening per D-0020), and checks calls and
//! returns. Richer features — struct fields, generics, interfaces, full
//! pattern typing — are intentionally handled loosely (as `Unknown`) for now.

use std::collections::{HashMap, HashSet};

use zapc_diag::ErrorCode;
use zapc_syntax::ast::*;
use zapc_syntax::fstring::{self, FStringPart, FormatSpec};
use zapc_syntax::genanalysis;
use zapc_syntax::span::Span;
use zapc_syntax::{BUILTIN_SHAREABLE_TYPES, SCOPED_MARKER, SHAREABLE_MARKER};

mod scoped;

// The source-level type model (`Ty`, the `*Info` records, and the pure type
// operations `assignable`/`join`/`num_join`/`default_type` and the dunder
// tables) lives in the `zapc_sema_types` leaf (#129). Glob-import it so the
// `Checker` and the per-module type-check twins keep referring to those names
// unqualified (and via `crate::` from submodules) as before; re-export `Ty` so
// the public `zapc_sema::Ty` is unchanged for downstream code.
pub use zapc_sema_types::Ty;
use zapc_sema_types::*;

/// Built-in stdlib module registry (ticket #121): per-module type-checking lives
/// in `modules/<name>.rs`, dispatched by a data table instead of a hand-written
/// `match (module, method)` block, so new modules are conflict-free parallel-
/// addable. See [`modules`] for the seam contract.
mod modules;

/// Whether `name` is a compiler-known built-in (stdlib) module (D-0056). Used by
/// the driver's Zap-to-Zap module resolver (I#283/D-0081) to leave stdlib imports
/// to the existing built-in handling.
pub use modules::is_builtin_module;

/// A type-checking diagnostic with its stable [`ErrorCode`] and the offending
/// source span.
///
/// Despite the name this carries both errors and warnings (the severity is
/// decided by which list — `errors` or `warnings` — it lands in). The `code` is
/// the machine-parseable identifier (see `zapc_diag`); `message` is human-facing
/// prose that may change between releases.
#[derive(Clone, Debug, PartialEq)]
pub struct TypeError {
    pub code: ErrorCode,
    pub message: String,
    pub span: Span,
}

/// A compile-time-constant non-negative integer index, if `e` is one (used for
/// tuple element access `t[k]`, D-0049). Only integer literals (with an optional
/// unary `+`) are constant folds here; a negative or non-literal index is not a
/// valid tuple index.
fn const_index(e: &Expr) -> Option<u64> {
    match e {
        Expr::Int(s, _) => {
            let clean: String = s.chars().filter(|c| *c != '_').collect();
            if let Some(h) = clean
                .strip_prefix("0x")
                .or_else(|| clean.strip_prefix("0X"))
            {
                u64::from_str_radix(h, 16).ok()
            } else if let Some(o) = clean
                .strip_prefix("0o")
                .or_else(|| clean.strip_prefix("0O"))
            {
                u64::from_str_radix(o, 8).ok()
            } else if let Some(b) = clean
                .strip_prefix("0b")
                .or_else(|| clean.strip_prefix("0B"))
            {
                u64::from_str_radix(b, 2).ok()
            } else {
                clean.parse::<u64>().ok()
            }
        }
        Expr::Unary {
            op: UnOp::Pos,
            expr,
            ..
        } => const_index(expr),
        _ => None,
    }
}

/// The inner target of a `*rest`, or the target itself otherwise. Used when the
/// value type is unknown and every leaf binds loosely.
fn rest_inner(t: &BindTarget) -> &BindTarget {
    match t {
        BindTarget::Rest(inner, _) => inner,
        other => other,
    }
}

/// Substitute type-parameter names with concrete argument types in a type
/// expression (#606, transparent-alias expansion). A bare `Named` whose name is
/// a parameter in `subst` is replaced by the mapped type; every other node is
/// rebuilt with its children substituted.
fn subst_type(t: &Type, subst: &HashMap<&str, &Type>) -> Type {
    match t {
        Type::Named { name, args, span } => {
            if args.is_empty() {
                if let Some(replacement) = subst.get(name.as_str()) {
                    return (*replacement).clone();
                }
            }
            Type::Named {
                name: name.clone(),
                args: args.iter().map(|a| subst_type(a, subst)).collect(),
                span: *span,
            }
        }
        Type::Tuple { elems, span } => Type::Tuple {
            elems: elems.iter().map(|e| subst_type(e, subst)).collect(),
            span: *span,
        },
        Type::Func { params, ret, span } => Type::Func {
            params: params.iter().map(|p| subst_type(p, subst)).collect(),
            ret: ret.as_ref().map(|r| Box::new(subst_type(r, subst))),
            span: *span,
        },
    }
}

/// Collect the names of the aliases referenced anywhere in `t` (#606 cycle
/// detection). Only names present in `aliases` are recorded.
fn collect_alias_refs(
    t: &Type,
    aliases: &HashMap<String, (Vec<String>, Type)>,
    out: &mut Vec<String>,
) {
    match t {
        Type::Named { name, args, .. } => {
            if aliases.contains_key(name) {
                out.push(name.clone());
            }
            for a in args {
                collect_alias_refs(a, aliases, out);
            }
        }
        Type::Tuple { elems, .. } => {
            for e in elems {
                collect_alias_refs(e, aliases, out);
            }
        }
        Type::Func { params, ret, .. } => {
            for p in params {
                collect_alias_refs(p, aliases, out);
            }
            if let Some(r) = ret {
                collect_alias_refs(r, aliases, out);
            }
        }
    }
}

/// Type-check a module, returning any errors.
pub fn check(module: &Module) -> Vec<TypeError> {
    analyze(module).0
}

/// Type-check a module, returning `(errors, warnings)`. Warnings are non-fatal
/// diagnostics (e.g. the `Eq`-without-`Hash` dict/set key warning, D-0047).
pub fn analyze(module: &Module) -> (Vec<TypeError>, Vec<TypeError>) {
    // Prepend the built-in prelude (`Signal` enum, `MemStats` struct) so the
    // signals surface reuses ordinary enum/struct type-checking (ticket #77).
    let mut module = zapc_syntax::parser::with_prelude(module);
    // Resolve compile-time `when target.*` conditional compilation (I#139,
    // D-0079) before any checking, so dead arms are never type-checked against a
    // target they were not written for. The driver resolves against the active
    // `--target` first; here (e.g. the LSP path) the host target is used, and
    // the re-resolve is a no-op on an already-resolved module.
    zapc_syntax::cfg::resolve_cfg(&mut module, &zapc_syntax::cfg::Target::host());
    let module = &module;
    let mut c = Checker::new();
    c.collect_types(module);
    c.check_value_struct_recursion(module);
    c.check_bitfields(module);
    c.collect_sigs(module);
    c.validate_decorators(module);
    c.check_scoped(module);
    c.check_module(module);
    (c.errors, c.warnings)
}

/// Type-check a module in **analysis mode** (I#228), returning the same
/// `(errors, warnings)` as [`analyze`] *plus* the retained resolver state
/// ([`AnalysisFacts`]) that the normal path discards.
///
/// This runs the identical passes in the identical order as [`analyze`] — so
/// the diagnostics are byte-identical — and differs only in that the checker
/// records, as it goes, the per-expression types, the local use→def
/// resolutions, and the binding definition sites. The `zapc_analysis` crate
/// turns those facts into the position-keyed query surface.
///
/// The prelude is prepended exactly as in [`analyze`] (diagnostic parity); its
/// items carry no expression bodies, so they contribute no facts.
pub fn analyze_queryable(module: &Module) -> (Vec<TypeError>, Vec<TypeError>, AnalysisFacts) {
    let mut module = zapc_syntax::parser::with_prelude(module);
    zapc_syntax::cfg::resolve_cfg(&mut module, &zapc_syntax::cfg::Target::host());
    let module = &module;
    let mut c = Checker::new();
    c.analysis = Some(AnalysisFacts::default());
    c.collect_types(module);
    c.check_value_struct_recursion(module);
    c.check_bitfields(module);
    c.collect_sigs(module);
    c.validate_decorators(module);
    c.check_scoped(module);
    c.check_module(module);
    let facts = c.analysis.take().unwrap_or_default();
    (c.errors, c.warnings, facts)
}

/// Convenience for tests: lex, parse, and type-check `src`.
pub fn check_str(
    src: &str,
) -> (
    Vec<TypeError>,
    Vec<zapc_syntax::parser::ParseError>,
    Vec<zapc_syntax::lexer::LexError>,
) {
    let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
    let terrs = check(&module);
    (terrs, perrs, lerrs)
}

/// Convenience for tests: lex, parse, and type-check `src`, returning the
/// non-fatal warnings with per-site suppressions already applied (so a test
/// exercises the whole user-visible warning surface, [`apply_suppressions`]
/// included).
pub fn warnings_str(src: &str) -> Vec<TypeError> {
    let (module, _perrs, _lerrs) = zapc_syntax::parser::parse_str(src);
    apply_suppressions(src, analyze(&module).1)
}

/// Drop any warning whose source line carries a suppression pragma (#664,
/// D-0113). The pragma is a trailing line comment in one of two `# noqa`-style
/// forms, matching the Python convention the issue references:
///
/// - `# noqa` — suppress *every* warning reported on that line.
/// - `# noqa: ZAP5125[, ZAP5126, …]` — suppress only the listed codes.
///
/// Suppression is line-based and applies to the line of a warning's **start**
/// offset, so one pragma silences every access written on that one physical
/// line (and nothing on any other line). It is deliberately a warning-only
/// filter: errors are never suppressible. The scan only inspects the comment
/// tail after the first `#` on the line; a `#` inside a string literal earlier
/// on the line can therefore shadow a real trailing comment, which is an
/// accepted limitation of the lightweight line scan (documented in SPEC.md).
///
/// This is the single, pure, source-driven implementation shared by the
/// in-process test path ([`warnings_str`]) and the `zap` driver, so the
/// behavior a sema test observes is exactly the behavior a build produces.
pub fn apply_suppressions(src: &str, warnings: Vec<TypeError>) -> Vec<TypeError> {
    if warnings.is_empty() {
        return warnings;
    }
    // Byte offset of the start of each line, so a span start maps to its line.
    let mut line_starts: Vec<usize> = vec![0];
    for (i, b) in src.bytes().enumerate() {
        if b == b'\n' {
            line_starts.push(i + 1);
        }
    }
    let line_of = |offset: usize| -> usize {
        // The index of the greatest line-start <= offset (0-based line number).
        match line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        }
    };
    let lines: Vec<&str> = src.lines().collect();

    /// What a line's pragma suppresses: everything, or a specific code set.
    enum Suppress {
        All,
        Codes(Vec<ErrorCode>),
    }
    // Parse the suppression (if any) carried by one source line.
    fn parse_pragma(line: &str) -> Option<Suppress> {
        // The comment tail: everything after the first `#`.
        let hash = line.find('#')?;
        let comment = &line[hash + 1..];
        // Find the `noqa` token in the comment.
        let idx = comment.find("noqa")?;
        let rest = comment[idx + "noqa".len()..].trim_start();
        if let Some(list) = rest.strip_prefix(':') {
            let codes: Vec<ErrorCode> = list
                .split(',')
                .filter_map(|tok| ErrorCode::parse(tok.trim()))
                .collect();
            // `# noqa:` with no parseable code suppresses nothing (a typo should
            // not silently swallow every warning on the line).
            Some(Suppress::Codes(codes))
        } else {
            Some(Suppress::All)
        }
    }

    warnings
        .into_iter()
        .filter(|w| {
            let ln = line_of(w.span.start as usize);
            match lines.get(ln).and_then(|l| parse_pragma(l)) {
                Some(Suppress::All) => false,
                Some(Suppress::Codes(codes)) => !codes.contains(&w.code),
                None => true,
            }
        })
        .collect()
}

struct Checker {
    sigs: HashMap<String, FnSig>,
    user_types: HashSet<String>,
    /// Registered enums (tier-1 and tier-3), keyed by type name.
    enums: HashMap<String, EnumInfo>,
    /// Registered structs, keyed by type name (operator overloading, D-0047).
    structs: HashMap<String, StructInfo>,
    /// Registered structural interfaces, keyed by name (D-0016). A struct is
    /// assignable to an interface type iff it structurally conforms.
    interfaces: HashMap<String, InterfaceInfo>,
    /// Registered transparent type aliases (#606, D-0105), keyed by alias name:
    /// the declared generic parameter names and the target type they expand to.
    /// An alias is *not* a nominal type — [`Checker::resolve_type`] expands it to
    /// its target (substituting the generic parameters) so the alias and its
    /// target share one identity.
    aliases: HashMap<String, (Vec<String>, Type)>,
    /// Alias names that participate in a definition cycle (`type A = B; type B =
    /// A`), computed in [`Checker::collect_types`]. Resolving one yields
    /// `Ty::Unknown` instead of looping; the cycle itself is reported once.
    cyclic_aliases: HashSet<String>,
    /// Declared `annotation` names -> their field names (D-0036). Used to
    /// validate `@Annotation(...)` applications (keyword args must name fields).
    annotations: HashMap<String, Vec<String>>,
    /// Declared `decorator` names (behavioral, D-0036).
    user_decorators: HashSet<String>,
    /// Functions and methods carrying `@deprecated` (I#472, D-0109), mapping the
    /// call key to the optional user message. A free function is keyed by its
    /// bare name; a method by `"Type.method"`. Each call site resolving to one of
    /// these raises the non-fatal ZAP5124 use-site warning.
    deprecated: HashMap<String, Option<String>>,
    /// User structs marked `@shareable` (#664, D-0113), by type name. The
    /// sharing lint (ZAP5125) treats a mutable module global of such a type as
    /// safe. This is a *trusted assertion*, never verified. Built-in shareable
    /// types ([`SHAREABLE_MARKER`]-bearing primitives) are not stored here; they
    /// are recognized by [`Checker::name_is_shareable`] through the shared
    /// [`BUILTIN_SHAREABLE_TYPES`] seed, so the lint runs one uniform rule.
    shareable_types: HashSet<String>,
    /// Structs that are `@scoped` (#876, D-0159): an affine, with-bound resource.
    /// Seeded with structs carrying the `@scoped` decorator, then closed under
    /// *contagion* — a struct owning a `@scoped` field is itself `@scoped` — by a
    /// fixpoint in [`Checker::check_scoped`]. Keyed on the nominal base name, so a
    /// generic spelling is covered by its base. Consulted by the `@scoped`
    /// well-formedness, with-head-only, and escape checks (`crates/sema/src/scoped.rs`).
    scoped_types: HashSet<String>,
    /// The receiver name (`self`) whose field is a legal move target for a fresh
    /// `@scoped` value (#876, D-0159), set only while walking a method of a
    /// `@scoped` struct. `Some("self")` inside such a method; `None` in a free
    /// function, a non-`@scoped` struct's method, or anywhere else — so a
    /// `@scoped` value may be moved into `self.<field>` ONLY inside its owner's
    /// own method, never into an arbitrary object's field. See `scoped.rs`.
    scoped_self_base: Option<String>,
    /// Names of mutable module-level globals whose type is NOT `@shareable`
    /// (#664, D-0113). Each read or write of one of these — resolving to the
    /// module frame, not a shadowing local — raises the ZAP5125 sharing-lint
    /// warning. A `const` global is never recorded here (it can never race), nor
    /// is a mutable global of a `@shareable` type.
    share_lint_globals: HashSet<String>,
    scopes: Vec<HashMap<String, Ty>>,
    /// Immutability flag per binding, parallel to `scopes` (D-0003): a frame's
    /// set holds the names in the same `scopes` frame that were introduced by a
    /// `const` declaration. Maintained alongside `scopes` in `push_scope` /
    /// `pop_scope`; `declare_const` records a name here, every other `declare`
    /// path leaves it absent (mutable). A reassignment to a name that is const in
    /// its innermost binding frame is rejected (ZAP3017). Module-level `const`s
    /// (D-0112, #658 Phase 1) live in the outermost frame, so a reassignment from
    /// inside any function is caught too.
    const_scopes: Vec<HashSet<String>>,
    /// Interface bounds on the type parameters of the declaration currently being
    /// checked (I#164): type-parameter name -> bound interface name. Set while
    /// checking a bounded generic body so that a method call on a value of a
    /// bounded type parameter `T: Iface` resolves through `Iface` (static
    /// dispatch after monomorphization), and calling a method the bound does not
    /// provide is rejected at definition (check-at-definition, Fork M1).
    tyvar_bounds: HashMap<String, String>,
    /// Interface bounds on the type parameters of the *enclosing* type whose
    /// methods are currently being checked (I#811): e.g. the `K: Comparable` of
    /// `struct BST[K: Comparable, V]` while checking `BST`'s methods. These are
    /// merged into [`Self::tyvar_bounds`] by [`Self::check_function`] so a method
    /// body may call the bound's methods (`__cmp__`/`__hash__`) on a parameter
    /// typed as the enclosing type parameter — the user-container counterpart of
    /// the bounded-free-function body check. Empty for free functions.
    enclosing_type_bounds: Vec<(String, String)>,
    ret_ty: Ty,
    /// `true` while checking the body of a generator function (a free `def`
    /// whose body contains `yield`, I#67/D-0026). Gates `yield`/`return`
    /// validation.
    in_generator: bool,
    errors: Vec<TypeError>,
    /// Non-fatal diagnostics (e.g. `Eq`-without-`Hash` used as a dict/set key,
    /// D-0047). Reported by the driver but do not fail compilation.
    warnings: Vec<TypeError>,
    /// Analysis-mode sink (I#228). `None` on the normal compile path, so codegen
    /// builds record nothing and pay nothing beyond dropping a `Copy` span in
    /// `declare`. `Some` only under [`analyze_queryable`], where the checker
    /// retains the resolver decisions it would otherwise discard (expr types by
    /// span, local use→def resolutions, binding definition sites).
    analysis: Option<AnalysisFacts>,
    /// Parallel scope stack mirroring `scopes`, carrying each binding's
    /// definition span and kind so a name *use* can be linked to its definition
    /// *site* (go-to-def / references). Populated and maintained only while
    /// `analysis` is `Some`; the type-checking `scopes` and every `lookup`
    /// against it are untouched.
    def_scopes: Vec<HashMap<String, (Span, bool)>>,
    /// For each comprehension currently being checked (innermost last), the
    /// length of `scopes` just before that comprehension pushed its own scope
    /// (#600). A walrus binding inside a comprehension filter must land in the
    /// scope containing the **outermost** enclosing comprehension — PEP 572 —
    /// not the comprehension's own iteration scope, so `declare_walrus` targets
    /// the frame at `comp_scopes[0] - 1`. Empty outside any comprehension, where
    /// a walrus binds into the current innermost scope.
    comp_scopes: Vec<usize>,
}

/// The resolver state retained in analysis mode (I#228), handed to
/// `zapc_analysis` which turns it into the position-keyed query surface. All
/// three tables are empty unless the check ran via [`analyze_queryable`].
///
/// Spans are raw byte offsets ([`Span`]); presentation (line/col, UTF-16,
/// document symbols) lives in the analysis crate, not here.
#[derive(Clone, Debug, Default)]
pub struct AnalysisFacts {
    /// The inferred type of every expression the checker visited, keyed by the
    /// expression's span, in visitation (post-order) sequence. The basis of
    /// `type_at` / `symbol_at`.
    pub expr_types: Vec<(Span, Ty)>,
    /// `(use_span, def_span)` for every name *use* that resolved to a *local*
    /// binding (param, let, loop/comprehension var, match capture, …). Global
    /// resolutions (functions, types) are AST-derivable and are reconstructed in
    /// the analysis crate, so they are deliberately not recorded here.
    pub resolutions: Vec<(Span, Span)>,
    /// Every binding's definition site: `(def_span, ty, is_param)`. Lets
    /// `symbol_at` answer when the cursor sits on the definition itself.
    pub local_defs: Vec<(Span, Ty, bool)>,
}

/// The synthetic iterator type name for bare generator expressions (#596). Like
/// a generator's `$gen:<name>` type it carries the two `Iterator` dunders
/// (D-0048); unlike a generator every genexpr shares this one type because the
/// element type stays permissive on the consuming side (see
/// [`Checker::register_generators`]). Prefixed with `$` so it can never collide
/// with a user type.
const GENEXPR_ITER_TYPE: &str = "$genexpr";

const BUILTINS: &[&str] = &[
    "print",
    "range",
    "len",
    "sum",
    "dot",
    "concatenate",
    "stack",
    "where",
    "sorted",
    // String / char builtins (ticket #284, D-0025).
    "ord",
    "chr",
    "char",
    "str",
    "int",
    "float",
    "bigint",
    "complex",
    "bool",
    "format",
    "reflect_has",
    "reflect_names",
    // Function/callable signature reflection (I#458, D-0089).
    "reflect_params",
    "reflect_param_type",
    "reflect_param_has_default",
    "reflect_param_annotations",
    "reflect_sig",
    "checkpoint",
    "BaseException",
    "Exception",
    "ValueError",
    "TypeError",
    "LookupError",
    "IndexError",
    "KeyError",
    "ArithmeticError",
    "ZeroDivisionError",
    "RuntimeError",
    "StopIteration",
    "Cancelled",
    "ScopeTimeout",
    "ExceptionGroup",
    "TimeoutError",
    "AssertionError",
    // REPL session-binding slot intrinsics (I#159): synthesized by the `zap repl`
    // driver to persist `let` values across entries, never written by users.
    "__repl_load_i64",
    "__repl_load_ptr",
    "__repl_load_f64",
    "__repl_store_i64",
    "__repl_store_ptr",
    "__repl_store_f64",
    // Internal netpoller integration-test hook (ticket #80). No public I/O
    // surface exists yet (sockets #79, signals #77), so this drives the
    // netpoller end-to-end from the runtime; removed/replaced when #79 lands.
    "__netpoll_selftest",
    // Internal signal integration-test hook (ticket #77): drives the self-pipe +
    // netpoller + dispatch + cap-1 coalescing end-to-end from the runtime before
    // the full `signal` surface is exercised in a program.
    "__signal_selftest",
    // Internal test hooks (ticket #77): send a signal to this process, and block
    // until the signal thread has dispatched everything raised so far, so the
    // signal goldens are deterministic. Not public surface.
    "__raise_signal",
    "__signal_drain",
    // Internal finalizer integration-test hook (ticket #92): exercises the
    // precise collector's register + post-mark finalizer queue end-to-end with a
    // stand-in finalizer and returns the number that ran. Not public surface.
    "__gc_finalizer_selftest",
];

fn is_exception_type(name: &str) -> bool {
    matches!(
        name,
        "BaseException"
            | "Exception"
            | "ValueError"
            | "TypeError"
            | "LookupError"
            | "IndexError"
            | "KeyError"
            | "ArithmeticError"
            | "ZeroDivisionError"
            | "RuntimeError"
            | "StopIteration"
            | "Cancelled"
            | "ScopeTimeout"
            | "ExceptionGroup"
            | "TimeoutError"
            | "AssertionError"
    )
}

/// The classification of an `arena` allocation's first (type) argument (#553,
/// D-0158): a `bytes` buffer, a leaf struct, a machine scalar, or an invalid /
/// already-reported argument.
enum ArenaElem {
    Bytes,
    LeafStruct(String),
    Scalar,
    Bad,
}

/// A pointer-free machine-scalar type name — the leaf set an arena object may be
/// built from (#553, D-0158 §5.1). Deliberately excludes `bigint`/`complex` (both
/// GC-managed, i.e. pointer-bearing) and every aggregate/reference type.
fn is_leaf_scalar_name(n: &str) -> bool {
    matches!(
        n,
        "int32"
            | "int64"
            | "uint8"
            | "uint16"
            | "uint32"
            | "uint64"
            | "usize"
            | "float32"
            | "float64"
            | "bool"
            | "char"
    )
}

/// Is `t` a pointer-free machine scalar (the leaf-field test for `a.new(T, ...)`,
/// #553)? Mirrors [`is_leaf_scalar_name`] over resolved [`Ty`] values; any
/// aggregate, reference, `Named`, `Str`/`Bytes`, `Bigint`, `Complex`, or `Py`
/// field makes the owning struct non-leaf (fails safe to reject).
fn is_leaf_scalar_ty(t: &Ty) -> bool {
    matches!(
        t,
        Ty::Int64
            | Ty::Int32
            | Ty::Uint8
            | Ty::Uint16
            | Ty::Uint32
            | Ty::Uint64
            | Ty::Usize
            | Ty::Float64
            | Ty::Float32
            | Ty::Bool
            | Ty::Char
    )
}

/// The curated built-in decorator set shipped in v1 (D-0036,
/// `design/0004-decorators.md` §"Built-in set"). An `@name` whose name is not in
/// this set, not a declared `annotation`, and not a declared `decorator` is a
/// compile-time error (consistent with compile-time typo-catching, D-0016).
const BUILTIN_DECORATORS: &[&str] = &[
    "derive",
    "cache",
    "default",
    "validate",
    "range",
    "deprecated",
    "inject",
    // Concurrency-safety marker (#664, D-0113): `@shareable` on a `struct`
    // asserts the type is safe under concurrent shared access (a trusted,
    // unverified claim), reflectable via `reflect_has(T, "shareable")`.
    SHAREABLE_MARKER,
    // Affine with-bound resource marker (#876, D-0159): `@scoped` on a `struct`
    // makes it a with-bound affine resource (must define __enter__/__exit__,
    // contagious through ownership). See `docs/design/scoped.md`.
    SCOPED_MARKER,
    // JSON codec hooks (ticket #346, D-0086): `@json(timestamp="epoch"|"iso8601")`
    // on a `DateTime` field selects its wire form; `@json(open)` on an enum marks
    // it an open (forward-compatible) string enum. See `design/0010-json.md`.
    "json",
    // Signals (ticket #77, D-0058): `@on_signal(Signal.X)` on a function
    // registers it as a handler for signal X (sugar over `signal.notify`).
    "on_signal",
    // Test framework (D-0054, issue #73). These mark functions for `zap test`;
    // normal builds strip the annotated functions, so they never reach codegen.
    "test",
    "before",
    "after",
    "before_all",
    "after_all",
    "parametrize",
    "skip",
    "skip_if",
    "expect_raises",
];

/// The `@test`-suite decorators (D-0054). A function carrying any of these is a
/// test or a lifecycle hook: it is compiled only under `zap test` and excluded
/// from release binaries.
pub const TEST_DECORATORS: &[&str] = &[
    "test",
    "before",
    "after",
    "before_all",
    "after_all",
    "parametrize",
    "skip",
    "skip_if",
    "expect_raises",
];

/// Does `f` carry any `@test`-suite decorator (so it belongs to the test
/// harness, not a release binary)?
pub fn is_test_item(f: &zapc_syntax::ast::Function) -> bool {
    f.decorators
        .iter()
        .any(|d| TEST_DECORATORS.contains(&d.name.as_str()))
}

/// If `decorators` carries `@deprecated` (I#472, D-0109), return its optional
/// message: `Some(None)` for a bare `@deprecated`, `Some(Some(msg))` for
/// `@deprecated("msg")`, and `None` when the function is not deprecated. A
/// non-string-literal argument (already an ill-formed application, ZAP5042) is
/// treated as "no message" so the warning still fires.
fn deprecated_message(decorators: &[zapc_syntax::ast::Decorator]) -> Option<Option<String>> {
    let d = decorators.iter().find(|d| d.name == "deprecated")?;
    let msg = d.args.first().and_then(|a| match &a.value {
        Expr::Str { value, .. } => Some(value.clone()),
        _ => None,
    });
    Some(msg)
}

/// The traits `@derive(...)` can generate (D-0036). Only `Eq` and `Repr` are
/// lowered to code in this increment; the rest are accepted here and reported as
/// deferred by the lowering pass.
const DERIVABLE_TRAITS: &[&str] = &["Eq", "Ord", "Json", "Csv", "Yaml", "Repr", "Hash"];

/// Where a decorator/annotation is attached — used for target-suitability
/// checks (e.g. `@derive` only on a struct).
#[derive(Clone, Copy, PartialEq)]
enum DecoTarget {
    Struct,
    Field,
    Function,
    Enum,
    Param,
}

impl DecoTarget {
    fn describe(self) -> &'static str {
        match self {
            DecoTarget::Struct => "a struct",
            DecoTarget::Field => "a field",
            DecoTarget::Function => "a function",
            DecoTarget::Enum => "an enum",
            DecoTarget::Param => "a parameter",
        }
    }
}

/// A built-in scalar name the backend cannot yet lay out as a collection slot —
/// an honest *representation-readiness* gap under the capability model
/// (D-0139/#807), not a type-law failure. `f64` is **not** one of these: it is a
/// first-class collection element in every position (#800/#803).
fn is_f32_name(n: &str) -> bool {
    matches!(n, "f32" | "float32")
}
/// Built-in collections whose element slot is where the `f32` representation gap
/// bites. `array` is deliberately absent — a dense numeric `array` carries its
/// own dtype slots (incl. `f32`), so it is not gated here.
fn is_f32_gap_container(n: &str) -> bool {
    matches!(n, "list" | "heap" | "channel" | "dict" | "set")
}
/// The honest diagnostic text for the remaining collection-element
/// representation gaps (D-0139/#807): `f32`/`float32`, and `bigint`/`complex`.
const F32_COLLECTION_MSG: &str =
    "'f32'/'float32' is not supported as a collection element yet (only 'f64' is); \
     use 'array[f32]' for dense f32 data";
const BIGINT_COMPLEX_COLLECTION_MSG: &str =
    "bigint and complex values are not supported as collection elements yet";

impl Checker {
    /// Representation-readiness gate for the element/key/value *positions* of the
    /// built-in collections (capability model, D-0139/#807). Each position
    /// carries a semantic capability bound —
    ///
    /// | container | element / key bound |
    /// |---|---|
    /// | `list[T]` / `array[T]` / `channel[T]`, and `dict` VALUE | none |
    /// | `dict[K, _]` key, `set[T]` | `Hashable` (⊃ `Equatable`) |
    /// | `heap[T]` | `Comparable` (⊃ `Equatable`) |
    ///
    /// — but every type admitted here already satisfies its position's bound
    /// (`f64` included, #800/#803), so what actually gates admission is whether
    /// the backend can lay out a slot for the element yet. The built-in scalars
    /// whose slot is not yet lowered are an honest representation gap, not a
    /// type-law failure: `f32`/`float32` (`ZAP4027`) and `bigint`/`complex`
    /// (`ZAP4024`). This is the syntactic (`Type`) gate run during collection;
    /// [`Self::check_float_container_value`] / [`Self::check_numeric_container_value`]
    /// are the resolved-type (`Ty`) gate run on expressions.
    fn check_numeric_container_type(&mut self, ty: &Type) {
        let span = match ty {
            Type::Named { span, .. } | Type::Tuple { span, .. } | Type::Func { span, .. } => *span,
        };
        // An `f32`/`float32` slot is a representation gap iff some collection node
        // gated for it holds `f32`/`float32` as a *direct* type argument — e.g.
        // `dict[f32, _]`, `set[f32]`, `list[list[f32]]` (the inner `list`). It is
        // reached only through gap containers, never through a tuple or an
        // unrelated generic (those are a separate representation question), which
        // preserves the pre-capability behaviour exactly while dropping the old
        // per-container-name match arms.
        fn is_f32_arg(a: &Type) -> bool {
            matches!(a, Type::Named { name, .. } if is_f32_name(name))
        }
        fn f32_slot(ty: &Type) -> bool {
            match ty {
                Type::Named { name, args, .. } => {
                    (is_f32_gap_container(name) && args.iter().any(is_f32_arg))
                        || args.iter().any(f32_slot)
                }
                Type::Tuple { elems, .. } => elems.iter().any(f32_slot),
                Type::Func { params, ret, .. } => {
                    params.iter().any(f32_slot) || ret.as_deref().is_some_and(f32_slot)
                }
            }
        }
        if f32_slot(ty) {
            self.error(ErrorCode::ZAP4027, F32_COLLECTION_MSG.to_string(), span);
        }
        // `bigint`/`complex` have no collection slot in any container (`array`
        // included). Once inside a container, the gap applies at any depth.
        fn contains_numeric(ty: &Type, in_container: bool) -> bool {
            match ty {
                Type::Named { name, args, .. } => {
                    let is_container = matches!(
                        name.as_str(),
                        "list" | "dict" | "set" | "array" | "heap" | "channel"
                    );
                    let now_in_container = in_container || is_container;
                    (now_in_container && matches!(name.as_str(), "bigint" | "complex"))
                        || args
                            .iter()
                            .any(|arg| contains_numeric(arg, now_in_container))
                }
                Type::Tuple { elems, .. } => elems
                    .iter()
                    .any(|elem| contains_numeric(elem, in_container)),
                Type::Func { params, ret, .. } => {
                    params
                        .iter()
                        .any(|param| contains_numeric(param, in_container))
                        || ret
                            .as_deref()
                            .is_some_and(|ret| contains_numeric(ret, in_container))
                }
            }
        }

        if contains_numeric(ty, false) {
            let span = match ty {
                Type::Named { span, .. } | Type::Tuple { span, .. } | Type::Func { span, .. } => {
                    *span
                }
            };
            self.error(
                ErrorCode::ZAP4024,
                BIGINT_COMPLEX_COLLECTION_MSG.to_string(),
                span,
            );
        }
    }

    /// Resolved-type (`Ty`) sibling of the `bigint`/`complex` representation gate
    /// (D-0139/#807): `bigint`/`complex` have no collection slot in any container.
    fn check_numeric_container_value(&mut self, ty: &Ty, span: Span) {
        fn contains_numeric(ty: &Ty, in_container: bool) -> bool {
            match ty {
                Ty::Bigint | Ty::Complex => in_container,
                Ty::List(elem) | Ty::Set(elem) | Ty::Heap(elem) | Ty::Chan(elem) => {
                    contains_numeric(elem, true)
                }
                Ty::Dict(key, value) => {
                    contains_numeric(key, true) || contains_numeric(value, true)
                }
                Ty::Tuple(elems) => elems
                    .iter()
                    .any(|elem| contains_numeric(elem, in_container)),
                _ => false,
            }
        }

        if contains_numeric(ty, false) {
            self.error(
                ErrorCode::ZAP4024,
                BIGINT_COMPLEX_COLLECTION_MSG.to_string(),
                span,
            );
        }
    }

    /// Resolved-type (`Ty`) sibling of the `f32` representation gate
    /// (D-0139/#807). Every built-in collection admits an `f64` element in every
    /// position — value positions (list I#569, dict VALUE / heap / channel #800
    /// Phase 1) and the hashed/ordered positions that need `Hashable`/`Comparable`
    /// (dict KEY, set MEMBER, heap — #800 Phase 2 / #803, `f64` satisfying them
    /// intrinsically with `-0.0` canonicalised and NaN rejected at runtime,
    /// D-0138). Only an `f32`/`float32` element (`Ty::Float32`) is still a
    /// representation gap; an inferred `UntypedFloat` defaults to `f64`, so it is
    /// admitted.
    fn check_float_container_value(&mut self, ty: &Ty, span: Span) {
        fn unsupported(ty: &Ty) -> bool {
            match ty {
                Ty::List(t) | Ty::Heap(t) | Ty::Chan(t) | Ty::Set(t) => {
                    matches!(**t, Ty::Float32) || unsupported(t)
                }
                Ty::Dict(k, v) => {
                    matches!(**k, Ty::Float32)
                        || matches!(**v, Ty::Float32)
                        || unsupported(k)
                        || unsupported(v)
                }
                Ty::Tuple(ts) => ts.iter().any(unsupported),
                _ => false,
            }
        }
        if unsupported(ty) {
            self.error(ErrorCode::ZAP4027, F32_COLLECTION_MSG.to_string(), span);
        }
    }

    fn new() -> Checker {
        Checker {
            sigs: HashMap::new(),
            user_types: HashSet::new(),
            enums: HashMap::new(),
            structs: HashMap::new(),
            interfaces: HashMap::new(),
            aliases: HashMap::new(),
            cyclic_aliases: HashSet::new(),
            annotations: HashMap::new(),
            user_decorators: HashSet::new(),
            deprecated: HashMap::new(),
            shareable_types: HashSet::new(),
            scoped_types: HashSet::new(),
            scoped_self_base: None,
            share_lint_globals: HashSet::new(),
            scopes: Vec::new(),
            const_scopes: Vec::new(),
            tyvar_bounds: HashMap::new(),
            enclosing_type_bounds: Vec::new(),
            ret_ty: Ty::Unit,
            in_generator: false,
            errors: Vec::new(),
            warnings: Vec::new(),
            analysis: None,
            def_scopes: Vec::new(),
            comp_scopes: Vec::new(),
        }
    }

    fn error(&mut self, code: ErrorCode, message: impl Into<String>, span: Span) {
        self.errors.push(TypeError {
            code,
            message: message.into(),
            span,
        });
    }

    fn warn(&mut self, code: ErrorCode, message: impl Into<String>, span: Span) {
        self.warnings.push(TypeError {
            code,
            message: message.into(),
            span,
        });
    }

    // --- scopes ---

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
        self.const_scopes.push(HashSet::new());
        if self.analysis.is_some() {
            self.def_scopes.push(HashMap::new());
        }
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
        self.const_scopes.pop();
        if self.analysis.is_some() {
            self.def_scopes.pop();
        }
    }

    /// Push the iteration scope of a comprehension, recording the enclosing
    /// scope depth so a walrus target inside it binds in the containing scope
    /// (#600). Paired with [`pop_comp_scope`].
    fn push_comp_scope(&mut self) {
        self.comp_scopes.push(self.scopes.len());
        self.push_scope();
    }

    fn pop_comp_scope(&mut self) {
        self.pop_scope();
        self.comp_scopes.pop();
    }

    /// Declare a walrus target `name: ty` (#600). Outside any comprehension the
    /// binding lands in the current innermost scope — which, for an `if`/`while`
    /// condition, is already the enclosing block (the condition is checked
    /// before the body's scope is pushed), so the name outlives the construct.
    /// Inside one or more comprehensions it lands in the scope containing the
    /// outermost comprehension (PEP 572), so it is visible both in the
    /// comprehension body and after the comprehension.
    fn declare_walrus(&mut self, name: &str, ty: Ty, def: Span) {
        let idx = match self.comp_scopes.first() {
            Some(&len) => len.saturating_sub(1),
            None => self.scopes.len().saturating_sub(1),
        };
        if let Some(scope) = self.scopes.get_mut(idx) {
            scope.insert(name.to_string(), ty.clone());
        }
        if let Some(facts) = &mut self.analysis {
            facts.local_defs.push((def, ty, false));
            if let Some(ds) = self.def_scopes.get_mut(idx) {
                ds.insert(name.to_string(), (def, false));
            }
        }
    }

    /// Introduce a binding `name: ty` whose definition is at `def`. `def` is a
    /// `Copy` span dropped on the normal compile path; only in analysis mode is
    /// the definition site recorded (into `def_scopes` for use→def linkage and
    /// into `local_defs` for symbol-at-definition). Type-checking reads only the
    /// `scopes` entry, exactly as before.
    fn declare(&mut self, name: &str, ty: Ty, def: Span) {
        self.declare_kind(name, ty, def, false);
    }

    /// As [`declare`], marking the binding a parameter (vs. an ordinary local)
    /// so the analysis surface can distinguish the two.
    fn declare_param(&mut self, name: &str, ty: Ty, def: Span) {
        self.declare_kind(name, ty, def, true);
    }

    fn declare_kind(&mut self, name: &str, ty: Ty, def: Span, is_param: bool) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), ty.clone());
        }
        if let Some(facts) = &mut self.analysis {
            facts.local_defs.push((def, ty, is_param));
            if let Some(ds) = self.def_scopes.last_mut() {
                ds.insert(name.to_string(), (def, is_param));
            }
        }
    }

    /// Declare an immutable binding (`const`, D-0003): like [`declare`], but also
    /// records the name as const in the innermost frame so a later reassignment
    /// is rejected (ZAP3017). A fresh `const` shadowing a mutable binding in an
    /// inner scope is fine — constness is per binding frame.
    fn declare_const(&mut self, name: &str, ty: Ty, def: Span) {
        self.declare(name, ty, def);
        if let Some(frame) = self.const_scopes.last_mut() {
            frame.insert(name.to_string());
        }
    }

    /// Whether `name`'s innermost binding is a `const` (D-0003). Scans frames
    /// from the top exactly like [`lookup`], so an inner mutable binding shadows
    /// an outer `const` and vice versa; the first frame that binds the name
    /// decides. Unbound names are not const (a plain assignment to an undeclared
    /// name is a different diagnostic, ZAP3002).
    fn is_const_binding(&self, name: &str) -> bool {
        for (vars, consts) in self.scopes.iter().rev().zip(self.const_scopes.iter().rev()) {
            if vars.contains_key(name) {
                return consts.contains(name);
            }
        }
        false
    }

    /// Reject reassigning a `const` binding (D-0003, ZAP3017). Returns `true`
    /// when an error was raised, so the caller can skip the normal assignment
    /// type-check (the binding is immutable regardless).
    fn reject_const_reassign(&mut self, name: &str, span: Span) -> bool {
        if self.is_const_binding(name) {
            self.error(
                ErrorCode::ZAP3017,
                format!("cannot reassign '{name}': it is declared `const` (immutable)"),
                span,
            );
            true
        } else {
            false
        }
    }

    fn lookup(&self, name: &str) -> Option<Ty> {
        for scope in self.scopes.iter().rev() {
            if let Some(t) = scope.get(name) {
                return Some(t.clone());
            }
        }
        None
    }

    /// The definition span of the innermost local binding of `name`, if any
    /// (analysis mode only). Mirrors [`lookup`] against `def_scopes`.
    fn lookup_def(&self, name: &str) -> Option<Span> {
        for scope in self.def_scopes.iter().rev() {
            if let Some((def, _)) = scope.get(name) {
                return Some(*def);
            }
        }
        None
    }

    // --- sharing lint (#664, D-0113) ---------------------------------------

    /// Is a type named `name` `@shareable` (#664, D-0113)? The one uniform rule
    /// the sharing lint consults: a user `struct` marked `@shareable`, or one of
    /// the built-in concurrency primitives / concurrent collections that carry
    /// the marker implicitly ([`BUILTIN_SHAREABLE_TYPES`]). No other allowlist.
    fn name_is_shareable(&self, name: &str) -> bool {
        self.shareable_types.contains(name) || BUILTIN_SHAREABLE_TYPES.contains(&name)
    }

    /// Is a module global's declared type `@shareable`? Decided on the type's
    /// nominal base name, so a generic spelling (`Atomic[int64]`, `Queue[str]`)
    /// is covered by its base. `ty` is the declared-type AST when the global
    /// carried an explicit annotation; otherwise the inferred `resolved` type is
    /// consulted (a user struct resolves to `Ty::Named`). Scalars and the
    /// built-in containers (`list`/`dict`/`set`/…) are never shareable.
    fn declared_type_is_shareable(&self, ty: Option<&Type>, resolved: &Ty) -> bool {
        if let Some(Type::Named { name, .. }) = ty {
            return self.name_is_shareable(name);
        }
        match resolved {
            Ty::Named(n) => self.name_is_shareable(n),
            _ => false,
        }
    }

    /// Does a bare `name` resolve to the module-global frame (frame 0) — i.e. a
    /// module-level binding not shadowed by a parameter or local? Scans frames
    /// top-down exactly like [`lookup`]; the first frame that binds the name
    /// decides, so a shadowing local correctly wins.
    fn resolves_to_module_global(&self, name: &str) -> bool {
        for (i, scope) in self.scopes.iter().enumerate().rev() {
            if scope.contains_key(name) {
                return i == 0;
            }
        }
        false
    }

    /// Note one access (read or write) of `name` for the sharing lint (ZAP5125,
    /// #664/D-0113). Warns iff `name` is a mutable module global of a
    /// non-`@shareable` type (recorded in `share_lint_globals`), the access is
    /// inside a function/method body (`scopes.len() > 1`, so the single-threaded
    /// module-init pre-pass is exempt), and the name actually resolves to the
    /// module frame rather than a shadowing local. Per-site suppression is
    /// applied later, line-based, on the rendered warnings (see
    /// [`apply_suppressions`]).
    fn note_share_access(&mut self, name: &str, span: Span) {
        if self.scopes.len() > 1
            && self.share_lint_globals.contains(name)
            && self.resolves_to_module_global(name)
        {
            self.warn(
                ErrorCode::ZAP5125,
                format!(
                    "access to shared mutable global '{name}' of a non-@shareable type; \
                     under the light concurrency model coroutines share it without \
                     synchronization (#661/#664). Mark its type @shareable, use an \
                     Atomic/Mutex or a concurrent collection, make it `const`, or \
                     suppress this site with `# noqa`"
                ),
                span,
            );
        }
    }

    // --- collection passes ---

    fn collect_types(&mut self, module: &Module) {
        // The compiler-known capability interfaces (D-0139/#806) exist before any
        // user item, so a `[K: Hashable]` bound or a conformance check resolves
        // even in a module that never declares them. A user interface of the same
        // name (declared below) overwrites, keeping the names in the user's hands.
        self.register_builtin_capabilities();
        // First register every type name so field/payload types (which may
        // reference a type declared later, or the enum itself — a recursive
        // variant, D-0046) resolve in the second pass.
        for item in &module.items {
            match item {
                Item::Struct(s) => {
                    self.user_types.insert(s.name.clone());
                }
                Item::Enum(e) => {
                    self.user_types.insert(e.name.clone());
                }
                Item::Interface(i) => {
                    self.user_types.insert(i.name.clone());
                }
                Item::TypeAlias(a) => {
                    // A transparent alias is not a nominal type, so it is not
                    // added to `user_types`; it is expanded to its target in
                    // `resolve_type`. Record its parameters and target here.
                    self.aliases.insert(
                        a.name.clone(),
                        (
                            a.generics.iter().map(|g| g.name.clone()).collect(),
                            a.target.clone(),
                        ),
                    );
                }
                Item::Annotation(a) => {
                    self.user_types.insert(a.name.clone());
                    self.annotations.insert(
                        a.name.clone(),
                        a.fields.iter().map(|f| f.name.clone()).collect(),
                    );
                }
                Item::Decorator(d) => {
                    self.user_decorators.insert(d.name.clone());
                }
                _ => {}
            }
        }
        // Second pass: build the enum and struct registries now that all type
        // names exist (fields/methods may reference a type declared later).
        for item in &module.items {
            match item {
                Item::Enum(e) => {
                    for v in &e.variants {
                        for f in &v.fields {
                            self.check_numeric_container_type(&f.ty);
                        }
                    }
                    let is_sum = e.variants.iter().any(|v| !v.fields.is_empty());
                    let variants = e
                        .variants
                        .iter()
                        .map(|v| EnumVariantInfo {
                            name: v.name.clone(),
                            fields: v
                                .fields
                                .iter()
                                .map(|f| (f.name.clone(), self.resolve_type(&f.ty)))
                                .collect(),
                        })
                        .collect();
                    let derives_json = e.decorators.iter().any(|d| {
                        d.name == "derive"
                            && d.args
                                .iter()
                                .any(|a| matches!(&a.value, Expr::Name(t, _) if t == "Json"))
                    });
                    // Enum method signatures (D-0046, #429/#449), resolved exactly
                    // like a struct's (`self` typed as the enum) so the enum can be
                    // checked for structural interface conformance (D-0016/#731).
                    let mut methods: HashMap<String, FnSig> = HashMap::new();
                    for m in &e.methods {
                        methods.insert(m.name.clone(), self.sig_of_method(m, &e.name));
                    }
                    self.enums.insert(
                        e.name.clone(),
                        EnumInfo {
                            is_sum,
                            variants,
                            derives_json,
                            methods,
                        },
                    );
                }
                Item::Struct(s) => {
                    for f in &s.fields {
                        self.check_numeric_container_type(&f.ty);
                    }
                    let info = self.struct_info_of(s);
                    self.structs.insert(s.name.clone(), info);
                    // Record a `@shareable` assertion (#664, D-0113) for the
                    // sharing lint and reflection. A trusted, unverified claim.
                    if s.decorators.iter().any(|d| d.name == SHAREABLE_MARKER) {
                        self.shareable_types.insert(s.name.clone());
                    }
                    // Seed the `@scoped` set (#876, D-0159) with explicitly
                    // marked structs; contagion closes it in `check_scoped`.
                    if s.decorators.iter().any(|d| d.name == SCOPED_MARKER) {
                        self.scoped_types.insert(s.name.clone());
                    }
                }
                Item::Interface(i) => {
                    for m in &i.methods {
                        for p in &m.params {
                            if let Some(t) = &p.ty {
                                self.check_numeric_container_type(t);
                            }
                        }
                        if let Some(t) = &m.ret {
                            self.check_numeric_container_type(t);
                        }
                    }
                    let info = self.interface_info_of(i);
                    self.interfaces.insert(i.name.clone(), info);
                }
                _ => {}
            }
        }
        // Detect transparent-alias cycles (#606): an alias whose target expands,
        // directly or via other aliases, back to itself. Such aliases are marked
        // so `resolve_type` yields `Unknown` instead of looping, and each cycle
        // is reported once with a clear diagnostic.
        self.detect_alias_cycles(module);
        self.check_interface_refinements(module);
        self.auto_derive_value_aggregates();
    }

    /// Register the auto-derived `Equatable`+`Hashable` pair on every qualifying
    /// `value struct` (D-0139/#808), now that all structs/enums are collected. A
    /// value struct auto-derives iff it declares NEITHER `__eq__` nor `__hash__`
    /// (hand-written or `@derive`d — either one is an all-or-nothing override) and
    /// every field is hashable ([`Self::ty_auto_hashable`]). Registering the pair
    /// in `StructInfo` is what makes `==`/`!=`, the `Hashable` bound, and dict/set
    /// key use resolve in sema; the field-wise method *bodies* are generated in
    /// lowering (`crates/ir`), and the two sides share the same hashability rule.
    /// `Comparable`/`Ord` stays explicit (ordering is a choice), and reference
    /// structs never auto-derive (identity is their Python default, D-0023).
    fn auto_derive_value_aggregates(&mut self) {
        let mut to_inject: Vec<String> = Vec::new();
        for (name, info) in &self.structs {
            if !info.is_value
                || info.methods.contains_key("__eq__")
                || info.methods.contains_key("__hash__")
            {
                continue;
            }
            let fields = info.fields.clone();
            let mut visited = HashSet::new();
            if fields
                .iter()
                .all(|t| self.ty_auto_hashable(t, &mut visited))
            {
                to_inject.push(name.clone());
            }
        }
        for name in to_inject {
            let named = Ty::Named(name.clone());
            if let Some(info) = self.structs.get_mut(&name) {
                info.methods.insert(
                    "__eq__".into(),
                    FnSig {
                        params: vec![param(named.clone()), param(named.clone())],
                        ret: Ty::Bool,
                    },
                );
                info.methods.insert(
                    "__hash__".into(),
                    FnSig {
                        params: vec![param(named.clone())],
                        ret: Ty::Int64,
                    },
                );
            }
        }
    }

    /// Is `t` hashable as a field of an auto-derived value aggregate
    /// (D-0139/#808)? The sema mirror of the lowering predicate
    /// (`crates/ir` `field_type_hashable`): scalars (incl. float, #803),
    /// `str`/`bytes`/`char`, a tier-1 enum, and a tuple or `value struct` all of
    /// whose fields/elements are hashable; a struct declaring both `__eq__` and
    /// `__hash__` (hand or `@derive`) is hashable, a partial override is not, and
    /// a reference struct without the pair, a tier-3 enum, and every mutable
    /// collection (`list`/`dict`/`set`/`heap`/`channel`) are not. Cycle-safe.
    fn ty_auto_hashable(&self, t: &Ty, visited: &mut HashSet<String>) -> bool {
        match t {
            Ty::Int64
            | Ty::Int32
            | Ty::Uint8
            | Ty::Uint16
            | Ty::Uint32
            | Ty::Uint64
            | Ty::Usize
            | Ty::Bool
            | Ty::Char
            | Ty::Str
            | Ty::Bytes
            | Ty::Float64
            | Ty::Float32
            | Ty::UntypedInt
            | Ty::UntypedFloat => true,
            Ty::Tuple(elems) => elems.iter().all(|e| self.ty_auto_hashable(e, visited)),
            Ty::Named(n) => {
                if let Some(e) = self.enums.get(n) {
                    // Tier-1 enums hash as their backing int; tier-3 (sum) enums
                    // are a value aggregate deferred to #809.
                    return !e.is_sum;
                }
                if !visited.insert(n.clone()) {
                    return true; // cycle guard (value structs cannot truly cycle)
                }
                let r = match self.structs.get(n) {
                    Some(info) => {
                        let has_eq = info.methods.contains_key("__eq__");
                        let has_hash = info.methods.contains_key("__hash__");
                        if has_eq && has_hash {
                            true
                        } else if info.is_value && !has_eq && !has_hash {
                            info.fields
                                .clone()
                                .iter()
                                .all(|f| self.ty_auto_hashable(f, visited))
                        } else {
                            false
                        }
                    }
                    None => false,
                };
                visited.remove(n);
                r
            }
            _ => false,
        }
    }

    /// Validate `interface X: Y` refinement (D-0139/#806) at each definition
    /// site, now that every interface name is registered:
    ///   * each supertype must name an **interface** (ZAP4081);
    ///   * a supertype may not be listed **twice** (ZAP4083);
    ///   * refinement must be **acyclic** — no interface may refine, directly or
    ///     transitively, back to itself (ZAP4082).
    ///
    /// The built-in capability interfaces are well-formed by construction, so only
    /// user `interface` items are checked here.
    fn check_interface_refinements(&mut self, module: &Module) {
        // Diagnostics are collected first (immutable borrows of `self`), then
        // emitted, so the interfaces map is not borrowed while `self.error` runs.
        let mut diags: Vec<(ErrorCode, String, Span)> = Vec::new();
        for item in &module.items {
            let Item::Interface(i) = item else { continue };
            // Non-interface / unknown supertype, and duplicate supertype. A
            // supertype `type` that is not a plain `Named` (a tuple or fn type)
            // is likewise not an interface.
            let mut seen: HashSet<String> = HashSet::new();
            for sup in &i.supertypes {
                let (name, span) = match sup {
                    Type::Named { name, span, .. } => (name.clone(), *span),
                    Type::Tuple { span, .. } | Type::Func { span, .. } => {
                        diags.push((
                            ErrorCode::ZAP4081,
                            format!("interface '{}' cannot refine a non-interface type", i.name),
                            *span,
                        ));
                        continue;
                    }
                };
                if !seen.insert(name.clone()) {
                    diags.push((
                        ErrorCode::ZAP4083,
                        format!(
                            "interface '{}' lists supertype '{name}' more than once",
                            i.name
                        ),
                        span,
                    ));
                    continue;
                }
                if !self.is_interface(&name) {
                    diags.push((
                        ErrorCode::ZAP4081,
                        format!(
                            "interface '{}' cannot refine '{name}': '{name}' is not an \
                             interface",
                            i.name
                        ),
                        span,
                    ));
                }
            }
            // Cyclic refinement: can `i` reach itself through the supertype graph?
            if self.refinement_reaches(&i.name, &i.name) {
                diags.push((
                    ErrorCode::ZAP4082,
                    format!("interface '{}' refines itself (cyclic refinement)", i.name),
                    i.span,
                ));
            }
        }
        for (code, msg, span) in diags {
            self.error(code, msg, span);
        }
    }

    /// Does interface `from` reach `target` by following refined supertypes one or
    /// more edges (D-0139/#806)? Used to detect cyclic refinement; cycle-safe via
    /// a visited set so the walk itself cannot loop.
    fn refinement_reaches(&self, from: &str, target: &str) -> bool {
        let mut visited = HashSet::new();
        let mut stack: Vec<String> = self
            .interfaces
            .get(from)
            .map(|i| i.supertypes.clone())
            .unwrap_or_default();
        while let Some(cur) = stack.pop() {
            if cur == target {
                return true;
            }
            if !visited.insert(cur.clone()) {
                continue;
            }
            if let Some(iface) = self.interfaces.get(&cur) {
                stack.extend(iface.supertypes.iter().cloned());
            }
        }
        false
    }

    /// Mark every alias that lies on a definition cycle and report it (#606,
    /// ZAP4090). Uses an iterative colour DFS over the alias dependency graph
    /// (edge `A -> B` whenever alias `B`'s name appears anywhere in alias `A`'s
    /// target type). An alias is cyclic iff it can reach itself.
    fn detect_alias_cycles(&mut self, module: &Module) {
        // Reachability of alias names from each alias's target.
        let mut deps: HashMap<String, Vec<String>> = HashMap::new();
        for (name, (_params, target)) in &self.aliases {
            let mut referenced = Vec::new();
            collect_alias_refs(target, &self.aliases, &mut referenced);
            deps.insert(name.clone(), referenced);
        }
        // An alias is cyclic iff it is reachable from itself.
        let names: Vec<String> = self.aliases.keys().cloned().collect();
        for start in &names {
            let mut stack = deps.get(start).cloned().unwrap_or_default();
            let mut seen: HashSet<String> = HashSet::new();
            while let Some(n) = stack.pop() {
                if n == *start {
                    self.cyclic_aliases.insert(start.clone());
                    break;
                }
                if seen.insert(n.clone()) {
                    if let Some(next) = deps.get(&n) {
                        stack.extend(next.iter().cloned());
                    }
                }
            }
        }
        // Report each cyclic alias once, at its declaration site.
        if !self.cyclic_aliases.is_empty() {
            for item in &module.items {
                if let Item::TypeAlias(a) = item {
                    if self.cyclic_aliases.contains(&a.name) {
                        self.error(
                            ErrorCode::ZAP4090,
                            format!("type alias '{}' is defined in terms of itself", a.name),
                            a.span,
                        );
                    }
                }
            }
        }
    }

    /// Register the compiler-known **capability interfaces** (D-0139/#806):
    /// `Equatable`, `Hashable`, and `Comparable`, the adjectival vocabulary a
    /// built-in collection's element requirement is a bound on. `Hashable` and
    /// `Comparable` each **refine** `Equatable`: the `Hashable` contract is
    /// defined relative to equality (`a == b ⟹ hash(a) == hash(b)`), and a
    /// `Comparable` type is `Equatable` for free (`__cmp__ == 0` yields equality).
    /// The self-referential dunders leave their `other` parameter untyped so a
    /// conforming type's own concrete `other` matches (see [`sigs_match`]). These
    /// are *definitions only* here; wiring them into the container element guards
    /// is a later epic phase (#807).
    fn register_builtin_capabilities(&mut self) {
        let iface = |name: &str,
                     method: Option<(&str, FnSig)>,
                     supertypes: &[&str]|
         -> (String, InterfaceInfo) {
            (
                name.to_string(),
                InterfaceInfo {
                    methods: method
                        .into_iter()
                        .map(|(n, s)| (n.to_string(), s))
                        .collect(),
                    supertypes: supertypes.iter().map(|s| s.to_string()).collect(),
                },
            )
        };
        let eq_sig = FnSig {
            params: vec![param(Ty::Named("Equatable".into())), param(Ty::Unknown)],
            ret: Ty::Bool,
        };
        let hash_sig = FnSig {
            params: vec![param(Ty::Named("Hashable".into()))],
            ret: Ty::Int64,
        };
        let cmp_sig = FnSig {
            params: vec![param(Ty::Named("Comparable".into())), param(Ty::Unknown)],
            ret: Ty::Int64,
        };
        for (name, info) in [
            iface("Equatable", Some(("__eq__", eq_sig)), &[]),
            iface("Hashable", Some(("__hash__", hash_sig)), &["Equatable"]),
            iface("Comparable", Some(("__cmp__", cmp_sig)), &["Equatable"]),
        ] {
            self.user_types.insert(name.clone());
            self.interfaces.insert(name, info);
        }
    }

    /// Build an interface's [`InterfaceInfo`] (D-0016): resolve each declared
    /// method signature, in declaration order. `self` is typed as the interface
    /// itself; the remaining parameter and return types are resolved normally.
    fn interface_info_of(&self, i: &InterfaceDef) -> InterfaceInfo {
        let methods = i
            .methods
            .iter()
            .map(|m| (m.name.clone(), self.sig_of_method_sig(m, &i.name)))
            .collect();
        // Refined supertypes (D-0139/#806): record each named supertype. Only a
        // plain `Named` type can name an interface; anything else is validated and
        // reported in `check_interface_refinements`, so here it is recorded by the
        // name it writes (or ignored when it has none).
        let supertypes = i
            .supertypes
            .iter()
            .filter_map(|t| match t {
                Type::Named { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect();
        InterfaceInfo {
            methods,
            supertypes,
        }
    }

    /// Resolve an interface [`MethodSig`] to a [`FnSig`] (its `self` typed as the
    /// enclosing interface `iname`).
    fn sig_of_method_sig(&self, m: &MethodSig, iname: &str) -> FnSig {
        let params = m
            .params
            .iter()
            .map(|p| {
                let ty = if p.name == "self" {
                    Ty::Named(iname.to_string())
                } else {
                    p.ty.as_ref()
                        .map(|t| self.resolve_type_self(t, iname))
                        .unwrap_or(Ty::Unknown)
                };
                ParamInfo {
                    name: p.name.clone(),
                    ty,
                    has_default: false,
                    variadic: p.variadic,
                    kwargs: p.kwargs,
                    keyword_only: p.keyword_only,
                    positional_only: p.positional_only,
                }
            })
            .collect();
        let ret = m
            .ret
            .as_ref()
            .map(|t| self.resolve_type_self(t, iname))
            .unwrap_or(Ty::Unit);
        FnSig { params, ret }
    }

    /// Is `name` a declared structural interface (D-0016)?
    fn is_interface(&self, name: &str) -> bool {
        self.interfaces.contains_key(name)
    }

    /// Does a value of type `from` satisfy where `to` is expected, allowing
    /// structural interface conformance (D-0016) in addition to ordinary
    /// [`assignable`] widening? A concrete struct conforms to an interface iff it
    /// provides every interface method with a matching signature.
    fn coercible(&self, from: &Ty, to: &Ty) -> bool {
        if assignable(from, to) {
            return true;
        }
        match (from, to) {
            // A struct OR enum value used where an interface is expected:
            // structural conformance (the concrete -> interface fat-pointer
            // conversion, D-0016). A `value struct` conforms too — on conversion
            // it is boxed (copied to the heap) so the fat pointer's `data` word
            // holds a stable, GC-traced pointer (I#75 Part 1, D-0051). An enum
            // conforms likewise (#731): a tier-3 enum boxes its tagged-struct
            // pointer, a tier-1 enum boxes its scalar tag into a heap cell. All
            // boxing is emitted in lowering.
            (Ty::Named(s), Ty::Named(i)) if self.is_interface(i) => self.conforms(s, i).is_ok(),
            // Element-wise interface coercion at collection VALUE positions
            // (#831, a D-0016 clarification): admit a concrete conformer where an
            // interface element is expected — a `list`/`array` element or a
            // `dict` **value** — exactly where `assignable` already permits
            // collection covariance (the scalar `list[int32] -> list[int64]`
            // rule), lifting the same `struct/enum -> interface` conversion one
            // level in. This is not an interface special case: the recursion
            // routes through `coercible`, so an element widens by any rule
            // `assignable`/`coercible` already allows (and nests —
            // `list[list[Shape]]`). The matching IR boxing is in place (#829/#832:
            // `box_elem_for_store`), so an admitted literal/binding lowers
            // end-to-end. KEYS STAY EXACT: a `dict` key (and a `set` member,
            // which has no arm here) is a hash position and an interface value is
            // unhashable, so the key uses `assignable` — a conformer key is still
            // rejected. Channels are invariant (`assignable` has no channel arm),
            // so they are untouched too.
            (Ty::List(a), Ty::List(b)) => self.coercible(a, b),
            (Ty::Dict(ak, av), Ty::Dict(bk, bv)) => assignable(ak, bk) && self.coercible(av, bv),
            _ => false,
        }
    }

    /// A parenthetical explaining *why* a struct→interface coercion failed, for
    /// appending to a type-mismatch diagnostic. Empty unless `to` is an interface
    /// and `from` a struct that does not conform; otherwise it lists the concrete
    /// reasons, which name the refined supertype when the missing method comes
    /// from one (D-0139/#806), e.g. "missing method '__eq__' (required by
    /// supertype 'Equatable')".
    fn conformance_detail(&self, from: &Ty, to: &Ty) -> String {
        match (from, to) {
            (Ty::Named(s), Ty::Named(i)) if self.is_interface(i) => {
                if let Err(reasons) = self.conforms(s, i) {
                    return format!(" ({s} does not satisfy '{i}': {})", reasons.join("; "));
                }
                String::new()
            }
            // Descend collection VALUE positions (#831) so a rejected
            // `list[Bad] -> list[Shape]` / `dict[K, Bad] -> dict[K, Shape]`
            // still explains *why* the element does not conform. Keys are not a
            // conformance position, so they are not descended.
            (Ty::List(a), Ty::List(b)) => self.conformance_detail(a, b),
            (Ty::Dict(_, a), Ty::Dict(_, b)) => self.conformance_detail(a, b),
            _ => String::new(),
        }
    }

    /// Does a collection literal's inferred type `vt` carry `Ty::Unknown` at a
    /// VALUE position where the declared type `dt` expects an interface (#831)?
    ///
    /// `infer_display_elems` / `infer_dict_items` `join` a literal of *distinct*
    /// element types to `Ty::Unknown` (e.g. `[Tri(), Quad()]` → `list[Unknown]`,
    /// `[Bad(), Other()]` → `list[Unknown]`). `coercible` then lets `Unknown`
    /// flow into any element, so BOTH a heterogeneous literal of genuine
    /// conformers and one holding a non-conformer slip the top-level check even
    /// though only the former should be admitted — and a slipped non-conformer
    /// later crashes lowering (`'Bad__sides' not implemented`). This predicate
    /// (pure; no re-inference) flags that slip so the caller re-verifies the
    /// literal's element *expressions*. It descends collection value positions
    /// (nested `list[list[Shape]]`), matching `coercible`; keys are never a
    /// conformance position and are not descended.
    fn collection_literal_needs_elem_recheck(&self, vt: &Ty, dt: &Ty) -> bool {
        match (vt, dt) {
            (Ty::List(ve), Ty::List(de)) => {
                (matches!(**ve, Ty::Unknown) && self.ty_mentions_interface(de))
                    || self.collection_literal_needs_elem_recheck(ve, de)
            }
            (Ty::Dict(_, vv), Ty::Dict(_, dv)) => {
                (matches!(**vv, Ty::Unknown) && self.ty_mentions_interface(dv))
                    || self.collection_literal_needs_elem_recheck(vv, dv)
            }
            _ => false,
        }
    }

    /// Does `t`, or a VALUE position nested inside it, name an interface (#831)?
    /// Used to decide whether an `Unknown`-widened literal element could hide a
    /// (non-)conformer that must be re-verified against a declared interface.
    fn ty_mentions_interface(&self, t: &Ty) -> bool {
        match t {
            Ty::Named(n) => self.is_interface(n),
            Ty::List(e) => self.ty_mentions_interface(e),
            Ty::Dict(_, v) => self.ty_mentions_interface(v),
            _ => false,
        }
    }

    /// Re-verify each VALUE-position element *expression* of a collection
    /// display literal against the declared element type `dt` (#831), used when
    /// [`collection_literal_needs_elem_recheck`] reports that the literal's
    /// inferred element widened to `Ty::Unknown` against a declared interface.
    /// A non-conforming element is a clean `ZAP4001` at its own span (so
    /// `[Bad(), Other()]` is rejected here instead of crashing lowering), while a
    /// heterogeneous literal of genuine conformers (`[Tri(), Quad()]`) passes by
    /// real conformance. Nested displays descend; `dict` keys and `set` members
    /// are hash positions, never descended (an interface value is unhashable).
    fn verify_collection_literal_conformance(&mut self, value: &Expr, dt: &Ty) {
        match (value, dt) {
            (Expr::List(elems, _), Ty::List(de)) => {
                for el in elems {
                    // A `*expr` splat contributes its already-checked element
                    // type via iteration, not a value expression to re-check.
                    if let Expr::Starred { .. } = el {
                        continue;
                    }
                    self.verify_elem_expr_conformance(el, de);
                }
            }
            (Expr::Dict(items, _), Ty::Dict(_, dv)) => {
                for it in items {
                    if let DictItem::Pair(_, v) = it {
                        self.verify_elem_expr_conformance(v, dv);
                    }
                }
            }
            _ => {}
        }
    }

    /// Verify one VALUE-position element expression `el` against the declared
    /// element type `elem_ty` (#831). A nested display descends; a leaf whose
    /// declared type is an interface must structurally conform (else `ZAP4001`);
    /// any other declared type is left to the ordinary checker.
    fn verify_elem_expr_conformance(&mut self, el: &Expr, elem_ty: &Ty) {
        match (el, elem_ty) {
            (Expr::List(_, _), Ty::List(_)) | (Expr::Dict(_, _), Ty::Dict(_, _)) => {
                self.verify_collection_literal_conformance(el, elem_ty);
            }
            (_, Ty::Named(iname)) if self.is_interface(iname) => {
                let et = self.infer(el);
                if !self.coercible(&et, elem_ty) {
                    self.error(
                        ErrorCode::ZAP4001,
                        format!(
                            "collection element of type {} does not conform to interface '{}'{}",
                            et.display(),
                            iname,
                            self.conformance_detail(&et, elem_ty),
                        ),
                        expr_span(el),
                    );
                }
            }
            _ => {}
        }
    }

    /// Check that collection *display literal* `value` (inferred type `vt`),
    /// assigned where `dt` is expected, does not slip a non-conformer through
    /// the `Ty::Unknown`-widening of a heterogeneous literal (#831). A no-op
    /// unless the slip applies; see [`collection_literal_needs_elem_recheck`].
    fn check_collection_literal_elems(&mut self, value: &Expr, vt: &Ty, dt: &Ty) {
        if self.collection_literal_needs_elem_recheck(vt, dt) {
            self.verify_collection_literal_conformance(value, dt);
        }
    }

    /// Check that struct `sname` structurally conforms to interface `iname`
    /// (D-0016): every interface method is present with a matching signature
    /// (parameter types excluding `self`, and return type). On failure, returns
    /// the list of human-readable reasons (missing / mismatched methods) for a
    /// precise diagnostic. A non-struct `sname` (e.g. not yet registered) simply
    /// fails to conform.
    fn conforms(&self, sname: &str, iname: &str) -> Result<(), Vec<String>> {
        if !self.interfaces.contains_key(iname) {
            return Err(vec![format!("'{iname}' is not an interface")]);
        }
        // A struct OR an enum conforms by structural method set (D-0016 extended
        // to enums, #731): both enum tiers are eligible — a tier-3 (data-carrying)
        // enum lowers to a tagged struct and boxes that pointer into the interface
        // fat pointer; a tier-1 (unboxed-integer) enum boxes its scalar tag into a
        // small heap cell (see `crates/ir`). Either way conformance here is "has
        // every required method with a matching signature", read off the type's
        // registered method table.
        let methods = if let Some(st) = self.structs.get(sname) {
            &st.methods
        } else if let Some(en) = self.enums.get(sname) {
            &en.methods
        } else {
            return Err(vec![format!("'{sname}' is not a struct or enum")]);
        };
        // Refinement (D-0139/#806): conformance to `iname` requires every method
        // of `iname` *and* of all its transitive supertypes. A method missing
        // because of a supertype names that supertype in the diagnostic, so
        // "satisfies `Hashable` but not `Equatable`" points at `Equatable`.
        let mut reasons = Vec::new();
        for (mname, isig, origin) in self.required_methods(iname) {
            let from = if origin == iname {
                String::new()
            } else {
                format!(" (required by supertype '{origin}')")
            };
            match methods.get(&mname) {
                None => reasons.push(format!("missing method '{mname}'{from}")),
                Some(msig) => {
                    if !sigs_match(msig, &isig) {
                        reasons.push(format!("method '{mname}' has a mismatched signature{from}"));
                    }
                }
            }
        }
        if reasons.is_empty() {
            Ok(())
        } else {
            Err(reasons)
        }
    }

    /// Every method required to conform to interface `iname`, flattened over
    /// `iname`'s own declarations and all its transitive refined supertypes
    /// (D-0139/#806), each paired with the interface that *introduced* the
    /// requirement (for diagnostics). Cycle-safe via a visited set, so a
    /// (malformed) cyclic refinement cannot loop here — the cycle is reported
    /// separately at the definition site (ZAP4082).
    fn required_methods(&self, iname: &str) -> Vec<(String, FnSig, String)> {
        let mut out = Vec::new();
        let mut visited = HashSet::new();
        let mut stack = vec![iname.to_string()];
        while let Some(cur) = stack.pop() {
            if !visited.insert(cur.clone()) {
                continue;
            }
            if let Some(iface) = self.interfaces.get(&cur) {
                for (mname, sig) in &iface.methods {
                    out.push((mname.clone(), sig.clone(), cur.clone()));
                }
                for sup in &iface.supertypes {
                    stack.push(sup.clone());
                }
            }
        }
        out
    }

    /// Resolve method `mname` on interface `iname`, searching `iname` and then its
    /// transitive refined supertypes (D-0139/#806). This is what makes a supertype
    /// method callable on a value of a bounded type parameter `[T: iname]`.
    /// Cycle-safe via a visited set.
    fn iface_method(&self, iname: &str, mname: &str) -> Option<FnSig> {
        let mut visited = HashSet::new();
        let mut stack = vec![iname.to_string()];
        while let Some(cur) = stack.pop() {
            if !visited.insert(cur.clone()) {
                continue;
            }
            let Some(iface) = self.interfaces.get(&cur) else {
                continue;
            };
            if let Some(sig) = iface.method(mname) {
                return Some(sig.clone());
            }
            for sup in &iface.supertypes {
                stack.push(sup.clone());
            }
        }
        None
    }

    /// Build a struct's [`StructInfo`] (operator overloading, D-0047): resolve
    /// fields and method signatures, collect `@derive`d traits, and synthesize
    /// the signatures of the dunders those derives will generate.
    fn struct_info_of(&self, s: &StructDef) -> StructInfo {
        let mut methods: HashMap<String, FnSig> = HashMap::new();
        for m in &s.methods {
            methods.insert(m.name.clone(), self.sig_of_method(m, &s.name));
        }
        let mut derived: HashSet<String> = HashSet::new();
        for d in &s.decorators {
            if d.name == "derive" {
                for a in &d.args {
                    if let Expr::Name(t, _) = &a.value {
                        derived.insert(t.clone());
                    }
                }
            }
        }
        // Dunders `@derive(...)` generates (D-0047) — registered so an operator
        // on a derived type resolves and is typed, unless the user hand-wrote the
        // method (then their signature wins).
        let named = Ty::Named(s.name.clone());
        let unary = |ret: Ty| FnSig {
            params: vec![param(named.clone())],
            ret,
        };
        let binary = |ret: Ty| FnSig {
            params: vec![param(named.clone()), param(named.clone())],
            ret,
        };
        if derived.contains("Eq") {
            methods
                .entry("__eq__".into())
                .or_insert_with(|| binary(Ty::Bool));
        }
        if derived.contains("Ord") {
            methods
                .entry("__cmp__".into())
                .or_insert_with(|| binary(Ty::Int64));
        }
        if derived.contains("Hash") {
            methods
                .entry("__hash__".into())
                .or_insert_with(|| unary(Ty::Int64));
        }
        if derived.contains("Repr") {
            methods
                .entry("__repr__".into())
                .or_insert_with(|| unary(Ty::Str));
        }
        if derived.contains("Json") {
            // Compile-time JSON codec (D-0059, I#100): `self.to_json() -> str`.
            // The decode direction, `Struct.from_json(s)`, is an associated
            // function resolved in `check_call` (keyed on `to_json` existing).
            methods
                .entry("to_json".into())
                .or_insert_with(|| unary(Ty::Str));
        }
        if derived.contains("Csv") {
            // Compile-time CSV record codec (D-0059 §4b, I#132):
            // `self.to_csv() -> str`; `Struct.from_csv(s)` is the associated
            // decode function (resolved in `check_call`, keyed on `to_csv`).
            methods
                .entry("to_csv".into())
                .or_insert_with(|| unary(Ty::Str));
        }
        if derived.contains("Yaml") {
            // Compile-time YAML codec (D-0063, I#149): `self.to_yaml() -> str`;
            // `Struct.from_yaml(s)` is the associated decode function (resolved
            // in `check_call`, keyed on `to_yaml`).
            methods
                .entry("to_yaml".into())
                .or_insert_with(|| unary(Ty::Str));
        }
        let fields = s.fields.iter().map(|f| self.resolve_type(&f.ty)).collect();
        StructInfo {
            methods,
            is_value: s.is_value,
            fields,
        }
    }

    /// Is `name` a `value struct` (value type, D-0051)?
    fn is_value_struct(&self, name: &str) -> bool {
        self.structs.get(name).is_some_and(|s| s.is_value)
    }

    /// A `value struct` is laid out inline (D-0051), so it cannot contain itself
    /// by value — directly or through a chain of value-struct fields — as that
    /// would be an infinitely large aggregate. (A plain/reference `struct` field,
    /// a `list`/`dict`/`set`, or any other pointer-backed type breaks the chain,
    /// so recursive *reference* types remain fine, D-0023.) Report a clear error
    /// on each value struct that lies on such a cycle.
    fn check_value_struct_recursion(&mut self, module: &Module) {
        // Direct value-struct → value-struct field edges (by type name), with the
        // field span for diagnostics.
        let mut edges: HashMap<String, Vec<(String, Span)>> = HashMap::new();
        for item in &module.items {
            if let Item::Struct(s) = item {
                if !s.is_value {
                    continue;
                }
                let mut es = Vec::new();
                for f in &s.fields {
                    if let Type::Named { name, .. } = &f.ty {
                        if self.is_value_struct(name) {
                            es.push((name.clone(), f.span));
                        }
                    }
                }
                edges.insert(s.name.clone(), es);
            }
        }
        // DFS from each value struct; a back-edge to a node on the current stack
        // is a cycle. Report once per struct that participates in one.
        let mut reported: HashSet<String> = HashSet::new();
        let roots: Vec<String> = edges.keys().cloned().collect();
        for root in roots {
            let mut stack: Vec<String> = Vec::new();
            let mut on_stack: HashSet<String> = HashSet::new();
            self.detect_value_cycle(&root, &edges, &mut stack, &mut on_stack, &mut reported);
        }
    }

    fn detect_value_cycle(
        &mut self,
        node: &str,
        edges: &HashMap<String, Vec<(String, Span)>>,
        stack: &mut Vec<String>,
        on_stack: &mut HashSet<String>,
        reported: &mut HashSet<String>,
    ) {
        if on_stack.contains(node) {
            if !reported.contains(node) {
                reported.insert(node.to_string());
                // Blame the field that re-enters the cycle, if we can find it.
                let span = stack
                    .last()
                    .and_then(|p| edges.get(p))
                    .and_then(|es| es.iter().find(|(n, _)| n == node))
                    .map(|(_, sp)| *sp)
                    .unwrap_or_default();
                self.error(
                    ErrorCode::ZAP5001,
                    format!(
                        "value struct '{node}' cannot contain itself by value \
                         (an inline value type would be infinitely large); use a \
                         plain (reference) struct for a recursive type (D-0051)"
                    ),
                    span,
                );
            }
            return;
        }
        if reported.contains(node) {
            return;
        }
        stack.push(node.to_string());
        on_stack.insert(node.to_string());
        if let Some(es) = edges.get(node) {
            for (next, _) in es.clone() {
                self.detect_value_cycle(&next, edges, stack, on_stack, reported);
            }
        }
        stack.pop();
        on_stack.remove(node);
    }

    /// Validate C-style bitfields (I#173, D-0068). A bitfield is a field with an
    /// explicit bit width (`int64 kind : 3`). The rules:
    ///
    /// * bitfields are allowed **only on a `value struct`** — never on a plain
    ///   (reference) struct or an `annotation`;
    /// * **all-or-nothing**: if any field declares a width, every field must
    ///   (v1 does not mix bitfields with full-width fields — the whole value
    ///   struct becomes a single packed scalar blob);
    /// * each bitfield's type must be a fixed-width signed integer
    ///   (`int32`/`int64`), `bool`, or a tier-1 (unboxed) `enum` — never a
    ///   pointer/reference, float, `char`, `bigint`, or tagged enum;
    /// * `1 <= width <= type_bits` (`int32`=32, `int64`=64, `bool`=1, a tier-1
    ///   enum must hold every discriminant), and the widths of one struct sum to
    ///   at most 64 bits (one machine word).
    fn check_bitfields(&mut self, module: &Module) {
        // Enum defs by name, for discriminant-range checks on enum bitfields.
        let enum_defs: HashMap<&str, &EnumDef> = module
            .items
            .iter()
            .filter_map(|it| match it {
                Item::Enum(e) => Some((e.name.as_str(), e)),
                _ => None,
            })
            .collect();
        for item in &module.items {
            match item {
                // A bitfield on an annotation field is meaningless (annotations
                // are metadata, not laid-out values).
                Item::Annotation(a) => {
                    for f in &a.fields {
                        if f.bits.is_some() {
                            self.error(
                                ErrorCode::ZAP5010,
                                "a bit width is only allowed on a `value struct` field (D-0068)"
                                    .to_string(),
                                f.span,
                            );
                        }
                    }
                }
                Item::Struct(s) => {
                    let any_bits = s.fields.iter().any(|f| f.bits.is_some());
                    if !any_bits {
                        continue;
                    }
                    if !s.is_value {
                        for f in s.fields.iter().filter(|f| f.bits.is_some()) {
                            self.error(
                                ErrorCode::ZAP5010,
                                format!(
                                    "bit widths require a `value struct`: field '{}' of the \
                                     reference struct '{}' cannot be a bitfield (a plain struct \
                                     is a reference type, D-0023/D-0068)",
                                    f.name, s.name
                                ),
                                f.span,
                            );
                        }
                        continue;
                    }
                    self.check_bitfield_struct(s, &enum_defs);
                }
                _ => {}
            }
        }
    }

    /// Validate one `value struct` that uses bitfields (every field, the widths,
    /// and the one-word total). See [`Checker::check_bitfields`].
    fn check_bitfield_struct(&mut self, s: &StructDef, enum_defs: &HashMap<&str, &EnumDef>) {
        let mut total: u64 = 0;
        for f in &s.fields {
            let Some(width) = f.bits else {
                // All-or-nothing: a width-less field in a bitfield struct.
                self.error(
                    ErrorCode::ZAP5011,
                    format!(
                        "every field of bitfield value struct '{}' must declare a bit width \
                         (v1 does not mix bitfields with full-width fields, D-0068); field '{}' \
                         has none",
                        s.name, f.name
                    ),
                    f.span,
                );
                continue;
            };
            // The field type must be a bitfield-eligible scalar.
            let Some((type_bits, _signed, min_bits)) =
                self.bitfield_field_type(&f.ty, enum_defs, f.span)
            else {
                continue;
            };
            let w = u64::from(width);
            if width == 0 {
                self.error(
                    ErrorCode::ZAP5012,
                    format!("bitfield '{}' must be at least 1 bit wide", f.name),
                    f.span,
                );
            } else if w > type_bits {
                self.error(
                    ErrorCode::ZAP5013,
                    format!(
                        "bitfield '{}' is {} bits wide but its type holds at most {} bits (D-0068)",
                        f.name, width, type_bits
                    ),
                    f.span,
                );
            } else if w < min_bits {
                self.error(
                    ErrorCode::ZAP5014,
                    format!(
                        "bitfield '{}' is {} bits wide but its enum needs at least {} bits to \
                         hold every discriminant (D-0068)",
                        f.name, width, min_bits
                    ),
                    f.span,
                );
            }
            total += w;
        }
        if total > 64 {
            self.error(
                ErrorCode::ZAP5015,
                format!(
                    "bitfield value struct '{}' packs {} bits, more than one 64-bit machine \
                     word (D-0068)",
                    s.name, total
                ),
                s.span,
            );
        }
    }

    /// Classify a bitfield field type: returns `(type_bits, signed, min_bits)`
    /// for an eligible type (`int32`/`int64`, `bool`, tier-1 `enum`), or `None`
    /// (after reporting an error) for an ineligible one. `type_bits` is the most
    /// bits the field may use; `min_bits` is the fewest it must (1 for integers
    /// and `bool`; for a tier-1 enum, the bits needed to hold every
    /// discriminant). A tier-1 enum with a **negative** discriminant cannot be an
    /// unsigned bitfield and is rejected here.
    fn bitfield_field_type(
        &mut self,
        ty: &Type,
        enum_defs: &HashMap<&str, &EnumDef>,
        span: Span,
    ) -> Option<(u64, bool, u64)> {
        if let Type::Named { name, args, .. } = ty {
            if args.is_empty() {
                match name.as_str() {
                    "int64" | "i64" => return Some((64, true, 1)),
                    "int32" | "i32" => return Some((32, true, 1)),
                    // Unsigned fixed-width integers are natural C-style
                    // bitfields (#552): unsigned storage, exact bit width.
                    "uint64" | "u64" | "usize" => return Some((64, false, 1)),
                    "uint32" | "u32" => return Some((32, false, 1)),
                    "uint16" | "u16" => return Some((16, false, 1)),
                    "uint8" | "u8" => return Some((8, false, 1)),
                    "bool" => return Some((1, false, 1)),
                    other => {
                        // A tier-1 (unboxed-integer) enum is a legal bitfield; a
                        // tagged (data-carrying) enum is a heap value and is not.
                        if let Some(info) = self.enums.get(other) {
                            if info.is_sum {
                                self.error(
                                    ErrorCode::ZAP5016,
                                    format!(
                                        "a tagged enum '{other}' is a reference value and cannot \
                                         be a bitfield (only tier-1 enums, D-0068)"
                                    ),
                                    span,
                                );
                                return None;
                            }
                            // Bits needed to hold the enum's largest discriminant.
                            match enum_defs.get(other).and_then(|e| enum_discriminant_bits(e)) {
                                Some(bits) => return Some((64, false, bits)),
                                None => {
                                    self.error(
                                        ErrorCode::ZAP5017,
                                        format!(
                                            "enum '{other}' has a negative discriminant and \
                                             cannot be an unsigned bitfield (D-0068)"
                                        ),
                                        span,
                                    );
                                    return None;
                                }
                            }
                        }
                    }
                }
            }
        }
        self.error(
            ErrorCode::ZAP5018,
            "a bitfield must be a fixed-width integer (int32/int64), bool, or a tier-1 enum — \
             not a pointer/reference, float, char, or bigint (D-0068)"
                .to_string(),
            span,
        );
        None
    }

    /// A struct method's signature, mapping the `self` receiver and any `Self`
    /// annotation to the enclosing struct type.
    fn sig_of_method(&self, f: &Function, sname: &str) -> FnSig {
        let params = f
            .params
            .iter()
            .map(|p| {
                let ty = if p.name == "self" {
                    Ty::Named(sname.to_string())
                } else {
                    p.ty.as_ref()
                        .map(|t| self.resolve_type_self(t, sname))
                        .unwrap_or(Ty::Unknown)
                };
                ParamInfo {
                    name: p.name.clone(),
                    ty,
                    has_default: p.default.is_some(),
                    variadic: p.variadic,
                    kwargs: p.kwargs,
                    keyword_only: p.keyword_only,
                    positional_only: p.positional_only,
                }
            })
            .collect();
        let ret = f
            .ret
            .as_ref()
            .map(|t| self.resolve_type_self(t, sname))
            .unwrap_or(Ty::Unit);
        FnSig { params, ret }
    }

    /// Like [`resolve_type`], but resolves the `Self` type name to the enclosing
    /// struct (used in dunder signatures such as `__add__(self, other: Self)`).
    fn resolve_type_self(&self, t: &Type, sname: &str) -> Ty {
        if let Type::Named { name, .. } = t {
            if name == "Self" {
                return Ty::Named(sname.to_string());
            }
        }
        self.resolve_type(t)
    }

    fn collect_sigs(&mut self, module: &Module) {
        for item in &module.items {
            match item {
                Item::Function(f) => {
                    let sig = self.sig_of(f);
                    self.sigs.insert(f.name.clone(), sig);
                    // `@deprecated` on a free function (I#472, D-0109): record
                    // the optional message so every call site warns (ZAP5124).
                    if let Some(msg) = deprecated_message(&f.decorators) {
                        self.deprecated.insert(f.name.clone(), msg);
                    }
                }
                Item::Struct(s) => {
                    // `@deprecated` on a method, keyed `"Type.method"`.
                    for m in &s.methods {
                        if let Some(msg) = deprecated_message(&m.decorators) {
                            self.deprecated
                                .insert(format!("{}.{}", s.name, m.name), msg);
                        }
                    }
                }
                _ => {}
            }
        }
        self.register_generators(module);
    }

    /// Register the compiler-synthesized iterator type for every generator
    /// (I#67, D-0026/D-0048), so a call `gen(...)` — or a generator method call
    /// `obj.gen(...)` (I#429) — types as an `Iterator` and drops into
    /// `for`/comprehensions through the #53 protocol.
    ///
    /// The synthetic type `$gen:<name>` is given the two structural `Iterator`
    /// dunders — `__iter__(self) -> Self` and comma-ok `__next__(self) ->
    /// (T, bool)` — and the generator's return type is rewritten to it. For a
    /// method the synthetic name is keyed on the mangled `Struct__method` so it
    /// cannot collide across types; `self` is simply a captured parameter of the
    /// generator (handled by lowering), not part of this consuming-side typing.
    /// The element type `T` is left permissive (`Unknown`) here: it is not
    /// needed to type-check the consuming side (any numeric op against it stays
    /// permissive), and the precise element type is computed by lowering from
    /// the `yield` operand types for codegen (`crates/ir`).
    fn register_generators(&mut self, module: &Module) {
        // The shared iterator type for bare generator expressions `(x for x in
        // xs)` (#596, epic #614 D4). A genexpr is lazy — like a generator call
        // its value satisfies the `Iterator` protocol (D-0048) — and, as with
        // generators, the element type is left permissive (`Unknown`) on the
        // consuming side; lowering computes the precise element type for
        // codegen. One shared type suffices because the element type is not
        // part of the consuming-side typing.
        self.register_generator_iter_type(GENEXPR_ITER_TYPE);
        for item in &module.items {
            match item {
                Item::Function(f) => {
                    if !genanalysis::is_generator(f) {
                        continue;
                    }
                    // A free `main` is rejected in `check_function`; do not give
                    // it an iterator type.
                    if f.name == "main" {
                        continue;
                    }
                    let gen_name = genanalysis::gen_state_name(&f.name);
                    self.register_generator_iter_type(&gen_name);
                    if let Some(sig) = self.sigs.get_mut(&f.name) {
                        sig.ret = Ty::Named(gen_name);
                    }
                }
                // Struct method generators (I#429): a `self`-bearing method whose
                // body `yield`s is a generator too (this is the paginator shape
                // the generated AWS clients emit). Key its synthetic type on the
                // mangled `Struct__method` so it cannot collide across types, and
                // rewrite the method entry in the struct's method table so a call
                // `obj.gen(...)` types as the iterator.
                Item::Struct(StructDef { name, methods, .. }) => {
                    for m in methods {
                        if !genanalysis::is_generator(m) {
                            continue;
                        }
                        let gen_name = genanalysis::gen_state_name(&format!("{name}__{}", m.name));
                        self.register_generator_iter_type(&gen_name);
                        if let Some(info) = self.structs.get_mut(name) {
                            if let Some(sig) = info.methods.get_mut(&m.name) {
                                sig.ret = Ty::Named(gen_name);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Insert the synthetic iterator `StructInfo` for a generator's state type
    /// `gen_name` (`$gen:…`): the two structural `Iterator` dunders over `self`.
    fn register_generator_iter_type(&mut self, gen_name: &str) {
        let self_param = |ty: Ty| ParamInfo {
            name: "self".to_string(),
            ty,
            has_default: false,
            variadic: false,
            kwargs: false,
            keyword_only: false,
            positional_only: false,
        };
        let mut methods = HashMap::new();
        methods.insert(
            "__iter__".to_string(),
            FnSig {
                params: vec![self_param(Ty::Named(gen_name.to_string()))],
                ret: Ty::Named(gen_name.to_string()),
            },
        );
        methods.insert(
            "__next__".to_string(),
            FnSig {
                params: vec![self_param(Ty::Named(gen_name.to_string()))],
                ret: Ty::Tuple(vec![Ty::Unknown, Ty::Bool]),
            },
        );
        self.structs.insert(
            gen_name.to_string(),
            StructInfo {
                methods,
                is_value: false,
                fields: Vec::new(),
            },
        );
    }

    fn sig_of(&self, f: &Function) -> FnSig {
        let params = f
            .params
            .iter()
            .map(|p| ParamInfo {
                name: p.name.clone(),
                ty: p
                    .ty
                    .as_ref()
                    .map(|t| self.resolve_type(t))
                    .unwrap_or(Ty::Unknown),
                has_default: p.default.is_some(),
                variadic: p.variadic,
                kwargs: p.kwargs,
                keyword_only: p.keyword_only,
                positional_only: p.positional_only,
            })
            .collect();
        let ret = f
            .ret
            .as_ref()
            .map(|t| self.resolve_type(t))
            .unwrap_or(Ty::Unit);
        FnSig { params, ret }
    }

    fn resolve_type(&self, t: &Type) -> Ty {
        match t {
            Type::Named { name, args, .. } => match name.as_str() {
                // `int` is the Python-facing name for the 64-bit signed integer
                // (I#581); `float` is likewise the name for the 64-bit double.
                "int" | "int64" | "i64" => Ty::Int64,
                "int32" | "i32" => Ty::Int32,
                // Unsigned fixed-width integers + pointer-width `usize` (#552).
                "uint8" | "u8" => Ty::Uint8,
                "uint16" | "u16" => Ty::Uint16,
                "uint32" | "u32" => Ty::Uint32,
                "uint64" | "u64" => Ty::Uint64,
                "usize" => Ty::Usize,
                "float" | "float64" | "f64" => Ty::Float64,
                "float32" | "f32" => Ty::Float32,
                "bool" => Ty::Bool,
                "str" => Ty::Str,
                "bytes" => Ty::Bytes,
                "char" => Ty::Char,
                "bigint" => Ty::Bigint,
                "complex" => Ty::Complex,
                "Match" => Ty::Match,
                "pyobject" => Ty::Py,
                "list" => {
                    let el = args
                        .first()
                        .map(|a| self.resolve_type(a))
                        .unwrap_or(Ty::Unknown);
                    Ty::List(Box::new(el))
                }
                "array" => {
                    // Numeric arrays (ticket #24) are typed loosely for now:
                    // element/dtype typing stays `Unknown` so array programs
                    // type-check without false errors (modeling them as lists
                    // would wrongly reject `array * array`). The ZIR lowering
                    // carries the precise dtype.
                    for a in args {
                        let _ = self.resolve_type(a);
                    }
                    Ty::Unknown
                }
                "dict" => {
                    let k = args
                        .first()
                        .map(|a| self.resolve_type(a))
                        .unwrap_or(Ty::Unknown);
                    let v = args
                        .get(1)
                        .map(|a| self.resolve_type(a))
                        .unwrap_or(Ty::Unknown);
                    Ty::Dict(Box::new(k), Box::new(v))
                }
                "set" => {
                    let el = args
                        .first()
                        .map(|a| self.resolve_type(a))
                        .unwrap_or(Ty::Unknown);
                    Ty::Set(Box::new(el))
                }
                "heap" => {
                    let el = args
                        .first()
                        .map(|a| self.resolve_type(a))
                        .unwrap_or(Ty::Unknown);
                    Ty::Heap(Box::new(el))
                }
                "channel" => {
                    let el = args
                        .first()
                        .map(|a| self.resolve_type(a))
                        .unwrap_or(Ty::Unknown);
                    Ty::Chan(Box::new(el))
                }
                "AtomicArray" => {
                    let el = args
                        .first()
                        .map(|a| self.resolve_type(a))
                        .unwrap_or(Ty::Unknown);
                    Ty::AtomicArray(Box::new(el))
                }
                other => {
                    // A transparent type alias (#606) resolves to its target
                    // with the generic parameters substituted, recursively, so
                    // the alias and its target share one identity. A cyclic alias
                    // yields `Unknown` (the cycle is reported in `collect_types`).
                    if let Some((params, target)) = self.aliases.get(other) {
                        if self.cyclic_aliases.contains(other) {
                            return Ty::Unknown;
                        }
                        let mut subst: HashMap<&str, &Type> = HashMap::new();
                        for (p, a) in params.iter().zip(args.iter()) {
                            subst.insert(p.as_str(), a);
                        }
                        let expanded = subst_type(target, &subst);
                        return self.resolve_type(&expanded);
                    }
                    if self.user_types.contains(other) {
                        Ty::Named(other.to_string())
                    } else {
                        Ty::Unknown
                    }
                }
            },
            Type::Tuple { elems, .. } => {
                Ty::Tuple(elems.iter().map(|e| self.resolve_type(e)).collect())
            }
            Type::Func { .. } => Ty::Unknown,
        }
    }

    /// The channel element type named by a `channel[T]` subscript, where `T`
    /// appears as a bare name in expression position (ticket #25, D-0031).
    fn chan_elem_from_expr(&self, e: &Expr) -> Ty {
        match e {
            Expr::Name(n, _) => match n.as_str() {
                "int" | "int64" | "i64" => Ty::Int64,
                "int32" | "i32" => Ty::Int32,
                "float" | "float64" | "f64" => Ty::Float64,
                "float32" | "f32" => Ty::Float32,
                "bool" => Ty::Bool,
                "str" => Ty::Str,
                "bytes" => Ty::Bytes,
                "char" => Ty::Char,
                "bigint" => Ty::Bigint,
                "complex" => Ty::Complex,
                other if self.user_types.contains(other) => Ty::Named(other.to_string()),
                _ => Ty::Unknown,
            },
            _ => Ty::Unknown,
        }
    }

    // --- decorator / annotation validation (D-0036) ---
    //
    // Every `@name` application is validated here: the name must be a built-in
    // decorator, a declared `annotation`, or a declared `decorator`; an unknown
    // name is a compile error (compile-time typo-catching, D-0016). The
    // built-ins additionally get argument-shape and target-suitability checks.
    // The *behavior* of the built-ins (code generation, reflection metadata) is
    // produced later, by the AST->ZIR lowering pass.

    fn validate_decorators(&mut self, module: &Module) {
        for item in &module.items {
            match item {
                Item::Struct(s) => {
                    for d in &s.decorators {
                        self.check_decorator(d, DecoTarget::Struct, None);
                    }
                    for f in &s.fields {
                        for d in &f.decorators {
                            self.check_decorator(d, DecoTarget::Field, Some(f));
                        }
                    }
                    for m in &s.methods {
                        self.check_fn_decorators(m);
                    }
                }
                Item::Function(f) => self.check_fn_decorators(f),
                Item::Enum(e) => {
                    for d in &e.decorators {
                        self.check_decorator(d, DecoTarget::Enum, None);
                    }
                }
                _ => {}
            }
        }
    }

    fn check_fn_decorators(&mut self, f: &Function) {
        for d in &f.decorators {
            self.check_decorator(d, DecoTarget::Function, None);
        }
        for p in &f.params {
            for d in &p.decorators {
                self.check_decorator(d, DecoTarget::Param, None);
            }
        }
    }

    /// Validate one decorator application. `field` is the decorated field when
    /// `target == Field`, so the field-level built-ins (`@default`/`@validate`/
    /// `@range`, D-0101) can check the field's declared type and literal default.
    fn check_decorator(&mut self, d: &Decorator, target: DecoTarget, field: Option<&Field>) {
        if BUILTIN_DECORATORS.contains(&d.name.as_str()) {
            self.check_builtin_decorator(d, target, field);
            return;
        }
        if let Some(fields) = self.annotations.get(&d.name).cloned() {
            // An annotation application: every keyword arg must name a field
            // (typo-catching). Positional args are matched by declaration order.
            for a in &d.args {
                if let Some(argname) = &a.name {
                    if !fields.contains(argname) {
                        self.error(
                            ErrorCode::ZAP5030,
                            format!("annotation '@{}' has no field '{argname}'", d.name),
                            d.span,
                        );
                    }
                }
            }
            if d.args.len() > fields.len() {
                self.error(
                    ErrorCode::ZAP5031,
                    format!(
                        "annotation '@{}' takes at most {} argument(s), found {}",
                        d.name,
                        fields.len(),
                        d.args.len()
                    ),
                    d.span,
                );
            }
            return;
        }
        if self.user_decorators.contains(&d.name) {
            // User-defined behavioral decorator — accepted; its arguments are
            // checked loosely for now (the user-macro API is deferred,
            // OPEN-QUESTIONS.md).
            return;
        }
        self.error(
            ErrorCode::ZAP5032,
            format!(
                "unknown decorator or annotation '@{}' (not a built-in, \
                 `annotation`, or `decorator`)",
                d.name
            ),
            d.span,
        );
    }

    fn check_builtin_decorator(
        &mut self,
        d: &Decorator,
        target: DecoTarget,
        field: Option<&Field>,
    ) {
        let keywords = d.args.iter().filter(|a| a.name.is_some()).count();
        let positionals = d.args.len() - keywords;
        match d.name.as_str() {
            "derive" => {
                // `@derive(...)` applies to a struct, and additionally to an enum
                // when every derived trait is `Json` — an open string-enum codec
                // (ticket #346, D-0086). Other traits on an enum stay unsupported.
                if target == DecoTarget::Enum {
                    let all_json = d
                        .args
                        .iter()
                        .all(|a| matches!(&a.value, Expr::Name(t, _) if t == "Json"));
                    if !all_json {
                        self.error(
                            ErrorCode::ZAP5047,
                            "only @derive(Json) is supported on an enum (ticket #346); \
                             other derivable traits apply to a struct",
                            d.span,
                        );
                    }
                } else {
                    self.require_target(d, target, DecoTarget::Struct);
                }
                if d.args.is_empty() {
                    self.error(
                        ErrorCode::ZAP5033,
                        "@derive(...) needs at least one trait, e.g. @derive(Eq, Repr)",
                        d.span,
                    );
                }
                for a in &d.args {
                    if a.name.is_some() {
                        self.error(
                            ErrorCode::ZAP5034,
                            "@derive takes positional trait names, not keywords",
                            d.span,
                        );
                    } else if let Expr::Name(t, _) = &a.value {
                        if !DERIVABLE_TRAITS.contains(&t.as_str()) {
                            self.error(
                                ErrorCode::ZAP5035,
                                format!(
                                    "unknown derivable trait '{t}' (expected one of \
                                     Eq, Ord, Json, Repr, Hash)"
                                ),
                                d.span,
                            );
                        }
                    } else {
                        self.error(
                            ErrorCode::ZAP5036,
                            "@derive expects trait names, e.g. @derive(Eq)",
                            d.span,
                        );
                    }
                }
            }
            "json" => {
                // JSON codec hooks (ticket #346, D-0086). On a field:
                // `@json(timestamp="epoch"|"iso8601")` selects a DateTime field's
                // wire form. On an enum: `@json(open)` marks an open string enum.
                match target {
                    DecoTarget::Field => {
                        if positionals != 0 {
                            self.error(
                                ErrorCode::ZAP5047,
                                "@json(...) on a field takes keyword options, e.g. \
                                 @json(timestamp=\"iso8601\")",
                                d.span,
                            );
                        }
                        for a in &d.args {
                            match a.name.as_deref() {
                                Some("timestamp") => {
                                    let ok = matches!(
                                        &a.value,
                                        Expr::Str { value, .. }
                                            if value == "epoch" || value == "iso8601"
                                    );
                                    if !ok {
                                        self.error(
                                            ErrorCode::ZAP5047,
                                            "@json(timestamp=...) must be \"epoch\" or \"iso8601\"",
                                            d.span,
                                        );
                                    }
                                }
                                Some(other) => self.error(
                                    ErrorCode::ZAP5047,
                                    format!(
                                        "@json(...) on a field has no option '{other}' \
                                         (only 'timestamp')"
                                    ),
                                    d.span,
                                ),
                                None => {}
                            }
                        }
                    }
                    DecoTarget::Enum => {
                        let ok = keywords == 0
                            && positionals == 1
                            && matches!(&d.args[0].value, Expr::Name(n, _) if n == "open");
                        if !ok {
                            self.error(
                                ErrorCode::ZAP5047,
                                "@json(...) on an enum takes only the flag `open`, i.e. @json(open)",
                                d.span,
                            );
                        }
                    }
                    _ => self.error(
                        ErrorCode::ZAP5047,
                        format!(
                            "@json applies to a field or an enum, not {}",
                            target.describe()
                        ),
                        d.span,
                    ),
                }
            }
            "on_signal" => {
                // `@on_signal(Signal.X)` on a function (ticket #77, D-0058):
                // register it as a handler for signal X. Exactly one positional
                // argument, a `Signal` enum member.
                self.require_target(d, target, DecoTarget::Function);
                if positionals != 1 || keywords != 0 {
                    self.error(
                        ErrorCode::ZAP5037,
                        "@on_signal(Signal.X) takes exactly one Signal argument, \
                         e.g. @on_signal(Signal.SIGINT)",
                        d.span,
                    );
                } else {
                    let ok = matches!(
                        &d.args[0].value,
                        Expr::Attr { base, name, .. }
                            if matches!(base.as_ref(), Expr::Name(n, _) if n == "Signal")
                                && self
                                    .enums
                                    .get("Signal")
                                    .is_some_and(|e| e.variant(name).is_some())
                    );
                    if !ok {
                        self.error(
                            ErrorCode::ZAP5037,
                            "@on_signal expects a Signal member, e.g. @on_signal(Signal.SIGINT)",
                            d.span,
                        );
                    }
                }
            }
            "default" => {
                self.require_target(d, target, DecoTarget::Field);
                if positionals != 1 || keywords != 0 {
                    self.error(
                        ErrorCode::ZAP5038,
                        "@default(expr) takes exactly one expression, e.g. @default(now())",
                        d.span,
                    );
                }
                // A field may carry a literal default (`T x = e`) or a
                // `@default(e)` decorator, but not both — they would race to
                // initialize the same slot (D-0101).
                if let Some(f) = field {
                    if f.default.is_some() {
                        self.error(
                            ErrorCode::ZAP5038,
                            format!(
                                "field '{}' has both a literal default and @default(...); keep one",
                                f.name
                            ),
                            d.span,
                        );
                    }
                }
            }
            "range" => {
                self.require_target(d, target, DecoTarget::Field);
                if positionals != 0 {
                    self.error(
                        ErrorCode::ZAP5039,
                        "@range(...) takes keyword bounds: @range(min=0, max=150)",
                        d.span,
                    );
                }
                let mut min_lit: Option<f64> = None;
                let mut max_lit: Option<f64> = None;
                for a in &d.args {
                    if let Some(n) = &a.name {
                        if n != "min" && n != "max" {
                            self.error(
                                ErrorCode::ZAP5039,
                                format!("@range has no bound '{n}' (expected 'min' or 'max')"),
                                d.span,
                            );
                            continue;
                        }
                        match numeric_literal(&a.value) {
                            Some(v) => {
                                if n == "min" {
                                    min_lit = Some(v);
                                } else {
                                    max_lit = Some(v);
                                }
                            }
                            None => self.error(
                                ErrorCode::ZAP5039,
                                format!("@range {n} must be a numeric literal, e.g. @range({n}=0)"),
                                d.span,
                            ),
                        }
                    }
                }
                if d.args.is_empty() {
                    self.error(
                        ErrorCode::ZAP5039,
                        "@range(...) needs at least one of min/max",
                        d.span,
                    );
                }
                if let (Some(lo), Some(hi)) = (min_lit, max_lit) {
                    if lo > hi {
                        self.error(
                            ErrorCode::ZAP5039,
                            "@range(min, max) has min greater than max",
                            d.span,
                        );
                    }
                }
                // `@range` constrains a numeric field (D-0101).
                if let Some(f) = field {
                    let ty = self.resolve_type(&f.ty);
                    if !ty.is_numeric() && !matches!(ty, Ty::Unknown) {
                        self.error(
                            ErrorCode::ZAP5039,
                            format!(
                                "@range applies to a numeric field, not '{}' (field '{}')",
                                ty.display(),
                                f.name
                            ),
                            d.span,
                        );
                    }
                }
            }
            "validate" => {
                self.require_target(d, target, DecoTarget::Field);
                if positionals != 0 {
                    self.error(
                        ErrorCode::ZAP5040,
                        "@validate(...) takes keyword constraints, e.g. @validate(max_len=50)",
                        d.span,
                    );
                }
                let mut has_len = false;
                for a in &d.args {
                    match a.name.as_deref() {
                        Some("max_len") | Some("min_len") => {
                            has_len = true;
                            match numeric_literal(&a.value) {
                                Some(v) if v >= 0.0 && v.fract() == 0.0 => {}
                                _ => self.error(
                                    ErrorCode::ZAP5040,
                                    format!(
                                        "@validate({}=...) needs a non-negative integer literal",
                                        a.name.as_deref().unwrap_or("")
                                    ),
                                    d.span,
                                ),
                            }
                        }
                        // `matches=` (regex) is a documented follow-up (#472); it
                        // parses and sema-checks nowhere yet, so reject it cleanly
                        // rather than silently accepting an unimplemented option.
                        Some(other) => self.error(
                            ErrorCode::ZAP5040,
                            format!(
                                "@validate has no constraint '{other}' \
                                 (supported: max_len, min_len)"
                            ),
                            d.span,
                        ),
                        None => {}
                    }
                }
                // Length constraints apply to a string/bytes field (D-0101).
                if has_len {
                    if let Some(f) = field {
                        let ty = self.resolve_type(&f.ty);
                        if !matches!(ty, Ty::Str | Ty::Bytes | Ty::Unknown) {
                            self.error(
                                ErrorCode::ZAP5040,
                                format!(
                                    "@validate(max_len/min_len) applies to a str or bytes field, \
                                     not '{}' (field '{}')",
                                    ty.display(),
                                    f.name
                                ),
                                d.span,
                            );
                        }
                    }
                }
            }
            "cache" => {
                // `@cache` (I#666, D-0115, epic #661 — supersedes the D-0109
                // deferral now that module-level state exists) is a free-function
                // decorator expanded *before* sema into ordinary, `zap expand`-
                // visible Zap over a module-level dict (crates/driver's
                // behavioral-decorator pass). A correct application on a function
                // is consumed by that pass and never reaches this arm, so all this
                // does is catch a *misapplication* to a non-function target; the
                // per-signature checks (hashable params, a return value, free
                // function only) live in the expansion pass as ZAP5056.
                self.require_target(d, target, DecoTarget::Function);
            }
            "inject" => {
                // Declined for core (I#472, D-0109): `@inject` is a DI-framework
                // hook; core has no DI container/registry, and the concrete
                // FastAPI-style `Depends(...)` need is met at the framework level
                // by the reflection-driven handler adapter (D-0089/D-0092, I#466).
                // There is no planned core `@inject`; the name stays reserved only
                // so this redirect-to-`Depends` message fires instead of "unknown
                // decorator".
                self.error(
                    ErrorCode::ZAP5055,
                    "@inject is not a core decorator: dependency injection is provided by \
                     reflection-driven `Depends(...)` adapters (D-0089/D-0092), not a core \
                     decorator (I#472, D-0109); see STABILITY.md",
                    d.span,
                );
            }
            "deprecated" => {
                // A function/method decorator (I#472, D-0109): every call site
                // of the decorated callable gets the ZAP5124 use-site warning.
                // Deprecating a type/field/parameter is deferred (no well-defined
                // v1 use site), so the target is restricted to a function here.
                self.require_target(d, target, DecoTarget::Function);
                // Optional single positional message string.
                // `map_or(true, …)` rather than `is_none_or` to stay within the
                // crate's MSRV (`rust-version = 1.74`; `is_none_or` is 1.82+).
                #[allow(clippy::unnecessary_map_or)]
                let well_formed = d.args.len() <= 1
                    && keywords == 0
                    && d.args
                        .first()
                        .map_or(true, |a| matches!(&a.value, Expr::Str { .. }));
                if !well_formed {
                    self.error(
                        ErrorCode::ZAP5042,
                        "@deprecated takes an optional message string, e.g. @deprecated(\"use X\")",
                        d.span,
                    );
                }
            }
            // Concurrency-safety marker (#664, D-0113). `@shareable` applies to a
            // `struct` and takes no arguments; it is a metadata-only assertion
            // recorded for the sharing lint and reflection.
            SHAREABLE_MARKER => {
                self.require_target(d, target, DecoTarget::Struct);
                if !d.args.is_empty() {
                    self.error(
                        ErrorCode::ZAP5041,
                        "@shareable takes no arguments".to_string(),
                        d.span,
                    );
                }
            }
            // Affine with-bound resource marker (#876, D-0159). `@scoped` applies
            // to a `struct` and takes no arguments; the affine/with-head-only/
            // escape semantics and the __enter__/__exit__ well-formedness rule are
            // enforced in `check_scoped` (`crates/sema/src/scoped.rs`).
            SCOPED_MARKER => {
                self.require_target(d, target, DecoTarget::Struct);
                if !d.args.is_empty() {
                    self.error(
                        ErrorCode::ZAP5041,
                        "@scoped takes no arguments".to_string(),
                        d.span,
                    );
                }
            }
            // --- test framework (D-0054, issue #73) ---
            "test" | "before" | "after" | "before_all" | "after_all" => {
                self.require_target(d, target, DecoTarget::Function);
                if !d.args.is_empty() {
                    self.error(
                        ErrorCode::ZAP5041,
                        format!("@{} takes no arguments", d.name),
                        d.span,
                    );
                }
            }
            "skip" => {
                self.require_target(d, target, DecoTarget::Function);
                // Optional single positional reason string.
                if d.args.len() > 1 || keywords != 0 {
                    self.error(
                        ErrorCode::ZAP5043,
                        "@skip takes an optional reason string, e.g. @skip(\"flaky\")",
                        d.span,
                    );
                }
            }
            "skip_if" => {
                self.require_target(d, target, DecoTarget::Function);
                // `@skip_if(cond [, reason])`: a runtime condition and an
                // optional reason. The condition is evaluated in the harness, so
                // it is not type-checked here.
                if !(1..=2).contains(&positionals) || keywords != 0 {
                    self.error(
                        ErrorCode::ZAP5044,
                        "@skip_if takes a condition and an optional reason, \
                         e.g. @skip_if(slow, \"CI only\")",
                        d.span,
                    );
                }
            }
            "expect_raises" => {
                self.require_target(d, target, DecoTarget::Function);
                // `@expect_raises(ErrType)`: exactly one positional naming the
                // expected exception type. Typed matching is deferred (catch-all
                // today), so the name is not resolved here.
                if positionals != 1 || keywords != 0 {
                    self.error(
                        ErrorCode::ZAP5045,
                        "@expect_raises takes one exception type, e.g. @expect_raises(ValueError)",
                        d.span,
                    );
                }
            }
            "parametrize" => {
                self.require_target(d, target, DecoTarget::Function);
                // `@parametrize(cases=[...])`: a keyword `cases` list of case
                // rows. Each row is spread across the test's parameters.
                let has_cases = d.args.iter().any(|a| a.name.as_deref() == Some("cases"));
                let only_cases = d.args.iter().all(|a| a.name.as_deref() == Some("cases"));
                if !has_cases || !only_cases || d.args.len() != 1 {
                    self.error(
                        ErrorCode::ZAP5046,
                        "@parametrize takes a single `cases` list, \
                         e.g. @parametrize(cases=[(1, 2, 3), (4, 5, 9)])",
                        d.span,
                    );
                } else if let Some(a) = d.args.first() {
                    if !matches!(a.value, Expr::List(..)) {
                        self.error(
                            ErrorCode::ZAP5046,
                            "@parametrize(cases=...) expects a list of cases",
                            d.span,
                        );
                    }
                }
            }
            _ => {}
        }
    }

    fn require_target(&mut self, d: &Decorator, got: DecoTarget, want: DecoTarget) {
        if got != want {
            self.error(
                ErrorCode::ZAP5047,
                format!(
                    "@{} applies to {}, not {}",
                    d.name,
                    want.describe(),
                    got.describe()
                ),
                d.span,
            );
        }
    }

    fn check_reflect_args(&mut self, fname: &str, arity: usize, args: &[Arg], span: Span) {
        if args.len() != arity {
            self.error(
                ErrorCode::ZAP4025,
                format!(
                    "{fname}() takes exactly {arity} argument(s), found {}",
                    args.len()
                ),
                span,
            );
        }
        for a in args {
            let t = self.infer(&a.value);
            if !matches!(t, Ty::Str | Ty::Unknown) {
                self.error(
                    ErrorCode::ZAP4024,
                    format!("{fname}() expects str arguments, found {}", t.display()),
                    span,
                );
            }
        }
    }

    // --- checking ---

    fn check_module(&mut self, module: &Module) {
        self.push_scope();
        // Python imports (D-0012): bind each imported module's name to a Python
        // handle type in module scope so references like `np.array(...)` resolve
        // (and don't trip the "unknown name" check). Declared up front so a
        // function defined before the `import` still sees the alias.
        for item in &module.items {
            if let Item::Import(Import::Python {
                module: m,
                alias,
                span,
            }) = item
            {
                let name = alias.clone().unwrap_or_else(|| m.clone());
                self.declare(&name, Ty::Py, *span);
            }
        }
        // Module-level declarations (D-0112): `const T NAME = expr` (immutable,
        // #658 Phase 1) and `T NAME = expr` (mutable, #662 Phase 2). Declared up
        // front — before any function body is checked — so a reference resolves
        // regardless of source order (eager, import-topological init; the linker
        // ordered the items, D-0081). Each initializer is type-checked here, in
        // source order, so a later binding may read an earlier one (and a forward
        // reference to a not-yet-declared one is an unknown-name error, which
        // matches the use-before-init rule). Function signatures were already
        // collected (`collect_sigs`), so an initializer may call a function. The
        // only difference between the two forms is constness: a `const` is
        // recorded immutable (reassignment → ZAP3017), a mutable binding stays
        // reassignable from any function (the light concurrency model, #661).
        for item in &module.items {
            if let Item::Stmt(Stmt::Let {
                is_const,
                ty,
                name,
                value,
                span,
            }) = item
            {
                let vt = self.infer(value);
                let dt = match ty {
                    None => default_type(vt),
                    Some(t) => {
                        self.check_numeric_container_type(t);
                        let dt = self.resolve_type(t);
                        if !self.coercible(&vt, &dt) {
                            self.error(
                                ErrorCode::ZAP4001,
                                format!(
                                    "cannot assign value of type {} to {} {}{}",
                                    vt.display(),
                                    dt.display(),
                                    name,
                                    self.conformance_detail(&vt, &dt),
                                ),
                                expr_span(value),
                            );
                        }
                        self.check_collection_literal_elems(value, &vt, &dt);
                        dt
                    }
                };
                if *is_const {
                    self.declare_const(name, dt, *span);
                } else {
                    // Sharing lint (#664, D-0113): a MUTABLE module global whose
                    // type is not `@shareable` is careless shared state — every
                    // later read/write of it warns (ZAP5125). A `const` global
                    // can never race, and a `@shareable`-typed one is asserted
                    // safe, so neither is recorded. Checked before `declare`
                    // consumes `dt`.
                    if !self.declared_type_is_shareable(ty.as_ref(), &dt) {
                        self.share_lint_globals.insert(name.clone());
                    }
                    self.declare(name, dt, *span);
                }
            }
        }
        for item in &module.items {
            match item {
                Item::Function(f) => self.check_function(f, None),
                Item::Struct(s) => {
                    self.enclosing_type_bounds = self.type_param_bounds(&s.generics);
                    for m in &s.methods {
                        self.check_function(m, Some(&s.name));
                    }
                    self.enclosing_type_bounds = Vec::new();
                }
                Item::Enum(e) => {
                    // A tier-1 (constant) enum's `Equatable`/`Hashable` are
                    // INTRINSIC and non-overridable (D-0139/#809): equality is
                    // backing-value identity (D-0018), so a hand-written
                    // `__eq__`/`__hash__` — which could try to merge two distinct
                    // members — is rejected. `__cmp__` stays allowed (ordering is
                    // a choice; it never changes equality). Tier-3 (data-carrying)
                    // enums are value aggregates and may override, like structs.
                    let is_tier1 = !e.variants.iter().any(|v| !v.fields.is_empty());
                    if is_tier1 {
                        for m in &e.methods {
                            if m.name == "__eq__" || m.name == "__hash__" {
                                self.error(
                                    ErrorCode::ZAP5126,
                                    format!(
                                        "tier-1 enum '{}' cannot define '{}': equality and \
                                         hashing are intrinsic (by backing value, D-0018); use \
                                         '__cmp__' to customize ordering only (D-0139/#809)",
                                        e.name, m.name
                                    ),
                                    m.span,
                                );
                            }
                        }
                    }
                    // Methods on an enum (D-0046) — checked at definition like
                    // struct methods, with `self` typed as the enum. For a
                    // generic enum (`Optional[T]`, I#154) the type parameters are
                    // treated as opaque: they resolve to `Unknown`, so a method
                    // body may use only universal operations (bind/pass/return/
                    // match), which is the unconstrained check-at-definition rule
                    // (Fork B/M1). Enum-method *lowering* lands in the ir slice.
                    self.enclosing_type_bounds = self.type_param_bounds(&e.generics);
                    for m in &e.methods {
                        self.check_function(m, Some(&e.name));
                    }
                    self.enclosing_type_bounds = Vec::new();
                }
                // Module-level bindings (const and mutable) were already declared
                // and checked in the pre-pass above (D-0112); skip them here so
                // the binding is not re-declared.
                Item::Stmt(Stmt::Let { .. }) => {}
                Item::Stmt(st) => self.check_stmt(st),
                _ => {}
            }
        }
        self.pop_scope();
    }

    /// Validate the structural rules on a parameter list (#601): a `V **name`
    /// keyword-collector must be the last parameter, may appear at most once,
    /// and may not carry a default; any `TYPE... name` variadic must precede it.
    /// (The parser already forbids a parameter being both variadic and kwargs.)
    fn check_param_shape(&mut self, params: &[Param]) {
        let mut seen_kwargs = false;
        for (i, p) in params.iter().enumerate() {
            if seen_kwargs {
                // Anything after a `**` collector is misplaced; report it once
                // against the collector having already been seen.
                self.error(
                    ErrorCode::ZAP5121,
                    format!(
                        "parameter '{}' follows the `**` keyword-collector, which \
                         must be last",
                        p.name
                    ),
                    p.span,
                );
            }
            if p.kwargs {
                if seen_kwargs {
                    self.error(
                        ErrorCode::ZAP5121,
                        format!(
                            "'{}' is a second `**` keyword-collector; at most one \
                             is allowed",
                            p.name
                        ),
                        p.span,
                    );
                }
                seen_kwargs = true;
                if p.default.is_some() {
                    self.error(
                        ErrorCode::ZAP5121,
                        format!(
                            "the `**` keyword-collector '{}' cannot carry a \
                             default value",
                            p.name
                        ),
                        p.span,
                    );
                }
                let _ = i;
            }
        }
    }

    /// The `(type-parameter name, bound interface name)` pairs of a declaration's
    /// type parameters whose bound names a known interface (I#811). Used to seed
    /// [`Self::enclosing_type_bounds`] for a generic type's method bodies.
    fn type_param_bounds(&self, generics: &[TypeParam]) -> Vec<(String, String)> {
        generics
            .iter()
            .filter_map(|tp| match &tp.bound {
                Some(Type::Named { name: iface, .. }) if self.interfaces.contains_key(iface) => {
                    Some((tp.name.clone(), iface.clone()))
                }
                _ => None,
            })
            .collect()
    }

    fn check_function(&mut self, f: &Function, self_ty: Option<&str>) {
        for p in &f.params {
            if let Some(t) = &p.ty {
                self.check_numeric_container_type(t);
            }
        }
        if let Some(t) = &f.ret {
            self.check_numeric_container_type(t);
        }
        self.check_param_shape(&f.params);
        self.push_scope();
        let new_ret = f
            .ret
            .as_ref()
            .map(|t| self.resolve_type(t))
            .unwrap_or(Ty::Unit);
        let saved = std::mem::replace(&mut self.ret_ty, new_ret);
        // Interface bounds on this declaration's own type parameters (I#164,
        // slice 5): record `T -> Iface` for each `T: Iface` whose bound names a
        // known interface, so that inside the body a method call on a value of a
        // bounded type parameter resolves through the interface (static dispatch
        // after monomorphization) and a call to a method the bound does not
        // provide is rejected here, at definition (check-at-definition, Fork M1).
        // Unbounded type parameters are left opaque (`Unknown`) as before.
        let saved_bounds = std::mem::take(&mut self.tyvar_bounds);
        // Bounds on the *enclosing* type's parameters (I#811): a method of
        // `struct BST[K: Comparable, V]` may call `Comparable`'s methods on a
        // parameter typed `K`. A method's own type parameter shadows an enclosing
        // one of the same name (inserted after, below).
        for (name, iface) in &self.enclosing_type_bounds {
            self.tyvar_bounds.insert(name.clone(), iface.clone());
        }
        for tp in &f.generics {
            // A method's own type parameter is a fresh variable that shadows an
            // enclosing one of the same name: drop any inherited bound first, then
            // apply this parameter's own (an unbounded own parameter stays opaque).
            self.tyvar_bounds.remove(&tp.name);
            if let Some(Type::Named { name: iface, .. }) = &tp.bound {
                if self.interfaces.contains_key(iface) {
                    self.tyvar_bounds.insert(tp.name.clone(), iface.clone());
                }
            }
        }
        // Generator validation (I#67, D-0026/D-0048): a `def` whose body contains
        // `yield` is a generator and returns an `Iterator`. Free functions,
        // struct methods, and tier-3 (data-carrying) enum methods may be
        // generators (I#429 extended the struct-method case; I#449 the enum one):
        // `self` is captured into the state frame like any other parameter. A
        // generator `main` is still rejected, as is an explicit return type (its
        // return type is the synthetic iterator). A *tier-1* enum method
        // generator stays rejected: tier-1 (unboxed-integer) enum methods do not
        // lower at all yet, generator or not, so this is the broader tier-1
        // method-lowering gap rather than anything generator-specific (EnumDef).
        let is_gen = genanalysis::is_generator(f);
        let saved_in_gen = std::mem::replace(&mut self.in_generator, is_gen);
        if is_gen {
            // Only a *free* `main` is the entry point; a method named `main` is
            // not and may be a generator.
            if self_ty.is_none() && f.name == "main" {
                self.error(
                    ErrorCode::ZAP5061,
                    "the entry point 'main' cannot be a generator (it contains \
                     'yield', I#67)"
                        .to_string(),
                    f.span,
                );
            }
            if self_ty.is_some_and(|n| self.enums.get(n).is_some_and(|e| !e.is_sum)) {
                self.error(
                    ErrorCode::ZAP5060,
                    "generators on a tier-1 enum method are not supported yet \
                     (tier-1 enum methods do not lower at all yet; I#449 enabled \
                     generators on tier-3 data-carrying enum methods); define the \
                     generator as a free function, a struct method, or a method \
                     on a data-carrying enum"
                        .to_string(),
                    f.span,
                );
            }
            if f.ret.is_some() {
                self.error(
                    ErrorCode::ZAP5062,
                    "a generator must not declare a return type: calling it returns \
                     an Iterator over the yielded values (I#67, D-0048)"
                        .to_string(),
                    f.span,
                );
            }
        }
        if let Some(name) = self_ty {
            self.declare_param("self", Ty::Named(name.to_string()), f.span);
        }
        for p in &f.params {
            if p.name == "self" {
                self.declare_param(
                    "self",
                    self_ty
                        .map(|n| Ty::Named(n.to_string()))
                        .unwrap_or(Ty::Unknown),
                    p.span,
                );
                continue;
            }
            // A parameter typed exactly as a bounded type parameter `T: Iface`
            // keeps the type-variable identity (`Ty::Named("T")`) so method calls
            // on it resolve through the bound (I#164); other types resolve
            // normally (an unbounded `T` stays `Unknown`).
            let base = match &p.ty {
                Some(Type::Named { name, args, .. })
                    if args.is_empty() && self.tyvar_bounds.contains_key(name) =>
                {
                    Ty::Named(name.clone())
                }
                Some(t) => self.resolve_type(t),
                None => Ty::Unknown,
            };
            let ty = if p.variadic {
                Ty::List(Box::new(base))
            } else if p.kwargs {
                // A `V **name` collector is bound in the body as `dict[str, V]`
                // (#601), mirroring how a variadic binds as `list[elem]`.
                Ty::Dict(Box::new(Ty::Str), Box::new(base))
            } else {
                base
            };
            self.declare_param(&p.name, ty, p.span);
        }
        for st in &f.body.stmts {
            self.check_stmt(st);
        }
        self.ret_ty = saved;
        self.tyvar_bounds = saved_bounds;
        self.in_generator = saved_in_gen;
        self.pop_scope();
    }

    fn check_block(&mut self, b: &Block) {
        self.push_scope();
        for st in &b.stmts {
            self.check_stmt(st);
        }
        self.pop_scope();
    }

    /// Check the options on a `scope(...)` block (D-0044). `on_error` takes the
    /// `OnError` enum (`OnError.Cancel` | `OnError.Wait`) — a typed policy so a
    /// misspelling is a compile-time error, not a silent runtime surprise.
    /// Deadlines are a nonnegative integer number of milliseconds (no unit
    /// suffix; `_` digit separators are fine, e.g. `timeout=30_000`).
    fn check_scope_opts(&mut self, opts: &[Arg], span: Span) {
        for a in opts {
            match a.name.as_deref() {
                Some("on_error") => {
                    if !is_on_error_policy(&a.value) {
                        self.error(
                            ErrorCode::ZAP5070,
                            "scope on_error expects OnError.Cancel or OnError.Wait".to_string(),
                            expr_span(&a.value),
                        );
                    }
                }
                Some("timeout") => {
                    let ty = self.infer(&a.value);
                    if !matches!(ty, Ty::Int32 | Ty::Int64 | Ty::UntypedInt | Ty::Unknown) {
                        self.error(
                            ErrorCode::ZAP4024,
                            "scope timeout requires an integer duration in milliseconds"
                                .to_string(),
                            expr_span(&a.value),
                        );
                    }
                }
                // `scope(cancel_on=[Signal.X, ...])` (ticket #77, D-0058): a
                // signal in the list cancels the scope tree. The value is a list
                // of `Signal` members.
                Some("cancel_on") => {
                    if let Expr::List(elems, _) = &a.value {
                        for e in elems {
                            let et = self.infer(e);
                            if !matches!(&et, Ty::Named(n) if n == "Signal")
                                && !matches!(et, Ty::Unknown)
                            {
                                self.error(
                                    ErrorCode::ZAP5072,
                                    format!(
                                        "scope cancel_on expects a list of Signal, found {}",
                                        et.display()
                                    ),
                                    expr_span(e),
                                );
                            }
                        }
                    } else {
                        self.error(
                            ErrorCode::ZAP5072,
                            "scope cancel_on expects a list, e.g. \
                             cancel_on=[Signal.SIGINT, Signal.SIGTERM]"
                                .to_string(),
                            expr_span(&a.value),
                        );
                    }
                }
                // `scope(grace=<ms>)` (ticket #77): hard-exit deadline, in
                // milliseconds, measured from the first cancel_on signal.
                Some("grace") => {
                    let gt = self.infer(&a.value);
                    if !gt.is_int() && !matches!(gt, Ty::Unknown) {
                        self.error(
                            ErrorCode::ZAP5073,
                            format!(
                                "scope grace must be an integer number of milliseconds, found {}",
                                gt.display()
                            ),
                            expr_span(&a.value),
                        );
                    }
                }
                Some(other) => self.error(
                    ErrorCode::ZAP5074,
                    format!(
                        "scope() has no option '{other}' \
                         (expected 'timeout', 'on_error', 'cancel_on', or 'grace')"
                    ),
                    expr_span(&a.value),
                ),
                None => self.error(
                    ErrorCode::ZAP5075,
                    "scope() options must be keyword arguments (e.g. on_error=OnError.Wait)"
                        .to_string(),
                    span,
                ),
            }
        }
    }

    fn check_stmt(&mut self, st: &Stmt) {
        match st {
            // Conditional-compilation blocks are expanded by `resolve_cfg`
            // before type-checking (see `analyze`), so the checker never sees
            // one. Reaching here means that invariant was violated.
            Stmt::Cfg(_) => unreachable!(
                "`when target.*` must be cfg-resolved before type-checking \
                 (zapc_syntax::cfg::resolve_cfg)"
            ),
            Stmt::Let {
                is_const,
                ty,
                name,
                value,
                span,
            } => {
                let vt = self.infer(value);
                let dt = match ty {
                    None => default_type(vt),
                    Some(t) => {
                        self.check_numeric_container_type(t);
                        let dt = self.resolve_type(t);
                        if !self.coercible(&vt, &dt) {
                            self.error(
                                ErrorCode::ZAP4001,
                                format!(
                                    "cannot assign value of type {} to {} {}{}",
                                    vt.display(),
                                    dt.display(),
                                    name,
                                    self.conformance_detail(&vt, &dt),
                                ),
                                expr_span(value),
                            );
                        }
                        self.check_collection_literal_elems(value, &vt, &dt);
                        dt
                    }
                };
                // A `const` binding (D-0003) is immutable — record it so a later
                // reassignment is rejected (ZAP3017).
                if *is_const {
                    self.declare_const(name, dt, *span);
                } else {
                    self.declare(name, dt, *span);
                }
            }
            Stmt::Assign { target, value, .. } => {
                let vt = self.infer(value);
                if let Expr::Name(n, span) = target {
                    // A `const` binding cannot be reassigned (D-0003, ZAP3017).
                    if self.reject_const_reassign(n, *span) {
                        return;
                    }
                    // Sharing lint: writing a shared mutable global (#664/D-0113).
                    self.note_share_access(n, *span);
                    match self.lookup(n) {
                        Some(dt) => {
                            if !self.coercible(&vt, &dt) {
                                self.error(
                                    ErrorCode::ZAP4001,
                                    format!(
                                        "cannot assign value of type {} to {} of type {}",
                                        vt.display(),
                                        n,
                                        dt.display()
                                    ),
                                    expr_span(value),
                                );
                            }
                            self.check_collection_literal_elems(value, &vt, &dt);
                        }
                        None => self.error(
                            ErrorCode::ZAP3002,
                            format!("assignment to undeclared variable '{n}'"),
                            *span,
                        ),
                    }
                } else {
                    self.infer(target);
                }
            }
            Stmt::AssignChain { targets, value, .. } => {
                // Chained assignment `t0 = t1 = ... = value` (#594): the RHS is
                // evaluated once and assigned to every target. Each target is
                // checked with exactly the same rule as a single `Assign` — a
                // name must already be a declared binding (chained assignment
                // never declares), a subscript/field is checked by inference,
                // and any other form is not a valid assignment target.
                let vt = self.infer(value);
                for target in targets {
                    match target {
                        Expr::Name(n, span) => {
                            // A `const` target cannot be reassigned (D-0003).
                            if self.reject_const_reassign(n, *span) {
                                continue;
                            }
                            // Sharing lint: writing a shared mutable global
                            // (#664/D-0113).
                            self.note_share_access(n, *span);
                            match self.lookup(n) {
                                Some(dt) => {
                                    if !self.coercible(&vt, &dt) {
                                        self.error(
                                            ErrorCode::ZAP4001,
                                            format!(
                                                "cannot assign value of type {} to {} of type {}",
                                                vt.display(),
                                                n,
                                                dt.display()
                                            ),
                                            expr_span(value),
                                        );
                                    }
                                    self.check_collection_literal_elems(value, &vt, &dt);
                                }
                                None => self.error(
                                    ErrorCode::ZAP3002,
                                    format!("assignment to undeclared variable '{n}'"),
                                    *span,
                                ),
                            }
                        }
                        Expr::Index { .. } | Expr::Attr { .. } => {
                            self.infer(target);
                        }
                        other => self.error(
                            ErrorCode::ZAP4001,
                            "a chained-assignment target must be a name, a subscript, \
                             or a field"
                                .to_string(),
                            expr_span(other),
                        ),
                    }
                }
            }
            Stmt::Del { target, .. } => {
                // `del d[k]` / `del xs[i]` (#595). The parser guarantees the
                // target is a subscript; inferring it type-checks the base and
                // the key/index exactly as the corresponding read `d[k]` would,
                // which is also what the removal lowering reuses.
                self.infer(target);
            }
            Stmt::AugAssign {
                target, op, value, ..
            } => {
                let vt = self.infer(value);
                if let Expr::Name(n, span) = target {
                    // An augmented assignment rebinds the name, so a `const` is
                    // just as illegal here as a plain reassignment (D-0003).
                    if self.reject_const_reassign(n, *span) {
                        return;
                    }
                    // Sharing lint: an augmented assign is a read-modify-write of
                    // a shared mutable global — one access site (#664/D-0113).
                    self.note_share_access(n, *span);
                    match self.lookup(n) {
                        Some(tt) => {
                            // `t += v` on a user struct calls the in-place dunder
                            // (`__iadd__` etc.) when defined, else derives from the
                            // binary operator's `__add__` etc. (D-0047).
                            let r = match self.check_struct_aug_assign(*op, &tt, &vt, *span) {
                                Some(ty) => ty,
                                None => self.arith_result(*op, &tt, &vt, *span),
                            };
                            if !assignable(&r, &tt) {
                                self.error(
                                    ErrorCode::ZAP4002,
                                    format!(
                                        "augmented assignment produces {} but '{}' has type {}",
                                        r.display(),
                                        n,
                                        tt.display()
                                    ),
                                    *span,
                                );
                            }
                        }
                        None => self.error(
                            ErrorCode::ZAP3002,
                            format!("assignment to undeclared variable '{n}'"),
                            *span,
                        ),
                    }
                } else {
                    self.infer(target);
                }
            }
            Stmt::Expr(e) => {
                self.infer(e);
            }
            Stmt::If {
                branches,
                else_block,
                ..
            } => {
                for (cond, body) in branches {
                    self.infer(cond);
                    self.check_block(body);
                }
                if let Some(b) = else_block {
                    self.check_block(b);
                }
            }
            Stmt::While { cond, body, .. } => {
                self.infer(cond);
                self.check_block(body);
            }
            Stmt::DoWhile { body, cond, .. } => {
                self.check_block(body);
                self.infer(cond);
            }
            Stmt::For {
                target,
                iter,
                body,
                span,
            } => {
                // Iterating an enum *type* yields its members in declaration
                // order (`for col in Color:`, D-0018). The iterable is a type
                // name, not a value, so it is matched before inferring.
                let elem = if let Expr::Name(n, _) = iter {
                    if let Some(info) = self.enums.get(n) {
                        if info.is_sum {
                            self.error(
                                ErrorCode::ZAP4070,
                                "only tier-1 enum types can be iterated".to_string(),
                                expr_span(iter),
                            );
                            Ty::Unknown
                        } else {
                            Ty::Named(n.clone())
                        }
                    } else {
                        let it = self.infer(iter);
                        self.iter_element(&it, *span)
                    }
                } else {
                    let it = self.infer(iter);
                    self.iter_element(&it, *span)
                };
                self.push_scope();
                match target {
                    Expr::Name(n, s) => self.declare(n, elem, *s),
                    // Tuple-destructuring target (`for k, v in d.items()`,
                    // ticket #299): bind each name to the matching element-type
                    // field when the element is a statically-known tuple;
                    // otherwise (e.g. an `Unknown` element) fall back to loose.
                    Expr::Tuple(elems, _) => {
                        let field_tys = match &elem {
                            Ty::Tuple(ts) if ts.len() == elems.len() => Some(ts.clone()),
                            _ => None,
                        };
                        for (i, e) in elems.iter().enumerate() {
                            if let Expr::Name(n, s) = e {
                                let t = field_tys
                                    .as_ref()
                                    .map(|ts| ts[i].clone())
                                    .unwrap_or(Ty::Unknown);
                                self.declare(n, t, *s);
                            }
                        }
                    }
                    _ => {}
                }
                for st in &body.stmts {
                    self.check_stmt(st);
                }
                self.pop_scope();
            }
            Stmt::Match {
                scrutinee,
                arms,
                span,
            } => {
                let scrut = self.infer(scrutinee);
                let mut covered: HashSet<String> = HashSet::new();
                let mut has_catch_all = false;
                let mut guarded_catch_all = false;
                for arm in arms {
                    self.push_scope();
                    let (vs, catch_all) = self.check_pattern(&arm.pattern, &scrut);
                    if arm.guard.is_none() {
                        for v in vs {
                            covered.insert(v);
                        }
                    }
                    if catch_all {
                        // A guarded catch-all does not make the match exhaustive
                        // (the guard may fail).
                        if arm.guard.is_some() {
                            guarded_catch_all = true;
                        } else {
                            has_catch_all = true;
                        }
                    }
                    if let Some(g) = &arm.guard {
                        self.infer(g);
                    }
                    for st in &arm.body.stmts {
                        self.check_stmt(st);
                    }
                    self.pop_scope();
                }
                // Exhaustiveness (D-0040): a `match` on an enum that omits a
                // variant and has no `_`/catch-all arm is a compile error. This
                // one upgrade over Python applies only to enum scrutinees;
                // non-enum matches stay Python-like (no-op on no match).
                let _ = guarded_catch_all;
                if let Ty::Named(ename) = &scrut {
                    if let Some(info) = self.enums.get(ename).cloned() {
                        if !has_catch_all {
                            let missing: Vec<String> = info
                                .variants
                                .iter()
                                .filter(|v| !covered.contains(&v.name))
                                .map(|v| format!("{ename}.{}", v.name))
                                .collect();
                            if !missing.is_empty() {
                                self.error(
                                    ErrorCode::ZAP5080,
                                    format!(
                                        "non-exhaustive match on enum '{ename}': missing {} \
                                         (add the arm(s) or a '_' case)",
                                        missing.join(", ")
                                    ),
                                    *span,
                                );
                            }
                        }
                    }
                }
            }
            Stmt::Return { value, span } if self.in_generator => {
                // A generator ends by falling off the end or a bare `return`
                // (D-0048/PEP 479); `return value` (which would set a
                // StopIteration value in Python) is deferred past v1.
                if let Some(e) = value {
                    self.infer(e);
                    self.error(
                        ErrorCode::ZAP5063,
                        "a generator's 'return' cannot carry a value yet (I#67 v1); \
                         use a bare 'return' to end iteration"
                            .to_string(),
                        *span,
                    );
                }
            }
            Stmt::Return { value, span } => match value {
                Some(e) => {
                    let vt = self.infer(e);
                    let ret = self.ret_ty.clone();
                    if !self.coercible(&vt, &ret) {
                        self.error(
                            ErrorCode::ZAP4003,
                            format!(
                                "return type mismatch: expected {}, found {}",
                                ret.display(),
                                vt.display()
                            ),
                            expr_span(e),
                        );
                    }
                    self.check_collection_literal_elems(e, &vt, &ret);
                }
                None => {
                    if !matches!(self.ret_ty, Ty::Unit | Ty::Unknown) {
                        self.error(
                            ErrorCode::ZAP4004,
                            format!(
                                "function must return a value of type {}",
                                self.ret_ty.display()
                            ),
                            *span,
                        );
                    }
                }
            },
            Stmt::Raise { value, cause, .. } => {
                // `raise StopIteration` (the iterator exhaustion signal, D-0048)
                // names a compiler-known exception, not a user value, so it is
                // not inferred (which would flag it as an unknown name).
                if let Some(e) = value {
                    if !is_stop_iteration(e) {
                        self.infer(e);
                    }
                }
                // `raise <exc> from <cause>` (#604, D-0103): the cause is an
                // ordinary expression (conventionally a caught exception, whose
                // `.cause` is then inspectable); infer it for name resolution.
                if let Some(c) = cause {
                    self.infer(c);
                }
            }
            Stmt::Assert { cond, msg, .. } => {
                // `assert cond [, msg]` (D-0054). The condition is inferred for
                // its side effects / name resolution; the optional message is a
                // normal expression (conventionally a str).
                self.infer(cond);
                if let Some(m) = msg {
                    self.infer(m);
                }
            }
            Stmt::Try {
                body,
                excepts,
                finally,
                ..
            } => {
                self.check_block(body);
                // A `try` is all-`except` or all-`except*` — the two forms
                // cannot be mixed (#605, D-0129). The first clause sets the mode;
                // any clause disagreeing with it is rejected once, at its own
                // span, so the diagnostic points at the offending clause.
                let any_star = excepts.iter().any(|ex| ex.is_star);
                let all_star = excepts.iter().all(|ex| ex.is_star);
                if any_star && !all_star {
                    for ex in excepts.iter() {
                        if ex.is_star != excepts[0].is_star {
                            self.error(
                                ErrorCode::ZAP3001,
                                "a 'try' cannot mix 'except' and 'except*' clauses: use one \
                                 form for all handlers (#605)"
                                    .to_string(),
                                ex.span,
                            );
                        }
                    }
                }
                for (i, ex) in excepts.iter().enumerate() {
                    match &ex.ty {
                        Some(Type::Named { name, args, .. })
                            if args.is_empty()
                                && (is_exception_type(name) || self.user_types.contains(name)) => {}
                        Some(Type::Named { name, .. }) => self.error(
                            ErrorCode::ZAP3001,
                            format!("unknown exception type '{name}'"),
                            ex.span,
                        ),
                        Some(_) => self.error(
                            ErrorCode::ZAP3001,
                            "except expects a named exception type".to_string(),
                            ex.span,
                        ),
                        None => {
                            // `except*` always peels a typed sub-group, so a
                            // bare catch-all `except*:` is meaningless (#605).
                            if ex.is_star {
                                self.error(
                                    ErrorCode::ZAP3001,
                                    "'except*' requires an exception type; a bare catch-all \
                                     'except*:' is not allowed (#605)"
                                        .to_string(),
                                    ex.span,
                                );
                            } else if i + 1 < excepts.len() {
                                self.error(
                                    ErrorCode::ZAP3001,
                                    "a catch-all except clause must be last".to_string(),
                                    ex.span,
                                );
                            }
                        }
                    }
                    self.push_scope();
                    if let Some(b) = &ex.binding {
                        self.declare(b, Ty::Unknown, ex.span);
                    }
                    for st in &ex.body.stmts {
                        self.check_stmt(st);
                    }
                    self.pop_scope();
                }
                if let Some(b) = finally {
                    self.check_block(b);
                }
            }
            Stmt::With { items, body, .. } => {
                self.push_scope();
                for it in items {
                    // `with expect_raises(ErrType):` (D-0054) is a special test
                    // form: it names an exception type, not a value, so its
                    // callee/argument are not inferred (which would flag them as
                    // unknown names) and `as NAME` is typeless.
                    if is_expect_raises(&it.expr) {
                        if let Expr::Call { args, .. } = &it.expr {
                            let type_ok = args.len() == 1
                                && matches!(
                                    args.first().map(|a| &a.value),
                                    Some(Expr::Name(name, _)) if is_exception_type(name)
                                );
                            if !type_ok {
                                self.error(
                                    ErrorCode::ZAP3001,
                                    "expect_raises expects one built-in exception type".to_string(),
                                    it.span,
                                );
                            }
                        }
                        if let Some(a) = &it.alias {
                            self.declare(a, Ty::Unknown, it.span);
                        }
                        continue;
                    }
                    // General context manager (I#185, D-0069): `as NAME` binds the
                    // `__enter__` result, so give it that type when the manager is
                    // a struct implementing the protocol (else stay permissive).
                    let mt = self.infer(&it.expr);
                    if let Some(a) = &it.alias {
                        let bind_ty = self
                            .struct_info(&mt)
                            .and_then(|si| si.methods.get("__enter__"))
                            .map(|sig| sig.ret.clone())
                            .unwrap_or(Ty::Unknown);
                        self.declare(a, bind_ty, it.span);
                    }
                }
                for st in &body.stmts {
                    self.check_stmt(st);
                }
                self.pop_scope();
            }
            Stmt::Spawn { call, .. } => {
                // Supported `spawn` callable forms (I#475, D-0118), all with
                // positional arguments only. sema has no first-class-function
                // type (a lambda infers `Unknown`), so it classifies the callee
                // structurally and leaves the precise `fn(...)` / method check to
                // IR lowering, which emits the same `ZAP5076` for a form that
                // slips through (e.g. `spawn n()` where `n` is not callable).
                let supported = match call {
                    Expr::Call { callee, args, .. } => {
                        let positional = args
                            .iter()
                            .all(|a| a.name.is_none() && a.spread == ArgSpread::None);
                        positional
                            && match callee.as_ref() {
                                // A bare lambda literal has no function-typed
                                // context at a spawn site; bind it to a name first.
                                Expr::Lambda { .. } => false,
                                // A local name is a possible first-class function
                                // value (IR confirms its type); an unshadowed
                                // top-level function must supply every parameter
                                // positionally.
                                Expr::Name(name, _) => {
                                    self.lookup(name).is_some()
                                        || self
                                            .sigs
                                            .get(name)
                                            .is_some_and(|sig| args.len() == sig.params.len())
                                }
                                // A method call, or any other callable expression:
                                // IR lowering confirms the receiver/value.
                                _ => true,
                            }
                    }
                    _ => false,
                };
                if !supported {
                    self.error(
                        ErrorCode::ZAP5076,
                        "spawn requires a named function, a user-struct method call, or a \
                         first-class function value, with all arguments supplied positionally"
                            .to_string(),
                        expr_span(call),
                    );
                }
                self.infer(call);
            }
            Stmt::Scope { opts, body, span } => {
                self.check_scope_opts(opts, *span);
                self.check_block(body);
            }
            Stmt::LetTuple {
                ty,
                target,
                value,
                span,
                is_const: _,
            } => {
                let vt = self.infer(value);
                let declared = match ty {
                    Some(t) => {
                        self.check_numeric_container_type(t);
                        let dt = self.resolve_type(t);
                        if !assignable(&vt, &dt) {
                            self.error(
                                ErrorCode::ZAP4001,
                                format!(
                                    "cannot assign value of type {} to {}",
                                    vt.display(),
                                    dt.display()
                                ),
                                expr_span(value),
                            );
                        }
                        dt
                    }
                    None => default_type(vt),
                };
                self.bind_target(target, &declared, true, *span);
            }
            Stmt::AssignTuple {
                target,
                value,
                span,
            } => {
                let vt = default_type(self.infer(value));
                self.bind_target(target, &vt, false, *span);
            }
            Stmt::Break(_) | Stmt::Continue(_) | Stmt::Pass(_) => {}
        }
    }

    /// Type-check a destructuring bind target against the value type `ty`.
    /// `is_decl` distinguishes a declaration (leaves introduce new names) from a
    /// reassignment (leaves are existing lvalues). D-0049.
    fn bind_target(&mut self, t: &BindTarget, ty: &Ty, is_decl: bool, _span: Span) {
        match t {
            BindTarget::Elem(Expr::Name(n, nspan)) => {
                if is_decl {
                    self.declare(n, ty.clone(), *nspan);
                } else if !self.reject_const_reassign(n, *nspan) {
                    // A `const` leaf cannot be a tuple-assignment target (D-0003);
                    // otherwise it is an ordinary reassignment.
                    match self.lookup(n) {
                        Some(dt) => {
                            if !assignable(ty, &dt) {
                                self.error(
                                    ErrorCode::ZAP4001,
                                    format!(
                                        "cannot assign value of type {} to {} of type {}",
                                        ty.display(),
                                        n,
                                        dt.display()
                                    ),
                                    *nspan,
                                );
                            }
                        }
                        None => self.error(
                            ErrorCode::ZAP3002,
                            format!("assignment to undeclared variable '{n}'"),
                            *nspan,
                        ),
                    }
                }
            }
            BindTarget::Elem(e) => {
                if is_decl {
                    self.error(
                        ErrorCode::ZAP5090,
                        "a declaration target must be a name".to_string(),
                        expr_span(e),
                    );
                } else {
                    // An lvalue such as `a[i]` or `a.f`: type-check it as an
                    // ordinary expression (its mutability is enforced elsewhere).
                    self.infer(e);
                }
            }
            BindTarget::Rest(_, rspan) => {
                self.error(
                    ErrorCode::ZAP5091,
                    "'*rest' is only valid as an element of a tuple target".to_string(),
                    *rspan,
                );
            }
            BindTarget::Tuple(elems, tspan) => match ty {
                Ty::Tuple(ts) => self.bind_tuple(elems, ts, is_decl, *tspan),
                Ty::Unknown => {
                    // The value type is unknown (e.g. a Python handle): bind every
                    // leaf loosely rather than cascading errors.
                    for e in elems {
                        self.bind_target(rest_inner(e), &Ty::Unknown, is_decl, *tspan);
                    }
                }
                other => self.error(
                    ErrorCode::ZAP5092,
                    format!("cannot destructure a value of type {}", other.display()),
                    *tspan,
                ),
            },
        }
    }

    /// Bind a tuple pattern's element targets against the tuple's element types,
    /// honoring at most one `*rest` which captures the remaining elements as a
    /// sub-tuple (D-0049).
    fn bind_tuple(&mut self, elems: &[BindTarget], ts: &[Ty], is_decl: bool, span: Span) {
        let rest_positions: Vec<usize> = elems
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e, BindTarget::Rest(..)))
            .map(|(i, _)| i)
            .collect();
        if rest_positions.len() > 1 {
            self.error(
                ErrorCode::ZAP5093,
                "a tuple target may have at most one '*rest' element".to_string(),
                span,
            );
            return;
        }
        match rest_positions.first() {
            None => {
                if elems.len() != ts.len() {
                    self.error(
                        ErrorCode::ZAP5094,
                        format!(
                            "tuple destructuring expects {} element(s), found {}",
                            ts.len(),
                            elems.len()
                        ),
                        span,
                    );
                    return;
                }
                for (e, t) in elems.iter().zip(ts) {
                    self.bind_target(e, t, is_decl, span);
                }
            }
            Some(&r) => {
                // `elems.len() - 1` fixed targets (before and after the rest)
                // must fit within the tuple's arity.
                let fixed = elems.len() - 1;
                if ts.len() < fixed {
                    self.error(
                        ErrorCode::ZAP5094,
                        format!(
                            "tuple destructuring with '*rest' needs at least {fixed} \
                             element(s), found {}",
                            ts.len()
                        ),
                        span,
                    );
                    return;
                }
                let after = elems.len() - r - 1;
                let tail_start = ts.len() - after;
                for (e, t) in elems[..r].iter().zip(&ts[..r]) {
                    self.bind_target(e, t, is_decl, span);
                }
                let rest_ty = Ty::Tuple(ts[r..tail_start].to_vec());
                if let BindTarget::Rest(inner, _) = &elems[r] {
                    self.bind_target(inner, &rest_ty, is_decl, span);
                }
                for (e, t) in elems[r + 1..].iter().zip(&ts[tail_start..]) {
                    self.bind_target(e, t, is_decl, span);
                }
            }
        }
    }

    // --- expression inference ---

    /// Infer the type of `e`, recording `(span, ty)` in analysis mode. The
    /// inference logic lives in [`infer_inner`]; this thin wrapper is the single
    /// place that retains the per-expression type for `type_at` (I#228). On the
    /// normal compile path (`analysis == None`) it is just a tail call.
    fn infer(&mut self, e: &Expr) -> Ty {
        let ty = self.infer_inner(e);
        self.check_float_container_value(&ty, expr_span(e));
        if let Some(facts) = &mut self.analysis {
            facts.expr_types.push((expr_span(e), ty.clone()));
        }
        ty
    }

    /// Type-check an f-string literal (D-0026). Each `{expr[!conv][:spec]}`
    /// field is split out, its expression type-checked like any other, and its
    /// conversion/format-spec validated against that type. The literal itself
    /// is a `str`. Errors inside a field land at the field's source location
    /// (best-effort span inside the literal).
    fn check_fstring(&mut self, value: &str, span: Span) -> Ty {
        // The literal's content begins after the one-char `f` prefix and the
        // opening quote; triple/raw-prefixed f-strings shift this by a little,
        // which only nudges field diagnostics within the literal.
        let base = span.start.saturating_add(2);
        let (parts, errors) = fstring::split_fstring(value, base);
        for e in errors {
            self.error(e.code, e.message, e.span);
        }
        for part in &parts {
            if let FStringPart::Field {
                expr,
                conv,
                spec,
                field_span,
                ..
            } = part
            {
                let ty = self.infer(expr);
                self.check_renderable(&ty, *field_span);
                if matches!(conv, Some('r') | Some('a')) {
                    self.error(
                        ErrorCode::ZAP4026,
                        "the '!r'/'!a' conversions require the Display protocol (#274); \
                         only '!s' is supported in v1"
                            .to_string(),
                        *field_span,
                    );
                }
                if let Some(fs) = spec {
                    self.check_spec_type(fs, &ty, *field_span);
                }
            }
        }
        Ty::Str
    }

    /// A value is renderable to text (via `str()` / an f-string field) when it
    /// is a scalar, char, bool or string; an `Optional[T]` (rendered as its
    /// payload when present, `None` when absent — #498, matching Python's
    /// `f"{d.get(k)}"`); or — since the general Display protocol landed (#274,
    /// D-0124) — a **user struct/enum** (`Named`), a **tuple**, or a
    /// **`list`/`dict`/`set`**, which the lowerer renders via
    /// `__str__`/`__repr__`/`@derive(Repr)`, the tier-3-enum structural form
    /// (#706), or the default field-wise / Python container form. An interface
    /// value (also `Named`) renders by dynamic dispatch to its concrete type's
    /// renderer (#707).
    fn check_renderable(&mut self, ty: &Ty, span: Span) {
        // `Optional[T]` is the prelude generic enum (I#154); sema tracks it
        // opaquely as `Named("Optional")` without its payload type, so the
        // payload's own renderability is enforced later by the lowerer.
        if let Ty::Named(n) = ty {
            if n == "Optional" || n.starts_with("Optional$") {
                return;
            }
        }
        if !matches!(
            ty,
            Ty::Str
                | Ty::Char
                | Ty::Bool
                | Ty::Unknown
                | Ty::Py
                | Ty::Named(_)
                | Ty::Tuple(_)
                | Ty::List(_)
                | Ty::Dict(_, _)
                | Ty::Set(_)
        ) && !ty.is_numeric()
        {
            self.error(
                ErrorCode::ZAP4024,
                format!(
                    "cannot render {} in an f-string: scalars, char, bool, str, and \
                     user struct/enum/tuple and list/dict/set values are supported",
                    ty.display()
                ),
                span,
            );
        }
    }

    /// Validate a format spec's presentation type against the value type.
    fn check_spec_type(&mut self, spec: &FormatSpec, ty: &Ty, span: Span) {
        let Some(t) = spec.ty else { return };
        if matches!(ty, Ty::Unknown | Ty::Py) {
            return;
        }
        let ok = if FormatSpec::is_int_type(t) {
            // Integer presentation accepts integers, chars and bools.
            ty.is_int() || matches!(ty, Ty::Char | Ty::Bool)
        } else if FormatSpec::is_float_type(t) {
            ty.is_numeric() && !matches!(ty, Ty::Complex)
        } else {
            // 's' — strings only.
            matches!(ty, Ty::Str)
        };
        if !ok {
            self.error(
                ErrorCode::ZAP4026,
                format!(
                    "format type '{t}' is not valid for a value of type {}",
                    ty.display()
                ),
                span,
            );
        }
    }

    fn infer_inner(&mut self, e: &Expr) -> Ty {
        match e {
            Expr::Int(s, _) => {
                if parse_int_lit(s).is_some() {
                    Ty::UntypedInt
                } else {
                    Ty::Bigint
                }
            }
            Expr::Float(..) => Ty::UntypedFloat,
            Expr::Imaginary(..) => Ty::Complex,
            Expr::Str {
                bytes,
                fstring,
                value,
                span,
                ..
            } => {
                if *fstring {
                    return self.check_fstring(value, *span);
                }
                if *bytes {
                    Ty::Bytes
                } else {
                    Ty::Str
                }
            }
            Expr::Bool(..) => Ty::Bool,
            Expr::None(_) => Ty::Unknown,
            Expr::Name(n, span) => {
                if let Some(t) = self.lookup(n) {
                    // Record this use → its local definition site (I#228). Global
                    // resolutions (functions/types) are AST-derivable, so the
                    // analysis crate reconstructs those rather than us recording
                    // them here.
                    if self.analysis.is_some() {
                        if let Some(def) = self.lookup_def(n) {
                            if let Some(facts) = &mut self.analysis {
                                facts.resolutions.push((*span, def));
                            }
                        }
                    }
                    // Sharing lint: reading a shared mutable global (#664/D-0113).
                    self.note_share_access(n, *span);
                    t
                } else if self.sigs.contains_key(n) || BUILTINS.contains(&n.as_str()) {
                    Ty::Unknown // a function used as a value
                } else {
                    self.error(ErrorCode::ZAP3001, format!("unknown name '{n}'"), *span);
                    Ty::Unknown
                }
            }
            Expr::Named {
                name, value, span, ..
            } => {
                // Walrus `(name := value)` (#600, epic #614 D8): infer the
                // operand, bind `name` by inference (like `auto`) into the
                // enclosing scope, and evaluate to the operand's type.
                let vt = self.infer(value);
                let t = default_type(vt);
                self.declare_walrus(name, t.clone(), *span);
                t
            }
            Expr::Starred { value, span } => {
                // A `*expr` is only meaningful as a direct element of a
                // list/set/tuple display, which consume it themselves (#598).
                // Reaching the generic dispatch means it appeared elsewhere.
                self.infer(value);
                self.error(
                    ErrorCode::ZAP5123,
                    "`*expr` may appear only as an element of a list, set, or \
                     tuple display (#598)"
                        .to_string(),
                    *span,
                );
                Ty::Unknown
            }
            Expr::Tuple(elems, _) => {
                // A tuple literal's type is the (defaulted) element types in
                // positional order (D-0049); `()` is the empty tuple. A `*t`
                // splat (#598) expands a tuple operand's element types in place;
                // only a statically-sized tuple can be spliced into a tuple.
                let mut tys = Vec::new();
                for el in elems {
                    match el {
                        Expr::Starred { value, span } => {
                            let t = self.infer(value);
                            match t {
                                Ty::Tuple(inner) => {
                                    tys.extend(inner.into_iter().map(default_type));
                                }
                                Ty::Unknown => {}
                                other => self.error(
                                    ErrorCode::ZAP5123,
                                    format!(
                                        "`*` in a tuple display requires a tuple \
                                         (statically sized), found '{}' (#598)",
                                        other.display()
                                    ),
                                    *span,
                                ),
                            }
                        }
                        _ => tys.push(default_type(self.infer(el))),
                    }
                }
                Ty::Tuple(tys)
            }
            Expr::List(elems, _) => {
                let elem = self.infer_display_elems(elems);
                let ty = Ty::List(Box::new(elem));
                self.check_numeric_container_value(&ty, expr_span(e));
                ty
            }
            Expr::Dict(items, span) => {
                let (kt, vt) = self.infer_dict_items(items);
                self.warn_struct_key(&kt, *span);
                let ty = Ty::Dict(Box::new(kt), Box::new(vt));
                self.check_numeric_container_value(&ty, *span);
                ty
            }
            Expr::ListComp { elem, clauses, .. } => {
                self.push_comp_scope();
                self.check_comp_clauses(clauses);
                let et = self.infer(elem);
                self.pop_comp_scope();
                let ty = Ty::List(Box::new(et));
                self.check_numeric_container_value(&ty, expr_span(e));
                ty
            }
            Expr::DictComp {
                key,
                value,
                clauses,
                span,
            } => {
                self.push_comp_scope();
                self.check_comp_clauses(clauses);
                let kt = self.infer(key);
                let vt = self.infer(value);
                self.pop_comp_scope();
                let ty = Ty::Dict(Box::new(kt), Box::new(vt));
                self.check_numeric_container_value(&ty, *span);
                ty
            }
            Expr::Set(elems, span) => {
                let elem = self.infer_display_elems(elems);
                self.warn_struct_key(&elem, *span);
                let ty = Ty::Set(Box::new(elem));
                self.check_numeric_container_value(&ty, *span);
                ty
            }
            Expr::SetComp {
                elem,
                clauses,
                span,
            } => {
                self.push_comp_scope();
                self.check_comp_clauses(clauses);
                let et = self.infer(elem);
                self.pop_comp_scope();
                let ty = Ty::Set(Box::new(et));
                self.check_numeric_container_value(&ty, *span);
                ty
            }
            // `(elem for CLAUSES)` — a bare generator expression (#596, epic
            // #614 D4): evaluates to a lazy `Iterator` rather than a materialized
            // collection. The loop variables live only in the comprehension
            // scope (they do not leak to the enclosing scope, matching
            // comprehensions and Python), while a walrus in a filter still binds
            // the enclosing scope. The element expression is type-checked for its
            // own errors, but the result type is the shared permissive genexpr
            // iterator type — the precise element type is computed by lowering.
            Expr::GenExp { elem, clauses, .. } => {
                self.push_comp_scope();
                self.check_comp_clauses(clauses);
                let _ = self.infer(elem);
                self.pop_comp_scope();
                Ty::Named(GENEXPR_ITER_TYPE.to_string())
            }
            Expr::Unary { op, expr, span } => {
                let t = self.infer(expr);
                if let Some(ty) = self.check_struct_unary(*op, &t, *span) {
                    return ty;
                }
                match op {
                    UnOp::Not => Ty::Bool,
                    UnOp::Neg | UnOp::Pos => {
                        if t.is_numeric() || matches!(t, Ty::Unknown) {
                            t
                        } else {
                            self.error(
                                ErrorCode::ZAP4010,
                                format!("unary operator needs a number, found {}", t.display()),
                                *span,
                            );
                            Ty::Unknown
                        }
                    }
                    UnOp::BitNot => {
                        if (t.is_int() && !matches!(t, Ty::Bigint)) || matches!(t, Ty::Unknown) {
                            t
                        } else {
                            self.error(
                                ErrorCode::ZAP4011,
                                format!("'~' needs an integer, found {}", t.display()),
                                *span,
                            );
                            Ty::Unknown
                        }
                    }
                }
            }
            Expr::Binary { op, lhs, rhs, span } => {
                let l = self.infer(lhs);
                let r = self.infer(rhs);
                // `is` / `is not` (#611, D18) are not operator-overloadable and
                // follow their own operand rules (reference identity, or a
                // `None` check), so they are checked before the struct/built-in
                // dispatch below.
                if matches!(op, BinOp::Is | BinOp::IsNot) {
                    return self.check_identity(
                        &l,
                        &r,
                        matches!(lhs.as_ref(), Expr::None(_)),
                        matches!(rhs.as_ref(), Expr::None(_)),
                        *span,
                    );
                }
                // `x == None` / `x != None` (#374/B3): per SPEC §5 this is
                // consistent with `x is None` — a null/empty check on the other
                // operand. A never-None value type is rejected the same way
                // `is None` rejects it (ZAP4074), rather than passing sema and
                // then failing in lowering.
                if matches!(op, BinOp::Eq | BinOp::Ne) {
                    let l_none = matches!(lhs.as_ref(), Expr::None(_));
                    let r_none = matches!(rhs.as_ref(), Expr::None(_));
                    if l_none || r_none {
                        if l_none && r_none {
                            return Ty::Bool;
                        }
                        let other = if l_none { &r } else { &l };
                        if !self.is_nullable_ty(other) {
                            self.error(
                                ErrorCode::ZAP4074,
                                format!(
                                    "a value of type '{}' can never be None, so \
                                     `== None` / `!= None` is always constant; \
                                     compare by value or use a pattern match",
                                    other.display()
                                ),
                                *span,
                            );
                        }
                        return Ty::Bool;
                    }
                }
                // Operator overloading on user structs (D-0047) takes precedence
                // over the built-in-type rules.
                if let Some(ty) = self.check_struct_binop(*op, &l, &r, *span) {
                    ty
                } else {
                    self.arith_result(*op, &l, &r, *span)
                }
            }
            Expr::Compare { first, rest, span } => {
                // A chained comparison type-checks like the sequence of pairwise
                // comparisons it desugars to (I#386). Each operand is inferred
                // exactly once (so names resolve and errors report once), then
                // every adjacent pair is checked as the corresponding `Binary`
                // comparison would be. The whole expression is a strict `bool`.
                let mut tys = Vec::with_capacity(rest.len() + 1);
                tys.push(self.infer(first));
                for (_, operand) in rest {
                    tys.push(self.infer(operand));
                }
                for (i, (op, _)) in rest.iter().enumerate() {
                    let l = &tys[i];
                    let r = &tys[i + 1];
                    // `is` / `is not` links (#611, D18) use the identity operand
                    // rules; a literal `None` operand is recognized from the
                    // expression so the None-check path applies in a chain too.
                    if matches!(op, BinOp::Is | BinOp::IsNot) {
                        let lhs_expr = if i == 0 {
                            first.as_ref()
                        } else {
                            &rest[i - 1].1
                        };
                        self.check_identity(
                            l,
                            r,
                            matches!(lhs_expr, Expr::None(_)),
                            matches!(&rest[i].1, Expr::None(_)),
                            *span,
                        );
                    } else if self.check_struct_binop(*op, l, r, *span).is_none() {
                        self.arith_result(*op, l, r, *span);
                    }
                }
                // A comparison against a loose (array) operand is itself loose
                // (#510): `a > 3` on an `array` is a boolean *mask*, not a scalar
                // bool, so it must stay `Unknown` to combine with `& | ~` and to
                // index. Concrete scalar comparisons remain a strict `bool`.
                if tys.iter().any(|t| matches!(t, Ty::Unknown)) {
                    Ty::Unknown
                } else {
                    Ty::Bool
                }
            }
            Expr::Call { callee, args, span } => self.check_call(callee, args, *span),
            Expr::Index { base, index, span } => {
                let b = self.infer(base);
                // Tuple element access `t[k]` requires a compile-time-constant
                // `k` and yields that position's static type (D-0049) — the key
                // difference from `list[i]`, which takes a runtime index.
                if let Ty::Tuple(elems) = &b {
                    match const_index(index) {
                        Some(k) if (k as usize) < elems.len() => {
                            return elems[k as usize].clone();
                        }
                        Some(k) => {
                            self.error(
                                ErrorCode::ZAP4050,
                                format!(
                                    "tuple index {k} out of range for a {}-element tuple",
                                    elems.len()
                                ),
                                *span,
                            );
                            return Ty::Unknown;
                        }
                        None => {
                            self.infer(index);
                            self.error(
                                ErrorCode::ZAP4051,
                                "a tuple index must be a compile-time-constant \
                                 integer (a tuple is heterogeneous, so the element \
                                 type must be known statically)"
                                    .to_string(),
                                *span,
                            );
                            return Ty::Unknown;
                        }
                    }
                }
                self.infer(index);
                // Indexing overloading (D-0047): `a[i]` -> `a.__getitem__(i)`.
                if self.struct_info(&b).is_some() {
                    return self.op_result(&b, "__getitem__", "Index", "[]", *span);
                }
                match b {
                    Ty::List(t) => *t,
                    Ty::Dict(_, v) => *v,
                    Ty::Str => Ty::Char,
                    // `bytes` indexing yields the byte value as an `int64`
                    // (0..=255), not a char (I#337).
                    Ty::Bytes => Ty::Int64,
                    Ty::Range => Ty::Int64,
                    _ => Ty::Unknown,
                }
            }
            Expr::Slice {
                base, lo, hi, step, ..
            } => {
                let b = self.infer(base);
                for part in [lo, hi, step].into_iter().flatten() {
                    self.infer(part);
                }
                match b {
                    // A `list[T]` slice is a `list[T]` copy (D-0030). An
                    // `array[T]` slice is a view of the same type, but arrays are
                    // typed loosely (ticket #24), so the base is already
                    // `Unknown` and falls through below.
                    Ty::List(_) => b,
                    Ty::Str => Ty::Str,
                    // A `bytes` slice is `bytes` (byte-based, I#337).
                    Ty::Bytes => Ty::Bytes,
                    _ => Ty::Unknown,
                }
            }
            // A baseless slice spec only appears inside a multi-axis array
            // subscript (`m[:, j]`, #502), which types loosely; infer its bounds
            // for side effects and leave the spec itself `Unknown`.
            Expr::SliceSpec { lo, hi, step, .. } => {
                for part in [lo, hi, step].into_iter().flatten() {
                    self.infer(part);
                }
                Ty::Unknown
            }
            Expr::Attr { base, name, span } => {
                // Qualified enum member/variant access: `Color.RED`,
                // `Shape.Unit` (D-0018/D-0046). The base is a *type* name, not a
                // value, so it is matched before inferring the base as a value.
                if let Expr::Name(tyname, _) = base.as_ref() {
                    if let Some(info) = self.enums.get(tyname).cloned() {
                        match info.variant(name) {
                            Some(v) if v.fields.is_empty() => {
                                return Ty::Named(tyname.clone());
                            }
                            Some(_) => {
                                self.error(
                                    ErrorCode::ZAP4061,
                                    format!(
                                        "variant '{name}' of enum '{tyname}' carries data; \
                                         construct it with '{tyname}.{name}(...)'"
                                    ),
                                    *span,
                                );
                                return Ty::Named(tyname.clone());
                            }
                            None => {
                                self.error(
                                    ErrorCode::ZAP4060,
                                    format!("enum '{tyname}' has no variant '{name}'"),
                                    *span,
                                );
                                return Ty::Unknown;
                            }
                        }
                    }
                    // Built-in module constant `math.pi`-style access (ticket
                    // #98, D-0056/D-0058): recognized only when the name is not
                    // shadowed by a local value, and intercepted before the base
                    // is resolved as a value (module names are not bindings).
                    if self.lookup(tyname).is_none() {
                        if let Some(ty) = modules::check_builtin_module_const(tyname, name) {
                            return ty;
                        }
                    }
                }
                let b = self.infer(base);
                // `.name` on an enum value yields its member name (D-0018).
                if matches!(&b, Ty::Named(n) if self.enums.contains_key(n)) && name == "name" {
                    return Ty::Str;
                }
                // Attribute access on a Python handle yields another Python
                // handle (D-0012); everything else is modeled loosely for now.
                if matches!(b, Ty::Py) {
                    Ty::Py
                } else {
                    Ty::Unknown
                }
            }
            Expr::Lambda { .. } => Ty::Unknown,
            Expr::RegexMatch {
                subject,
                negated,
                span,
                ..
            } => {
                let t = self.infer(subject);
                if !assignable(&t, &Ty::Str) && !assignable(&t, &Ty::Bytes) {
                    self.error(
                        ErrorCode::ZAP4040,
                        format!(
                            "the left operand of '=~'/'!~' must be a str, found {}",
                            t.display()
                        ),
                        *span,
                    );
                }
                // `!~` is a boolean test; `=~ m/.../` yields a match object
                // (truthy on success, `None` on no match — D-0010).
                if *negated {
                    Ty::Bool
                } else {
                    Ty::Match
                }
            }
            Expr::RegexSubst { subject, span, .. } => {
                let t = self.infer(subject);
                if !assignable(&t, &Ty::Str) && !assignable(&t, &Ty::Bytes) {
                    self.error(ErrorCode::ZAP4041,
                        format!(
                            "the left operand of a substitution 's/.../.../' must be a str, found {}",
                            t.display()
                        ),
                        *span,
                    );
                }
                // `s///` returns a new string; the subject is never mutated.
                Ty::Str
            }
            Expr::ChanRecv { chan, span } => {
                let ct = self.infer(chan);
                match ct {
                    Ty::Chan(elem) => *elem,
                    Ty::Unknown => Ty::Unknown,
                    other => {
                        self.error(
                            ErrorCode::ZAP4030,
                            format!(
                                "the operand of '<-' (receive) must be a channel, found {}",
                                other.display()
                            ),
                            *span,
                        );
                        Ty::Unknown
                    }
                }
            }
            Expr::Yield { value, span } => {
                // `yield` is only meaningful inside a generator; detection makes
                // any free `def` containing it a generator, so an out-of-context
                // `yield` can only occur where the function itself was already
                // rejected (a method/`main`). v1 requires an operand so the
                // element type `T` is well-defined.
                match value {
                    Some(v) => {
                        self.infer(v);
                    }
                    None => self.error(
                        ErrorCode::ZAP5064,
                        "'yield' requires a value in this version (I#67 v1)".to_string(),
                        *span,
                    ),
                }
                Ty::Unknown
            }
            Expr::IfElse {
                cond, then, els, ..
            } => {
                self.infer(cond);
                let t = self.infer(then);
                let e = self.infer(els);
                join(&t, &e)
            }
        }
    }

    fn arith_result(&mut self, op: BinOp, l: &Ty, r: &Ty, span: Span) -> Ty {
        use BinOp::*;
        let numeric = (l.is_numeric() || matches!(l, Ty::Unknown))
            && (r.is_numeric() || matches!(r, Ty::Unknown));
        // Reject mixing a concrete unsigned operand with a concrete *signed*
        // (or other-signed: bigint/float/complex) operand in one numeric,
        // comparison, or bitwise operation (#552, D-0019). Zap does no implicit
        // signed<->unsigned conversion. An untyped-int literal or `Unknown`
        // partners either sign (the literal adapts), and two unsigned operands
        // are fine. `is`/`is not`/`in`/`and`/`or`/`<-` are not sign-sensitive.
        if matches!(
            op,
            Add | Sub
                | Mul
                | Pow
                | Div
                | FloorDiv
                | Mod
                | BitAnd
                | BitOr
                | BitXor
                | Shl
                | Shr
                | Eq
                | Ne
                | Lt
                | Le
                | Gt
                | Ge
        ) {
            let mix = |a: &Ty, b: &Ty| {
                a.is_unsigned()
                    && b.is_numeric()
                    && !b.is_unsigned()
                    && !matches!(b, Ty::UntypedInt | Ty::Unknown)
            };
            if mix(l, r) || mix(r, l) {
                self.error(
                    ErrorCode::ZAP4015,
                    format!(
                        "cannot apply '{}' to a signed and an unsigned operand ({} and {}); \
                         convert one side explicitly (e.g. uint64(x) or int64(x))",
                        binop_symbol(op),
                        l.display(),
                        r.display()
                    ),
                    span,
                );
                return match op {
                    Eq | Ne | Lt | Le | Gt | Ge => Ty::Bool,
                    _ => Ty::Unknown,
                };
            }
        }
        match op {
            // `@` matrix multiply (#509, PEP 465) is defined only for arrays,
            // which type loosely (Unknown) here; a user struct's `__matmul__` is
            // dispatched through the operator-dunder machinery before
            // arith_result is reached. Any concrete non-array operand (a scalar,
            // str, list, dict) is a "no such operator" type error.
            MatMul => {
                if matches!(l, Ty::Unknown) || matches!(r, Ty::Unknown) {
                    Ty::Unknown
                } else {
                    self.error(
                        ErrorCode::ZAP4012,
                        format!(
                            "operator '@' (matrix multiply) is only defined for arrays, \
                             found {} and {}",
                            l.display(),
                            r.display()
                        ),
                        span,
                    );
                    Ty::Unknown
                }
            }
            Add => {
                if l.is_numeric() && r.is_numeric() {
                    num_join(l, r)
                } else if matches!(l, Ty::Unknown) || matches!(r, Ty::Unknown) {
                    Ty::Unknown
                } else if matches!(l, Ty::Str) && matches!(r, Ty::Str) {
                    Ty::Str
                } else if matches!(l, Ty::Bytes) && matches!(r, Ty::Bytes) {
                    // `bytes + bytes` concatenates (I#337).
                    Ty::Bytes
                } else if matches!(l, Ty::List(_)) && matches!(r, Ty::List(_)) {
                    l.clone()
                } else {
                    self.error(
                        ErrorCode::ZAP4012,
                        format!("cannot add {} and {}", l.display(), r.display()),
                        span,
                    );
                    Ty::Unknown
                }
            }
            Sub | Mul | Pow => {
                if numeric {
                    num_join(l, r)
                } else {
                    self.error(
                        ErrorCode::ZAP4012,
                        format!(
                            "arithmetic needs numbers, found {} and {}",
                            l.display(),
                            r.display()
                        ),
                        span,
                    );
                    Ty::Unknown
                }
            }
            FloorDiv | Mod => {
                if numeric && !matches!(l, Ty::Complex) && !matches!(r, Ty::Complex) {
                    num_join(l, r)
                } else {
                    self.error(
                        ErrorCode::ZAP4012,
                        format!(
                            "operator '{}' is not supported for complex values",
                            binop_symbol(op)
                        ),
                        span,
                    );
                    Ty::Unknown
                }
            }
            Div => {
                if numeric {
                    if matches!(l, Ty::Complex) || matches!(r, Ty::Complex) {
                        Ty::Complex
                    } else {
                        Ty::Float64
                    }
                } else {
                    self.error(
                        ErrorCode::ZAP4013,
                        format!(
                            "division needs numbers, found {} and {}",
                            l.display(),
                            r.display()
                        ),
                        span,
                    );
                    Ty::Unknown
                }
            }
            BitAnd | BitOr | BitXor | Shl | Shr => {
                if ((l.is_int() && !matches!(l, Ty::Bigint)) || matches!(l, Ty::Unknown))
                    && ((r.is_int() && !matches!(r, Ty::Bigint)) || matches!(r, Ty::Unknown))
                {
                    // Preserve unsignedness so `>>` lowers to a *logical* shift
                    // and the result keeps its unsigned type (#552); a bitwise op
                    // on any unsigned operand promotes to `uint64` just as the
                    // signed path promotes to `int64`.
                    if l.is_unsigned() || r.is_unsigned() {
                        Ty::Uint64
                    } else {
                        Ty::Int64
                    }
                } else {
                    self.error(
                        ErrorCode::ZAP4014,
                        format!(
                            "bitwise operator needs integers, found {} and {}",
                            l.display(),
                            r.display()
                        ),
                        span,
                    );
                    Ty::Unknown
                }
            }
            // A comparison against a loose (array) operand is a boolean *mask*,
            // itself loose (#510), so it stays `Unknown` to combine with
            // `& | ~` and to index; a concrete scalar comparison is `bool`.
            Eq | Ne => {
                if matches!(l, Ty::Unknown) || matches!(r, Ty::Unknown) {
                    Ty::Unknown
                } else {
                    Ty::Bool
                }
            }
            Lt | Le | Gt | Ge => {
                if matches!(l, Ty::Complex) || matches!(r, Ty::Complex) {
                    self.error(
                        ErrorCode::ZAP4012,
                        "complex values do not support ordering comparisons".to_string(),
                        span,
                    );
                }
                if matches!(l, Ty::Unknown) || matches!(r, Ty::Unknown) {
                    Ty::Unknown
                } else {
                    Ty::Bool
                }
            }
            In | NotIn => Ty::Bool,
            // `is` / `is not` (#611, D18): the operand rules are enforced by
            // `check_identity` (which has the operand expressions, so it can see
            // a literal `None`); reaching here the result is simply `bool`.
            Is | IsNot => Ty::Bool,
            And | Or => Ty::Bool,
            // Channel send `ch <- v` (ticket #25, D-0031): the left operand must
            // be a channel and the value must be assignable to its element type.
            Send => {
                match l {
                    Ty::Chan(elem) => {
                        if !assignable(r, elem) {
                            self.error(
                                ErrorCode::ZAP4032,
                                format!(
                                    "cannot send a value of type {} on a channel[{}]",
                                    r.display(),
                                    elem.display()
                                ),
                                span,
                            );
                        }
                    }
                    Ty::Unknown => {}
                    other => self.error(
                        ErrorCode::ZAP4031,
                        format!(
                            "the left operand of '<-' (send) must be a channel, found {}",
                            other.display()
                        ),
                        span,
                    ),
                }
                Ty::Unit
            }
        }
    }

    // --- operator overloading (D-0047) ---

    /// The struct type behind a `Ty`, if it is a user struct (not an enum, which
    /// is also `Ty::Named`).
    fn struct_info(&self, t: &Ty) -> Option<&StructInfo> {
        if let Ty::Named(n) = t {
            if !self.enums.contains_key(n) {
                return self.structs.get(n);
            }
        }
        None
    }

    /// The element type produced by iterating a value of type `it`, following the
    /// iteration protocol (D-0048), or `None` if `it` is not iterable.
    ///
    /// Built-ins (`range`/list/set/str) keep their known element types. A user
    /// struct is iterable when it provides `__iter__` (the `Iterable` interface);
    /// its return names the iterator, whose `__next__` (`Iterator`) gives the
    /// element type. A struct that itself has `__next__` is treated as its own
    /// iterator. The element type comes from `__next__`'s return signature: the
    /// comma-ok form `-> (T, bool)` and the Python/`StopIteration` form `-> T`
    /// both yield `T`. `Unknown`/`Py` stay permissive.
    fn iterable_elem(&self, it: &Ty) -> Option<Ty> {
        match it {
            Ty::Range => Some(Ty::Int64),
            Ty::List(t) | Ty::Set(t) | Ty::Heap(t) => Some((**t).clone()),
            // Iterating a dict yields its keys (Python semantics, ticket #299);
            // `for k, v` uses `d.items()`. Order is unspecified (hash order).
            Ty::Dict(k, _) => Some((**k).clone()),
            Ty::Str => Some(Ty::Char),
            Ty::Unknown | Ty::Py => Some(Ty::Unknown),
            Ty::Named(_) => {
                let info = self.struct_info(it)?;
                let iter_ty = if let Some(sig) = info.methods.get("__iter__") {
                    sig.ret.clone()
                } else if info.methods.contains_key("__next__") {
                    it.clone()
                } else {
                    return None;
                };
                let next = self.struct_info(&iter_ty)?.methods.get("__next__")?;
                Some(next_elem(&next.ret))
            }
            _ => None,
        }
    }

    /// The element type of `for`/comprehension iteration over `it`, emitting a
    /// compile error (D-0048) when `it` is not iterable. Returns [`Ty::Unknown`]
    /// on error so checking continues.
    /// Infer the element type of a list/set display whose elements may include
    /// `*expr` splats (#598). A splat contributes the spliced iterable's element
    /// type; a plain element contributes its own type. Element types are joined,
    /// and a splat whose element type cannot agree with the display's is a clear
    /// error (type-first).
    fn infer_display_elems(&mut self, elems: &[Expr]) -> Ty {
        let mut elem = Ty::Unknown;
        let mut first = true;
        for el in elems {
            let t = match el {
                Expr::Starred { value, span } => {
                    let it = self.infer(value);
                    self.iter_element(&it, *span)
                }
                _ => self.infer(el),
            };
            if first {
                elem = t;
                first = false;
            } else {
                let joined = join(&elem, &t);
                if matches!(joined, Ty::Unknown)
                    && !matches!(elem, Ty::Unknown)
                    && !matches!(t, Ty::Unknown)
                    && matches!(el, Expr::Starred { .. })
                {
                    self.error(
                        ErrorCode::ZAP5123,
                        format!(
                            "spliced elements have type '{}', which does not \
                             match the display's element type '{}' (#598)",
                            t.display(),
                            elem.display()
                        ),
                        expr_span(el),
                    );
                }
                elem = joined;
            }
        }
        elem
    }

    /// Infer the key/value types of a dict display whose entries may include
    /// `**expr` merges (#598). A merge contributes the spliced dict's key/value
    /// types; a pair contributes its key/value types. A `**` operand that is not
    /// a dict, or whose entry types disagree with the display's, is a clear
    /// error (type-first).
    fn infer_dict_items(&mut self, items: &[DictItem]) -> (Ty, Ty) {
        let mut kt = Ty::Unknown;
        let mut vt = Ty::Unknown;
        let mut first = true;
        for it in items {
            let (k2, v2) = match it {
                DictItem::Pair(k, v) => (self.infer(k), self.infer(v)),
                DictItem::Spread(e) => {
                    let t = self.infer(e);
                    match &t {
                        Ty::Dict(dk, dv) => ((**dk).clone(), (**dv).clone()),
                        Ty::Unknown => (Ty::Unknown, Ty::Unknown),
                        other => {
                            self.error(
                                ErrorCode::ZAP5123,
                                format!(
                                    "`**` in a dict display requires a dict, \
                                     found '{}' (#598)",
                                    other.display()
                                ),
                                expr_span(e),
                            );
                            (Ty::Unknown, Ty::Unknown)
                        }
                    }
                }
            };
            if first {
                kt = k2;
                vt = v2;
                first = false;
            } else {
                if matches!(it, DictItem::Spread(_)) {
                    let jk = join(&kt, &k2);
                    let jv = join(&vt, &v2);
                    let bad = (matches!(jk, Ty::Unknown)
                        && !matches!(kt, Ty::Unknown)
                        && !matches!(k2, Ty::Unknown))
                        || (matches!(jv, Ty::Unknown)
                            && !matches!(vt, Ty::Unknown)
                            && !matches!(v2, Ty::Unknown));
                    if bad {
                        self.error(
                            ErrorCode::ZAP5123,
                            format!(
                                "merged entries have type 'dict[{}, {}]', which \
                                 does not match the display's 'dict[{}, {}]' (#598)",
                                k2.display(),
                                v2.display(),
                                kt.display(),
                                vt.display()
                            ),
                            expr_span(match it {
                                DictItem::Spread(e) => e,
                                DictItem::Pair(k, _) => k,
                            }),
                        );
                    }
                }
                kt = join(&kt, &k2);
                vt = join(&vt, &v2);
            }
        }
        (kt, vt)
    }

    /// Declare each comprehension clause's loop variable and infer its iterable
    /// and `if` filters, left to right, in the current (shared) scope (#599).
    /// Later clauses may refer to earlier clauses' variables. The caller pushes
    /// and pops the surrounding scope.
    fn check_comp_clauses(&mut self, clauses: &[CompClause]) {
        for c in clauses {
            let it = self.infer(&c.iter);
            let e_ty = self.iter_element(&it, c.span);
            self.declare(&c.var, e_ty, c.span);
            for f in &c.ifs {
                self.infer(f);
            }
        }
    }

    fn iter_element(&mut self, it: &Ty, span: Span) -> Ty {
        match self.iterable_elem(it) {
            Some(e) => e,
            None => {
                self.error(
                    ErrorCode::ZAP4070,
                    format!(
                        "type '{}' is not iterable: it must define '__iter__' \
                         (the 'Iterable' interface, D-0048)",
                        it.display()
                    ),
                    span,
                );
                Ty::Unknown
            }
        }
    }

    /// The signature of a struct's dunder, if defined or derived.
    fn lookup_dunder(&self, t: &Ty, dunder: &str) -> Option<FnSig> {
        self.struct_info(t)
            .and_then(|i| i.methods.get(dunder).cloned())
    }

    /// Require a struct to provide `dunder` for operator `sym` (interface
    /// `iface`), erroring with a clear message if it does not (D-0047).
    fn require_dunder(&mut self, t: &Ty, dunder: &str, iface: &str, sym: &str, span: Span) {
        if self.lookup_dunder(t, dunder).is_none() {
            self.error(
                ErrorCode::ZAP4071,
                format!(
                    "type '{}' does not support operator '{sym}': it must define \
                     '{dunder}' (the '{iface}' operator interface, D-0047)",
                    t.display()
                ),
                span,
            );
        }
    }

    /// The result type of an operator whose result is the dunder's return type
    /// (arithmetic / bitwise / unary / indexing). Errors if the dunder is missing.
    fn op_result(&mut self, t: &Ty, dunder: &str, iface: &str, sym: &str, span: Span) -> Ty {
        match self.lookup_dunder(t, dunder) {
            Some(sig) => sig.ret,
            None => {
                self.require_dunder(t, dunder, iface, sym, span);
                Ty::Unknown
            }
        }
    }

    /// Check a binary operator with a user-struct operand (D-0047), returning the
    /// result type, or `None` to fall back to the built-in-type rules.
    fn check_struct_binop(&mut self, op: BinOp, l: &Ty, r: &Ty, span: Span) -> Option<Ty> {
        use BinOp::*;
        // `x in a` dispatches on the container (right operand). A struct that
        // defines `__contains__` uses it; otherwise, if the container is iterable
        // (D-0048), membership falls back to iterating and comparing. Only a
        // struct that is neither is an error.
        if matches!(op, In) {
            if self.struct_info(r).is_some() {
                if self.lookup_dunder(r, "__contains__").is_none()
                    && self.iterable_elem(r).is_none()
                {
                    self.error(
                        ErrorCode::ZAP4072,
                        format!(
                            "type '{}' does not support operator 'in': define \
                             '__contains__' or make it iterable ('__iter__', D-0048)",
                            r.display()
                        ),
                        span,
                    );
                }
                return Some(Ty::Bool);
            }
            return None;
        }
        // Every other operator dispatches on the left operand.
        self.struct_info(l)?;
        match op {
            Eq | Ne => {
                self.require_dunder(l, "__eq__", "Eq", binop_symbol(op), span);
                Some(Ty::Bool)
            }
            Lt | Le | Gt | Ge => {
                // The four relational operators derive from a single `__cmp__`.
                self.require_dunder(l, "__cmp__", "Ord", binop_symbol(op), span);
                Some(Ty::Bool)
            }
            And | Or => Some(Ty::Bool),
            Send => None,
            _ => {
                let (dunder, iface) = binary_op_dunder(op)?;
                Some(self.op_result(l, dunder, iface, binop_symbol(op), span))
            }
        }
    }

    /// Check an augmented assignment `a OP= b` with a user-struct LHS (D-0047),
    /// returning the result type, or `None` to fall back to the built-in-type
    /// rules. If the LHS type defines the in-place dunder (`__iadd__` etc.), the
    /// assignment calls `a.__iOP__(b)` and is typed from that dunder's return
    /// type; otherwise it derives from the binary operator exactly like `a OP b`
    /// (so a struct with only `__add__` still supports `+=`).
    fn check_struct_aug_assign(&mut self, op: BinOp, l: &Ty, r: &Ty, span: Span) -> Option<Ty> {
        self.struct_info(l)?;
        if let Some(dunder) = inplace_op_dunder(op) {
            if let Some(sig) = self.lookup_dunder(l, dunder) {
                return Some(sig.ret);
            }
        }
        self.check_struct_binop(op, l, r, span)
    }

    /// Whether a type names the prelude `Optional[T]` enum (bare `Optional`
    /// before monomorphization, or a mangled `Optional$…` instance). `Optional`
    /// is tracked opaquely as a `Named` type, so this is a name test.
    fn is_optional_ty(t: &Ty) -> bool {
        matches!(t, Ty::Named(n) if n == "Optional" || n.starts_with("Optional$"))
    }

    /// Whether a value of type `t` can hold `None` — the operand rule for
    /// `x is None` / `x is not None` (#611, D18). The two nullable categories
    /// are a plain reference `struct` (D-0023; physically a pointer that may be
    /// null) and `Optional[T]` (whose `None` is the empty variant).
    /// `Unknown`/`Py` stay permissive. A value struct, an enum, and every
    /// scalar / `str` / `bytes` / tuple can never be None.
    fn is_nullable_ty(&self, t: &Ty) -> bool {
        match t {
            Ty::Unknown | Ty::Py => true,
            _ if Self::is_optional_ty(t) => true,
            Ty::Named(n) => self.structs.get(n).is_some_and(|s| !s.is_value),
            _ => false,
        }
    }

    /// Whether a value of type `t` has *reference identity* — the operand rule
    /// for `a is b` on two non-`None` operands (#611, D18). Reference types
    /// carry a distinct heap allocation: a plain reference `struct`, the GC
    /// collections (`list`/`dict`/`set`/`heap`), a `channel`, and `Optional[T]`
    /// (a pointer or tagged box). `Unknown`/`Py` stay permissive. Value types
    /// (scalars, `str`, `bytes`, `bool`, `char`, a `value struct`, an `enum`, a
    /// tuple, `range`) have value semantics and no distinct identity.
    fn is_identity_ty(&self, t: &Ty) -> bool {
        match t {
            Ty::Unknown | Ty::Py => true,
            Ty::List(_) | Ty::Dict(_, _) | Ty::Set(_) | Ty::Heap(_) | Ty::Chan(_) => true,
            Ty::AtomicArray(_) => true,
            _ if Self::is_optional_ty(t) => true,
            Ty::Named(n) => self.structs.get(n).is_some_and(|s| !s.is_value),
            _ => false,
        }
    }

    /// Type-check `a is b` / `a is not b` (#611, D18), always yielding `bool`.
    /// `l_none`/`r_none` say whether that operand is a literal `None`, so the
    /// None-check path applies even though `None` itself infers to `Unknown`.
    ///
    /// - `None is None`: trivially valid.
    /// - `x is None` / `None is x`: the common null/empty check — the other
    ///   operand must be nullable ([`is_nullable_ty`]), else [`ZAP4074`].
    /// - `a is b` (both non-`None`): reference identity — both operands must be
    ///   reference types ([`is_identity_ty`]), else [`ZAP4073`], which points at
    ///   `==`. Two *different* reference types are permitted (the result is
    ///   simply `false`), consistent with how `==` does not constrain operand
    ///   types.
    fn check_identity(&mut self, l: &Ty, r: &Ty, l_none: bool, r_none: bool, span: Span) -> Ty {
        if l_none && r_none {
            return Ty::Bool;
        }
        if l_none || r_none {
            let other = if l_none { r } else { l };
            if !self.is_nullable_ty(other) {
                self.error(
                    ErrorCode::ZAP4074,
                    format!(
                        "a value of type '{}' can never be None, so this `is` \
                         test is always constant; use `==` to compare by value, \
                         or a pattern match",
                        other.display()
                    ),
                    span,
                );
            }
            return Ty::Bool;
        }
        let offending = if !self.is_identity_ty(l) {
            Some(l)
        } else if !self.is_identity_ty(r) {
            Some(r)
        } else {
            None
        };
        if let Some(t) = offending {
            self.error(
                ErrorCode::ZAP4073,
                format!(
                    "`is` / `is not` test reference identity, but '{}' is a value \
                     type with no distinct identity; use `==` to compare by value",
                    t.display()
                ),
                span,
            );
        }
        Ty::Bool
    }

    /// Check a unary operator with a user-struct operand (D-0047).
    fn check_struct_unary(&mut self, op: UnOp, t: &Ty, span: Span) -> Option<Ty> {
        self.struct_info(t)?;
        Some(match op {
            UnOp::Not => Ty::Bool, // truthiness; `__bool__` is optional
            UnOp::Neg => self.op_result(t, "__neg__", "Neg", "-", span),
            UnOp::Pos => self.op_result(t, "__pos__", "Pos", "+", span),
            UnOp::BitNot => self.op_result(t, "__invert__", "Invert", "~", span),
        })
    }

    /// Warn when a struct that defines equality but not `__hash__` is used as a
    /// `dict`/`set` key — then `a == b ⟹ hash(a) == hash(b)` is not guaranteed
    /// (D-0047). `@derive(Hash)` or a hand-written `__hash__` silences it.
    /// Emit the use-site deprecation warning (ZAP5124) if `key` names a
    /// `@deprecated` function or method (I#472, D-0109). `key` is the free
    /// function's bare name or a method's `"Type.method"`.
    fn warn_deprecated_call(&mut self, key: &str, span: Span) {
        if let Some(msg) = self.deprecated.get(key).cloned() {
            let base = format!("call to deprecated '{key}'");
            let full = match msg {
                Some(m) if !m.is_empty() => format!("{base}: {m}"),
                _ => base,
            };
            self.warn(ErrorCode::ZAP5124, full, span);
        }
    }

    fn warn_struct_key(&mut self, key: &Ty, span: Span) {
        let (has_eq, has_hash) = match self.struct_info(key) {
            Some(i) => (i.has("__eq__"), i.has("__hash__")),
            None => return,
        };
        if has_eq && !has_hash {
            self.warn(
                ErrorCode::ZAP5120,
                format!(
                    "type '{}' defines equality but no '__hash__', yet is used as a \
                     dict/set key; add @derive(Hash) or define '__hash__' so that \
                     'a == b' implies 'hash(a) == hash(b)' (D-0047)",
                    key.display()
                ),
                span,
            );
        }
    }

    fn check_call(&mut self, callee: &Expr, args: &[Arg], span: Span) -> Ty {
        // Spread call arguments (`*expr` / `**expr`, #597) are supported in this
        // version only for a call to a named free function (the variadic /
        // `**kwargs` forwarding path). Any other callee — construction, a
        // built-in, a method, a Python handle — rejects them with one clear
        // error; the arguments are still inferred so later checks don't cascade.
        if args.iter().any(|a| a.spread != ArgSpread::None) {
            let is_free_fn = matches!(callee, Expr::Name(n, _) if self.sigs.contains_key(n));
            if !is_free_fn {
                self.error(
                    ErrorCode::ZAP5122,
                    "`*`/`**` spread arguments are supported only in a call to a named \
                     function in this version (#597)"
                        .to_string(),
                    span,
                );
                for a in args {
                    self.infer(&a.value);
                }
                return Ty::Unknown;
            }
        }
        // Qualified enum-variant construction: `Shape.Circle(r=2.0)` /
        // `Shape.Circle(2.0)` (D-0046), accepting named or positional args.
        if let Expr::Attr { base, name, .. } = callee {
            if let Expr::Name(tyname, _) = base.as_ref() {
                if let Some(info) = self.enums.get(tyname).cloned() {
                    // `Enum.from_json(s)` — the decode associated function an
                    // `@derive(Json)` (open) enum gets (ticket #346, D-0086).
                    // Intercepted before variant construction so `from_json` is
                    // not mistaken for a variant name.
                    if name == "from_json" && info.derives_json {
                        if args.len() != 1 {
                            self.error(
                                ErrorCode::ZAP4025,
                                format!(
                                    "{tyname}.from_json(s) takes 1 argument, found {}",
                                    args.len()
                                ),
                                span,
                            );
                        } else {
                            let t = self.infer(&args[0].value);
                            if !assignable(&t, &Ty::Str) && !assignable(&t, &Ty::Bytes) {
                                self.error(
                                    ErrorCode::ZAP4024,
                                    format!(
                                        "{tyname}.from_json() expects a str or bytes argument, found {}",
                                        t.display()
                                    ),
                                    expr_span(&args[0].value),
                                );
                            }
                        }
                        return Ty::Named(tyname.clone());
                    }
                    return self.check_variant_construction(tyname, name, &info, args, span);
                }
                // `Struct.from_json(s)` — the decode associated function a
                // `@derive(Json)` struct gets (D-0059, I#100). Recognized when
                // the struct derived the codec (its `to_<fmt>` method exists).
                let decode_encode = match name.as_str() {
                    "from_json" => Some("to_json"),
                    "from_csv" => Some("to_csv"),
                    "from_yaml" => Some("to_yaml"),
                    _ => None,
                };
                if let Some(enc) = decode_encode {
                    if self
                        .structs
                        .get(tyname)
                        .is_some_and(|i| i.methods.contains_key(enc))
                    {
                        if args.len() != 1 {
                            self.error(
                                ErrorCode::ZAP4025,
                                format!(
                                    "{tyname}.{name}(s) takes 1 argument, found {}",
                                    args.len()
                                ),
                                span,
                            );
                        } else {
                            let t = self.infer(&args[0].value);
                            if !assignable(&t, &Ty::Str) && !assignable(&t, &Ty::Bytes) {
                                self.error(ErrorCode::ZAP4024,
                                    format!(
                                        "{tyname}.{name}() expects a str or bytes argument, found {}",
                                        t.display()
                                    ),
                                    expr_span(&args[0].value),
                                );
                            }
                        }
                        return Ty::Named(tyname.clone());
                    }
                }
                // Built-in `signal`/`gc`/`runtime` module calls (ticket #77,
                // D-0058). Recognized only when the name is not shadowed by a
                // local value, and is intercepted before the base is resolved as
                // a value (these module names are not value bindings).
                if self.lookup(tyname).is_none() {
                    if let Some(ty) =
                        modules::check_builtin_module_call(self, tyname, name, args, span)
                    {
                        return ty;
                    }
                }
            }
        }
        // Array construction `array[dtype]([...])` (ticket #24): the callee is a
        // subscript whose base (`array`) and index (the dtype name) are not
        // ordinary value names, so they are not inferred as such. Typed loosely.
        if let Expr::Index { base, index, .. } = callee {
            if matches!(base.as_ref(), Expr::Name(n, _) if n == "array") {
                let valid_dtype = matches!(index.as_ref(), Expr::Name(n, _) if matches!(n.as_str(),
                    "float" | "f64" | "float64" | "f32" | "float32"
                    | "int" | "i64" | "int64" | "i32" | "int32" | "i16" | "int16" | "i8" | "int8"
                    | "uint8" | "u8" | "u16" | "uint16" | "u32" | "uint32" | "u64" | "uint64"
                    | "bool"));
                if !valid_dtype {
                    self.error(
                        ErrorCode::ZAP4028,
                        "unsupported array dtype; expected f32/f64, i8/i16/i32/i64, u8/u16/u32/u64, or bool".to_string(),
                        expr_span(index),
                    );
                }
                if args.len() != 1 || args.first().is_some_and(|a| a.name.is_some()) {
                    self.error(
                        ErrorCode::ZAP4028,
                        "array[dtype] requires one positional flat list literal".to_string(),
                        span,
                    );
                }
                for a in args {
                    if matches!(&a.value, Expr::List(..)) {
                        // Walk the (possibly nested) literal to its scalar leaves:
                        // a flat literal builds a 1-D array, a nested rectangular
                        // one an N-D array (#500, D-0164). Every leaf must be a
                        // numeric scalar; rectangularity is enforced in lowering.
                        let mut stack = vec![&a.value];
                        while let Some(e) = stack.pop() {
                            if let Expr::List(elements, _) = e {
                                for element in elements {
                                    stack.push(element);
                                }
                                continue;
                            }
                            let ty = self.infer(e);
                            if !matches!(
                                ty,
                                Ty::Unknown
                                    | Ty::Bool
                                    | Ty::Char
                                    | Ty::Int32
                                    | Ty::Int64
                                    | Ty::UntypedInt
                                    | Ty::Float32
                                    | Ty::Float64
                                    | Ty::UntypedFloat
                            ) {
                                self.error(ErrorCode::ZAP4028, "array elements must be numeric scalars in a (possibly nested) list literal".to_string(), expr_span(e));
                            }
                        }
                    } else {
                        self.infer(&a.value);
                        self.error(ErrorCode::ZAP4028, "array construction supports only a (possibly nested) list literal, not a list variable or iterable".to_string(), expr_span(&a.value));
                    }
                }
                return Ty::Unknown;
            }
            // Array constructors `zeros[T](shape)` / `ones` / `full[T](shape, v)` /
            // `arange[T](...)` / `linspace[T](...)` (#504, D-0167). Validate the
            // dtype name and type-check the arguments; the result is typed loosely
            // (`array` is Unknown in sema), with concrete shape/arity checks in
            // lowering.
            if matches!(base.as_ref(), Expr::Name(n, _)
                if matches!(n.as_str(), "zeros" | "ones" | "full" | "arange" | "linspace"))
            {
                let valid_dtype = matches!(index.as_ref(), Expr::Name(n, _) if matches!(n.as_str(),
                    "float" | "f64" | "float64" | "f32" | "float32"
                    | "int" | "i64" | "int64" | "i32" | "int32" | "i16" | "int16" | "i8" | "int8"
                    | "uint8" | "u8" | "u16" | "uint16" | "u32" | "uint32" | "u64" | "uint64"
                    | "bool"));
                if !valid_dtype {
                    self.error(
                        ErrorCode::ZAP4028,
                        "unsupported array dtype; expected f32/f64, i8/i16/i32/i64, u8/u16/u32/u64, or bool".to_string(),
                        expr_span(index),
                    );
                }
                for a in args {
                    self.infer(&a.value);
                }
                return Ty::Unknown;
            }
            // Channel construction `channel[T]()` / `channel[T](cap)` (ticket #25,
            // D-0031). The element type `T` appears as the subscript; an optional
            // single argument is the buffer capacity (an integer).
            if matches!(base.as_ref(), Expr::Name(n, _) if n == "channel") {
                let elem = self.chan_elem_from_expr(index);
                if args.len() > 1 {
                    self.error(
                        ErrorCode::ZAP4025,
                        "channel[T](...) takes at most one argument (the buffer capacity)"
                            .to_string(),
                        span,
                    );
                }
                for a in args {
                    let at = self.infer(&a.value);
                    if !at.is_int() && !matches!(at, Ty::Unknown) {
                        self.error(
                            ErrorCode::ZAP4033,
                            format!(
                                "channel capacity must be an integer, found {}",
                                at.display()
                            ),
                            expr_span(&a.value),
                        );
                    }
                }
                return Ty::Chan(Box::new(elem));
            }
            // AtomicArray construction `AtomicArray[T](cap)` (#844). The element
            // type `T` is the subscript; the sole argument is the fixed capacity
            // (a non-negative integer). All slots start None/0.
            if matches!(base.as_ref(), Expr::Name(n, _) if n == "AtomicArray") {
                let elem = self.chan_elem_from_expr(index);
                // v1 supports exactly reference elements (a pointer slot) and
                // `int64` (a word slot) — the map needs reference slots, a counter
                // needs int64. A value struct, float, bool/char, enum, int32,
                // bigint/complex, or tuple is not a pointer-width slot and is
                // rejected here. (`is_identity_ty` admits user *reference* structs,
                // the built-in collections, `Optional`, and `Py`.)
                let elem_ok =
                    matches!(elem, Ty::Int64 | Ty::Str | Ty::Bytes) || self.is_identity_ty(&elem);
                if !elem_ok {
                    self.error(
                        ErrorCode::ZAP4028,
                        format!(
                            "AtomicArray[T] supports reference types or int64, not {} \
                             (Experimental; other element kinds are a follow-up)",
                            elem.display()
                        ),
                        span,
                    );
                }
                if args.len() != 1 {
                    self.error(
                        ErrorCode::ZAP4025,
                        format!(
                            "AtomicArray[T](cap) takes exactly one argument (the \
                             capacity), got {}",
                            args.len()
                        ),
                        span,
                    );
                } else {
                    let at = self.infer(&args[0].value);
                    if !at.is_int() && !matches!(at, Ty::Unknown) {
                        self.error(
                            ErrorCode::ZAP4033,
                            format!(
                                "AtomicArray capacity must be an integer, found {}",
                                at.display()
                            ),
                            expr_span(&args[0].value),
                        );
                    }
                }
                return Ty::AtomicArray(Box::new(elem));
            }
            // Heap construction `heap[T]()` / `heap[T](cmp="max")` (D-0057). The
            // element type `T` is the subscript; the only accepted argument is the
            // keyword `cmp`, whose value is the string "min" (default) or "max"
            // selecting heap direction.
            if matches!(base.as_ref(), Expr::Name(n, _) if n == "heap") {
                let elem = self.chan_elem_from_expr(index);
                for a in args {
                    let at = self.infer(&a.value);
                    match a.name.as_deref() {
                        Some("cmp") => {
                            if let Expr::Str { value, .. } = &a.value {
                                if value != "min" && value != "max" {
                                    self.error(
                                        ErrorCode::ZAP5100,
                                        format!(
                                            "heap cmp must be \"min\" or \"max\", found {value:?}"
                                        ),
                                        expr_span(&a.value),
                                    );
                                }
                            } else if !matches!(at, Ty::Unknown) {
                                self.error(
                                    ErrorCode::ZAP5101,
                                    "heap cmp must be a string literal (\"min\" or \"max\")"
                                        .to_string(),
                                    expr_span(&a.value),
                                );
                            }
                        }
                        Some(other) => self.error(
                            ErrorCode::ZAP5102,
                            format!("heap[T](...) has no keyword argument '{other}' (only 'cmp')"),
                            expr_span(&a.value),
                        ),
                        None => self.error(
                            ErrorCode::ZAP5103,
                            "heap[T](...) takes only the keyword argument 'cmp' (e.g. cmp=\"max\")"
                                .to_string(),
                            expr_span(&a.value),
                        ),
                    }
                }
                return Ty::Heap(Box::new(elem));
            }
            // Set construction `set[T]()` / `set[T](iterable)` (D17, #609). The
            // element type `T` is the subscript; with no argument it builds an
            // empty `set[T]`, and with one positional iterable it builds a
            // `set[T]` of that iterable's (deduplicated) elements, mirroring
            // Python's `set(xs)`.
            if matches!(base.as_ref(), Expr::Name(n, _) if n == "set") {
                let elem = self.chan_elem_from_expr(index);
                match args.len() {
                    0 => {}
                    1 if args[0].name.is_none() => {
                        let at = self.infer(&args[0].value);
                        // Require an iterable operand (D-0048); the element type
                        // is checked against `T` at lowering (element-rep match).
                        self.iter_element(&at, expr_span(&args[0].value));
                    }
                    _ => self.error(
                        ErrorCode::ZAP4025,
                        "set[T](...) takes at most one positional iterable argument".to_string(),
                        span,
                    ),
                }
                self.warn_struct_key(&elem, span);
                let ty = Ty::Set(Box::new(elem));
                self.check_numeric_container_value(&ty, span);
                return ty;
            }
            // Call-site explicit type arguments on a generic callee (I#161,
            // Fork S): `f[T](...)`, `Type[T](...)`, or `recv.m[T](...)`. The
            // `index` here is type arguments, not a value subscript, so it is NOT
            // inferred as a value (its type names would otherwise be rejected as
            // unknown names). Value arguments are still type-checked loosely.
            let type_args_callee = match base.as_ref() {
                Expr::Name(n, _) => {
                    self.sigs.contains_key(n)
                        || self.user_types.contains(n)
                        || self.enums.contains_key(n)
                }
                // `recv.m[T](...)` — a generic method with explicit type args.
                Expr::Attr { .. } => true,
                _ => false,
            };
            if type_args_callee {
                if let Expr::Attr { base: recv, .. } = base.as_ref() {
                    self.infer(recv);
                }
                for a in args {
                    self.infer(&a.value);
                }
                return match base.as_ref() {
                    Expr::Name(n, _) => {
                        if let Some(sig) = self.sigs.get(n) {
                            sig.ret.clone()
                        } else if self.user_types.contains(n) || self.enums.contains_key(n) {
                            Ty::Named(n.clone())
                        } else {
                            Ty::Unknown
                        }
                    }
                    _ => Ty::Unknown,
                };
            }
        }
        if let Expr::Name(fname, _) = callee {
            match fname.as_str() {
                name if is_exception_type(name) => {
                    if args.len() != 1 || args[0].name.is_some() {
                        self.error(
                            ErrorCode::ZAP4025,
                            format!("{name}() takes one positional message"),
                            span,
                        );
                    }
                    for a in args {
                        let at = self.infer(&a.value);
                        if !matches!(at, Ty::Str | Ty::Unknown) {
                            self.error(
                                ErrorCode::ZAP4024,
                                format!("{name}() message must be str, found {}", at.display()),
                                expr_span(&a.value),
                            );
                        }
                    }
                    return Ty::Unknown;
                }
                "sum" => {
                    // Reduction over an array -> scalar; typed loosely for now.
                    for a in args {
                        self.infer(&a.value);
                    }
                    return Ty::Unknown;
                }
                "dot" => {
                    // `dot(a, b)` — matrix multiply / dot product (#509), the
                    // function form of `a @ b`. Arrays type loosely, so the
                    // result is loose; the lowering computes the real shape.
                    for a in args {
                        self.infer(&a.value);
                    }
                    return Ty::Unknown;
                }
                "concatenate" | "stack" => {
                    // Shape ops over a list of arrays (#511); typed loosely, the
                    // lowering derives the real rank from the list element type.
                    for a in args {
                        self.infer(&a.value);
                    }
                    return Ty::Unknown;
                }
                "where" => {
                    // where(cond, x, y) selects element-wise; where(cond) returns
                    // a tuple of index arrays (#510). Arrays type loosely, so the
                    // result is loose; the lowering derives the real shape/tuple.
                    for a in args {
                        self.infer(&a.value);
                    }
                    return Ty::Unknown;
                }
                // `sorted(xs, key=..., reverse=...)` -> a new list of the same
                // element type, leaving `xs` unchanged (ticket #336). The element
                // type is carried through so the result can be indexed/iterated
                // with precise types.
                "sorted" => {
                    let mut result = Ty::Unknown;
                    for a in args {
                        let at = self.infer(&a.value);
                        if a.name.is_none() {
                            if let Ty::List(_) = &at {
                                result = at.clone();
                            }
                        }
                    }
                    return result;
                }
                "print" => {
                    for a in args {
                        self.infer(&a.value);
                    }
                    return Ty::Unit;
                }
                // REPL session-binding slot intrinsics (I#159, design/0025-repl.md).
                // Synthesized only by the `zap repl` driver, never written by
                // users. A load yields `Unknown` so it coerces into the binding's
                // declared type; a store is a statement (`Unit`). `Unknown` is
                // universally assignable, so `<T> x = __repl_load_*(i)` checks.
                "__repl_load_i64" | "__repl_load_ptr" | "__repl_load_f64" => {
                    for a in args {
                        self.infer(&a.value);
                    }
                    return Ty::Unknown;
                }
                "__repl_store_i64" | "__repl_store_ptr" | "__repl_store_f64" => {
                    for a in args {
                        self.infer(&a.value);
                    }
                    return Ty::Unit;
                }
                "range" => {
                    for a in args {
                        let t = self.infer(&a.value);
                        if !t.is_int() && !matches!(t, Ty::Unknown) {
                            self.error(
                                ErrorCode::ZAP4024,
                                format!("range expects integers, found {}", t.display()),
                                expr_span(&a.value),
                            );
                        }
                    }
                    return Ty::Range;
                }
                "len" => {
                    for a in args {
                        self.infer(&a.value);
                    }
                    return Ty::Int64;
                }
                // String / char builtins (ticket #284, D-0025).
                "ord" => {
                    // ord(c): the codepoint number of a `char`.
                    if args.len() != 1 {
                        self.error(
                            ErrorCode::ZAP4025,
                            "ord() takes exactly one argument".to_string(),
                            span,
                        );
                    }
                    for a in args {
                        let at = self.infer(&a.value);
                        if !matches!(at, Ty::Char | Ty::Unknown) && !at.is_int() {
                            self.error(
                                ErrorCode::ZAP4024,
                                format!("ord() expects a char, found {}", at.display()),
                                expr_span(&a.value),
                            );
                        }
                    }
                    return Ty::Int64;
                }
                "chr" | "char" => {
                    // chr(n)/char(n): the `char` whose codepoint is `n`.
                    if args.len() != 1 {
                        self.error(
                            ErrorCode::ZAP4025,
                            format!("{fname}() takes exactly one argument"),
                            span,
                        );
                    }
                    for a in args {
                        let at = self.infer(&a.value);
                        if !at.is_int() && !matches!(at, Ty::Unknown) {
                            self.error(
                                ErrorCode::ZAP4024,
                                format!(
                                    "{fname}() expects an integer codepoint, found {}",
                                    at.display()
                                ),
                                expr_span(&a.value),
                            );
                        }
                    }
                    return Ty::Char;
                }
                "str" => {
                    // str(x): the textual form of a scalar or string.
                    if args.len() != 1 {
                        self.error(
                            ErrorCode::ZAP4025,
                            "str() takes exactly one argument".to_string(),
                            span,
                        );
                    }
                    for a in args {
                        let at = self.infer(&a.value);
                        // `str(bytes)` decodes UTF-8 (I#337); invalid UTF-8 faults
                        // at runtime. A user struct/enum (`Named`), a tuple, and a
                        // `list`/`dict`/`set` all render through the general
                        // Display protocol (#274, D-0124): the lowerer picks
                        // `__str__`/`__repr__`/`@derive(Repr)`, the tier-3-enum
                        // structural form (#706), or the default field-wise /
                        // container form. An interface value (also `Named`) renders
                        // by dynamic dispatch to its concrete type's renderer
                        // (#707).
                        if !matches!(
                            at,
                            Ty::Str
                                | Ty::Bytes
                                | Ty::Char
                                | Ty::Bool
                                | Ty::Unknown
                                | Ty::Py
                                | Ty::Named(_)
                                | Ty::Tuple(_)
                                | Ty::List(_)
                                | Ty::Dict(_, _)
                                | Ty::Set(_)
                        ) && !at.is_numeric()
                        {
                            self.error(
                                ErrorCode::ZAP4024,
                                format!("str() cannot render {}", at.display()),
                                expr_span(&a.value),
                            );
                        }
                    }
                    return Ty::Str;
                }
                "bytes" => {
                    // bytes(): empty; bytes(str): UTF-8 encode; bytes(list[int64]
                    // | list[int32]): one byte per element (0..=255 at runtime).
                    if args.len() > 1 {
                        self.error(
                            ErrorCode::ZAP4025,
                            "bytes() takes at most one argument".to_string(),
                            span,
                        );
                    }
                    for a in args {
                        let at = self.infer(&a.value);
                        let ok = matches!(at, Ty::Str | Ty::Bytes | Ty::Unknown | Ty::Py)
                            || matches!(&at, Ty::List(e) if e.is_int() || matches!(**e, Ty::Unknown));
                        if !ok {
                            self.error(
                                ErrorCode::ZAP4024,
                                format!(
                                    "bytes() expects a str or list of integers, found {}",
                                    at.display()
                                ),
                                expr_span(&a.value),
                            );
                        }
                    }
                    return Ty::Bytes;
                }
                "set" => {
                    // set(iterable): a new set of the iterable's deduplicated
                    // elements (D17, #609), mirroring Python's `set(xs)`. The
                    // element type is the iterable's. An empty set has no element
                    // type to infer, so `set()` directs the user to `set[T]()`.
                    if args.is_empty() {
                        self.error(
                            ErrorCode::ZAP4025,
                            "set() needs an iterable; build an empty set with set[T]() (e.g. set[int64]())"
                                .to_string(),
                            span,
                        );
                        return Ty::Set(Box::new(Ty::Unknown));
                    }
                    if args.len() != 1 || args[0].name.is_some() {
                        self.error(
                            ErrorCode::ZAP4025,
                            "set(iterable) takes exactly one positional argument".to_string(),
                            span,
                        );
                    }
                    let at = self.infer(&args[0].value);
                    let elem = self.iter_element(&at, expr_span(&args[0].value));
                    self.warn_struct_key(&elem, span);
                    let ty = Ty::Set(Box::new(elem));
                    self.check_numeric_container_value(&ty, span);
                    return ty;
                }
                "int" => {
                    // int(x): parse a `str`, or convert a number, to an `int`.
                    if args.len() != 1 {
                        self.error(
                            ErrorCode::ZAP4025,
                            "int() takes exactly one argument".to_string(),
                            span,
                        );
                    }
                    for a in args {
                        let at = self.infer(&a.value);
                        if !matches!(at, Ty::Str | Ty::Char | Ty::Unknown | Ty::Py)
                            && (!at.is_numeric() || matches!(at, Ty::Complex))
                            && !self.is_tier1_enum(&at)
                        {
                            self.error(
                                ErrorCode::ZAP4024,
                                format!("int() expects a str or number, found {}", at.display()),
                                expr_span(&a.value),
                            );
                        }
                    }
                    return Ty::Int64;
                }
                "float" => {
                    // float(x): parse a `str`, or convert a number/bool, to a
                    // float (#282/#285). Distinct from `float64(x)`, which only
                    // converts numbers (it does not parse strings).
                    if args.len() != 1 {
                        self.error(
                            ErrorCode::ZAP4025,
                            "float() takes exactly one argument".to_string(),
                            span,
                        );
                    }
                    for a in args {
                        let at = self.infer(&a.value);
                        if !matches!(at, Ty::Str | Ty::Bool | Ty::Unknown | Ty::Py)
                            && (!at.is_numeric() || matches!(at, Ty::Complex))
                        {
                            self.error(
                                ErrorCode::ZAP4024,
                                format!("float() expects a str or number, found {}", at.display()),
                                expr_span(&a.value),
                            );
                        }
                    }
                    return Ty::Float64;
                }
                "bool" => {
                    // bool(x): Python-style truthiness (#285). A number/char is
                    // falsey when zero, a str when empty, a bool is itself.
                    if args.len() != 1 {
                        self.error(
                            ErrorCode::ZAP4025,
                            "bool() takes exactly one argument".to_string(),
                            span,
                        );
                    }
                    for a in args {
                        let at = self.infer(&a.value);
                        if !matches!(at, Ty::Str | Ty::Char | Ty::Bool | Ty::Unknown | Ty::Py)
                            && !at.is_numeric()
                        {
                            self.error(
                                ErrorCode::ZAP4024,
                                format!(
                                    "bool() expects a str, char, number or bool, found {}",
                                    at.display()
                                ),
                                expr_span(&a.value),
                            );
                        }
                    }
                    return Ty::Bool;
                }
                "format" => {
                    // format(value, spec): render `value` per the format
                    // mini-language `spec` (D-0026). The spec must be a string
                    // literal in v1 (the same parser the f-string fields use),
                    // so the spec is validated against the value's type here.
                    if args.len() != 2 {
                        self.error(
                            ErrorCode::ZAP4025,
                            "format() takes exactly two arguments (value, spec)".to_string(),
                            span,
                        );
                        for a in args {
                            self.infer(&a.value);
                        }
                        return Ty::Str;
                    }
                    let vt = self.infer(&args[0].value);
                    self.check_renderable(&vt, expr_span(&args[0].value));
                    let st = self.infer(&args[1].value);
                    match &args[1].value {
                        Expr::Str {
                            value,
                            fstring: false,
                            bytes: false,
                            ..
                        } => match fstring::parse_format_spec(value) {
                            Ok(fs) => self.check_spec_type(&fs, &vt, expr_span(&args[1].value)),
                            Err(msg) => self.error(
                                ErrorCode::ZAP4026,
                                format!("invalid format spec '{value}': {msg}"),
                                expr_span(&args[1].value),
                            ),
                        },
                        _ => {
                            if !matches!(st, Ty::Unknown) {
                                self.error(
                                    ErrorCode::ZAP4026,
                                    "format()'s spec must be a plain string literal in v1"
                                        .to_string(),
                                    expr_span(&args[1].value),
                                );
                            }
                        }
                    }
                    return Ty::Str;
                }
                // `close(ch)` (ticket #25, D-0031): the argument must be a channel.
                "close" => {
                    if args.len() != 1 {
                        self.error(
                            ErrorCode::ZAP4025,
                            "close() takes exactly one argument".to_string(),
                            span,
                        );
                    }
                    for a in args {
                        let at = self.infer(&a.value);
                        if !matches!(at, Ty::Chan(_) | Ty::Unknown) {
                            self.error(
                                ErrorCode::ZAP4024,
                                format!("close() expects a channel, found {}", at.display()),
                                expr_span(&a.value),
                            );
                        }
                    }
                    return Ty::Unit;
                }
                "bigint" => {
                    if args.len() != 1 {
                        self.error(
                            ErrorCode::ZAP4025,
                            "bigint() takes exactly one numeric argument".to_string(),
                            span,
                        );
                    }
                    for a in args {
                        let at = self.infer(&a.value);
                        if (!at.is_numeric() || matches!(at, Ty::Complex))
                            && !matches!(at, Ty::Unknown)
                        {
                            self.error(
                                ErrorCode::ZAP4024,
                                format!("bigint() expects a real number, found {}", at.display()),
                                expr_span(&a.value),
                            );
                        }
                    }
                    return Ty::Bigint;
                }
                "complex" => {
                    if args.len() > 2 {
                        self.error(
                            ErrorCode::ZAP4025,
                            "complex() takes at most two numeric arguments".to_string(),
                            span,
                        );
                    }
                    for a in args {
                        let at = self.infer(&a.value);
                        if !at.is_numeric() && !matches!(at, Ty::Unknown) {
                            self.error(
                                ErrorCode::ZAP4024,
                                format!("complex() expects a number, found {}", at.display()),
                                expr_span(&a.value),
                            );
                        } else if args.len() == 2 && matches!(at, Ty::Complex) {
                            self.error(
                                ErrorCode::ZAP4024,
                                "complex(real, imag) arguments must be real numbers".to_string(),
                                expr_span(&a.value),
                            );
                        }
                    }
                    return Ty::Complex;
                }
                "int32" | "int64" | "float32" | "float64" | "i32" | "i64" | "f32" | "f64"
                | "uint8" | "uint16" | "uint32" | "uint64" | "u8" | "u16" | "u32" | "u64"
                | "usize" => {
                    // Explicit numeric conversion (D-0020): the way to narrow or
                    // cross the int/float *or* the signed<->unsigned boundary that
                    // implicit coercion refuses (#552). Out-of-range / negative
                    // inputs wrap (C cast semantics, documented in D-0019).
                    let to = match fname.as_str() {
                        "int32" | "i32" => Ty::Int32,
                        "int64" | "i64" => Ty::Int64,
                        "uint8" | "u8" => Ty::Uint8,
                        "uint16" | "u16" => Ty::Uint16,
                        "uint32" | "u32" => Ty::Uint32,
                        "uint64" | "u64" => Ty::Uint64,
                        "usize" => Ty::Usize,
                        "float32" | "f32" => Ty::Float32,
                        _ => Ty::Float64,
                    };
                    if args.len() != 1 {
                        self.error(
                            ErrorCode::ZAP4025,
                            format!("{fname}() takes exactly one argument"),
                            span,
                        );
                    }
                    for a in args {
                        let at = self.infer(&a.value);
                        // A Python handle is accepted: the conversion marshals it
                        // back to a Zap number (D-0012). A tier-1 enum converts to
                        // its backing int (`int64(c)`, D-0018 explicit narrowing).
                        if !at.is_numeric()
                            && !matches!(at, Ty::Unknown | Ty::Py)
                            && !self.is_tier1_enum(&at)
                        {
                            self.error(
                                ErrorCode::ZAP4024,
                                format!("{}() expects a number, found {}", fname, at.display()),
                                expr_span(&a.value),
                            );
                        } else if matches!(at, Ty::Complex) {
                            self.error(
                                ErrorCode::ZAP4024,
                                format!("{}() cannot convert a complex value", fname),
                                expr_span(&a.value),
                            );
                        }
                    }
                    return to;
                }
                // Runtime reflection (D-0036). `reflect_has(type, name)` reports
                // whether the type was annotated/decorated with `name`;
                // `reflect_names(type)` returns the comma-joined names. The type
                // and name are identified by string (the reflection surface is
                // intentionally string-keyed; see `design/0004-decorators.md`).
                "reflect_has" => {
                    self.check_reflect_args(fname, 2, args, span);
                    return Ty::Bool;
                }
                "reflect_names" => {
                    self.check_reflect_args(fname, 1, args, span);
                    return Ty::Str;
                }
                // Function/callable signature reflection (I#458, D-0089). All are
                // string-keyed like the struct/type surface above: the owner is a
                // function name (top-level `def`) or `"Type.method"`. `reflect_params`
                // / `reflect_sig` take just the owner; the per-parameter queries take
                // the owner plus a parameter name. See `design/0004-decorators.md`.
                "reflect_params" | "reflect_sig" => {
                    self.check_reflect_args(fname, 1, args, span);
                    return Ty::Str;
                }
                "reflect_param_type" | "reflect_param_annotations" => {
                    self.check_reflect_args(fname, 2, args, span);
                    return Ty::Str;
                }
                "reflect_param_has_default" => {
                    self.check_reflect_args(fname, 2, args, span);
                    return Ty::Bool;
                }
                // `checkpoint()` (D-0044): an explicit cancellation point for long
                // CPU-bound loops. Takes no arguments; may raise `Cancelled`.
                "checkpoint" => {
                    if !args.is_empty() {
                        self.error(
                            ErrorCode::ZAP4025,
                            "checkpoint() takes no arguments".to_string(),
                            span,
                        );
                    }
                    return Ty::Unit;
                }
                // `__netpoll_selftest()` (ticket #80): runs the netpoller
                // integration self-test and returns the number of sub-checks
                // that passed. Internal; takes no arguments.
                "__netpoll_selftest" => {
                    if !args.is_empty() {
                        self.error(
                            ErrorCode::ZAP4025,
                            "__netpoll_selftest() takes no arguments".to_string(),
                            span,
                        );
                    }
                    return Ty::Int64;
                }
                // `__signal_selftest()` (ticket #77): runs the signal subsystem
                // integration self-test (self-pipe + netpoller + cap-1 coalescing)
                // and returns the number of sub-checks that passed. Internal.
                "__signal_selftest" => {
                    if !args.is_empty() {
                        self.error(
                            ErrorCode::ZAP4025,
                            "__signal_selftest() takes no arguments".to_string(),
                            span,
                        );
                    }
                    return Ty::Int64;
                }
                // `__gc_finalizer_selftest()` (ticket #92): runs the precise
                // collector's finalizer self-test and returns the number of
                // finalizers that ran. Internal; takes no arguments.
                "__gc_finalizer_selftest" => {
                    if !args.is_empty() {
                        self.error(
                            ErrorCode::ZAP4025,
                            "__gc_finalizer_selftest() takes no arguments".to_string(),
                            span,
                        );
                    }
                    return Ty::Int64;
                }
                "__raise_signal" => {
                    if args.len() != 1 {
                        self.error(
                            ErrorCode::ZAP4025,
                            "__raise_signal(n) takes one argument".to_string(),
                            span,
                        );
                    }
                    for a in args {
                        let at = self.infer(&a.value);
                        if !at.is_int() && !matches!(at, Ty::Unknown) {
                            self.error(
                                ErrorCode::ZAP4024,
                                "__raise_signal(n) expects an integer signal number".to_string(),
                                expr_span(&a.value),
                            );
                        }
                    }
                    return Ty::Unit;
                }
                "__signal_drain" => {
                    if !args.is_empty() {
                        self.error(
                            ErrorCode::ZAP4025,
                            "__signal_drain() takes no arguments".to_string(),
                            span,
                        );
                    }
                    return Ty::Unit;
                }
                _ => {
                    if let Some(sig) = self.sigs.get(fname).cloned() {
                        // `@deprecated` free function (I#472, D-0109): warn here.
                        self.warn_deprecated_call(fname, span);
                        return self.check_user_call(&sig, fname, args, span);
                    }
                    // Tier-1 enum conversion `Color(x)` — the explicit int->enum
                    // direction (D-0018/D-0020): one integer argument.
                    if let Some(info) = self.enums.get(fname).cloned() {
                        if info.is_sum {
                            self.error(
                                ErrorCode::ZAP5110,
                                format!(
                                    "'{fname}' is a sum type; construct a variant \
                                     (e.g. '{fname}.Variant(...)'), not '{fname}(...)'"
                                ),
                                span,
                            );
                        } else if info.variants.is_empty() {
                            self.error(
                                ErrorCode::ZAP4024,
                                format!("empty enum '{fname}' has no values to construct"),
                                span,
                            );
                        } else if args.len() != 1 {
                            self.error(
                                ErrorCode::ZAP4025,
                                format!("{fname}() takes exactly one argument"),
                                span,
                            );
                        } else {
                            let at = self.infer(&args[0].value);
                            if !at.is_int() && !matches!(at, Ty::Unknown | Ty::Py) {
                                self.error(
                                    ErrorCode::ZAP4024,
                                    format!("{fname}() expects an integer, found {}", at.display()),
                                    expr_span(&args[0].value),
                                );
                            }
                        }
                        return Ty::Named(fname.clone());
                    }
                    // Struct construction: `StructName(fields...)`.
                    if self.user_types.contains(fname) {
                        // Arena (#553, D-0158): reject a statically-known
                        // `size > cap` at compile time — the arena could never
                        // satisfy its own initial reservation (ZAP5132). A runtime
                        // `size`/`cap` is bounds-checked at `arena()` open instead.
                        if fname == "arena" {
                            self.check_arena_sizing(args, span);
                        }
                        for a in args {
                            self.infer(&a.value);
                        }
                        return Ty::Named(fname.clone());
                    }
                    // A callable user struct held in a variable (D-0047):
                    // `f(args)` desugars to `f.__call__(args)`.
                    if let Some(vt) = self.lookup(fname) {
                        if self.struct_info(&vt).is_some() {
                            for a in args {
                                self.infer(&a.value);
                            }
                            return self.op_result(&vt, "__call__", "Call", "()", span);
                        }
                        // A first-class function value held in a variable (I#163):
                        // `f(args)` is an indirect call. Function values are typed
                        // loosely (`Unknown`) for now, so accept the call and
                        // type the result loosely; the lowerer carries the real
                        // signature.
                        for a in args {
                            self.infer(&a.value);
                        }
                        return Ty::Unknown;
                    }
                    self.error(
                        ErrorCode::ZAP3003,
                        format!("call to unknown function '{fname}'"),
                        span,
                    );
                    for a in args {
                        self.infer(&a.value);
                    }
                    return Ty::Unknown;
                }
            }
        }
        // Dynamic dispatch on an interface-typed receiver (D-0016): resolve the
        // method against the interface's declared set so the call is typed by its
        // return type and a misspelled method is caught at compile time. Handled
        // for a simple `name.method(...)` receiver (a binding or parameter), the
        // form interface values take today.
        if let Expr::Attr { base, name, span } = callee {
            if let Expr::Name(bn, _) = base.as_ref() {
                if let Some(Ty::Named(tn)) = self.lookup(bn) {
                    // The receiver is either an interface-typed value (D-0016,
                    // dynamic dispatch) or a value of a bounded type parameter
                    // `T: Iface` (I#164, static dispatch after monomorphization).
                    // Both resolve the call against the interface's declared set:
                    // the call is typed by the method's return type and a method
                    // the interface/bound does not provide is caught here.
                    let is_iface = self.interfaces.contains_key(&tn);
                    let iface_name = if is_iface {
                        Some(tn.clone())
                    } else {
                        self.tyvar_bounds.get(&tn).cloned()
                    };
                    if let Some(iname) = iface_name {
                        // Resolve against the bound/interface *and its refined
                        // supertypes* (D-0139/#806): a supertype method is
                        // callable on a value of a bounded type parameter.
                        let ret = match self.iface_method(&iname, name) {
                            Some(sig) => sig.ret.clone(),
                            None => {
                                let what = if is_iface {
                                    format!("interface '{tn}' has no method '{name}'")
                                } else {
                                    format!(
                                        "type parameter '{tn}' (bound '{iname}') has no \
                                         method '{name}'"
                                    )
                                };
                                self.error(ErrorCode::ZAP4080, what, *span);
                                Ty::Unknown
                            }
                        };
                        for a in args {
                            self.infer(&a.value);
                        }
                        return ret;
                    }
                }
            }
        }
        // Method call `recv.method(args)`. `str` methods (ticket #284, D-0025)
        // are typed precisely (e.g. `s.split(",")` is `list[str]`); a call
        // through a Python handle yields a Python handle (D-0012); anything else
        // is modeled loosely for now.
        if let Expr::Attr { base, name, span } = callee {
            let bt = self.infer(base);
            // Built-in `arena` methods (#553, D-0158): `a.alloc(bytes, n) -> bytes`
            // and `a.new(T, ...) -> T` for a leaf `T`. Typed precisely here, before
            // the generic struct-method path, because the first argument is a *type*
            // (not a value) and the result type is derived from it, and because
            // leaf-only is enforced at this site (ZAP5130).
            if matches!(&bt, Ty::Named(n) if n == "arena") {
                return self.check_arena_method(name, args, *span);
            }
            if matches!(bt, Ty::Str) {
                return self.check_str_method(name, args, *span);
            }
            // Built-in `dict` methods (ticket #299), typed precisely so that
            // `for k in d.keys()`, `for k, v in d.items()`, and a `copy()` that
            // is then indexed all check against the real element types.
            if let Ty::Dict(k, v) = &bt {
                let (k, v) = (k.clone(), v.clone());
                return self.check_dict_method(name, args, &k, &v, *span);
            }
            // AtomicArray[T] methods (#844): load(i)->T, store(i,v), len()->int64,
            // compare_and_swap(i,old,new)->bool — all seq_cst, typed precisely.
            if let Ty::AtomicArray(elem) = &bt {
                let elem = (**elem).clone();
                return self.check_atomicarray_method(name, args, &elem, *span);
            }
            // A method call on a user struct `recv.method(args)` is typed by the
            // method's declared return type (D-0009). The struct's method table
            // already carries the resolved signature, with `self`/`Self` bound to
            // the receiver's type (`sig_of_method`) and a generator method rewritten
            // to its synthetic iterator (I#429), so the return type is read straight
            // off it. Resolving it here — rather than falling back to `Unknown` —
            // lets a method-chain result drive operator overloading: the fluent
            // `Key("pk").eq(x) & Attr("age").gt(y)` condition builders (operators
            // #52) now dispatch `&`/`|`/`~` on the struct instead of collapsing to
            // the built-in int64 bitwise rule. Args stay inferred but otherwise
            // unconstrained, matching the loose arity behavior other receiver paths
            // rely on. An unknown method keeps the former loose `Unknown` fallback.
            if let Some(ret) = self
                .struct_info(&bt)
                .and_then(|i| i.methods.get(name))
                .map(|sig| sig.ret.clone())
            {
                // `@deprecated` method (I#472, D-0109): warn at the call site,
                // keyed `"Type.method"`. The receiver type of a user struct is
                // `Ty::Named(TypeName)` (see `struct_info`).
                if let Ty::Named(owner) = &bt {
                    self.warn_deprecated_call(&format!("{owner}.{name}"), *span);
                }
                for a in args {
                    self.infer(&a.value);
                }
                return ret;
            }
            for a in args {
                self.infer(&a.value);
            }
            return if matches!(bt, Ty::Py) {
                Ty::Py
            } else {
                Ty::Unknown
            };
        }
        let ct = self.infer(callee);
        for a in args {
            self.infer(&a.value);
        }
        if matches!(ct, Ty::Py) {
            Ty::Py
        } else {
            Ty::Unknown
        }
    }

    /// Type-check a `str` method call (ticket #284, D-0025), returning its
    /// result type. Arity and argument types are checked here so a misspelled
    /// or misused method is caught at compile time.
    /// Type-check a built-in `arena` method call (#553, D-0158): `a.alloc(bytes, n)`
    /// -> `bytes` and `a.new(T, ...)` -> `T`. The first argument is a *type*, and
    /// the result type is derived from it. Arena v1 is LEAF-ONLY: a pointer-holding
    /// element type is rejected here (ZAP5130). Typed numeric arena arrays are a
    /// tracked follow-up (#881), so v1 `alloc` supports only `bytes`.
    fn check_arena_method(&mut self, name: &str, args: &[Arg], span: Span) -> Ty {
        match name {
            "alloc" => {
                if args.len() != 2 || args.iter().any(|a| a.name.is_some()) {
                    self.error(
                        ErrorCode::ZAP5130,
                        "a.alloc(T, n) takes exactly two positional arguments: a leaf \
                         element type and an integer count"
                            .to_string(),
                        span,
                    );
                    for a in args {
                        self.infer(&a.value);
                    }
                    return Ty::Unknown;
                }
                let elem = self.arena_type_arg(&args[0].value, args[0].span);
                let ct = self.infer(&args[1].value);
                if !matches!(ct, Ty::Unknown) && !ct.is_int() {
                    self.error(
                        ErrorCode::ZAP5130,
                        format!("a.alloc count must be an integer, got `{}`", ct.display()),
                        args[1].span,
                    );
                }
                match elem {
                    ArenaElem::Bytes => Ty::Bytes,
                    ArenaElem::Bad => Ty::Unknown,
                    ArenaElem::LeafStruct(_) | ArenaElem::Scalar => {
                        self.error(
                            ErrorCode::ZAP5130,
                            "arena v1 `a.alloc` supports only `bytes` (a non-moving byte \
                             buffer); use `a.new(T, ...)` for a single leaf object. Typed \
                             numeric arena arrays are a follow-up (#881)"
                                .to_string(),
                            args[0].span,
                        );
                        Ty::Unknown
                    }
                }
            }
            "new" => {
                if args.is_empty() || args[0].name.is_some() {
                    self.error(
                        ErrorCode::ZAP5130,
                        "a.new(T, ...) requires a leaf struct type as its first argument"
                            .to_string(),
                        span,
                    );
                    for a in args {
                        self.infer(&a.value);
                    }
                    return Ty::Unknown;
                }
                let elem = self.arena_type_arg(&args[0].value, args[0].span);
                // Constructor arguments are inferred loosely, matching the loose
                // arity behaviour of ordinary struct construction.
                for a in &args[1..] {
                    self.infer(&a.value);
                }
                match elem {
                    // Non-leaf structs already reported in `arena_type_arg`; recover
                    // to the named type so downstream use does not cascade.
                    ArenaElem::LeafStruct(n) => Ty::Named(n),
                    ArenaElem::Bad => Ty::Unknown,
                    ArenaElem::Bytes => {
                        self.error(
                            ErrorCode::ZAP5130,
                            "a.new(T, ...) constructs a single leaf struct; use \
                             `a.alloc(bytes, n)` for a byte buffer"
                                .to_string(),
                            args[0].span,
                        );
                        Ty::Unknown
                    }
                    ArenaElem::Scalar => {
                        self.error(
                            ErrorCode::ZAP5130,
                            "a.new(T, ...) requires a leaf struct type, not a scalar".to_string(),
                            args[0].span,
                        );
                        Ty::Unknown
                    }
                }
            }
            other => {
                self.error(
                    ErrorCode::ZAP5130,
                    format!(
                        "`arena` has no method `{other}`; use `a.alloc(bytes, n)` or \
                         `a.new(T, ...)`"
                    ),
                    span,
                );
                for a in args {
                    self.infer(&a.value);
                }
                Ty::Unknown
            }
        }
    }

    /// Classify the first (type) argument of an `arena` allocation, emitting
    /// ZAP5130 for a non-type argument, an unknown/unsupported type, or a
    /// pointer-holding (non-leaf) struct. A non-leaf struct still returns
    /// [`ArenaElem::LeafStruct`] after reporting, so the caller recovers cleanly.
    fn arena_type_arg(&mut self, e: &Expr, span: Span) -> ArenaElem {
        let Expr::Name(n, _) = e else {
            self.error(
                ErrorCode::ZAP5130,
                "the first argument to `a.alloc`/`a.new` must be a type".to_string(),
                span,
            );
            return ArenaElem::Bad;
        };
        if n == "bytes" {
            return ArenaElem::Bytes;
        }
        if is_leaf_scalar_name(n) {
            return ArenaElem::Scalar;
        }
        // A user struct: leaf iff every field is a pointer-free machine scalar.
        if let Some(info) = self.structs.get(n) {
            let leaf = info.fields.iter().all(is_leaf_scalar_ty);
            if !leaf {
                self.error(
                    ErrorCode::ZAP5130,
                    format!(
                        "arena objects must be pointer-free (leaf) in v1, but struct \
                         `{n}` holds a non-scalar (pointer-bearing) field; express \
                         arena-internal links as scalar indices into the arena"
                    ),
                    span,
                );
            }
            return ArenaElem::LeafStruct(n.clone());
        }
        self.error(
            ErrorCode::ZAP5130,
            format!("`{n}` is not a known type usable as an arena element"),
            span,
        );
        ArenaElem::Bad
    }

    /// Reject a statically-known `size > cap` on an `arena(...)` head (#553, D-0158;
    /// `ZAP5132`). Fires only when BOTH `size` and `cap` fold to compile-time
    /// integer constants (the common `size=64*1024, cap=1<<20` case); a runtime
    /// `size`/`cap` cannot be caught here and is bounds-checked at `arena()` open.
    /// `cap == 0` means unbounded, so it is never a contradiction.
    fn check_arena_sizing(&mut self, args: &[Arg], span: Span) {
        let (mut size, mut cap): (Option<i64>, Option<i64>) = (None, None);
        let mut positional = 0usize;
        for a in args {
            let val = arena_const_int(&a.value);
            match a.name.as_deref() {
                Some("size") => size = val,
                Some("cap") => cap = val,
                Some(_) => {}
                None => {
                    if positional == 0 {
                        size = val;
                    } else if positional == 1 {
                        cap = val;
                    }
                    positional += 1;
                }
            }
        }
        if let (Some(s), Some(c)) = (size, cap) {
            if c > 0 && s > c {
                self.error(
                    ErrorCode::ZAP5132,
                    format!(
                        "arena size ({s}) exceeds cap ({c}); the arena can never \
                         satisfy its own initial reservation"
                    ),
                    span,
                );
            }
        }
    }

    fn check_str_method(&mut self, name: &str, args: &[Arg], span: Span) -> Ty {
        // Methods taking no arguments → `str`.
        let no_arg_str = matches!(name, "upper" | "lower" | "strip" | "lstrip" | "rstrip");
        // Methods taking a single `str` argument, with their result type.
        let one_str_arg: Option<Ty> = match name {
            "startswith" | "endswith" | "contains" => Some(Ty::Bool),
            "find" | "index" => Some(Ty::Int64),
            "split" => Some(Ty::List(Box::new(Ty::Str))),
            _ => None,
        };
        if no_arg_str {
            if !args.is_empty() {
                self.error(
                    ErrorCode::ZAP4025,
                    format!("str.{name}() takes no arguments"),
                    span,
                );
                for a in args {
                    self.infer(&a.value);
                }
            }
            return Ty::Str;
        }
        if let Some(ret) = one_str_arg {
            self.check_str_args(name, args, 1, span);
            return ret;
        }
        match name {
            "replace" => {
                self.check_str_args(name, args, 2, span);
                Ty::Str
            }
            "join" => {
                // `sep.join(parts)`: one `list[str]` argument.
                if args.len() != 1 {
                    self.error(
                        ErrorCode::ZAP4025,
                        "str.join() takes exactly one argument".to_string(),
                        span,
                    );
                }
                for a in args {
                    let at = self.infer(&a.value);
                    if !assignable(&at, &Ty::List(Box::new(Ty::Str))) && !matches!(at, Ty::Unknown)
                    {
                        self.error(
                            ErrorCode::ZAP4024,
                            format!("str.join() expects a list[str], found {}", at.display()),
                            expr_span(&a.value),
                        );
                    }
                }
                Ty::Str
            }
            "encode" => {
                if !args.is_empty() {
                    self.error(
                        ErrorCode::ZAP4025,
                        "str.encode() takes no arguments".to_string(),
                        span,
                    );
                    for a in args {
                        self.infer(&a.value);
                    }
                }
                Ty::List(Box::new(Ty::Int64))
            }
            _ => {
                self.error(
                    ErrorCode::ZAP4080,
                    format!("str has no method '{name}'"),
                    span,
                );
                for a in args {
                    self.infer(&a.value);
                }
                Ty::Unknown
            }
        }
    }

    /// Type-check a built-in `dict[K, V]` method call (tickets #299, #281).
    /// `keys() -> list[K]`, `values() -> list[V]`,
    /// `items() -> list[(K, V)]`, and `copy() -> dict[K, V]` (a shallow copy)
    /// take no arguments; `get(key) -> Optional[V]` (ticket #281) takes the key
    /// and returns the value wrapped in `Some`, or `None` when the key is absent
    /// — a safe, non-aborting read (unlike `d[key]`, which raises on a miss).
    /// Type an `AtomicArray[T]` method (#844). All ops are seq_cst. The index is
    /// an integer; stored/compared values are assignable to the element type `T`.
    ///   `len() -> int64`
    ///   `load(i) -> T`                       (raw slot; None/0 when unset)
    ///   `store(i, v)`                        (-> unit)
    ///   `compare_and_swap(i, old, new) -> bool`
    fn check_atomicarray_method(&mut self, name: &str, args: &[Arg], elem: &Ty, span: Span) -> Ty {
        // Shared helper: the first argument is the index, which must be an integer.
        let check_index = |s: &mut Self, a: &Arg| {
            let at = s.infer(&a.value);
            if !at.is_int() && !matches!(at, Ty::Unknown) {
                s.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "AtomicArray index must be an integer, found {}",
                        at.display()
                    ),
                    expr_span(&a.value),
                );
            }
        };
        // Shared helper: a value argument must be assignable to the element type.
        let check_val = |s: &mut Self, a: &Arg| {
            let vt = s.infer(&a.value);
            if !assignable(&vt, elem) && !matches!(vt, Ty::Unknown) {
                s.error(
                    ErrorCode::ZAP4024,
                    format!(
                        "AtomicArray[{}] value has type {}",
                        elem.display(),
                        vt.display()
                    ),
                    expr_span(&a.value),
                );
            }
        };
        match name {
            "len" => {
                if !args.is_empty() {
                    self.error(
                        ErrorCode::ZAP4025,
                        "AtomicArray.len() takes no arguments".to_string(),
                        span,
                    );
                }
                Ty::Int64
            }
            "load" => {
                if args.len() != 1 {
                    self.error(
                        ErrorCode::ZAP4025,
                        format!("AtomicArray.load(i) takes one argument, got {}", args.len()),
                        span,
                    );
                } else {
                    check_index(self, &args[0]);
                }
                elem.clone()
            }
            "store" => {
                if args.len() != 2 {
                    self.error(
                        ErrorCode::ZAP4025,
                        format!(
                            "AtomicArray.store(i, v) takes two arguments, got {}",
                            args.len()
                        ),
                        span,
                    );
                } else {
                    check_index(self, &args[0]);
                    check_val(self, &args[1]);
                }
                Ty::Unit
            }
            "compare_and_swap" => {
                if args.len() != 3 {
                    self.error(
                        ErrorCode::ZAP4025,
                        format!(
                            "AtomicArray.compare_and_swap(i, old, new) takes three \
                             arguments, got {}",
                            args.len()
                        ),
                        span,
                    );
                } else {
                    check_index(self, &args[0]);
                    check_val(self, &args[1]);
                    check_val(self, &args[2]);
                }
                Ty::Bool
            }
            other => {
                self.error(
                    ErrorCode::ZAP4080,
                    format!(
                        "AtomicArray has no method '{other}' (supported: load, store, \
                         compare_and_swap, len)"
                    ),
                    span,
                );
                Ty::Unknown
            }
        }
    }

    fn check_dict_method(&mut self, name: &str, args: &[Arg], k: &Ty, v: &Ty, span: Span) -> Ty {
        // `get` is the one dict method that takes arguments, so it is handled
        // before the shared no-argument check below. Two forms (ticket #570):
        //   `get(key)`          -> `Optional[V]` — a safe, non-aborting read
        //                          (`Some(value)` present, `None` absent).
        //   `get(key, default)` -> `V`           — the value when present, else
        //                          `default` (a plain `V`, never `Optional[V]`).
        // The one-arg result is the prelude `Optional[V]`, spelled
        // `Ty::Named("Optional")` exactly as every other `Optional[T]`
        // annotation resolves (I#154), so `match`, `is_none()`/`unwrap_or(...)`,
        // and binding all type-check unchanged.
        if name == "get" {
            let n = args.len();
            if n != 1 && n != 2 {
                self.error(
                    ErrorCode::ZAP4025,
                    format!(
                        "dict.get() takes one or two arguments (the key, and an \
                         optional default), got {n}"
                    ),
                    span,
                );
            }
            for (i, a) in args.iter().enumerate() {
                let at = self.infer(&a.value);
                // First argument is the key, checked against the dict's key type.
                if i == 0 && (n == 1 || n == 2) && !assignable(&at, k) && !matches!(at, Ty::Unknown)
                {
                    self.error(
                        ErrorCode::ZAP4025,
                        format!(
                            "dict.get() key has type {}, but this dict is keyed by {}",
                            at.display(),
                            k.display()
                        ),
                        span,
                    );
                }
                // Second argument is the default, unified against the value type
                // `V` under the ordinary assignment rules — a `str` default for a
                // `dict[str, int64]` is rejected just as an assignment would be.
                if i == 1 && n == 2 && !assignable(&at, v) && !matches!(at, Ty::Unknown) {
                    self.error(
                        ErrorCode::ZAP4024,
                        format!(
                            "dict.get() default has type {}, but this dict has values \
                             of type {}",
                            at.display(),
                            v.display()
                        ),
                        span,
                    );
                }
            }
            if n == 2 {
                return v.clone();
            }
            return Ty::Named("Optional".to_string());
        }
        let ret = match name {
            "keys" => Ty::List(Box::new(k.clone())),
            "values" => Ty::List(Box::new(v.clone())),
            "items" => Ty::List(Box::new(Ty::Tuple(vec![k.clone(), v.clone()]))),
            "copy" => Ty::Dict(Box::new(k.clone()), Box::new(v.clone())),
            other => {
                self.error(
                    ErrorCode::ZAP4080,
                    format!(
                        "dict has no method '{other}' \
                         (supported: keys, values, items, copy, get)"
                    ),
                    span,
                );
                for a in args {
                    self.infer(&a.value);
                }
                return Ty::Unknown;
            }
        };
        if !args.is_empty() {
            self.error(
                ErrorCode::ZAP4025,
                format!("dict.{name}() takes no arguments"),
                span,
            );
            for a in args {
                self.infer(&a.value);
            }
        }
        ret
    }

    /// Check that a `str` method got exactly `n` `str` arguments.
    fn check_str_args(&mut self, name: &str, args: &[Arg], n: usize, span: Span) {
        if args.len() != n {
            self.error(
                ErrorCode::ZAP4025,
                format!("str.{name}() takes exactly {n} argument(s)"),
                span,
            );
        }
        for a in args {
            let at = self.infer(&a.value);
            if !assignable(&at, &Ty::Str) && !matches!(at, Ty::Unknown) {
                self.error(
                    ErrorCode::ZAP4024,
                    format!(
                        "str.{name}() expects a str argument, found {}",
                        at.display()
                    ),
                    expr_span(&a.value),
                );
            }
        }
    }

    fn check_user_call(&mut self, sig: &FnSig, fname: &str, args: &[Arg], span: Span) -> Ty {
        if args.iter().any(|a| a.spread != ArgSpread::None) {
            self.check_user_call_spread(sig, fname, args, span)
        } else {
            self.check_user_call_plain(sig, fname, args, span)
        }
    }

    /// Type-check a keyword argument `name=value` of type `at` against `sig`.
    /// A keyword naming an ordinary parameter is checked against it; any other
    /// keyword is collected by a `**kwargs` parameter (#601), checked against
    /// its value type `V`; without such a parameter it is an unknown keyword.
    fn check_keyword_arg(&mut self, sig: &FnSig, fname: &str, name: &str, at: &Ty, at_span: Span) {
        // A keyword never binds the collector itself (`def f(V **kw)` then
        // `f(kw=1)` puts `kw` into the dict, exactly like Python). A parameter
        // declared before the bare `/` separator is positional-only (#602) and
        // likewise never binds by name: the keyword is collected by `**kw` if the
        // callee has one, otherwise it is an error.
        match sig
            .params
            .iter()
            .find(|p| p.name == name && !p.kwargs && !p.positional_only)
        {
            Some(p) => {
                if !self.coercible(at, &p.ty) {
                    self.error(
                        ErrorCode::ZAP4022,
                        format!(
                            "keyword argument '{}' to '{}' has type {}, expected {}",
                            name,
                            fname,
                            at.display(),
                            p.ty.display()
                        ),
                        at_span,
                    );
                }
            }
            None if sig
                .params
                .iter()
                .any(|p| p.name == name && p.positional_only)
                && !sig.params.iter().any(|p| p.kwargs) =>
            {
                self.error(
                    ErrorCode::ZAP4034,
                    format!(
                        "parameter '{name}' of '{fname}' is positional-only and must \
                         be passed positionally, not by name ('{name}=...')"
                    ),
                    at_span,
                );
            }
            None => match sig.params.iter().find(|p| p.kwargs) {
                Some(kw) => {
                    if !self.coercible(at, &kw.ty) {
                        self.error(
                            ErrorCode::ZAP4022,
                            format!(
                                "keyword argument '{}' collected by '{}' has type {}, \
                                 expected {} (the `**` collector's value type)",
                                name,
                                fname,
                                at.display(),
                                kw.ty.display()
                            ),
                            at_span,
                        );
                    }
                }
                None => self.error(
                    ErrorCode::ZAP4023,
                    format!("'{fname}' has no parameter named '{name}'"),
                    at_span,
                ),
            },
        }
    }

    fn check_user_call_plain(&mut self, sig: &FnSig, fname: &str, args: &[Arg], span: Span) -> Ty {
        let positional: Vec<&Arg> = args.iter().filter(|a| a.name.is_none()).collect();
        let keyword: Vec<&Arg> = args.iter().filter(|a| a.name.is_some()).collect();
        // A `TYPE... name` variadic and a `V **name` collector are not fixed
        // positional parameters: they absorb any number of (extra) arguments, so
        // neither counts toward the positional arity or the required minimum.
        let non_variadic = sig
            .params
            .iter()
            .filter(|p| !p.variadic && !p.kwargs)
            .count();
        let has_var = sig.params.iter().any(|p| p.variadic);
        let required = sig
            .params
            .iter()
            .filter(|p| !p.has_default && !p.variadic && !p.kwargs)
            .count();

        if !has_var && positional.len() > non_variadic {
            self.error(
                ErrorCode::ZAP4020,
                format!("too many arguments to '{fname}' (expected at most {non_variadic})"),
                span,
            );
        }
        if positional.len() + keyword.len() < required {
            self.error(
                ErrorCode::ZAP4020,
                format!("too few arguments to '{fname}' (expected at least {required})"),
                span,
            );
        }

        for (i, a) in positional.iter().enumerate() {
            let at = self.infer(&a.value);
            // A parameter after the bare `*` separator is keyword-only (I#496) and
            // cannot be filled positionally. Positional args bind to params by
            // index, and keyword-only params always follow the positional-or-
            // keyword ones, so a positional arg landing on one is the violation.
            if let Some(p) = sig.params.get(i) {
                if p.keyword_only {
                    self.error(
                        ErrorCode::ZAP4029,
                        format!(
                            "parameter '{}' of '{}' is keyword-only and must be \
                             passed by name ('{}=...')",
                            p.name, fname, p.name
                        ),
                        expr_span(&a.value),
                    );
                    continue;
                }
            }
            // A plain positional never lands on the trailing `**` collector.
            let expected = match sig.params.get(i) {
                Some(p) if p.kwargs => None,
                Some(p) => Some(p.ty.clone()),
                None if has_var => sig.params.iter().find(|p| p.variadic).map(|p| p.ty.clone()),
                None => None,
            };
            if let Some(pt) = expected {
                if !self.coercible(&at, &pt) {
                    self.error(
                        ErrorCode::ZAP4021,
                        format!(
                            "argument {} to '{}' has type {}, expected {}",
                            i + 1,
                            fname,
                            at.display(),
                            pt.display()
                        ),
                        expr_span(&a.value),
                    );
                }
            }
        }

        for a in &keyword {
            let name = a.name.clone().unwrap();
            let at = self.infer(&a.value);
            self.check_keyword_arg(sig, fname, &name, &at, expr_span(&a.value));
        }

        sig.ret.clone()
    }

    /// Type-check a call that carries `*`/`**` spread arguments (#597). The
    /// checks are deliberately conservative: the arity of a statically-unknown
    /// length `*list` is not known at compile time, so arity errors are only
    /// raised where they are decidable, and the spreads are validated against
    /// the callee's variadic / `**kwargs` parameters.
    fn check_user_call_spread(&mut self, sig: &FnSig, fname: &str, args: &[Arg], span: Span) -> Ty {
        let has_var = sig.params.iter().any(|p| p.variadic);
        let var_elem = sig.params.iter().find(|p| p.variadic).map(|p| p.ty.clone());
        let has_kwargs = sig.params.iter().any(|p| p.kwargs);
        let kwargs_vty = sig.params.iter().find(|p| p.kwargs).map(|p| p.ty.clone());
        // Fixed (positional-capacity) parameters, excluding variadic / collector.
        let fixed: Vec<&ParamInfo> = sig
            .params
            .iter()
            .filter(|p| !p.variadic && !p.kwargs)
            .collect();

        // Validate argument ordering: plain positionals, then one `*`, then
        // keywords, then one `**` (Python order). Ranks must not decrease.
        let rank = |a: &Arg| -> u8 {
            match a.spread {
                ArgSpread::Iterable => 1,
                ArgSpread::Mapping => 3,
                ArgSpread::None if a.name.is_some() => 2,
                ArgSpread::None => 0,
            }
        };
        let mut last_rank = 0u8;
        let mut iter_count = 0usize;
        let mut map_count = 0usize;
        for a in args {
            let r = rank(a);
            if r < last_rank {
                self.error(
                    ErrorCode::ZAP5122,
                    format!(
                        "misordered argument to '{fname}': arguments must appear as \
                         positionals, then `*spread`, then keywords, then `**spread`"
                    ),
                    expr_span(&a.value),
                );
            }
            last_rank = last_rank.max(r);
            match a.spread {
                ArgSpread::Iterable => iter_count += 1,
                ArgSpread::Mapping => map_count += 1,
                ArgSpread::None => {}
            }
        }
        if iter_count > 1 {
            self.error(
                ErrorCode::ZAP5122,
                format!(
                    "'{fname}' call has more than one `*` spread (at most one in this version)"
                ),
                span,
            );
        }
        if map_count > 1 {
            self.error(
                ErrorCode::ZAP5122,
                format!(
                    "'{fname}' call has more than one `**` spread (at most one in this version)"
                ),
                span,
            );
        }

        // Explicit plain positionals bind to the fixed parameters in order, then
        // to the variadic element type.
        let mut pos_idx = 0usize;
        for a in args
            .iter()
            .filter(|a| a.spread == ArgSpread::None && a.name.is_none())
        {
            let at = self.infer(&a.value);
            let expected = if pos_idx < fixed.len() {
                if fixed[pos_idx].keyword_only {
                    self.error(
                        ErrorCode::ZAP4029,
                        format!(
                            "parameter '{}' of '{}' is keyword-only and must be \
                             passed by name ('{}=...')",
                            fixed[pos_idx].name, fname, fixed[pos_idx].name
                        ),
                        expr_span(&a.value),
                    );
                    None
                } else {
                    Some(fixed[pos_idx].ty.clone())
                }
            } else {
                var_elem.clone()
            };
            if let Some(pt) = expected {
                if !self.coercible(&at, &pt) {
                    self.error(
                        ErrorCode::ZAP4021,
                        format!(
                            "argument {} to '{}' has type {}, expected {}",
                            pos_idx + 1,
                            fname,
                            at.display(),
                            pt.display()
                        ),
                        expr_span(&a.value),
                    );
                }
            }
            pos_idx += 1;
        }

        // The `*` spread.
        if let Some(a) = args.iter().find(|a| a.spread == ArgSpread::Iterable) {
            let t = self.infer(&a.value);
            match &t {
                Ty::Tuple(elems) => {
                    // A statically-known tuple expands elementwise. Its elements
                    // continue binding where the explicit positionals left off.
                    for (k, et) in elems.iter().enumerate() {
                        let slot = pos_idx + k;
                        let expected = if slot < fixed.len() {
                            Some(fixed[slot].ty.clone())
                        } else {
                            var_elem.clone()
                        };
                        if let Some(pt) = expected {
                            if !self.coercible(et, &pt) {
                                self.error(
                                    ErrorCode::ZAP4021,
                                    format!(
                                        "spread tuple element {} to '{}' has type {}, expected {}",
                                        k + 1,
                                        fname,
                                        et.display(),
                                        pt.display()
                                    ),
                                    expr_span(&a.value),
                                );
                            }
                        } else if !has_var {
                            self.error(
                                ErrorCode::ZAP5122,
                                format!(
                                    "spread tuple has more elements than '{fname}' has \
                                     parameters and '{fname}' is not variadic"
                                ),
                                expr_span(&a.value),
                            );
                            break;
                        }
                    }
                }
                Ty::List(elem) | Ty::Set(elem) => {
                    if !has_var {
                        self.error(
                            ErrorCode::ZAP5122,
                            format!(
                                "cannot `*`-spread a value of unknown length into '{fname}', \
                                 which has no `TYPE... name` variadic parameter"
                            ),
                            expr_span(&a.value),
                        );
                    } else if let Some(ve) = &var_elem {
                        if !self.coercible(elem, ve) {
                            self.error(
                                ErrorCode::ZAP4021,
                                format!(
                                    "`*`-spread of {} into '{}' variadic expects element type {}",
                                    t.display(),
                                    fname,
                                    ve.display()
                                ),
                                expr_span(&a.value),
                            );
                        }
                    }
                }
                Ty::Range => {
                    if !has_var {
                        self.error(
                            ErrorCode::ZAP5122,
                            format!(
                                "cannot `*`-spread a range into '{fname}', which has no \
                                 `TYPE... name` variadic parameter"
                            ),
                            expr_span(&a.value),
                        );
                    } else if let Some(ve) = &var_elem {
                        if !self.coercible(&Ty::Int64, ve) {
                            self.error(
                                ErrorCode::ZAP4021,
                                format!(
                                    "`*`-spread of a range into '{}' variadic expects element \
                                     type {} (ranges yield int64)",
                                    fname,
                                    ve.display()
                                ),
                                expr_span(&a.value),
                            );
                        }
                    }
                }
                Ty::Unknown => {}
                other => self.error(
                    ErrorCode::ZAP5122,
                    format!(
                        "`*` spread expects a list, tuple, or range, found {}",
                        other.display()
                    ),
                    expr_span(&a.value),
                ),
            }
        }

        // Explicit keyword arguments.
        for a in args
            .iter()
            .filter(|a| a.spread == ArgSpread::None && a.name.is_some())
        {
            let name = a.name.clone().unwrap();
            let at = self.infer(&a.value);
            self.check_keyword_arg(sig, fname, &name, &at, expr_span(&a.value));
        }

        // The `**` spread must be a `dict[str, V]` feeding a `**kwargs` collector.
        if let Some(a) = args.iter().find(|a| a.spread == ArgSpread::Mapping) {
            let t = self.infer(&a.value);
            match &t {
                Ty::Dict(k, v) => {
                    if !matches!(**k, Ty::Str) {
                        self.error(
                            ErrorCode::ZAP5122,
                            format!(
                                "`**` spread expects a dict with str keys, found {}",
                                t.display()
                            ),
                            expr_span(&a.value),
                        );
                    } else if !has_kwargs {
                        self.error(
                            ErrorCode::ZAP5122,
                            format!(
                                "cannot `**`-spread into '{fname}', which has no \
                                 `V **name` keyword-collector parameter"
                            ),
                            expr_span(&a.value),
                        );
                    } else if let Some(vty) = &kwargs_vty {
                        if !self.coercible(v, vty) {
                            self.error(
                                ErrorCode::ZAP4022,
                                format!(
                                    "`**`-spread of {} into '{}' collector expects value type {}",
                                    t.display(),
                                    fname,
                                    vty.display()
                                ),
                                expr_span(&a.value),
                            );
                        }
                    }
                }
                Ty::Unknown => {}
                other => self.error(
                    ErrorCode::ZAP5122,
                    format!(
                        "`**` spread expects a dict[str, V], found {}",
                        other.display()
                    ),
                    expr_span(&a.value),
                ),
            }
        }

        sig.ret.clone()
    }

    /// Is `t` a tier-1 (all-constant) enum, convertible to/from an integer?
    fn is_tier1_enum(&self, t: &Ty) -> bool {
        matches!(t, Ty::Named(n) if self.enums.get(n).is_some_and(|e| !e.is_sum))
    }

    /// Type a qualified enum-variant construction `Enum.Variant(args)` (D-0046),
    /// accepting named or positional payload arguments and checking their
    /// types. Returns the enum type.
    fn check_variant_construction(
        &mut self,
        enum_name: &str,
        variant: &str,
        info: &EnumInfo,
        args: &[Arg],
        span: Span,
    ) -> Ty {
        let Some(v) = info.variant(variant) else {
            self.error(
                ErrorCode::ZAP4060,
                format!("enum '{enum_name}' has no variant '{variant}'"),
                span,
            );
            for a in args {
                self.infer(&a.value);
            }
            return Ty::Named(enum_name.to_string());
        };
        let positional = args.iter().filter(|a| a.name.is_none()).count();
        if positional > v.fields.len() {
            self.error(
                ErrorCode::ZAP4062,
                format!(
                    "variant '{enum_name}.{variant}' takes {} field(s), found {}",
                    v.fields.len(),
                    positional
                ),
                span,
            );
        }
        let mut pos = 0usize;
        for a in args {
            let at = self.infer(&a.value);
            let field = match &a.name {
                Some(fname) => {
                    let f = v.fields.iter().find(|(n, _)| n == fname);
                    if f.is_none() {
                        self.error(
                            ErrorCode::ZAP4063,
                            format!("variant '{enum_name}.{variant}' has no field '{fname}'"),
                            expr_span(&a.value),
                        );
                    }
                    f
                }
                None => {
                    let f = v.fields.get(pos);
                    pos += 1;
                    f
                }
            };
            if let Some((fname, fty)) = field {
                if !assignable(&at, fty) {
                    self.error(
                        ErrorCode::ZAP4064,
                        format!(
                            "field '{fname}' of '{enum_name}.{variant}' has type {}, found {}",
                            fty.display(),
                            at.display()
                        ),
                        expr_span(&a.value),
                    );
                }
            }
        }
        Ty::Named(enum_name.to_string())
    }

    /// Type-check one `match` arm's pattern against the scrutinee type,
    /// declaring its bindings into the current scope. Returns the set of enum
    /// variant names this pattern covers (for exhaustiveness) and whether it is
    /// an irrefutable catch-all (`_` or a bare capture).
    fn check_pattern(&mut self, p: &Pattern, scrut: &Ty) -> (Vec<String>, bool) {
        match p {
            Pattern::Wildcard(_) => (Vec::new(), true),
            Pattern::Capture { name, span } => {
                self.declare(name, scrut.clone(), *span);
                (Vec::new(), true)
            }
            Pattern::Literal(e) => {
                self.infer(e);
                (Vec::new(), false)
            }
            Pattern::Or(ps, _) => {
                let mut covered = Vec::new();
                let mut catch_all = false;
                for sub in ps {
                    let (c, ca) = self.check_pattern(sub, scrut);
                    covered.extend(c);
                    catch_all |= ca;
                }
                (covered, catch_all)
            }
            Pattern::Sequence(ps, span) | Pattern::Tuple(ps, span) => {
                if ps
                    .iter()
                    .filter(|p| matches!(p, Pattern::Rest { .. }))
                    .count()
                    > 1
                {
                    self.error(
                        ErrorCode::ZAP3001,
                        "a pattern may contain only one rest capture".to_string(),
                        *span,
                    );
                }
                for sub in ps {
                    self.check_pattern(sub, &Ty::Unknown);
                }
                (Vec::new(), false)
            }
            Pattern::Rest {
                name: Some(n),
                span,
            } => {
                self.declare(n, Ty::Unknown, *span);
                (Vec::new(), false)
            }
            Pattern::Rest { name: None, .. } => (Vec::new(), false),
            Pattern::Struct { path, args, span } => {
                self.check_qualified_pattern(path, args.as_deref(), *span, scrut)
            }
            Pattern::As {
                pattern,
                name,
                span,
            } => {
                // Match the inner pattern (declaring its own captures), then
                // bind `name` to the whole matched value, typed as the
                // scrutinee for this arm (D11, #603). The capture is in scope in
                // the arm body and its guard because both are checked in the
                // scope this populates. The `as` wrapper does not change which
                // enum variants the arm covers, nor its catch-all status (`_ as
                // n` / `x as n` stay irrefutable).
                let (covered, catch_all) = self.check_pattern(pattern, scrut);
                self.declare(name, scrut.clone(), *span);
                (covered, catch_all)
            }
        }
    }

    /// Type a qualified/struct pattern: an enum member/variant (`Color.RED`,
    /// `Shape.Circle(r)`) or a plain struct pattern (`Point(x=0)`). Binds
    /// payload fields positionally or by name (D-0046). Returns covered enum
    /// variants and whether it is a catch-all (never, for these).
    fn check_qualified_pattern(
        &mut self,
        path: &[String],
        args: Option<&[PatArg]>,
        span: Span,
        scrut: &Ty,
    ) -> (Vec<String>, bool) {
        // Qualified `Enum.Variant`: the head names an enum type.
        if path.len() == 2 {
            if let Some(info) = self.enums.get(&path[0]).cloned() {
                let (enum_name, variant) = (&path[0], &path[1]);
                // The scrutinee must be this enum (keep going to bind on error).
                if let Ty::Named(sn) = scrut {
                    if sn != enum_name {
                        self.error(
                            ErrorCode::ZAP5081,
                            format!(
                                "pattern '{enum_name}.{variant}' does not match a value of \
                                 type {}",
                                scrut.display()
                            ),
                            span,
                        );
                    }
                }
                let Some(v) = info.variant(variant) else {
                    self.error(
                        ErrorCode::ZAP4060,
                        format!("enum '{enum_name}' has no variant '{variant}'"),
                        span,
                    );
                    return (Vec::new(), false);
                };
                self.bind_variant_fields(enum_name, v, args, span);
                let covers_payload = args.map_or(true, |args| {
                    args.iter().all(|a| pattern_is_irrefutable(&a.pattern))
                });
                return (
                    if covers_payload {
                        vec![variant.clone()]
                    } else {
                        Vec::new()
                    },
                    false,
                );
            }
        }
        // A plain struct pattern (`Point(x=0, y=y)`) or an unresolved qualified
        // name: typed loosely (struct-field typing is not modeled yet), but its
        // bindings are still declared so arm bodies type-check.
        if let Some(args) = args {
            for a in args {
                self.check_pattern(&a.pattern, &Ty::Unknown);
            }
        }
        (Vec::new(), false)
    }

    /// Declare the bindings of a variant pattern's payload fields, matching them
    /// positionally or by name and binding each capture to the field's type.
    fn bind_variant_fields(
        &mut self,
        enum_name: &str,
        v: &EnumVariantInfo,
        args: Option<&[PatArg]>,
        span: Span,
    ) {
        let Some(args) = args else {
            return; // `case Shape.Unit:` — no fields to bind.
        };
        let positional = args.iter().filter(|a| a.name.is_none()).count();
        if positional > v.fields.len() {
            self.error(
                ErrorCode::ZAP5082,
                format!(
                    "pattern '{enum_name}.{}' binds {} field(s) but the variant has {}",
                    v.name,
                    positional,
                    v.fields.len()
                ),
                span,
            );
        }
        let mut pos = 0usize;
        for a in args {
            let fty = match &a.name {
                Some(fname) => match v.fields.iter().find(|(n, _)| n == fname) {
                    Some((_, t)) => t.clone(),
                    None => {
                        self.error(
                            ErrorCode::ZAP4063,
                            format!("variant '{enum_name}.{}' has no field '{fname}'", v.name),
                            a.span,
                        );
                        Ty::Unknown
                    }
                },
                None => {
                    let t = v
                        .fields
                        .get(pos)
                        .map(|(_, t)| t.clone())
                        .unwrap_or(Ty::Unknown);
                    pos += 1;
                    t
                }
            };
            self.check_pattern(&a.pattern, &fty);
        }
    }
}

// Only these payload patterns cover every value of a variant field.
fn pattern_is_irrefutable(pattern: &Pattern) -> bool {
    match pattern {
        Pattern::Wildcard(_) | Pattern::Capture { .. } => true,
        Pattern::Or(ps, _) => ps.iter().any(pattern_is_irrefutable),
        Pattern::As { pattern, .. } => pattern_is_irrefutable(pattern),
        _ => false,
    }
}

/// `StopIteration` or `StopIteration()` — the iterator exhaustion signal
/// (D-0048), a compiler-known exception name.
fn is_stop_iteration(e: &Expr) -> bool {
    match e {
        Expr::Name(n, _) => n == "StopIteration",
        Expr::Call { callee, .. } => {
            matches!(callee.as_ref(), Expr::Name(n, _) if n == "StopIteration")
        }
        _ => false,
    }
}

/// `expect_raises(ErrType)` — the exception-assertion context manager (D-0054).
fn is_expect_raises(e: &Expr) -> bool {
    matches!(e, Expr::Call { callee, .. }
        if matches!(callee.as_ref(), Expr::Name(n, _) if n == "expect_raises"))
}

/// `OnError.Cancel` / `OnError.Wait` — the typed `scope(on_error=...)` policy
/// (D-0044). Recognized structurally: the `OnError` enum is a compiler-known
/// policy in this increment (a real stdlib enum is a follow-up), so a misspelled
/// member or a non-`OnError` value is rejected by [`check_scope_opts`].
fn is_on_error_policy(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Attr { base, name, .. }
            if matches!(base.as_ref(), Expr::Name(n, _) if n == "OnError")
                && (name == "Cancel" || name == "Wait")
    )
}

/// The element type an iterator's `__next__` yields (D-0048): the comma-ok form
/// `-> (T, bool)` yields `T`; the plain `-> T` (StopIteration) form yields `T`.
fn next_elem(ret: &Ty) -> Ty {
    if let Ty::Tuple(ts) = ret {
        if ts.len() == 2 && matches!(ts[1], Ty::Bool) {
            return ts[0].clone();
        }
    }
    ret.clone()
}

fn expr_span(e: &Expr) -> Span {
    match e {
        Expr::Int(_, s)
        | Expr::Float(_, s)
        | Expr::Imaginary(_, s)
        | Expr::Bool(_, s)
        | Expr::None(s)
        | Expr::Name(_, s)
        | Expr::Tuple(_, s)
        | Expr::List(_, s)
        | Expr::Dict(_, s)
        | Expr::Set(_, s) => *s,
        Expr::Str { span, .. }
        | Expr::Starred { span, .. }
        | Expr::ListComp { span, .. }
        | Expr::DictComp { span, .. }
        | Expr::SetComp { span, .. }
        | Expr::GenExp { span, .. }
        | Expr::Unary { span, .. }
        | Expr::Binary { span, .. }
        | Expr::Compare { span, .. }
        | Expr::Call { span, .. }
        | Expr::Index { span, .. }
        | Expr::Slice { span, .. }
        | Expr::SliceSpec { span, .. }
        | Expr::Attr { span, .. }
        | Expr::Lambda { span, .. }
        | Expr::RegexMatch { span, .. }
        | Expr::RegexSubst { span, .. }
        | Expr::ChanRecv { span, .. }
        | Expr::Yield { span, .. }
        | Expr::IfElse { span, .. }
        | Expr::Named { span, .. } => *span,
    }
}

/// The number of bits needed to hold every discriminant of a tier-1 enum as an
/// **unsigned** value, or `None` if any discriminant is negative (so the enum
/// cannot be an unsigned bitfield). Discriminants follow the C rule: an explicit
/// value, else the previous + 1 from 0 (I#173/D-0068).
fn enum_discriminant_bits(e: &EnumDef) -> Option<u64> {
    let mut next: i64 = 0;
    let mut max: i64 = 0;
    for v in &e.variants {
        let val = v.value.as_ref().and_then(const_int_lit).unwrap_or(next);
        if val < 0 {
            return None;
        }
        max = max.max(val);
        next = val.wrapping_add(1);
    }
    Some(bits_for(max as u64))
}

/// Bits needed to represent `n` as an unsigned value (at least 1).
fn bits_for(n: u64) -> u64 {
    u64::from(64 - n.leading_zeros()).max(1)
}

/// Evaluate a simple constant integer expression (an int literal, optionally
/// signed) for an enum discriminant; `None` for anything more complex.
fn const_int_lit(e: &Expr) -> Option<i64> {
    match e {
        Expr::Int(s, _) => parse_int_lit(s),
        Expr::Unary {
            op: UnOp::Neg,
            expr,
            ..
        } => const_int_lit(expr).map(|v| -v),
        Expr::Unary {
            op: UnOp::Pos,
            expr,
            ..
        } => const_int_lit(expr),
        _ => None,
    }
}

/// Fold an integer constant expression for the arena `size`/`cap` static check
/// (#553, `ZAP5132`): integer literals, unary `+`/`-`, and the common arithmetic
/// / bitwise binary operators (`*`, `<<`, `+`, `-`, `/`, `%`, `&`, `|`, `^`,
/// `>>`) so `64*1024` and `1<<20` fold. Returns `None` for anything not provably
/// constant (a runtime value), so the static check never fires on a value the
/// compiler cannot evaluate. Wrapping arithmetic matches the runtime's i64 wrap.
fn arena_const_int(e: &Expr) -> Option<i64> {
    match e {
        Expr::Int(..) | Expr::Unary { .. } => const_int_lit(e),
        Expr::Binary { op, lhs, rhs, .. } => {
            let a = arena_const_int(lhs)?;
            let b = arena_const_int(rhs)?;
            match op {
                BinOp::Add => Some(a.wrapping_add(b)),
                BinOp::Sub => Some(a.wrapping_sub(b)),
                BinOp::Mul => Some(a.wrapping_mul(b)),
                BinOp::Div | BinOp::FloorDiv => (b != 0).then(|| a.wrapping_div(b)),
                BinOp::Mod => (b != 0).then(|| a.wrapping_rem(b)),
                BinOp::BitAnd => Some(a & b),
                BinOp::BitOr => Some(a | b),
                BinOp::BitXor => Some(a ^ b),
                BinOp::Shl => (0..64).contains(&b).then(|| a.wrapping_shl(b as u32)),
                BinOp::Shr => (0..64).contains(&b).then(|| a.wrapping_shr(b as u32)),
                _ => None,
            }
        }
        _ => None,
    }
}

/// A compile-time numeric literal (int or float), with an optional leading
/// unary `+`/`-`, as an `f64`. Used to validate `@range`/`@validate` bounds
/// (D-0101). `None` for any non-literal expression.
fn numeric_literal(e: &Expr) -> Option<f64> {
    match e {
        Expr::Int(s, _) => parse_int_lit(s).map(|v| v as f64),
        Expr::Float(s, _) => s.replace('_', "").parse::<f64>().ok(),
        Expr::Unary {
            op: UnOp::Neg,
            expr,
            ..
        } => numeric_literal(expr).map(|v| -v),
        Expr::Unary {
            op: UnOp::Pos,
            expr,
            ..
        } => numeric_literal(expr),
        _ => None,
    }
}

/// Parse an integer literal string (decimal/hex/octal/binary with `_`
/// separators) to an `i64`; `None` if it does not parse.
fn parse_int_lit(s: &str) -> Option<i64> {
    let t = s.replace('_', "");
    let (radix, digits) = if let Some(r) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        (16, r)
    } else if let Some(r) = t.strip_prefix("0o").or_else(|| t.strip_prefix("0O")) {
        (8, r)
    } else if let Some(r) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) {
        (2, r)
    } else {
        (10, t.as_str())
    };
    i64::from_str_radix(digits, radix).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn errors(src: &str) -> Vec<TypeError> {
        let (terrs, perrs, lerrs) = check_str(src);
        assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
        assert!(perrs.is_empty(), "parse errors: {perrs:?}");
        terrs
    }

    fn ok(src: &str) {
        let e = errors(src);
        assert!(e.is_empty(), "unexpected type errors: {e:?}");
    }

    // --- AtomicArray[T] (#844) -------------------------------------------------

    #[test]
    fn concurrent_set_and_stack_type_check() {
        // ConcurrentSet[K] (wrapping the map) and the Treiber ConcurrentStack[T],
        // monomorphized; element types come from the annotations.
        ok("def main():\n    \
            ConcurrentSet[int64] s = new_concurrent_set()\n    \
            s.add(1)\n    \
            auto c = s.contains(1)\n    \
            auto r = s.remove(1)\n    \
            auto n = s.length()\n    \
            ConcurrentStack[int64] st = new_concurrent_stack()\n    \
            st.push(5)\n    \
            auto v, ok = st.pop()\n");
    }

    #[test]
    fn concurrent_map_surface_type_checks() {
        // The full ConcurrentMap[K, V] surface monomorphized at K=str, V=int64
        // (the element types come from the annotation, as with `new_queue`).
        ok("def main():\n    \
            ConcurrentMap[str, int64] m = new_concurrent_map()\n    \
            m.put(\"a\", 1)\n    \
            auto v, ok = m.get(\"a\")\n    \
            auto c = m.contains(\"a\")\n    \
            auto d = m.delete(\"a\")\n    \
            auto n = m.length()\n    \
            auto g = m.get_or(\"b\", 9)\n");
    }

    #[test]
    fn atomicarray_int64_surface_type_checks() {
        // The full word-element surface: len()->int64, store(i,v), load(i)->int64,
        // compare_and_swap(i,old,new)->bool. All clean.
        ok("def main():\n    \
            AtomicArray[int64] a = AtomicArray[int64](8)\n    \
            auto n = a.len()\n    \
            a.store(0, 5)\n    \
            auto v = a.load(0)\n    \
            auto swapped = a.compare_and_swap(0, 5, 6)\n");
    }

    #[test]
    fn atomicarray_reference_element_load_is_typed_as_element() {
        // A reference element: load() returns the element type, so a field access
        // on the result type-checks (it would not if load returned Unknown/ptr).
        ok("struct Node:\n    int64 v\n\
            def main():\n    \
            AtomicArray[Node] a = AtomicArray[Node](2)\n    \
            a.store(0, Node(v=1))\n    \
            auto x = a.load(0)\n    \
            print(x.v)\n");
    }

    #[test]
    fn atomicarray_rejects_unsupported_element() {
        // v1 supports reference elements or int64; a float element is rejected.
        let e = errors("def main():\n    AtomicArray[float64] a = AtomicArray[float64](4)\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP4028),
            "float element must be rejected (ZAP4028), got {e:?}"
        );
    }

    #[test]
    fn atomicarray_load_arity_is_checked() {
        let e = errors(
            "def main():\n    \
            AtomicArray[int64] a = AtomicArray[int64](4)\n    \
            auto v = a.load()\n",
        );
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP4025),
            "load() with no index must error (ZAP4025), got {e:?}"
        );
    }

    #[test]
    fn atomicarray_unknown_method_is_rejected() {
        let e = errors(
            "def main():\n    \
            AtomicArray[int64] a = AtomicArray[int64](4)\n    \
            a.frobnicate(0)\n",
        );
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP4080),
            "unknown method must error (ZAP4080), got {e:?}"
        );
    }

    #[test]
    fn atomicarray_capacity_must_be_integer() {
        let e = errors(
            "def main():\n    \
            AtomicArray[int64] a = AtomicArray[int64](\"nope\")\n",
        );
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP4033),
            "non-integer capacity must error (ZAP4033), got {e:?}"
        );
    }

    #[test]
    fn iface_list_element_literal_is_accepted() {
        // #829: a heterogeneous list of conformers (distinct types) typed as
        // `list[Iface]` is admitted — the user-facing heterogeneous-collection
        // need. (Lowering boxes each element into the fat pointer; see the ir
        // crate's walled-garden tests.)
        ok("interface Shape:\n    def sides(self) -> int64\n\n\
            struct Tri:\n    def sides(self) -> int64:\n        return 3\n\n\
            struct Quad:\n    def sides(self) -> int64:\n        return 4\n\n\
            def main():\n    list[Shape] xs = [Tri(), Quad()]\n    print(xs[0].sides())\n");
    }

    #[test]
    fn append_conformer_to_iface_list_is_accepted() {
        // #829: appending a concrete conformer to a `list[Iface]` type-checks.
        ok(
            "interface Shape:\n    def sides(self) -> int64\n\n\
            struct Tri:\n    def sides(self) -> int64:\n        return 3\n\n\
            def main():\n    list[Shape] xs = []\n    xs.append(Tri())\n    print(xs[0].sides())\n",
        );
    }

    #[test]
    fn non_conforming_list_element_is_rejected() {
        // #829 negative: a value that does NOT provide the interface's methods is
        // rejected when the list is typed as `list[Iface]` — the conformance
        // guard is not bypassed by the element-boxing path.
        let m = msgs(
            "interface Shape:\n    def sides(self) -> int64\n\n\
             struct Bad:\n    def nope(self) -> int64:\n        return 0\n\n\
             def main():\n    list[Shape] xs = [Bad(), Bad()]\n",
        );
        assert!(
            m.contains("Shape") && m.contains("Bad"),
            "a non-conforming element is rejected, naming the types; got:\n{m}"
        );
    }

    #[test]
    fn refined_interface_binding_accepts_conformer_with_inherited_methods() {
        // #830: a struct providing BOTH the refined interface's own method and its
        // inherited supertype method conforms and is usable as an interface value;
        // the inherited method call type-checks (its return type resolves through
        // the supertype).
        ok("interface Named:\n    def name(self) -> int64\n\n\
            interface Ranked: Named\n    def rank(self) -> int64\n\n\
            struct Color:\n    def name(self) -> int64:\n        return 0\n\
            \x20   def rank(self) -> int64:\n        return 9\n\n\
            def main():\n    Ranked r = Color()\n    print(r.rank())\n    print(r.name())\n");
    }

    #[test]
    fn refined_interface_binding_missing_inherited_method_is_rejected() {
        // #830 negative: a type missing the INHERITED supertype method does not
        // conform to the refined interface, and the diagnostic names the missing
        // method and the supertype that introduced it (D-0139).
        let m = msgs(
            "interface Named:\n    def name(self) -> int64\n\n\
             interface Ranked: Named\n    def rank(self) -> int64\n\n\
             struct OnlyRank:\n    def rank(self) -> int64:\n        return 9\n\n\
             def main():\n    Ranked r = OnlyRank()\n",
        );
        assert!(
            m.contains("missing method 'name'") && m.contains("supertype 'Named'"),
            "missing inherited method names the supertype; got:\n{m}"
        );
    }

    // ---- #831: interface conformers at collection VALUE positions ---------
    //
    // `coercible`/`assignable` only lifted a TOP-LEVEL `struct/enum -> interface`
    // conversion (D-0016). These exercise the clarification that the same
    // conversion is admitted one level in, at collection value positions (list
    // element, dict value) — wherever `assignable` already permits collection
    // covariance — while keys stay exact, and the heterogeneous-`Unknown`
    // literal slip is closed (a non-conformer is a clean ZAP4001, not a codegen
    // crash). IR boxing is the #829/#832 counterpart; see the ir crate tests.

    #[test]
    fn single_conformer_list_element_is_accepted() {
        // The core repro: a SINGLE conformer keeps its concrete element type
        // (`list[Tri]`, not widened to Unknown), so it exercises the `coercible`
        // descent — not the old Unknown bypass that only a heterogeneous literal
        // triggered. The element dispatches as an interface value.
        ok(r#"
interface Shape:
    def sides(self) -> int

struct Tri:
    def sides(self) -> int:
        return 3

def main():
    list[Shape] xs = [Tri()]
    print(xs[0].sides())
"#);
    }

    #[test]
    fn single_conformer_dict_value_is_accepted() {
        // The dict VALUE position admits a conformer (`dict[int, Tri]` ->
        // `dict[int, Shape]`); the key type is exact (`int`).
        ok(r#"
interface Shape:
    def sides(self) -> int

struct Tri:
    def sides(self) -> int:
        return 3

def main():
    dict[int, Shape] d = {1: Tri()}
    print(d[1].sides())
"#);
    }

    #[test]
    fn tier3_enum_conformer_list_element_is_accepted() {
        // A tier-3 (data-carrying) enum conformer as a `list[Iface]` element
        // (ties to #731/#828: enums conform structurally).
        ok(r#"
interface Node:
    def rank(self) -> int

enum Expr:
    Lit(int v)
    Add(int l, int r)

    def rank(self) -> int:
        return 1

def main():
    list[Node] xs = [Expr.Lit(v=5)]
    print(xs[0].rank())
"#);
    }

    #[test]
    fn tier1_enum_conformer_list_element_is_accepted() {
        // A tier-1 (unboxed-integer) enum conformer as a `list[Iface]` element;
        // lowering boxes the scalar tag into the fat pointer (#731/#829).
        ok(r#"
interface Ranked:
    def rank(self) -> int

enum Color:
    Red
    Green
    Blue

    def rank(self) -> int:
        return 7

def main():
    list[Ranked] xs = [Color.Green]
    print(xs[0].rank())
"#);
    }

    #[test]
    fn refined_supertype_list_element_is_accepted() {
        // #831 + D-0139/#806: the declared element is a REFINED interface, and a
        // type conforming through the transitive supertype is admitted as an
        // element; both the own and the inherited method dispatch.
        ok(r#"
interface Named:
    def name(self) -> int

interface Ranked: Named
    def rank(self) -> int

struct Color:
    def name(self) -> int:
        return 0
    def rank(self) -> int:
        return 9

def main():
    list[Ranked] xs = [Color()]
    print(xs[0].name())
    print(xs[0].rank())
"#);
    }

    #[test]
    fn nested_list_of_iface_is_accepted() {
        // The descent recurses: `list[list[Shape]]` admits a conformer at the
        // innermost value position.
        ok(r#"
interface Shape:
    def sides(self) -> int

struct Tri:
    def sides(self) -> int:
        return 3

def main():
    list[list[Shape]] xs = [[Tri()]]
    print(xs[0][0].sides())
"#);
    }

    #[test]
    fn heterogeneous_conformer_dict_values_are_accepted() {
        // Distinct conformer VALUES widen to `dict[int, Unknown]`; the re-check
        // admits them by real conformance (not the Unknown bypass).
        ok(r#"
interface Shape:
    def sides(self) -> int

struct Tri:
    def sides(self) -> int:
        return 3

struct Quad:
    def sides(self) -> int:
        return 4

def main():
    dict[int, Shape] d = {1: Tri(), 2: Quad()}
    print(d[1].sides())
"#);
    }

    #[test]
    fn iface_list_returned_single_conformer_is_accepted() {
        // The descent applies at the RETURN position too.
        ok(r#"
interface Shape:
    def sides(self) -> int

struct Tri:
    def sides(self) -> int:
        return 3

def make() -> list[Shape]:
    return [Tri()]

def main():
    print(make()[0].sides())
"#);
    }

    #[test]
    fn single_non_conformer_list_element_is_rejected() {
        // A single non-conformer keeps its concrete type (`list[Bad]`) and is
        // rejected with the conformance reason descended into the element.
        let m = msgs(
            r#"
interface Shape:
    def sides(self) -> int

struct Bad:
    def nope(self) -> int:
        return 0

def main():
    list[Shape] xs = [Bad()]
"#,
        );
        assert!(
            m.contains("Bad") && m.contains("Shape") && m.contains("does not satisfy"),
            "single non-conformer rejected with reason; got:\n{m}"
        );
    }

    #[test]
    fn heterogeneous_non_conformers_list_is_rejected_cleanly() {
        // THE secondary hazard (#831): `[Bad(), Other()]` joins to
        // `list[Unknown]`, which used to slip sema and then crash lowering
        // (`'Bad__sides' not implemented`). It must now be a clean ZAP4001 at the
        // binding — a sema error, never a codegen crash.
        let m = msgs(
            r#"
interface Shape:
    def sides(self) -> int

struct Bad:
    def nope(self) -> int:
        return 0

struct Other:
    def meh(self) -> int:
        return 1

def main():
    list[Shape] xs = [Bad(), Other()]
"#,
        );
        assert!(
            m.contains("does not conform") && m.contains("Shape"),
            "heterogeneous non-conformers rejected naming the interface; got:\n{m}"
        );
        assert!(
            !m.contains("not implemented"),
            "must be a sema error, not a codegen crash; got:\n{m}"
        );
    }

    #[test]
    fn mixed_conformer_and_non_conformer_list_is_rejected() {
        // `[Tri(), Bad()]` -> `list[Unknown]`: the re-check admits `Tri` and
        // rejects only `Bad`, so a genuine conformer is never flagged.
        let m = msgs(
            r#"
interface Shape:
    def sides(self) -> int

struct Tri:
    def sides(self) -> int:
        return 3

struct Bad:
    def nope(self) -> int:
        return 0

def main():
    list[Shape] xs = [Tri(), Bad()]
"#,
        );
        assert!(
            m.contains("Bad") && m.contains("does not conform"),
            "the non-conforming element is rejected; got:\n{m}"
        );
        assert!(
            !m.contains("type Tri does not conform"),
            "the genuine conformer must NOT be flagged; got:\n{m}"
        );
    }

    #[test]
    fn dict_value_non_conformer_is_rejected() {
        let m = msgs(
            r#"
interface Shape:
    def sides(self) -> int

struct Bad:
    def nope(self) -> int:
        return 0

def main():
    dict[int, Shape] d = {1: Bad()}
"#,
        );
        assert!(
            m.contains("Shape") && m.contains("Bad"),
            "a non-conforming dict value is rejected; got:\n{m}"
        );
    }

    #[test]
    fn heterogeneous_non_conformer_dict_values_are_rejected_cleanly() {
        // The dict-value analogue of the hazard: `{1: Bad(), 2: Other()}` widens
        // to `dict[int, Unknown]` and must be rejected, not slipped.
        let m = msgs(
            r#"
interface Shape:
    def sides(self) -> int

struct Bad:
    def nope(self) -> int:
        return 0

struct Other:
    def meh(self) -> int:
        return 1

def main():
    dict[int, Shape] d = {1: Bad(), 2: Other()}
"#,
        );
        assert!(
            m.contains("does not conform") && m.contains("Shape"),
            "heterogeneous non-conformer dict values rejected; got:\n{m}"
        );
        assert!(
            !m.contains("not implemented"),
            "must be a sema error, not a codegen crash; got:\n{m}"
        );
    }

    #[test]
    fn iface_dict_key_conformer_is_rejected_keys_stay_exact() {
        // VALUE positions gain conformance, but KEYS stay exact: an interface
        // value is unhashable, so a conformer at a `dict` KEY is still rejected.
        let m = msgs(
            r#"
interface Shape:
    def sides(self) -> int

struct Tri:
    def sides(self) -> int:
        return 3

def main():
    dict[Shape, int] d = {Tri(): 1}
"#,
        );
        assert!(
            !m.is_empty(),
            "a conformer at a dict key is rejected (keys stay exact); got:\n{m}"
        );
    }

    #[test]
    fn iface_set_member_conformer_is_rejected_members_stay_exact() {
        // A `set` member is a hash position too, so a conformer set member is
        // still rejected (consistent with dict keys, per the maintainer ruling).
        let m = msgs(
            r#"
interface Shape:
    def sides(self) -> int

struct Tri:
    def sides(self) -> int:
        return 3

def main():
    set[Shape] s = {Tri()}
"#,
        );
        assert!(
            !m.is_empty(),
            "a conformer at a set member is rejected (members stay exact); got:\n{m}"
        );
    }

    #[test]
    fn empty_list_into_iface_list_is_accepted() {
        // An empty literal infers `list[Unknown]` with no element expressions;
        // the re-check loops zero times, so it stays accepted (then `append`
        // supplies the conformers).
        ok(r#"
interface Shape:
    def sides(self) -> int

struct Tri:
    def sides(self) -> int:
        return 3

def main():
    list[Shape] xs = []
    xs.append(Tri())
    print(xs[0].sides())
"#);
    }

    // ---- crypto asymmetric-key surface (#643, D-0126) ---------------------

    #[test]
    fn crypto_asym_surface_type_checks() {
        // key_info returns the KeyInfo value struct; its fields carry the right
        // types; imports/sign return bytes and verify returns bool.
        ok(r#"def main():
    auto rsa_pub = crypto.rsa_public_key_pem(b"n", b"e")
    auto ec_pub = crypto.p256_public_key_pem(b"x", b"y")
    auto ed_pub = crypto.ed25519_public_key_pem(b"x")
    auto info = crypto.key_info(rsa_pub)
    str kind = info.kind
    int64 bits = info.bits
    str curve = info.curve
    bool priv = info.is_private
    bytes es = crypto.p256_sha256_sign("pem", "msg")
    bool ok1 = crypto.p256_sha256_verify(ec_pub, "msg", es)
    bytes ed = crypto.ed25519_sign("pem", b"msg")
    bool ok2 = crypto.ed25519_verify(ed_pub, b"msg", ed)
    assert ok1 and ok2
"#);
    }

    #[test]
    fn crypto_asym_arity_is_checked() {
        // Wrong argument counts are rejected (ZAP4025), one per entry point.
        for bad in [
            "crypto.key_info(\"a\", \"b\")",
            "crypto.rsa_public_key_pem(b\"n\")",
            "crypto.p256_public_key_pem(b\"x\")",
            "crypto.ed25519_public_key_pem(b\"x\", b\"y\")",
            "crypto.p256_sha256_sign(\"pem\")",
            "crypto.ed25519_sign(\"pem\")",
            "crypto.p256_sha256_verify(\"pem\", \"msg\")",
            "crypto.ed25519_verify(\"pem\", \"msg\")",
        ] {
            let src = format!("def main():\n    auto x = {bad}\n");
            let e = errors(&src);
            assert!(!e.is_empty(), "expected an arity error for `{bad}`");
        }
    }

    // ---- transparent type aliases (#606, D-0105) --------------------------

    #[test]
    fn type_alias_is_transparent_to_its_target() {
        // A plain alias resolves to its target everywhere: a `Count`-typed
        // function body and a spelled-out `int64` one behave identically.
        ok(r#"
type Count = int64
def total() -> Count:
    return 42
"#);
        // Interchangeable: an alias names the same type as its target, so a
        // value produced at one spelling flows into the other, and the alias is
        // usable as a collection element (`list[Row]` / `dict[str, Row]`).
        ok(r#"
type Row = (int64, str)
def make() -> Row:
    return (1, "a")
def first(Row r) -> int64:
    return r[0]
def main():
    auto p = make()
    auto n = first(p)
    list[Row] rows = []
    dict[str, Row] table = {}
"#);
    }

    #[test]
    fn type_alias_resolves_to_target_for_diagnostics() {
        // Proof of transparent identity: returning a `str` from a `Count`-typed
        // (`= int64`) function is the SAME return-type mismatch (ZAP4003) as
        // returning a `str` where `int64` is spelled out — the alias carries no
        // identity of its own.
        let e = errors("type Count = int64\ndef f() -> Count:\n    return \"nope\"\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP4003),
            "alias must resolve to int64 for the mismatch, got {e:?}"
        );
    }

    #[test]
    fn generic_type_alias_resolves_on_instantiation() {
        // `type Boxed[T] = T` instantiated as `Boxed[int64]` resolves to `int64`,
        // so returning a `str` is the int64 return-type mismatch (ZAP4003).
        let e = errors("type Boxed[T] = T\ndef f() -> Boxed[int64]:\n    return \"nope\"\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP4003),
            "generic alias Boxed[int64] must resolve to int64, got {e:?}"
        );
        // And the matching return is accepted.
        ok("type Pair[T] = (T, T)\ndef g() -> Pair[int64]:\n    return (1, 2)\n");
    }

    // ---- module-level `const` (D-0112, #658 Phase 1) ----------------------

    #[test]
    fn module_const_scalar_is_visible_in_functions() {
        // A scalar module `const` resolves and types inside any function, and
        // regardless of source order (declared up front in the pre-pass).
        ok("const int64 N = 7\ndef main() -> int64:\n    return N\n");
        // Function declared BEFORE the const still sees it.
        ok("def use() -> int64:\n    return N\nconst int64 N = 7\ndef main() -> int64:\n    return use()\n");
    }

    #[test]
    fn module_const_reference_type_is_visible_in_functions() {
        // A reference-typed (heap) module `const` is usable across functions.
        ok("const list[int64] XS = [1, 2, 3]\ndef total() -> int64:\n    auto s = 0\n    for x in XS:\n        s += x\n    return s\ndef main() -> int64:\n    return total()\n");
        ok("const dict[str, int64] T = {\"a\": 1}\ndef get() -> int64:\n    return T[\"a\"]\ndef main() -> int64:\n    return get()\n");
    }

    #[test]
    fn module_const_auto_infers_type() {
        // `const auto` infers from the initializer, usable like any binding.
        ok("const auto PI = 3.14\ndef main() -> f64:\n    return PI\n");
        ok("const auto NAME = \"zap\"\ndef greet() -> str:\n    return NAME\ndef main() -> int64:\n    return 0\n");
    }

    #[test]
    fn module_const_initializer_may_reference_earlier_const() {
        // Source-order init: a later const reads an earlier one (and a
        // const-returning function). This is the eager-init dependency chain.
        ok("const int64 A = 10\nconst int64 B = A + 5\ndef main() -> int64:\n    return B\n");
        ok("def base() -> int64:\n    return 2\nconst int64 C = base() * 3\ndef main() -> int64:\n    return C\n");
    }

    #[test]
    fn module_const_type_mismatch_is_rejected() {
        // The declared type must accept the initializer (ZAP4001), exactly as a
        // local `const`/`let` of the same type would.
        let e = errors("const int64 N = \"hi\"\ndef main() -> int64:\n    return 0\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP4001),
            "expected ZAP4001, got {e:?}"
        );
    }

    #[test]
    fn module_const_reassignment_is_rejected() {
        // A module `const` is immutable: reassigning it from inside a function
        // is ZAP3017.
        let e = errors("const int64 N = 1\ndef main() -> int64:\n    N = 2\n    return N\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP3017),
            "expected ZAP3017, got {e:?}"
        );
        // Augmented assignment to a module `const` is equally rejected.
        let e = errors("const int64 N = 1\ndef main() -> int64:\n    N += 2\n    return N\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP3017),
            "expected ZAP3017 for `+=`, got {e:?}"
        );
    }

    #[test]
    fn local_const_reassignment_is_rejected() {
        // The same immutability holds for a `const` local (D-0003) — plain,
        // augmented, and chained assignment all raise ZAP3017.
        let e = errors("def main() -> int64:\n    const int64 x = 1\n    x = 2\n    return x\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP3017),
            "expected ZAP3017, got {e:?}"
        );
        let e = errors("def main() -> int64:\n    const int64 x = 1\n    x += 2\n    return x\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP3017),
            "expected ZAP3017 for `+=`, got {e:?}"
        );
        let e = errors(
            "def main() -> int64:\n    const int64 x = 1\n    auto y = 0\n    y = x = 3\n    return x\n",
        );
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP3017),
            "expected ZAP3017 for chained assignment, got {e:?}"
        );
    }

    #[test]
    fn mutable_binding_reassignment_is_still_allowed() {
        // Immutability is a `const`-only rule — a plain (mutable) binding is
        // freely reassignable, and a `const` in an inner scope shadowing a
        // mutable outer binding does not freeze the outer one.
        ok("def main() -> int64:\n    int64 x = 1\n    x = 2\n    x += 3\n    return x\n");
        // A fresh `const` may shadow a mutable binding of the same name in a
        // nested block without error; the inner const is immutable, the outer
        // binding stays mutable after the block.
        ok("def main() -> int64:\n    int64 x = 1\n    if True:\n        const int64 x = 9\n    x = 5\n    return x\n");
    }

    // ---- module-level mutable globals (D-0112, #662 Phase 2) --------------

    #[test]
    fn module_mutable_scalar_is_reassignable_in_functions() {
        // A mutable module global (no `const`) is persistent state that any
        // function may read AND reassign — plain, chained, and augmented — with
        // no ZAP3017 (that is a `const`-only rule). Declared up front, so a
        // function declared before it still resolves it.
        ok("int64 N = 0\ndef bump():\n    N = N + 1\ndef main() -> int64:\n    bump()\n    return N\n");
        ok("def use() -> int64:\n    N = 5\n    return N\nint64 N = 0\ndef main() -> int64:\n    return use()\n");
        ok("int64 N = 0\ndef main() -> int64:\n    N += 3\n    return N\n");
    }

    #[test]
    fn module_mutable_reference_type_mutation_and_reassign() {
        // A reference-typed mutable global can have its contents mutated and be
        // wholly reassigned across functions.
        ok("list[int64] LOG = []\ndef add(int64 x):\n    LOG.append(x)\ndef reset():\n    LOG = []\ndef main() -> int64:\n    add(1)\n    reset()\n    return len(LOG)\n");
        ok("dict[str, int64] M = {}\ndef put(str k, int64 v):\n    M[k] = v\ndef main() -> int64:\n    put(\"a\", 1)\n    return M[\"a\"]\n");
    }

    #[test]
    fn module_mutable_auto_infers_type() {
        // `auto NAME = expr` (no `const`) infers the type and is mutable.
        ok("auto counter = 0\ndef main() -> int64:\n    counter = counter + 1\n    return counter\n");
    }

    #[test]
    fn module_mutable_type_mismatch_is_rejected() {
        // The declared type must accept the initializer (ZAP4001), exactly as a
        // module `const` or a local binding would.
        let e = errors("int64 N = \"hi\"\ndef main() -> int64:\n    return 0\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP4001),
            "expected ZAP4001, got {e:?}"
        );
        // And a reassignment of the wrong type is rejected too.
        let e = errors("int64 N = 0\ndef main() -> int64:\n    N = \"hi\"\n    return N\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP4001),
            "expected ZAP4001 on reassignment, got {e:?}"
        );
    }

    #[test]
    fn module_mutable_initializer_may_reference_earlier_binding() {
        // Source-order init: a mutable global's initializer may read an earlier
        // module binding (const or mutable) and call a function.
        ok("const int64 A = 10\nint64 B = A + 5\ndef main() -> int64:\n    return B\n");
        ok("int64 A = 10\nint64 B = A + 5\ndef main() -> int64:\n    return B\n");
        ok("def base() -> int64:\n    return 2\nint64 C = base() * 3\ndef main() -> int64:\n    return C\n");
    }

    #[test]
    fn module_mutable_binding_may_be_shadowed_by_a_local_decl() {
        // Declaring a local of the same name shadows the mutable global inside
        // that function (an ordinary `auto`/typed declaration); a bare assignment
        // would instead reassign the global. Both are legal.
        ok("int64 x = 1\ndef f() -> int64:\n    auto x = 99\n    return x\ndef main() -> int64:\n    return f() + x\n");
    }

    // ---- @shareable marker + sharing lint (ZAP5125, #664/D-0113) ----------

    // A ZAP5125 count helper: how many sharing-lint warnings the source raises
    // (raw, before any `# noqa` suppression).
    fn share_warnings(src: &str) -> usize {
        warns(src)
            .iter()
            .filter(|w| w.code == ErrorCode::ZAP5125)
            .count()
    }

    #[test]
    fn sharing_lint_fires_on_bare_mutable_global_read_and_write() {
        // A mutable module global of a non-@shareable type (a plain scalar) is
        // careless shared state: both a read and a write warn (ZAP5125). None of
        // this is an error — the program is still legal.
        // Write.
        assert_eq!(
            share_warnings(
                "int64 counter = 0\ndef bump():\n    counter = 1\ndef main():\n    bump()\n"
            ),
            1,
        );
        // Read.
        assert_eq!(
            share_warnings(
                "int64 counter = 0\ndef show():\n    print(counter)\ndef main():\n    show()\n"
            ),
            1,
        );
        // Augmented assign is a single read-modify-write access → one warning.
        assert_eq!(
            share_warnings(
                "int64 counter = 0\ndef bump():\n    counter += 1\ndef main():\n    bump()\n"
            ),
            1,
        );
        // No type errors are produced by any of this.
        ok("int64 counter = 0\ndef bump():\n    counter += 1\ndef main():\n    bump()\n");
    }

    #[test]
    fn sharing_lint_fires_for_reference_and_inferred_globals() {
        // A reference-typed mutable global (list/dict) is equally flagged — a
        // read of it (here, iterating / indexing) is an access.
        assert!(
            share_warnings("list[int64] log = []\ndef add(int64 x):\n    log.append(x)\ndef main():\n    add(1)\n")
                >= 1,
        );
        // An `auto`-typed mutable global infers a non-shareable scalar type, so
        // it is flagged too.
        assert_eq!(
            share_warnings("auto n = 0\ndef bump():\n    n = n + 1\ndef main():\n    bump()\n"),
            // one read (RHS `n`) + one write (target `n`).
            2,
        );
    }

    #[test]
    fn sharing_lint_silent_for_const_global() {
        // A `const` global can never race — it is immutable — so no ZAP5125,
        // for either a scalar or a reference-typed const.
        assert_eq!(
            share_warnings("const int64 N = 7\ndef use() -> int64:\n    return N\ndef main():\n    print(use())\n"),
            0,
        );
        assert_eq!(
            share_warnings("const list[int64] XS = [1, 2, 3]\ndef total() -> int64:\n    auto s = 0\n    for x in XS:\n        s += x\n    return s\ndef main():\n    print(total())\n"),
            0,
        );
    }

    #[test]
    fn sharing_lint_silent_for_shareable_user_struct_global() {
        // A mutable global whose type is a user `struct` marked `@shareable` is
        // an asserted-safe type → no warning on access.
        let src = "@shareable\nstruct Registry:\n    int64 n\n\nRegistry R = Registry(0)\ndef touch():\n    auto r = R\ndef main():\n    touch()\n";
        assert_eq!(share_warnings(src), 0);
        // The same struct WITHOUT the marker DOES warn, proving the marker — not
        // the struct-ness — is what silences it.
        let unmarked = "struct Registry:\n    int64 n\n\nRegistry R = Registry(0)\ndef touch():\n    auto r = R\ndef main():\n    touch()\n";
        assert_eq!(share_warnings(unmarked), 1);
    }

    #[test]
    fn sharing_lint_silent_for_builtin_shareable_type_globals() {
        // A global of a built-in concurrency primitive / concurrent collection
        // carries the marker implicitly → no warning. Decided on the nominal
        // base name, so the generic spelling is covered.
        for ty in [
            "Atomic[int64]",
            "Mutex",
            "Counter",
            "Queue[int64]",
            "WaitGroup",
            "Once",
        ] {
            let src = format!(
                "{ty} g = frob()\ndef frob() -> {ty}:\n    return g\ndef touch():\n    auto x = g\ndef main():\n    touch()\n"
            );
            // These types resolve loosely (Unknown) in sema, so the declared-type
            // name is what proves shareability; the point is ZERO ZAP5125.
            assert_eq!(
                share_warnings(&src),
                0,
                "built-in shareable `{ty}` global must not warn",
            );
        }
    }

    #[test]
    fn sharing_lint_silent_for_local_shadow_and_plain_locals() {
        // A plain function local of the same name shadows the global, so an
        // access of the local is NOT a shared-state access.
        assert_eq!(
            share_warnings("int64 x = 1\ndef f() -> int64:\n    auto x = 99\n    return x\ndef main():\n    print(f())\n"),
            0,
        );
        // An ordinary local (no module global at all) never warns.
        assert_eq!(
            share_warnings("def main() -> int64:\n    int64 y = 0\n    y = y + 1\n    return y\n"),
            0,
        );
    }

    #[test]
    fn sharing_lint_silent_during_module_init_prepass() {
        // A global's initializer reading an earlier mutable global runs during
        // single-threaded, ordered module init — not concurrently — so the
        // read in the initializer itself is exempt (only accesses inside a
        // function body are flagged). Here `B = A + 5` reads `A` at init: the
        // only ZAP5125 is the later in-body read, not the initializer read.
        assert_eq!(
            share_warnings("int64 A = 10\nint64 B = A + 5\ndef use() -> int64:\n    return A\ndef main():\n    print(use())\n"),
            1,
        );
    }

    #[test]
    fn shareable_marker_rejects_wrong_target_and_args() {
        // `@shareable` applies to a struct only (ZAP5047) and takes no arguments
        // (ZAP5041).
        let e = errors("@shareable\ndef f():\n    print(1)\ndef main():\n    f()\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP5047),
            "expected ZAP5047 for @shareable on a function, got {e:?}"
        );
        let e = errors("@shareable(1)\nstruct S:\n    int64 n\ndef main():\n    print(0)\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP5041),
            "expected ZAP5041 for @shareable with arguments, got {e:?}"
        );
        // A well-formed application is accepted.
        ok("@shareable\nstruct S:\n    int64 n\ndef main():\n    print(0)\n");
    }

    // --- `@scoped` affine resource marker (#876, D-0159) ----------------------

    /// A well-formed `@scoped` struct `Arena` (both dunders), plus `main`.
    fn arena_src(extra: &str) -> String {
        format!(
            "@scoped\nstruct Arena:\n    int64 id = 0\n    \
             def __enter__(self) -> Arena:\n        return self\n    \
             def __exit__(self):\n        pass\n{extra}"
        )
    }

    #[test]
    fn scoped_marker_rejects_wrong_target_and_args() {
        // `@scoped` applies to a struct only (ZAP5047) and takes no args (ZAP5041).
        let e = errors("@scoped\ndef f():\n    print(1)\ndef main():\n    f()\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP5047),
            "expected ZAP5047 for @scoped on a function, got {e:?}"
        );
        let e = errors(&arena_src(
            "@scoped(1)\nstruct B:\n    int64 n\ndef main():\n    print(0)\n",
        ));
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP5041),
            "expected ZAP5041 for @scoped with arguments, got {e:?}"
        );
    }

    #[test]
    fn scoped_requires_enter_and_exit() {
        // Missing both.
        let e = errors("@scoped\nstruct Arena:\n    int64 id = 0\ndef main():\n    print(0)\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP5127),
            "expected ZAP5127 when both dunders missing, got {e:?}"
        );
        // Missing only __exit__.
        let e = errors(
            "@scoped\nstruct Arena:\n    int64 id = 0\n    \
             def __enter__(self) -> Arena:\n        return self\ndef main():\n    print(0)\n",
        );
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP5127),
            "expected ZAP5127 when __exit__ missing, got {e:?}"
        );
        // Missing only __enter__.
        let e = errors(
            "@scoped\nstruct Arena:\n    int64 id = 0\n    \
             def __exit__(self):\n        pass\ndef main():\n    print(0)\n",
        );
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP5127),
            "expected ZAP5127 when __enter__ missing, got {e:?}"
        );
        // Both present: well-formed.
        ok(&arena_src("def main():\n    print(0)\n"));
    }

    #[test]
    fn scoped_accepts_with_binding_and_receiver_use() {
        // The canonical valid form: construct at the `with` head, use the bound
        // value as a receiver / field read inside the body.
        ok(&arena_src(
            "def main():\n    with Arena() as a:\n        print(a.id)\n",
        ));
        // An unused `with`-bound value is still consumed by the `with`.
        ok(&arena_src(
            "def main():\n    with Arena() as a:\n        pass\n",
        ));
        // Nested `with`s.
        ok(&arena_src(
            "def main():\n    with Arena() as outer:\n        with Arena() as inner:\n            print(inner.id)\n",
        ));
    }

    #[test]
    fn scoped_rejects_detached_construction() {
        for stmt in [
            "    auto a = Arena()\n",
            "    Arena a = Arena()\n",
            "    Arena()\n",
            "    print(Arena().id)\n",
        ] {
            let src = arena_src(&format!("def main():\n{stmt}"));
            let e = errors(&src);
            assert!(
                e.iter().any(|x| x.code == ErrorCode::ZAP5128),
                "expected ZAP5128 for detached construction `{stmt}`, got {e:?}"
            );
        }
    }

    #[test]
    fn scoped_rejects_return_of_fresh_value() {
        let src =
            arena_src("def make() -> Arena:\n    return Arena()\ndef main():\n    print(0)\n");
        let e = errors(&src);
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP5128),
            "expected ZAP5128 for returning a fresh @scoped value, got {e:?}"
        );
    }

    #[test]
    fn scoped_rejects_escape_of_bound_value() {
        // return the bound alias.
        let src = arena_src(
            "def make() -> Arena:\n    with Arena() as a:\n        return a\ndef main():\n    print(0)\n",
        );
        let e = errors(&src);
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP5129),
            "expected ZAP5129 for returning a with-bound @scoped value, got {e:?}"
        );
        // pass the bound alias to a retaining call.
        let src = arena_src(
            "def sink(Arena a):\n    pass\ndef main():\n    with Arena() as a:\n        sink(a)\n",
        );
        let e = errors(&src);
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP5129),
            "expected ZAP5129 for passing a with-bound @scoped value to a call, got {e:?}"
        );
    }

    #[test]
    fn scoped_rejects_cross_spawn_boundary() {
        let src = arena_src(
            "def work(Arena a):\n    pass\ndef main():\n    with Arena() as a:\n        spawn work(a)\n",
        );
        let e = errors(&src);
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP5129),
            "expected ZAP5129 for a @scoped value crossing a spawn boundary, got {e:?}"
        );
        // Also inside a `scope:` region.
        let src = arena_src(
            "def work(Arena a):\n    pass\ndef main():\n    with Arena() as a:\n        scope:\n            spawn work(a)\n",
        );
        let e = errors(&src);
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP5129),
            "expected ZAP5129 for a @scoped value crossing a scope: boundary, got {e:?}"
        );
    }

    #[test]
    fn scoped_contagion_makes_owner_scoped() {
        // `Pool` owns a `@scoped` field, so it is `@scoped` by contagion and must
        // itself define both dunders — a well-formed owner is accepted, and used
        // via `with`; a field move in __enter__ is a legal construction site.
        let pool = "@scoped\nstruct Arena:\n    int64 id = 0\n    \
             def __enter__(self) -> Arena:\n        return self\n    \
             def __exit__(self):\n        pass\n\
             struct Pool:\n    Arena a\n    \
             def __enter__(self) -> Pool:\n        self.a = Arena()\n        return self\n    \
             def __exit__(self):\n        pass\n\
             def main():\n    with Pool() as p:\n        pass\n";
        ok(pool);
    }

    #[test]
    fn scoped_contagion_requires_owner_dunders() {
        // `Pool` owns a `@scoped` field but defines neither dunder: ZAP5127 fires
        // on the implicitly-`@scoped` owner (D-0159 Q5).
        let src = "@scoped\nstruct Arena:\n    int64 id = 0\n    \
             def __enter__(self) -> Arena:\n        return self\n    \
             def __exit__(self):\n        pass\n\
             struct Pool:\n    Arena a\n\
             def main():\n    print(0)\n";
        let e = errors(src);
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP5127),
            "expected ZAP5127 on a contagious @scoped owner missing dunders, got {e:?}"
        );
    }

    #[test]
    fn scoped_contagion_rejects_detached_owner() {
        // A contagious `@scoped` owner is itself with-head-only: a detached
        // `Pool()` is ZAP5128.
        let src = "@scoped\nstruct Arena:\n    int64 id = 0\n    \
             def __enter__(self) -> Arena:\n        return self\n    \
             def __exit__(self):\n        pass\n\
             struct Pool:\n    Arena a\n    \
             def __enter__(self) -> Pool:\n        self.a = Arena()\n        return self\n    \
             def __exit__(self):\n        pass\n\
             def main():\n    auto p = Pool()\n";
        let e = errors(src);
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP5128),
            "expected ZAP5128 for a detached contagious @scoped owner, got {e:?}"
        );
    }

    #[test]
    fn scoped_unrelated_struct_unaffected() {
        // A plain struct is never @scoped: free to construct detached, return,
        // store — no @scoped diagnostics.
        ok("struct Plain:\n    int64 n = 0\ndef make() -> Plain:\n    return Plain()\ndef main():\n    auto p = Plain()\n    print(p.n)\n");
    }

    #[test]
    fn scoped_rejects_construction_in_a_method() {
        // A @scoped value constructed detached inside a *method* (here a plain
        // struct's method) is rejected exactly as in a free function: the
        // with-head-only rule holds everywhere, not just in `main`/free functions.
        let src = arena_src(
            "struct Plain:\n    int64 n = 0\n    \
             def build(self):\n        auto a = Arena()\ndef main():\n    print(0)\n",
        );
        let e = errors(&src);
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP5128),
            "expected ZAP5128 for detached @scoped construction in a method, got {e:?}"
        );
    }

    #[test]
    fn scoped_rejects_producer_as_method_arg() {
        // Passing a freshly produced @scoped value to a call from within a method
        // is a retaining-call escape — rejected (ZAP5128 at the producer).
        let src = arena_src(
            "struct Plain:\n    int64 n = 0\n    \
             def use(self):\n        self.sink(Arena())\n    \
             def sink(self, Arena a):\n        pass\ndef main():\n    print(0)\n",
        );
        let e = errors(&src);
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP5128),
            "expected ZAP5128 for a fresh @scoped value passed to a call in a method, got {e:?}"
        );
    }

    /// A well-formed `@scoped` `Arena` + a `@scoped` `Pool` owner that moves an
    /// `Arena` into its field in `__enter__`.
    fn pool_src(extra: &str) -> String {
        format!(
            "@scoped\nstruct Arena:\n    int64 id = 0\n    \
             def __enter__(self) -> Arena:\n        return self\n    \
             def __exit__(self):\n        pass\n\
             @scoped\nstruct Pool:\n    Arena a\n    \
             def __enter__(self) -> Pool:\n        self.a = Arena()\n        return self\n    \
             def __exit__(self):\n        pass\n{extra}"
        )
    }

    #[test]
    fn scoped_rejects_field_move_outside_the_owner() {
        // The legal field move is ONLY `self.<field> = Producer()` inside the
        // owner's own method. Storing a fresh @scoped value into another object's
        // field (here a `Pool` parameter, not `self`) is NOT a legal move — it is
        // a detached construction stored into an arbitrary slot (ZAP5128).
        let src = pool_src("def tamper(Pool p):\n    p.a = Arena()\ndef main():\n    print(0)\n");
        let e = errors(&src);
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP5128),
            "expected ZAP5128 for a @scoped field move outside the owner, got {e:?}"
        );
    }

    #[test]
    fn scoped_accepts_self_field_move_in_owner_enter() {
        // The complement of the previous test: the SAME `self.a = Arena()` move is
        // accepted inside the `@scoped` owner's own `__enter__`.
        ok(&pool_src(
            "def main():\n    with Pool() as p:\n        pass\n",
        ));
    }

    #[test]
    fn sharing_lint_suppressed_by_noqa_pragma_on_the_site() {
        // `# noqa` on an access line silences exactly that line's warning.
        // Without it, the write warns; with it, silent; a second un-pragma'd
        // access on another line still warns.
        let base = "int64 c = 0\ndef bump():\n    c = 1\ndef main():\n    bump()\n";
        assert_eq!(
            warnings_str(base)
                .iter()
                .filter(|w| w.code == ErrorCode::ZAP5125)
                .count(),
            1,
        );
        let suppressed = "int64 c = 0\ndef bump():\n    c = 1  # noqa\ndef main():\n    bump()\n";
        assert_eq!(
            warnings_str(suppressed)
                .iter()
                .filter(|w| w.code == ErrorCode::ZAP5125)
                .count(),
            0,
        );
        // Code-specific form targets exactly ZAP5125.
        let coded =
            "int64 c = 0\ndef bump():\n    c = 1  # noqa: ZAP5125\ndef main():\n    bump()\n";
        assert_eq!(
            warnings_str(coded)
                .iter()
                .filter(|w| w.code == ErrorCode::ZAP5125)
                .count(),
            0,
        );
        // A noqa for an UNRELATED code does not silence ZAP5125.
        let other =
            "int64 c = 0\ndef bump():\n    c = 1  # noqa: ZAP5124\ndef main():\n    bump()\n";
        assert_eq!(
            warnings_str(other)
                .iter()
                .filter(|w| w.code == ErrorCode::ZAP5125)
                .count(),
            1,
        );
        // Suppression is per-line: a pragma on one access line does not silence
        // an access on a different line.
        let two = "int64 c = 0\ndef read() -> int64:\n    return c  # noqa\ndef write():\n    c = 2\ndef main():\n    write()\n    print(read())\n";
        assert_eq!(
            warnings_str(two)
                .iter()
                .filter(|w| w.code == ErrorCode::ZAP5125)
                .count(),
            1,
        );
    }

    #[test]
    fn apply_suppressions_is_a_standalone_line_filter() {
        // The pure filter drops a warning whose line carries the pragma and keeps
        // one whose line does not, independent of the checker.
        let src = "a\nb  # noqa\nc\n";
        let span = |line0: u32| -> Span {
            // byte offset of the start of a 0-based line (each line is "x\n" or
            // "x  # noqa\n"); compute from the source.
            let mut off = 0u32;
            for (i, l) in src.split_inclusive('\n').enumerate() {
                if i as u32 == line0 {
                    break;
                }
                off += l.len() as u32;
            }
            Span {
                start: off,
                end: off + 1,
            }
        };
        let ws = vec![
            TypeError {
                code: ErrorCode::ZAP5125,
                message: "w0".into(),
                span: span(0),
            },
            TypeError {
                code: ErrorCode::ZAP5125,
                message: "w1".into(),
                span: span(1),
            },
        ];
        let kept = apply_suppressions(src, ws);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].message, "w0");
    }

    #[test]
    fn type_alias_chain_resolves_through_aliases() {
        // A non-cyclic alias chain resolves fully: `Meters -> Len -> int64`.
        let e =
            errors("type Len = int64\ntype Meters = Len\ndef f() -> Meters:\n    return \"x\"\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP4003),
            "alias chain must resolve to int64, got {e:?}"
        );
    }

    #[test]
    fn cyclic_type_alias_is_rejected() {
        // A two-step cycle is reported (ZAP4090) rather than looping.
        let e = errors("type A = B\ntype B = A\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP4090),
            "expected ZAP4090 for the cycle, got {e:?}"
        );
    }

    #[test]
    fn self_referential_type_alias_is_rejected() {
        // The degenerate self-cycle `type A = A`, and an indirect cycle through a
        // collection element, are both reported.
        let e = errors("type A = A\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP4090),
            "expected ZAP4090 for `type A = A`, got {e:?}"
        );
        let e = errors("type A = list[B]\ntype B = (int64, A)\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP4090),
            "expected ZAP4090 for the indirect cycle, got {e:?}"
        );
    }

    #[test]
    fn type_is_a_usable_identifier_in_sema() {
        // The soft keyword does not reserve `type`: an ordinary local named
        // `type` type-checks with no alias involved.
        ok("def main():\n    auto type = 3\n    print(type + 1)\n");
    }

    #[test]
    fn chained_assignment_type_checks(/* #594 */) {
        // Names and subscript targets, with the one RHS assignable to each.
        ok(r#"
def main():
    int64 a = 0
    int64 b = 0
    a = b = 5
    list[int64] xs = [1, 2, 3]
    dict[str, int64] d = {"k": 0}
    xs[0] = d["k"] = 9
"#);
    }

    #[test]
    fn chained_assignment_to_undeclared_is_rejected(/* #594 */) {
        // A chained target is never a declaration: an unknown name is ZAP3002,
        // exactly as a single reassignment to an undeclared variable is.
        let e = errors("def main():\n    int64 a = 0\n    a = zzz = 5\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP3002),
            "expected ZAP3002, got {e:?}"
        );
    }

    #[test]
    fn chained_assignment_type_mismatch_is_rejected(/* #594 */) {
        // The single RHS must be assignable to every target; a str RHS against
        // an int64 target is ZAP4001.
        let e = errors("def main():\n    int64 a = 0\n    int64 b = 0\n    a = b = \"x\"\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP4001),
            "expected ZAP4001, got {e:?}"
        );
    }

    #[test]
    fn raise_from_cause_type_checks(/* #604, D-0103 */) {
        // `raise <exc> from <cause>` chaining: the raised exception and the
        // caught cause both resolve, and the caught binding's `.cause` is
        // inspectable (loosely typed, so it never errors).
        ok(r#"
def main():
    try:
        raise ValueError("first")
    except Exception as e:
        raise RuntimeError("second") from e

def inspect():
    try:
        raise ValueError("boom")
    except Exception as e:
        try:
            raise RuntimeError("wrap") from e
        except RuntimeError as w:
            print(w.cause)
"#);
    }

    #[test]
    fn del_element_type_checks(/* #595 */) {
        ok(r#"
def main():
    list[int64] xs = [1, 2, 3]
    dict[str, int64] d = {"a": 1}
    del xs[0]
    del d["a"]
"#);
    }

    #[test]
    fn array_collection_contract_fixtures_typecheck() {
        ok(include_str!(
            "../../../tests/conformance/array_contract.zap"
        ));
        ok(include_str!(
            "../../../tests/conformance/collection_elements.zap"
        ));
    }

    #[test]
    fn floating_collection_slots_are_diagnosed() {
        // Under the capability model (D-0139/#807) the only float element still
        // unsupported is `f32`/`float32` — a REPRESENTATION-readiness gap (the
        // backend has no `f32` collection slot yet), not a type-law failure — in
        // every container and every position, value or hashed/ordered. Each still
        // raises ZAP4027, now with f32-specific representation-gap wording (NOT the
        // old "floating-point collection elements are unsupported"). Every `f64`
        // position is accepted — see `float_value_collections_ok` /
        // `float_key_member_collections_ok`.
        for source in [
            // `f32` element — a representation gap everywhere, key and value.
            "list[f32] x = []",
            "dict[str, f32] x = {}",
            "dict[f32, str] x = {}",
            "dict[f32, f32] x = {}",
            "set[f32] x = set()",
            "set[float32] x = set()",
            "heap[f32] x = heap[f32]()",
            "channel[f32] x = channel[f32]()",
            // `f32` nested inside an otherwise-admitted container.
            "list[dict[f32, str]] x = []",
            "dict[str, set[f32]] x = {}",
        ] {
            let source = format!("def main():\n    {source}\n");
            let es = errors(&source);
            let f32_err = es.iter().find(|e| e.code == ErrorCode::ZAP4027);
            assert!(f32_err.is_some(), "expected ZAP4027 for: {source}");
            // Representation-gap wording, f32-specific (f64 IS supported now).
            let m = &f32_err.unwrap().message;
            assert!(
                m.contains("f32") && m.contains("collection element"),
                "expected f32-specific representation-gap wording, got: {m}"
            );
            assert!(
                !m.contains("floating-point collection elements are unsupported"),
                "the old enumerated-float wording must be retired, got: {m}"
            );
        }
    }

    #[test]
    fn array_f32_is_an_accepted_dtype() {
        // `array[f32]` is a supported dense dtype (#503, D-0166). It never tripped
        // the `f32` collection-slot gate (ZAP4027 — `array` is exempt), and it is
        // no longer rejected by the array-dtype guard (ZAP4028): dense f32 buffers
        // are exactly what `array[f32]` is for.
        let es =
            errors("def main():\n    array[f32] a = array[f32]([1.0, 2.0])\n    print(len(a))\n");
        assert!(
            es.is_empty(),
            "array[f32] should now be accepted (no ZAP4027/ZAP4028): {es:?}"
        );
    }

    #[test]
    fn value_struct_with_derive_eq_hash_is_a_valid_key_via_the_bound() {
        // Ties #807 to the refinement machinery (#806): a `value struct` deriving
        // (Eq, Hash) satisfies the `Hashable` bound a dict key / set member needs,
        // so it type-checks in both positions. (Pins existing behaviour — the
        // capability reframe must keep it passing.)
        ok(
            "@derive(Eq, Hash)\nvalue struct P:\n    int64 x\n    int64 y\n\n\
            def main():\n    dict[P, str] d = {}\n    d[P(x=1, y=2)] = \"a\"\n    \
            set[P] s = set[P]()\n    s.add(P(x=3, y=4))\n    print(len(d) + len(s))\n",
        );
    }

    #[test]
    fn float_value_collections_ok() {
        // #800 Phase 1: `float64` is a first-class *value* in a dict value, a
        // heap element, and a channel element (no hashing needed). Annotation and
        // `auto`-inferred forms, every key kind, and read-back all type-check.
        for source in [
            // dict VALUES — str/int keys, annotated and inferred.
            "dict[str, float64] d = {}\n    d[\"a\"] = 1.5\n    print(d[\"a\"])",
            "dict[str, f64] d = {\"a\": 1.5}\n    print(len(d))",
            "dict[int64, float64] d = {1: 1.5, 2: 2.5}\n    print(d[1])",
            "auto d = {1: 1.5}\n    print(len(d))",
            "auto d = {\"a\": 1.5, \"b\": 2.5}\n    print(len(d))",
            "dict[str, float64] d = {\"a\": 1.5}\n    list[float64] vs = d.values()\n    print(len(vs))",
            // heap ELEMENTS — min and max.
            "heap[float64] h = heap[float64]()\n    h.push(1.5)\n    print(h.pop())",
            "auto h = heap[f64](cmp=\"max\")\n    h.push(2.5)\n    print(h.peek())",
            // channel ELEMENTS — annotation + construction, send/recv.
            "channel[float64] c = channel[float64](1)\n    c <- 3.5\n    float x = <-c\n    print(x)",
            "auto c = channel[f64](2)\n    c <- 1.5",
            // nested: a list of float values is still fine (I#569).
            "list[float64] xs = [1.0, 2.0]\n    print(len(xs))",
        ] {
            let source = format!("def main():\n    {source}\n");
            assert!(
                errors(&source).is_empty(),
                "unexpected type errors for: {source}\n{:?}",
                errors(&source)
            );
        }
    }

    #[test]
    fn float_key_member_collections_ok() {
        // #800 Phase 2 (#803): `float64`/`float` is a first-class dict KEY and set
        // MEMBER. Annotated and `auto`-inferred forms, construction, insert,
        // read-back, membership, `.keys()`, `del`, and an `f64`-keyed `f64`-valued
        // dict all type-check. (The `-0.0`/NaN semantics are a runtime concern —
        // D-0138 — exercised by the goldens; sema only lifts the ZAP4027 guard.)
        for source in [
            // dict KEYS — annotated, every float spelling, with reads and del.
            "dict[float64, str] d = {}\n    d[1.5] = \"a\"\n    print(d[1.5])",
            "dict[f64, int64] d = {1.5: 1, 2.5: 2}\n    print(d[2.5])",
            "dict[float, int64] d = {}\n    d[0.0] = 7\n    print(1.5 in d)\n    del d[0.0]",
            // dict with BOTH a float key and a float value.
            "dict[float64, float64] d = {1.5: 2.5}\n    print(d[1.5])",
            // `.keys()` of a float-keyed dict yields a list[float64].
            "dict[float64, str] d = {1.5: \"a\"}\n    list[float64] ks = d.keys()\n    print(len(ks))",
            // set MEMBERS — annotated and inferred, add + membership + iterate.
            "set[float64] s = set[float64]()\n    s.add(1.5)\n    print(1.5 in s)",
            "set[f64] s = set[f64]()\n    s.add(0.0)\n    print(len(s))",
            "set[float] s = {1.5, 2.5}\n    for x in s:\n        print(x)",
            // inferred float-key dict / float set literals.
            "auto d = {1.5: 1}\n    print(len(d))",
            "auto s = {1.5, 2.5}\n    print(len(s))",
        ] {
            let source = format!("def main():\n    {source}\n");
            assert!(
                errors(&source).is_empty(),
                "unexpected type errors for: {source}\n{:?}",
                errors(&source)
            );
        }
    }

    #[test]
    fn bigint_complex_collection_elements_still_rejected() {
        // #800 lifts the *float* collection guard (ZAP4027) for value positions,
        // but the sibling bigint/complex guard (ZAP4024) is unrelated and
        // unchanged: bigint/complex remain unsupported as any collection
        // element/key, in every container.
        for source in [
            "list[bigint] x = []",
            "dict[str, bigint] x = {}",
            "dict[bigint, str] x = {}",
            "set[bigint] x = set()",
            "heap[bigint] x = heap[bigint]()",
            "list[complex] x = []",
            "dict[str, complex] x = {}",
            "channel[complex] x = channel[complex]()",
        ] {
            let source = format!("def main():\n    {source}\n");
            assert!(
                errors(&source).iter().any(|e| e.code == ErrorCode::ZAP4024),
                "expected ZAP4024 for: {source}"
            );
        }
    }

    #[test]
    fn float_list_elements_typecheck() {
        // `list[float]` / `list[f64]` is a first-class scalar-float list (I#569):
        // bindings, params, a float-literal display, a list comprehension, and a
        // nested `list[list[f64]]` all type-check.
        ok("def main():\n    list[float] xs = [1.0, 2.0, 3.0]\n    print(len(xs))\n");
        ok("def main():\n    auto xs = [1.5, 2.5]\n    print(len(xs))\n");
        ok("def f(list[f64] xs) -> float:\n    float t = 0.0\n    for x in xs:\n        t = t + x\n    return t\n");
        ok("def main():\n    auto xs = [f64(n) for n in [1, 2]]\n    print(len(xs))\n");
        ok("def main():\n    list[list[f64]] x = []\n    print(len(x))\n");
    }

    #[test]
    fn int_scalar_type_checks() {
        // `int` is an exact alias for `int64` (I#581): binding, params/return,
        // `list[int]`, and `array[int]` all type-check...
        ok("def main():\n    int x = 1\n    list[int] xs = [1, 2, 3]\n    print(x + len(xs))\n");
        ok("def add(int a, int b) -> int:\n    return a + b\ndef main():\n    print(add(1, 2))\n");
        ok("def main():\n    array[int] a = array[int]([1, 2, 3])\n    print(len(a))\n");
        // ...and `int` is a *real* type, not `Unknown`: assigning a non-numeric
        // value to it, or returning one from an `-> int` function, is rejected
        // (this is the type-checking the alias restores).
        assert!(errors("def main():\n    int x = \"hello\"\n")
            .iter()
            .any(|e| e.code == ErrorCode::ZAP4001));
        assert!(errors("def f(int n) -> int:\n    return \"x\"\n")
            .iter()
            .any(|e| e.code == ErrorCode::ZAP4003));
    }

    #[test]
    fn kwargs_collector_param_and_extra_keywords() {
        // A `V **name` collector accepts any extra keyword arguments whose value
        // type coerces to V, and is bound in the body as `dict[str, V]`.
        ok("def f(int64 base, int64 **counts) -> int64:\n\
            \x20   auto t = base\n\
            \x20   for k in counts.keys():\n\
            \x20       t = t + counts[k]\n\
            \x20   return t\n\
            def main():\n    auto x = f(0, a=1, b=2)\n");

        // An extra keyword whose value type does not coerce to V is rejected.
        assert!(errors(
            "def f(int64 **counts) -> int64:\n    return 0\n\
             def main():\n    auto x = f(a=\"s\")\n"
        )
        .iter()
        .any(|e| e.code == ErrorCode::ZAP4022));

        // Without a collector an unknown keyword is still ZAP4023.
        assert!(errors(
            "def f(int64 a) -> int64:\n    return a\n\
             def main():\n    auto x = f(1, b=2)\n"
        )
        .iter()
        .any(|e| e.code == ErrorCode::ZAP4023));
    }

    #[test]
    fn kwargs_collector_param_shape_errors() {
        // A collector must be the last parameter.
        assert!(errors(
            "def f(int64 **kw, int64 a) -> int64:\n    return a\n\
             def main():\n    auto x = f(1)\n"
        )
        .iter()
        .any(|e| e.code == ErrorCode::ZAP5121));

        // At most one collector.
        assert!(errors(
            "def f(int64 **a, int64 **b) -> int64:\n    return 0\n\
             def main():\n    auto x = f()\n"
        )
        .iter()
        .any(|e| e.code == ErrorCode::ZAP5121));

        // A collector cannot carry a default.
        assert!(errors(
            "def f(int64 **kw = 0) -> int64:\n    return 0\n\
             def main():\n    auto x = f()\n"
        )
        .iter()
        .any(|e| e.code == ErrorCode::ZAP5121));
    }

    #[test]
    fn starred_call_arguments_checked() {
        // `*list` forwards into a variadic; `**dict` forwards into a collector.
        ok("def sum_all(int64... nums) -> int64:\n\
            \x20   auto t = 0\n\
            \x20   for n in nums:\n\
            \x20       t = t + n\n\
            \x20   return t\n\
            def tally(int64 base, int64 **counts) -> int64:\n    return base\n\
            def forward(int64... args, int64 **opts) -> int64:\n\
            \x20   return sum_all(*args) + tally(0, **opts)\n\
            def main():\n\
            \x20   auto xs = [1, 2, 3]\n\
            \x20   auto d = {\"x\": 1}\n\
            \x20   auto a = sum_all(*xs)\n\
            \x20   auto b = tally(0, **d)\n");

        // A statically-sized tuple expands into fixed parameters.
        ok("def dist(int64 x, int64 y) -> int64:\n    return x + y\n\
            def main():\n    auto p = (3, 4)\n    auto d = dist(*p)\n");

        // `*list` into a non-variadic callee is rejected (unknown length).
        assert!(errors(
            "def f(int64 a, int64 b) -> int64:\n    return a + b\n\
             def main():\n    auto xs = [1, 2]\n    auto y = f(*xs)\n"
        )
        .iter()
        .any(|e| e.code == ErrorCode::ZAP5122));

        // `**dict` into a callee with no collector is rejected.
        assert!(errors(
            "def f(int64 a) -> int64:\n    return a\n\
             def main():\n    auto d = {\"x\": 1}\n    auto y = f(1, **d)\n"
        )
        .iter()
        .any(|e| e.code == ErrorCode::ZAP5122));

        // Spreads are rejected on a non-free-function callee (a built-in here).
        assert!(
            errors("def main():\n    auto xs = [1, 2]\n    print(*xs)\n")
                .iter()
                .any(|e| e.code == ErrorCode::ZAP5122)
        );
    }

    #[test]
    fn display_unpacking_type_checks(/* #598 */) {
        // `*` splices into list/set/tuple; `**` merges into a dict. Spliced
        // element/entry types agree with the display.
        ok(r#"
def main():
    list[int64] xs = [2, 3]
    list[int64] ys = [1, *xs, 4, *xs]
    set[int64] s = {*xs, 5, *[6, 7]}
    auto tup = (10, 20)
    auto t = (1, *tup, 2)
    dict[str, int64] base = {"a": 1}
    dict[str, int64] m = {"b": 2, **base, "c": 3}
"#);
    }

    #[test]
    fn display_unpacking_type_mismatch_rejected(/* #598 */) {
        // A `*`-splat whose element type disagrees with the list element type.
        let e = errors("def main():\n    list[str] ss = [\"a\"]\n    list[int64] xs = [1, *ss]\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP5123),
            "expected ZAP5123, got {e:?}"
        );

        // A `**`-merge whose operand is not a dict.
        let e = errors("def main():\n    list[int64] xs = [1]\n    auto d = {\"a\": 1, **xs}\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP5123),
            "expected ZAP5123, got {e:?}"
        );

        // A `*`-splat into a tuple requires a tuple operand, not a list.
        let e = errors("def main():\n    list[int64] xs = [1, 2]\n    auto t = (0, *xs)\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP5123),
            "expected ZAP5123, got {e:?}"
        );
    }

    #[test]
    fn multi_clause_comprehension_type_checks(/* #599 */) {
        // Later clauses see earlier clauses' variables; filters reference them.
        ok(r#"
def main():
    list[int64] xs = [1, 2, 3]
    list[int64] ys = [1, 2, 3]
    list[int64] prod = [x * y for x in xs for y in ys if x != y]
    dict[int64, int64] grid = {x * 10 + y: x + y for x in range(2) for y in range(2)}
    set[int64] cells = {x + y for x in xs for y in ys if x > 0 if y > 0}
"#);
    }

    // ---- generator expressions (#596, epic #614 D4) -----------------------

    #[test]
    fn genexpr_is_a_lazy_iterator() {
        // `auto g = (x for x in xs)` binds an Iterator consumable by `for`.
        ok(r#"
def main():
    list[int64] xs = [1, 2, 3]
    auto g = (x * x for x in xs if x > 0)
    for v in g:
        print(v)
"#);
    }

    #[test]
    fn genexpr_consumed_by_sum() {
        // `sum(x*x for x in range(5))` — the headline acceptance (#596); a
        // genexpr as the sole call argument needs no inner parentheses.
        ok(r#"
def main():
    auto total = sum(x * x for x in range(5))
    print(total)
"#);
    }

    #[test]
    fn genexpr_in_for_loop_header() {
        // `for y in (f(x) for x in xs)` iterates the genexpr lazily.
        ok(r#"
def f(int64 x) -> int64:
    return x + 1
def main():
    list[int64] xs = [1, 2, 3]
    for y in (f(x) for x in xs):
        print(y)
"#);
    }

    #[test]
    fn genexpr_loop_var_does_not_leak() {
        // The loop variable lives only inside the genexpr (like a comprehension
        // and Python); referencing it afterward is an undefined name.
        let e = errors(
            "def main():\n    list[int64] xs = [1, 2, 3]\n    auto g = (x for x in xs)\n    auto z = x\n",
        );
        assert!(
            !e.is_empty(),
            "the genexpr loop variable must not leak into the enclosing scope"
        );
    }

    // ---- walrus assignment-expression (#600, epic #614 D8) ----------------

    #[test]
    fn walrus_binds_in_if_condition_and_is_visible_after() {
        // `(n := len(xs))` binds `n` in the enclosing scope, so `n` is usable
        // both inside the branch body and after the `if`.
        ok(r#"
def main():
    list[int64] xs = [1, 2, 3]
    if (n := len(xs)) > 2:
        auto inside = n + 1
    auto after = n + 1
"#);
    }

    #[test]
    fn walrus_binds_in_while_condition_and_is_visible_after() {
        // `(line := read())` binds `line` once per iteration in the enclosing
        // scope; the name survives the loop.
        ok(r#"
def read() -> str:
    return ""
def main():
    while (line := read()) != "":
        auto inside = line
    auto after = line
"#);
    }

    #[test]
    fn walrus_binds_in_comprehension_filter_and_is_visible_after() {
        // A walrus in a comprehension `if` filter binds in the scope containing
        // the comprehension (PEP 572), so `y` is visible after it.
        ok(r#"
def f(int64 x) -> int64:
    return x
def main():
    list[int64] xs = [1, 2, 3]
    list[int64] kept = [y for x in xs if (y := f(x)) > 0]
    auto after = y + 1
"#);
    }

    #[test]
    fn walrus_value_type_flows_to_the_binding() {
        // The bound name takes the operand's (defaulted) type: binding a `str`
        // walrus and then using it as an int is the usual operator mismatch, so
        // the walrus is not silently typed as `Unknown`.
        let e = errors(
            "def read() -> str:\n    return \"\"\n\
             def main():\n    if (line := read()) != \"\":\n        auto n = line + 1\n",
        );
        assert!(
            !e.is_empty(),
            "using the str-typed walrus binding as an int should error"
        );
    }

    #[test]
    fn bare_walrus_statement_is_a_parse_error_steering_to_auto() {
        // `a := 10` as a bare statement is rejected before type-checking with
        // ZAP2020 (#600 non-goal); the pipeline surfaces that parse error.
        let (_terrs, perrs, lerrs) = check_str("def main():\n    a := 10\n");
        assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
        assert!(
            perrs.iter().any(|e| e.code == ErrorCode::ZAP2020),
            "expected ZAP2020 steering to `auto`, got {perrs:?}"
        );
    }

    #[test]
    fn keyword_only_params_enforced_at_call_site() {
        // #298 defaults + keyword args compose with the I#496 keyword-only marker.
        // Supplying a keyword-only parameter by name works, and a default lets it
        // be omitted entirely.
        ok(
            "def f(int64 a, *, int64 b = 1) -> int64:\n    return a + b\n\
            def main():\n    auto x = f(1)\n    auto y = f(1, b=5)\n",
        );

        // Passing a keyword-only parameter positionally is rejected (ZAP4029).
        assert!(errors(
            "def f(int64 a, *, int64 b = 1) -> int64:\n    return a + b\n\
             def main():\n    auto x = f(1, 5)\n"
        )
        .iter()
        .any(|e| e.code == ErrorCode::ZAP4029));

        // A required keyword-only parameter omitted at the call site is a missing
        // required argument (ZAP4020).
        assert!(errors(
            "def f(int64 a, *, int64 b) -> int64:\n    return a + b\n\
             def main():\n    auto x = f(1)\n"
        )
        .iter()
        .any(|e| e.code == ErrorCode::ZAP4020));

        // A required keyword-only parameter supplied by name is accepted.
        ok("def f(int64 a, *, int64 b) -> int64:\n    return a + b\n\
            def main():\n    auto x = f(1, b=2)\n");
    }

    #[test]
    fn positional_only_params_enforced_at_call_site() {
        // #602 positional-only `/` separator. Parameters before `/` must be passed
        // positionally; parameters after it still accept positional or keyword.
        ok(
            "def f(int64 x, int64 y, /, int64 z) -> int64:\n    return x + y + z\n\
             def main():\n    auto a = f(1, 2, 3)\n    auto b = f(1, 2, z=3)\n",
        );

        // Supplying a positional-only parameter by name is rejected (ZAP4034).
        assert!(errors(
            "def f(int64 x, int64 y, /, int64 z) -> int64:\n    return x + y + z\n\
             def main():\n    auto a = f(x=1, y=2, z=3)\n"
        )
        .iter()
        .any(|e| e.code == ErrorCode::ZAP4034));

        // Only the first positional-only parameter named is enough to diagnose.
        assert!(errors(
            "def f(int64 x, int64 y, /, int64 z) -> int64:\n    return x + y + z\n\
             def main():\n    auto a = f(1, y=2, z=3)\n"
        )
        .iter()
        .any(|e| e.code == ErrorCode::ZAP4034));

        // With a `**` collector, a positional-only name is collected rather than
        // rejected (Python semantics): `x` lands in `kw`.
        ok("def f(int64 x, /, int64 **kw) -> int64:\n    return x\n\
            def main():\n    auto a = f(1, x=9)\n");
    }

    #[test]
    fn unsupported_array_constructors_are_diagnosed() {
        for value in [
            "array[str]([1])",
            "array[complex]([1])",
            "array[f64]([\"x\"])",
            "array[f64]([[\"x\"]])", // a non-numeric leaf, even nested, is rejected
            "array[f64]()",
            "array[f64](values=[1])",
            "array[f64]([1], [2])",
            "array[i64](values)",
        ] {
            let source = format!("def main():\n    auto values = [1,2]\n    auto a = {value}\n");
            assert!(
                errors(&source).iter().any(|e| e.code == ErrorCode::ZAP4028),
                "{source}"
            );
        }
    }

    #[test]
    fn nd_array_constructors_pass_sema() {
        // Nested rectangular literals build N-D arrays (#500, D-0164): sema accepts
        // them (every scalar leaf is numeric); rectangularity is a lowering concern.
        ok("def main():\n    auto m = array[f64]([[1.0, 2.0], [3.0, 4.0]])\n    print(len(m))\n");
        ok("def main():\n    auto c = array[i64]([[[1, 2], [3, 4]], [[5, 6], [7, 8]]])\n    print(c[1, 0, 1])\n");
        // Shape attributes and multi-axis indexing type loosely (no false error).
        ok(
            "def main():\n    auto m = array[i64]([[1, 2], [3, 4]])\n    \
            print(m.ndim, m.shape[0], m.size, m[0, 1])\n",
        );
    }

    #[test]
    fn scope_cleanup_fixture_typechecks() {
        ok(include_str!("../../../tests/conformance/scope_cleanup.zap"));
    }

    #[test]
    fn unsupported_spawn_forms_fail_in_sema() {
        // Forms the sema gate rejects with ZAP5076 (I#475, D-0118): a bare lambda
        // literal callee, a keyword argument, an arity mismatch against a named
        // function, and a non-call operand.
        for call in [
            "(lambda: print(1))()", // bare lambda literal — bind it first
            "worker(value=1)",      // keyword argument — spawn is positional-only
            "worker()",             // too few positional arguments (no defaults)
            "worker",               // not a call
        ] {
            let source = format!("def worker(int64 value=1):\n    print(value)\ndef main():\n    auto g = lambda: print(1)\n    spawn {call}\n");
            assert!(
                errors(&source).iter().any(|e| e.code == ErrorCode::ZAP5076),
                "expected ZAP5076 for: {call}"
            );
        }
    }

    #[test]
    fn supported_spawn_forms_pass_sema() {
        // The callable forms I#475 adds: a first-class function value held in a
        // local, and a user-struct method call. sema has no function type (the
        // lambda infers `Unknown`), so it accepts these structurally and leaves
        // the precise check to IR lowering.
        let fnvalue = "def worker(int64 value):\n    print(value)\ndef main():\n    auto g = lambda: print(1)\n    spawn g()\n";
        assert!(
            !errors(fnvalue).iter().any(|e| e.code == ErrorCode::ZAP5076),
            "a function-value spawn is accepted by sema: {:?}",
            errors(fnvalue)
        );
        let method = "struct Worker:\n    def run(self):\n        print(1)\ndef main():\n    auto w = Worker()\n    spawn w.run()\n";
        assert!(
            !errors(method).iter().any(|e| e.code == ErrorCode::ZAP5076),
            "a method spawn is accepted by sema: {:?}",
            errors(method)
        );
        // A named top-level function with the full positional argument list.
        let named = "def worker(int64 a):\n    print(a)\ndef main():\n    spawn worker(5)\n";
        assert!(
            !errors(named).iter().any(|e| e.code == ErrorCode::ZAP5076),
            "a named-function spawn is accepted: {:?}",
            errors(named)
        );
    }

    #[test]
    fn structural_match_conformance_typechecks() {
        let (module, parse, lex) = zapc_syntax::parser::parse_str(include_str!(
            "../../../tests/conformance/match_structural.zap"
        ));
        assert!(parse.is_empty() && lex.is_empty());
        let errors = check(&module);
        assert!(errors.is_empty(), "{errors:?}");
    }

    // ---- enums conform to interfaces structurally (#731, D-0016) -----------

    #[test]
    fn tier3_enum_conforms_to_interface() {
        // The issue repro: a tier-3 (data-carrying) enum with the interface's
        // method set is assignable to an interface binding and parameter.
        ok(r#"
interface Node:
    def rank(self) -> int

enum Expr:
    Lit(int v)
    Add(int l, int r)

    def rank(self) -> int:
        return 1

def rank_of(Node n) -> int:
    return n.rank()

def main():
    Node n = Expr.Add(l=1, r=2)
    print(n.rank())
    print(rank_of(Expr.Lit(v=5)))
"#);
    }

    #[test]
    fn tier1_enum_conforms_to_interface() {
        // A tier-1 (unboxed-integer) enum with the method set conforms too — the
        // scalar is boxed in lowering; conformance here is purely structural.
        ok(r#"
interface Ranked:
    def rank(self) -> int

enum Color:
    Red
    Green
    Blue

    def rank(self) -> int:
        return 7

def rank_of(Ranked r) -> int:
    return r.rank()

def main():
    Ranked r = Color.Green
    print(r.rank())
    print(rank_of(Color.Red))
"#);
    }

    #[test]
    fn struct_and_both_enum_tiers_conform_to_one_interface() {
        // Uniformity: a reference struct, a tier-1 enum, and a tier-3 enum all
        // conform to the same interface and are each assignable to it.
        ok(r#"
interface Shape:
    def sides(self) -> int

struct Tri:
    def sides(self) -> int:
        return 3

enum Poly:
    Quad
    Pent

    def sides(self) -> int:
        return 4

enum Spur:
    Star(int points)

    def sides(self) -> int:
        return 5

def main():
    Shape a = Tri()
    Shape b = Poly.Quad
    Shape c = Spur.Star(points=5)
    print(a.sides())
    print(b.sides())
    print(c.sides())
"#);
    }

    #[test]
    fn enum_missing_an_interface_method_does_not_conform() {
        // A tier-3 enum lacking a required method is rejected (ZAP4001), and the
        // diagnostic names the missing method.
        let e = errors(
            r#"
interface Node:
    def rank(self) -> int
    def depth(self) -> int

enum Expr:
    Lit(int v)

    def rank(self) -> int:
        return 1

def main():
    Node n = Expr.Lit(v=1)
    print(n.rank())
"#,
        );
        assert!(
            e.iter()
                .any(|d| d.message.contains("missing method 'depth'")
                    && d.message.contains("Expr does not satisfy 'Node'")),
            "{e:?}"
        );
    }

    #[test]
    fn tier1_enum_missing_an_interface_method_does_not_conform() {
        // Same for a tier-1 enum — no pointer/box path masks a missing method.
        let e = errors(
            r#"
interface Ranked:
    def rank(self) -> int

enum Color:
    Red
    Green

def main():
    Ranked r = Color.Red
    print(r.rank())
"#,
        );
        assert!(
            e.iter()
                .any(|d| d.message.contains("missing method 'rank'")),
            "{e:?}"
        );
    }

    #[test]
    fn enum_with_mismatched_method_signature_does_not_conform() {
        // The method exists but with the wrong return type: not conforming.
        let e = errors(
            r#"
interface Node:
    def rank(self) -> int

enum Expr:
    Lit(int v)

    def rank(self) -> str:
        return "x"

def main():
    Node n = Expr.Lit(v=1)
    print(n.rank())
"#,
        );
        assert!(
            e.iter().any(|d| d.message.contains("mismatched signature")),
            "{e:?}"
        );
    }

    #[test]
    fn enum_conforms_to_refined_interface() {
        // Refinement (D-0139/#806): an enum conforms to `Ranked: Named` only when
        // it has BOTH the own and the inherited supertype method.
        ok(r#"
interface Named:
    def name(self) -> int

interface Ranked: Named
    def rank(self) -> int

enum Color:
    Red
    Green

    def name(self) -> int:
        return 0

    def rank(self) -> int:
        return 9

def main():
    Ranked r = Color.Green
    print(r.rank())
"#);
        // Missing the supertype method -> rejected, naming the supertype.
        let e = errors(
            r#"
interface Named:
    def name(self) -> int

interface Ranked: Named
    def rank(self) -> int

enum Color:
    Red
    Green

    def rank(self) -> int:
        return 9

def main():
    Ranked r = Color.Green
    print(r.rank())
"#,
        );
        assert!(
            e.iter().any(|d| d.message.contains("missing method 'name'")
                && d.message.contains("supertype 'Named'")),
            "{e:?}"
        );
    }

    #[test]
    fn refutable_payload_and_guard_do_not_cover_a_variant() {
        for arm in ["case Box.Value(1):", "case Box.Value(n) if n > 0:"] {
            let source = format!("enum Box:\n    Value(int64 n)\ndef main():\n    match Box.Value(1):\n        {arm}\n            print(1)\n");
            let (module, _, _) = zapc_syntax::parser::parse_str(&source);
            let errors = check(&module);
            assert!(
                errors.iter().any(|e| e.message.contains("non-exhaustive")),
                "{errors:?}"
            );
        }
    }

    #[test]
    fn as_pattern_binds_capture_and_inner_names(/* #603, D11 */) {
        // `case [a, b] as pair:` binds `pair` (the whole matched value) AND the
        // inner captures `a`, `b`. The `as` name is typed as the scrutinee for
        // the arm and is in scope in the arm body.
        ok(r#"
def describe(list[int64] xs):
    match xs:
        case [a, b] as pair:
            print(a, b, len(pair))
        case _:
            print("other")
"#);
        // `case Point(x=0) as origin:` binds `origin`, typed as the scrutinee
        // struct, so its fields are accessible in the body.
        ok(r#"
struct Point:
    int64 x
    int64 y
def at_origin(Point p):
    match p:
        case Point(x=0) as origin:
            print(origin.x, origin.y)
        case _:
            print("elsewhere")
"#);
    }

    #[test]
    fn as_capture_is_scrutinee_typed_and_visible_in_guard(/* #603, D11 */) {
        // Proof the `as` capture carries the scrutinee's type (not `Unknown`):
        // `n` flows into `int64` arithmetic. And it is in scope in the guard,
        // not only the body — `if n > 0` references the capture.
        ok(r#"
def classify(int64 v):
    match v:
        case 1 | 2 | 3 as n if n > 0:
            int64 doubled = n + n
            print(doubled)
        case other as whole:
            print(other, whole)
"#);
    }

    #[test]
    fn fib_type_checks() {
        ok(include_str!("../../../tests/golden/fib.zap"));
    }

    #[test]
    fn cfg_dead_arm_is_not_type_checked() {
        // Dead-arm skip (I#139, D-0079): the Windows arm calls an undefined
        // function. `analyze` resolves `when target.*` against the host, so on a
        // non-Windows host the arm is dead and must never be type-checked —
        // compiling must succeed with no errors. (The workspace test suite runs
        // on Linux hosts; guard the negative assertion so a future Windows
        // `cargo test` job, where that arm would be live, does not flake.)
        let src = "\
when target.os == .windows:
    def plat() -> int64:
        return undefined_windows_fn()
else:
    def plat() -> int64:
        return 0

def main():
    print(plat())
";
        if !cfg!(target_os = "windows") {
            ok(src);
        }
    }

    #[test]
    fn cfg_live_arm_is_type_checked() {
        // The complement: a type error in the *selected* arm is still reported.
        // The `else` arm is live on every non-Windows host and returns a `str`
        // where `int64` is declared, which must be a type error.
        let src = "\
when target.os == .windows:
    def plat() -> int64:
        return 0
else:
    def plat() -> int64:
        return \"not an int\"

def main():
    print(plat())
";
        if !cfg!(target_os = "windows") {
            assert!(
                !errors(src).is_empty(),
                "a type error in the live arm must be reported"
            );
        }
    }

    #[test]
    fn generator_drives_a_for_loop() {
        // A `def` with `yield` is a generator (I#67); its call is an Iterator
        // and drops into a `for` loop through the D-0048 protocol.
        ok(
            "def count(int64 n):\n    auto i = 0\n    while i < n:\n        yield i\n        \
            i = i + 1\n\ndef main():\n    auto total = 0\n    for x in count(5):\n        \
            total = total + x\n    print(total)\n",
        );
    }

    #[test]
    fn generator_feeds_a_comprehension() {
        ok(
            "def ups(int64 n):\n    auto i = 0\n    while i < n:\n        yield i * i\n        \
            i = i + 1\n\ndef main():\n    auto xs = [v for v in ups(4)]\n    print(len(xs))\n",
        );
    }

    #[test]
    fn generator_return_value_is_rejected() {
        assert!(!errors(
            "def g():\n    yield 1\n    return 7\n\ndef main():\n    for x in g():\n        \
             print(x)\n"
        )
        .is_empty());
    }

    #[test]
    fn generator_with_declared_return_type_is_rejected() {
        assert!(
            !errors("def g() -> int64:\n    yield 1\n\ndef main():\n    print(1)\n").is_empty()
        );
    }

    #[test]
    fn yield_without_value_is_rejected() {
        assert!(!errors(
            "def g():\n    yield\n\ndef main():\n    for x in g():\n        print(x)\n"
        )
        .is_empty());
    }

    #[test]
    fn generator_main_is_rejected() {
        assert!(!errors("def main():\n    yield 1\n").is_empty());
    }

    #[test]
    fn generator_method_is_accepted() {
        // A `self`-bearing method whose body `yield`s is a generator (I#429):
        // calling it is an Iterator (captured `self.start` persists across
        // resumes), so it drops into a `for` loop through the D-0048 protocol.
        ok(
            "struct Seq:\n    int64 start\n    def nums(self, int64 n):\n        auto i = 0\n        \
             while i < n:\n            yield self.start + i\n            i = i + 1\n\ndef main():\n    \
             auto total = 0\n    for x in Seq(start=10).nums(5):\n        total = total + x\n    \
             print(total)\n",
        );
    }

    #[test]
    fn generator_method_with_declared_return_type_is_rejected() {
        // A generator (method or free) must not declare a return type: its type
        // is the synthetic iterator (I#67, D-0048).
        assert!(!errors(
            "struct Seq:\n    def nums(self) -> int64:\n        yield 1\n\ndef main():\n    print(1)\n"
        )
        .is_empty());
    }

    #[test]
    fn generator_tier1_enum_method_is_rejected() {
        // A tier-1 (unboxed-integer) enum has no methods lowered yet (generator
        // or not), so a generator on one is rejected up front (ZAP5060). This is
        // the broader tier-1 method-lowering gap, not anything generator-specific.
        assert!(!errors(
            "enum E:\n    A\n    B\n    def nums(self):\n        yield 1\n\ndef main():\n    print(1)\n"
        )
        .is_empty());
    }

    #[test]
    fn generator_tier3_enum_method_is_accepted() {
        // A tier-3 (data-carrying) enum lowers to a tagged struct, so its methods
        // dispatch through the struct-method path — a generator method on one is
        // accepted and captures `self` into the state frame (I#449, like I#429).
        ok(
            "enum Box:\n    Hold(int64 start)\n    def nums(self, int64 n):\n        \
             match self:\n            case Box.Hold(start):\n                auto i = 0\n                \
             while i < n:\n                    yield start + i\n                    i = i + 1\n\n\
             def main():\n    auto total = 0\n    for x in Box.Hold(10).nums(5):\n        \
             total = total + x\n    print(total)\n",
        );
    }

    #[test]
    fn test_decorators_are_accepted() {
        // The whole `@test` suite (D-0054) passes decorator validation.
        ok("@test\ndef test_a():\n    assert True\n");
        ok("@before\ndef setup():\n    print(1)\n");
        ok("@after_all\ndef teardown():\n    print(1)\n");
        ok("@test\n@skip(\"later\")\ndef test_s():\n    assert False\n");
        ok("@test\n@skip_if(True, \"ci\")\ndef test_si():\n    assert False\n");
        ok("@test\n@expect_raises(ValueError)\ndef test_r():\n    raise \"x\"\n");
        ok("@test\n@parametrize(cases=[(1, 2)])\ndef test_p(int64 a, int64 b):\n    assert a < b\n");
    }

    #[test]
    fn test_decorator_misuse_is_reported() {
        // Wrong arity / shape produces a clear error (typo-catching, D-0016).
        assert!(!errors("@test(1)\ndef t():\n    assert True\n").is_empty());
        assert!(!errors("@expect_raises\ndef t():\n    assert True\n").is_empty());
        assert!(!errors("@parametrize([1, 2])\ndef t():\n    assert True\n").is_empty());
    }

    #[test]
    fn assert_and_expect_raises_check() {
        ok("def main():\n    assert 1 == 1, \"ok\"\n");
        ok("def boom():\n    raise \"x\"\n\
            def main():\n    with expect_raises(ValueError):\n        boom()\n");
    }

    #[test]
    fn loop_sum_type_checks() {
        ok(include_str!("../../../tests/golden/loop_sum.zap"));
    }

    #[test]
    fn coroutines_type_check() {
        // The full coroutines fixture type-checks: channel construction,
        // `spawn`, send/receive, and `close` (ticket #25, D-0013/D-0031).
        ok(include_str!("../../../tests/golden/coroutines.zap"));
    }

    #[test]
    fn structured_concurrency_fixtures_type_check() {
        // The structured-concurrency goldens type-check: `scope:` join, the
        // aggregate-failure path, cooperative cancellation via `checkpoint()`,
        // and `spawn detached` (ticket #50, D-0044/D-0045).
        ok(include_str!("../../../tests/golden/scope_join.zap"));
        ok(include_str!("../../../tests/golden/scope_group.zap"));
        ok(include_str!("../../../tests/golden/scope_cancel.zap"));
        ok(include_str!("../../../tests/golden/scope_detached.zap"));
    }

    #[test]
    fn channel_send_and_recv_are_typed() {
        // `<-ch` yields the element type, and a send of a compatible value is
        // accepted; the whole program type-checks.
        ok("def main():\n    auto ch = channel[int64]()\n    ch <- 7\n    int64 v = <-ch\n    print(v)\n");
    }

    #[test]
    fn channel_send_type_mismatch_errors() {
        // Sending a str on a channel[int64] is a type error.
        let e = errors("def main():\n    auto ch = channel[int64]()\n    ch <- \"x\"\n");
        assert_eq!(e.len(), 1, "expected a send-type error, got {e:?}");
    }

    #[test]
    fn send_requires_a_channel_on_the_left() {
        // `x <- v` where `x` is not a channel is rejected.
        let e = errors("def main():\n    auto x = 1\n    x <- 2\n");
        assert_eq!(e.len(), 1, "expected a non-channel send error, got {e:?}");
    }

    #[test]
    fn receive_requires_a_channel() {
        // `<-x` where `x` is not a channel is rejected.
        let e = errors("def main():\n    auto x = 1\n    auto y = <-x\n    print(y)\n");
        assert_eq!(
            e.len(),
            1,
            "expected a non-channel receive error, got {e:?}"
        );
    }

    #[test]
    fn close_requires_a_channel() {
        let e = errors("def main():\n    auto x = 1\n    close(x)\n");
        assert_eq!(
            e.len(),
            1,
            "expected a close-on-non-channel error, got {e:?}"
        );
    }

    #[test]
    fn scope_block_and_spawn_detached_type_check() {
        // A `scope:` with owned spawns, a `spawn detached`, `on_error=OnError.Wait`
        // and a `checkpoint()` all type-check (ticket #50, D-0044).
        ok("def f():\n    checkpoint()\n\ndef main():\n    scope(on_error=OnError.Wait):\n        spawn f()\n        spawn f()\n    spawn detached f()\n");
    }

    #[test]
    fn scope_on_error_rejects_bad_policy() {
        // A misspelled / non-OnError policy is a compile-time error, not a silent
        // runtime surprise (D-0044).
        let e = errors("def f():\n    print(1)\n\ndef main():\n    scope(on_error=OnError.Nope):\n        spawn f()\n");
        assert_eq!(
            e.len(),
            1,
            "expected a bad on_error policy error, got {e:?}"
        );
    }

    #[test]
    fn scope_timeout_typechecks() {
        ok(include_str!("../../../tests/conformance/scope_timeout.zap"));
        ok(include_str!(
            "../../../tests/conformance/scope_timeout_grace.zap"
        ));
        ok("def main():\n    scope(timeout=30_000, grace=100):\n        checkpoint()\n");
        ok("def main():\n    scope(timeout=10):\n        auto done = 1\n");
        assert!(
            errors("def main():\n    scope(timeout=1.5):\n        auto done = 1\n")
                .iter()
                .any(|e| e.code == ErrorCode::ZAP4024)
        );
    }

    #[test]
    fn checkpoint_takes_no_arguments() {
        let e = errors("def main():\n    checkpoint(1)\n");
        assert_eq!(e.len(), 1, "expected a checkpoint arity error, got {e:?}");
    }

    // --- signals (ticket #77, D-0058) ---

    #[test]
    fn signal_notify_and_gc_runtime_modules_type_check() {
        // `signal.notify` returns a channel[Signal]; `gc`/`runtime` module calls
        // type-check; the built-in `Signal` enum and `MemStats` struct are
        // available without an explicit declaration.
        ok("def main():\n\
            \x20   auto ch = signal.notify(Signal.SIGINT)\n\
            \x20   auto s = <-ch\n\
            \x20   gc.collect()\n\
            \x20   auto m = gc.stats()\n\
            \x20   print(m.heap_bytes)\n\
            \x20   print(runtime.num_coroutines())\n\
            \x20   runtime.coroutine_dump()\n");
    }

    #[test]
    fn signal_notify_rejects_non_signal() {
        let e = errors("def main():\n    auto ch = signal.notify(42)\n");
        assert!(!e.is_empty(), "expected signal.notify(non-Signal) to error");
    }

    #[test]
    fn runtime_scheduler_controls_type_check() {
        // Ticket #172: the MAXPROCS knob getter returns int64; LockOSThread pin /
        // unpin are statement calls.
        ok("def main():\n\
            \x20   print(runtime.maxprocs())\n\
            \x20   runtime.lock_os_thread()\n\
            \x20   runtime.unlock_os_thread()\n");
    }

    #[test]
    fn unknown_module_function_is_an_error() {
        let e = errors("def main():\n    gc.frobnicate()\n");
        assert!(!e.is_empty(), "expected unknown gc.* function to error");
    }

    #[test]
    fn math_module_functions_and_constants_type_check() {
        // Constants are attributes (`math.pi`); functions widen their args to
        // float64 (ticket #98). `import math` is accepted but not required.
        ok("import math\n\n\
            def main():\n\
            \x20   float64 a = math.sqrt(2.0)\n\
            \x20   float64 b = math.pow(2.0, 10.0)\n\
            \x20   float64 c = math.sin(math.pi)\n\
            \x20   float64 d = math.hypot(3.0, 4.0)\n\
            \x20   float64 e = math.log(math.e)\n\
            \x20   bool f = math.isnan(math.nan)\n\
            \x20   bool g = math.isinf(math.inf)\n\
            \x20   float64 h = math.floor(math.tau)\n\
            \x20   print(a + b + c + d + e + h)\n\
            \x20   print(f)\n\
            \x20   print(g)\n");
    }

    #[test]
    fn math_accepts_int_arguments() {
        // Integer arguments widen to float64 (ticket #98).
        ok("def main():\n    float64 r = math.sqrt(4)\n    print(r)\n");
    }

    #[test]
    fn unknown_math_function_and_constant_are_errors() {
        assert!(
            !errors("def main():\n    auto x = math.frobnicate(1.0)\n").is_empty(),
            "expected unknown math.* function to error"
        );
        // `math.quux` is not a known constant, so the base resolves as a value
        // and fails (no `math` binding).
        assert!(
            !errors("def main():\n    auto x = math.quux\n").is_empty(),
            "expected unknown math constant to error"
        );
    }

    #[test]
    fn random_module_surface_type_checks() {
        // `choice` yields the list element type; `shuffle` returns unit
        // (ticket #98).
        ok("import random\n\n\
            def main():\n\
            \x20   random.seed(42)\n\
            \x20   float64 a = random.random()\n\
            \x20   float64 b = random.uniform(1.0, 2.0)\n\
            \x20   int64 c = random.randint(1, 6)\n\
            \x20   auto xs = [10, 20, 30]\n\
            \x20   int64 pick = random.choice(xs)\n\
            \x20   random.shuffle(xs)\n\
            \x20   print(a + b)\n\
            \x20   print(c + pick)\n");
    }

    #[test]
    fn random_choice_requires_a_list() {
        let e = errors("def main():\n    auto x = random.choice(5)\n");
        assert!(!e.is_empty(), "expected random.choice(non-list) to error");
    }

    #[test]
    fn logging_module_surface_type_checks() {
        // Level thresholds are int64 attributes (`logging.DEBUG`), emitters and
        // config setters return unit, `get_level` returns int64 (ticket #101).
        // `import logging` is accepted but not required.
        ok("import logging\n\n\
            def main():\n\
            \x20   logging.set_level(logging.DEBUG)\n\
            \x20   logging.set_stream(sys.stderr_fd())\n\
            \x20   logging.set_format(\"%(levelname)s:%(message)s\")\n\
            \x20   logging.debug(\"d\")\n\
            \x20   logging.info(\"i\")\n\
            \x20   logging.warning(\"w\")\n\
            \x20   logging.error(\"e\")\n\
            \x20   logging.critical(\"c\")\n\
            \x20   logging.log(logging.ERROR, \"x\")\n\
            \x20   int64 lvl = logging.get_level()\n\
            \x20   print(lvl)\n");
    }

    #[test]
    fn unknown_logging_function_and_level_are_errors() {
        assert!(
            !errors("def main():\n    logging.frobnicate(\"x\")\n").is_empty(),
            "expected unknown logging.* function to error"
        );
        // `logging.NOTSET` is not a known level, so the base resolves as a value
        // and fails (no `logging` binding).
        assert!(
            !errors("def main():\n    auto x = logging.NOTSET\n").is_empty(),
            "expected unknown logging level to error"
        );
    }

    #[test]
    fn argparse_module_surface_type_checks() {
        // Declarations/parse return unit; typed getters return str/int64/bool;
        // `format_help` -> str, `help_requested` -> bool (ticket #101).
        ok("import argparse\n\n\
            def main():\n\
            \x20   argparse.set_prog(\"demo\")\n\
            \x20   argparse.set_description(\"desc\")\n\
            \x20   argparse.add_flag(\"verbose\")\n\
            \x20   argparse.add_option(\"name\", \"anon\")\n\
            \x20   argparse.add_int(\"count\", 1)\n\
            \x20   argparse.add_positional(\"src\")\n\
            \x20   auto argv = [\"prog\", \"src.txt\"]\n\
            \x20   argparse.parse(argv)\n\
            \x20   str n = argparse.get(\"name\")\n\
            \x20   int64 c = argparse.get_int(\"count\")\n\
            \x20   str h = argparse.format_help()\n\
            \x20   if argparse.get_flag(\"verbose\"):\n\
            \x20       print(n)\n\
            \x20   if argparse.help_requested():\n\
            \x20       print(h)\n\
            \x20   print(c)\n\
            \x20   argparse.reset()\n");
    }

    #[test]
    fn unknown_argparse_function_is_an_error() {
        assert!(
            !errors("def main():\n    argparse.frobnicate()\n").is_empty(),
            "expected unknown argparse.* function to error"
        );
    }

    #[test]
    fn concurrent_module_surface_type_checks() {
        // `concurrent.Counter()`/`Counter(start)` yield the prelude `Counter`
        // (its `add`/`incr`/`get` return int64); the MPSC `Queue[T]` is a bare
        // prelude generic built with `new_queue[T]()`, with comma-ok `pop`
        // (ticket #184, D-0078). Both pointer and non-pointer payloads type-check.
        ok("import concurrent\n\n\
            def main():\n\
            \x20   Counter c = concurrent.Counter()\n\
            \x20   print(c.get())\n\
            \x20   print(c.add(2))\n\
            \x20   print(c.incr())\n\
            \x20   Counter c2 = concurrent.Counter(10)\n\
            \x20   print(c2.get())\n\
            \x20   Queue[int64] q = new_queue()\n\
            \x20   q.push(5)\n\
            \x20   auto v, ok = q.pop()\n\
            \x20   if ok:\n\
            \x20       print(v.unwrap())\n\
            \x20   Queue[str] qs = new_queue[str]()\n\
            \x20   qs.push(\"x\")\n");
    }

    #[test]
    fn unknown_concurrent_function_is_an_error() {
        assert!(
            !errors("def main():\n    concurrent.frobnicate()\n").is_empty(),
            "expected unknown concurrent.* function to error"
        );
    }

    #[test]
    fn on_signal_decorator_type_checks() {
        ok("@on_signal(Signal.SIGTERM)\ndef cleanup():\n    print(1)\n\ndef main():\n    print(0)\n");
    }

    #[test]
    fn on_signal_rejects_non_signal_argument() {
        let e =
            errors("@on_signal(42)\ndef cleanup():\n    print(1)\n\ndef main():\n    print(0)\n");
        assert!(!e.is_empty(), "expected @on_signal(non-Signal) to error");
    }

    #[test]
    fn scope_cancel_on_and_grace_type_check() {
        ok("def f():\n    print(1)\n\ndef main():\n\
            \x20   scope(cancel_on=[Signal.SIGINT, Signal.SIGTERM], grace=5000):\n\
            \x20       spawn f()\n");
    }

    #[test]
    fn scope_cancel_on_rejects_non_signal_list() {
        let e = errors(
            "def f():\n    print(1)\n\ndef main():\n\
            \x20   scope(cancel_on=[1, 2]):\n        spawn f()\n",
        );
        assert!(!e.is_empty(), "expected cancel_on=[non-Signal] to error");
    }

    #[test]
    fn regex_type_checks() {
        // The full regex fixture type-checks (match objects, captures, `!~`,
        // substitution) — ticket #23, D-0010.
        ok(include_str!("../../../tests/golden/regex.zap"));
    }

    #[test]
    fn regex_subject_must_be_a_string() {
        // The left operand of `=~`/`!~` and of `s///` must be a str.
        let e = errors("def main():\n    auto m = 42 =~ m/x/\n");
        assert_eq!(e.len(), 1, "expected a subject-type error, got {e:?}");
        let e = errors("def main():\n    auto s = 42 =~ s/a/b/\n");
        assert_eq!(e.len(), 1, "expected a subject-type error, got {e:?}");
    }

    #[test]
    fn auto_inference() {
        ok("auto x = 1\nauto y = x + 2\nint64 z = y\n");
    }

    #[test]
    fn widening_is_allowed() {
        ok("int32 a = 0\nint64 b = a\nfloat64 c = b\n");
    }

    #[test]
    fn narrowing_is_rejected() {
        let e = errors("int64 b = 0\nint32 c = b\n");
        assert_eq!(e.len(), 1, "expected one narrowing error, got {e:?}");
    }

    #[test]
    fn explicit_conversion_allows_narrowing() {
        // The explicit cast is the sanctioned way around the narrowing rule
        // (D-0020), and also crosses the int/float boundary.
        ok("int64 b = 0\nint32 c = int32(b)\n");
        ok("float64 x = 1.5\nint32 n = int32(x)\nfloat32 f = float32(x)\n");
    }

    #[test]
    fn conversion_rejects_non_numeric() {
        let e = errors("auto s = \"hi\"\nint32 n = int32(s)\n");
        assert_eq!(
            e.len(),
            1,
            "expected a non-numeric conversion error, got {e:?}"
        );
    }

    #[test]
    fn power_and_floordiv_type_check() {
        // `**` and `//` are ordinary numeric ops: int result for ints, float
        // when a float operand is present; `/` is always float (D-0024).
        ok("int64 p = 2 ** 10\nint64 q = 7 // 2\nfloat64 r = 2.0 ** 0.5\nfloat64 s = 7.0 // 2.0\nfloat64 t = 7 / 2\n");
    }

    #[test]
    fn type_mismatch_in_declaration() {
        let e = errors("int64 x = \"hi\"\n");
        assert_eq!(e.len(), 1);
    }

    #[test]
    fn unknown_variable() {
        let e = errors("x = 1\n");
        assert_eq!(e.len(), 1);
    }

    #[test]
    fn unknown_function() {
        let e = errors("def main():\n    nope(1)\n");
        assert_eq!(e.len(), 1);
    }

    #[test]
    fn return_type_mismatch() {
        let e = errors("def f() -> int64:\n    return \"x\"\n");
        assert_eq!(e.len(), 1);
    }

    #[test]
    fn call_arity_and_type() {
        // fib wants one int64; passing a string and too many args errors.
        let e = errors("def fib(int64 n) -> int64:\n    return n\ndef main():\n    print(fib(\"x\"))\n    print(fib(1, 2))\n");
        assert_eq!(e.len(), 2, "got {e:?}");
    }

    #[test]
    fn string_concat_ok() {
        ok("def greet(str name) -> str:\n    return \"hi \" + name\n");
    }

    #[test]
    fn for_over_range_binds_int() {
        ok("def main():\n    auto s = 0\n    for i in range(0, 10):\n        s = s + i\n");
    }

    #[test]
    fn array_programs_type_check() {
        // Arrays are typed loosely (ticket #24): construction, element-wise ops,
        // scalar broadcast, indexing, len, and sum must not raise false errors.
        // This covers the uint8/bool dtypes and mixed-dtype promotion (#49) too,
        // since those stay loosely typed at the sema layer (the dtype lives in
        // the ZIR lowering).
        ok(include_str!("../../../tests/golden/array_int.zap"));
        ok(include_str!("../../../tests/golden/array_f64.zap"));
        ok(include_str!("../../../tests/golden/array_promote.zap"));
        ok(include_str!("../../../tests/golden/array_uint8.zap"));
        ok(include_str!("../../../tests/golden/array_bool.zap"));
        ok(include_str!("../../../tests/golden/array_division.zap"));
        ok(include_str!("../../../tests/golden/array_reductions.zap"));
        ok(include_str!("../../../tests/golden/array_strided.zap"));
        ok(include_str!("../../../tests/golden/array_matmul.zap"));
        ok(include_str!("../../../tests/golden/array_shape_ops.zap"));
        ok(include_str!("../../../tests/golden/array_fancy_mask.zap"));
    }

    // --- decorators & annotations (ticket #16, D-0036) ---

    #[test]
    fn decorators_golden_type_checks() {
        ok(include_str!("../../../tests/golden/decorators.zap"));
    }

    #[test]
    fn unknown_decorator_is_error() {
        // Compile-time typo-catching (D-0016): an undeclared `@name` is rejected.
        let e = errors("@frobnicate\nstruct S:\n    int64 x\n");
        assert_eq!(e.len(), 1, "got {e:?}");
        assert!(e[0].message.contains("unknown decorator"), "{e:?}");
    }

    #[test]
    fn known_annotation_and_decorator_accepted() {
        ok("annotation Tag:\n    str name = \"\"\n\n@Tag(name=\"x\")\nstruct S:\n    int64 x\n");
    }

    #[test]
    fn annotation_unknown_field_is_error() {
        let e = errors(
            "annotation Tag:\n    str name = \"\"\n\n@Tag(nope=\"x\")\nstruct S:\n    int64 x\n",
        );
        assert_eq!(e.len(), 1, "got {e:?}");
        assert!(e[0].message.contains("no field 'nope'"), "{e:?}");
    }

    #[test]
    fn derive_unknown_trait_is_error() {
        let e = errors("@derive(Nonsense)\nstruct S:\n    int64 x\n");
        assert_eq!(e.len(), 1, "got {e:?}");
        assert!(e[0].message.contains("unknown derivable trait"), "{e:?}");
    }

    #[test]
    fn derive_needs_a_trait() {
        let e = errors("@derive\nstruct S:\n    int64 x\n");
        assert_eq!(e.len(), 1, "got {e:?}");
    }

    #[test]
    fn derive_only_on_structs() {
        let e = errors("@derive(Eq)\ndef f():\n    print(1)\n");
        assert_eq!(e.len(), 1, "got {e:?}");
        assert!(e[0].message.contains("applies to a struct"), "{e:?}");
    }

    // --- operator overloading (ticket #52, D-0047) ---

    #[test]
    fn operator_on_struct_requires_the_dunder() {
        // `a + b` on a struct with no `__add__` is a clear compile error that
        // names the dunder and interface (D-0047).
        let e = errors(
            "struct V:\n    int64 x\n\ndef main():\n    \
             auto a = V(x=1)\n    auto b = V(x=2)\n    auto c = a + b\n",
        );
        assert_eq!(e.len(), 1, "got {e:?}");
        assert!(e[0].message.contains("__add__"), "{e:?}");
        assert!(e[0].message.contains("Add"), "{e:?}");
    }

    #[test]
    fn comparison_on_struct_requires_cmp() {
        // `<` needs `__cmp__`; the error names it and the `Ord` interface.
        let e = errors(
            "struct V:\n    int64 x\n\ndef main():\n    \
             auto a = V(x=1)\n    auto b = V(x=2)\n    if a < b:\n        print(1)\n",
        );
        assert_eq!(e.len(), 1, "got {e:?}");
        assert!(e[0].message.contains("__cmp__"), "{e:?}");
    }

    #[test]
    fn equality_on_struct_requires_eq() {
        let e = errors(
            "struct V:\n    int64 x\n\ndef main():\n    \
             auto a = V(x=1)\n    auto b = V(x=2)\n    if a == b:\n        print(1)\n",
        );
        assert_eq!(e.len(), 1, "got {e:?}");
        assert!(e[0].message.contains("__eq__"), "{e:?}");
    }

    #[test]
    fn hand_written_dunders_make_operators_type_check() {
        // A struct defining the dunders satisfies the operator interfaces; the
        // operator's result type is the dunder's return type.
        ok("struct V:\n    int64 x\n\n    \
            def __add__(self, V other) -> V:\n        return V(x=self.x)\n\n    \
            def __eq__(self, V other) -> bool:\n        return True\n\n    \
            def __cmp__(self, V other) -> int64:\n        return 0\n\n\
            def main():\n    auto a = V(x=1)\n    auto b = V(x=2)\n    \
            auto c = a + b\n    if a == b:\n        print(1)\n    \
            if a < b:\n        print(2)\n");
    }

    #[test]
    fn aug_assign_accepts_inplace_only_dunder() {
        // A struct that defines only `__iadd__` (no `__add__`) type-checks `+=`:
        // the assignment calls the in-place dunder (D-0047/D-0023).
        ok("struct Acc:\n    int64 total\n\n    \
            def __iadd__(self, Acc other) -> Acc:\n        \
            self.total = self.total + other.total\n        return self\n\n\
            def main():\n    auto a = Acc(total=1)\n    auto b = Acc(total=2)\n    \
            a += b\n    print(a.total)\n");
    }

    #[test]
    fn aug_assign_falls_back_to_binary_dunder() {
        // With only `__add__` defined, `+=` still type-checks by deriving from
        // the binary operator.
        ok("struct V:\n    int64 x\n\n    \
            def __add__(self, V other) -> V:\n        return V(x=self.x + other.x)\n\n\
            def main():\n    auto a = V(x=1)\n    auto b = V(x=2)\n    \
            a += b\n    print(a.x)\n");
    }

    #[test]
    fn aug_assign_without_either_dunder_errors() {
        // `+=` on a struct that defines neither `__iadd__` nor `__add__` is a
        // clean error naming the binary dunder it would fall back to.
        let e = errors(
            "struct V:\n    int64 x\n\ndef main():\n    \
             auto a = V(x=1)\n    auto b = V(x=2)\n    a += b\n",
        );
        assert_eq!(e.len(), 1, "got {e:?}");
        assert!(e[0].message.contains("__add__"), "{e:?}");
    }

    #[test]
    fn user_struct_method_return_type_is_resolved() {
        // A user-struct method call `recv.method(args)` is typed by the method's
        // declared return type (D-0009), not the former loose `Unknown`. The
        // result is assignable to a binding of that declared type.
        ok("struct Box:\n    int64 n\n\n    \
            def make(self, int64 v) -> Box:\n        return Box(n=v)\n\n\
            def main():\n    Box b = Box(n=1).make(2)\n    print(b.n)\n");
        // Crucially the result is the *concrete* struct type, not `Unknown`:
        // assigning it to an incompatible declared type is a real error. An
        // `Unknown` result would have silently passed (Unknown is assignable to
        // anything), so this is what distinguishes the fix from the old behavior.
        let e = errors(
            "struct Box:\n    int64 n\n\n    \
             def make(self, int64 v) -> Box:\n        return Box(n=v)\n\n\
             def main():\n    int64 bad = Box(n=1).make(2)\n    print(bad)\n",
        );
        assert!(
            !e.is_empty(),
            "method result should be typed Box, not Unknown: {e:?}"
        );
    }

    #[test]
    fn method_chain_result_drives_operator_overloading() {
        // The gap this fixes (D-0009): a method returning its own struct, chained
        // into an overloaded operator, must dispatch that operator on the struct.
        // Before the fix the method result was typed `Unknown`, so `Unknown &
        // Unknown` (and `~Unknown`) collapsed to the built-in bitwise `int64` rule
        // and the hand-written `__and__` / `__or__` / `__invert__` never ran —
        // exactly what blocked the fluent `Key("pk").eq(x) & Attr("age").gt(y)`
        // condition builders (operators #52) the generated AWS SDK emits. The
        // `Cond` bindings below pin each operator result to `Cond`; were any result
        // `int64`, the assignment would be a type error.
        ok("struct Cond:\n    int64 tag\n\n    \
            def __and__(self, Cond other) -> Cond:\n        return Cond(tag=self.tag)\n\n    \
            def __or__(self, Cond other) -> Cond:\n        return Cond(tag=self.tag)\n\n    \
            def __invert__(self) -> Cond:\n        return Cond(tag=self.tag)\n\n    \
            def eq(self, int64 v) -> Cond:\n        return Cond(tag=v)\n\n\
            def main():\n    \
            Cond both = Cond(tag=0).eq(1) & Cond(tag=0).eq(2)\n    \
            Cond either = Cond(tag=0).eq(1) | Cond(tag=0).eq(2)\n    \
            Cond neg = ~Cond(tag=0).eq(3)\n    \
            print(both.tag + either.tag + neg.tag)\n");
    }

    #[test]
    fn derive_ord_and_hash_are_accepted() {
        // @derive(Eq, Ord, Hash, Repr) satisfies ==, <, and makes the type a key.
        ok("@derive(Eq, Ord, Hash, Repr)\nstruct V:\n    int64 x\n\n\
            def main():\n    auto a = V(x=1)\n    auto b = V(x=2)\n    \
            if a == b:\n        print(1)\n    if a < b:\n        print(2)\n    \
            auto s = {a, b}\n");
    }

    #[test]
    fn eq_without_hash_key_warns() {
        // A struct with equality but no `__hash__` used as a set key warns
        // (non-fatal) — D-0047 invariant guard.
        let w = warnings_str(
            "@derive(Eq)\nstruct V:\n    int64 x\n\ndef main():\n    \
             auto a = V(x=1)\n    auto s = {a}\n",
        );
        assert_eq!(w.len(), 1, "expected one warning, got {w:?}");
        assert!(w[0].message.contains("__hash__"), "{w:?}");
    }

    #[test]
    fn eq_with_hash_key_does_not_warn() {
        // Deriving Hash alongside Eq silences the dict/set-key warning.
        let w = warnings_str(
            "@derive(Eq, Hash)\nstruct V:\n    int64 x\n\ndef main():\n    \
             auto a = V(x=1)\n    auto d = {a: 1}\n",
        );
        assert!(w.is_empty(), "unexpected warnings: {w:?}");
    }

    #[test]
    fn builtin_arg_shapes_checked() {
        // @default takes exactly one expr.
        assert_eq!(
            errors("struct S:\n    @default(1, 2)\n    int64 x\n").len(),
            1
        );
    }

    // ---- @cache shipped (I#666, D-0115); @inject declined (I#472, D-0109) --

    #[test]
    fn cache_accepted_on_function_rejected_off_function() {
        // `@cache` (I#666, D-0115 — supersedes the D-0109 deferral now that
        // module-level state exists) is a free-function decorator expanded into
        // real Zap *before* sema. Sema itself raises nothing on a function (the
        // expansion and the hashable-signature checks live in the driver pass as
        // ZAP5056); a correct application never even reaches the cache arm,
        // because the pass consumes it. Misapplied to a non-function target it is
        // a clean require-target error (ZAP5047).
        assert!(
            errors("@cache\ndef f(int64 n) -> int64:\n    return n\n").is_empty(),
            "@cache on a function should not be a sema error"
        );
        let e = errors("@cache\nstruct S:\n    int64 x\n");
        assert_eq!(e.len(), 1, "expected one diagnostic, got {e:?}");
        assert_eq!(e[0].code, ErrorCode::ZAP5047);
    }

    #[test]
    fn inject_is_declined_for_core() {
        // `@inject` is declined for core (I#472, D-0109): dependency injection is
        // provided by reflection-driven `Depends(...)` adapters (D-0089/D-0092),
        // not a core decorator; rejected cleanly at sema with ZAP5055.
        let e = errors("struct S:\n    @inject\n    int64 x\n");
        assert_eq!(e.len(), 1, "expected one diagnostic, got {e:?}");
        assert_eq!(e[0].code, ErrorCode::ZAP5055);
        let e = errors("struct S:\n    @inject(1)\n    int64 x\n");
        assert_eq!(e.len(), 1, "expected one diagnostic, got {e:?}");
        assert_eq!(e[0].code, ErrorCode::ZAP5055);
    }

    // ---- @deprecated use-site warning (I#472, D-0109) ---------------------

    /// Convenience: the non-fatal warnings for `src` (no lex/parse errors).
    fn warns(src: &str) -> Vec<TypeError> {
        let (module, perrs, lerrs) = zapc_syntax::parser::parse_str(src);
        assert!(lerrs.is_empty(), "lex errors: {lerrs:?}");
        assert!(perrs.is_empty(), "parse errors: {perrs:?}");
        analyze(&module).1
    }

    #[test]
    fn deprecated_fn_warns_at_call_site() {
        // A bare `@deprecated` free function: every call site warns (ZAP5124),
        // non-fatally (no errors).
        let src = "@deprecated\ndef old() -> int64:\n    return 1\n\n\
                   def main():\n    print(old())\n";
        assert!(errors(src).is_empty(), "deprecation must be non-fatal");
        let w = warns(src);
        assert_eq!(w.len(), 1, "expected one deprecation warning, got {w:?}");
        assert_eq!(w[0].code, ErrorCode::ZAP5124);
        assert!(
            w[0].message.contains("old"),
            "message names the callee: {w:?}"
        );
    }

    #[test]
    fn deprecated_fn_message_is_included() {
        let src = "@deprecated(\"use new_api\")\ndef old() -> int64:\n    return 1\n\n\
                   def main():\n    print(old())\n";
        let w = warns(src);
        assert_eq!(w.len(), 1, "expected one warning, got {w:?}");
        assert!(
            w[0].message.contains("use new_api"),
            "the user message must be appended: {w:?}"
        );
    }

    #[test]
    fn deprecated_fn_warns_once_per_call_site() {
        // Two call sites → two warnings; a non-deprecated function → none.
        let src = "@deprecated\ndef old() -> int64:\n    return 1\n\
                   \ndef fresh() -> int64:\n    return 2\n\n\
                   def main():\n    print(old())\n    print(old())\n    print(fresh())\n";
        let w = warns(src);
        assert_eq!(w.len(), 2, "one warning per call site, got {w:?}");
        assert!(w.iter().all(|d| d.code == ErrorCode::ZAP5124));
    }

    #[test]
    fn non_deprecated_call_does_not_warn() {
        let src = "def fresh() -> int64:\n    return 1\n\n\
                   def main():\n    print(fresh())\n";
        assert!(warns(src).is_empty(), "no deprecation warning expected");
    }

    #[test]
    fn deprecated_method_warns_at_call_site() {
        // `@deprecated` on a method warns at the call site, keyed `Type.method`.
        let src = "struct S:\n    int64 x\n\n    \
                   @deprecated(\"use y()\")\n    def legacy(self) -> int64:\n        return self.x\n\n\
                   def main():\n    auto s = S(x=1)\n    print(s.legacy())\n";
        assert!(
            errors(src).is_empty(),
            "deprecation must be non-fatal: {:?}",
            errors(src)
        );
        let w = warns(src);
        assert_eq!(
            w.len(),
            1,
            "expected one method-deprecation warning, got {w:?}"
        );
        assert_eq!(w[0].code, ErrorCode::ZAP5124);
        assert!(w[0].message.contains("S.legacy") && w[0].message.contains("use y()"));
    }

    #[test]
    fn deprecated_stacks_with_reflection_decorator() {
        // `@deprecated` composes with a declared annotation on the same function
        // (bottom-up): the reflection metadata still records, and the call still
        // warns. No error.
        let src = "annotation Audited:\n    pass\n\n\
                   @Audited\n@deprecated\ndef old() -> int64:\n    return 1\n\n\
                   def main():\n    print(old())\n    print(reflect_has(\"old\", \"Audited\"))\n";
        assert!(
            errors(src).is_empty(),
            "unexpected errors: {:?}",
            errors(src)
        );
        let w = warns(src);
        assert_eq!(w.len(), 1, "expected one deprecation warning, got {w:?}");
        assert_eq!(w[0].code, ErrorCode::ZAP5124);
    }

    #[test]
    fn deprecated_wrong_target_rejected() {
        // Restricted to a function/method target (type/field/parameter
        // deprecation has no well-defined v1 use site): ZAP5047.
        let e = errors("@deprecated\nstruct S:\n    int64 x\n");
        assert!(
            e.iter().any(|d| d.code == ErrorCode::ZAP5047),
            "expected a wrong-target error, got {e:?}"
        );
        let e = errors("struct S:\n    @deprecated\n    int64 x\n");
        assert!(
            e.iter().any(|d| d.code == ErrorCode::ZAP5047),
            "expected a wrong-target error on a field, got {e:?}"
        );
    }

    #[test]
    fn deprecated_bad_message_arg_rejected() {
        // A non-string / multi-arg / keyword message is ZAP5042.
        assert!(errors("@deprecated(1)\ndef f() -> int64:\n    return 1\n")
            .iter()
            .any(|d| d.code == ErrorCode::ZAP5042));
        assert!(
            errors("@deprecated(\"a\", \"b\")\ndef f() -> int64:\n    return 1\n")
                .iter()
                .any(|d| d.code == ErrorCode::ZAP5042)
        );
    }

    #[test]
    fn field_decorator_validation_checked() {
        // `@default`/`@validate`/`@range` field decorators (D-0101) now parse and
        // type-check on well-formed, suitably-typed fields.
        ok(
            "struct S:\n    @default(now())\n    int64 created\n    @validate(max_len=50)\n    \
            str name\n    @range(min=0, max=150)\n    int64 age\n\ndef now() -> int64:\n    \
            return 0\n\ndef main():\n    auto s = S(name=\"a\", age=1)\n    print(s.age)\n",
        );
        // `@range` on a non-numeric field.
        assert!(
            errors("struct S:\n    @range(min=0, max=5)\n    str name\n")
                .iter()
                .any(|e| e.code == ErrorCode::ZAP5039),
            "@range on str should be ZAP5039"
        );
        // `@validate(max_len)` on a non-string field.
        assert!(
            errors("struct S:\n    @validate(max_len=5)\n    int64 n\n")
                .iter()
                .any(|e| e.code == ErrorCode::ZAP5040),
            "@validate(max_len) on int should be ZAP5040"
        );
        // A field may not carry both a literal default and `@default(...)`.
        assert!(
            errors("struct S:\n    @default(0)\n    int64 n = 5\n")
                .iter()
                .any(|e| e.code == ErrorCode::ZAP5038),
            "double default should be ZAP5038"
        );
        // Unknown `@validate` constraint (`matches` is a documented follow-up).
        assert!(
            errors("struct S:\n    @validate(matches=5)\n    str name\n")
                .iter()
                .any(|e| e.code == ErrorCode::ZAP5040),
            "unknown @validate constraint should be ZAP5040"
        );
        // `@range(min, max)` with min greater than max.
        assert!(
            errors("struct S:\n    @range(min=10, max=0)\n    int64 n\n")
                .iter()
                .any(|e| e.code == ErrorCode::ZAP5039),
            "inverted @range bounds should be ZAP5039"
        );
    }

    #[test]
    fn reflection_builtins_type_check() {
        ok("def main():\n    if reflect_has(\"S\", \"Tag\"):\n        print(reflect_names(\"S\"))\n");
        // Non-string argument is rejected.
        let e = errors("def main():\n    print(reflect_has(1, 2))\n");
        assert!(!e.is_empty(), "reflect_has should want str args");
    }

    #[test]
    fn signature_reflection_builtins_type_check() {
        // Signature reflection (I#458, D-0089): string-keyed queries returning
        // str / bool. A declared annotation on a parameter is accepted.
        ok("annotation Inject:\n    str service = \"\"\n\n\
            def h(str name, @Inject int64 dep, str q = \"x\") -> str:\n    return name\n\n\
            def main():\n    print(reflect_params(\"h\"))\n    print(reflect_sig(\"h\"))\n    \
            print(reflect_param_type(\"h\", \"name\"))\n    \
            print(reflect_param_annotations(\"h\", \"dep\"))\n    \
            if reflect_param_has_default(\"h\", \"q\"):\n        print(\"d\")\n");
        // Wrong arity: the per-parameter queries take exactly two string args.
        let e = errors("def main():\n    print(reflect_param_type(\"h\"))\n");
        assert!(!e.is_empty(), "reflect_param_type wants 2 args");
        // Non-string argument is rejected.
        let e = errors("def main():\n    print(reflect_params(1))\n");
        assert!(!e.is_empty(), "reflect_params should want a str arg");
    }

    // --- Python interop (ticket #27, D-0012) ---

    #[test]
    fn python_interop_golden_type_checks() {
        // The golden program type-checks: imported module aliases resolve, and
        // attribute access / calls on Python handles plus scalar conversions are
        // accepted (Python values are dynamic).
        ok(include_str!("../../../tests/golden/python_interop.zap"));
    }

    #[test]
    fn python_module_alias_is_in_scope() {
        // Referencing an imported alias is not an "unknown name" error, and a
        // Python call result flows as a dynamic value usable anywhere.
        ok("import python \"math\" as m\n\ndef main():\n    auto r = m.sqrt(2.0)\n    print(int64(r))\n");
        ok("import python \"json\" as j\n\ndef main():\n    auto s = j.dumps(\"x\")\n    print(s)\n");
    }

    #[test]
    fn python_conversion_accepts_a_handle() {
        // `int64(...)` / `float64(...)` accept a Python handle (they marshal it
        // back), where a non-numeric Zap value would be rejected.
        ok("import python \"math\" as m\n\ndef main():\n    int64 g = int64(m.gcd(6, 4))\n    print(g)\n");
    }

    #[test]
    fn pyobject_type_annotation_is_accepted() {
        // The opaque `pyobject` handle type can be written explicitly.
        ok("import python \"math\" as m\n\ndef main():\n    pyobject r = m.sqrt(2.0)\n    print(r)\n");
    }

    // --- enums & match (ticket #51, D-0018/D-0040/D-0046) ---

    fn msgs(src: &str) -> String {
        errors(src)
            .iter()
            .map(|e| e.message.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn tier1_enum_members_conversions_and_exhaustive_match() {
        // Member access, explicit int<->enum conversions, and an exhaustive
        // match all type-check (D-0018).
        ok("enum Color:\n    RED\n    GREEN = 10\n    BLUE\n\n\
            def main():\n    auto c = Color.GREEN\n\
            \x20   int64 n = int64(c)\n    auto d = Color(n)\n\
            \x20   match c:\n        case Color.RED:\n            print(1)\n\
            \x20       case Color.GREEN:\n            print(2)\n\
            \x20       case Color.BLUE:\n            print(3)\n");
    }

    #[test]
    fn tier3_enum_type_iteration_is_rejected_in_sema() {
        let m = msgs(
            "enum Shape:\n    Circle(float64 radius)\n\n\
             def main():\n    for shape in Shape:\n        print(shape)\n",
        );
        assert!(
            m.contains("only tier-1 enum types can be iterated"),
            "expected an early tier-3 iteration diagnostic, got:\n{m}"
        );
    }

    #[test]
    fn enum_match_missing_variant_is_an_error() {
        // D-0040 exhaustiveness: omitting a variant with no `_` is a compile
        // error naming the missing variant.
        let m = msgs(
            "enum Color:\n    RED\n    GREEN\n    BLUE\n\n\
            def main():\n    auto c = Color.RED\n    match c:\n\
            \x20       case Color.RED:\n            print(1)\n\
            \x20       case Color.GREEN:\n            print(2)\n",
        );
        assert!(
            m.contains("non-exhaustive") && m.contains("Color.BLUE"),
            "expected a non-exhaustive error naming Color.BLUE, got:\n{m}"
        );
    }

    #[test]
    fn enum_match_wildcard_is_exhaustive() {
        // A `_` arm makes any enum match exhaustive.
        ok("enum Color:\n    RED\n    GREEN\n    BLUE\n\n\
            def main():\n    auto c = Color.RED\n    match c:\n\
            \x20       case Color.RED:\n            print(1)\n\
            \x20       case _:\n            print(0)\n");
    }

    #[test]
    fn tier3_construction_and_destructuring_type_check() {
        // Data-carrying variants construct with named or positional args and
        // destructure positionally or by name; payload captures are typed
        // (D-0046). The exhaustive match needs no `_`.
        ok(
            "enum Shape:\n    Circle(f64 r)\n    Rect(f64 w, f64 h)\n    Unit\n\n\
            def area(Shape s) -> f64:\n    match s:\n\
            \x20       case Shape.Circle(r):\n            return 3.14 * r * r\n\
            \x20       case Shape.Rect(w=w, h=h):\n            return w * h\n\
            \x20       case Shape.Unit:\n            return 1.0\n\n\
            def main():\n\
            \x20   print(area(Shape.Circle(r=2.0)))\n\
            \x20   print(area(Shape.Rect(3.0, 4.0)))\n",
        );
    }

    #[test]
    fn tier3_unknown_variant_and_field_are_errors() {
        let m = msgs(
            "enum Shape:\n    Circle(f64 r)\n    Unit\n\n\
            def main():\n    auto s = Shape.Square(1.0)\n",
        );
        assert!(m.contains("no variant 'Square'"), "got:\n{m}");
        let m = msgs(
            "enum Shape:\n    Circle(f64 r)\n    Unit\n\n\
            def main():\n    auto s = Shape.Circle(radius=1.0)\n",
        );
        assert!(m.contains("no field 'radius'"), "got:\n{m}");
    }

    #[test]
    fn recursive_enum_type_checks() {
        // A (mutually) recursive variant field references the enum itself
        // (D-0046); the whole vertical type-checks.
        ok("enum Expr:\n    Num(int64 v)\n    Add(Expr l, Expr r)\n\n\
            def eval(Expr e) -> int64:\n    match e:\n\
            \x20       case Expr.Num(v):\n            return v\n\
            \x20       case Expr.Add(l, r):\n            return eval(l) + eval(r)\n\n\
            def main():\n    print(eval(Expr.Add(Expr.Num(2), Expr.Num(3))))\n");
    }

    #[test]
    fn enum_is_a_distinct_type_from_int() {
        // An enum is its own type: assigning a raw int to an enum (or vice
        // versa) without an explicit conversion is rejected (D-0018/D-0020).
        let m = msgs(
            "enum Color:\n    RED\n    GREEN\n\n\
            def main():\n    Color c = 1\n",
        );
        assert!(m.contains("int") && m.contains("Color"), "got:\n{m}");
    }

    #[test]
    fn enum_methods_type_check_at_definition() {
        // Methods on an enum (D-0046) are now checked at definition, with `self`
        // typed as the enum (I#154 lifts the earlier deferral for generic-enum
        // methods like `Optional.unwrap`).
        ok("enum Color:\n    RED\n    GREEN\n\n\
            \x20   def tag(self) -> int64:\n        return 0\n");
    }

    #[test]
    fn generic_struct_and_enum_type_check() {
        // A generic struct and a generic enum with methods type-check at
        // definition; type parameters are opaque (unconstrained, Fork B/M1), and
        // instantiations/`None` coerce permissively (I#154 slice 1).
        ok(r#"
struct Box[T]:
    T value
    def get(self) -> T:
        return self.value

enum Maybe[T]:
    Just(T value)
    Nothing

    def ok(self) -> bool:
        match self:
            case Maybe.Just(v):
                return True
            case Maybe.Nothing:
                return False

def main():
    Maybe[int64] a = Maybe.Just(5)
    Maybe[str] b = Maybe.Nothing
    Box[int64] c = Box(7)
    print(a.ok())
    print(c.get())
"#);
    }

    #[test]
    fn prelude_option_is_usable() {
        // Optional[T] ships in the prelude (I#154 slice 3): construct, match, and
        // call its methods without declaring it.
        ok(r#"
def main():
    Optional[int64] a = Optional.Some(5)
    Optional[str] b = None
    print(a.unwrap())
    print(a.is_some())
    print(b.unwrap_or("x"))
"#);
    }

    #[test]
    fn optional_is_renderable_in_fstring() {
        // #498: an `Optional[T]` field renders (payload when present, `None`
        // when absent) rather than being rejected as a non-renderable type.
        ok(r#"
def main():
    Optional[str] o = Optional.Some("v")
    Optional[int64] n = None
    print(f"{o}")
    print(f"n={n}")
"#);
    }

    #[test]
    fn dict_get_renders_in_fstring() {
        // #498: `dict.get(key) -> Optional[V]` used directly in an f-string field
        // type-checks (the field is renderable; the Optional payload is known).
        ok(r#"
def main():
    dict[str, str] d = {"k": "v"}
    print(f"{d.get("k")}")
    print(f"{d["k"]}")
"#);
    }

    #[test]
    fn user_struct_renders_in_str_and_fstring() {
        // #274 (D-0124): the general Display protocol lets a user struct render
        // via str()/f-string (the lowerer supplies the default field-wise form
        // when no `__str__`/`__repr__`/`@derive(Repr)` is present). Before #274
        // sema rejected str(struct) / f"{struct}" outright (ZAP4024).
        ok(r#"
struct Point:
    int64 x
    int64 y
def main():
    auto p = Point(x=1, y=2)
    print(p)
    print(str(p))
    print(f"pt={p}")
"#);
    }

    #[test]
    fn tier1_enum_renders_in_str_and_fstring() {
        // #274 (D-0124): a tier-1 enum renders its qualified variant name through
        // the protocol, so str()/f-string/print accept it.
        ok(r#"
enum Color:
    RED
    GREEN
def main():
    print(Color.RED)
    print(str(Color.GREEN))
    print(f"c={Color.RED}")
"#);
    }

    #[test]
    fn tier3_enum_renders_in_str_and_fstring() {
        // #706 (D-0124 extended to D-0046): a tier-3 data-carrying enum renders
        // its structural form through the protocol, so str()/f-string/print
        // accept it (the lowerer synthesizes the tag-dispatch renderer).
        ok(r#"
enum Shape:
    Circle(f64 r)
    Unit
def main():
    print(Shape.Circle(r=2.0))
    print(str(Shape.Unit))
    print(f"s={Shape.Circle(r=1.0)}")
"#);
    }

    #[test]
    fn tuple_renders_in_str_and_fstring() {
        // #274 (D-0124): a tuple renders the Python (v0, v1) form; str()/f-string
        // accept it, including a tuple of renderable user values.
        ok(r#"
struct Point:
    int64 x
    int64 y
def main():
    auto t = (1, "a")
    print(t)
    print(str(t))
    print(f"{t}")
    print((Point(x=1, y=2), 3))
"#);
    }

    #[test]
    fn interface_value_renders_in_str_and_fstring() {
        // #707 (D-0124): an interface value is `Ty::Named(iface)`, so it already
        // type-checks for print/str/f-string here; the lowerer renders it by
        // dynamic dispatch to the concrete type's renderer (an `IfaceRender`),
        // rather than the pre-#707 ZAP6001 deferral.
        ok(r#"
interface Shape:
    def area(self) -> f64
struct Circle:
    f64 r
    def area(self) -> f64:
        return self.r
def main():
    Shape s = Circle(r=2.0)
    print(s)
    print(str(s))
    print(f"[{s}]")
"#);
    }

    #[test]
    fn containers_render_in_str_and_fstring() {
        // #274 (D-0124): native list/dict/set rendering — str() and f-strings of a
        // bare container now type-check (the lowerer emits the Python form).
        ok(r#"
def main():
    list[int64] xs = [1, 2]
    print(str(xs))
    dict[str, int64] d = {"a": 1}
    print(f"{d}")
    set[int64] s = {1, 2}
    print(str(s))
    list[str] ss = ["a"]
    print(ss)
"#);
    }

    #[test]
    fn inline_dict_get_is_a_method_receiver() {
        // The `Optional[V]` from `d.get(k)` is a method receiver in its own
        // right: an inline `d.get(k).unwrap_or(...)` / `.is_none()` type-checks
        // and dispatches without first binding to an annotated `Optional[V]`
        // local, and without the program ever spelling `Optional` (#497).
        ok(r#"
def main():
    dict[str, str] d = {"k": "v"}
    print(d.get("k").unwrap_or("x"))
    print(str(d.get("absent").is_none()))
"#);
    }

    #[test]
    fn dict_get_two_arg_default_is_the_value_type() {
        // #570: `d.get(key, default)` returns a plain `V` (not `Optional[V]`):
        // an `int64` default makes the result an `int64` usable directly in
        // arithmetic, and a `str` default a `str` passed where a `str` is
        // wanted. The default unifies with the value type under the ordinary
        // rules, so a widening `int64` default for an `int64` dict is fine.
        ok(r#"
def main():
    dict[str, int64] d = {"a": 1}
    int64 n = d.get("b", 0)
    print(n + 1)
    dict[str, str] s = {"k": "v"}
    str hit = s.get("k", "fallback")
    print(hit)
"#);
    }

    #[test]
    fn dict_get_two_arg_default_type_mismatch_is_rejected() {
        // #570: the default is unified against the value type `V` the same way an
        // assignment is — a `str` default for a `dict[str, int64]` is rejected
        // (ZAP4024, built-in argument type mismatch), just as `int64 x = "s"`
        // would be.
        assert!(errors(
            "def main():\n    dict[str, int64] d = {\"a\": 1}\n    print(d.get(\"b\", \"wrong\"))\n"
        )
        .iter()
        .any(|e| e.code == ErrorCode::ZAP4024));
    }

    #[test]
    fn dict_get_one_arg_still_returns_optional() {
        // #570 must not regress the one-arg form: `d.get(key)` still yields an
        // `Optional[V]`, so binding it to an annotated `Optional[int64]` and
        // calling `unwrap_or` type-checks, while binding it to a bare `int64`
        // (the two-arg result type) is a mismatch.
        ok(r#"
def main():
    dict[str, int64] d = {"a": 1}
    Optional[int64] o = d.get("a")
    print(o.unwrap_or(-1))
"#);
        assert!(!errors(
            "def main():\n    dict[str, int64] d = {\"a\": 1}\n    int64 x = d.get(\"a\")\n"
        )
        .is_empty());
    }

    #[test]
    fn dict_get_wrong_arity_is_rejected() {
        // #570: zero args, or three or more, is a `wrong number of arguments`
        // error (ZAP4025) — only one or two are accepted.
        for src in [
            "def main():\n    dict[str, int64] d = {\"a\": 1}\n    print(d.get())\n",
            "def main():\n    dict[str, int64] d = {\"a\": 1}\n    print(d.get(\"a\", 0, 9))\n",
        ] {
            assert!(
                errors(src).iter().any(|e| e.code == ErrorCode::ZAP4025),
                "{src}"
            );
        }
    }

    // --- interfaces (ticket #84, D-0016) ---

    const SHAPE_IFACE: &str =
        "interface Shape:\n    def area(self) -> f64\n    def name(self) -> str\n\n\
        struct Circle:\n    f64 r\n\
        \x20   def area(self) -> f64:\n        return self.r\n\
        \x20   def name(self) -> str:\n        return \"circle\"\n\n";

    #[test]
    fn struct_conforms_to_interface() {
        // A struct whose method set matches the interface is assignable to the
        // interface type (structural conformance, D-0016), and an interface
        // method dispatches with its declared return type.
        ok(&format!(
            "{SHAPE_IFACE}def main():\n    Shape s = Circle(r=2.0)\n    f64 a = s.area()\n    print(a)\n    print(s.name())\n"
        ));
    }

    #[test]
    fn struct_missing_method_is_rejected() {
        // A struct missing an interface method cannot be used where the interface
        // is expected — the typo-class bug D-0016 catches at compile time.
        let m = msgs(
            "interface Shape:\n    def area(self) -> f64\n    def name(self) -> str\n\n\
            struct Square:\n    f64 side\n\
            \x20   def area(self) -> f64:\n        return self.side\n\n\
            def main():\n    Shape s = Square(side=1.0)\n",
        );
        assert!(m.contains("Square") && m.contains("Shape"), "got:\n{m}");
    }

    #[test]
    fn interface_method_typo_is_rejected() {
        // Calling a method the interface does not declare is a compile error.
        let m = msgs(&format!(
            "{SHAPE_IFACE}def main():\n    Shape s = Circle(r=2.0)\n    print(s.aera())\n"
        ));
        assert!(m.contains("no method 'aera'"), "got:\n{m}");
    }

    #[test]
    fn struct_passes_as_interface_argument() {
        // A conforming struct is accepted where an interface-typed parameter is
        // expected (a conversion site, D-0016).
        ok(&format!(
            "{SHAPE_IFACE}def describe(Shape s) -> f64:\n    return s.area()\n\n\
            def main():\n    print(describe(Circle(r=1.0)))\n"
        ));
    }

    #[test]
    fn value_struct_conforms_to_interface() {
        // A `value struct` whose method set matches conforms to the interface
        // (I#75 Part 1): the conversion boxes it to the heap in lowering, so it
        // is accepted here exactly like a reference struct.
        ok("interface Shape:\n    def area(self) -> f64\n\n\
            value struct Vec2:\n    f64 x\n    f64 y\n\
            \x20   def area(self) -> f64:\n        return self.x\n\n\
            def main():\n    Shape s = Vec2(x=1.0, y=2.0)\n    print(s.area())\n");
    }

    // --- interface bounds on type parameters (I#164, D-0017) ---

    const DISPLAY_IFACE: &str = "interface Display:\n    def show(self) -> str\n\n\
        struct Greeter:\n    str msg\n\
        \x20   def show(self) -> str:\n        return self.msg\n\n";

    #[test]
    fn bound_method_call_on_type_parameter_resolves() {
        // Inside a `[T: Display]` body, a method call on a value of T resolves
        // through the bound interface and is typed by the method's return type
        // (check-at-definition, Fork M1). Static dispatch is applied after
        // monomorphization (see the ir/mono tests), so no interface value is
        // built here.
        ok(&format!(
            "{DISPLAY_IFACE}def announce[T: Display](T x) -> str:\n    return x.show()\n\n\
             def main():\n    print(announce(Greeter(msg=\"hi\")))\n"
        ));
    }

    #[test]
    fn calling_a_non_bound_method_is_rejected_at_definition() {
        // The bound limits what the body may call on T: a method the bound does
        // not declare is a compile error at the definition, even before any
        // instantiation (the Rust rule, not C++ instantiation-time errors).
        let m = msgs(&format!(
            "{DISPLAY_IFACE}def announce[T: Display](T x) -> str:\n    return x.nope()\n\n\
             def main():\n    print(1)\n"
        ));
        assert!(
            m.contains("type parameter 'T'")
                && m.contains("bound 'Display'")
                && m.contains("no method 'nope'"),
            "got:\n{m}"
        );
    }

    #[test]
    fn unbounded_type_parameter_stays_opaque() {
        // Without a bound, T is opaque: a method call on it type-checks loosely
        // (no bound to resolve against) and is not rejected here. This keeps the
        // unconstrained generic behavior (slices 1-4) unchanged.
        ok("def identity[T](T x) -> T:\n    return x\n\ndef main():\n    print(identity(1))\n");
    }

    // --- interface refinement `interface X: Y` (D-0139/#806) ---
    //
    // The self-referential capability dunders (`__eq__(self, other)` etc.) are
    // compiler-provided (the built-in `Equatable`/`Hashable`/`Comparable`), since
    // Zap is type-first and an interface method cannot leave `other` untyped. The
    // *general* refinement mechanism is exercised with ordinary self-only-method
    // user interfaces (`A`/`B`/`C`), and the capability interfaces are exercised
    // through their built-in definitions + `@derive`.

    #[test]
    fn interface_refines_single_supertype_parses() {
        // `interface C: A` parses and type-checks; a struct providing both A's and
        // C's methods conforms to the refined interface at a coercion site.
        ok("interface A:\n    def a(self) -> int64\n\n\
            interface C: A\n    def c(self) -> int64\n\n\
            struct S:\n    int64 v\n\
            \x20   def a(self) -> int64:\n        return 1\n\
            \x20   def c(self) -> int64:\n        return 3\n\n\
            def main():\n    C x = S(v=0)\n    print(x.a() + x.c())\n");
    }

    #[test]
    fn interface_refines_multiple_supertypes() {
        // An interface may refine more than one supertype at once; a type must
        // satisfy every listed supertype (and the interface's own methods).
        ok("interface A:\n    def a(self) -> int64\n\n\
            interface B:\n    def b(self) -> int64\n\n\
            interface C: A, B\n    def c(self) -> int64\n\n\
            struct S:\n    int64 v\n\
            \x20   def a(self) -> int64:\n        return 1\n\
            \x20   def b(self) -> int64:\n        return 2\n\
            \x20   def c(self) -> int64:\n        return 3\n\n\
            def main():\n    C x = S(v=0)\n    print(x.a() + x.b() + x.c())\n");
    }

    #[test]
    fn interface_refinement_is_transitive() {
        // A, B: A, C: B — conforming to C requires A's, B's, and C's methods. A
        // type with the full transitive set conforms.
        ok("interface A:\n    def a(self) -> int64\n\n\
            interface B: A\n    def b(self) -> int64\n\n\
            interface C: B\n    def c(self) -> int64\n\n\
            struct S:\n    int64 v\n\
            \x20   def a(self) -> int64:\n        return 1\n\
            \x20   def b(self) -> int64:\n        return 2\n\
            \x20   def c(self) -> int64:\n        return 3\n\n\
            def main():\n    C x = S(v=0)\n    print(x.a())\n");
    }

    #[test]
    fn missing_direct_supertype_method_names_the_supertype() {
        // Conforming to C (C: A) but missing A's method fails, and the diagnostic
        // names A as the supertype that introduced the requirement.
        let m = msgs(
            "interface A:\n    def a(self) -> int64\n\n\
             interface C: A\n    def c(self) -> int64\n\n\
             struct S:\n    int64 v\n\
             \x20   def c(self) -> int64:\n        return 3\n\n\
             def main():\n    C x = S(v=0)\n",
        );
        assert!(
            m.contains("missing method 'a'") && m.contains("supertype 'A'"),
            "got:\n{m}"
        );
    }

    #[test]
    fn transitive_missing_grandparent_method_names_it() {
        // Conforming to C (C: B, B: A) but missing A's (grandparent) method fails,
        // naming A — the transitive requirement is enforced and attributed.
        let m = msgs(
            "interface A:\n    def a(self) -> int64\n\n\
             interface B: A\n    def b(self) -> int64\n\n\
             interface C: B\n    def c(self) -> int64\n\n\
             struct S:\n    int64 v\n\
             \x20   def b(self) -> int64:\n        return 2\n\
             \x20   def c(self) -> int64:\n        return 3\n\n\
             def main():\n    C x = S(v=0)\n",
        );
        assert!(
            m.contains("missing method 'a'") && m.contains("supertype 'A'"),
            "got:\n{m}"
        );
    }

    #[test]
    fn bound_supertype_method_callable_on_tyvar() {
        // Inside a `[T: C]` body (C: A) both `c` (the bound's own method) and `a`
        // (inherited from the refined supertype A) are callable on a value of T —
        // refinement propagates the supertype's methods onto the bound.
        ok("interface A:\n    def a(self) -> int64\n\n\
            interface C: A\n    def c(self) -> int64\n\n\
            def use[T: C](T x) -> int64:\n    return x.a() + x.c()\n\n\
            def main():\n    print(1)\n");
    }

    #[test]
    fn method_outside_bound_and_its_supertypes_is_rejected() {
        // A method neither the bound nor any refined supertype declares is still
        // rejected at definition (refinement widens the callable set to the
        // transitive closure, no further).
        let m = msgs(
            "interface A:\n    def a(self) -> int64\n\n\
             interface C: A\n    def c(self) -> int64\n\n\
             def bad[T: C](T x) -> int64:\n    return x.nope()\n\n\
             def main():\n    print(1)\n",
        );
        assert!(
            m.contains("bound 'C'") && m.contains("no method 'nope'"),
            "got:\n{m}"
        );
    }

    #[test]
    fn builtin_hashable_refines_equatable_on_bound() {
        // `Equatable`/`Hashable`/`Comparable` are compiler-known even when the
        // module never declares them: a `[K: Hashable]` bound resolves, and the
        // refined `__eq__` (from Equatable) is callable alongside `__hash__`.
        ok("def key_of[K: Hashable](K k) -> int64:\n\
            \x20   if k.__eq__(k):\n        return k.__hash__()\n    return 0\n\n\
            def main():\n    print(1)\n");
    }

    #[test]
    fn builtin_comparable_refines_equatable_on_bound() {
        // `Comparable` likewise refines `Equatable`: `__cmp__` and `__eq__` both
        // resolve on a `[T: Comparable]` bound.
        ok("def order[T: Comparable](T a, T b) -> int64:\n\
            \x20   if a.__eq__(b):\n        return 0\n    return a.__cmp__(b)\n\n\
            def main():\n    print(1)\n");
    }

    #[test]
    fn derived_type_satisfies_refined_builtin_bound() {
        // A struct whose dunders come from `@derive(Eq, Hash)` (D-0047) satisfies
        // the refined built-in `Hashable` contract: `@derive(Eq)` provides the
        // supertype's `__eq__`, `@derive(Hash)` the `__hash__`.
        ok("@derive(Eq, Hash)\nstruct Key:\n    int64 x\n\n\
            def main():\n    Hashable h = Key(x=1)\n    print(h.__hash__())\n");
    }

    #[test]
    fn derived_eq_only_fails_the_refined_builtin_bound() {
        // `@derive(Eq)` alone provides `__eq__` (Equatable) but NOT `__hash__`:
        // conformance to the refined `Hashable` is rejected, naming the missing
        // own-method — the derive path is checked like a hand-written one.
        let m = msgs(
            "@derive(Eq)\nstruct Key:\n    int64 x\n\n\
             def main():\n    Hashable h = Key(x=1)\n",
        );
        assert!(
            m.contains("missing method '__hash__'") && m.contains("Hashable"),
            "got:\n{m}"
        );
    }

    #[test]
    fn struct_with_hash_but_not_eq_fails_hashable_naming_equatable() {
        // The keystone acceptance case: a struct providing Hashable's own method
        // (`__hash__`) but NOT its refined supertype Equatable (`__eq__`) is
        // rejected, and the message names the missing supertype (D-0139/#806).
        let m = msgs(
            "struct Half:\n    int64 x\n\
             \x20   def __hash__(self) -> int64:\n        return self.x\n\n\
             def main():\n    Hashable h = Half(x=1)\n",
        );
        assert!(
            m.contains("missing method '__eq__'")
                && m.contains("supertype 'Equatable'")
                && m.contains("Hashable"),
            "got:\n{m}"
        );
    }

    // ---- capability bounds on user generic containers (D-0139/I#811) ------
    //
    // A user generic container declares a capability bound exactly as a built-in
    // one does. Inside a method body, the bound's methods (and those of its
    // refined supertypes) resolve on a value of the enclosing type parameter —
    // the user-container counterpart of the bounded-free-function body check.

    #[test]
    fn enclosing_comparable_bound_method_resolves_in_body() {
        // A method of `struct Sorter[K: Comparable]` may call `__cmp__` (and, by
        // refinement, `__eq__`) on a parameter typed `K`.
        ok("value struct Sorter[K: Comparable]:\n    K pivot\n\
            \x20   def before(self, K a, K b) -> int64:\n        return a.__cmp__(b)\n\
            \x20   def same(self, K a, K b) -> bool:\n        return a.__eq__(b)\n\n\
            def main():\n    print(1)\n");
    }

    #[test]
    fn enclosing_hashable_bound_method_resolves_in_body() {
        // A method of `struct HMap[K: Hashable, V]` may call `__hash__` (and, by
        // refinement, `__eq__`) on a value of `K`.
        ok("value struct HMap[K: Hashable, V]:\n    K k\n    V v\n\
            \x20   def slot(self, K key) -> int64:\n        return key.__hash__()\n\n\
            def main():\n    print(1)\n");
    }

    #[test]
    fn enclosing_bound_does_not_leak_a_method_outside_the_capability() {
        // The bound limits the callable set at definition: a method neither the
        // capability nor its supertypes declare is rejected, naming the bound.
        let m = msgs(
            "value struct Sorter[K: Comparable]:\n    K pivot\n\
             \x20   def bad(self, K a) -> int64:\n        return a.nope()\n\n\
             def main():\n    print(1)\n",
        );
        assert!(
            m.contains("type parameter 'K'")
                && m.contains("bound 'Comparable'")
                && m.contains("no method 'nope'"),
            "got:\n{m}"
        );
    }

    #[test]
    fn enclosing_unbounded_type_parameter_stays_opaque() {
        // Without a bound the enclosing type parameter is opaque in method bodies,
        // exactly as for an unbounded free-function parameter (no regression).
        ok("value struct Box[T]:\n    T item\n\
            \x20   def get(self, T x) -> T:\n        return x\n\n\
            def main():\n    print(1)\n");
    }

    #[test]
    fn method_own_generic_shadows_enclosing_bound() {
        // A method's own type parameter of the same name as an enclosing one is a
        // fresh, unbounded variable that shadows the enclosing one: the body
        // becomes opaque over it. A call the enclosing `Comparable` bound would
        // reject (`nope`) is therefore loosely accepted here — proof the inherited
        // bound was dropped rather than leaking onto the shadowing parameter.
        ok("value struct Sorter[K: Comparable]:\n    K pivot\n\
            \x20   def opaque[K](self, K a) -> int64:\n        a.nope()\n        return 0\n\n\
            def main():\n    print(1)\n");
    }

    #[test]
    fn refining_a_non_interface_is_rejected() {
        // A supertype must name an interface; refining a struct is ZAP4081 and
        // names the offending type.
        let m = msgs(
            "struct NotIface:\n    int64 x\n\n\
             interface Bad: NotIface\n    def m(self) -> int64\n\n\
             def main():\n    print(1)\n",
        );
        assert!(m.contains("is not an interface"), "got:\n{m}");
    }

    #[test]
    fn refining_an_unknown_type_is_rejected() {
        // An unknown supertype name is likewise not an interface (ZAP4081).
        let m = msgs(
            "interface Bad: DoesNotExist\n    def m(self) -> int64\n\n\
             def main():\n    print(1)\n",
        );
        assert!(
            m.contains("DoesNotExist") && m.contains("not an interface"),
            "got:\n{m}"
        );
    }

    #[test]
    fn cyclic_refinement_is_rejected() {
        // Mutually-refining interfaces (A: B, B: A) form a cycle — each is
        // reported (ZAP4082).
        let m = msgs(
            "interface A: B\n    def a(self) -> int64\n\n\
             interface B: A\n    def b(self) -> int64\n\n\
             def main():\n    print(1)\n",
        );
        assert!(m.contains("refines itself"), "got:\n{m}");
    }

    #[test]
    fn self_refinement_is_rejected() {
        // An interface that lists itself as a supertype is a (degenerate) cycle.
        let m = msgs(
            "interface A: A\n    def a(self) -> int64\n\n\
             def main():\n    print(1)\n",
        );
        assert!(m.contains("refines itself"), "got:\n{m}");
    }

    #[test]
    fn duplicate_supertype_is_rejected() {
        // Listing the same supertype twice is ZAP4083.
        let m = msgs(
            "interface A:\n    def a(self) -> int64\n\n\
             interface B: A, A\n    def b(self) -> int64\n\n\
             def main():\n    print(1)\n",
        );
        assert!(
            m.contains("more than once") && m.contains("'A'"),
            "got:\n{m}"
        );
    }

    #[test]
    fn plain_interface_without_refinement_is_unchanged() {
        // A non-refining interface behaves exactly as before: conformance checks
        // only its own methods, no supertype machinery engaged.
        ok("interface Shape:\n    def area(self) -> f64\n\n\
            struct Circle:\n    f64 r\n\
            \x20   def area(self) -> f64:\n        return self.r\n\n\
            def main():\n    Shape s = Circle(r=1.0)\n    print(s.area())\n");
    }

    // --- tuples (ticket #54, D-0049) ---

    #[test]
    fn tuple_literal_elements_are_typed() {
        // A tuple literal carries its element types positionally: `t[1]` is a
        // `str`, so assigning it into a `str` is fine but into an `int64` is not.
        ok("def main():\n    auto t = (1, \"two\", 3.0)\n    str s = t[1]\n    print(s)\n");
        let e = errors(
            "def main():\n    auto t = (1, \"two\", 3.0)\n    int64 n = t[1]\n    print(n)\n",
        );
        assert_eq!(e.len(), 1, "got {e:?}");
    }

    #[test]
    fn tuple_const_index_access_types() {
        // Constant indices yield each position's static type.
        ok("def main():\n    auto t = (10, 20)\n    int64 a = t[0]\n    int64 b = t[1]\n    print(a + b)\n");
    }

    #[test]
    fn tuple_nonconstant_index_is_error() {
        // A runtime index is rejected: a tuple is heterogeneous, so the element
        // type must be known statically (the key difference from `list[i]`).
        let e = errors(
            "def main():\n    auto t = (1, 2)\n    auto i = 0\n    auto x = t[i]\n    print(x)\n",
        );
        assert_eq!(e.len(), 1, "got {e:?}");
        assert!(e[0].message.contains("compile-time-constant"), "got {e:?}");
    }

    #[test]
    fn tuple_index_out_of_range_is_error() {
        let e = errors("def main():\n    auto t = (1, 2)\n    auto x = t[5]\n    print(x)\n");
        assert_eq!(e.len(), 1, "got {e:?}");
        assert!(e[0].message.contains("out of range"), "got {e:?}");
    }

    #[test]
    fn tuple_destructuring_binds_element_types() {
        // Positional destructuring binds each name to its element's type.
        ok("def main():\n    auto (a, b, c) = (1, \"two\", 3.0)\n    int64 x = a\n    str y = b\n    float64 z = c\n    print(x)\n    print(y)\n    print(z)\n");
    }

    #[test]
    fn tuple_destructuring_arity_mismatch_is_error() {
        let e = errors("def main():\n    auto (a, b) = (1, 2, 3)\n    print(a)\n");
        assert!(
            e.iter().any(|x| x.message.contains("element")),
            "expected an arity error, got {e:?}"
        );
    }

    #[test]
    fn tuple_rest_captures_remaining_as_tuple() {
        // `*rest` binds the remaining elements as a sub-tuple, so `rest[k]` is
        // typed by position.
        ok("def main():\n    auto (first, *rest) = (10, 20, 30, 40)\n    int64 a = first\n    int64 b = rest[0]\n    int64 c = rest[2]\n    print(a + b + c)\n");
    }

    #[test]
    fn multiple_return_is_tuple_typed() {
        // A function returning `(int64, int64)` accepts a bare comma-list return
        // and the caller may destructure or bind-and-index it.
        ok(include_str!("../../../tests/golden/tuple_multiret.zap"));
    }

    #[test]
    fn multiple_return_arity_mismatch_is_error() {
        // Returning a single value where a 2-tuple is declared is rejected.
        let e = errors(
            "def f() -> (int64, int64):\n    return 1\n\ndef main():\n    auto t = f()\n    print(t[0])\n",
        );
        assert_eq!(e.len(), 1, "got {e:?}");
        assert!(e[0].message.contains("return type mismatch"), "got {e:?}");
    }

    #[test]
    fn swap_reassignment_type_checks() {
        ok(include_str!("../../../tests/golden/tuple_swap.zap"));
    }

    // --- iteration protocol (D-0048) ---

    /// A comma-ok user iterable: `__iter__` yields an iterator whose `__next__`
    /// returns `(T, bool)`.
    const RANGE_SRC: &str = "\
struct Range:
    int64 lo
    int64 hi
    def __iter__(self) -> RangeIter:
        return RangeIter(cur=self.lo, hi=self.hi)

struct RangeIter:
    int64 cur
    int64 hi
    def __iter__(self) -> RangeIter:
        return self
    def __next__(self) -> (int64, bool):
        if self.cur >= self.hi:
            return (0, False)
        auto v = self.cur
        self.cur = self.cur + 1
        return (v, True)
";

    #[test]
    fn user_iterable_commaok_type_checks() {
        ok(&format!(
            "{RANGE_SRC}\ndef main():\n    auto total = 0\n    for x in Range(lo=0, hi=5):\n        total = total + x\n    print(total)\n"
        ));
    }

    #[test]
    fn user_iterable_loop_var_is_typed_from_next() {
        // The loop variable has the element type `__next__` yields (int64 here),
        // so an int-only use type-checks and a wrong-type use would not.
        ok(&format!(
            "{RANGE_SRC}\ndef main():\n    for x in Range(lo=0, hi=5):\n        int64 y = x + 1\n        print(y)\n"
        ));
    }

    #[test]
    fn stopiteration_iterable_type_checks() {
        // The Python form: `__next__ -> T` raising `StopIteration` at exhaustion.
        ok("\
struct Count:
    int64 n
    def __iter__(self) -> CountIter:
        return CountIter(cur=0, n=self.n)

struct CountIter:
    int64 cur
    int64 n
    def __iter__(self) -> CountIter:
        return self
    def __next__(self) -> int64:
        if self.cur >= self.n:
            raise StopIteration
        auto v = self.cur
        self.cur = self.cur + 1
        return v

def main():
    auto total = 0
    for x in Count(n=4):
        total = total + x
    print(total)
");
    }

    #[test]
    fn non_iterable_in_for_is_an_error() {
        let e = errors(
            "struct Point:\n    int64 x\ndef main():\n    for p in Point(x=1):\n        print(1)\n",
        );
        assert_eq!(e.len(), 1, "expected one not-iterable error, got {e:?}");
        assert!(e[0].message.contains("not iterable"), "got {e:?}");
    }

    #[test]
    fn comprehension_over_user_iterable_type_checks() {
        ok(&format!(
            "{RANGE_SRC}\ndef main():\n    auto squares = [x * x for x in Range(lo=1, hi=5) if x != 3]\n    print(len(squares))\n"
        ));
    }

    #[test]
    fn comprehension_over_non_iterable_is_an_error() {
        let e = errors("def main():\n    auto xs = [y for y in 7]\n    print(len(xs))\n");
        assert!(
            e.iter().any(|e| e.message.contains("not iterable")),
            "expected a not-iterable error, got {e:?}"
        );
    }

    #[test]
    fn membership_fallback_without_contains_type_checks() {
        // A struct with no `__contains__` but which is iterable supports `in`
        // (membership by iteration, D-0048).
        ok(&format!(
            "{RANGE_SRC}\ndef main():\n    auto r = Range(lo=0, hi=5)\n    if 3 in r:\n        print(1)\n"
        ));
    }

    // --- value structs (#65, D-0051) ---

    #[test]
    fn value_struct_type_checks_like_a_struct() {
        // A `value struct` type-checks exactly like a reference struct: methods,
        // construction, and field access all work (the kind only changes value
        // semantics, D-0051).
        ok("value struct Vec2:\n    f64 x\n    f64 y\n\
            \x20   def len2(self) -> f64:\n        return self.x * self.x + self.y * self.y\n\
            def main():\n    auto v = Vec2(x=3.0, y=4.0)\n    auto w = v\n    print(w.len2())\n");
    }

    #[test]
    fn value_struct_operators_and_derive_work() {
        // Operator overloading (D-0047) and `@derive` work identically on a value
        // struct: `==` resolves to the derived `__eq__`, and the value struct can
        // be used as a dict key (Eq + Hash).
        ok("@derive(Eq, Hash)\nvalue struct Key:\n    int64 a\n    int64 b\n\
            def main():\n    auto k = Key(a=1, b=2)\n    auto d = {k: 10}\n    if k == k:\n        print(1)\n");
    }

    #[test]
    fn value_struct_auto_derives_key_without_derive() {
        // D-0139/#808: a `value struct` of all-hashable fields is usable as a dict
        // key and set member with NO `@derive` (auto-Equatable + auto-Hashable),
        // and does not trip the Eq-without-Hash warning (it has neither a
        // hand-written `__eq__` nor `__hash__`). Sema models struct fields loosely,
        // so the real auto-derivation is in lowering (`crates/ir`); here we pin
        // that sema accepts the key/member use with no error or warning.
        let src = "value struct P:\n    int64 x\n    str name\n\n\
             def main():\n    dict[P, int64] d = {}\n    d[P(x=1, name=\"a\")] = 10\n    \
             set[P] s = set[P]()\n    s.add(P(x=2, name=\"b\"))\n    print(len(d) + len(s))\n";
        let errs = errors(src);
        let warns = warnings_str(src);
        assert!(
            errs.is_empty(),
            "no-derive value struct key should type-check: {errs:?}"
        );
        assert!(
            !warns.iter().any(|w| w.message.contains("__hash__")),
            "a value struct with neither hand __eq__ nor __hash__ must not warn: {warns:?}"
        );
    }

    #[test]
    fn value_struct_auto_equality_without_derive() {
        // D-0139/#808: `==`/`!=` resolve on a no-`@derive` value struct via the
        // auto-synthesized `__eq__` (auto-Equatable). A nested value-struct field
        // composes. A reference struct still needs an explicit `@derive(Eq)`.
        ok("value struct Inner:\n    int64 a\n\n\
            value struct P:\n    Inner i\n    int64 b\n\n\
            def main():\n    if P(i=Inner(a=1), b=2) == P(i=Inner(a=1), b=2):\n        print(1)\n");
        // A `value struct` with a non-hashable field does NOT auto-derive, so `==`
        // on it is still rejected (names the operator interface).
        let m = msgs(
            "value struct Bag:\n    list[int64] xs\n\n\
             def main():\n    if Bag(xs=[1]) == Bag(xs=[2]):\n        print(1)\n",
        );
        assert!(
            m.contains("__eq__") && m.contains("=="),
            "a list-field value struct must not auto-derive ==, got:\n{m}"
        );
        // A reference struct never auto-derives equality.
        let r = msgs(
            "struct R:\n    int64 x\n\n\
             def main():\n    if R(x=1) == R(x=1):\n        print(1)\n",
        );
        assert!(
            r.contains("__eq__"),
            "reference struct must not auto-derive ==, got:\n{r}"
        );
    }

    #[test]
    fn tier1_enum_intrinsic_eq_hash_cannot_be_overridden() {
        // D-0139/#809: a tier-1 enum's equality/hashing are intrinsic (by backing
        // value, D-0018), so a hand-written `__eq__`/`__hash__` is rejected
        // (ZAP5126) — you cannot merge two distinct members via custom equality.
        let eq = msgs(
            "enum Color:\n    Red\n    Green\n\
             \x20   def __eq__(self, Color other) -> bool:\n        return True\n\n\
             def main():\n    print(1)\n",
        );
        assert!(
            eq.contains("intrinsic") && eq.contains("Color"),
            "tier-1 __eq__ override must be rejected, got:\n{eq}"
        );
        let hash = msgs(
            "enum Color:\n    Red\n    Green\n\
             \x20   def __hash__(self) -> int64:\n        return 0\n\n\
             def main():\n    print(1)\n",
        );
        assert!(
            hash.contains("intrinsic"),
            "tier-1 __hash__ override must be rejected, got:\n{hash}"
        );
        // `__cmp__` (ordering) stays allowed on a tier-1 enum.
        ok("enum Color:\n    Red\n    Green\n\
            \x20   def __cmp__(self, Color other) -> int64:\n        return 0\n\n\
            def main():\n    print(1)\n");
    }

    #[test]
    fn tier3_enum_may_override_eq() {
        // A tier-3 (data-carrying) enum is a value aggregate, not intrinsic-Eq, so
        // a hand-written `__eq__` is allowed (no ZAP5126).
        ok("enum Tag:\n    A(int64 x)\n    B\n\
            \x20   def __eq__(self, Tag other) -> bool:\n        return True\n\n\
            def main():\n    print(1)\n");
    }

    #[test]
    fn value_struct_direct_recursion_errors() {
        // A value struct cannot contain itself by value (infinite inline size).
        let e = errors("value struct Node:\n    int64 v\n    Node next\n");
        assert_eq!(e.len(), 1, "expected one recursion error, got {e:?}");
        assert!(
            e[0].message.contains("cannot contain itself by value"),
            "message was: {}",
            e[0].message
        );
    }

    #[test]
    fn value_struct_indirect_recursion_errors() {
        // An indirect cycle through another value struct is also rejected.
        let e = errors("value struct A:\n    B b\n\nvalue struct B:\n    A a\n");
        assert!(
            !e.is_empty()
                && e.iter()
                    .any(|x| x.message.contains("cannot contain itself by value")),
            "expected an indirect-recursion error, got {e:?}"
        );
    }

    #[test]
    fn reference_struct_recursion_is_allowed() {
        // A plain (reference) struct may be recursive — the field is a pointer
        // (D-0023), so no infinite-size problem.
        ok("struct Node:\n    int64 v\n    Node next\n");
    }

    #[test]
    fn value_struct_holding_a_reference_struct_is_fine() {
        // A value struct may hold a reference-struct field (a pointer); that does
        // not make it infinitely sized.
        ok("struct Ref:\n    int64 n\n\nvalue struct Holder:\n    Ref r\n    int64 k\n");
    }

    #[test]
    fn heap_type_checks() {
        // heap[T] (D-0057): construction, push/pop/peek, len, and iteration all
        // type-check. The min-heap and the max-heap-via-comparator forms are both
        // accepted, and the full golden fixture type-checks.
        ok(include_str!("../../../tests/golden/heap.zap"));
    }

    #[test]
    fn heap_element_type_is_tracked() {
        // Iterating a `heap[int64]` yields `int64` loop elements, and `len(h)` is
        // an integer — the heap carries its element type like list/set.
        ok("def main():\n    \
                auto h = heap[int64]()\n    \
                h.push(3)\n    \
                int64 total = 0\n    \
                for x in h:\n        total = total + x\n    \
                print(total)\n    \
                int64 n = len(h)\n    \
                print(n)\n");
    }

    #[test]
    fn heap_bad_comparator_errors() {
        // `cmp` accepts only the string literals "min" / "max".
        let e = errors("def main():\n    auto h = heap[int64](cmp=\"biggest\")\n");
        assert!(
            e.iter().any(|x| x.message.contains("cmp must be")),
            "expected a heap-cmp error, got {e:?}"
        );
    }

    #[test]
    fn heap_unknown_keyword_errors() {
        // Only the `cmp` keyword argument is accepted.
        let e = errors("def main():\n    auto h = heap[int64](dir=\"max\")\n");
        assert!(
            e.iter().any(|x| x.message.contains("no keyword argument")),
            "expected an unknown-keyword error, got {e:?}"
        );
    }

    #[test]
    fn heap_positional_argument_errors() {
        // A positional argument (no `cmp=`) is rejected.
        let e = errors("def main():\n    auto h = heap[int64](3)\n");
        assert!(
            e.iter()
                .any(|x| x.message.contains("only the keyword argument")),
            "expected a positional-argument error, got {e:?}"
        );
    }

    // --- set constructors (D17, #609) ---

    #[test]
    fn set_empty_construction_type_checks() {
        // `set[T]()` builds an empty `set[T]`, parallel to heap[T]()/channel[T]().
        // The element type is tracked: `.add`, `in`, and `len` all check.
        ok("def main():\n    \
                auto s = set[int64]()\n    \
                s.add(5)\n    \
                print(5 in s)\n    \
                int64 n = len(s)\n    \
                print(n)\n");
    }

    #[test]
    fn set_from_list_type_checks() {
        // `set(iterable)` and `set[T](iterable)` build a set from any iterable.
        ok("def main():\n    \
                auto a = set([1, 1, 2, 3])\n    \
                auto b = set[int64]([4, 4, 5])\n    \
                print(len(a) + len(b))\n");
    }

    #[test]
    fn set_from_string_type_checks() {
        // Iterating a `str` yields chars, so `set(\"aab\")` is a set of codepoints.
        ok("def main():\n    \
                auto s = set(\"aab\")\n    \
                print(len(s))\n");
    }

    #[test]
    fn set_no_argument_errors() {
        // `set()` has no element type to infer; sema points at `set[T]()`.
        let e = errors("def main():\n    auto s = set()\n");
        assert!(
            e.iter().any(|x| x.message.contains("set[T]()")),
            "expected a set() empty-construction error, got {e:?}"
        );
    }

    #[test]
    fn set_from_noniterable_errors() {
        // A non-iterable operand to `set(...)` is a D-0048 error.
        let e = errors("def main():\n    auto s = set(5)\n");
        assert!(
            e.iter().any(|x| x.message.contains("not iterable")),
            "expected a not-iterable error, got {e:?}"
        );
    }

    // --- C-style bitfields on value structs (I#173, D-0068) ---

    #[test]
    fn bitfield_value_struct_ok() {
        // A value struct whose fields all declare widths that fit their types
        // and sum to <= 64 bits type-checks.
        ok("enum Color:\n    RED\n    GREEN\n    BLUE\n    WHITE\n\n\
            value struct Pack:\n    int64 a : 4\n    int64 b : 4\n    \
            Color c : 2\n    bool d : 1\n");
    }

    #[test]
    fn bitfield_on_reference_struct_errors() {
        // Bitfields are a value-struct-only layout feature (D-0068).
        let e = errors("struct Bad:\n    int64 flags : 3\n");
        assert!(
            e.iter()
                .any(|x| x.message.contains("bit widths require a `value struct`")),
            "expected a reference-struct bitfield error, got {e:?}"
        );
    }

    #[test]
    fn bitfield_on_pointer_field_errors() {
        // A reference-typed field (here `str`) can never be a bitfield.
        let e = errors("value struct Bad:\n    str name : 8\n");
        assert!(
            e.iter()
                .any(|x| x.message.contains("must be a fixed-width integer")),
            "expected an ineligible-type error, got {e:?}"
        );
    }

    #[test]
    fn bitfield_width_exceeds_type_errors() {
        // A 40-bit slice does not fit an int32 (32 bits).
        let e = errors("value struct Bad:\n    int32 x : 40\n");
        assert!(
            e.iter()
                .any(|x| x.message.contains("its type holds at most")),
            "expected a width-exceeds-type error, got {e:?}"
        );
    }

    #[test]
    fn bitfield_all_or_nothing_errors() {
        // Mixing a bitfield with a full-width field is rejected in v1.
        let e = errors("value struct Bad:\n    int64 a : 3\n    int64 b\n");
        assert!(
            e.iter()
                .any(|x| x.message.contains("must declare a bit width")),
            "expected an all-or-nothing error, got {e:?}"
        );
    }

    #[test]
    fn bitfield_total_exceeds_word_errors() {
        // The widths of one struct may not exceed a 64-bit machine word.
        let e = errors("value struct Bad:\n    int64 a : 40\n    int64 b : 40\n");
        assert!(
            e.iter()
                .any(|x| x.message.contains("more than one 64-bit machine word")),
            "expected a word-overflow error, got {e:?}"
        );
    }

    #[test]
    fn bitfield_tagged_enum_errors() {
        // A data-carrying (tier-3) enum is a heap value, not a packable scalar.
        let e = errors(
            "enum Shape:\n    Circle(int64 r)\n    Square(int64 s)\n\n\
            value struct Bad:\n    Shape s : 2\n",
        );
        assert!(
            e.iter().any(|x| x.message.contains("tagged enum")),
            "expected a tagged-enum bitfield error, got {e:?}"
        );
    }

    #[test]
    fn bitfield_enum_needs_enough_bits_errors() {
        // A 4-variant enum needs 2 bits; 1 bit cannot hold its top discriminant.
        let e = errors(
            "enum Color:\n    RED\n    GREEN\n    BLUE\n    WHITE\n\n\
            value struct Bad:\n    Color c : 1\n",
        );
        assert!(
            e.iter().any(|x| x.message.contains("needs at least")),
            "expected an enum-too-narrow error, got {e:?}"
        );
    }

    // --- f-strings and conversion builtins (#285, D-0026) ---

    #[test]
    fn fstring_fields_type_check() {
        ok("def main():\n    auto n = 42\n    auto x = 3.14\n    print(f\"{n} {x} {n:02x}\")\n");
        // A field referencing an undefined name is reported.
        let e = errors("def main():\n    print(f\"{missing}\")\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP3001),
            "expected an unknown-name error inside the field, got {e:?}"
        );
    }

    #[test]
    fn fstring_bad_spec_and_conv() {
        // A hex spec on a string value is invalid.
        let e = errors("def main():\n    auto s = \"hi\"\n    print(f\"{s:x}\")\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP4026),
            "expected ZAP4026 for a hex spec on a str, got {e:?}"
        );
        // The repr conversion is not supported in v1.
        let e = errors("def main():\n    auto n = 1\n    print(f\"{n!r}\")\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP4026),
            "expected ZAP4026 for the !r conversion, got {e:?}"
        );
        // A malformed spec grammar.
        let e = errors("def main():\n    auto n = 1\n    print(f\"{n:0q}\")\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP4026),
            "expected ZAP4026 for an unknown format type, got {e:?}"
        );
    }

    #[test]
    fn fstring_unmatched_brace() {
        let e = errors("def main():\n    print(f\"a}b\")\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP2017),
            "expected ZAP2017 for a single '}}', got {e:?}"
        );
    }

    #[test]
    fn conversion_builtins_type_check() {
        ok(
            "def main():\n    auto a = float(\"3.14\")\n    auto b = bool(0)\n    \
            auto c = bool(\"x\")\n    auto d = format(255, \"02x\")\n    print(a)\n    \
            print(d)\n",
        );
        // format's spec must be a string literal in v1.
        let e = errors("def main():\n    auto s = \"02x\"\n    print(format(1, s))\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP4026),
            "expected ZAP4026 for a non-literal format spec, got {e:?}"
        );
    }

    #[test]
    fn bigint_and_complex_numeric_forms_type_check() {
        ok("def main():\n    \
            bigint large = 123456789012345678901234567890\n    \
            bigint rounded = bigint(10.75)\n    \
            bigint total = large + rounded\n    \
            int64 small = int64(rounded)\n    \
            complex z = complex(1.0, 2.0) + 3j\n    \
            complex product = z * 2j\n    \
            bool same = product == product\n    \
            print(str(total))\n    \
            print(str(z))\n");
    }

    #[test]
    fn complex_ordering_is_rejected() {
        let e = errors("def main():\n    bool ordered = 1j < 2j\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP4012),
            "expected ZAP4012 for ordered complex comparison, got {e:?}"
        );
    }

    #[test]
    fn bytes_type_checks() {
        // Construction, byte indexing (-> int64), slicing (-> bytes), len,
        // concatenation, comparison, and the str<->bytes round-trip (I#337).
        ok("def main():\n    \
            auto b = b\"hi\"\n    \
            auto c = bytes(\"hi\")\n    \
            auto d = bytes([104, 105])\n    \
            auto e = bytes()\n    \
            int64 n = b[0]\n    \
            bytes sl = b[0:1]\n    \
            int64 l = len(b)\n    \
            bytes cat = b + c\n    \
            bool eq = b == c\n    \
            str s = str(b)\n    \
            print(s)\n    \
            print(n)\n");
        // bytes(int) is not a valid construction.
        let e = errors("def main():\n    auto b = bytes(5)\n    print(b)\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP4024),
            "expected ZAP4024 for bytes(int), got {e:?}"
        );
    }

    #[test]
    fn base64_hex_modules_type_check() {
        ok("def main():\n    \
            auto e = base64.encode(b\"x\")\n    \
            auto u = base64.urlsafe_encode_nopad(b\"x\")\n    \
            bytes d = base64.decode(\"eA==\")\n    \
            auto h = hex.encode(b\"x\")\n    \
            bytes hd = hex.decode(\"78\")\n    \
            print(e)\n    print(u)\n    print(h)\n");
        // A codec needs a str/bytes argument, not an int.
        let e = errors("def main():\n    print(hex.encode(5))\n");
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP4024),
            "expected ZAP4024 for hex.encode(int), got {e:?}"
        );
    }

    // ---- `is` / `is not` identity operators (#611, D18) -------------------

    #[test]
    fn is_none_on_optional_and_reference_struct_type_checks() {
        // `x is None` / `x is not None` is the common case: valid on the two
        // nullable categories — `Optional[T]` (empty variant) and a plain
        // reference `struct` (null pointer, D-0023) — and yields `bool`, so it
        // reads in a boolean context.
        ok(r#"
def main():
    Optional[int64] a = Optional.Some(5)
    Optional[int64] b = None
    bool r0 = a is None
    bool r1 = a is not None
    bool r2 = b is None
    if b is None:
        print("none")
"#);
        ok(r#"
struct Node:
    int64 v
def main():
    Node n = Node(v=1)
    Node m = None
    bool r0 = n is None
    bool r1 = m is not None
"#);
        // Niche-layout optional (pointer payload) is nullable too.
        ok(r#"
def main():
    Optional[str] s = None
    bool r = s is not None
"#);
        // `None is x` (None on the left) is equally valid.
        ok(r#"
def main():
    Optional[int64] a = None
    bool r = None is a
"#);
    }

    #[test]
    fn is_on_reference_types_type_checks() {
        // `a is b` between two reference values is reference identity and yields
        // `bool`: reference structs, and the GC collections `list`/`dict`/`set`.
        ok(r#"
struct Node:
    int64 v
def main():
    Node a = Node(v=1)
    Node b = a
    bool same = a is b
    bool diff = a is not b
"#);
        ok(r#"
def main():
    list[int64] xs = [1, 2]
    list[int64] ys = xs
    bool r = xs is ys
    dict[str, int64] d = {}
    dict[str, int64] e = d
    bool r2 = d is not e
    set[int64] s = {1}
    set[int64] t = s
    bool r3 = s is t
"#);
    }

    #[test]
    fn is_none_on_non_nullable_value_type_is_rejected() {
        // A value that can never be None (scalars, `str`, `bytes`, `bool`,
        // `char`, a tuple) makes `x is None` a constant — ZAP4074, directing to
        // `==` / a pattern match.
        for src in [
            "def main():\n    int64 x = 1\n    bool r = x is None\n",
            "def main():\n    float64 x = 1.0\n    bool r = x is not None\n",
            "def main():\n    str s = \"x\"\n    bool r = s is None\n",
            "def main():\n    bytes b = b\"x\"\n    bool r = b is None\n",
            "def main():\n    bool t = True\n    bool r = t is None\n",
            "def main():\n    auto p = (1, 2)\n    bool r = p is None\n",
        ] {
            let e = errors(src);
            assert!(
                e.iter().any(|x| x.code == ErrorCode::ZAP4074),
                "expected ZAP4074 (value can never be None) for {src:?}, got {e:?}"
            );
        }
    }

    #[test]
    fn is_on_value_types_is_rejected() {
        // `a is b` on value types / scalars has no distinct identity — ZAP4073,
        // directing to `==`. Covers numbers, `str`, `bytes`, `bool`, `char`, a
        // `value struct`, an `enum`, and a tuple.
        for src in [
            "def main():\n    int64 a = 1\n    int64 b = 2\n    bool r = a is b\n",
            "def main():\n    float64 a = 1.0\n    float64 b = 2.0\n    bool r = a is not b\n",
            "def main():\n    str a = \"x\"\n    str b = \"y\"\n    bool r = a is b\n",
            "def main():\n    bytes a = b\"x\"\n    bytes b = b\"y\"\n    bool r = a is b\n",
            "def main():\n    bool a = True\n    bool b = False\n    bool r = a is b\n",
            "def main():\n    char a = 'x'\n    char b = 'y'\n    bool r = a is b\n",
            "value struct V:\n    int64 x\ndef main():\n    V a = V(x=1)\n    V b = V(x=1)\n    bool r = a is b\n",
            "enum Color:\n    RED\n    BLUE\ndef main():\n    Color a = Color.RED\n    Color b = Color.BLUE\n    bool r = a is b\n",
            "def main():\n    auto a = (1, 2)\n    auto b = (1, 2)\n    bool r = a is b\n",
        ] {
            let e = errors(src);
            assert!(
                e.iter().any(|x| x.code == ErrorCode::ZAP4073),
                "expected ZAP4073 (`is` needs reference operands) for {src:?}, got {e:?}"
            );
        }
    }

    #[test]
    fn is_mixing_a_value_operand_with_a_reference_operand_is_rejected() {
        // Only one operand needs to be a value type for ZAP4073 (one reference,
        // one scalar): identity cannot be tested against a value.
        let e = errors(
            "struct Node:\n    int64 v\ndef main():\n    Node n = Node(v=1)\n    int64 x = 1\n    bool r = n is x\n",
        );
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP4073),
            "expected ZAP4073 for a reference-vs-value `is`, got {e:?}"
        );
    }

    #[test]
    fn is_chains_like_other_comparisons() {
        // `is`/`is not` chain (Python semantics): `a is b is c` desugars to
        // `(a is b) and (b is c)` — each a reference-identity test — not the
        // left-associative `(a is b) is c` (which would be `bool is c`, an
        // error). That it type-checks proves the chain, not left-folding.
        ok(r#"
struct Node:
    int64 v
def main():
    Node a = Node(v=1)
    Node b = a
    Node c = a
    bool r = a is b is c
"#);
        // A chain mixing identity with `==` on a type that supports both.
        ok(r#"
def main():
    int64 one = 1
    Optional[int64] o = Optional.Some(1)
    bool r = o is not None
    # identity link chained with an equality link on ints
    bool s = 1 == one == 1
"#);
    }

    #[test]
    fn is_on_unknown_or_dynamic_stays_permissive() {
        // A `python` (dynamic) handle has no static identity constraint, so `is`
        // against it is permitted (it lowers to a pointer compare).
        ok(r#"
import python "math" as m
def main():
    auto v = m.pi
    bool r = v is None
"#);
    }

    // ---- `except*` exception-group handling (#605, D-0129) ----------------

    #[test]
    fn except_star_type_checks() {
        // A single `except*` clause, multiple `except*` clauses, `as` binding,
        // and `.exceptions` iteration on the bound group all type-check.
        ok(r#"
def main():
    try:
        raise ValueError("bad")
    except* ValueError as eg:
        for m in eg.exceptions:
            print(m)
"#);
        ok(r#"
def main():
    try:
        raise TimeoutError("t")
    except* TimeoutError as t:
        print(t)
    except* ValueError as v:
        print(v)
"#);
        // `except*` with no binding is fine (the group is handled, not inspected).
        ok(r#"
def main():
    try:
        raise ValueError("bad")
    except* ValueError:
        print("handled")
"#);
        // A user-defined exception type is a valid `except*` target.
        ok(r#"
struct MyError:
    str msg
def main():
    try:
        raise MyError("x")
    except* MyError as eg:
        print(eg)
"#);
        // Nested `try`/`except*`.
        ok(r#"
def main():
    try:
        try:
            raise ValueError("inner")
        except* ValueError as inner:
            print(inner)
    except* TimeoutError as outer:
        print(outer)
"#);
    }

    #[test]
    fn except_star_cannot_mix_with_plain_except() {
        // A `try` is all-`except` or all-`except*` — never a mix (#605, D-0129).
        // The offending clause is flagged with ZAP3001, in either order.
        for src in [
            "def main():\n    try:\n        raise ValueError(\"x\")\n    except ValueError:\n        print(1)\n    except* TimeoutError:\n        print(2)\n",
            "def main():\n    try:\n        raise ValueError(\"x\")\n    except* TimeoutError:\n        print(1)\n    except ValueError:\n        print(2)\n",
        ] {
            let e = errors(src);
            assert!(
                e.iter().any(|x| x.code == ErrorCode::ZAP3001
                    && x.message.contains("cannot mix")),
                "expected ZAP3001 'cannot mix' for {src:?}, got {e:?}"
            );
        }
    }

    #[test]
    fn bare_except_star_is_rejected() {
        // `except*` must name a type; a bare catch-all `except*:` is meaningless
        // (it would peel no particular sub-group) and is rejected (ZAP3001).
        let e = errors(
            "def main():\n    try:\n        raise ValueError(\"x\")\n    except*:\n        print(1)\n",
        );
        assert!(
            e.iter().any(|x| x.code == ErrorCode::ZAP3001
                && x.message.contains("'except*' requires an exception type")),
            "expected ZAP3001 'requires an exception type' for bare except*, got {e:?}"
        );
    }

    #[test]
    fn except_star_unknown_type_is_rejected() {
        // An `except*` naming an unknown type is rejected exactly like `except`.
        let e = errors(
            "def main():\n    try:\n        raise ValueError(\"x\")\n    except* NoSuchError:\n        print(1)\n",
        );
        assert!(
            e.iter()
                .any(|x| x.code == ErrorCode::ZAP3001
                    && x.message.contains("unknown exception type")),
            "expected ZAP3001 'unknown exception type' for except* NoSuchError, got {e:?}"
        );
    }

    #[test]
    fn all_except_star_clauses_are_accepted() {
        // Three `except*` clauses (all-star, the valid form) produce no mixing
        // error — the mixing check fires only when the forms actually differ.
        ok(r#"
def main():
    try:
        raise ValueError("x")
    except* ValueError:
        print(1)
    except* TimeoutError:
        print(2)
    except* KeyError:
        print(3)
"#);
    }

    // ---- #552: unsigned integer types -------------------------------------

    fn has_code(src: &str, code: ErrorCode) -> bool {
        errors(src).iter().any(|e| e.code == code)
    }

    #[test]
    fn unsigned_types_are_resolvable_and_constructible() {
        // Every width + `usize` is a usable scalar type (binding, literal init,
        // param, return), and the conversion functions type-check both ways.
        ok("def main():\n    uint8 a = 0\n    uint16 b = 1\n    uint32 c = 2\n    uint64 d = 3\n    usize e = 4\n    print(a)\n    print(e)\n");
        ok("def widen(uint32 x) -> uint64:\n    return uint64(x)\ndef main():\n    print(widen(uint32(5)))\n");
        // Signed <-> unsigned crossing is possible, but only *explicitly*.
        ok("def main():\n    int64 s = -1\n    uint64 u = uint64(s)\n    int64 back = int64(u)\n    print(u)\n    print(back)\n");
        // Short spellings resolve identically.
        ok("def main():\n    u8 a = 1\n    u16 b = 2\n    u32 c = 3\n    u64 d = 4\n    print(a)\n");
    }

    #[test]
    fn unsigned_tower_widens_implicitly_but_usize_does_not() {
        // uint8 -> uint16 -> uint32 -> uint64 widen implicitly (value-preserving
        // zero-extend), like int32 -> int64.
        ok("def main():\n    uint8 a = 1\n    uint32 c = a\n    uint64 d = c\n    print(d)\n");
        // Narrowing down the tower still needs an explicit cast (D-0020).
        assert!(has_code(
            "def main():\n    uint64 d = 5\n    uint8 a = d\n    print(a)\n",
            ErrorCode::ZAP4001,
        ));
        // `usize` takes part in NO implicit conversion (platform-dependent
        // width): neither to nor from `uint64`, both directions need a cast.
        assert!(has_code(
            "def main():\n    uint64 d = 5\n    usize n = d\n    print(n)\n",
            ErrorCode::ZAP4001,
        ));
        assert!(has_code(
            "def main():\n    usize n = 5\n    uint64 d = n\n    print(d)\n",
            ErrorCode::ZAP4001,
        ));
    }

    #[test]
    fn mixed_signed_unsigned_arithmetic_is_rejected() {
        // Every arithmetic / comparison / bitwise operator rejects a concrete
        // signed + concrete unsigned pairing with ZAP4015 (#552, D-0019). No
        // implicit signed<->unsigned conversion.
        for expr in [
            "s + u", "s - u", "s * u", "s // u", "s % u", "s / u", "s ** u", "u + s", "s & u",
            "s | u", "s ^ u", "s << u", "u >> s", "s == u", "s < u", "s >= u",
        ] {
            let src = format!(
                "def main():\n    int64 s = 1\n    uint64 u = 2\n    auto r = {expr}\n    print(1)\n"
            );
            assert!(
                has_code(&src, ErrorCode::ZAP4015),
                "expected ZAP4015 for `{expr}`, got {:?}",
                errors(&src)
            );
        }
    }

    #[test]
    fn same_sign_and_literal_arithmetic_is_allowed() {
        // Two unsigned operands, or an unsigned operand with an untyped literal,
        // are fine — the literal adapts; no ZAP4015.
        ok("def main():\n    uint64 a = 10\n    uint64 b = 3\n    auto q = a // b\n    auto m = a % b\n    auto s = a + b\n    auto c = a < b\n    print(q)\n    print(m)\n    print(s)\n    print(c)\n");
        ok("def main():\n    uint8 x = 5\n    auto y = x + 1\n    auto z = x * 2\n    auto w = x << 1\n    print(y)\n    print(z)\n    print(w)\n");
        // The all-signed tower is unaffected.
        ok("def main():\n    int64 s = 1\n    int32 t = 2\n    auto r = s + t\n    print(r)\n");
    }

    #[test]
    fn unsigned_division_is_true_division_to_float() {
        // `/` is true division everywhere (D-0024): unsigned `/` yields float.
        ok("def main():\n    uint64 a = 7\n    uint64 b = 2\n    float r = a / b\n    print(r)\n");
    }

    #[test]
    fn unsigned_values_are_printable_and_usable_as_uint8_array_dtype() {
        ok("def main():\n    uint32 x = 4000000000\n    print(x)\n    auto s = str(x)\n    print(s)\n");
        // `array[uint8]` (image data) still type-checks — the dtype path is
        // unchanged by the new scalar `uint8` (#49/#552).
        ok("def main():\n    array[uint8] buf = array[uint8]([1, 2, 3])\n    print(buf[0])\n");
    }
}
