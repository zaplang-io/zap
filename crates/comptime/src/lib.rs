//! Zap compile-time evaluation engine — the Tier-B1 tree-walking evaluator
//! (#1250, epic #1249, phase E2 of the compile-time reflection proposal #1248 /
//! Discussion #1243).
//!
//! # What this is
//!
//! A **pure, deterministic, host-side** interpreter over a restricted subset of
//! Zap — "the comptime fragment". It is the greenfield core E2 needs: nothing in
//! sema/ir/mono evaluates expressions today (mono *substitutes types and
//! rewrites references*, it never computes), and the REPL/JIT runs via LLVM
//! codegen, not a host interpreter. So this crate is a new tree-walking
//! evaluator with **no LLVM dependency**, usable later by monomorphization
//! (`crates/mono`) — which is itself a leaf over `zapc_syntax` + `zapc_sema_types`
//! and does **not** depend on the sema `Checker` — to fold `comptime` code during
//! per-instantiation specialization (#1251 wires that; this issue is just the
//! value model + evaluator).
//!
//! # The fragment (Tier B1)
//!
//! Value model ([`Value`]): `int64`, `bool`, `str`, `char`; `list`/`tuple` of
//! comptime values; and the read-only **E1 reflection records**
//! (`TypeInfo`/`FieldInfo`/`VariantInfo`/`AttrInfo`/`AttrArg`/`TypeKind`) reused
//! from [`zapc_sema_types`]. Floats and bigint are deferred to B2 (#946 / #1084).
//!
//! Evaluable constructs: `let` bindings (lexically scoped, shadowing);
//! `comptime if`; field-access-by-index on the reflection records; arithmetic,
//! comparison, boolean and string ops (concat/case/slice); and **non-recursive
//! calls to user functions reducible within the fragment** — the HQ9 unblock, so
//! a library can validate attribute meaning and compute `rename_all` transforms
//! at compile time rather than being limited to fixed primitives.
//!
//! # The soundness backstop
//!
//! Everything else — I/O, FFI, `spawn`/channels, global mutation, time/RNG,
//! dict/set iteration (#1066, unspecified order), recursion,
//! `match`-over-comptime-values, and float/bigint/`bytes` values — is **out of
//! the fragment**. The evaluator is the physical backstop (epic #1249, locked
//! decision 2b): it simply has **no impure primitive**, so reaching one is a
//! clean span-attributed [`CtError`] carrying the exact phrase `not available at
//! comptime`, never a panic. The static early-reject lint and the shared leaf
//! table are a *separate* issue (#1254); this crate only makes the evaluator fail
//! cleanly.
//!
//! # Numeric parity
//!
//! `int64` `+`/`-`/`*`, `//`/`%`, shifts and comparisons produce **exactly** the
//! runtime bits (two's-complement wrap, floor division/modulo, the `INT64_MIN`
//! edge cases); see [`ops`], whose implementations mirror the runtime C helpers
//! line for line. B1 restricts arithmetic to `int64`/`bool`/`str`/`char` to keep
//! that parity surface small.
//!
//! # Surface
//!
//! There is deliberately **no `comptime` keyword in the parser yet** — the
//! user-facing `comptime for` / `comptime <binding>` surface is wired by #1251,
//! exactly as E1 (#1257) shipped its model with no Zap syntax. This crate is
//! driven host-side over ordinary parsed/constructed AST: register the
//! [`Function`]s that may be called, seed an [`Env`] with any reflection-record
//! inputs, and evaluate an expression or call a function.
//!
//! ```
//! use zapc_comptime::{Env, Evaluator, Value};
//! use zapc_syntax::ast::Expr;
//! use zapc_syntax::span::Span;
//!
//! // `2 * 3 + 1` built directly as AST.
//! let two = Expr::Int("2".into(), Span::new(0, 1));
//! let three = Expr::Int("3".into(), Span::new(4, 5));
//! let one = Expr::Int("1".into(), Span::new(8, 9));
//! use zapc_syntax::ast::BinOp;
//! let prod = Expr::Binary {
//!     op: BinOp::Mul,
//!     lhs: Box::new(two),
//!     rhs: Box::new(three),
//!     span: Span::new(0, 5),
//! };
//! let sum = Expr::Binary {
//!     op: BinOp::Add,
//!     lhs: Box::new(prod),
//!     rhs: Box::new(one),
//!     span: Span::new(0, 9),
//! };
//!
//! let mut ev = Evaluator::new();
//! let mut env = Env::new();
//! assert_eq!(ev.eval(&sum, &mut env).unwrap(), Value::Int(7));
//! ```

