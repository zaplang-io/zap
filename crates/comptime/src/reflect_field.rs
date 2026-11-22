//! Typed `reflect.field(value, comptime i)` access (#1252).
//!
//! `reflect.field(value, comptime f.index)` yields a value of the field's
//! **statically-known type** — `int`, `str`, a nested struct, `list[T]`, `T?` —
//! never `any`, with **no boxing**, mirroring the fully-monomorphized/unboxed
//! goal of generics (#154/#164) and mono-recv (#671).
//!
//! # Approach: desugar to ordinary field access
//!
//! The index is a comptime constant (produced by the #1250 evaluator / folded by
//! the #1251 unroller), so `reflect.field(value, <const>)` resolves to the field
//! at that declaration index and **desugars to `value.<name>`** — an ordinary
//! `Expr::Attr` field access. That reuses the existing per-layout field-access
//! lowering, which already compiles for all three struct layouts (#1252's "hard
//! part"):
//!
//! - **value-struct** — inline field read (`StructLayout::inline`, D-0051);
//! - **heap-struct** — pointer + field offset, no copy, identity preserved;
//! - **`@scoped`** — the arena/region field-access discipline (#876, D-0159).
//!
//! So the typed, unboxed, direct field load and the three-layout coverage come
//! from the existing, already-golden `s.field` path — this phase adds the
//! comptime index→name resolution, not a new codegen path. Read access only
//! (v1); field *write* is deferred until a codec consumer (E4/E6) needs it.
//!
//! As with #1250/#1251 there is **no `reflect` parser surface yet**; this is the
//! host-tested resolution primitive the deferred surface and the derive codecs
//! will call.

use zapc_sema_types::TypeInfo;
use zapc_syntax::ast::{Expr, Type};
use zapc_syntax::span::Span;

use crate::walk::map_child_exprs;
use crate::CtError;

/// Resolve `reflect.field(value, index)` for a struct described by `info`:
/// return the typed field access `value.<name>` for the field at declaration
/// `index`. An out-of-range index is a compile error at the comptime site
/// (never a runtime out-of-bounds); a non-struct `info` has no fields and is
/// likewise an error.
pub fn reflect_field(
    value: &Expr,
    index: i64,
    info: &TypeInfo,
    span: Span,
) -> Result<Expr, CtError> {
    let fields = info.fields();
    if index < 0 || index as usize >= fields.len() {
        return Err(CtError::value(
            format!(
                "comptime field index {index} is out of range for `{}` ({} field(s))",
                info.name().unwrap_or("<type>"),
                fields.len()
            ),
            span,
        ));
    }
    Ok(Expr::Attr {
        base: Box::new(value.clone()),
        name: fields[index as usize].name.clone(),
        span,
    })
}

/// Rewrite every `reflect.field(value, <int literal>)` call in `expr` to the
/// typed field access `value.<name>`, using `info` to resolve the index. The
/// index must already be a comptime-constant integer literal (the #1251
/// unroller folds `comptime f.index` to one); a non-constant index is left
/// untouched for a later pass. Recurses through the whole expression.
pub fn desugar_reflect_field(expr: &Expr, info: &TypeInfo) -> Result<Expr, CtError> {
    // Recognize `reflect.field(value, <int>)`.
    if let Expr::Call { callee, args, span } = expr {
        if is_reflect_field(callee) && args.len() == 2 {
            if let Some(index) = const_int(&args[1].value) {
                // Desugar this call; the value operand is itself recursed into
                // first (it may contain further reflect.field calls).
                let value = desugar_reflect_field(&args[0].value, info)?;
                return reflect_field(&value, index, info, *span);
            }
        }
    }
    map_child_exprs(expr, &mut |c| desugar_reflect_field(c, info))
}

/// Is `callee` the `reflect.field` member access?
fn is_reflect_field(callee: &Expr) -> bool {
    matches!(
        callee,
        Expr::Attr { base, name, .. }
            if name == "field" && matches!(&**base, Expr::Name(m, _) if m == "reflect")
    )
}

/// The constant `i64` an integer-literal expression denotes (bare literal, or a
/// unary negation of one), else `None`.
fn const_int(e: &Expr) -> Option<i64> {
    match e {
        Expr::Int(lex, _) => parse_i64(lex),
        Expr::Unary {
            op: zapc_syntax::ast::UnOp::Neg,
            expr,
            ..
        } => match &**expr {
            Expr::Int(lex, _) => parse_i64(lex).map(|n| n.wrapping_neg()),
            _ => None,
        },
        _ => None,
    }
}

fn parse_i64(lex: &str) -> Option<i64> {
    let cleaned: String = lex.chars().filter(|c| *c != '_').collect();
    let (radix, digits) = if let Some(r) = cleaned
        .strip_prefix("0x")
        .or_else(|| cleaned.strip_prefix("0X"))
    {
        (16, r)
    } else if let Some(r) = cleaned
        .strip_prefix("0o")
        .or_else(|| cleaned.strip_prefix("0O"))
    {
        (8, r)
    } else if let Some(r) = cleaned
        .strip_prefix("0b")
        .or_else(|| cleaned.strip_prefix("0B"))
    {
        (2, r)
    } else {
        (10, cleaned.as_str())
    };
    i64::from_str_radix(digits, radix).ok()
}

/// The declared element [`Type`] of a field, for a consumer that needs the
/// field's static type alongside its access expression (e.g. to annotate a
/// generated local). `None` for an out-of-range index.
pub fn field_decl_type(index: i64, info: &TypeInfo) -> Option<Type> {
    let fields = info.fields();
    if index < 0 || index as usize >= fields.len() {
        return None;
    }
    Some(fields[index as usize].decl_ty.clone())
}
