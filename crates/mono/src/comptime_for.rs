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

use std::collections::HashMap;

use zapc_comptime::{
    desugar_reflect_field, error_code, reflect_field, seed_kind_constants, specialize_one,
    type_info_of_enum, unroll, CtError, CtErrorKind, Env, Evaluator, TypeInfo, TypeRegistry, Value,
};
use zapc_syntax::ast::{Block, EnumDef, Expr, Function, MatchArm, Module, Stmt, StructDef, Type};
use zapc_syntax::span::Span;

use crate::{DeriveErrorInfo, LowerError};

/// Convert a comptime [`CtError`] raised while unrolling a `@derive` generator
/// into a [`LowerError`] carrying a [`DeriveErrorInfo`] (E3d, #1293). The trait
/// and type are not known here (the unroll sees only the generator body), so
/// `trait_and_type` is `None` and the backtrace is empty; monomorphization
/// upgrades both once it knows which derive this descent belongs to. The code is
/// routed by the cause kind — a library `comptime.error` is `ZAP7002`, an engine
/// fault `ZAP7001` — so a plain (non-derive) comptime failure still renders with
/// the right code and span.
fn ct_lower_error(ce: CtError) -> LowerError {
    let cause_is_user = ce.kind == CtErrorKind::UserError;
    LowerError {
        code: error_code(&ce),
        message: ce.message.clone(),
        derive: Some(Box::new(DeriveErrorInfo {
            trait_and_type: None,
            cause_message: ce.message,
            cause_span: ce.span,
            cause_is_user,
            instantiation_span: None,
            frames: Vec::new(),
        })),
    }
}

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

/// Specialize a `case comptime v in …variants [as _]:` arm body once per
/// variant of the concrete enum `def` (#1374, epic #1371). Returns, in
/// declaration order, each variant's specialized body [`Block`]: `loop_var` is
/// bound to that variant's reflect record, so `comptime <v>.name` and any inner
/// `comptime for f in <v>.fields:` resolve/unroll, while runtime constructs —
/// including `reflect.field(payload, i)` — are left intact for the caller to
/// rewrite to the arm's payload bindings. The returned Vec is parallel to
/// `def.variants`.
pub fn specialize_variant_arms(
    def: &EnumDef,
    loop_var: &str,
    payload: Option<&str>,
    body: &Block,
    funcs: &[Function],
    registry: Option<&TypeRegistry>,
) -> Result<Vec<Block>, LowerError> {
    let info = type_info_of_enum(def);
    let mut ev = Evaluator::with_functions(funcs);
    if let Some(reg) = registry {
        ev = ev.with_registry(reg);
    }
    let base = Env::new();
    let mut out = Vec::with_capacity(info.variants().len());
    for variant in info.variants() {
        let item = Value::Variant(Rc::new(variant.clone()));
        out.push(
            specialize_one(loop_var, item, payload, body, &base, &mut ev)
                .map_err(ct_lower_error)?,
        );
    }
    Ok(out)
}

