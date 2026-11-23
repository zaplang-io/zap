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

use zapc_comptime::{
    desugar_reflect_field, reflect_field, unroll, Env, Evaluator, TypeInfo, TypeRegistry, Value,
};
use zapc_syntax::ast::{Block, EnumDef, Expr, Module, Stmt, StructDef};
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
    registry: Option<&TypeRegistry>,
) -> Result<Vec<Stmt>, LowerError> {
    let info = zapc_comptime::type_info_of_struct(def);
    unroll_member(info, loop_var, "fields", body, funcs, registry)
}

/// Unroll `comptime for v in info.variants:` over the concrete enum `def`'s
/// variants.
pub fn unroll_enum_variants(
    def: &EnumDef,
    loop_var: &str,
    body: &Block,
    funcs: &[zapc_syntax::ast::Function],
    registry: Option<&TypeRegistry>,
) -> Result<Vec<Stmt>, LowerError> {
    let info = zapc_comptime::type_info_of_enum(def);
    unroll_member(info, loop_var, "variants", body, funcs, registry)
}

/// Build the comptime [`TypeRegistry`] for a module's user `struct`/`enum`
/// declarations (#1253): the no-sema resolver that drives `type_info(T).kind`-
/// switch container recursion. Mono attaches this to the evaluator (via
/// `Evaluator::with_registry`) so `info.elem` / `info.key` / `info.value` /
/// `info.inner` / `info.elems` and `field.ty` resolve a container's element /
/// a field's type back to a nested `TypeInfo`. Reflect the **post-substitution**
/// module so the registry holds the concrete instantiations (#154/#164).
pub fn type_registry(module: &Module) -> TypeRegistry {
    TypeRegistry::from_module(module)
}

/// Typed `reflect.field(value, index)` for the concrete struct `def` (#1252):
/// the comptime `index` resolves to the field at that declaration position and
/// becomes the ordinary typed field access `value.<name>`, which the existing
/// per-layout field-access lowering (value-struct / heap-struct / `@scoped`)
/// compiles unchanged — a direct, unboxed field load, not an `any`. An
/// out-of-range index is a compile error here, never a runtime out-of-bounds.
/// Read access (v1); field *write* is deferred until a codec consumer needs it.
pub fn reflect_field_access(def: &StructDef, value: &Expr, index: i64) -> Result<Expr, LowerError> {
    let info = zapc_comptime::type_info_of_struct(def);
    reflect_field(value, index, &info, Span::new(0, 0)).map_err(|e| err(e.message))
}

/// Unroll `comptime for <loop_var> in info.fields:` over `def` **and** desugar
/// every `reflect.field(value, f.index)` in the emitted body to its typed field
/// access — the combined #1251 + #1252 pipeline a derive codec drives. Each
/// emitted statement's expression is desugared (the forms a comptime-for body
/// uses to emit per-field code: expression statements, `return`, `let`); the
/// full statement-position wiring lands with the parser surface.
pub fn unroll_struct_fields_with_access(
    def: &StructDef,
    loop_var: &str,
    body: &Block,
    funcs: &[zapc_syntax::ast::Function],
    registry: Option<&TypeRegistry>,
) -> Result<Vec<Stmt>, LowerError> {
    let info = zapc_comptime::type_info_of_struct(def);
    let stmts = unroll_struct_fields(def, loop_var, body, funcs, registry)?;
    stmts
        .iter()
        .map(|st| desugar_stmt_fields(st, &info))
        .collect()
}

/// Desugar `reflect.field` in the expression(s) of one emitted statement.
fn desugar_stmt_fields(st: &Stmt, info: &TypeInfo) -> Result<Stmt, LowerError> {
    let d = |e: &Expr| desugar_reflect_field(e, info).map_err(|e| err(e.message));
    Ok(match st {
        Stmt::Expr(e) => Stmt::Expr(d(e)?),
        Stmt::Return { value, span } => Stmt::Return {
            value: match value {
                Some(e) => Some(d(e)?),
                None => None,
            },
            span: *span,
        },
        Stmt::Let {
            is_const,
            ty,
            name,
            value,
            span,
        } => Stmt::Let {
            is_const: *is_const,
            ty: ty.clone(),
            name: name.clone(),
            value: d(value)?,
            span: *span,
        },
        other => other.clone(),
    })
}

/// Shared driver: bind `info` to the reflected type, build the `info.<member>`
/// iterable, and run the engine.
fn unroll_member(
    info: TypeInfo,
    loop_var: &str,
    member: &str,
    body: &Block,
    funcs: &[zapc_syntax::ast::Function],
    registry: Option<&TypeRegistry>,
) -> Result<Vec<Stmt>, LowerError> {
    let z = Span::new(0, 0);
    let mut ev = Evaluator::with_functions(funcs);
    if let Some(reg) = registry {
        ev = ev.with_registry(reg);
    }
    let mut base = Env::new();
    base.define("info", Value::Type(Rc::new(info)));
    let iterable = Expr::Attr {
        base: Box::new(Expr::Name("info".to_string(), z)),
        name: member.to_string(),
        span: z,
    };
    unroll(loop_var, &iterable, body, &base, &mut ev).map_err(|e| err(e.message))
}