mod eval;
mod ops;
mod reflect_ast;
mod reflect_field;
mod unroll;
mod value;
mod walk;

pub mod error;

use std::collections::HashMap;

use zapc_syntax::ast::{Expr, Function};
use zapc_syntax::span::Span;

pub use error::{CtError, CtErrorKind};
pub use reflect_ast::{type_info_of_enum, type_info_of_struct, TypeRegistry};
pub use reflect_field::{desugar_reflect_field, field_decl_type, reflect_field};
pub use unroll::{materialize, unroll};
pub use value::Value;

// Re-export the E1 reflection records so a consumer can build comptime inputs
// without a second dependency edge.
pub use zapc_sema_types::{
    AttrArg, AttrInfo, FieldInfo, TypeInfo, TypeKind, TypeShape, VariantInfo,
};

/// The default ceiling on evaluation steps (one per expression/statement
/// visited). A *fixed hard limit* with a clear diagnostic, not a tunable quota
/// (epic #1249, HQ5): a runaway comptime computation fails fast instead of
/// hanging the compiler.
pub const DEFAULT_STEP_CAP: u64 = 1_000_000;

/// The default ceiling on nested comptime call depth. Recursion is rejected
/// outright (out of fragment), so this only bounds a deeply-but-finitely nested
/// non-recursive call chain.
pub const DEFAULT_DEPTH_CAP: usize = 128;

/// A lexically-scoped binding environment for comptime `let`s and function
/// parameters.
///
/// Scopes nest: a `comptime if` branch body and each called function get their
/// own scope, so an inner `let` shadows an outer binding and is discarded when
/// the scope ends. A redefinition within one scope simply shadows the earlier
/// binding (the evaluator is not the type-checker — sema rejects illegal
/// redeclarations before this runs).
#[derive(Clone, Debug, Default)]
pub struct Env {
    scopes: Vec<HashMap<String, Value>>,
}

impl Env {
    /// A fresh environment with a single empty outermost scope.
    pub fn new() -> Env {
        Env {
            scopes: vec![HashMap::new()],
        }
    }

    /// Bind `name` to `value` in the current (innermost) scope, shadowing any
    /// outer binding of the same name.
    pub fn define(&mut self, name: &str, value: Value) {
        self.scopes
            .last_mut()
            .expect("environment always has at least one scope")
            .insert(name.to_string(), value);
    }

    /// Look up `name`, searching from the innermost scope outward.
    pub fn lookup(&self, name: &str) -> Option<&Value> {
        self.scopes.iter().rev().find_map(|scope| scope.get(name))
    }

    /// Enter a new nested scope.
    pub(crate) fn push(&mut self) {
        self.scopes.push(HashMap::new());
    }

    /// Leave the innermost scope.
    pub(crate) fn pop(&mut self) {
        self.scopes.pop();
    }
}

/// The result of evaluating a statement/block: either fall through, or a
/// `return` that unwinds to the enclosing function call.
pub(crate) enum Flow {
    /// Continue with the next statement.
    Normal,
    /// `return <value>` — unwind to the call boundary with this value.
    Return(Value),
}

