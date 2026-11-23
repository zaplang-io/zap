//! A generic "rebuild an expression with its direct children transformed" walk,
//! shared by the `comptime for` unroller's constant folding (#1251) and the
//! `reflect.field` desugaring (#1252).
//!
//! Leaves clone unchanged. A few rarely-body-relevant forms
//! (dict/set-comprehension/generator/lambda/regex) are cloned whole in this cut
//! — a transform inside one of those is not applied (documented limitation,
//! shared by both callers).

use zapc_syntax::ast::{Arg, Expr};

use crate::CtError;

type Mapper<'f> = dyn FnMut(&Expr) -> Result<Expr, CtError> + 'f;

fn each(es: &[Expr], f: &mut Mapper) -> Result<Vec<Expr>, CtError> {
    es.iter().map(f).collect()
}

fn opt(e: &Option<Box<Expr>>, f: &mut Mapper) -> Result<Option<Box<Expr>>, CtError> {
    match e {
        None => Ok(None),
        Some(inner) => Ok(Some(Box::new(f(inner)?))),
    }
}

fn arg(a: &Arg, f: &mut Mapper) -> Result<Arg, CtError> {
    Ok(Arg {
        name: a.name.clone(),
        value: f(&a.value)?,
        spread: a.spread,
        span: a.span,
    })
}

/// Rebuild `e` with each direct child expression replaced by `f(child)`.
pub(crate) fn map_child_exprs<F>(e: &Expr, f: &mut F) -> Result<Expr, CtError>
where
    F: FnMut(&Expr) -> Result<Expr, CtError>,
{
    Ok(match e {
        Expr::Tuple(xs, s) => Expr::Tuple(each(xs, f)?, *s),
        Expr::List(xs, s) => Expr::List(each(xs, f)?, *s),
        Expr::Set(xs, s) => Expr::Set(each(xs, f)?, *s),
        Expr::Starred { value, span } => Expr::Starred {
            value: Box::new(f(value)?),
            span: *span,
        },
        Expr::Unary { op, expr, span } => Expr::Unary {
            op: *op,
            expr: Box::new(f(expr)?),
            span: *span,
        },
        Expr::Comptime { expr, span } => Expr::Comptime {
            expr: Box::new(f(expr)?),
            span: *span,
        },
        Expr::Binary { op, lhs, rhs, span } => Expr::Binary {
            op: *op,
            lhs: Box::new(f(lhs)?),
            rhs: Box::new(f(rhs)?),
            span: *span,
        },
        Expr::Compare { first, rest, span } => Expr::Compare {
            first: Box::new(f(first)?),
            rest: rest
                .iter()
                .map(|(op, ex)| Ok((*op, f(ex)?)))
                .collect::<Result<Vec<_>, CtError>>()?,
            span: *span,
        },
        Expr::Call { callee, args, span } => Expr::Call {
            callee: Box::new(f(callee)?),
            args: args
                .iter()
                .map(|a| arg(a, f))
                .collect::<Result<Vec<_>, CtError>>()?,
            span: *span,
        },
        Expr::Index { base, index, span } => Expr::Index {
            base: Box::new(f(base)?),
            index: Box::new(f(index)?),
            span: *span,
        },
        Expr::Slice {
            base,
            lo,
            hi,
            step,
            span,
        } => Expr::Slice {
            base: Box::new(f(base)?),
            lo: opt(lo, f)?,
            hi: opt(hi, f)?,
            step: opt(step, f)?,
            span: *span,
        },
        Expr::SliceSpec { lo, hi, step, span } => Expr::SliceSpec {
            lo: opt(lo, f)?,
            hi: opt(hi, f)?,
            step: opt(step, f)?,
            span: *span,
        },
        Expr::Attr { base, name, span } => Expr::Attr {
            base: Box::new(f(base)?),
            name: name.clone(),
            span: *span,
        },
        Expr::IfElse {
            cond,
            then,
            els,
            span,
        } => Expr::IfElse {
            cond: Box::new(f(cond)?),
            then: Box::new(f(then)?),
            els: Box::new(f(els)?),
            span: *span,
        },
        Expr::ChanRecv { chan, span } => Expr::ChanRecv {
            chan: Box::new(f(chan)?),
            span: *span,
        },
        Expr::Yield { value, span } => Expr::Yield {
            value: match value {
                Some(v) => Some(Box::new(f(v)?)),
                None => None,
            },
            span: *span,
        },
        Expr::Named {
            name,
            name_span,
            value,
            span,
        } => Expr::Named {
            name: name.clone(),
            name_span: *name_span,
            value: Box::new(f(value)?),
            span: *span,
        },
        // Leaves and whole-clone forms.
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Imaginary(..)
        | Expr::Str { .. }
        | Expr::Bool(..)
        | Expr::None(_)
        | Expr::Name(..)
        | Expr::Dict(..)
        | Expr::ListComp { .. }
        | Expr::DictComp { .. }
        | Expr::SetComp { .. }
        | Expr::GenExp { .. }
        | Expr::Lambda { .. }
        | Expr::RegexMatch { .. }
        | Expr::RegexSubst { .. } => e.clone(),
    })
}
