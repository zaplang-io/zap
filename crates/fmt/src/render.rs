//! Pure, single-line renderers for the AST (D-0037).
//!
//! Every function here returns canonical *inline* source for one node, with no
//! line breaks and no comments (the [`crate`] printer owns wrapping + comment
//! interleaving). Rendering is precedence-aware: [`pexpr`] parenthesizes a child
//! exactly when the tree would otherwise re-parse differently, so the output is
//! guaranteed to round-trip to the same AST.

use crate::strings::format_string;
use zapc_syntax::ast::*;

// Precedence ladder (higher binds tighter). Mirrors the parser's binding powers.
const P_LAMBDA: u8 = 0;
const P_TERNARY: u8 = 1;
const P_SEND: u8 = 5;
const P_CMP: u8 = 6;
const P_NOT: u8 = 4;
const P_UNARY: u8 = 13;
const P_POW: u8 = 14;
const P_POSTFIX: u8 = 15;
const P_ATOM: u8 = 16;

/// The binding precedence and associativity of a binary operator.
fn binop_prec(op: BinOp) -> (u8, bool /* right-assoc */) {
    use BinOp::*;
    match op {
        Or => (2, false),
        And => (3, false),
        Send => (P_SEND, false),
        Eq | Ne | Lt | Le | Gt | Ge | In => (P_CMP, false),
        BitOr => (7, false),
        BitXor => (8, false),
        BitAnd => (9, false),
        Shl | Shr => (10, false),
        Add | Sub => (11, false),
        Mul | Div | FloorDiv | Mod => (12, false),
        Pow => (P_POW, true),
    }
}

/// The precedence of an expression as a whole (how tightly it already binds).
fn prec(e: &Expr) -> u8 {
    match e {
        Expr::Binary { op, .. } => binop_prec(*op).0,
        Expr::Unary { op: UnOp::Not, .. } => P_NOT,
        Expr::Unary { .. } | Expr::ChanRecv { .. } => P_UNARY,
        Expr::IfElse { .. } => P_TERNARY,
        Expr::Lambda { .. } => P_LAMBDA,
        Expr::Yield { .. } => P_TERNARY,
        Expr::RegexMatch { .. } | Expr::RegexSubst { .. } => P_CMP,
        Expr::Call { .. } | Expr::Index { .. } | Expr::Slice { .. } | Expr::Attr { .. } => {
            P_POSTFIX
        }
        _ => P_ATOM,
    }
}

/// Render `e`, wrapping it in parentheses iff its precedence is below `min`.
pub fn pexpr(e: &Expr, min: u8) -> String {
    let s = render(e);
    if prec(e) < min {
        format!("({s})")
    } else {
        s
    }
}