/// Select the taken branch of a generator's `if` when its condition chain is a
/// **comptime constant** (#1394) — a `reflect.type_info(T).kind`-based kind
/// dispatch, evaluable because `type_subst` resolves the type parameter to the
/// concrete instantiation. Returns the statements of the first branch whose
/// condition is comptime-`true` (or the `else` branch when all are comptime-
/// `false`), so mono can splice just that branch and drop the dead ones. Returns
/// `None` the moment a condition is **not** a comptime-constant bool — then the
/// whole `if` stays an ordinary runtime `if` (never partially pruned).
pub fn select_comptime_if<'b>(
    branches: &'b [(Expr, Block)],
    else_block: &'b Option<Block>,
    registry: &TypeRegistry,
    type_subst: HashMap<String, Type>,
) -> Result<Option<&'b [Stmt]>, LowerError> {
    let mut ev = Evaluator::new()
        .with_registry(registry)
        .with_type_subst(type_subst);
    let mut env = Env::new();
    seed_kind_constants(&mut env);
    for (cond, body) in branches {
        match ev.eval(cond, &mut env) {
            Ok(Value::Bool(true)) => return Ok(Some(&body.stmts)),
            Ok(Value::Bool(false)) => continue,
            // A non-comptime (runtime) condition, or a non-bool: do not prune.
            _ => return Ok(None),
        }
    }
    Ok(Some(
        else_block
            .as_ref()
            .map(|b| b.stmts.as_slice())
            .unwrap_or(&[]),
    ))
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
    reflect_field(value, index, &info, Span::new(0, 0)).map_err(ct_lower_error)
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
    let d = |e: &Expr| desugar_reflect_field(e, info).map_err(ct_lower_error);
    let db = |b: &Block| desugar_block_fields(b, info);
    // `reflect.set_field(target, comptime i, v)` → `target.<name_i> = v` (#1454):
    // the field-write counterpart of `reflect.field`, resolved here where the
    // concrete field name is known (e.g. a codec decoder populating a built value
    // inside a `comptime for` over the fields).
    if let Some(assign) = zapc_comptime::desugar_set_field(st, info).map_err(ct_lower_error)? {
        return Ok(assign);
    }
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
        // Accumulation forms — `acc = acc + reflect.field(...)`,
        // `acc += reflect.field(...)` — must desugar their operand(s) too
        // (#1374/#1375: a codec that folds over fields/payload uses these).
        Stmt::Assign {
            target,
            value,
            span,
        } => Stmt::Assign {
            target: d(target)?,
            value: d(value)?,
            span: *span,
        },
        Stmt::AugAssign {
            target,
            op,
            value,
            span,
        } => Stmt::AugAssign {
            target: d(target)?,
            op: *op,
            value: d(value)?,
            span: *span,
        },
        Stmt::AssignChain {
            targets,
            value,
            span,
        } => Stmt::AssignChain {
            targets: targets.iter().map(&d).collect::<Result<_, _>>()?,
            value: d(value)?,
            span: *span,
        },
        // Runtime control flow in a generator body: desugar conditions/operands
        // and recurse into the nested blocks.
        Stmt::If {
            branches,
            else_block,
            span,
        } => Stmt::If {
            branches: branches
                .iter()
                .map(|(c, b)| Ok((d(c)?, db(b)?)))
                .collect::<Result<_, LowerError>>()?,
            else_block: match else_block {
                Some(b) => Some(db(b)?),
                None => None,
            },
            span: *span,
        },
        Stmt::While { cond, body, span } => Stmt::While {
            cond: d(cond)?,
            body: db(body)?,
            span: *span,
        },
        Stmt::For {
            target,
            iter,
            body,
            span,
        } => Stmt::For {
            target: target.clone(),
            iter: d(iter)?,
            body: db(body)?,
            span: *span,
        },
        // `match <scrutinee>: case <pat> [if <guard>]: <body>` — a codec decoder
        // inspects the DOM with a `match` and populates fields from within the
        // arms (#1506). Desugar the scrutinee and each guard, and recurse into
        // every arm body so a `reflect.field` / `reflect.set_field` inside an arm
        // is rewritten too (the body block runs back through this walker, so an
        // arm-local `reflect.set_field` is desugared by `desugar_set_field` there).
        // Patterns bind names only — no reflect call to rewrite — so they are kept
        // as-is.
        Stmt::Match {
            scrutinee,
            arms,
            span,
        } => Stmt::Match {
            scrutinee: d(scrutinee)?,
            arms: arms
                .iter()
                .map(|a| {
                    Ok(MatchArm {
                        pattern: a.pattern.clone(),
                        guard: match &a.guard {
                            Some(g) => Some(d(g)?),
                            None => None,
                        },
                        body: db(&a.body)?,
                        span: a.span,
                    })
                })
                .collect::<Result<_, LowerError>>()?,
            span: *span,
        },
        other => other.clone(),
    })
}

/// Desugar `reflect.field` in every statement of a block (#1374/#1375).
fn desugar_block_fields(b: &Block, info: &TypeInfo) -> Result<Block, LowerError> {
    Ok(Block {
        stmts: b
            .stmts
            .iter()
            .map(|s| desugar_stmt_fields(s, info))
            .collect::<Result<_, _>>()?,
        span: b.span,
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
    base.define("info", Value::Type(Rc::new(info), None));
    let iterable = Expr::Attr {
        base: Box::new(Expr::Name("info".to_string(), z)),
        name: member.to_string(),
        span: z,
    };
    unroll(loop_var, &iterable, body, &base, &mut ev).map_err(ct_lower_error)
}
