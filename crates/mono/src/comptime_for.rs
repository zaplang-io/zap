//! `comptime for` unroll, wired to monomorphization (#1251).
//!
//! The unroll *engine* lives in `zapc_comptime` (a leaf over `zapc_syntax` +
//! `zapc_sema_types`, no sema `Checker`, no LLVM). This module is the mono-side
//! entry: by the time mono specializes a `comptime for` the concrete
//! `struct`/`enum` declaration is in hand as plain AST, so mono reflects its
//! shape directly ([`zapc_comptime::type_info_of_struct`] /
//! `type_info_of_enum` — the no-sema reflector, #1249's generator/verification
//! boundary) and hands the body to the engine, which clones+specializes it once
//! per field/variant (HQ3: an outlineable call per iteration, never a forced
//! inline).
//!
//! Ordering: callers must reflect the **post-substitution** concrete
//! declaration, so `info.fields` reflects the actual instantiation (#154/#164).
//! The engine evaluates the loop over `info.<member>` with `info` bound to the
//! reflected type.
//!
//! The parser `comptime for` surface and the auto-detection in `rewrite_stmt`
//! that calls this are the deferred follow-up; this is the engine-first wiring
//! that reflects a concrete declaration and unrolls, host-tested over
//! constructed AST exactly as E1/#1250 shipped.

use std::rc::Rc;

use zapc_comptime::{unroll, Env, Evaluator, TypeInfo, Value};
use zapc_syntax::ast::{Block, EnumDef, Expr, Stmt, StructDef};
use zapc_syntax::span::Span;

use crate::{err, LowerError};

/// Unroll `comptime for <loop_var> in info.fields:` over the concrete struct
/// `def`'s declared fields, returning the specialized statements to splice.
/// `funcs` are the user functions the body may call at comptime (empty for the
/// common field-name/index case; non-empty for the HQ9 `rename_all` path).
pub fn unroll_struct_fields(
    def: &StructDef,
    loop_var: &str,
    body: &Block,
    funcs: &[zapc_syntax::ast::Function],
) -> Result<Vec<Stmt>, LowerError> {
    let info = zapc_comptime::type_info_of_struct(def);
    unroll_member(info, loop_var, "fields", body, funcs)
}

/// Unroll `comptime for v in info.variants:` over the concrete enum `def`'s
/// variants.
pub fn unroll_enum_variants(
    def: &EnumDef,
    loop_var: &str,
    body: &Block,
    funcs: &[zapc_syntax::ast::Function],
) -> Result<Vec<Stmt>, LowerError> {
    let info = zapc_comptime::type_info_of_enum(def);
    unroll_member(info, loop_var, "variants", body, funcs)
}

/// Shared driver: bind `info` to the reflected type, build the `info.<member>`
/// iterable, and run the engine.
fn unroll_member(
    info: TypeInfo,
    loop_var: &str,
    member: &str,
    body: &Block,
    funcs: &[zapc_syntax::ast::Function],
) -> Result<Vec<Stmt>, LowerError> {
    let z = Span::new(0, 0);
    let mut ev = Evaluator::with_functions(funcs);
    let mut base = Env::new();
    base.define("info", Value::Type(Rc::new(info)));
    let iterable = Expr::Attr {
        base: Box::new(Expr::Name("info".to_string(), z)),
        name: member.to_string(),
        span: z,
    };
    unroll(loop_var, &iterable, body, &base, &mut ev).map_err(|e| err(e.message))
}