/// Render an expression inline (no surrounding parentheses of its own).
pub fn render(e: &Expr) -> String {
    match e {
        Expr::Int(s, _) | Expr::Float(s, _) | Expr::Imaginary(s, _) => s.clone(),
        Expr::Str {
            value,
            raw,
            bytes,
            fstring,
            ..
        } => format_string(value, *raw, *bytes, *fstring),
        Expr::Bool(b, _) => if *b { "True" } else { "False" }.to_string(),
        Expr::None(_) => "None".to_string(),
        Expr::Name(n, _) => n.clone(),

        Expr::Tuple(xs, _) => render_tuple(xs),
        Expr::List(xs, _) => format!("[{}]", render_elems(xs)),
        Expr::Set(xs, _) => format!("{{{}}}", render_elems(xs)),
        Expr::Dict(pairs, _) => {
            if pairs.is_empty() {
                "{}".to_string()
            } else {
                format!("{{{}}}", render_pairs(pairs))
            }
        }

        Expr::ListComp {
            elem,
            var,
            iter,
            cond,
            ..
        } => format!("[{}]", comp_body(&pexpr(elem, P_TERNARY), var, iter, cond)),
        Expr::SetComp {
            elem,
            var,
            iter,
            cond,
            ..
        } => format!(
            "{{{}}}",
            comp_body(&pexpr(elem, P_TERNARY), var, iter, cond)
        ),
        Expr::DictComp {
            key,
            value,
            var,
            iter,
            cond,
            ..
        } => {
            let head = format!("{}: {}", pexpr(key, P_TERNARY), pexpr(value, P_TERNARY));
            format!("{{{}}}", comp_body(&head, var, iter, cond))
        }

        Expr::Unary { op, expr, .. } => match op {
            UnOp::Not => format!("not {}", pexpr(expr, P_NOT)),
            UnOp::Neg => format!("-{}", pexpr(expr, P_UNARY)),
            UnOp::Pos => format!("+{}", pexpr(expr, P_UNARY)),
            UnOp::BitNot => format!("~{}", pexpr(expr, P_UNARY)),
        },
        Expr::Binary { op, lhs, rhs, .. } => {
            let (p, right_assoc) = binop_prec(*op);
            let (lmin, rmin) = if right_assoc { (p + 1, p) } else { (p, p + 1) };
            format!(
                "{} {} {}",
                pexpr(lhs, lmin),
                binop_str(*op),
                pexpr(rhs, rmin)
            )
        }
        Expr::ChanRecv { chan, .. } => format!("<-{}", pexpr(chan, P_UNARY)),
        Expr::Yield { value, .. } => match value {
            Some(v) => format!("yield {}", pexpr(v, P_TERNARY)),
            None => "yield".to_string(),
        },
        Expr::IfElse {
            cond, then, els, ..
        } => format!(
            "{} if {} else {}",
            pexpr(then, 2),
            pexpr(cond, 2),
            pexpr(els, P_TERNARY)
        ),
        Expr::Lambda { params, body, .. } => {
            let ps = if params.is_empty() {
                String::new()
            } else {
                format!(" {}", params.join(", "))
            };
            format!("lambda{ps}: {}", pexpr(body, P_LAMBDA))
        }

        Expr::Call { callee, args, .. } => {
            format!("{}({})", pexpr(callee, P_POSTFIX), render_args(args))
        }
        Expr::Index { base, index, .. } => {
            format!("{}[{}]", pexpr(base, P_POSTFIX), render_index(index))
        }
        Expr::Slice {
            base, lo, hi, step, ..
        } => {
            format!("{}[{}]", pexpr(base, P_POSTFIX), render_slice(lo, hi, step))
        }
        Expr::Attr { base, name, .. } => format!("{}.{name}", pexpr(base, P_POSTFIX)),

        Expr::RegexMatch {
            subject,
            pattern,
            flags,
            negated,
            ..
        } => {
            let op = if *negated { "!~" } else { "=~" };
            format!(
                "{} {op} {}",
                pexpr(subject, P_CMP),
                regex_match_lit(pattern, flags)
            )
        }
        Expr::RegexSubst {
            subject,
            pattern,
            replacement,
            flags,
            ..
        } => format!(
            "{} =~ {}",
            pexpr(subject, P_CMP),
            regex_subst_lit(pattern, replacement, flags)
        ),
    }
}