/// The comptime tree-walking evaluator (#1250).
///
/// Holds the set of user [`Function`]s that comptime code may call (by name),
/// the step/recursion budget, and the active call stack (for recursion
/// detection). It borrows the functions for its lifetime, so register the whole
/// module's functions once and evaluate many expressions against them.
pub struct Evaluator<'a> {
    funcs: HashMap<&'a str, &'a Function>,
    steps: u64,
    step_cap: u64,
    depth_cap: usize,
    /// Names of the functions currently on the comptime call stack, for
    /// recursion detection (direct or transitive self-calls are out of
    /// fragment).
    active: Vec<&'a str>,
    /// The user-type registry (#1253): resolves a container's element/key/value
    /// type and a field's declared type back to a nested `TypeInfo`, driving the
    /// `type_info(T).kind`-switch container recursion. `None` means container
    /// element accessors (`.elem`/`.key`/…) and `field.ty` are unavailable
    /// (they report the out-of-fragment error as in #1250).
    registry: Option<&'a TypeRegistry>,
}

impl<'a> Default for Evaluator<'a> {
    fn default() -> Self {
        Evaluator::new()
    }
}

impl<'a> Evaluator<'a> {
    /// A new evaluator with no callable functions and the default budget.
    pub fn new() -> Evaluator<'a> {
        Evaluator {
            funcs: HashMap::new(),
            steps: 0,
            step_cap: DEFAULT_STEP_CAP,
            depth_cap: DEFAULT_DEPTH_CAP,
            active: Vec::new(),
            registry: None,
        }
    }

    /// Attach a [`TypeRegistry`] so container recursion (`.elem`/`.key`/
    /// `.value`/`.inner`/`.elems`), `field.ty`, and the `reflect.has_override`
    /// hook resolve user types (#1253).
    pub fn with_registry(mut self, registry: &'a TypeRegistry) -> Self {
        self.registry = Some(registry);
        self
    }

    /// An evaluator that can call any of `funcs` by name (the common case:
    /// register a module's free functions). A later duplicate name shadows an
    /// earlier one.
    pub fn with_functions(funcs: &'a [Function]) -> Evaluator<'a> {
        let mut ev = Evaluator::new();
        for f in funcs {
            ev.register(f);
        }
        ev
    }

    /// Make `func` callable by its name from comptime code.
    pub fn register(&mut self, func: &'a Function) {
        self.funcs.insert(func.name.as_str(), func);
    }

    /// Override the step budget (default [`DEFAULT_STEP_CAP`]).
    pub fn with_step_cap(mut self, cap: u64) -> Self {
        self.step_cap = cap;
        self
    }

    /// Override the recursion/call-depth budget (default [`DEFAULT_DEPTH_CAP`]).
    pub fn with_depth_cap(mut self, cap: usize) -> Self {
        self.depth_cap = cap;
        self
    }

    /// Evaluate a single comptime expression in `env`.
    pub fn eval(&mut self, expr: &Expr, env: &mut Env) -> Result<Value, CtError> {
        self.eval_expr(expr, env)
    }

    /// Call a registered user function by name with already-evaluated
    /// positional `args`, returning its comptime value. `span` anchors any error
    /// to the call site.
    pub fn call(&mut self, name: &str, args: Vec<Value>, span: Span) -> Result<Value, CtError> {
        self.call_fn(name, args, span)
    }
}

/// Bind the seven `TypeKind` names (`Struct`, `Enum`, `Tuple`, `Primitive`,
/// `List`, `Dict`, `Optional`) as comptime [`Value::Kind`] constants in `env`,
/// so a comptime kind-switch can be written `comptime if info.kind == Struct:`
/// (#1253). This seeds the names the deferred `reflect`/`comptime` surface will
/// expose as a `Kind` enum; host-side the driver seeds them here.
pub fn seed_kind_constants(env: &mut Env) {
    for (name, kind) in [
        ("Struct", TypeKind::Struct),
        ("Enum", TypeKind::Enum),
        ("Tuple", TypeKind::Tuple),
        ("Primitive", TypeKind::Primitive),
        ("List", TypeKind::List),
        ("Dict", TypeKind::Dict),
        ("Optional", TypeKind::Optional),
    ] {
        env.define(name, Value::Kind(kind));
    }
}