fn render_elems(xs: &[Expr]) -> String {
    xs.iter()
        .map(|e| pexpr(e, P_LAMBDA))
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_tuple(xs: &[Expr]) -> String {
    if xs.is_empty() {
        "()".to_string()
    } else if xs.len() == 1 {
        format!("({},)", pexpr(&xs[0], P_LAMBDA))
    } else {
        format!("({})", render_elems(xs))
    }
}

fn render_pairs(pairs: &[(Expr, Expr)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}: {}", pexpr(k, P_LAMBDA), pexpr(v, P_LAMBDA)))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn render_args(args: &[Arg]) -> String {
    args.iter().map(arg_str).collect::<Vec<_>>().join(", ")
}

pub fn arg_str(a: &Arg) -> String {
    match &a.name {
        Some(n) => format!("{n}={}", pexpr(&a.value, P_LAMBDA)),
        None => pexpr(&a.value, P_LAMBDA),
    }
}

fn render_index(index: &Expr) -> String {
    // `base[i, j]` is stored as an `Index` over a tuple; render it without the
    // tuple's own parentheses (it re-parses to the same multi-subscript).
    if let Expr::Tuple(elems, _) = index {
        if elems.is_empty() {
            return "()".to_string();
        }
        let inner = render_elems(elems);
        if elems.len() == 1 {
            return format!("{inner},");
        }
        return inner;
    }
    pexpr(index, P_LAMBDA)
}

fn render_slice(
    lo: &Option<Box<Expr>>,
    hi: &Option<Box<Expr>>,
    step: &Option<Box<Expr>>,
) -> String {
    let part = |o: &Option<Box<Expr>>| o.as_ref().map(|e| pexpr(e, P_LAMBDA)).unwrap_or_default();
    let (l, h) = (part(lo), part(hi));
    match step {
        Some(s) => format!("{l}:{h}:{}", pexpr(s, P_LAMBDA)),
        None => format!("{l}:{h}"),
    }
}

fn comp_body(head: &str, var: &str, iter: &Expr, cond: &Option<Box<Expr>>) -> String {
    let base = format!("{head} for {var} in {}", pexpr(iter, 2));
    match cond {
        Some(c) => format!("{base} if {}", pexpr(c, 2)),
        None => base,
    }
}

/// Choose a regex delimiter that does not collide with the pattern text.
pub fn regex_match_lit(pattern: &str, flags: &str) -> String {
    if !pattern.contains('/') {
        format!("m/{pattern}/{flags}")
    } else if !pattern.contains('}') {
        format!("m{{{pattern}}}{flags}")
    } else {
        format!("m/{pattern}/{flags}")
    }
}

pub fn regex_subst_lit(pattern: &str, replacement: &str, flags: &str) -> String {
    let slash_ok = !pattern.contains('/') && !replacement.contains('/');
    let brace_ok = !pattern.contains('}') && !replacement.contains('}');
    if slash_ok {
        format!("s/{pattern}/{replacement}/{flags}")
    } else if brace_ok {
        format!("s{{{pattern}}}{{{replacement}}}{flags}")
    } else {
        format!("s/{pattern}/{replacement}/{flags}")
    }
}

// --- types, params, generics, patterns, targets ---

pub fn type_str(t: &Type) -> String {
    match t {
        Type::Named { name, args, .. } => {
            if args.is_empty() {
                name.clone()
            } else {
                format!(
                    "{name}[{}]",
                    args.iter().map(type_str).collect::<Vec<_>>().join(", ")
                )
            }
        }
        Type::Tuple { elems, .. } => {
            format!(
                "({})",
                elems.iter().map(type_str).collect::<Vec<_>>().join(", ")
            )
        }
        Type::Func { params, ret, .. } => {
            let ps = params.iter().map(type_str).collect::<Vec<_>>().join(", ");
            match ret {
                Some(r) => format!("fn({ps}) -> {}", type_str(r)),
                None => format!("fn({ps})"),
            }
        }
    }
}

pub fn param_str(p: &Param) -> String {
    match &p.ty {
        None => p.name.clone(), // untyped `self`
        Some(ty) => {
            let dots = if p.variadic { "..." } else { "" };
            let default = match &p.default {
                Some(d) => format!(" = {}", pexpr(d, P_LAMBDA)),
                None => String::new(),
            };
            format!("{}{} {}{}", type_str(ty), dots, p.name, default)
        }
    }
}

pub fn generics_str(gs: &[TypeParam]) -> String {
    if gs.is_empty() {
        return String::new();
    }
    let inner = gs
        .iter()
        .map(|g| match &g.bound {
            Some(b) => format!("{}: {}", g.name, type_str(b)),
            None => g.name.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{inner}]")
}

pub fn pattern_str(p: &Pattern) -> String {
    match p {
        Pattern::Wildcard(_) => "_".to_string(),
        Pattern::Capture { name, .. } => name.clone(),
        Pattern::Literal(e) => pexpr(e, P_LAMBDA),
        Pattern::Or(alts, _) => alts.iter().map(pattern_str).collect::<Vec<_>>().join(" | "),
        Pattern::Sequence(pats, _) => {
            format!(
                "[{}]",
                pats.iter().map(pattern_str).collect::<Vec<_>>().join(", ")
            )
        }
        Pattern::Tuple(pats, _) => {
            let inner = pats.iter().map(pattern_str).collect::<Vec<_>>().join(", ");
            if pats.len() == 1 {
                format!("({inner},)")
            } else {
                format!("({inner})")
            }
        }
        Pattern::Struct { path, args, .. } => {
            let p = path.join(".");
            match args {
                Some(a) => format!(
                    "{p}({})",
                    a.iter().map(pat_arg_str).collect::<Vec<_>>().join(", ")
                ),
                None => p,
            }
        }
        Pattern::Rest { name, .. } => match name {
            Some(n) => format!("*{n}"),
            None => "*".to_string(),
        },
    }
}

fn pat_arg_str(pa: &PatArg) -> String {
    match &pa.name {
        Some(n) => format!("{n}={}", pattern_str(&pa.pattern)),
        None => pattern_str(&pa.pattern),
    }
}

/// Render an assignment/`for` target. At the top level a tuple is rendered
/// without its parentheses (`a, b = ...`, `for a, b in ...`), the idiomatic
/// Python form; nested tuples keep their parentheses.
pub fn bind_target_str(bt: &BindTarget, top: bool) -> String {
    match bt {
        BindTarget::Elem(e) => pexpr(e, P_LAMBDA),
        BindTarget::Rest(inner, _) => format!("*{}", bind_target_str(inner, false)),
        BindTarget::Tuple(elems, _) => {
            let inner = elems
                .iter()
                .map(|e| bind_target_str(e, false))
                .collect::<Vec<_>>()
                .join(", ");
            let one = elems.len() == 1;
            if top {
                if one {
                    format!("{inner},")
                } else {
                    inner
                }
            } else if one {
                format!("({inner},)")
            } else {
                format!("({inner})")
            }
        }
    }
}

/// Render a `for`-loop target expression, unwrapping a top-level tuple.
pub fn for_target_str(e: &Expr) -> String {
    if let Expr::Tuple(elems, _) = e {
        if !elems.is_empty() {
            return elems
                .iter()
                .map(|x| pexpr(x, P_LAMBDA))
                .collect::<Vec<_>>()
                .join(", ");
        }
    }
    pexpr(e, P_LAMBDA)
}
